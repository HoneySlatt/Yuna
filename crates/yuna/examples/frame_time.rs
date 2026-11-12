//! Measures what a frame of `yuna --headless` costs while real clients draw without pause: the
//! figure M1.4 asks for on a 4K output, on llvmpipe and on the GPU (`docs/PLAN.md`).
//!
//! It starts the `yuna` program built beside it, on one output (by default Yuna's own,
//! 3840 × 2160 at 240 Hz and scale 1.5), then `vkcube` in as many windows as asked, two by
//! default: in Yuna's columns of half the output's width, two windows cover it, so that every
//! frame draws the whole output. After a warm-up, it reads, over the measured window:
//!
//! - the frames Yuna drew and the commits it got, from its `counters` lines;
//! - each frame's time, from its `frame drawn` lines (`took_us`): from when Yuna starts to build
//!   the frame, its clients' buffers imported, to when the drawing is done, as Yuna's own clock
//!   says. It decides when the frame is presented;
//! - the CPU time of each of Yuna's threads, from `/proc/<pid>/task/<tid>/schedstat`: on
//!   llvmpipe, its `llvmpipe-N` threads rasterize, the work a GPU would do;
//! - the GPU's busy time for Yuna, from the kernel's DRM usage statistics in
//!   `/proc/<pid>/fdinfo` (`drm-engine-<engine>`, nanoseconds), summed over the DRM clients its
//!   files open;
//! - the same CPU and GPU times for the clients, and the buffers Yuna holds: dma-bufs or memfds.
//!
//! Build it and Yuna for release, then run it in the development shell, through
//! `agent-headless` on the owner's machine, alone among measurements. It stops what it started
//! when it ends, and keeps its folder of logs when it fails; killed outright, it leaves them to
//! `agent-headless`, which stops every process of its run:
//!
//! ```sh
//! nix develop -c cargo build --release -p yuna --bin yuna --example frame_time
//! nix develop -c agent-headless -- target/release/examples/frame_time
//! ```
//!
//! Options: `--gpu` draws on the GPU's render node (never in the tests); `--client-vulkan
//! <icd.json>` gives `vkcube` that Vulkan driver rather than lavapipe, such as the shell's Mesa's
//! `radeon_icd.x86_64.json`, beside `$YUNA_LAVAPIPE_ICD`; `--windows <n>`; `--output <mode>`, as
//! Yuna takes it; `--seconds <s>`, the measured window, 10 by default. The figures go to
//! `docs/measurements.md` with the machine, its load and the commit.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, DirBuilder, File};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use smithay::reexports::rustix::process::{Pid, Signal, kill_process};

/// How long the clients draw before the measured window opens.
const WARM_UP: Duration = Duration::from_secs(3);
/// The longest Yuna and its clients may take to start and show their windows.
const PATIENCE: Duration = Duration::from_secs(60);

/// What the command line asks for.
struct Options {
    gpu: bool,
    client_vulkan: Option<String>,
    windows: u32,
    output: Option<String>,
    seconds: u64,
}

fn main() -> ExitCode {
    let options = match read_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(reason) => {
            eprintln!("frame_time: {reason}");
            return ExitCode::from(2);
        }
    };
    match measure(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("frame_time: {reason}");
            ExitCode::FAILURE
        }
    }
}

fn read_options(arguments: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        gpu: false,
        client_vulkan: None,
        windows: 2,
        output: None,
        seconds: 10,
    };
    let mut arguments = arguments;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        match argument.as_str() {
            "--gpu" => options.gpu = true,
            "--client-vulkan" => options.client_vulkan = Some(value()?),
            "--output" => options.output = Some(value()?),
            "--windows" => {
                options.windows = value()?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or("--windows takes a whole number above 0")?;
            }
            "--seconds" => {
                options.seconds = value()?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or("--seconds takes a whole number above 0")?;
            }
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }
    Ok(options)
}

/// Runs the measure in a folder of its own, kept with its logs when the measure fails.
fn measure(options: &Options) -> Result<(), String> {
    let run = Run::new()?;
    measure_in(&run, options).map_err(|reason| format!("{reason}; logs kept in {}", run.keep()))
}

fn measure_in(run: &Run, options: &Options) -> Result<(), String> {
    let yuna = run.start_yuna(options)?;
    let socket = yuna.socket.clone();
    let mut clients = Clients(Vec::new());
    for n in 0..options.windows {
        clients.0.push(run.start_vkcube(n, &socket, options)?);
    }
    yuna.wait_for_windows(options.windows, &mut clients.0)?;
    std::thread::sleep(WARM_UP);

    let window = Duration::from_secs(options.seconds);
    let before = Sample::take(&yuna, &clients.0)?;
    std::thread::sleep(window);
    let after = Sample::take(&yuna, &clients.0)?;
    for client in &mut clients.0 {
        if let Ok(Some(status)) = client.try_wait() {
            return Err(format!("a vkcube left during the measure: {status}"));
        }
    }
    let log = fs::read_to_string(&yuna.log).map_err(|e| format!("Yuna's log: {e}"))?;
    let frame_times = frame_times(&log[before.log_length..after.log_length]);
    let warnings: Vec<&str> = log
        .lines()
        .filter(|line| line.contains(" WARN ") || line.contains(" ERROR "))
        .take(5)
        .collect();
    report(options, &before, &after, &frame_times, &warnings);
    drop(clients);
    yuna.stop()
}

/// The run's folder: Yuna's runtime folder, its log, and the clients' homes and logs. Removed at
/// the end.
struct Run {
    folder: PathBuf,
}

impl Run {
    fn new() -> Result<Run, String> {
        let folder = std::env::temp_dir().join(format!("yuna-frame-time-{}", std::process::id()));
        // Wayland asks that the runtime folder be the user's alone.
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder.join("runtime"))
            .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
        Ok(Run { folder })
    }

    /// Starts the `yuna` built beside this program, logging at `debug`, with nothing of the
    /// caller's environment but what it draws with.
    fn start_yuna(&self, options: &Options) -> Result<Yuna, String> {
        let program = std::env::current_exe()
            .ok()
            .and_then(|me| Some(me.parent()?.parent()?.join("yuna")))
            .filter(|path| path.exists())
            .ok_or("no yuna beside this program: build it with --bin yuna")?;
        let log = self.folder.join("yuna.log");
        let mut command = Command::new(program);
        command.arg("--headless");
        if options.gpu {
            command.arg("--gpu");
        }
        if let Some(mode) = &options.output {
            command.args(["--output", mode]);
        }
        command
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
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(File::create(&log).map_err(|e| format!("Yuna's log: {e}"))?);
        let mut child = command
            .spawn()
            .map_err(|e| format!("yuna does not start: {e}"))?;
        let stdout = child.stdout.take().ok_or("Yuna's standard output")?;
        let mut yuna = Yuna {
            child,
            log,
            socket: PathBuf::new(),
        };
        // Read on a thread of its own, so that a Yuna that never prints fails in time.
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = sender.send(line);
        });
        let line = receiver.recv_timeout(PATIENCE).unwrap_or_default();
        let name = line
            .trim_end()
            .strip_prefix("WAYLAND_DISPLAY=")
            .ok_or_else(|| format!("yuna printed no socket: {line:?}"))?;
        yuna.socket = self.folder.join("runtime").join(name);
        Ok(yuna)
    }

    /// Starts `vkcube` number `n`, drawing without end, with the environment the applications'
    /// tests give it (`crates/yuna/tests/applications.rs`) and the Vulkan driver asked.
    fn start_vkcube(&self, n: u32, socket: &Path, options: &Options) -> Result<Child, String> {
        let home = self.folder.join(format!("vkcube-{n}"));
        fs::create_dir_all(&home).map_err(|e| format!("cannot create {}: {e}", home.display()))?;
        let output = File::create(home.join("log")).map_err(|e| format!("vkcube's log: {e}"))?;
        let driver = match &options.client_vulkan {
            Some(icd) => icd.clone(),
            None => shell_variable("YUNA_LAVAPIPE_ICD")?,
        };
        Command::new("vkcube")
            .args(["--wsi", "wayland"])
            .env_clear()
            .env("PATH", shell_variable("YUNA_APPLICATIONS_PATH")?)
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", self.folder.join("runtime"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("WAYLAND_DISPLAY", socket)
            .env("LD_LIBRARY_PATH", shell_variable("LD_LIBRARY_PATH")?)
            .env("VK_DRIVER_FILES", driver)
            .env("VK_LOADER_LAYERS_DISABLE", "~implicit~")
            .stdin(Stdio::null())
            .stdout(
                output
                    .try_clone()
                    .map_err(|e| format!("vkcube's log: {e}"))?,
            )
            .stderr(output)
            .spawn()
            .map_err(|e| format!("vkcube does not start: {e}"))
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

/// The `vkcube` processes, killed when dropped: a `Child` dropped alone keeps running.
struct Clients(Vec<Child>);

impl Drop for Clients {
    fn drop(&mut self) {
        for client in &mut self.0 {
            let _ = client.kill();
            let _ = client.wait();
        }
    }
}

/// A variable the development shell sets.
fn shell_variable(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set: run in the shell, `nix develop`"))
}

/// The `yuna` process, its log and its socket.
struct Yuna {
    child: Child,
    log: PathBuf,
    socket: PathBuf,
}

impl Yuna {
    /// Waits until `windows` windows have mapped and a frame was drawn after the last.
    fn wait_for_windows(&self, windows: u32, clients: &mut [Child]) -> Result<(), String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let log = fs::read_to_string(&self.log).unwrap_or_default();
            let opened = log.matches("window opened").count();
            if let Some(last) = log.rfind("window opened")
                && opened >= windows as usize
                && log[last..].contains("frame drawn")
            {
                return Ok(());
            }
            for client in clients.iter_mut() {
                if let Ok(Some(status)) = client.try_wait() {
                    return Err(format!("a vkcube ended before the measure: {status}"));
                }
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "{opened} of {windows} windows shown in {} s",
                    PATIENCE.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Stops Yuna with SIGTERM, as an agent does.
    fn stop(mut self) -> Result<(), String> {
        let terminated = kill_process(Pid::from_child(&self.child), Signal::TERM).is_ok();
        let deadline = Instant::now() + Duration::from_secs(10);
        while terminated && Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("yuna ended with {status}"))
                };
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err("yuna did not stop on SIGTERM".to_owned())
    }
}

impl Drop for Yuna {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What is read at each end of the measured window.
struct Sample {
    at: Instant,
    /// Yuna's process id, which is also its main thread's.
    pid: u32,
    log_length: usize,
    frames_drawn: u64,
    commits: u64,
    /// Each of Yuna's threads: its name, and its CPU time in nanoseconds.
    yuna_threads: BTreeMap<u32, (String, u64)>,
    /// The GPU's busy time for Yuna, per engine, in nanoseconds.
    yuna_gpu: BTreeMap<String, u64>,
    /// The CPU time of all the clients' threads, in nanoseconds.
    clients_cpu: u64,
    /// The GPU's busy time for all the clients, per engine, in nanoseconds.
    clients_gpu: BTreeMap<String, u64>,
    /// The files Yuna holds open that are dma-bufs, and memfds.
    yuna_dmabufs: usize,
    yuna_memfds: usize,
}

impl Sample {
    fn take(yuna: &Yuna, clients: &[Child]) -> Result<Sample, String> {
        let log = fs::read_to_string(&yuna.log).map_err(|e| format!("Yuna's log: {e}"))?;
        // A last line without its end may be half written: the sample stops before it.
        let log_length = log.rfind('\n').map_or(0, |end| end + 1);
        let last_counters = log[..log_length]
            .lines()
            .rev()
            .find(|line| line.contains(" counters "))
            .ok_or("Yuna logged no counters")?;
        let counter =
            |key| field(last_counters, key).ok_or_else(|| format!("no {key} in {last_counters:?}"));
        let pid = yuna.child.id();
        let mut clients_cpu = 0;
        let mut clients_gpu = BTreeMap::new();
        for client in clients {
            clients_cpu += threads(client.id())?
                .values()
                .map(|(_, ns)| ns)
                .sum::<u64>();
            add_engines(&mut clients_gpu, gpu_engines(client.id()));
        }
        let (yuna_dmabufs, yuna_memfds) = buffer_files(pid);
        Ok(Sample {
            at: Instant::now(),
            pid,
            log_length,
            frames_drawn: counter("frames_drawn")?,
            commits: counter("commits")?,
            yuna_threads: threads(pid)?,
            yuna_gpu: gpu_engines(pid),
            clients_cpu,
            clients_gpu,
            yuna_dmabufs,
            yuna_memfds,
        })
    }
}

/// The value of `key=<number>` in a log line.
fn field(line: &str, key: &str) -> Option<u64> {
    line.split_whitespace()
        .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
        .and_then(|value| value.parse().ok())
}

/// Each frame's time in the log's lines, in microseconds.
fn frame_times(log: &str) -> Vec<u64> {
    log.lines()
        .filter(|line| line.contains("frame drawn"))
        .filter_map(|line| field(line, "took_us"))
        .collect()
}

/// Each thread of process `pid`: its name, and the CPU time it has used, in nanoseconds (the
/// first field of `schedstat`).
fn threads(pid: u32) -> Result<BTreeMap<u32, (String, u64)>, String> {
    let tasks = PathBuf::from(format!("/proc/{pid}/task"));
    let mut threads = BTreeMap::new();
    for task in fs::read_dir(&tasks)
        .map_err(|e| format!("{}: {e}", tasks.display()))?
        .flatten()
    {
        let Some(tid) = task.file_name().to_str().and_then(|id| id.parse().ok()) else {
            continue;
        };
        // A thread that ended between the listing and the read is left out.
        let (Ok(name), Ok(schedstat)) = (
            fs::read_to_string(task.path().join("comm")),
            fs::read_to_string(task.path().join("schedstat")),
        ) else {
            continue;
        };
        let Some(ns) = schedstat
            .split_whitespace()
            .next()
            .and_then(|ns| ns.parse().ok())
        else {
            continue;
        };
        threads.insert(tid, (name.trim_end().to_owned(), ns));
    }
    Ok(threads)
}

/// The GPU's busy time for process `pid`, per engine, in nanoseconds: the `drm-engine-<engine>`
/// keys of the kernel's DRM usage statistics, in the `fdinfo` of each DRM file it holds, each
/// DRM client counted once however many files share it.
fn gpu_engines(pid: u32) -> BTreeMap<String, u64> {
    let mut engines = BTreeMap::new();
    let mut seen_clients = Vec::new();
    let Ok(files) = fs::read_dir(format!("/proc/{pid}/fd")) else {
        return engines;
    };
    for file in files.flatten() {
        let is_drm = fs::read_link(file.path()).is_ok_and(|target| target.starts_with("/dev/dri/"));
        if !is_drm {
            continue;
        }
        let fdinfo = format!("/proc/{pid}/fdinfo/{}", file.file_name().to_string_lossy());
        let Ok(info) = fs::read_to_string(fdinfo) else {
            continue;
        };
        let mut client = None;
        let mut file_engines = HashMap::new();
        for line in info.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            if key == "drm-client-id" {
                client = Some(value.to_owned());
            } else if let Some(engine) = key.strip_prefix("drm-engine-")
                && let Some(ns) = value.strip_suffix(" ns").and_then(|ns| ns.parse().ok())
            {
                file_engines.insert(engine.to_owned(), ns);
            }
        }
        if let Some(client) = client {
            if seen_clients.contains(&client) {
                continue;
            }
            seen_clients.push(client);
        }
        add_engines(&mut engines, file_engines);
    }
    engines
}

fn add_engines(sum: &mut BTreeMap<String, u64>, more: impl IntoIterator<Item = (String, u64)>) {
    for (engine, ns) in more {
        *sum.entry(engine).or_default() += ns;
    }
}

/// How many of process `pid`'s open files are dma-bufs, and how many memfds: the buffers its
/// clients share with it.
fn buffer_files(pid: u32) -> (usize, usize) {
    let (mut dmabufs, mut memfds) = (0, 0);
    if let Ok(files) = fs::read_dir(format!("/proc/{pid}/fd")) {
        for file in files.flatten() {
            let Ok(target) = fs::read_link(file.path()) else {
                continue;
            };
            let target = target.to_string_lossy();
            if target.starts_with("/dmabuf:") {
                dmabufs += 1;
            } else if target.starts_with("/memfd:") {
                memfds += 1;
            }
        }
    }
    (dmabufs, memfds)
}

/// The rate of the output measured: the one `--output` names, else Yuna's default, 240 Hz.
fn output_rate(options: &Options) -> f64 {
    options
        .output
        .as_deref()
        .and_then(|mode| mode.split_once('@')?.1.split_once(',')?.0.parse().ok())
        .unwrap_or(240.0)
}

/// The value at percentile `p` of `sorted`, by the nearest rank.
fn percentile(sorted: &[u64], p: f64) -> u64 {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// A thread's name without its number, so that `llvmpipe-0` to `llvmpipe-15` count as one.
fn thread_kind(name: &str) -> &str {
    name.trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches(['-', ':', '_'])
}

fn report(
    options: &Options,
    before: &Sample,
    after: &Sample,
    frame_times: &[u64],
    warnings: &[&str],
) {
    let seconds = (after.at - before.at).as_secs_f64();
    let frames = after.frames_drawn - before.frames_drawn;
    let commits = after.commits - before.commits;
    println!(
        "Yuna on {}, {} vkcube on {}, window{} on {}, {:.1} s measured after {} s of warm-up",
        if options.gpu {
            "the GPU (--gpu)"
        } else {
            "llvmpipe"
        },
        options.windows,
        options.client_vulkan.as_deref().unwrap_or("lavapipe"),
        if options.windows > 1 { "s" } else { "" },
        options.output.as_deref().unwrap_or("Yuna's default output"),
        seconds,
        WARM_UP.as_secs(),
    );
    println!(
        "frames drawn: {frames} ({:.1} a second); commits: {commits} ({:.1} a second)",
        frames as f64 / seconds,
        commits as f64 / seconds,
    );
    if frames == 0 {
        println!("no frame drawn: nothing to divide by");
        return;
    }
    let per_frame = |ns: u64| ns as f64 / frames as f64 / 1_000.0;

    let mut sorted = frame_times.to_vec();
    sorted.sort_unstable();
    if !sorted.is_empty() {
        println!(
            "a frame's time (took_us), {} frames: median {} µs, 99th percentile {} µs, \
             longest {} µs",
            sorted.len(),
            percentile(&sorted, 50.0),
            percentile(&sorted, 99.0),
            sorted[sorted.len() - 1],
        );
        let period_us = 1e6 / output_rate(options);
        let longer = sorted.iter().filter(|&&us| us as f64 > period_us).count();
        println!("  {longer} frames took longer than the output's period, {period_us:.0} µs");
    }

    let mut kinds: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    let mut total = 0;
    for (tid, (name, after_ns)) in &after.yuna_threads {
        let before_ns = before.yuna_threads.get(tid).map_or(0, |(_, ns)| *ns);
        let used = after_ns.saturating_sub(before_ns);
        // The main thread runs the event loop and Yuna's own work; the others are Mesa's.
        let kind = if *tid == after.pid {
            "main thread"
        } else {
            thread_kind(name)
        };
        let kind = kinds.entry(kind).or_default();
        kind.0 += 1;
        kind.1 += used;
        total += used;
    }
    println!(
        "Yuna's CPU time per frame: {:.0} µs over its {} threads ({:.2} cores busy)",
        per_frame(total),
        after.yuna_threads.len(),
        total as f64 / 1e9 / seconds,
    );
    for (kind, (count, used)) in &kinds {
        if *used > 0 {
            println!("  {kind} ({count} threads): {:.0} µs", per_frame(*used));
        }
    }
    let gpu = |after: &BTreeMap<String, u64>, before: &BTreeMap<String, u64>| {
        let engines = after
            .iter()
            .map(|(engine, ns)| {
                let used = ns.saturating_sub(before.get(engine).copied().unwrap_or(0));
                format!("{engine} {:.0} µs", per_frame(used))
            })
            .collect::<Vec<_>>();
        if engines.is_empty() {
            "no DRM file open".to_owned()
        } else {
            engines.join(", ")
        }
    };
    println!(
        "Yuna's GPU time per frame (fdinfo): {}",
        gpu(&after.yuna_gpu, &before.yuna_gpu)
    );
    println!(
        "the clients, per frame Yuna drew: CPU {:.0} µs; GPU (fdinfo) {}",
        per_frame(after.clients_cpu - before.clients_cpu),
        gpu(&after.clients_gpu, &before.clients_gpu),
    );
    println!(
        "buffers Yuna holds at the end: {} dma-bufs, {} memfds",
        after.yuna_dmabufs, after.yuna_memfds
    );
    if !warnings.is_empty() {
        println!("Yuna's first warnings:\n  {}", warnings.join("\n  "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_take_the_nearest_rank() {
        let values: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&values, 50.0), 50);
        assert_eq!(percentile(&values, 99.0), 99);
        assert_eq!(percentile(&[7], 99.0), 7);
    }

    #[test]
    fn fields_and_thread_kinds_are_read() {
        let line = "DEBUG yuna::render: frame drawn output=1 elements=2 took_us=4170";
        assert_eq!(field(line, "took_us"), Some(4170));
        assert_eq!(field(line, "took"), None);
        assert_eq!(thread_kind("llvmpipe-12"), "llvmpipe");
        assert_eq!(thread_kind("yuna"), "yuna");
    }
}
