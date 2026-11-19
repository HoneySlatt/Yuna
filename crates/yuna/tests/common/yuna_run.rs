//! What the tests that run a `yuna` process in a folder of their own share, `applications.rs` and
//! `opium.rs`: its log's end for a failure's message, its stop on SIGTERM, and the end of a run.

use std::fs;
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

use smithay::reexports::rustix::process::{Pid, Signal, kill_process};

/// How often a test reads Yuna's log, captures or lists programs; reading the log never wakes
/// Yuna.
pub const POLL: Duration = Duration::from_millis(50);

/// The end of Yuna's `log`, its warnings and errors first, for a failure's message.
pub fn log_tail(log: &Path) -> String {
    let text = fs::read_to_string(log).unwrap_or_default();
    let warnings: Vec<&str> = text
        .lines()
        .filter(|line| line.contains(" WARN ") || line.contains(" ERROR "))
        .take(10)
        .collect();
    let lines: Vec<&str> = text.lines().collect();
    let tail = &lines[lines.len().saturating_sub(10)..];
    format!(
        "yuna's warnings:\n{}\nyuna's last lines ({}):\n{}",
        warnings.join("\n"),
        log.display(),
        tail.join("\n")
    )
}

/// What a failure shows: the ends of Yuna's log and of `program_log`, the log of its
/// `what`, an application or a program. The caller keeps the folder that holds both.
pub fn diagnosis(yuna_log: &Path, program_log: &Path, what: &str) -> String {
    let log = fs::read_to_string(program_log).unwrap_or_default();
    let lines: Vec<&str> = log.lines().collect();
    format!(
        "{}\nthe {what}'s last lines ({}):\n{}",
        log_tail(yuna_log),
        program_log.display(),
        lines[lines.len().saturating_sub(15)..].join("\n")
    )
}

/// Stops Yuna with SIGTERM, as an agent does, and requires that it ends well, within 10 s.
/// A failure carries the end of Yuna's `log`.
pub fn stop_on_sigterm(child: &mut Child, log: &Path) -> Result<(), String> {
    let _ = kill_process(Pid::from_child(child), Signal::TERM);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("yuna ended with {status}\n{}", log_tail(log))),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            _ => return Err(format!("yuna did not stop on SIGTERM\n{}", log_tail(log))),
        }
    }
}

/// Ends a run: kills Yuna if it still runs, which ends its programs. Its `folder` is kept when
/// the run `failed` or the test panics, for its logs and captures.
pub fn end_run(child: &mut Child, folder: &Path, failed: bool) {
    let _ = child.kill();
    let _ = child.wait();
    if failed || std::thread::panicking() {
        eprintln!("Yuna's folder is kept: {}", folder.display());
    } else {
        let _ = fs::remove_dir_all(folder);
    }
}
