//! The output of one WLCS process running tests, read line by line as it comes ([`Parser`]):
//! gtest's pretty printer, wrapped by WLCS's listener of skipped tests
//! (`src/xfail_supporting_test_listener.cpp` in WLCS 1.8.1), with the integration module's own
//! lines, from its standard error, on the same pipe.
//!
//! The lines it reads, each test's between its `[ RUN      ] <name>` and its end:
//!
//! - `[       OK ] <name> (<n> ms)`: it passed, unless the module printed
//!   `yuna-wlcs: unsupported: <what>` between its start and its end: it called a hook Yuna cannot
//!   honour, and is unsupported whatever gtest says.
//! - `[  FAILED  ] <name>[, where GetParam() = <value>] (<n> ms)`: it failed. A failure whose
//!   message holds `Timeout waiting for condition`, WLCS's own wait given up after 10 s, is a
//!   timeout too.
//! - `[          ] <reason>` lines, then `[     SKIP ] <name> (<n>ms)`: WLCS skipped it, such as
//!   a test of an extension the compositor does not offer. gtest's own `[  SKIPPED ] <name>
//!   (<n> ms)` is read the same.
//! - After the last test, WLCS's summary, `[==========] <n> tests from <m> test cases run. …`,
//!   then the lists of the skipped and the failed. A test that failed before it was skipped ends
//!   on `[     SKIP ]`, yet the listener counts it failed and lists it under `[  FAILED  ]`: it
//!   fails here too.
//!
//! A test with a start and no end when its process ends crashed; [`Parser::finish`] names it, or
//! the last test to end if the process crashed between tests, which is then no pass either.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The line the integration module prints, on its standard error, when a test calls a hook Yuna
/// cannot honour; what follows names the hook.
pub const UNSUPPORTED_MARKER: &str = "yuna-wlcs: unsupported: ";

/// The message of WLCS's `Timeout`, thrown when one of its waits gives up, after 10 s.
pub const WLCS_TIMEOUT: &str = "Timeout waiting for condition";

const RUN: &str = "[ RUN      ] ";
const OK: &str = "[       OK ] ";
const FAILED: &str = "[  FAILED  ] ";
const WLCS_SKIP: &str = "[     SKIP ] ";
const GTEST_SKIPPED: &str = "[  SKIPPED ] ";
const SKIP_REASON: &str = "[          ] ";
const SUMMARY: &str = "[==========] ";

/// What one run of one test gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Seen {
    /// It passed: an `[       OK ]` line, no unsupported marker, and no crash after it.
    Pass,
    /// It failed.
    Fail,
    /// WLCS skipped it.
    Skip,
    /// It called a hook Yuna cannot honour.
    Unsupported,
    /// Its process crashed while it ran, or just after it ended.
    Crash,
    /// Its process printed nothing for the hang guard's time while it ran.
    Hang,
    /// It did not run: the run stopped, or its process ended before it.
    NotRun,
}

impl Seen {
    /// Its name, as the record file writes it.
    pub fn name(self) -> &'static str {
        match self {
            Seen::Pass => "pass",
            Seen::Fail => "fail",
            Seen::Skip => "skip",
            Seen::Unsupported => "unsupported",
            Seen::Crash => "crash",
            Seen::Hang => "hang",
            Seen::NotRun => "not-run",
        }
    }

    /// Every outcome, in the order the summaries count them.
    pub const ALL: [Seen; 7] = [
        Seen::Pass,
        Seen::Fail,
        Seen::Skip,
        Seen::Unsupported,
        Seen::Crash,
        Seen::Hang,
        Seen::NotRun,
    ];
}

/// How a test's run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    /// Its full name.
    pub name: String,
    /// What it gave.
    pub seen: Seen,
    /// What the outcome alone does not say: the hook Yuna cannot honour, a skip's reason, how
    /// its process ended.
    pub note: Option<String>,
    /// Whether it failed by one of WLCS's own waits.
    pub timed_out: bool,
}

/// How a WLCS process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessEnd {
    /// It exited with this code: WLCS gives 0, or 1 when a test failed.
    Exited(i32),
    /// A signal the runner did not send killed it.
    Signalled(i32),
    /// The hang guard killed it: no output line for the guard's time.
    Hung,
    /// The runner killed it because the run stopped: the breaker, the sentinel or the deadline.
    Stopped,
    /// It did not end after it was killed: it is left behind.
    Lost,
}

impl ProcessEnd {
    /// How it ended, in words.
    pub fn describe(self) -> String {
        match self {
            ProcessEnd::Exited(code) => format!("exit {code}"),
            ProcessEnd::Signalled(signal) => match signal_name(signal) {
                Some(name) => format!("signal {signal}, {name}"),
                None => format!("signal {signal}"),
            },
            ProcessEnd::Hung => "no output for the hang guard's time: killed".to_owned(),
            ProcessEnd::Stopped => "killed: the run stopped".to_owned(),
            ProcessEnd::Lost => "did not end after SIGKILL: left behind".to_owned(),
        }
    }
}

/// The name of the signals a crash gives most often.
fn signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => return None,
    })
}

/// What a line said, when it said something of the tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A test began: its `[ RUN      ]` line.
    Began(String),
    /// A test ended.
    Ended(Ended),
}

/// What the process's end changed, once its output is read whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Finish {
    /// Tests whose outcome the end changes: the one running when the process ended, the last to
    /// end before it crashed, a skipped test WLCS's summary lists as failed.
    pub changed: Vec<Ended>,
    /// When the process failed before any test began: how it ended (a crash or a hang), and in
    /// words.
    pub failure: Option<(Seen, String)>,
    /// Whether it ended as WLCS ends: exit 0 or 1, its summary printed, no test cut.
    pub whole: bool,
    /// Whether any test began.
    pub began: bool,
    /// Unsupported markers printed while no test ran.
    pub stray_markers: Vec<String>,
}

/// The test running, while its output is read.
#[derive(Debug)]
struct Running {
    name: String,
    unsupported: Option<String>,
    timed_out: bool,
    skip_reasons: Vec<String>,
}

impl Running {
    fn end(self, seen: Seen, note: Option<String>) -> Ended {
        let note = match (&self.unsupported, seen) {
            (Some(what), _) => Some(format!("unsupported: {what}")),
            (None, Seen::Skip) if !self.skip_reasons.is_empty() => {
                Some(self.skip_reasons.join("; "))
            }
            _ => note,
        };
        Ended {
            name: self.name,
            seen,
            note,
            timed_out: self.timed_out,
        }
    }
}

/// Reads one WLCS process's output, line by line.
#[derive(Debug, Default)]
pub struct Parser {
    running: Option<Running>,
    /// Each test ended, with what it gave.
    ended: BTreeMap<String, Seen>,
    /// The last test to end.
    last: Option<String>,
    began: bool,
    /// Whether WLCS's summary began.
    summary: bool,
    /// The tests the summary lists as failed.
    failed_in_summary: BTreeSet<String>,
    stray_markers: Vec<String>,
}

/// A test's name at the start of what follows an end's tag: up to a space, or the comma of
/// `, where GetParam() = …`.
fn test_name(rest: &str) -> &str {
    rest.split([' ', ',']).next().unwrap_or_default()
}

/// `line` without the colours' escape sequences, `ESC [ … <letter>`, that a terminal's printer
/// would add: the runner reads a pipe, which turns them off, but a line keeping one would be lost.
fn without_colours(line: &str) -> Cow<'_, str> {
    if !line.contains('\u{1b}') {
        return Cow::Borrowed(line);
    }
    let mut plain = String::with_capacity(line.len());
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' && characters.peek() == Some(&'[') {
            characters.next();
            for inside in characters.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&inside) {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    Cow::Owned(plain)
}

/// Whether `rest`, after `[  FAILED  ] ` in the summary, is its count: `<n> test(s) failed:`.
fn is_failed_count(rest: &str) -> bool {
    rest.split_once(' ').is_some_and(|(count, words)| {
        !count.is_empty()
            && count.bytes().all(|b| b.is_ascii_digit())
            && (words == "test failed:" || words == "tests failed:")
    })
}

/// At most this many unsupported markers outside a test are kept, to report.
const STRAY_MARKERS_KEPT: usize = 20;

impl Parser {
    /// A parser for a new process's output.
    pub fn new() -> Parser {
        Parser::default()
    }

    /// Reads one line, its newline left out; gives what it said of the tests.
    pub fn feed(&mut self, line: &str) -> Vec<Event> {
        let line = without_colours(line);
        let line = line.as_ref();
        if let Some(rest) = line.strip_prefix(RUN) {
            let mut events = Vec::new();
            if let Some(cut) = self.running.take() {
                // gtest ends every test before it begins the next: a test without its end has
                // lost it, and its outcome cannot be read.
                let ended = cut.end(
                    Seen::Crash,
                    Some("its end line is missing: a new test began".to_owned()),
                );
                events.push(Event::Ended(self.record(ended)));
            }
            let name = test_name(rest).to_owned();
            self.began = true;
            self.running = Some(Running {
                name: name.clone(),
                unsupported: None,
                timed_out: false,
                skip_reasons: Vec::new(),
            });
            events.push(Event::Began(name));
            return events;
        }
        let Some(running) = &mut self.running else {
            if let Some(what) = line.strip_prefix(UNSUPPORTED_MARKER) {
                if self.stray_markers.len() < STRAY_MARKERS_KEPT {
                    self.stray_markers.push(what.to_owned());
                }
            } else if line.starts_with(SUMMARY) && line.contains(" test cases run.") {
                self.summary = true;
            } else if self.summary
                && let Some(rest) = line.strip_prefix(FAILED)
                && !is_failed_count(rest)
            {
                self.failed_in_summary.insert(test_name(rest).to_owned());
            }
            return Vec::new();
        };
        if let Some(what) = line.strip_prefix(UNSUPPORTED_MARKER) {
            running.unsupported.get_or_insert_with(|| what.to_owned());
            return Vec::new();
        }
        if let Some(reason) = line.strip_prefix(SKIP_REASON) {
            running.skip_reasons.push(reason.to_owned());
            return Vec::new();
        }
        let ending = [
            (OK, Seen::Pass),
            (FAILED, Seen::Fail),
            (WLCS_SKIP, Seen::Skip),
            (GTEST_SKIPPED, Seen::Skip),
        ]
        .into_iter()
        .find_map(|(tag, seen)| Some((line.strip_prefix(tag)?, seen)));
        let ends = ending
            .filter(|(rest, _)| test_name(rest) == running.name)
            .map(|(_, seen)| seen);
        if ends.is_none() && line.contains(WLCS_TIMEOUT) {
            running.timed_out = true;
        }
        match (ends, self.running.take()) {
            (Some(seen), Some(running)) => {
                let seen = if running.unsupported.is_some() {
                    Seen::Unsupported
                } else {
                    seen
                };
                let ended = running.end(seen, None);
                vec![Event::Ended(self.record(ended))]
            }
            (_, running) => {
                self.running = running;
                Vec::new()
            }
        }
    }

    /// Notes that `ended` ended, and gives it back.
    fn record(&mut self, ended: Ended) -> Ended {
        self.ended.insert(ended.name.clone(), ended.seen);
        self.last = Some(ended.name.clone());
        ended
    }

    /// Reads the process's end, once its output is read whole: who crashed or hung, and whether
    /// WLCS's summary changes a test's outcome.
    pub fn finish(mut self, end: ProcessEnd) -> Finish {
        let mut changed = Vec::new();
        // The listener's own count: failed, though its end line said skipped.
        for name in &self.failed_in_summary {
            if self.ended.get(name) == Some(&Seen::Skip) {
                changed.push(Ended {
                    name: name.clone(),
                    seen: Seen::Fail,
                    note: Some(
                        "it failed before it was skipped: WLCS's summary counts it failed"
                            .to_owned(),
                    ),
                    timed_out: false,
                });
            }
        }
        let whole =
            matches!(end, ProcessEnd::Exited(0 | 1)) && self.summary && self.running.is_none();
        let mut failure = None;
        if !whole {
            let blamed = match end {
                ProcessEnd::Stopped => None,
                ProcessEnd::Hung | ProcessEnd::Lost => Some(Seen::Hang),
                ProcessEnd::Exited(_) | ProcessEnd::Signalled(_) => Some(Seen::Crash),
            };
            let how = match end {
                ProcessEnd::Exited(_) if !self.summary => {
                    format!("{} without WLCS's summary", end.describe())
                }
                _ => end.describe(),
            };
            if let Some(running) = self.running.take() {
                let seen = blamed.unwrap_or(Seen::NotRun);
                changed.push(running.end(seen, Some(how)));
            } else if let Some(seen) = blamed {
                match self.last.take() {
                    Some(last) => changed.push(Ended {
                        name: last,
                        seen,
                        note: Some(format!("its process ended badly after it: {how}")),
                        timed_out: false,
                    }),
                    None => failure = Some((seen, format!("{how}, before any test began"))),
                }
            }
        }
        Finish {
            changed,
            failure,
            whole,
            began: self.began,
            stray_markers: self.stray_markers,
        }
    }
}

/// Every test's outcome in a whole output `text`, once its process ended as `end`: what the tests
/// gave, as the runner reads them, for the tests and for a log read again.
pub fn parse(text: &str, end: ProcessEnd) -> (BTreeMap<String, Ended>, Finish) {
    let mut parser = Parser::new();
    let mut outcomes = BTreeMap::new();
    for line in text.lines() {
        for event in parser.feed(line) {
            if let Event::Ended(ended) = event {
                outcomes.insert(ended.name.clone(), ended);
            }
        }
    }
    let finish = parser.finish(end);
    for ended in &finish.changed {
        outcomes.insert(ended.name.clone(), ended.clone());
    }
    (outcomes, finish)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run of WLCS 1.8.1 as its printer and listener write it: passes, failures, skips, a test
    /// skipped after it failed, an unsupported hook, parameterised names, its summary.
    const RUN_OUTPUT: &str = include_str!("fixtures/run.txt");
    /// A run cut after a test's start: the process crashed in it.
    const CUT: &str = include_str!("fixtures/cut.txt");

    fn seen(outcomes: &BTreeMap<String, Ended>, name: &str) -> Seen {
        outcomes
            .get(name)
            .unwrap_or_else(|| panic!("no outcome for {name}"))
            .seen
    }

    #[test]
    fn ok_failed_and_skip_lines_give_each_tests_outcome() {
        let (outcomes, finish) = parse(RUN_OUTPUT, ProcessEnd::Exited(1));
        assert!(finish.whole);
        assert!(finish.began);
        assert_eq!(finish.failure, None);
        assert_eq!(
            seen(
                &outcomes,
                "SelfTest.when_creating_second_client_nothing_bad_happens"
            ),
            Seen::Pass
        );
        let failed = &outcomes["XdgToplevelStableTest.wm_capabilities_are_sent"];
        assert_eq!(failed.seen, Seen::Fail);
        assert!(!failed.timed_out);
        let timed_out = &outcomes["XdgToplevelStableTest.pointer_respects_window_geom_offset"];
        assert_eq!(timed_out.seen, Seen::Fail);
        assert!(timed_out.timed_out, "WLCS's timeout is read");
        let skipped = &outcomes["SelfTest.expected_missing_extension_is_xfail"];
        assert_eq!(skipped.seen, Seen::Skip);
        assert_eq!(
            skipped.note.as_deref(),
            Some("Missing extension: xdg_not_really_an_extension>= 1")
        );
        assert_eq!(outcomes.len(), 8, "{outcomes:#?}");
    }

    #[test]
    fn a_test_skipped_after_it_failed_counts_failed() {
        let mut parser = Parser::new();
        let mut ends = Vec::new();
        for line in RUN_OUTPUT.lines() {
            for event in parser.feed(line) {
                if let Event::Ended(ended) = event {
                    ends.push(ended);
                }
            }
        }
        // Its own end line says skipped...
        let name = "SelfTest.acquiring_unsupported_extension_is_xfail";
        let own = ends.iter().find(|e| e.name == name).unwrap();
        assert_eq!(own.seen, Seen::Skip);
        // ...the listener's summary lists it failed: the end of the process makes it so.
        let finish = parser.finish(ProcessEnd::Exited(1));
        assert_eq!(finish.changed.len(), 1, "{:?}", finish.changed);
        assert_eq!(finish.changed[0].name, name);
        assert_eq!(finish.changed[0].seen, Seen::Fail);
        // A test the summary lists failed and that failed already is not changed.
        let (outcomes, _) = parse(RUN_OUTPUT, ProcessEnd::Exited(1));
        assert_eq!(outcomes[name].seen, Seen::Fail);
    }

    #[test]
    fn a_marker_between_run_and_ok_makes_the_test_unsupported() {
        let (outcomes, _) = parse(RUN_OUTPUT, ProcessEnd::Exited(1));
        let touch = &outcomes["XdgToplevelStableTest.touch_respects_window_geom_offset"];
        assert_eq!(touch.seen, Seen::Unsupported);
        assert_eq!(touch.note.as_deref(), Some("unsupported: touch"));
        // A marker in a test that fails makes it unsupported too, whatever gtest says.
        let text = "[ RUN      ] A.b\nyuna-wlcs: unsupported: touch\n[  FAILED  ] A.b (3 ms)\n\
                    [==========] 1 tests from 1 test cases run. (3ms total elapsed)\n";
        let (outcomes, finish) = parse(text, ProcessEnd::Exited(1));
        assert_eq!(outcomes["A.b"].seen, Seen::Unsupported);
        assert!(finish.whole);
        // A marker outside any test marks none; it is kept to report.
        let text = "yuna-wlcs: unsupported: touch\n[ RUN      ] A.b\n[       OK ] A.b (1 ms)\n\
                    [==========] 1 tests from 1 test cases run. (1ms total elapsed)\n";
        let (outcomes, finish) = parse(text, ProcessEnd::Exited(0));
        assert_eq!(outcomes["A.b"].seen, Seen::Pass);
        assert_eq!(finish.stray_markers, ["touch"]);
    }

    #[test]
    fn parameterised_names_are_read_whole() {
        let (outcomes, _) = parse(RUN_OUTPUT, ProcessEnd::Exited(1));
        assert_eq!(
            seen(
                &outcomes,
                "XdgShellStableSubsurfaces/SubsurfaceTest.place_above_simple/0"
            ),
            Seen::Pass
        );
        // gtest's failed line names the parameter after a comma.
        assert_eq!(
            seen(
                &outcomes,
                "Anchors/LayerSurfaceErrorsTest.\
                 specifying_zero_size_without_corresponding_anchors_is_an_error/1"
            ),
            Seen::Fail
        );
    }

    #[test]
    fn output_cut_after_a_run_line_is_a_crash_of_that_test() {
        let (outcomes, finish) = parse(CUT, ProcessEnd::Signalled(11));
        assert!(!finish.whole);
        assert_eq!(
            seen(
                &outcomes,
                "XdgSurfaceStableTest.supports_xdg_shell_stable_protocol"
            ),
            Seen::Pass
        );
        let culprit = &outcomes["XdgSurfaceStableTest.gets_configure_event"];
        assert_eq!(culprit.seen, Seen::Crash);
        assert_eq!(culprit.note.as_deref(), Some("signal 11, SIGSEGV"));
        // Killed by the hang guard, the same test hung.
        let (outcomes, _) = parse(CUT, ProcessEnd::Hung);
        assert_eq!(
            outcomes["XdgSurfaceStableTest.gets_configure_event"].seen,
            Seen::Hang
        );
        // Killed because the run stopped, it did not run.
        let (outcomes, finish) = parse(CUT, ProcessEnd::Stopped);
        assert_eq!(
            outcomes["XdgSurfaceStableTest.gets_configure_event"].seen,
            Seen::NotRun
        );
        assert_eq!(finish.failure, None);
        // Exited without its summary, the same: the process did not end as WLCS ends.
        let (outcomes, _) = parse(CUT, ProcessEnd::Exited(0));
        let culprit = &outcomes["XdgSurfaceStableTest.gets_configure_event"];
        assert_eq!(culprit.seen, Seen::Crash);
        assert_eq!(
            culprit.note.as_deref(),
            Some("exit 0 without WLCS's summary")
        );
    }

    #[test]
    fn a_crash_after_a_tests_end_is_that_tests() {
        let text = "[ RUN      ] A.a\n[       OK ] A.a (1 ms)\n[ RUN      ] A.b\n\
                    [       OK ] A.b (1 ms)\n";
        let (outcomes, finish) = parse(text, ProcessEnd::Signalled(6));
        assert_eq!(outcomes["A.a"].seen, Seen::Pass);
        assert_eq!(outcomes["A.b"].seen, Seen::Crash);
        assert_eq!(
            outcomes["A.b"].note.as_deref(),
            Some("its process ended badly after it: signal 6, SIGABRT")
        );
        assert_eq!(finish.failure, None);
        // Even with the summary printed: a crash at the process's exit.
        let text = format!(
            "{text}[==========] 2 tests from 1 test cases run. (2ms total elapsed)\n\
             [  PASSED  ] 2 tests\n"
        );
        let (outcomes, _) = parse(&text, ProcessEnd::Signalled(11));
        assert_eq!(outcomes["A.b"].seen, Seen::Crash);
        let (outcomes, finish) = parse(&text, ProcessEnd::Exited(0));
        assert_eq!(outcomes["A.b"].seen, Seen::Pass);
        assert!(finish.whole);
    }

    #[test]
    fn a_process_that_fails_before_any_test_blames_none() {
        let text = "Failed to load compositor integration module x.so: no such file\n";
        let (outcomes, finish) = parse(text, ProcessEnd::Exited(1));
        assert!(outcomes.is_empty());
        assert!(!finish.began);
        assert_eq!(
            finish.failure,
            Some((
                Seen::Crash,
                "exit 1 without WLCS's summary, before any test began".to_owned()
            ))
        );
        let (_, finish) = parse("", ProcessEnd::Hung);
        assert_eq!(finish.failure.map(|(seen, _)| seen), Some(Seen::Hang));
        // WLCS's own end when its filter matches nothing: whole, and nothing ran.
        let text = "Note: Google Test filter = NoSuch.test\n\
                    [==========] Running 0 tests from 0 test suites.\n\
                    [==========] 0 tests from 0 test cases run. (0ms total elapsed)\n\
                    [  PASSED  ] 0 tests\n\
                    WARNING: filter \"NoSuch.test\" did not match any test; no tests were run\n";
        let (outcomes, finish) = parse(text, ProcessEnd::Exited(0));
        assert!(outcomes.is_empty());
        assert!(finish.whole);
        assert!(!finish.began);
        assert_eq!(finish.failure, None);
    }

    #[test]
    fn a_test_whose_end_is_lost_is_not_a_pass() {
        let text = "[ RUN      ] A.a\n[ RUN      ] A.b\n[       OK ] A.b (1 ms)\n\
                    [==========] 2 tests from 1 test cases run. (2ms total elapsed)\n";
        let (outcomes, finish) = parse(text, ProcessEnd::Exited(0));
        assert_eq!(outcomes["A.a"].seen, Seen::Crash);
        assert_eq!(outcomes["A.b"].seen, Seen::Pass);
        assert!(finish.whole);
        // An end line for another test than the one running is no end.
        let text = "[ RUN      ] A.a\n[       OK ] A.ab (1 ms)\n";
        let (outcomes, _) = parse(text, ProcessEnd::Signalled(11));
        assert_eq!(outcomes["A.a"].seen, Seen::Crash);
        assert_eq!(outcomes.len(), 1);
    }

    #[test]
    fn colours_are_read_through() {
        let text = "\u{1b}[0;32m[ RUN      ] \u{1b}[mA.a\n\u{1b}[0;32m[       OK ] \u{1b}[mA.a (0 ms)\n\
                    \u{1b}[32m[==========] \u{1b}[00m1 tests from 1 test cases run. (0ms total elapsed)\n";
        let (outcomes, finish) = parse(text, ProcessEnd::Exited(0));
        assert_eq!(outcomes["A.a"].seen, Seen::Pass);
        assert!(finish.whole);
    }
}
