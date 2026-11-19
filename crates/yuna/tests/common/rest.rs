//! The rest measure of the ROADMAP's bar, shared by `applications.rs` and `opium.rs`: Yuna's
//! counters in its log, and its threads' context switches in `/proc`. A test that includes it
//! includes `yuna_run.rs` beside it, under that name, for the polling period they share.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::yuna_run::POLL;

/// How long Yuna must do nothing: the rest window of the bar (ROADMAP's quality bar).
pub const REST: Duration = Duration::from_secs(10);

/// What Yuna counted, from its last `counters` line (`Compositor::counters`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub frames_drawn: u64,
    pub frame_callbacks: u64,
    pub configures: u64,
    pub commits: u64,
    pub wakeups: u64,
}

impl Counters {
    /// Reads `frames_drawn=3 frame_callbacks=5 configures=2 commits=6 wakeups=40` after the
    /// message `counters`: all five, nothing else.
    fn parse(line: &str) -> Option<Counters> {
        let fields: Vec<(&str, u64)> = line
            .split_once(" counters ")?
            .1
            .split_whitespace()
            .map(|field| {
                let (key, value) = field.split_once('=')?;
                Some((key, value.parse().ok()?))
            })
            .collect::<Option<_>>()?;
        match fields.as_slice() {
            [
                ("frames_drawn", frames_drawn),
                ("frame_callbacks", frame_callbacks),
                ("configures", configures),
                ("commits", commits),
                ("wakeups", wakeups),
            ] => Some(Counters {
                frames_drawn: *frames_drawn,
                frame_callbacks: *frame_callbacks,
                configures: *configures,
                commits: *commits,
                wakeups: *wakeups,
            }),
            _ => None,
        }
    }

    /// What was counted since `before`. Yuna's counters only grow: a smaller value is a reading
    /// gone wrong, never Yuna at rest.
    fn since(self, before: Counters) -> Counters {
        let less = |after: u64, before: u64| {
            after
                .checked_sub(before)
                .unwrap_or_else(|| panic!("a counter of Yuna fell from {before} to {after}"))
        };
        Counters {
            frames_drawn: less(self.frames_drawn, before.frames_drawn),
            frame_callbacks: less(self.frame_callbacks, before.frame_callbacks),
            configures: less(self.configures, before.configures),
            commits: less(self.commits, before.commits),
            wakeups: less(self.wakeups, before.wakeups),
        }
    }
}

/// What Yuna did during a rest window that should have had nothing.
#[derive(Debug)]
pub struct Activity {
    pub counters: Counters,
    pub context_switches: u64,
    /// Each thread that switched, or that came or went: its id, and its switches or what
    /// happened to it.
    pub threads: Vec<String>,
}

impl fmt::Display for Activity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.counters;
        write!(
            f,
            "in {} s, {} frames drawn, {} frame callbacks, {} configures, {} commits, {} \
             wake-ups; {} context switches ({})",
            REST.as_secs(),
            c.frames_drawn,
            c.frame_callbacks,
            c.configures,
            c.commits,
            c.wakeups,
            self.context_switches,
            self.threads.join(", ")
        )
    }
}

/// The last `counters` line of Yuna's `log`, or zeros before its first round. A last line
/// without its end may be half written, and is left out.
pub fn counters(log: &Path) -> Counters {
    let text = fs::read_to_string(log).unwrap_or_default();
    let complete = text.rfind('\n').map_or("", |end| &text[..end]);
    complete
        .lines()
        .rev()
        .find_map(Counters::parse)
        .unwrap_or_default()
}

/// Waits until Yuna has had no round for `quiet`, at most `patience`, and gives its counters
/// then, as its `log` shows them.
pub fn wait_until_quiet(
    log: &Path,
    quiet: Duration,
    patience: Duration,
) -> Result<Counters, String> {
    let deadline = Instant::now() + patience;
    let mut last = counters(log);
    let mut since = Instant::now();
    loop {
        std::thread::sleep(POLL);
        let now = counters(log);
        if now != last {
            last = now;
            since = Instant::now();
        } else if since.elapsed() >= quiet {
            return Ok(now);
        }
        if Instant::now() > deadline {
            return Err(format!(
                "never quiet for {} s in {} s: {last:?}",
                quiet.as_secs(),
                patience.as_secs()
            ));
        }
    }
}

/// Requires [`REST`] with no context switch in any of the threads of process `pid`, Yuna, and no
/// round in its `log`: no frame, frame callback, configure, commit or wake-up. Says what it
/// measured.
pub fn measure_rest(pid: u32, log: &Path) -> Result<String, Activity> {
    let threads_before = thread_context_switches(pid);
    let counters_before = counters(log);
    std::thread::sleep(REST);
    let threads_after = thread_context_switches(pid);
    let counted = counters(log).since(counters_before);

    let mut context_switches = 0;
    let mut threads = Vec::new();
    for (thread, after) in &threads_after {
        match threads_before.get(thread) {
            Some(before) if before == after => {}
            Some(before) => {
                context_switches += after - before;
                threads.push(format!("thread {thread}: {}", after - before));
            }
            None => {
                context_switches += after;
                threads.push(format!("thread {thread} started"));
            }
        }
    }
    for thread in threads_before.keys() {
        if !threads_after.contains_key(thread) {
            threads.push(format!("thread {thread} ended"));
        }
    }
    if counted == Counters::default() && threads.is_empty() {
        Ok(format!(
            "{} s of rest: 0 context switches in its {} threads, 0 frames, frame \
             callbacks, configures, commits and wake-ups",
            REST.as_secs(),
            threads_after.len()
        ))
    } else {
        Err(Activity {
            counters: counted,
            context_switches,
            threads,
        })
    }
}

/// Each thread of process `pid`, and its context switches so far, voluntary and not.
fn thread_context_switches(pid: u32) -> BTreeMap<u32, u64> {
    let tasks = PathBuf::from(format!("/proc/{pid}/task"));
    let mut threads = BTreeMap::new();
    for task in fs::read_dir(tasks).expect("Yuna's threads").flatten() {
        let Some(thread) = task.file_name().to_str().and_then(|id| id.parse().ok()) else {
            continue;
        };
        // A thread that ended between the listing and the read is left out.
        let Ok(status) = fs::read_to_string(task.path().join("status")) else {
            continue;
        };
        let switches = status
            .lines()
            .filter(|line| line.contains("ctxt_switches:"))
            .filter_map(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
            .sum();
        threads.insert(thread, switches);
    }
    threads
}
