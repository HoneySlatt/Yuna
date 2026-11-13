//! Measures what an agent's calls cost in `yuna mcp`: the figures M1.5 asks for (`docs/PLAN.md`).
//!
//! It starts the `yuna` program built beside it as `yuna mcp`, creates a seat with one output
//! (by default Yuna's own, 3840 × 2160 at 240 Hz and scale 1.5), and reads:
//!
//! 1. An MCP call's time, at rest: `clock now`, `windows`, `pointer move` and `keyboard get`,
//!    each called again and again, timed from the request written to its reply read, as an agent
//!    waits for it.
//! 2. With two `vkcube` started through `spawn`, covering the output and drawing without pause,
//!    after a warm-up: the same `clock now` calls under that load; then a window with no
//!    capture, and a window of captures of the whole output at the logical size, one after the
//!    other. For each window, the frames drawn and those presented after the boundary they aimed
//!    at, *missed* (ADR-0031), from Yuna's `frame drawn` lines; for the captures, each call's
//!    time, and from Yuna's log, the copy of its pixels on the main thread (`capture copied`)
//!    and the encoding of its PNG on a thread of its own (`capture encoded`).
//! 3. A few captures at the physical size, which may pass the 5 MB an image's base64 may have.
//!
//! Build it and Yuna for release, then run it in the development shell, alone among
//! measurements; it needs no screen of its own:
//!
//! ```sh
//! nix develop -c cargo build --release -p yuna --bin yuna --example agent_time
//! nix develop -c target/release/examples/agent_time
//! ```
//!
//! `--gpu` draws on the GPU's render node (`yuna --gpu`); `--client-vulkan <icd.json>` gives
//! `vkcube` another Vulkan driver than lavapipe; `--output <mode>` takes `--output`'s form;
//! `--calls <n>`, 1,000 by default, the calls of each kind; `--seconds <n>`, 10 by default, each
//! window. It stops what it started when it ends, and keeps its folder of logs when it fails.

use std::fs::{self, DirBuilder, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long the clients draw before anything is measured.
const WARM_UP: Duration = Duration::from_secs(3);
/// The longest Yuna may take to answer a call, its clients to show their windows.
const PATIENCE: Duration = Duration::from_secs(60);
/// The captures at the physical size.
const PHYSICAL_CAPTURES: usize = 5;

/// What the command line asks for.
struct Options {
    gpu: bool,
    client_vulkan: Option<String>,
    output: String,
    calls: usize,
    seconds: u64,
}

fn main() -> ExitCode {
    let options = match read_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(reason) => {
            eprintln!("agent_time: {reason}");
            return ExitCode::from(2);
        }
    };
    let run = match Run::new() {
        Ok(run) => run,
        Err(reason) => {
            eprintln!("agent_time: {reason}");
            return ExitCode::FAILURE;
        }
    };
    match measure(&run, &options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("agent_time: {reason}; logs kept in {}", run.keep());
            ExitCode::FAILURE
        }
    }
}

fn read_options(arguments: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        gpu: false,
        client_vulkan: None,
        output: "3840x2160@240,1.5".to_owned(),
        calls: 1_000,
        seconds: 10,
    };
    let mut arguments = arguments;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        let whole = |value: String, what: &str| {
            value
                .parse::<u64>()
                .ok()
                .filter(|&n| n > 0)
                .ok_or(format!("{what} takes a whole number above 0"))
        };
        match argument.as_str() {
            "--gpu" => options.gpu = true,
            "--client-vulkan" => options.client_vulkan = Some(value()?),
            "--output" => options.output = value()?,
            "--calls" => options.calls = whole(value()?, "--calls")? as usize,
            "--seconds" => options.seconds = whole(value()?, "--seconds")?,
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }
    Ok(options)
}

fn measure(run: &Run, options: &Options) -> Result<(), String> {
    let mut yuna = run.start_yuna(options)?;
    println!(
        "agent_time: yuna mcp, output {}, {}",
        options.output,
        if options.gpu { "--gpu" } else { "llvmpipe" }
    );
    let seat = yuna.call("seat", json!({"action": "create"}))?["structuredContent"]["seat"]
        .as_u64()
        .ok_or("no seat")?;

    println!(
        "\nAn MCP call's time at rest, {} calls each:",
        options.calls
    );
    let tools = [
        ("clock now", "clock", json!({"action": "now"})),
        ("windows", "windows", json!({"seat": seat})),
        (
            "pointer move",
            "pointer",
            json!({"action": "move", "seat": seat, "x": 100, "y": 100}),
        ),
        (
            "keyboard get",
            "keyboard",
            json!({"action": "get", "seat": seat}),
        ),
    ];
    for (name, tool, arguments) in &tools {
        let times = yuna.time_calls(tool, arguments, options.calls)?;
        println!("  {name:<13} {}", spread(&times));
    }

    for n in 0..2 {
        yuna.call(
            "spawn",
            json!({"action": "start", "seat": seat, "command": ["vkcube", "--wsi", "wayland"],
                   "env": vkcube_environment(run, n, options)?}),
        )?;
    }
    let deadline = Instant::now() + PATIENCE;
    loop {
        let listed = yuna.call("windows", json!({"seat": seat}))?;
        let shown = listed["structuredContent"]["windows"]
            .as_array()
            .map_or(0, |windows| {
                windows.iter().filter(|w| !w["rect"].is_null()).count()
            });
        if shown == 2 {
            break;
        }
        if Instant::now() > deadline {
            return Err("the two vkcube windows never showed".to_owned());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(WARM_UP);

    let times = yuna.time_calls("clock", &json!({"action": "now"}), options.calls)?;
    println!(
        "\nWith two vkcube drawing:\n  clock now     {}",
        spread(&times)
    );

    let window = Duration::from_secs(options.seconds);
    let start = yuna.log_length()?;
    std::thread::sleep(window);
    let quiet = yuna.log_since(start)?;
    println!(
        "  {} s without captures: {}",
        options.seconds,
        frames(&quiet)
    );

    let start = yuna.log_length()?;
    let began = Instant::now();
    let mut capture_times = Vec::new();
    while began.elapsed() < window {
        let asked = Instant::now();
        yuna.call("capture", json!({"seat": seat}))?;
        capture_times.push(asked.elapsed().as_micros() as u64);
    }
    let captured = yuna.log_since(start)?;
    println!(
        "  {} s of captures one after the other, {} captures: {}",
        options.seconds,
        capture_times.len(),
        frames(&captured)
    );
    println!("    a capture's call   {}", spread(&capture_times));
    println!(
        "    its copy (main)    {}",
        spread(&logged_times(&captured, "capture copied"))
    );
    println!(
        "    its encoding       {}",
        spread(&logged_times(&captured, "capture encoded"))
    );

    let mut physical = Vec::new();
    let mut refused = 0;
    for _ in 0..PHYSICAL_CAPTURES {
        let asked = Instant::now();
        match yuna.call("capture", json!({"seat": seat, "size": "physical"})) {
            Ok(_) => physical.push(asked.elapsed().as_micros() as u64),
            Err(_) => refused += 1,
        }
    }
    println!(
        "  {PHYSICAL_CAPTURES} captures at the physical size: {refused} refused; those given {}",
        spread(&physical)
    );
    yuna.stop()
}

/// The environment of `vkcube` number `n`, as the applications' tests give it
/// (`crates/yuna/tests/applications.rs`), and the Vulkan driver asked.
fn vkcube_environment(run: &Run, n: u32, options: &Options) -> Result<Value, String> {
    let home = run.folder.join(format!("vkcube-{n}"));
    fs::create_dir_all(&home).map_err(|e| format!("cannot create {}: {e}", home.display()))?;
    let driver = match &options.client_vulkan {
        Some(icd) => icd.clone(),
        None => shell_variable("YUNA_LAVAPIPE_ICD")?,
    };
    Ok(json!({
        "PATH": shell_variable("YUNA_APPLICATIONS_PATH")?,
        "HOME": home,
        "XDG_CACHE_HOME": home.join("cache"),
        "LD_LIBRARY_PATH": shell_variable("LD_LIBRARY_PATH")?,
        "VK_DRIVER_FILES": driver,
        "VK_LOADER_LAYERS_DISABLE": "~implicit~",
    }))
}

/// The run's folder: Yuna's runtime folder, its log, and the clients' homes. Removed at the end.
struct Run {
    folder: PathBuf,
}

impl Run {
    fn new() -> Result<Run, String> {
        let folder = std::env::temp_dir().join(format!("yuna-agent-time-{}", std::process::id()));
        // Wayland asks that the runtime folder be the user's alone.
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder.join("runtime"))
            .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
        Ok(Run { folder })
    }

    /// Starts the `yuna` built beside this program as `yuna mcp`, logging at `debug`, with
    /// nothing of the caller's environment but what it draws with.
    fn start_yuna(&self, options: &Options) -> Result<Yuna, String> {
        let program = std::env::current_exe()
            .ok()
            .and_then(|me| Some(me.parent()?.parent()?.join("yuna")))
            .filter(|path| path.exists())
            .ok_or("no yuna beside this program: build it with --bin yuna")?;
        let log = self.folder.join("yuna.log");
        let mut command = Command::new(program);
        command.args(["mcp", "--output", &options.output]);
        if options.gpu {
            command.arg("--gpu");
        }
        let mut child = command
            .env_clear()
            .env("LD_LIBRARY_PATH", shell_variable("LD_LIBRARY_PATH")?)
            .env(
                "__EGL_VENDOR_LIBRARY_FILENAMES",
                shell_variable("__EGL_VENDOR_LIBRARY_FILENAMES")?,
            )
            .env("XKB_CONFIG_ROOT", shell_variable("XKB_CONFIG_ROOT")?)
            .env("HOME", self.folder.join("yuna-home"))
            .env("XDG_RUNTIME_DIR", self.folder.join("runtime"))
            .env("YUNA_LOG", "debug")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(File::create(&log).map_err(|e| format!("Yuna's log: {e}"))?)
            .spawn()
            .map_err(|e| format!("yuna does not start: {e}"))?;
        let stdout = child.stdout.take().ok_or("Yuna's standard output")?;
        let input = child.stdin.take().ok_or("Yuna's standard input")?;
        let (sender, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                    return;
                };
                if sender.send(reply).is_err() {
                    return;
                }
            }
        });
        Ok(Yuna {
            child,
            input: Some(input),
            replies,
            next_id: 1,
            log,
        })
    }

    /// Keeps the folder, for its logs, and names it.
    fn keep(&self) -> String {
        let kept = self.folder.with_extension("kept");
        let _ = fs::rename(&self.folder, &kept);
        kept.display().to_string()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.folder);
    }
}

/// A variable the development shell sets.
fn shell_variable(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set: run in the shell, `nix develop`"))
}

/// The `yuna mcp` process: its input, its replies, read on a thread, and its log.
struct Yuna {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<Value>,
    next_id: u64,
    log: PathBuf,
}

impl Yuna {
    /// Calls a tool and gives its result, or what it refused.
    fn call(&mut self, tool: &str, arguments: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
                "name": tool,
                "arguments": arguments,
            }
        });
        let input = self.input.as_mut().ok_or("Yuna's input is closed")?;
        writeln!(input, "{request}").map_err(|e| format!("{tool}: {e}"))?;
        loop {
            let reply = self
                .replies
                .recv_timeout(PATIENCE)
                .map_err(|_| format!("no reply to {tool}"))?;
            if reply["id"] == json!(id) {
                let result = reply["result"].clone();
                return if result["isError"] == json!(false) {
                    Ok(result)
                } else {
                    Err(format!("{tool} failed: {reply}"))
                };
            }
        }
    }

    /// Calls `tool` `count` times, and gives each call's time, in microseconds.
    fn time_calls(
        &mut self,
        tool: &str,
        arguments: &Value,
        count: usize,
    ) -> Result<Vec<u64>, String> {
        let mut times = Vec::with_capacity(count);
        for _ in 0..count {
            let asked = Instant::now();
            self.call(tool, arguments.clone())?;
            times.push(asked.elapsed().as_micros() as u64);
        }
        Ok(times)
    }

    fn log_length(&self) -> Result<usize, String> {
        Ok(fs::read_to_string(&self.log)
            .map_err(|e| format!("Yuna's log: {e}"))?
            .len())
    }

    /// Yuna's log from `start`, to its last whole line.
    fn log_since(&self, start: usize) -> Result<String, String> {
        let log = fs::read_to_string(&self.log).map_err(|e| format!("Yuna's log: {e}"))?;
        let end = log.rfind('\n').map_or(start, |end| end + 1).max(start);
        Ok(log[start..end].to_owned())
    }

    /// Closes Yuna's input, as an agent's client does, and waits for it to end well.
    fn stop(mut self) -> Result<(), String> {
        drop(self.input.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => return Err(format!("yuna ended with {status}")),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => return Err("yuna did not stop when its input closed".to_owned()),
            }
        }
    }
}

impl Drop for Yuna {
    /// Closes Yuna's input, so that it ends its programs, the `vkcube` among them, as it stops;
    /// kills it if it has not stopped 10 s later.
    fn drop(&mut self) {
        drop(self.input.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The value of `key=<number>` in a log line.
fn field(line: &str, key: &str) -> Option<u64> {
    line.split_whitespace()
        .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
        .and_then(|value| value.parse().ok())
}

/// The `took_us` of each log line holding `message`.
fn logged_times(log: &str, message: &str) -> Vec<u64> {
    log.lines()
        .filter(|line| line.contains(message))
        .filter_map(|line| field(line, "took_us"))
        .collect()
}

/// The frames drawn in the log's lines, those presented after the boundary they aimed at, and
/// the median of their drawing's time.
fn frames(log: &str) -> String {
    let drawn: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("frame drawn"))
        .collect();
    let missed = drawn
        .iter()
        .filter(|line| {
            matches!((field(line, "boundary"), field(line, "presented")),
                     (Some(aimed), Some(presented)) if presented > aimed)
        })
        .count();
    let mut took: Vec<u64> = drawn
        .iter()
        .filter_map(|line| field(line, "took_us"))
        .collect();
    took.sort_unstable();
    let median = if took.is_empty() {
        0
    } else {
        percentile(&took, 50.0)
    };
    format!(
        "{} frames drawn, {missed} missed ({:.1} %), a drawing's median {median} µs",
        drawn.len(),
        if drawn.is_empty() {
            0.0
        } else {
            missed as f64 * 100.0 / drawn.len() as f64
        }
    )
}

/// Median, 99th percentile and longest of `times`, in microseconds.
fn spread(times: &[u64]) -> String {
    if times.is_empty() {
        return "none".to_owned();
    }
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    format!(
        "median {} µs, p99 {} µs, longest {} µs ({} samples)",
        percentile(&sorted, 50.0),
        percentile(&sorted, 99.0),
        sorted[sorted.len() - 1],
        sorted.len()
    )
}

/// The value at percentile `p` of `sorted`, by the nearest rank.
fn percentile(sorted: &[u64], p: f64) -> u64 {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_missed_and_logged_times_are_read() {
        let log = "x frame drawn took_us=900 lead_us=4000 at_once=false boundary=7 presented=7\n\
                   x frame drawn took_us=5000 lead_us=4000 at_once=false boundary=8 presented=9\n\
                   x capture copied output=1 width=3840 height=2160 took_us=12000\n";
        assert!(frames(log).starts_with("2 frames drawn, 1 missed (50.0 %)"));
        assert_eq!(logged_times(log, "capture copied"), [12_000]);
        assert_eq!(
            spread(&[3, 1, 2]),
            "median 2 µs, p99 3 µs, longest 3 µs (3 samples)"
        );
    }
}
