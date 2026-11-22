//! `yuna-bench wlcs`: WLCS, the Wayland conformance suite, run against Yuna's integration module
//! (`crates/yuna-wlcs`), each test judged against a tracked file of its expected outcomes,
//! `crates/yuna-wlcs/expected.txt`.
//!
//! WLCS 1.8.1's runner, `$YUNA_WLCS_RUNNER <module.so> [gtest flags]`, loads the module and runs
//! its tests in one process, one after another, a fresh compositor for each. A crash of the
//! compositor kills the whole process, WLCS's exit status ignores skipped tests and is 0 when its
//! filter matches nothing, and gtest's XML and JSON reports count WLCS's skips as failures. So the
//! runner reads WLCS's printed output instead, line by line ([`output`]), and watches each process
//! itself ([`running`]).
//!
//! # The commands
//!
//! - `wlcs check --listed`: the tests the file expects to pass, alone: `scripts/check.sh`'s, fast.
//! - `wlcs check --full`: every test, CI's own job. A sentinel first: the suites `SelfTest`,
//!   `FrameSubmission` and `XdgSurfaceStableTest` in one process; if a test among them expected
//!   to pass does not, the run stops at once on a broad failure. Then one process per suite (a
//!   parameterised `Prefix/Suite` is a suite of its own), the largest first, `--jobs` at once.
//!   Last, each test not expected to pass that passed runs again alone.
//! - `wlcs record --runs <n> --out <file.json>`: every test, each in its own process, `n` times,
//!   `--jobs` at once; the outcomes seen go to a JSON file ([`record`]).
//! - `wlcs record --merge <a.json> <b.json>… --write <expected.txt>`: the file of expected
//!   outcomes from records.
//!
//! # A test's outcome
//!
//! A test passes only on gtest's `[       OK ] <its exact name>` line, without a marker of the
//! module's between its `[ RUN      ]` line and its end, and with its process not crashing after
//! it. The module's two markers override what gtest says: `yuna-wlcs: unsupported: <what>`, a
//! request Yuna refuses by design, makes the test unsupported; `yuna-wlcs: error: <what>`, the
//! module itself gone wrong, makes it a shim error. A test still running when its process
//! crashes crashed; a test running when its process went silent for 60 s hung (WLCS's own waits
//! give up after 10 s); when the process crashes between tests, the last test to end crashed,
//! and so when WLCS's process ends but a child of it holds its output open for 2 s more.
//! [`output`] gives the lines read.
//!
//! # The rules of `check`
//!
//! It fails (exit 1):
//!
//! - when a test the file expects to pass does not pass: it fails, skips, is unsupported,
//!   crashes, hangs or does not run;
//! - on any crash, hang or shim error, of any test, whatever the file expects of it, flaky
//!   included, and of a process that failed with no test to blame: before any of its tests began,
//!   WLCS's listing of its tests among them, or with the module's error marker outside any test;
//! - when the file names a test WLCS no longer lists, WLCS lists a test the file does not name,
//!   or the file's `wlcs` version is not `$YUNA_WLCS_VERSION` (judged before any test runs);
//! - when a test not expected to pass passes, then passes again alone in a fresh process: it
//!   passes now, and its line should say so. A single pass is only reported;
//! - when the run stops before its end: the sentinel, the breaker, or the deadline (`--deadline`,
//!   15 min). The breaker stops the run after 5 hangs or timeouts in a row, in the order the
//!   tests end across the jobs; a timeout of a test the file expects to fail or to be flaky does
//!   not count, and breaks the row as any other end does.
//!
//! It reports, without failing: each flaky test's outcome, never judged; other changes, such as
//! a test expected to fail that skips; a test not expected to pass that did not run.
//!
//! A timeout is a test failed by WLCS's own wait, `Timeout waiting for condition`. A test known
//! to fail by timing out is no sign of a broken build, hence the breaker's exception. In a
//! record, where nothing is expected yet, only hangs count. The summary at the end gives the
//! counts of each outcome, then every failure with its rule.
//!
//! Exit status: 0 passed; 1 a rule failed, a crash or a hang of WLCS while it lists its tests
//! among them; 2 the run could not judge: no runner, no module, the file unreadable or out of its
//! format, WLCS's list of tests unreadable or its runner exiting with an error.
//!
//! # The file of expected outcomes
//!
//! Plain text, one fact per line ([`expected`]):
//!
//! ```text
//! # Comment lines start with #; empty lines are ignored.
//! format 1
//! wlcs 1.8.1
//! fail XdgToplevelStableTest.wm_capabilities_are_sent
//! flaky XdgShellStableSubsurfaces/SubsurfaceTest.place_above_simple/0 # varied: pass 4, fail 1
//! pass SelfTest.when_creating_second_client_nothing_bad_happens
//! ```
//!
//! `format 1` comes first, then `wlcs <version>`, the WLCS the file was recorded with. Then one
//! line per test WLCS lists, sorted by name, each named once: `<outcome> <full gtest name>`, the
//! outcome one of `pass`, `fail`, `skip`, `unsupported` and `flaky`. A flaky line, and no other,
//! carries ` # <reason>`. The disabled tests, `DISABLED_…`, which WLCS lists but never runs, have
//! no line.
//!
//! # The record
//!
//! JSON, `format` 1 ([`record::RecordFile`]): `wlcs`, `module`, `commit`, `runs` (asked of each
//! test), `started_utc`, `written_utc`, `stopped` (why it stopped early, or null), `failures` (the
//! processes that failed with no test to blame), and `tests`, each test's name with the outcome of
//! each of its runs: `"Suite.test": ["pass", "pass", "fail"]`, an outcome one of `pass`, `fail`,
//! `skip`, `unsupported`, `shim-error`, `crash` and `hang`. The merge gives a test with one
//! outcome over every run of every record that outcome, and one whose outcome varied
//! `flaky # varied: <counts>`. It writes nothing, naming every reason, when a test crashed, hung
//! or met a shim error in any run, or never ran, when a record stopped or holds a failure, or
//! when the records are of other WLCS versions or other tests. A record whose listing of the
//! tests crashed or hung is not written.
//!
//! # The processes
//!
//! Each WLCS process runs in its own process group, killed whole on a hang or a stop, never by
//! name, and dies with the runner if the runner is killed or stopped by Ctrl-C; without
//! `WAYLAND_DISPLAY` and `WAYLAND_SOCKET`; with `LP_NUM_THREADS=0` and Mesa's shader cache in the
//! run's work folder, under the target folder unless `--work` names one. Its standard output and
//! error are one pipe, so that the module's markers fall between a test's start and its end; a
//! child of its own that holds the pipe open 2 s after it ended is killed with its group. Each
//! test's outcome is printed as it ends, so a run killed from outside still shows what ran. Each
//! process's output is kept in the work folder's `logs/`. A run that passes removes the work
//! folder it made; a folder `--work` names is never removed. [`running`] gives the detail.

pub mod expected;
pub mod judge;
pub mod listing;
pub mod output;
pub mod record;
pub mod running;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime};

use expected::{Expectation, ExpectedFile};
use judge::{Finding, Inputs, Mode};
use listing::Listing;
use output::Seen;
use running::{
    Batch, ListFailure, ProcessFailure, Settings, Stop, Supervisor, TestRun, Verdict, Watch,
};

const HELP: &str = "\
yuna-bench wlcs: WLCS, the Wayland conformance suite, run against Yuna's integration module
(crates/yuna-wlcs), each test judged against the file of its expected outcomes.

  yuna-bench wlcs check --listed [options]
      the tests the file expects to pass, alone: scripts/check.sh's, fast
  yuna-bench wlcs check --full [options]
      every test: CI's own job. A sentinel first (SelfTest, FrameSubmission and
      XdgSurfaceStableTest): a test expected to pass among them that fails stops the run at once.
      Then one process per suite, --jobs at once; then each test not expected to pass that passed,
      again alone
  yuna-bench wlcs record --runs <n> --out <file.json> [options]
      every test, each in its own process, n times: the outcomes seen, to merge
  yuna-bench wlcs record --merge <a.json> <b.json>... --write <expected.txt>
      the file of expected outcomes from records: a test with one outcome in every run gets it,
      one that varied is flaky; a test that crashed, hung or met a shim error anywhere writes
      nothing

Options:
  --module <path>     the integration module (default: $CARGO_TARGET_DIR, or target, then
                      debug/libyuna_wlcs.so)
  --expected <path>   check: the file of expected outcomes (default: crates/yuna-wlcs/expected.txt)
  --runner <path>     WLCS's runner (default: $YUNA_WLCS_RUNNER, which the development shell sets)
  --jobs <n>          processes at once (default: the processors, 4 at most)
  --deadline <min>    check: the run stops and fails after this many minutes (default: 15)
  --work <folder>     the processes' logs and Mesa's shader cache (default: a new folder in the
                      target folder's wlcs/, removed after a run that passes; a folder given here
                      is never removed)

check fails when a test expected to pass does not; on any crash, hang or shim error (the module's
line 'yuna-wlcs: error: ...') of any test, whatever the file expects of it; when the file and
WLCS's list differ, or the file's wlcs line is not $YUNA_WLCS_VERSION; when a test not expected
to pass passes twice, the second time alone; when the run stops: the sentinel, the breaker, the
deadline. The breaker stops the run after 5 hangs or timeouts in a row; a timeout of a test the
file expects to fail or to be flaky does not count, and breaks the row. Flaky tests and other changes are reported,
not judged. crates/yuna-bench/src/wlcs/mod.rs gives the rules and the files' formats.

Exit status: 0 passed; 1 a rule failed (a crash of WLCS's listing among them); 2 the run could
not judge (no runner, no module, a file unreadable).
Run it in the development shell: nix develop -c cargo run -p yuna-bench -- wlcs check --listed
";

/// The suites of `check --full`'s sentinel, run first.
pub const SENTINEL_SUITES: [&str; 3] = ["SelfTest", "FrameSubmission", "XdgSurfaceStableTest"];

/// The deadline of `check`, unless `--deadline` gives one.
pub const DEADLINE: Duration = Duration::from_secs(15 * 60);

/// What the command line reads of its environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Environment {
    /// `YUNA_WLCS_RUNNER`: WLCS's runner.
    pub runner: Option<String>,
    /// `YUNA_WLCS_VERSION`: its version.
    pub version: Option<String>,
    /// `CARGO_TARGET_DIR`: cargo's target folder.
    pub target: Option<String>,
}

impl Environment {
    /// This process's.
    pub fn read() -> Environment {
        let read = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Environment {
            runner: read("YUNA_WLCS_RUNNER"),
            version: read("YUNA_WLCS_VERSION"),
            target: read("CARGO_TARGET_DIR"),
        }
    }
}

/// How the WLCS processes of a check or a record run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    /// WLCS's runner.
    pub runner: PathBuf,
    /// The integration module.
    pub module: PathBuf,
    /// WLCS's version: `$YUNA_WLCS_VERSION`.
    pub version: String,
    /// Processes at once.
    pub jobs: usize,
    /// The hang guard's time: 60 s, unless a test shortens it (`--hang-ms`, not in the help).
    pub hang: Duration,
    /// The run's work folder.
    pub work: PathBuf,
    /// Whether the program named the work folder, and may remove it after a run that passes.
    pub own_work: bool,
}

impl RunOptions {
    /// The processes' settings, with `deadline`.
    fn settings(&self, deadline: Option<Duration>) -> Settings {
        Settings {
            runner: self.runner.clone(),
            module: self.module.clone(),
            work: self.work.clone(),
            jobs: self.jobs,
            hang: self.hang,
            deadline,
        }
    }

    /// The same, once the runner and the module are found, the module by its absolute path.
    fn found(&self) -> Result<RunOptions, String> {
        if !self.runner.is_file() {
            return Err(format!(
                "no WLCS runner at {}: run in the development shell, which sets \
                 YUNA_WLCS_RUNNER, or give --runner",
                self.runner.display()
            ));
        }
        let module = fs::canonicalize(&self.module)
            .ok()
            .filter(|module| module.is_file())
            .ok_or_else(|| {
                format!(
                    "no integration module at {}: build it, cargo build -p yuna-wlcs, or give \
                     --module",
                    self.module.display()
                )
            })?;
        Ok(RunOptions {
            module,
            ..self.clone()
        })
    }
}

/// `wlcs check`'s options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOptions {
    /// Which tests it runs.
    pub mode: Mode,
    /// The file of expected outcomes.
    pub expected: PathBuf,
    /// The deadline.
    pub deadline: Duration,
    /// How its processes run.
    pub run: RunOptions,
}

/// A command of `yuna-bench wlcs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `--help`.
    Help,
    /// `check`.
    Check(CheckOptions),
    /// `record --runs <n> --out <file>`.
    Record {
        /// How its processes run.
        run: RunOptions,
        /// Each test's runs.
        runs: u32,
        /// The record's path.
        out: PathBuf,
    },
    /// `record --merge <a.json>… --write <file>`.
    Merge {
        /// The records.
        inputs: Vec<PathBuf>,
        /// The file of expected outcomes to write.
        write: PathBuf,
    },
}

/// The options as given, before each command takes its own.
#[derive(Debug, Default)]
struct Flags {
    help: bool,
    listed: bool,
    full: bool,
    module: Option<PathBuf>,
    expected: Option<PathBuf>,
    runner: Option<PathBuf>,
    jobs: Option<usize>,
    deadline: Option<Duration>,
    work: Option<PathBuf>,
    hang: Option<Duration>,
    runs: Option<u32>,
    out: Option<PathBuf>,
    merge: Option<Vec<PathBuf>>,
    write: Option<PathBuf>,
}

impl Flags {
    /// The options given, by name, of those `allowed` leaves out: refused.
    fn refuse_others(&self, command: &str, allowed: &[&str]) -> Result<(), String> {
        let given = [
            ("--listed", self.listed),
            ("--full", self.full),
            ("--module", self.module.is_some()),
            ("--expected", self.expected.is_some()),
            ("--runner", self.runner.is_some()),
            ("--jobs", self.jobs.is_some()),
            ("--deadline", self.deadline.is_some()),
            ("--work", self.work.is_some()),
            ("--hang-ms", self.hang.is_some()),
            ("--runs", self.runs.is_some()),
            ("--out", self.out.is_some()),
            ("--merge", self.merge.is_some()),
            ("--write", self.write.is_some()),
        ];
        match given
            .iter()
            .find(|(name, given)| *given && !allowed.contains(name))
        {
            Some((name, _)) => Err(format!("{name} is not an option of {command}; see --help")),
            None => Ok(()),
        }
    }
}

/// The value after `flag`.
fn value<'a>(
    arguments: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<String, String> {
    arguments
        .next()
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// A whole number above 0, the value of `flag`.
fn above_zero<T: std::str::FromStr + PartialOrd + Default>(
    text: &str,
    flag: &str,
) -> Result<T, String> {
    text.parse()
        .ok()
        .filter(|n| *n > T::default())
        .ok_or_else(|| format!("{flag} takes a whole number above 0"))
}

fn read_flags(arguments: &[String]) -> Result<Flags, String> {
    let mut flags = Flags::default();
    let mut arguments = arguments.iter().peekable();
    while let Some(argument) = arguments.next() {
        let flag = argument.as_str();
        match flag {
            "-h" | "--help" => flags.help = true,
            "--listed" => flags.listed = true,
            "--full" => flags.full = true,
            "--module" => flags.module = Some(value(&mut arguments, flag)?.into()),
            "--expected" => flags.expected = Some(value(&mut arguments, flag)?.into()),
            "--runner" => flags.runner = Some(value(&mut arguments, flag)?.into()),
            "--work" => flags.work = Some(value(&mut arguments, flag)?.into()),
            "--out" => flags.out = Some(value(&mut arguments, flag)?.into()),
            "--write" => flags.write = Some(value(&mut arguments, flag)?.into()),
            "--jobs" => flags.jobs = Some(above_zero(&value(&mut arguments, flag)?, flag)?),
            "--runs" => flags.runs = Some(above_zero(&value(&mut arguments, flag)?, flag)?),
            "--hang-ms" => {
                let ms: u64 = above_zero(&value(&mut arguments, flag)?, flag)?;
                if ms > 86_400_000 {
                    return Err("--hang-ms takes a day at most".to_owned());
                }
                flags.hang = Some(Duration::from_millis(ms));
            }
            "--deadline" => {
                let minutes = value(&mut arguments, flag)?
                    .parse::<f64>()
                    .ok()
                    .filter(|m| m.is_finite() && *m > 0.0 && *m < 1e6)
                    .ok_or("--deadline takes minutes, a number above 0")?;
                flags.deadline = Some(Duration::from_secs_f64(minutes * 60.0));
            }
            "--merge" => {
                let mut inputs = Vec::new();
                while let Some(path) = arguments.next_if(|a| !a.starts_with("--")) {
                    inputs.push(PathBuf::from(path));
                }
                if inputs.is_empty() {
                    return Err("--merge needs one record at least".to_owned());
                }
                flags.merge = Some(inputs);
            }
            other => return Err(format!("unknown argument {other:?}; see --help")),
        }
    }
    Ok(flags)
}

/// The cargo target folder: `$CARGO_TARGET_DIR`, or the workspace's `target`.
fn target(environment: &Environment) -> PathBuf {
    environment.target.as_ref().map_or_else(
        || crate::scenarios::workspace().join("target"),
        PathBuf::from,
    )
}

/// The options shared by `check` and `record`.
fn run_options(
    flags: &Flags,
    environment: &Environment,
    command: &str,
) -> Result<RunOptions, String> {
    let runner = flags
        .runner
        .clone()
        .or_else(|| environment.runner.as_ref().map(PathBuf::from))
        .ok_or(
            "no WLCS runner: run in the development shell, which sets YUNA_WLCS_RUNNER, or give \
             --runner",
        )?;
    let version = environment.version.clone().ok_or(
        "YUNA_WLCS_VERSION is not set: run in the development shell, which names WLCS's version",
    )?;
    let target = target(environment);
    let (work, own_work) = match &flags.work {
        Some(work) => (work.clone(), false),
        None => {
            let stamp = crate::cli::utc(SystemTime::now()).replace([':', '-'], "");
            let name = format!("{command}-{stamp}-{}", std::process::id());
            (target.join("wlcs").join(name), true)
        }
    };
    Ok(RunOptions {
        runner,
        module: flags
            .module
            .clone()
            .unwrap_or_else(|| target.join("debug/libyuna_wlcs.so")),
        version,
        jobs: flags
            .jobs
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get().min(4))),
        hang: flags.hang.unwrap_or(running::HANG),
        work,
        own_work,
    })
}

/// Reads the command line, `wlcs` left out; `environment` gives the defaults.
pub fn read_options(arguments: &[String], environment: &Environment) -> Result<Command, String> {
    let Some((command, rest)) = arguments.split_first() else {
        return Ok(Command::Help);
    };
    let flags = read_flags(rest)?;
    if flags.help {
        return Ok(Command::Help);
    }
    match command.as_str() {
        "-h" | "--help" => Ok(Command::Help),
        "check" => {
            flags.refuse_others(
                "check",
                &[
                    "--listed",
                    "--full",
                    "--module",
                    "--expected",
                    "--runner",
                    "--jobs",
                    "--deadline",
                    "--work",
                    "--hang-ms",
                ],
            )?;
            let mode = match (flags.listed, flags.full) {
                (true, false) => Mode::Listed,
                (false, true) => Mode::Full,
                _ => return Err("check takes --listed or --full".to_owned()),
            };
            Ok(Command::Check(CheckOptions {
                mode,
                expected: flags.expected.clone().unwrap_or_else(|| {
                    crate::scenarios::workspace().join("crates/yuna-wlcs/expected.txt")
                }),
                deadline: flags.deadline.unwrap_or(DEADLINE),
                run: run_options(&flags, environment, "check")?,
            }))
        }
        "record" if flags.merge.is_some() => {
            flags.refuse_others("record --merge", &["--merge", "--write"])?;
            Ok(Command::Merge {
                inputs: flags.merge.clone().unwrap_or_default(),
                write: flags
                    .write
                    .clone()
                    .ok_or("record --merge needs --write <expected.txt>")?,
            })
        }
        "record" => {
            flags.refuse_others(
                "record",
                &[
                    "--module",
                    "--runner",
                    "--jobs",
                    "--work",
                    "--hang-ms",
                    "--runs",
                    "--out",
                ],
            )?;
            Ok(Command::Record {
                runs: flags.runs.ok_or("record needs --runs <n>")?,
                out: flags.out.clone().ok_or("record needs --out <file.json>")?,
                run: run_options(&flags, environment, "record")?,
            })
        }
        other => Err(format!("no command {other:?}: check or record; see --help")),
    }
}

/// `yuna-bench wlcs`: runs the command line `arguments`, `wlcs` left out.
pub fn main(arguments: &[String]) -> ExitCode {
    // First, before any thread or child: no descriptor this process inherited may reach a WLCS
    // process, which would hold it for as long as it lives.
    let inherited = crate::sandbox::close_inherited_on_exec();
    if !inherited.is_empty() {
        eprintln!("yuna-bench wlcs: descriptors inherited, kept from every child: {inherited:?}");
    }
    ExitCode::from(run(arguments, &Environment::read()))
}

/// Runs the command line `arguments` with `environment`; gives the exit status.
pub fn run(arguments: &[String], environment: &Environment) -> u8 {
    let result = read_options(arguments, environment).and_then(|command| match command {
        Command::Help => {
            print!("{HELP}");
            Ok(0)
        }
        Command::Check(options) => check(&options).map(|checked| checked.exit),
        Command::Record { run, runs, out } => record::record(&run.found()?, runs, &out),
        Command::Merge { inputs, write } => record::merge_files(&inputs, &write),
    });
    result.unwrap_or_else(|error| {
        eprintln!("yuna-bench wlcs: {error}");
        2
    })
}

/// A length of time in words: `4.2 s`, `3 min 12 s`.
pub(crate) fn duration(length: Duration) -> String {
    let seconds = length.as_secs();
    match (seconds / 60, seconds % 60) {
        (0, _) => format!("{:.1} s", length.as_secs_f64()),
        (minutes, 0) => format!("{minutes} min"),
        (minutes, seconds) => format!("{minutes} min {seconds} s"),
    }
}

/// `counts` of each outcome, in [`Seen::ALL`]'s order: `pass 3, fail 1`.
pub(crate) fn counts_text(counts: &BTreeMap<Seen, usize>) -> String {
    Seen::ALL
        .iter()
        .filter_map(|seen| counts.get(seen).map(|n| format!("{} {n}", seen.name())))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Prints one run's line, as it ends: its outcome, its label, and in brackets what was expected
/// when it differs and what the outcome alone does not say.
pub(crate) fn print_line(seen: Seen, label: &str, note: Option<&str>, expected: Option<&str>) {
    let extra: Vec<&str> = expected.into_iter().chain(note).collect();
    if extra.is_empty() {
        println!("{:<11} {label}", seen.name());
    } else {
        println!("{:<11} {label}  ({})", seen.name(), extra.join("; "));
    }
}

/// Removes a run's work folder after it passed, if the program named it.
fn remove_work(options: &RunOptions) {
    if options.own_work
        && let Err(error) = fs::remove_dir_all(&options.work)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("yuna-bench wlcs: {}: {error}", options.work.display());
    }
}

/// What `check` does as each test ends: prints it, says whether it counts toward the breaker,
/// and, in the sentinel, stops the run when a test expected to pass does not.
struct CheckWatch<'a> {
    expected: &'a ExpectedFile,
    sentinel: bool,
    alone: bool,
}

impl Watch for CheckWatch<'_> {
    fn ended(&self, run: &TestRun) -> Verdict {
        let name = &run.ended.name;
        let seen = run.ended.seen;
        let expectation = self.expected.tests.get(name);
        let expected = match expectation {
            Some(expectation) if expectation.is(seen) => None,
            Some(expectation) => Some(format!("expected {}", expectation.name())),
            None => Some("not in the file".to_owned()),
        };
        let label = if self.alone {
            format!("{name} alone")
        } else {
            name.clone()
        };
        print_line(seen, &label, run.ended.note.as_deref(), expected.as_deref());
        let expected_to_fail =
            matches!(expectation, Some(Expectation::Fail | Expectation::Flaky(_)));
        let broad = self.sentinel
            && expectation == Some(&Expectation::Pass)
            && !matches!(seen, Seen::Pass | Seen::NotRun);
        Verdict {
            toward_breaker: seen == Seen::Hang || (run.ended.timed_out && !expected_to_fail),
            stop: broad.then(|| {
                Stop::Sentinel(format!(
                    "{name}, expected to pass, saw {}: a broad failure, the other tests not run",
                    seen.name()
                ))
            }),
        }
    }

    fn failed(&self, failure: &ProcessFailure) {
        print_line(failure.seen, &failure.batch, Some(&failure.text), None);
    }
}

/// What a check gave.
#[derive(Debug)]
pub struct Checked {
    /// Its exit status: 0 passed, 1 a rule failed.
    pub exit: u8,
    /// Every finding.
    pub findings: Vec<Finding>,
    /// Each test's run in its batch.
    pub runs: BTreeMap<String, TestRun>,
    /// The runs again alone.
    pub alone: BTreeMap<String, TestRun>,
}

/// Sorts `batches` the largest first, so that the jobs end closer together.
fn largest_first(mut batches: Vec<Batch>) -> Vec<Batch> {
    batches.sort_by_key(|batch| std::cmp::Reverse(batch.tests.len()));
    batches
}

/// `check --full`'s batches: the sentinel, if WLCS lists any of its suites, then one per suite.
fn full_batches(listing: &Listing) -> (Option<Batch>, Vec<Batch>) {
    let mut sentinel = Batch {
        label: "sentinel".to_owned(),
        patterns: Vec::new(),
        tests: Vec::new(),
    };
    let mut batches = Vec::new();
    for (suite, names) in listing.suites() {
        let names = names.into_iter().map(str::to_owned);
        if SENTINEL_SUITES.contains(&suite) {
            sentinel.patterns.push(format!("{suite}.*"));
            sentinel.tests.extend(names);
        } else {
            batches.push(Batch {
                label: suite.to_owned(),
                patterns: vec![format!("{suite}.*")],
                tests: names.collect(),
            });
        }
    }
    let sentinel = (!sentinel.tests.is_empty()).then_some(sentinel);
    (sentinel, largest_first(batches))
}

/// `check --listed`'s batches: one per suite, of its tests the file expects to pass, each named.
fn listed_batches(listing: &Listing, expected: &ExpectedFile) -> Vec<Batch> {
    let batches = listing
        .suites()
        .into_iter()
        .filter_map(|(suite, names)| {
            let passing: Vec<String> = names
                .into_iter()
                .filter(|name| expected.tests.get(*name) == Some(&Expectation::Pass))
                .map(str::to_owned)
                .collect();
            (!passing.is_empty()).then(|| Batch {
                label: suite.to_owned(),
                patterns: passing.clone(),
                tests: passing,
            })
        })
        .collect();
    largest_first(batches)
}

/// Prints the summary ([`summary`]).
fn print_summary(
    runs: &BTreeMap<String, TestRun>,
    findings: &[Finding],
    elapsed: Duration,
    logs: Option<&Path>,
) {
    print!("{}", summary(runs, findings, elapsed, logs));
}

/// The summary: the counts of each outcome, every failure with its rule, then what is only
/// reported, and where the logs are when the run failed and has some.
fn summary(
    runs: &BTreeMap<String, TestRun>,
    findings: &[Finding],
    elapsed: Duration,
    logs: Option<&Path>,
) -> String {
    let mut counts: BTreeMap<Seen, usize> = BTreeMap::new();
    for run in runs.values() {
        *counts.entry(run.ended.seen).or_default() += 1;
    }
    let mut text = format!(
        "\nyuna-bench wlcs: {} tests in {}: {}\n",
        runs.len(),
        duration(elapsed),
        if counts.is_empty() {
            "none ran".to_owned()
        } else {
            counts_text(&counts)
        }
    );
    let line = |finding: &Finding| match &finding.test {
        Some(test) => format!("  [{}] {test}: {}\n", finding.rule.name(), finding.text),
        None => format!("  [{}] {}\n", finding.rule.name(), finding.text),
    };
    let failures: Vec<&Finding> = findings.iter().filter(|f| f.rule.judged()).collect();
    let reported: Vec<&Finding> = findings.iter().filter(|f| !f.rule.judged()).collect();
    if !failures.is_empty() {
        text.push_str("Failures, each with its rule:\n");
        for finding in &failures {
            text.push_str(&line(finding));
        }
    }
    if !reported.is_empty() {
        text.push_str("Reported, not judged:\n");
        for finding in &reported {
            text.push_str(&line(finding));
        }
    }
    text.push_str(&match (failures.len(), logs) {
        (0, _) => "Passed.\n".to_owned(),
        (failed, Some(logs)) => format!(
            "Failed: {failed} failures. The logs are kept in {}.\n",
            logs.display()
        ),
        (failed, None) => format!("Failed: {failed} failures.\n"),
    });
    text
}

/// `wlcs check`: reads the file, lists WLCS's tests, runs them as the mode says, judges them.
/// An error is a run that could not judge.
pub fn check(options: &CheckOptions) -> Result<Checked, String> {
    let started = Instant::now();
    let path = options.expected.display().to_string();
    let text = fs::read_to_string(&options.expected).map_err(|e| format!("{path}: {e}"))?;
    let expected = expected::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    let run = options.run.found()?;
    let mode = match options.mode {
        Mode::Listed => "--listed",
        Mode::Full => "--full",
    };
    println!(
        "yuna-bench wlcs check {mode}: WLCS {}, {} at once; module {}; expected {path}",
        run.version,
        run.jobs,
        run.module.display()
    );
    if let Some(finding) = judge::version(&expected, &run.version) {
        let findings = vec![finding];
        print_summary(&BTreeMap::new(), &findings, started.elapsed(), None);
        return Ok(Checked {
            exit: 1,
            findings,
            runs: BTreeMap::new(),
            alone: BTreeMap::new(),
        });
    }
    let supervisor = Supervisor::new(run.settings(Some(options.deadline)))?;
    let listing = match supervisor.list() {
        Ok(listing) => listing,
        Err(ListFailure::Unusable(text)) => return Err(text),
        Err(ListFailure::Broken(failure)) => {
            let findings = vec![judge::process_failure(&failure)];
            print_summary(
                &BTreeMap::new(),
                &findings,
                started.elapsed(),
                Some(&run.work),
            );
            return Ok(Checked {
                exit: 1,
                findings,
                runs: BTreeMap::new(),
                alone: BTreeMap::new(),
            });
        }
    };
    let listed: Vec<String> = listing.names().map(str::to_owned).collect();
    let mut runs = BTreeMap::new();
    let mut failures = Vec::new();
    let mut collect = |ran: running::Ran| {
        for test in ran.runs {
            runs.insert(test.ended.name.clone(), test);
        }
        failures.extend(ran.failures);
    };
    let watch = CheckWatch {
        expected: &expected,
        sentinel: false,
        alone: false,
    };
    match options.mode {
        Mode::Listed => collect(supervisor.run(listed_batches(&listing, &expected), &watch)),
        Mode::Full => {
            let (sentinel, batches) = full_batches(&listing);
            if let Some(sentinel) = sentinel {
                let watch = CheckWatch {
                    sentinel: true,
                    ..watch
                };
                collect(supervisor.run(vec![sentinel], &watch));
            }
            if supervisor.stop().is_none() {
                collect(supervisor.run(batches, &watch));
            }
        }
    }
    let mut alone = BTreeMap::new();
    let new_passes: Vec<String> = runs
        .values()
        .filter(|test| {
            test.ended.seen == Seen::Pass
                && matches!(
                    expected.tests.get(&test.ended.name),
                    Some(Expectation::Fail | Expectation::Skip | Expectation::Unsupported)
                )
        })
        .map(|test| test.ended.name.clone())
        .collect();
    if options.mode == Mode::Full && supervisor.stop().is_none() && !new_passes.is_empty() {
        println!(
            "yuna-bench wlcs: {} tests not expected to pass passed: each runs again alone",
            new_passes.len()
        );
        let batches = new_passes
            .iter()
            .map(|name| Batch {
                label: format!("{name} alone"),
                patterns: vec![name.clone()],
                tests: vec![name.clone()],
            })
            .collect();
        let watch = CheckWatch {
            alone: true,
            ..watch
        };
        let ran = supervisor.run(batches, &watch);
        for test in ran.runs {
            alone.insert(test.ended.name.clone(), test);
        }
        failures.extend(ran.failures);
    }
    let stop = supervisor.stop();
    let findings = judge::judge(&Inputs {
        mode: options.mode,
        expected: &expected,
        expected_path: &path,
        listed: &listed,
        runs: &runs,
        alone: &alone,
        failures: &failures,
        stop: stop.as_ref(),
    });
    print_summary(&runs, &findings, started.elapsed(), Some(&run.work));
    let failed = findings.iter().any(|f| f.rule.judged());
    if !failed {
        remove_work(&run);
    }
    Ok(Checked {
        exit: u8::from(failed),
        findings,
        runs,
        alone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use judge::Rule;

    /// The stand-in for WLCS's runner.
    const FAKE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/wlcs/fixtures/fake-wlcs.sh"
    );

    /// A test's folder: its scenario, in place of the module, and its file of expected outcomes.
    struct Case {
        folder: PathBuf,
        /// The hang guard's time: long enough that a slow start on a loaded machine is no hang,
        /// unless the test is of hangs.
        hang_ms: u32,
        /// Processes at once: one, unless the test is of jobs.
        jobs: usize,
    }

    impl Case {
        /// `scenario`'s lines are `<test> <behaviour>`; `expected`'s, the file's test lines.
        fn new(name: &str, scenario: &str, expected: &str) -> Case {
            let folder = crate::testing::folder(name);
            fs::write(folder.join("scenario"), scenario).unwrap();
            fs::write(
                folder.join("expected.txt"),
                format!("format 1\nwlcs 1.8.1\n{expected}"),
            )
            .unwrap();
            Case {
                folder,
                hang_ms: 5000,
                jobs: 1,
            }
        }

        /// The same, with a hang guard of 1 s, for a test of hangs.
        fn quick_hang(self) -> Case {
            Case {
                hang_ms: 1000,
                ..self
            }
        }

        /// The same, with `jobs` processes at once.
        fn jobs(self, jobs: usize) -> Case {
            Case { jobs, ..self }
        }

        /// `wlcs <command…>` with the fake runner, and the case's jobs and hang guard.
        fn arguments(&self, command: &[&str]) -> Vec<String> {
            let path = |name: &str| self.folder.join(name).display().to_string();
            let mut arguments: Vec<String> = command.iter().map(|a| (*a).to_owned()).collect();
            arguments.extend([
                "--runner".to_owned(),
                FAKE.to_owned(),
                "--module".to_owned(),
                path("scenario"),
                "--work".to_owned(),
                path("work"),
                "--jobs".to_owned(),
                self.jobs.to_string(),
                "--hang-ms".to_owned(),
                self.hang_ms.to_string(),
            ]);
            if command[0] == "check" {
                arguments.extend(["--expected".to_owned(), path("expected.txt")]);
            }
            arguments
        }

        fn check(&self, mode: &str, more: &[&str]) -> Checked {
            let mut command = vec!["check", mode];
            command.extend(more);
            match read_options(&self.arguments(&command), &environment()).unwrap() {
                Command::Check(options) => check(&options).unwrap(),
                other => panic!("not a check: {other:?}"),
            }
        }

        /// The fake runner's arguments, each call's, the module left out.
        fn invocations(&self) -> Vec<String> {
            fs::read_to_string(self.folder.join("invocations"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        /// The calls that ran tests: their filters.
        fn filters(&self) -> Vec<String> {
            self.invocations()
                .iter()
                .filter_map(|call| {
                    call.split(' ')
                        .find_map(|a| a.strip_prefix("--gtest_filter="))
                        .map(str::to_owned)
                })
                .collect()
        }
    }

    fn environment() -> Environment {
        Environment {
            runner: None,
            version: Some("1.8.1".to_owned()),
            target: None,
        }
    }

    fn seen(checked: &Checked, name: &str) -> Seen {
        checked
            .runs
            .get(name)
            .unwrap_or_else(|| panic!("no run of {name}: {:#?}", checked.runs))
            .ended
            .seen
    }

    fn rules(checked: &Checked) -> Vec<(Rule, Option<&str>)> {
        checked
            .findings
            .iter()
            .map(|f| (f.rule, f.test.as_deref()))
            .collect()
    }

    #[test]
    fn a_crash_resumes_its_batch_after_the_culprit() {
        let case = Case::new(
            "wlcs-crash",
            "Suite.a ok\nSuite.b crash\nSuite.c ok\nOther.a crash-after\nOther.b unsupported\n\
             Other.c ok\n",
            "pass Other.a\npass Other.b\npass Other.c\npass Suite.a\nfail Suite.b\npass Suite.c\n",
        );
        let checked = case.check("--full", &[]);
        assert_eq!(checked.exit, 1);
        assert_eq!(seen(&checked, "Suite.a"), Seen::Pass);
        assert_eq!(seen(&checked, "Suite.b"), Seen::Crash);
        assert_eq!(
            checked.runs["Suite.b"].ended.note.as_deref(),
            Some("signal 11, SIGSEGV")
        );
        assert_eq!(seen(&checked, "Suite.c"), Seen::Pass);
        // Its OK line printed, the test whose process crashed after it is no pass.
        assert_eq!(seen(&checked, "Other.a"), Seen::Crash);
        // The module's marker came through standard error, between its test's start and end.
        assert_eq!(seen(&checked, "Other.b"), Seen::Unsupported);
        assert_eq!(seen(&checked, "Other.c"), Seen::Pass);
        assert_eq!(
            case.filters(),
            [
                "Suite.*",
                "Suite.*-Suite.a:Suite.b",
                "Other.*",
                "Other.*-Other.a"
            ]
        );
        assert_eq!(
            rules(&checked),
            [
                (Rule::Crash, Some("Other.a")),
                (Rule::Pass, Some("Other.b")),
                (Rule::Crash, Some("Suite.b")),
            ]
        );
        // A failed run keeps its logs.
        assert!(case.folder.join("work/logs").is_dir());
    }

    #[test]
    fn the_hang_guard_kills_a_silent_test_and_resumes() {
        let case = Case::new(
            "wlcs-hang",
            "Suite.a ok\nSuite.b hang\nSuite.c ok\n",
            "pass Suite.a\nfail Suite.b\npass Suite.c\n",
        )
        .quick_hang();
        let started = Instant::now();
        let checked = case.check("--full", &[]);
        assert_eq!(checked.exit, 1);
        assert_eq!(seen(&checked, "Suite.a"), Seen::Pass);
        assert_eq!(seen(&checked, "Suite.b"), Seen::Hang);
        // The batch resumed: the group's sleeping child died with it, or the pipe would never
        // have ended and the process been left behind.
        assert_eq!(seen(&checked, "Suite.c"), Seen::Pass);
        assert_eq!(case.filters(), ["Suite.*", "Suite.*-Suite.a:Suite.b"]);
        assert_eq!(rules(&checked), [(Rule::Hang, Some("Suite.b"))]);
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_breaker_stops_after_five_hangs_or_timeouts_in_a_row() {
        // A timeout of a test expected to fail breaks the streak; one expected to pass counts.
        let case = Case::new(
            "wlcs-breaker",
            "Suite.a hang\nSuite.b timeout\nSuite.c hang\nSuite.d timeout\nSuite.e hang\n\
             Suite.f ok\nStreak.a hang\nStreak.b timeout\n",
            "pass Streak.a\nfail Streak.b\npass Suite.a\npass Suite.b\npass Suite.c\n\
             pass Suite.d\npass Suite.e\npass Suite.f\n",
        )
        .quick_hang();
        let checked = case.check("--full", &[]);
        assert_eq!(checked.exit, 1);
        assert_eq!(seen(&checked, "Suite.e"), Seen::Hang);
        assert_eq!(seen(&checked, "Suite.f"), Seen::NotRun);
        assert_eq!(
            checked.findings[0].text,
            "the breaker: 5 hangs or timeouts in a row: Suite.a, Suite.b, Suite.c, Suite.d, \
             Suite.e; 2 tests expected to pass did not run"
        );
        assert_eq!(checked.findings[0].rule, Rule::Stopped);
        // The largest batch first, then the next, which the stop leaves unrun.
        assert_eq!(seen(&checked, "Streak.a"), Seen::NotRun);
        assert!(!case.filters().iter().any(|f| f.starts_with("Streak")));

        // The same streak broken by an expected failure's timeout: no stop.
        let case = Case::new(
            "wlcs-breaker-broken",
            "Suite.a hang\nSuite.b hang\nSuite.c timeout\nSuite.d hang\nSuite.e hang\n\
             Suite.f ok\n",
            "pass Suite.a\npass Suite.b\nfail Suite.c\npass Suite.d\npass Suite.e\npass Suite.f\n",
        )
        .quick_hang();
        let checked = case.check("--full", &[]);
        assert_eq!(seen(&checked, "Suite.f"), Seen::Pass);
        assert!(!rules(&checked).contains(&(Rule::Stopped, None)));
    }

    #[test]
    fn the_sentinel_stops_the_run_at_once_on_a_broad_failure() {
        let case = Case::new(
            "wlcs-sentinel",
            "SelfTest.a ok\nSelfTest.b fail\nSelfTest.c ok\nXdgSurfaceStableTest.a ok\n\
             Suite.a ok\n",
            "pass SelfTest.a\npass SelfTest.b\npass SelfTest.c\npass Suite.a\n\
             pass XdgSurfaceStableTest.a\n",
        );
        let checked = case.check("--full", &[]);
        assert_eq!(checked.exit, 1);
        assert_eq!(seen(&checked, "SelfTest.b"), Seen::Fail);
        assert!(!checked.runs.contains_key("Suite.a"), "its batch never ran");
        assert_eq!(case.filters(), ["SelfTest.*:XdgSurfaceStableTest.*"]);
        assert_eq!(checked.findings[0].rule, Rule::Stopped);
        assert!(
            checked.findings[0].text.starts_with(
                "the sentinel: SelfTest.b, expected to pass, saw fail: a broad failure"
            ),
            "{}",
            checked.findings[0].text
        );
        // A sentinel whose failing test was expected to fail goes on to the batches.
        let case = Case::new(
            "wlcs-sentinel-expected",
            "SelfTest.a ok\nSelfTest.b fail\nSuite.a ok\n",
            "pass SelfTest.a\nfail SelfTest.b\npass Suite.a\n",
        );
        let checked = case.check("--full", &[]);
        assert_eq!(checked.exit, 0, "{:?}", checked.findings);
        assert_eq!(case.filters(), ["SelfTest.*", "Suite.*"]);
        // A run that passed removes no folder it did not name.
        assert!(case.folder.join("work/logs").is_dir());
    }

    #[test]
    fn a_new_pass_fails_once_it_passes_again_alone() {
        let case = Case::new(
            "wlcs-new-pass",
            "Suite.a ok\nSuite.b pass-first\nSuite.c fail\n",
            "fail Suite.a\nskip Suite.b\nfail Suite.c\n",
        );
        let checked = case.check("--full", &[]);
        assert_eq!(checked.exit, 1);
        assert_eq!(checked.alone["Suite.a"].ended.seen, Seen::Pass);
        assert_eq!(checked.alone["Suite.b"].ended.seen, Seen::Fail);
        assert!(!checked.alone.contains_key("Suite.c"));
        assert_eq!(case.filters(), ["Suite.*", "Suite.a", "Suite.b"]);
        assert_eq!(
            rules(&checked),
            [
                (Rule::PassesNow, Some("Suite.a")),
                (Rule::PassedOnce, Some("Suite.b"))
            ]
        );
        // A single pass alone is only reported.
        let case = Case::new(
            "wlcs-single-pass",
            "Suite.b pass-first\n",
            "unsupported Suite.b\n",
        );
        let checked = case.check("--full", &[]);
        assert_eq!(checked.exit, 0);
        assert_eq!(rules(&checked), [(Rule::PassedOnce, Some("Suite.b"))]);
    }

    #[test]
    fn listed_runs_the_pass_tests_alone_and_checks_the_file_against_the_list() {
        let case = Case::new(
            "wlcs-listed",
            "Suite.a ok\nSuite.b fail\nSuite.DISABLED_c ok\nOther/Param.a/0 ok\nOther/Param.a/1 \
             skip\n",
            "pass Other/Param.a/0\nskip Other/Param.a/1\npass Suite.a\nfail Suite.b\n",
        );
        let checked = case.check("--listed", &[]);
        assert_eq!(checked.exit, 0, "{:?}", checked.findings);
        assert_eq!(case.filters(), ["Suite.a", "Other/Param.a/0"]);
        assert_eq!(checked.runs.len(), 2);
        // A run that passed in its own work folder removes it.
        let mut arguments = case.arguments(&["check", "--listed"]);
        let work = arguments.iter().position(|a| a == "--work").unwrap();
        arguments.drain(work..work + 2);
        let target = case.folder.join("target");
        let environment = Environment {
            target: Some(target.display().to_string()),
            ..environment()
        };
        assert_eq!(run(&arguments, &environment), 0);
        assert_eq!(fs::read_dir(target.join("wlcs")).unwrap().count(), 0);

        // A test gone from WLCS's list, one the file lacks.
        let case = Case::new(
            "wlcs-listed-differ",
            "Suite.a ok\nSuite.new fail\n",
            "pass Suite.a\npass Suite.gone\n",
        );
        let checked = case.check("--listed", &[]);
        assert_eq!(checked.exit, 1);
        assert_eq!(
            rules(&checked),
            [
                (Rule::Gone, Some("Suite.gone")),
                (Rule::Unlisted, Some("Suite.new"))
            ]
        );
    }

    #[test]
    fn the_deadline_stops_and_fails_the_run() {
        let case = Case::new(
            "wlcs-deadline",
            "Suite.a ok\nSuite.b hang\nSuite.c ok\n",
            "pass Suite.a\npass Suite.b\npass Suite.c\n",
        );
        let mut command = case.arguments(&["check", "--full", "--deadline", "0.01"]);
        let hang = command.iter().position(|a| a == "--hang-ms").unwrap();
        command[hang + 1] = "30000".to_owned();
        let Command::Check(options) = read_options(&command, &environment()).unwrap() else {
            panic!("not a check");
        };
        let started = Instant::now();
        let checked = check(&options).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(checked.exit, 1);
        assert_eq!(seen(&checked, "Suite.a"), Seen::Pass);
        assert_eq!(seen(&checked, "Suite.b"), Seen::NotRun);
        assert_eq!(checked.findings[0].rule, Rule::Stopped);
        assert!(
            checked.findings[0]
                .text
                .starts_with("the deadline: 0.6 s passed"),
            "{}",
            checked.findings[0].text
        );
    }

    #[test]
    fn a_process_that_never_stops_printing_meets_the_deadline_too() {
        let case = Case::new("wlcs-spam", "Suite.a spam\n", "pass Suite.a\n");
        let started = Instant::now();
        let checked = case.check("--full", &["--deadline", "0.01"]);
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(checked.exit, 1);
        assert_eq!(seen(&checked, "Suite.a"), Seen::NotRun);
        assert_eq!(checked.findings[0].rule, Rule::Stopped);
    }

    #[test]
    fn a_run_that_cannot_judge_exits_2_and_a_version_mismatch_1() {
        let case = Case::new("wlcs-cannot", "Suite.a ok\n", "pass Suite.a\n");
        let arguments = case.arguments(&["check", "--listed"]);
        let with = |flag: &str, value: &str| {
            let mut arguments = arguments.clone();
            let at = arguments.iter().position(|a| a == flag).unwrap();
            arguments[at + 1] = value.to_owned();
            arguments
        };
        let missing = case.folder.join("missing").display().to_string();
        assert_eq!(run(&with("--runner", &missing), &environment()), 2);
        assert_eq!(run(&with("--module", &missing), &environment()), 2);
        assert_eq!(run(&with("--expected", &missing), &environment()), 2);
        fs::write(
            case.folder.join("bad.txt"),
            "format 1\nwlcs 1.8.1\npass B\npass A\n",
        )
        .unwrap();
        let bad = case.folder.join("bad.txt").display().to_string();
        assert_eq!(run(&with("--expected", &bad), &environment()), 2);
        let unset = Environment {
            version: None,
            ..environment()
        };
        assert_eq!(run(&arguments, &unset), 2);
        assert!(case.invocations().is_empty());
        let other = Environment {
            version: Some("1.9.0".to_owned()),
            ..environment()
        };
        // Another WLCS: judged at once, before any test runs.
        assert_eq!(run(&arguments, &other), 1);
        assert!(case.invocations().is_empty());
        assert_eq!(run(&arguments, &environment()), 0);
        // A list of no test cannot be judged.
        fs::write(case.folder.join("scenario"), "").unwrap();
        assert_eq!(run(&arguments, &environment()), 2);
    }

    #[test]
    fn record_and_merge_write_the_file_of_expected_outcomes() {
        let case = Case::new(
            "wlcs-record",
            "Suite.a ok\nSuite.b fail\nSuite.c skip\nSuite.d unsupported\nSuite.e pass-first\n",
            "",
        );
        let out = case.folder.join("records/a.json");
        let mut arguments = case.arguments(&["record", "--runs", "2", "--out"]);
        arguments.insert(4, out.display().to_string());
        assert_eq!(run(&arguments, &environment()), 0);
        let record = record::read(&out).unwrap();
        assert_eq!(record.wlcs, "1.8.1");
        assert_eq!(record.runs, 2);
        assert_eq!(record.tests["Suite.a"], [Seen::Pass, Seen::Pass]);
        assert_eq!(
            record.tests["Suite.d"],
            [Seen::Unsupported, Seen::Unsupported]
        );
        assert_eq!(record.tests["Suite.e"], [Seen::Pass, Seen::Fail]);
        // Each test in its own process, twice.
        assert_eq!(case.filters().len(), 10);
        let written = case.folder.join("expected-new.txt");
        let merge = |inputs: &[&Path]| {
            let mut arguments = vec!["record".to_owned(), "--merge".to_owned()];
            arguments.extend(inputs.iter().map(|p| p.display().to_string()));
            arguments.extend(["--write".to_owned(), written.display().to_string()]);
            run(&arguments, &environment())
        };
        assert_eq!(merge(&[&out]), 0);
        let file = expected::parse(&fs::read_to_string(&written).unwrap()).unwrap();
        assert_eq!(
            file.tests,
            [
                ("Suite.a", Expectation::Pass),
                ("Suite.b", Expectation::Fail),
                ("Suite.c", Expectation::Skip),
                ("Suite.d", Expectation::Unsupported),
                (
                    "Suite.e",
                    Expectation::Flaky("varied: pass 1, fail 1".to_owned())
                ),
            ]
            .into_iter()
            .map(|(name, e)| (name.to_owned(), e))
            .collect()
        );

        // A crash in a record: the record says so, and the merge writes nothing.
        let crashing = Case::new("wlcs-record-crash", "Suite.a ok\nSuite.b crash\n", "");
        let out = crashing.folder.join("b.json");
        let mut arguments = crashing.arguments(&["record", "--runs", "1", "--out"]);
        arguments.insert(4, out.display().to_string());
        assert_eq!(run(&arguments, &environment()), 1);
        assert_eq!(record::read(&out).unwrap().tests["Suite.b"], [Seen::Crash]);
        fs::remove_file(&written).unwrap();
        assert_eq!(merge(&[&out]), 1);
        assert!(!written.exists());
    }

    #[test]
    fn the_options_take_their_defaults_and_refuse_what_their_command_lacks() {
        let arguments =
            |line: &str| -> Vec<String> { line.split(' ').map(str::to_owned).collect() };
        let environment = Environment {
            runner: Some("/nix/store/aaaa-wlcs-1.8.1/libexec/wlcs/wlcs".to_owned()),
            version: Some("1.8.1".to_owned()),
            target: Some("/build/target".to_owned()),
        };
        let Command::Check(options) =
            read_options(&arguments("check --listed"), &environment).unwrap()
        else {
            panic!("not a check");
        };
        assert_eq!(options.mode, Mode::Listed);
        assert_eq!(options.deadline, DEADLINE);
        assert!(options.expected.ends_with("crates/yuna-wlcs/expected.txt"));
        assert_eq!(
            options.run.runner,
            Path::new("/nix/store/aaaa-wlcs-1.8.1/libexec/wlcs/wlcs")
        );
        assert_eq!(
            options.run.module,
            Path::new("/build/target/debug/libyuna_wlcs.so")
        );
        assert!(options.run.work.starts_with("/build/target/wlcs/"));
        assert!(options.run.own_work);
        assert_eq!(options.run.hang, running::HANG);
        assert!((1..=4).contains(&options.run.jobs));
        let refused = |line: &str, expected: &str| {
            let error = read_options(&arguments(line), &environment).unwrap_err();
            assert!(error.contains(expected), "{error:?} lacks {expected:?}");
        };
        refused("check", "check takes --listed or --full");
        refused("check --listed --full", "check takes --listed or --full");
        refused("check --full --runs 2", "--runs is not an option of check");
        refused(
            "check --full --jobs 0",
            "--jobs takes a whole number above 0",
        );
        refused("check --full --deadline -1", "--deadline takes minutes");
        refused("record --runs 2", "record needs --out");
        refused("record --out a.json", "record needs --runs");
        refused(
            "record --runs 2 --out a.json --expected e",
            "--expected is not an option",
        );
        refused(
            "record --merge --write e.txt",
            "--merge needs one record at least",
        );
        refused("record --merge a.json", "needs --write");
        refused(
            "record --merge a.json --write e --jobs 2",
            "--jobs is not an option",
        );
        refused("lint", "no command \"lint\"");
        assert_eq!(
            read_options(
                &arguments("record --merge a.json b.json --write e.txt"),
                &environment
            )
            .unwrap(),
            Command::Merge {
                inputs: vec!["a.json".into(), "b.json".into()],
                write: "e.txt".into()
            }
        );
        assert_eq!(read_options(&[], &environment).unwrap(), Command::Help);
        assert_eq!(
            read_options(&arguments("--help"), &environment).unwrap(),
            Command::Help
        );
        assert_eq!(
            read_options(&arguments("check --help"), &environment).unwrap(),
            Command::Help
        );
    }

    /// Whether process `pid` runs: it exists, and is not a zombie waiting to be reaped.
    fn runs(pid: i32) -> bool {
        fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(") ")
                .is_some_and(|(_, rest)| !rest.starts_with('Z'))
        })
    }

    #[test]
    fn jobs_run_their_batches_at_once() {
        let case = Case::new(
            "wlcs-jobs",
            "A.a hang\nB.a hang\nC.a hang\n",
            "pass A.a\npass B.a\npass C.a\n",
        )
        .quick_hang()
        .jobs(3);
        let started = Instant::now();
        let checked = case.check("--full", &[]);
        let elapsed = started.elapsed();
        // One after another, the three hangs would take 3 s at least.
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
        assert_eq!(checked.exit, 1);
        for name in ["A.a", "B.a", "C.a"] {
            assert_eq!(seen(&checked, name), Seen::Hang, "{name}");
        }
    }

    #[test]
    fn a_stop_in_one_job_kills_the_process_of_another() {
        // The breaker trips in T's job, once P's process, in the other, prints without end.
        let case = Case::new(
            "wlcs-jobs-stop",
            "T.a timeout-when-spamming\nT.b timeout\nT.c timeout\nT.d timeout\nT.e timeout\n\
             P.a spam\nP.b ok\n",
            "pass P.a\npass P.b\npass T.a\npass T.b\npass T.c\npass T.d\npass T.e\n",
        )
        .jobs(2);
        let started = Instant::now();
        let checked = case.check("--full", &["--deadline", "1"]);
        let elapsed = started.elapsed();
        // Far before the minute of the deadline, which P's process would otherwise meet.
        assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
        assert_eq!(checked.exit, 1);
        assert_eq!(checked.findings[0].rule, Rule::Stopped);
        assert!(
            checked.findings[0]
                .text
                .starts_with("the breaker: 5 hangs or timeouts in a row: T.a, T.b, T.c, T.d, T.e"),
            "{}",
            checked.findings[0].text
        );
        // P's process ran beside T's, and the stop killed it: its tests did not run.
        assert!(
            case.filters().contains(&"P.*".to_owned()),
            "{:?}",
            case.filters()
        );
        assert_eq!(seen(&checked, "P.a"), Seen::NotRun);
        assert_eq!(seen(&checked, "P.b"), Seen::NotRun);
    }

    #[test]
    fn a_shim_error_fails_the_check_whatever_the_test_expects() {
        let expected = "pass Suite.a\nfail Suite.b\npass Suite.c\npass Suite.d\n";
        let case = Case::new(
            "wlcs-shim-error",
            "Suite.a error\nSuite.b error-fail\nSuite.c ok\nSuite.d error-after\n",
            expected,
        );
        let checked = case.check("--full", &[]);
        assert_eq!(checked.exit, 1);
        assert_eq!(seen(&checked, "Suite.a"), Seen::ShimError);
        assert_eq!(seen(&checked, "Suite.b"), Seen::ShimError);
        assert_eq!(seen(&checked, "Suite.c"), Seen::Pass);
        // Printed after its test's end, the marker blames no test: its process failed.
        assert_eq!(seen(&checked, "Suite.d"), Seen::Pass);
        assert_eq!(
            rules(&checked),
            [
                (Rule::ShimError, None),
                (Rule::ShimError, Some("Suite.a")),
                (Rule::ShimError, Some("Suite.b")),
            ]
        );
        let text = summary(
            &checked.runs,
            &checked.findings,
            Duration::from_secs(1),
            None,
        );
        assert!(
            text.contains(
                "  [shim-error] Suite.a: shim-error (error: placing a surface that is no mapped \
                 window: wl_surface@3), expected pass"
            ),
            "{text}"
        );
        assert!(text.contains("  [shim-error] Suite.b: "), "{text}");
        // The same tests without the marker pass.
        let case = Case::new(
            "wlcs-shim-error-none",
            "Suite.a ok\nSuite.b fail\nSuite.c ok\nSuite.d ok\n",
            expected,
        );
        assert_eq!(case.check("--full", &[]).exit, 0);
    }

    #[test]
    fn a_shim_error_in_a_record_is_refused_by_the_merge() {
        let case = Case::new("wlcs-record-error", "Suite.a ok\nSuite.b error\n", "");
        let out = case.folder.join("r.json");
        let mut arguments = case.arguments(&["record", "--runs", "1", "--out"]);
        arguments.insert(4, out.display().to_string());
        assert_eq!(run(&arguments, &environment()), 1);
        let record = record::read(&out).unwrap();
        assert_eq!(record.tests["Suite.a"], [Seen::Pass]);
        assert_eq!(record.tests["Suite.b"], [Seen::ShimError]);
        let written = case.folder.join("expected-new.txt");
        let merge = [
            "record".to_owned(),
            "--merge".to_owned(),
            out.display().to_string(),
            "--write".to_owned(),
            written.display().to_string(),
        ];
        assert_eq!(run(&merge, &environment()), 1);
        assert!(!written.exists());
    }

    #[test]
    fn a_child_that_outlives_wlcs_is_killed_and_crashes_its_batch() {
        let case = Case::new(
            "wlcs-orphan",
            "Suite.a ok\nSuite.b orphan\nSuite.c ok\n",
            "pass Suite.a\npass Suite.b\npass Suite.c\n",
        );
        let started = Instant::now();
        let checked = case.check("--full", &[]);
        let elapsed = started.elapsed();
        // Its grace, and not the hang guard, 5 s here, ended the process.
        assert!(
            elapsed >= running::ORPHAN_GRACE && elapsed < Duration::from_secs(4),
            "{elapsed:?}"
        );
        assert_eq!(checked.exit, 1);
        assert_eq!(seen(&checked, "Suite.a"), Seen::Pass);
        assert_eq!(seen(&checked, "Suite.b"), Seen::Pass);
        // The last test to end is named.
        assert_eq!(seen(&checked, "Suite.c"), Seen::Crash);
        assert_eq!(
            checked.runs["Suite.c"].ended.note.as_deref(),
            Some("its process ended badly after it: a child of WLCS outlived it: killed")
        );
        assert_eq!(rules(&checked), [(Rule::Crash, Some("Suite.c"))]);
        // The child died with its group.
        let orphan: i32 = fs::read_to_string(case.folder.join("orphan"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(!runs(orphan), "the child {orphan} still runs");
    }

    #[test]
    fn a_listing_that_crashes_fails_the_check_and_the_record() {
        let case = Case::new("wlcs-list-crash", "Suite.a list-crash\n", "pass Suite.a\n");
        let checked = case.check("--listed", &[]);
        assert_eq!(checked.exit, 1);
        assert_eq!(rules(&checked), [(Rule::Crash, None)]);
        assert!(
            checked.findings[0].text.starts_with(
                "the list of tests: WLCS ended while it listed its tests: signal 11, SIGSEGV"
            ),
            "{}",
            checked.findings[0].text
        );
        assert_eq!(
            run(&case.arguments(&["check", "--listed"]), &environment()),
            1
        );
        let out = case.folder.join("r.json");
        let mut arguments = case.arguments(&["record", "--runs", "1", "--out"]);
        arguments.insert(4, out.display().to_string());
        assert_eq!(run(&arguments, &environment()), 1);
        assert!(!out.exists());
    }

    #[test]
    fn times_read_in_words() {
        assert_eq!(duration(Duration::from_millis(4210)), "4.2 s");
        assert_eq!(duration(Duration::from_secs(192)), "3 min 12 s");
        assert_eq!(duration(Duration::from_secs(900)), "15 min");
    }
}
