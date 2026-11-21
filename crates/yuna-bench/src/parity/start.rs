//! A start's run, for the timed tier: `yuna mcp` alone in the run's namespaces, on the real clock,
//! *cold*, timed from its spawn to its `seat create`'s reply and to its seat's first frame, the
//! figures of [ADR-0100](../../../../docs/adr/0100-the-budgets-of-yuna-mcp-at-their-conditions.md).
//!
//! Cold: each start has a new home, so an empty Mesa shader cache (Mesa's cache is on, and
//! `XDG_CACHE_HOME` unset puts it in the home), as an agent's thrown away folders leave it
//! (ADR-0082, item 27). llvmpipe draws on its own threads, as in an agent's Yuna.
//!
//! Its temporary folder, `TMPDIR`, is one of two ([`Tmpdir`]), as the owner chose on 2026-11-21:
//!
//! - *full*, for the starts' parity figures: [`FULL_TMPDIR_ENTRIES`] files of no Yuna's, the
//!   starts' shared folder, as a busy `/tmp` is. Yuna sweeps a killed Yuna's homes from it as it
//!   starts; reading each entry's metadata before its name cost a start 60 ms in a `/tmp` of
//!   38,700 entries, which `58f2e75` fixed: a full folder keeps that fix guarded.
//! - *empty*, a new folder of the run's own, for ADR-0100's budgets, judged at that ADR's
//!   conditions: the longest of 20 cold starts of the head.
//!
//! The launcher (`launch.rs`) starts no client, and writes the time it starts Yuna to the log;
//! the script writes `seat create` at once, so that it waits in Yuna's input as Yuna starts. The
//! reply is timed when the script reads it; the first frame by the time of Yuna's first `frame
//! drawn` line on the seat's output, both on the wall clock.

use std::fs::{self, File};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::launch::{SPAWNED, UNSHARE_OPTIONS};
use super::mcp::Mcp;
use super::scenario::{Build, Device, Outcome, ThreadTimes, Tools, kill_group, thread_times, wait};
use crate::procfs::Proc;

/// A start's case: the mode of its seat's output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Start {
    /// Its name, as the report and the thresholds file give it.
    pub name: &'static str,
    /// The output's width and height in pixels, its rate in hertz and its scale.
    pub output: (u32, u32, f64, f64),
}

/// The starts ADR-0100 budgets: 1920 × 1080 at 60 Hz, `examples/instances.rs`'s default, and the
/// seats' default output, 3840 × 2160 at 240 Hz and scale 1.5.
pub const STARTS: [Start; 2] = [
    Start {
        name: "start-1080p",
        output: (1920, 1080, 60.0, 1.0),
    },
    Start {
        name: "start-4k",
        output: (3840, 2160, 240.0, 1.5),
    },
];

/// Which temporary folder a start is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tmpdir {
    /// The starts' shared folder of [`FULL_TMPDIR_ENTRIES`] files: the parity figures'.
    Full,
    /// A new empty folder of the run's own: the budgets', ADR-0100's conditions.
    Empty,
}

/// The files of the starts' full temporary folder: about the `/tmp` of M2.1's bisection, 38,700.
pub const FULL_TMPDIR_ENTRIES: usize = 40_000;

/// Fills `folder` with [`FULL_TMPDIR_ENTRIES`] empty files, `entry-<n>`, those it lacks; Yuna
/// removes none of them, so a run fills it once.
pub fn fill_tmpdir(folder: &Path) -> Result<(), String> {
    fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    for n in 0..FULL_TMPDIR_ENTRIES {
        let entry = folder.join(format!("entry-{n:05}"));
        if !entry.exists() {
            File::create(&entry).map_err(|e| format!("{}: {e}", entry.display()))?;
        }
    }
    Ok(())
}

/// How long a start may take, its first frame included, before the run fails.
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How often Yuna's log is read while its first frame is waited for.
const POLL: Duration = Duration::from_millis(2);

/// The start named `name`.
pub fn named(name: &str) -> Option<&'static Start> {
    STARTS.iter().find(|start| start.name == name)
}

/// Starts `build`'s Yuna for `start`, on `device`, with the temporary folder `tmpdir`, in
/// `folder`, and reads `reply_ms` and `first_frame_ms`, from its spawn, and `own_cpu_ns` and
/// `own_wait_ns`, its own threads' times on a core and on a run queue over the start
/// ([`ThreadTimes`]).
pub fn run(
    start: &Start,
    (device, tmpdir): (Device, Tmpdir),
    build: &Build,
    tools: &Tools,
    folder: &Path,
    runtime_index: usize,
) -> Outcome {
    let began = Instant::now();
    let mut outcome = Outcome::default();
    match timed_start(start, (device, tmpdir), build, tools, folder, runtime_index) {
        Ok(times) => {
            let readings = &mut outcome.readings;
            readings.insert("reply_ms".to_owned(), times.reply_ms);
            readings.insert("first_frame_ms".to_owned(), times.first_frame_ms);
            readings.insert("own_cpu_ns".to_owned(), times.threads.own_cpu_ns as f64);
            readings.insert("own_wait_ns".to_owned(), times.threads.own_wait_ns as f64);
        }
        Err(error) => outcome.error = Some(error),
    }
    outcome.seconds = began.elapsed().as_secs_f64();
    outcome
}

/// What a start's run measured.
struct StartTimes {
    /// From the spawn to `seat create`'s reply, in milliseconds.
    reply_ms: f64,
    /// From the spawn to the seat's first frame.
    first_frame_ms: f64,
    /// Yuna's threads' times over the start, once its first frame is drawn.
    threads: ThreadTimes,
}

fn timed_start(
    start: &Start,
    (device, tmpdir): (Device, Tmpdir),
    build: &Build,
    tools: &Tools,
    folder: &Path,
    runtime_index: usize,
) -> Result<StartTimes, String> {
    let runtime = tools.runtime.join(format!("r{runtime_index}"));
    let _ = fs::remove_dir_all(&runtime);
    fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&runtime)
        .map_err(|e| format!("{}: {e}", runtime.display()))?;
    for sub in ["home", "tmp"] {
        fs::create_dir_all(folder.join(sub))
            .map_err(|e| format!("{}: {e}", folder.join(sub).display()))?;
    }
    let temporary = match tmpdir {
        Tmpdir::Full => tools.full_tmpdir.clone(),
        Tmpdir::Empty => folder.join("tmp"),
    };
    let log_path = folder.join("yuna.log");
    let log = File::create(&log_path).map_err(|e| format!("yuna.log: {e}"))?;
    let mut command = Command::new(&tools.unshare);
    command
        .args(UNSHARE_OPTIONS)
        .arg("--")
        .arg(&tools.program)
        .args(["parity", "launch", "--windows", "0", "--control"])
        // No client: the launcher reads neither.
        .arg(runtime.join("control"))
        .arg("--client-log")
        .arg(folder.join("client.log"))
        .arg("--")
        .arg(&build.yuna)
        .arg("mcp");
    if device == Device::Gpu {
        command.arg("--gpu");
    }
    command
        .env_clear()
        .envs(build.environment.iter().map(|(k, v)| (k, v)))
        .env("HOME", folder.join("home"))
        .env("TMPDIR", &temporary)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("YUNA_LOG", "debug")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(log)
        .process_group(0);
    if let Some(canary) = &build.canary {
        command.env("YUNA_PARITY_CANARY", canary);
    }
    let mut namespace = command
        .spawn()
        .map_err(|e| format!("{}: {e}", tools.unshare.display()))?;
    let (Some(input), Some(output)) = (namespace.stdin.take(), namespace.stdout.take()) else {
        kill_group(&mut namespace);
        return Err("Yuna's standard input or output".to_owned());
    };
    let mut mcp = Mcp::new(input, output);
    let (width, height, rate, scale) = start.output;
    let timed = (|| {
        let seat = mcp
            .call(
                "seat",
                json!({"action": "create", "width": width, "height": height, "rate": rate,
                       "scale": scale}),
                START_TIMEOUT,
            )
            .map_err(|e| e.to_string())?;
        let replied = SystemTime::now();
        let output = seat
            .pointer("/outputs/0/id")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("seat create gave no output: {seat}"))?;
        let (spawned, drawn) = wait_for_first_frame(&log_path, output)?;
        // Over the whole start: how long its own threads waited for a core.
        let threads = thread_times(&Proc::system(), namespace.id(), &build.yuna);
        let since = |at: SystemTime| -> Result<f64, String> {
            at.duration_since(spawned)
                .map(|d| d.as_secs_f64() * 1e3)
                .map_err(|_| "a time before Yuna's spawn: the wall clock stepped".to_owned())
        };
        Ok(StartTimes {
            reply_ms: since(replied)?,
            first_frame_ms: since(drawn)?,
            threads,
        })
    })();
    mcp.close();
    match wait(&mut namespace, START_TIMEOUT) {
        Some(status) if status.success() => timed,
        Some(status) => timed.and(Err(format!("Yuna ended with {status}"))),
        None => {
            kill_group(&mut namespace);
            timed.and(Err(format!(
                "Yuna did not stop in {} s once its input closed",
                START_TIMEOUT.as_secs()
            )))
        }
    }
}

/// Waits until the log at `path` holds the launcher's time of Yuna's spawn and a `frame drawn`
/// line on `output`, and gives both times.
fn wait_for_first_frame(path: &Path, output: u64) -> Result<(SystemTime, SystemTime), String> {
    let until = Instant::now() + START_TIMEOUT;
    loop {
        let log = fs::read_to_string(path).map_err(|e| format!("yuna.log: {e}"))?;
        if let Some(times) = first_frame(&log, output) {
            return Ok(times);
        }
        if Instant::now() > until {
            return Err(format!(
                "no frame drawn on output {output} in {} s",
                START_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// The launcher's time of Yuna's spawn in `log`, and the time of its first `frame drawn` line on
/// `output`; none until both are there.
pub fn first_frame(log: &str, output: u64) -> Option<(SystemTime, SystemTime)> {
    let spawned: u64 = log
        .lines()
        .find_map(|line| line.strip_prefix(SPAWNED))?
        .trim()
        .parse()
        .ok()?;
    let spawned = UNIX_EPOCH + Duration::from_nanos(spawned);
    let on_output = format!("output={output}");
    let drawn = log
        .lines()
        .filter(|line| line.contains(" frame drawn "))
        .find(|line| line.split_whitespace().any(|word| word == on_output))
        .and_then(crate::logs::yuna_log_time)?;
    Some((spawned, drawn))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_is_timed_from_the_launchers_spawn_to_the_outputs_first_frame() {
        let log = "parity launch: spawned_unix_ns=1794975178500000000\n\
                   2026-11-18T04:12:58.600000Z  INFO yuna::mcp: seat created seat=1\n\
                   2026-11-18T04:12:58.610000Z DEBUG yuna::frames: frame drawn output=10 \
                   took_us=900\n\
                   2026-11-18T04:12:58.637080Z DEBUG yuna::frames: frame drawn output=1 \
                   elements=0 took_us=4000\n";
        let (spawned, drawn) = first_frame(log, 1).expect("both times");
        assert_eq!(
            drawn.duration_since(spawned).unwrap(),
            Duration::from_micros(137_080)
        );
        // Not there yet: no line on that output, or no spawn's.
        assert_eq!(first_frame(log, 2), None);
        let unspawned: String = log.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(first_frame(&unspawned, 1), None);
        assert!(named("start-4k").is_some() && named("start").is_none());
    }

    #[test]
    fn the_starts_temporary_folder_is_filled_once() {
        let folder = crate::testing::folder("parity-full-tmpdir");
        let tmpdir = folder.join("full-tmp");
        fill_tmpdir(&tmpdir).unwrap();
        let count = || fs::read_dir(&tmpdir).unwrap().count();
        assert_eq!(count(), FULL_TMPDIR_ENTRIES);
        // A missing entry is made again; the others are kept.
        fs::remove_file(tmpdir.join("entry-00007")).unwrap();
        fill_tmpdir(&tmpdir).unwrap();
        assert_eq!(count(), FULL_TMPDIR_ENTRIES);
        fs::remove_dir_all(&folder).unwrap();
    }
}
