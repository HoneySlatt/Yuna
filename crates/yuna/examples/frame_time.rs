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
//!   the frame, its clients' buffers imported, to when the drawing is done, on `CLOCK_MONOTONIC`.
//!   It decides when the frame is presented;
//! - the time between two frames' presentations (`presented_at_us`), against the output's
//!   shortest refresh, 1/rate: under VRR, none may come sooner (ADR-0046);
//! - how many frames were drawn at once, the output idle, how many were presented after the
//!   boundary they aimed at, and their lead, from the same lines (ADR-0031);
//! - the CPU time of each of Yuna's threads, from `/proc/<pid>/task/<tid>/schedstat`: on
//!   llvmpipe, its `llvmpipe-N` threads rasterize, the work a GPU would do;
//! - the GPU's busy time for Yuna, from the kernel's DRM usage statistics in
//!   `/proc/<pid>/fdinfo` (`drm-engine-<engine>`, nanoseconds), summed over the DRM clients its
//!   files open;
//! - the same CPU and GPU times for the clients, and the buffers Yuna holds: dma-bufs or memfds.
//!
//! Build it and Yuna for release, then run it in the development shell, alone among
//! measurements; it needs no screen of its own:
//!
//! ```sh
//! nix develop -c cargo build --release -p yuna --bin yuna --example frame_time
//! nix develop -c target/release/examples/frame_time
//! ```
//!
//! It stops what it started when it ends, and keeps its folder of logs when it fails. Its
//! processes get no signal when it dies: killed outright, it leaves Yuna and `vkcube` running.
//! On the owner's machine, agents run it inside `agent-headless`, which stops every process of
//! its run.
//!
//! Options: `--gpu` draws on the GPU's render node (never in the tests); `--client-vulkan
//! <icd.json>` gives `vkcube` that Vulkan driver rather than lavapipe, such as the shell's Mesa's
//! `radeon_icd.x86_64.json`, beside `$YUNA_LAVAPIPE_ICD`; `--windows <n>`; `--output <mode>`, as
//! Yuna takes it; `--seconds <s>`, the measured window, 10 by default. The figures go to
//! `docs/measurements.md` with the machine, its load and the commit.

mod common;

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::{
    Clients, PATIENCE, Run, Usage, WARM_UP, field, percentile, print_usage, shell_variable,
    vkcube_command,
};
use smithay::reexports::rustix::process::{Pid, Signal, kill_process};

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
    let run = Run::new("yuna-frame-time")?;
    measure_in(&run, options).map_err(|reason| format!("{reason}; logs kept in {}", run.keep()))
}

fn measure_in(run: &Run, options: &Options) -> Result<(), String> {
    let yuna = start_yuna(run, options)?;
    let socket = yuna.socket.clone();
    let mut clients = Clients(Vec::new());
    for n in 0..options.windows {
        clients.0.push(start_vkcube(run, n, &socket, options)?);
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
    let measured = &log[before.log_length..after.log_length];
    let frame_times = frame_times(measured);
    let timing = frame_timing(measured);
    let intervals = presentation_intervals(measured);
    let warnings: Vec<&str> = log
        .lines()
        .filter(|line| line.contains(" WARN ") || line.contains(" ERROR "))
        .take(5)
        .collect();
    report(
        options,
        &before,
        &after,
        &frame_times,
        &intervals,
        timing,
        &warnings,
    );
    drop(clients);
    yuna.stop()
}

/// Starts the `yuna` built beside this program, logging at `debug`, with nothing of the caller's
/// environment but what it draws with.
fn start_yuna(run: &Run, options: &Options) -> Result<Yuna, String> {
    let program = std::env::current_exe()
        .ok()
        .and_then(|me| Some(me.parent()?.parent()?.join("yuna")))
        .filter(|path| path.exists())
        .ok_or("no yuna beside this program: build it with --bin yuna")?;
    let log = run.folder().join("yuna.log");
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
        .env("HOME", run.folder().join("yuna-home"))
        .env("XDG_RUNTIME_DIR", run.folder().join("runtime"))
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
    yuna.socket = run.folder().join("runtime").join(name);
    Ok(yuna)
}

/// Starts `vkcube` number `n` on Yuna's socket.
fn start_vkcube(run: &Run, n: u32, socket: &Path, options: &Options) -> Result<Child, String> {
    vkcube_command(run, n, options.client_vulkan.as_deref())?
        .env("WAYLAND_DISPLAY", socket)
        .spawn()
        .map_err(|e| format!("vkcube does not start: {e}"))
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
    usage: Usage,
    log_length: usize,
    frames_drawn: u64,
    commits: u64,
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
        Ok(Sample {
            log_length,
            frames_drawn: counter("frames_drawn")?,
            commits: counter("commits")?,
            usage: Usage::take(yuna.child.id(), clients)?,
        })
    }
}

/// Each frame's time in the log's lines, in microseconds.
fn frame_times(log: &str) -> Vec<u64> {
    log.lines()
        .filter(|line| line.contains("frame drawn"))
        .filter_map(|line| field(line, "took_us"))
        .collect()
}

/// The times between consecutive frames' presentations in the log's lines, in microseconds.
fn presentation_intervals(log: &str) -> Vec<u64> {
    let times: Vec<u64> = log
        .lines()
        .filter(|line| line.contains("frame drawn"))
        .filter_map(|line| field(line, "presented_at_us"))
        .collect();
    times
        .windows(2)
        .map(|pair| pair[1].saturating_sub(pair[0]))
        .collect()
}

/// Of the frames in the log's lines: how many were drawn at once, the output idle; how many were
/// presented after the boundary they aimed at, missed (ADR-0031); and each one's lead, in
/// microseconds.
fn frame_timing(log: &str) -> (usize, usize, Vec<u64>) {
    let (mut at_once, mut missed, mut leads) = (0, 0, Vec::new());
    for line in log.lines().filter(|line| line.contains("frame drawn")) {
        if line.contains("at_once=true") {
            at_once += 1;
        }
        if let (Some(aimed), Some(presented)) = (field(line, "boundary"), field(line, "presented"))
            && presented > aimed
        {
            missed += 1;
        }
        leads.extend(field(line, "lead_us"));
    }
    (at_once, missed, leads)
}

/// The rate of the output measured: the one `--output` names, else Yuna's default, 240 Hz.
fn output_rate(options: &Options) -> f64 {
    options
        .output
        .as_deref()
        .and_then(|mode| mode.split_once('@')?.1.split_once(',')?.0.parse().ok())
        .unwrap_or(240.0)
}

fn report(
    options: &Options,
    before: &Sample,
    after: &Sample,
    frame_times: &[u64],
    intervals: &[u64],
    timing: (usize, usize, Vec<u64>),
    warnings: &[&str],
) {
    let seconds = (after.usage.at - before.usage.at).as_secs_f64();
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
        let mut sorted_intervals = intervals.to_vec();
        sorted_intervals.sort_unstable();
        if !sorted_intervals.is_empty() {
            // Whole microseconds on both sides: an interval one under the period is not sooner.
            let sooner = sorted_intervals
                .iter()
                .filter(|&&us| (us as f64) < period_us - 1.0)
                .count();
            println!(
                "  between two presentations, {} intervals: shortest {} µs, 1st percentile {} \
                 µs, median {} µs, 99th percentile {} µs, longest {} µs; {sooner} sooner than \
                 the period",
                sorted_intervals.len(),
                sorted_intervals[0],
                percentile(&sorted_intervals, 1.0),
                percentile(&sorted_intervals, 50.0),
                percentile(&sorted_intervals, 99.0),
                sorted_intervals[sorted_intervals.len() - 1],
            );
        }
        let (at_once, missed, mut leads) = timing;
        leads.sort_unstable();
        if !leads.is_empty() {
            println!(
                "  {at_once} drawn at once; {missed} presented after the boundary they aimed at; \
                 lead median {} µs, longest {} µs",
                percentile(&leads, 50.0),
                leads[leads.len() - 1],
            );
        }
    }

    print_usage(&before.usage, &after.usage, frames);
    if !warnings.is_empty() {
        println!("Yuna's first warnings:\n  {}", warnings.join("\n  "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missed_and_at_once_frames_are_counted() {
        let log = "\
            DEBUG yuna::frames: frame drawn output=1 took_us=900 lead_us=3000 at_once=false boundary=5 presented=5\n\
            DEBUG yuna::frames: frame drawn output=1 took_us=4900 lead_us=0 at_once=true boundary=9 presented=10\n\
            DEBUG yuna: counters frames_drawn=2\n";
        assert_eq!(frame_timing(log), (1, 1, vec![3000, 0]));
    }

    #[test]
    fn presentation_intervals_are_read_between_consecutive_frames() {
        let log = "\
            DEBUG yuna::frames: frame drawn output=1 presented=5 presented_at_us=1000\n\
            DEBUG yuna::frames: counters frames_drawn=1\n\
            DEBUG yuna::frames: frame drawn output=1 presented=6 presented_at_us=5167\n\
            DEBUG yuna::frames: frame drawn output=1 presented=8 presented_at_us=15000\n";
        assert_eq!(presentation_intervals(log), vec![4167, 9833]);
    }
}
