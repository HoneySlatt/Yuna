//! Running WLCS's processes ([`Supervisor`]): batches of tests, `--jobs` at once, each WLCS process
//! watched as it runs.
//!
//! Each process is WLCS's runner, `<runner> <module> --gtest_color=no --gtest_filter=<filter>`,
//! started in its own process group, in the run's work folder, without `WAYLAND_DISPLAY` and
//! `WAYLAND_SOCKET`, with `LP_NUM_THREADS=0` and the run's own `MESA_SHADER_CACHE_DIR`. Its
//! standard output and error are one pipe, so that the integration module's markers fall between
//! a test's start and its end. A thread reads the pipe and, once every writer closed it, waits
//! for the process's end without reaping it; the batch's thread waits on what it sends, with a
//! time limit and nothing else: no polling. Every line goes to the process's log, in
//! `<work>/logs/`.
//!
//! - The hang guard: no new line for the guard's time (60 s; WLCS's own waits give up after
//!   10 s) kills the whole process group; the test running hangs.
//! - A batch resumes after a crash or a hang in a new process, its filter less every test that
//!   ended, the culprit with them, as long as each process began a test.
//! - The breaker: [`BREAKER`] hangs or timeouts in a row, in the order the tests end across the
//!   jobs, stop the run; the [`Watch`] says which ends count.
//! - The deadline stops the run once it passes.
//! - A stop kills every running process's group at once.
//!
//! Each process group is killed by its id while its leader is not yet reaped, so the id cannot
//! have been given to another process; nothing is killed by name. Each process gets SIGKILL too
//! when the thread that started it ends: when this program is killed, its processes end with it,
//! though a stray child of theirs would not.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, PipeReader, Write as _};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, WaitId, WaitIdOptions};

use super::listing::{self, Listing};
use super::output::{Ended, Event, Parser, ProcessEnd, Seen};

/// The hang guard's time, unless the tests shorten it: six times WLCS's own 10 s waits.
pub const HANG: Duration = Duration::from_secs(60);

/// The hangs or timeouts in a row that stop the run.
pub const BREAKER: usize = 5;

/// How long a killed process has to end before it is left behind.
const KILL_GRACE: Duration = Duration::from_secs(10);

/// The bytes of a line kept: the rest of a longer one is read and dropped.
const LINE_KEPT: usize = 64 * 1024;

/// The lines read and not yet handled, at most: past them, the reading thread waits, and WLCS
/// with it.
const LINES_QUEUED: usize = 1024;

/// The bytes of a process's log kept.
const LOG_KEPT: u64 = 16 * 1024 * 1024;

/// The bytes of the list of tests read at most; WLCS 1.8.1's is 100 KB.
const LISTING_KEPT: usize = 16 * 1024 * 1024;

/// The work folder's folder of Mesa's shader cache.
pub const SHADER_CACHE: &str = "mesa-shader-cache";

/// The work folder's folder of the processes' logs.
pub const LOGS: &str = "logs";

/// How the processes run.
#[derive(Debug, Clone)]
pub struct Settings {
    /// WLCS's runner.
    pub runner: PathBuf,
    /// The integration module, by an absolute path: WLCS loads it with `dlopen`, which would
    /// search the library path for a bare name.
    pub module: PathBuf,
    /// The run's folder: the processes' working folder, their logs and Mesa's shader cache.
    pub work: PathBuf,
    /// Processes at once.
    pub jobs: usize,
    /// The hang guard's time.
    pub hang: Duration,
    /// The run's deadline, from the supervisor's start; none for a record.
    pub deadline: Option<Duration>,
}

/// Tests that one process runs, and its successors after a crash or a hang.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    /// Its name, in the logs and the reports: a suite, or a test and its run.
    pub label: String,
    /// The filter's positive patterns: `Suite.*`, or the tests' names.
    pub patterns: Vec<String>,
    /// The tests the patterns select: those not ended when the batch ends did not run.
    pub tests: Vec<String>,
}

/// One test's run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRun {
    /// How it ended.
    pub ended: Ended,
    /// Its batch's label.
    pub batch: String,
    /// Its process's log; none when it did not run.
    pub log: Option<PathBuf>,
}

/// A process that failed before any of its tests began, with no test to blame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessFailure {
    /// Its batch's label.
    pub batch: String,
    /// A crash or a hang.
    pub seen: Seen,
    /// What happened, in words.
    pub text: String,
    /// Its log.
    pub log: PathBuf,
}

/// Why a run stopped before its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The sentinel's verdict: a test expected to pass among the first failed.
    Sentinel(String),
    /// The breaker: these tests, or batches, hung or timed out in a row.
    Breaker(Vec<String>),
    /// The deadline passed.
    Deadline(Duration),
}

impl Stop {
    /// Why, in words.
    pub fn describe(&self) -> String {
        match self {
            Stop::Sentinel(text) => format!("the sentinel: {text}"),
            Stop::Breaker(names) => format!(
                "the breaker: {} hangs or timeouts in a row: {}",
                names.len(),
                names.join(", ")
            ),
            Stop::Deadline(deadline) => {
                format!("the deadline: {} passed", super::duration(*deadline))
            }
        }
    }
}

/// What a mode says of a test's end.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Verdict {
    /// Whether it counts toward the breaker: a hang, or a timeout the mode does not expect.
    pub toward_breaker: bool,
    /// Whether the run stops, and why.
    pub stop: Option<Stop>,
}

/// What a mode does as the tests end: it prints each, and judges whether the run goes on.
pub trait Watch: Sync {
    /// A test's run ended, or its outcome changed once its process ended (`correction`): prints
    /// it and gives its verdict. Called one at a time, in the order the runs end.
    fn ended(&self, run: &TestRun, correction: bool) -> Verdict;
    /// A process failed before any test began.
    fn failed(&self, failure: &ProcessFailure);
}

/// What a run gave.
#[derive(Debug, Default)]
pub struct Ran {
    /// Each test's run, those that did not run included.
    pub runs: Vec<TestRun>,
    /// The processes that failed with no test to blame.
    pub failures: Vec<ProcessFailure>,
}

/// Runs WLCS's processes, and stops them.
pub struct Supervisor {
    settings: Settings,
    started: Instant,
    state: Mutex<State>,
    /// Processes started, which number their logs.
    processes: AtomicU64,
}

/// What the processes' threads share.
#[derive(Debug, Default)]
struct State {
    stop: Option<Stop>,
    /// The process groups running, by their leader's id: a stop kills them.
    groups: BTreeMap<i32, Pid>,
    /// The hangs and timeouts in a row.
    streak: Vec<String>,
}

impl State {
    /// Adds `name`'s hang or timeout to the streak; the [`BREAKER`]th in a row stops the run.
    fn count_toward_breaker(&mut self, name: &str) {
        self.streak.push(name.to_owned());
        if self.streak.len() >= BREAKER {
            let streak = std::mem::take(&mut self.streak);
            stop_all(self, Stop::Breaker(streak));
        }
    }
}

/// What the thread that reads a process's output sends.
enum Output {
    Line(String),
    /// The pipe ended, and then the process: it is not reaped yet.
    Exited,
}

/// Why the runner killed a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kill {
    Hang,
    Deadline,
}

/// Locks `mutex`, whose data no panic can leave half changed.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Sets the run's stop, unless it has one, and kills every running process group.
fn stop_all(state: &mut State, stop: Stop) {
    state.stop.get_or_insert(stop);
    for leader in state.groups.values() {
        let _ = rustix::process::kill_process_group(*leader, Signal::KILL);
    }
}

/// gtest's `--gtest_filter`: the positive patterns, then those that ended, to leave out.
fn filter_argument<'a>(patterns: &[String], ended: impl Iterator<Item = &'a String>) -> String {
    let mut filter = format!("--gtest_filter={}", patterns.join(":"));
    for (index, name) in ended.enumerate() {
        filter.push(if index == 0 { '-' } else { ':' });
        filter.push_str(name);
    }
    filter
}

/// A file's name for `label`: its letters, digits, `.`, `-` and `_`, others as `_`, at most 160.
fn file_name(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(160)
        .collect()
}

/// Makes `command`'s process end with SIGKILL when the thread that starts it ends, as when this
/// program is killed or stopped by Ctrl-C: in its own process group, it gets no signal of the
/// terminal's, and would run on alone. The thread that starts a WLCS process waits for its end,
/// so it never ends first otherwise. The same as the `yuna` crate's tests' `dies_with_test`, which
/// this crate cannot use (it depends on nothing of `yuna`'s).
fn ends_with_its_thread(command: &mut Command) {
    let parent = std::process::id();
    // SAFETY: the closure runs in the child between fork and exec, where only async-signal-safe
    // calls are sound: it makes two system calls, `prctl` and `getppid`, through rustix, and
    // allocates nothing, an `Errno` turning into an `io::Error` without a box.
    unsafe {
        command.pre_exec(move || {
            rustix::process::set_parent_process_death_signal(Some(Signal::KILL))?;
            // The parent may have ended before the signal was set: then it never comes.
            if u32::try_from(Pid::as_raw(rustix::process::getppid())) != Ok(parent) {
                return Err(rustix::io::Errno::SRCH.into());
            }
            Ok(())
        });
    }
}

/// Reads one line into `line`, its newline left out, its first [`LINE_KEPT`] bytes kept. Gives
/// false at the end of the output, with nothing read.
fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<bool> {
    line.clear();
    let mut read_any = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return Ok(read_any);
        }
        read_any = true;
        let newline = available.iter().position(|&byte| byte == b'\n');
        let taken = newline.unwrap_or(available.len());
        let room = LINE_KEPT.saturating_sub(line.len());
        line.extend_from_slice(&available[..taken.min(room)]);
        reader.consume(taken + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(true);
        }
    }
}

/// Reads the output of the process `leader` leads until every writer closed it, then waits for
/// the leader's end without reaping it, so that its process group's id stays its own until the
/// batch's thread has killed what remains of the group.
fn read_output(pipe: PipeReader, leader: Pid, events: &SyncSender<Output>) {
    let mut reader = BufReader::new(pipe);
    let mut line = Vec::new();
    loop {
        match read_line(&mut reader, &mut line) {
            Ok(true) => {
                let text = String::from_utf8_lossy(&line).into_owned();
                if events.send(Output::Line(text)).is_err() {
                    // The batch's thread left the process behind.
                    return;
                }
            }
            Ok(false) => break,
            Err(error) => {
                let _ = events.send(Output::Line(format!(
                    "yuna-bench: the output could not be read further: {error}"
                )));
                break;
            }
        }
    }
    drop(reader);
    while let Err(rustix::io::Errno::INTR) = rustix::process::waitid(
        WaitId::Pid(leader),
        WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
    ) {}
    let _ = events.send(Output::Exited);
}

/// A process's log, cut at [`LOG_KEPT`] bytes. An error writing it is said once, on the standard
/// error, and the rest of the log is lost; the run goes on.
struct Log {
    path: PathBuf,
    file: Option<BufWriter<File>>,
    written: u64,
}

impl Log {
    fn create(path: &Path) -> Result<Log, String> {
        let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Log {
            path: path.to_owned(),
            file: Some(BufWriter::new(file)),
            written: 0,
        })
    }

    fn line(&mut self, line: &str) {
        let Some(file) = &mut self.file else {
            return;
        };
        let length = line.len() as u64 + 1;
        let (written, keep) = if self.written + length > LOG_KEPT {
            (
                writeln!(file, "yuna-bench: the log is cut here, at {LOG_KEPT} bytes")
                    .and_then(|()| file.flush()),
                false,
            )
        } else {
            self.written += length;
            (writeln!(file, "{line}"), true)
        };
        let failed = written.is_err();
        if let Err(error) = written {
            eprintln!(
                "yuna-bench wlcs: {}: {error}; the rest of this log is lost",
                self.path.display()
            );
        }
        if failed || !keep {
            self.file = None;
        }
    }

    fn close(mut self) {
        if let Some(mut file) = self.file.take()
            && let Err(error) = file.flush()
        {
            eprintln!("yuna-bench wlcs: {}: {error}", self.path.display());
        }
    }
}

impl Supervisor {
    /// A supervisor for `settings`, its work folder's `logs/` and shader cache made; the
    /// deadline counts from now.
    pub fn new(settings: Settings) -> Result<Supervisor, String> {
        for folder in [LOGS, SHADER_CACHE] {
            let path = settings.work.join(folder);
            fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(Supervisor {
            settings,
            started: Instant::now(),
            state: Mutex::new(State::default()),
            processes: AtomicU64::new(0),
        })
    }

    /// Why the run stopped, if it did.
    pub fn stop(&self) -> Option<Stop> {
        lock(&self.state).stop.clone()
    }

    /// The tests WLCS lists, the disabled left out.
    pub fn list(&self) -> Result<Listing, String> {
        let log = self.next_log("list");
        let mut text = String::new();
        let mut too_long = false;
        let end = self.run_process(&log, &["--gtest_list_tests".to_owned()], &mut |line| {
            if text.len() + line.len() >= LISTING_KEPT {
                too_long = true;
            } else {
                text.push_str(line);
                text.push('\n');
            }
        })?;
        if too_long {
            return Err(format!(
                "WLCS's list of tests is longer than {LISTING_KEPT} bytes: {}",
                log.display()
            ));
        }
        if end != ProcessEnd::Exited(0) {
            return Err(format!(
                "WLCS could not list its tests ({}): its output is in {}",
                end.describe(),
                log.display()
            ));
        }
        listing::parse(&text)
            .map_err(|e| format!("WLCS's list of tests, kept in {}: {e}", log.display()))
    }

    /// Runs `batches`, `jobs` at once, the first first; gives every test's run once all ended or
    /// the run stopped.
    pub fn run(&self, batches: Vec<Batch>, watch: &dyn Watch) -> Ran {
        let workers = self.settings.jobs.clamp(1, batches.len().max(1));
        let queue = Mutex::new(VecDeque::from(batches));
        let ran = Mutex::new(Ran::default());
        let work = || {
            while let Some(batch) = lock(&queue).pop_front() {
                let (runs, failures) = self.run_batch(&batch, watch);
                let mut ran = lock(&ran);
                ran.runs.extend(runs);
                ran.failures.extend(failures);
            }
        };
        thread::scope(|scope| {
            let mut started = 0;
            for _ in 0..workers {
                match thread::Builder::new()
                    .name("wlcs-batches".to_owned())
                    .spawn_scoped(scope, work)
                {
                    Ok(_) => started += 1,
                    Err(error) => eprintln!("yuna-bench wlcs: a thread for the batches: {error}"),
                }
            }
            if started == 0 {
                work();
            }
        });
        ran.into_inner().unwrap_or_else(PoisonError::into_inner)
    }

    /// The path of the next process's log.
    fn next_log(&self, label: &str) -> PathBuf {
        let number = self.processes.fetch_add(1, Ordering::Relaxed) + 1;
        self.settings
            .work
            .join(LOGS)
            .join(format!("{number:04}-{}.log", file_name(label)))
    }

    /// Gives `run` to the watch, counts it toward the breaker, and stops the run if either says
    /// so. A correction leaves the breaker as it was, unless it is a hang.
    fn report(&self, watch: &dyn Watch, run: &TestRun, correction: bool) {
        let mut state = lock(&self.state);
        let verdict = watch.ended(run, correction);
        // A correction changes a test counted already: only a hang it reveals counts.
        if verdict.toward_breaker && (!correction || run.ended.seen == Seen::Hang) {
            state.count_toward_breaker(&run.ended.name);
        } else if !correction {
            state.streak.clear();
        }
        if let Some(stop) = verdict.stop {
            stop_all(&mut state, stop);
        }
    }

    /// Gives `failure` to the watch; a hang counts toward the breaker, a crash breaks the streak.
    fn report_failure(&self, watch: &dyn Watch, failure: &ProcessFailure) {
        let mut state = lock(&self.state);
        watch.failed(failure);
        if failure.seen == Seen::Hang {
            state.count_toward_breaker(&failure.batch);
        } else {
            state.streak.clear();
        }
    }

    /// Runs `batch` in as many processes as its crashes and hangs need; gives each of its tests'
    /// runs, and the processes that failed with no test to blame.
    fn run_batch(&self, batch: &Batch, watch: &dyn Watch) -> (Vec<TestRun>, Vec<ProcessFailure>) {
        let mut ended: BTreeMap<String, TestRun> = BTreeMap::new();
        let mut failures = Vec::new();
        while self.stop().is_none() && batch.tests.iter().any(|test| !ended.contains_key(test)) {
            let arguments = [
                "--gtest_color=no".to_owned(),
                filter_argument(&batch.patterns, ended.keys()),
            ];
            let log = self.next_log(&batch.label);
            let mut parser = Parser::new();
            let process = self.run_process(&log, &arguments, &mut |line| {
                for event in parser.feed(line) {
                    if let Event::Ended(test) = event {
                        let run = TestRun {
                            ended: test,
                            batch: batch.label.clone(),
                            log: Some(log.clone()),
                        };
                        self.report(watch, &run, false);
                        ended.insert(run.ended.name.clone(), run);
                    }
                }
            });
            let end = match process {
                Ok(end) => end,
                Err(text) => {
                    let failure = ProcessFailure {
                        batch: batch.label.clone(),
                        seen: Seen::Crash,
                        text,
                        log,
                    };
                    self.report_failure(watch, &failure);
                    failures.push(failure);
                    break;
                }
            };
            let finish = parser.finish(end);
            for test in finish.changed {
                let correction = ended.contains_key(&test.name);
                let run = TestRun {
                    ended: test,
                    batch: batch.label.clone(),
                    log: Some(log.clone()),
                };
                self.report(watch, &run, correction);
                ended.insert(run.ended.name.clone(), run);
            }
            if let Some((seen, text)) = finish.failure {
                let left: Vec<&String> = batch
                    .tests
                    .iter()
                    .filter(|test| !ended.contains_key(*test))
                    .collect();
                if let [test] = left[..] {
                    // The filter selected that test alone: the process could have run no other.
                    let run = TestRun {
                        ended: Ended {
                            name: test.clone(),
                            seen,
                            note: Some(format!("its process ended before it began: {text}")),
                            timed_out: false,
                        },
                        batch: batch.label.clone(),
                        log: Some(log.clone()),
                    };
                    self.report(watch, &run, false);
                    ended.insert(test.clone(), run);
                } else {
                    let failure = ProcessFailure {
                        batch: batch.label.clone(),
                        seen,
                        text,
                        log: log.clone(),
                    };
                    self.report_failure(watch, &failure);
                    failures.push(failure);
                }
            }
            for what in &finish.stray_markers {
                eprintln!(
                    "yuna-bench wlcs: {}: an unsupported marker outside any test: {what} ({})",
                    batch.label,
                    log.display()
                );
            }
            let resumable = matches!(
                end,
                ProcessEnd::Exited(_) | ProcessEnd::Signalled(_) | ProcessEnd::Hung
            ) && !finish.whole
                && finish.began;
            if !resumable {
                break;
            }
        }
        let note = match self.stop() {
            Some(stop) => format!("not run: {}", stop.describe()),
            None => "not run: its process ended before it".to_owned(),
        };
        let mut runs: Vec<TestRun> = Vec::with_capacity(batch.tests.len());
        for test in &batch.tests {
            if !ended.contains_key(test) {
                runs.push(TestRun {
                    ended: Ended {
                        name: test.clone(),
                        seen: Seen::NotRun,
                        note: Some(note.clone()),
                        timed_out: false,
                    },
                    batch: batch.label.clone(),
                    log: None,
                });
            }
        }
        runs.extend(ended.into_values());
        (runs, failures)
    }

    /// Runs one WLCS process with `arguments` after the module, giving `on_line` each line of its
    /// output as it comes, and writing it to `log`; gives how it ended. An error is a process
    /// that could not start.
    fn run_process(
        &self,
        log: &Path,
        arguments: &[String],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<ProcessEnd, String> {
        let mut log = Log::create(log)?;
        log.line(&format!(
            "$ {} {} {}",
            self.settings.runner.display(),
            self.settings.module.display(),
            arguments.join(" ")
        ));
        let (reader, writer) = io::pipe().map_err(|e| format!("a pipe: {e}"))?;
        let mut child = {
            let mut command = Command::new(&self.settings.runner);
            command
                .arg(&self.settings.module)
                .args(arguments)
                .current_dir(&self.settings.work)
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("WAYLAND_SOCKET")
                .env("LP_NUM_THREADS", "0")
                .env(
                    "MESA_SHADER_CACHE_DIR",
                    self.settings.work.join(SHADER_CACHE),
                )
                .stdin(Stdio::null())
                .stdout(writer.try_clone().map_err(|e| format!("a pipe: {e}"))?)
                .stderr(writer)
                .process_group(0);
            ends_with_its_thread(&mut command);
            command
                .spawn()
                .map_err(|e| format!("{}: {e}", self.settings.runner.display()))?
            // The command, and with it this process's ends of the pipe for writing, is dropped
            // here: the pipe ends once the process group closes it.
        };
        let leader = Pid::from_child(&child);
        let (events, received) = mpsc::sync_channel(LINES_QUEUED);
        let reading = thread::Builder::new()
            .name("wlcs-output".to_owned())
            .spawn(move || read_output(reader, leader, &events));
        let reading = match reading {
            Ok(reading) => reading,
            Err(error) => {
                let _ = rustix::process::kill_process_group(leader, Signal::KILL);
                let _ = child.wait();
                return Err(format!("a thread to read WLCS's output: {error}"));
            }
        };
        {
            let mut state = lock(&self.state);
            state.groups.insert(leader.as_raw_pid(), leader);
            if state.stop.is_some() {
                let _ = rustix::process::kill_process_group(leader, Signal::KILL);
            }
        }
        let deadline = self.settings.deadline.map(|d| (self.started + d, d));
        let mut killed = None;
        let mut killed_at: Option<Instant> = None;
        let mut last_line = Instant::now();
        // The limits are checked before each wait, not only when one times out: a process that
        // never stops printing would otherwise never meet its deadline.
        let left_behind = loop {
            let now = Instant::now();
            let hang = last_line + self.settings.hang;
            match (killed_at, deadline) {
                (Some(at), _) if now >= at + KILL_GRACE => break true,
                (Some(_), _) => {}
                (None, Some((at, length))) if now >= at => {
                    killed = Some(Kill::Deadline);
                    stop_all(&mut lock(&self.state), Stop::Deadline(length));
                    killed_at = Some(now);
                    continue;
                }
                (None, _) if now >= hang => {
                    killed = Some(Kill::Hang);
                    let _ = rustix::process::kill_process_group(leader, Signal::KILL);
                    killed_at = Some(now);
                    continue;
                }
                (None, _) => {}
            }
            let until = match (killed_at, deadline) {
                (Some(at), _) => at + KILL_GRACE,
                (None, Some((at, _))) => at.min(hang),
                (None, None) => hang,
            };
            match received.recv_timeout(until.saturating_duration_since(now)) {
                Ok(Output::Line(line)) => {
                    last_line = Instant::now();
                    log.line(&line);
                    on_line(&line);
                }
                Ok(Output::Exited) | Err(RecvTimeoutError::Disconnected) => break false,
                // The checks above act on it.
                Err(RecvTimeoutError::Timeout) => {}
            }
        };
        {
            // The leader ended and is not reaped: its group's id is still its own, and whatever
            // else of its group lives dies with it.
            let mut state = lock(&self.state);
            state.groups.remove(&leader.as_raw_pid());
            if !left_behind {
                let _ = rustix::process::kill_process_group(leader, Signal::KILL);
            }
        }
        if left_behind {
            log.line(&format!(
                "yuna-bench: the process did not end {} s after SIGKILL: left behind",
                KILL_GRACE.as_secs()
            ));
            log.close();
            // Neither the reading thread nor the child is waited for: both stay blocked on the
            // process that does not end.
            drop(reading);
            return Ok(ProcessEnd::Lost);
        }
        let _ = reading.join();
        let status = child
            .wait()
            .map_err(|e| format!("waiting for WLCS's runner: {e}"))?;
        log.line(&format!("yuna-bench: the process ended: {status}"));
        log.close();
        let stopped = self.stop().is_some();
        Ok(match (killed, status.code(), status.signal()) {
            (Some(Kill::Hang), _, _) => ProcessEnd::Hung,
            (Some(Kill::Deadline), _, _) => ProcessEnd::Stopped,
            (None, None, Some(9)) if stopped => ProcessEnd::Stopped,
            (None, Some(code), _) => ProcessEnd::Exited(code),
            (None, None, signal) => ProcessEnd::Signalled(signal.unwrap_or(0)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_leaves_out_what_ended() {
        let patterns = ["Suite.*".to_owned()];
        assert_eq!(
            filter_argument(&patterns, [].iter()),
            "--gtest_filter=Suite.*"
        );
        let ended = ["Suite.a".to_owned(), "Suite.b/0".to_owned()];
        assert_eq!(
            filter_argument(&patterns, ended.iter()),
            "--gtest_filter=Suite.*-Suite.a:Suite.b/0"
        );
        let patterns = ["SelfTest.*".to_owned(), "FrameSubmission.*".to_owned()];
        assert_eq!(
            filter_argument(&patterns, ended[..1].iter()),
            "--gtest_filter=SelfTest.*:FrameSubmission.*-Suite.a"
        );
    }

    #[test]
    fn a_long_line_is_cut_and_the_next_read_whole() {
        let long = "x".repeat(LINE_KEPT + 10);
        let text = format!("{long}\nnext\nlast without a newline");
        let mut reader = BufReader::with_capacity(1000, text.as_bytes());
        let mut line = Vec::new();
        assert!(read_line(&mut reader, &mut line).unwrap());
        assert_eq!(line.len(), LINE_KEPT);
        assert!(read_line(&mut reader, &mut line).unwrap());
        assert_eq!(line, b"next");
        assert!(read_line(&mut reader, &mut line).unwrap());
        assert_eq!(line, b"last without a newline");
        assert!(!read_line(&mut reader, &mut line).unwrap());
    }

    #[test]
    fn a_labels_file_name_keeps_its_safe_characters() {
        assert_eq!(
            file_name("XdgShellStableSubsurfaces/SubsurfaceTest.place_above_simple/0 run 2"),
            "XdgShellStableSubsurfaces_SubsurfaceTest.place_above_simple_0_run_2"
        );
        assert_eq!(file_name(&"a".repeat(300)).len(), 160);
    }
}
