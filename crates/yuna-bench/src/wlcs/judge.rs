//! The rules of `wlcs check`: what a run's outcomes, set against the file of expected outcomes,
//! fail ([`Rule::judged`]) and what they only report. [`judge`] gives every finding of a run;
//! [`version`] the one rule judged before any test runs. The parent module's documentation
//! states the rules for a reader.

use std::collections::{BTreeMap, BTreeSet};

use super::expected::{Expectation, ExpectedFile};
use super::output::Seen;
use super::running::{ProcessFailure, Stop, TestRun};

/// Which tests a check ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `--listed`: the tests expected to pass, alone.
    Listed,
    /// `--full`: every test.
    Full,
}

/// A rule a finding comes under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    /// The file's WLCS is not the runner's.
    Version,
    /// The file names a test WLCS no longer lists.
    Gone,
    /// WLCS lists a test the file does not name.
    Unlisted,
    /// A test expected to pass did not.
    Pass,
    /// A test crashed, or a process with no test to blame.
    Crash,
    /// A test hung, or a process with no test to blame.
    Hang,
    /// A test not expected to pass passed, and passed again alone.
    PassesNow,
    /// The run stopped before its end: the sentinel, the breaker or the deadline.
    Stopped,
    /// A test not expected to pass gave another outcome than expected; reported.
    Changed,
    /// A test not expected to pass passed once, but not again alone; reported.
    PassedOnce,
    /// A flaky test's outcome; reported.
    Flaky,
    /// A test not expected to pass did not run; reported.
    NotRun,
}

impl Rule {
    /// Its name in the summary.
    pub fn name(self) -> &'static str {
        match self {
            Rule::Version => "version",
            Rule::Gone => "gone",
            Rule::Unlisted => "unlisted",
            Rule::Pass => "pass",
            Rule::Crash => "crash",
            Rule::Hang => "hang",
            Rule::PassesNow => "passes-now",
            Rule::Stopped => "stopped",
            Rule::Changed => "changed",
            Rule::PassedOnce => "passed-once",
            Rule::Flaky => "flaky",
            Rule::NotRun => "not-run",
        }
    }

    /// Whether a finding under it fails the check; the others are reported.
    pub fn judged(self) -> bool {
        !matches!(
            self,
            Rule::Changed | Rule::PassedOnce | Rule::Flaky | Rule::NotRun
        )
    }
}

/// One thing a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The rule it comes under.
    pub rule: Rule,
    /// The test, if one is to blame.
    pub test: Option<String>,
    /// What was found, in words.
    pub text: String,
}

/// What a check gives the judge.
#[derive(Debug, Clone, Copy)]
pub struct Inputs<'a> {
    /// Which tests it ran.
    pub mode: Mode,
    /// The file of expected outcomes.
    pub expected: &'a ExpectedFile,
    /// The file's path.
    pub expected_path: &'a str,
    /// The tests WLCS lists, the disabled left out.
    pub listed: &'a [String],
    /// Each test's run in its batch, by name.
    pub runs: &'a BTreeMap<String, TestRun>,
    /// The runs again alone of the tests not expected to pass that passed, by name.
    pub alone: &'a BTreeMap<String, TestRun>,
    /// The processes that failed with no test to blame.
    pub failures: &'a [ProcessFailure],
    /// Why the run stopped, if it did.
    pub stop: Option<&'a Stop>,
}

/// The finding of a file recorded with another WLCS than the runner's, `runner_version`.
pub fn version(expected: &ExpectedFile, runner_version: &str) -> Option<Finding> {
    (expected.wlcs != runner_version).then(|| Finding {
        rule: Rule::Version,
        test: None,
        text: format!(
            "the file is for WLCS {}, the runner is WLCS {runner_version}: record it again \
             (yuna-bench wlcs record)",
            expected.wlcs
        ),
    })
}

/// ` (log: <path>)`, when `run` has a log.
fn log_of(run: &TestRun) -> String {
    run.log
        .as_ref()
        .map(|log| format!(" (log: {})", log.display()))
        .unwrap_or_default()
}

/// A run's outcome in words, its note after it: `fail`, `crash (signal 11, SIGSEGV)`.
fn outcome(run: &TestRun) -> String {
    match &run.ended.note {
        Some(note) => format!("{} ({note})", run.ended.seen.name()),
        None => run.ended.seen.name().to_owned(),
    }
}

/// The finding of a crash or a hang of `run`, whatever was expected of it; `context` says which
/// run it was.
fn crash_or_hang(run: &TestRun, expectation: Option<&Expectation>, context: &str) -> Finding {
    let rule = if run.ended.seen == Seen::Hang {
        Rule::Hang
    } else {
        Rule::Crash
    };
    let expected = expectation.map_or("not in the file".to_owned(), |e| {
        format!("expected {}", e.name())
    });
    Finding {
        rule,
        test: Some(run.ended.name.clone()),
        text: format!("{}{context}, {expected}{}", outcome(run), log_of(run)),
    }
}

/// Every finding of a run: first why it stopped and the processes that failed, then the file
/// against the list, then each test by name.
pub fn judge(inputs: &Inputs) -> Vec<Finding> {
    let mut findings = Vec::new();
    let listed: BTreeSet<&str> = inputs.listed.iter().map(String::as_str).collect();
    let crashed = |run: &TestRun| matches!(run.ended.seen, Seen::Crash | Seen::Hang);
    if let Some(stop) = inputs.stop {
        let not_run: Vec<&String> = inputs
            .expected
            .tests
            .iter()
            .filter(|(name, expectation)| {
                **expectation == Expectation::Pass
                    && listed.contains(name.as_str())
                    && inputs
                        .runs
                        .get(*name)
                        .is_none_or(|run| run.ended.seen == Seen::NotRun)
            })
            .map(|(name, _)| name)
            .collect();
        let mut text = stop.describe();
        if !not_run.is_empty() {
            text.push_str(&format!(
                "; {} tests expected to pass did not run",
                not_run.len()
            ));
        }
        findings.push(Finding {
            rule: Rule::Stopped,
            test: None,
            text,
        });
    }
    for failure in inputs.failures {
        findings.push(Finding {
            rule: if failure.seen == Seen::Hang {
                Rule::Hang
            } else {
                Rule::Crash
            },
            test: None,
            text: format!(
                "{}: {} (log: {})",
                failure.batch,
                failure.text,
                failure.log.display()
            ),
        });
    }
    for name in inputs.expected.tests.keys() {
        if !listed.contains(name.as_str()) {
            findings.push(Finding {
                rule: Rule::Gone,
                test: Some(name.clone()),
                text: format!(
                    "{} names it, WLCS does not list it: remove its line",
                    inputs.expected_path
                ),
            });
        }
    }
    for name in inputs.listed {
        if !inputs.expected.tests.contains_key(name) {
            let ran = inputs
                .runs
                .get(name)
                .filter(|run| run.ended.seen != Seen::NotRun)
                .map(|run| format!("; it ran: {}", outcome(run)))
                .unwrap_or_default();
            findings.push(Finding {
                rule: Rule::Unlisted,
                test: Some(name.clone()),
                text: format!(
                    "WLCS lists it, {} does not: give it a line{ran}",
                    inputs.expected_path
                ),
            });
            if let Some(run) = inputs.runs.get(name).filter(|run| crashed(run)) {
                findings.push(crash_or_hang(run, None, ""));
            }
        }
    }
    for (name, expectation) in &inputs.expected.tests {
        if !listed.contains(name.as_str()) {
            continue;
        }
        let finding = |rule: Rule, text: String| Finding {
            rule,
            test: Some(name.clone()),
            text,
        };
        let run = inputs.runs.get(name);
        if let Some(run) = run.filter(|run| crashed(run)) {
            findings.push(crash_or_hang(run, Some(expectation), ""));
            continue;
        }
        let Some(run) = run.filter(|run| run.ended.seen != Seen::NotRun) else {
            // A stop counts what it left unrun; --listed runs only the tests expected to pass.
            let note = run
                .and_then(|run| run.ended.note.clone())
                .map(|note| format!(" ({note})"))
                .unwrap_or_default();
            match expectation {
                _ if inputs.stop.is_some() => {}
                Expectation::Pass => {
                    findings.push(finding(Rule::Pass, format!("expected pass, not run{note}")));
                }
                _ if inputs.mode == Mode::Listed => {}
                _ => findings.push(finding(
                    Rule::NotRun,
                    format!("expected {}, not run{note}", expectation.name()),
                )),
            }
            continue;
        };
        let seen = run.ended.seen;
        match expectation {
            Expectation::Pass if seen == Seen::Pass => {}
            Expectation::Pass => findings.push(finding(
                Rule::Pass,
                format!("expected pass, saw {}{}", outcome(run), log_of(run)),
            )),
            Expectation::Flaky(reason) => findings.push(finding(
                Rule::Flaky,
                format!("flaky ({reason}): saw {}", outcome(run)),
            )),
            _ if seen == Seen::Pass => {
                let expected = expectation.name();
                match inputs.alone.get(name) {
                    Some(alone) if alone.ended.seen == Seen::Pass => findings.push(finding(
                        Rule::PassesNow,
                        format!(
                            "expected {expected}, passed in its batch and again alone: \
                             list it as pass"
                        ),
                    )),
                    Some(alone) if crashed(alone) => {
                        findings.push(crash_or_hang(alone, Some(expectation), " alone"));
                    }
                    Some(alone) => findings.push(finding(
                        Rule::PassedOnce,
                        format!(
                            "expected {expected}, passed in its batch, then {} alone{}",
                            outcome(alone),
                            log_of(alone)
                        ),
                    )),
                    None => findings.push(finding(
                        Rule::PassedOnce,
                        format!("expected {expected}, passed in its batch; not run again alone"),
                    )),
                }
            }
            _ if expectation.is(seen) => {}
            _ => findings.push(finding(
                Rule::Changed,
                format!(
                    "expected {}, saw {}{}",
                    expectation.name(),
                    outcome(run),
                    log_of(run)
                ),
            )),
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::wlcs::output::Ended;

    /// A file of `tests`, each `(name, expectation)`, for WLCS 1.8.1.
    fn file(tests: &[(&str, Expectation)]) -> ExpectedFile {
        ExpectedFile {
            wlcs: "1.8.1".to_owned(),
            tests: tests
                .iter()
                .map(|(name, e)| ((*name).to_owned(), e.clone()))
                .collect(),
        }
    }

    fn run(name: &str, seen: Seen) -> TestRun {
        TestRun {
            ended: Ended {
                name: name.to_owned(),
                seen,
                note: None,
                timed_out: false,
            },
            batch: "Suite".to_owned(),
            log: Some(PathBuf::from("/work/logs/0002-Suite.log")),
        }
    }

    fn runs(runs: &[(&str, Seen)]) -> BTreeMap<String, TestRun> {
        runs.iter()
            .map(|(name, seen)| ((*name).to_owned(), run(name, *seen)))
            .collect()
    }

    /// The findings of a full run of `expected`'s tests, all listed, that gave `seen`, and
    /// `alone` for the reruns.
    fn judged_full(
        expected: &ExpectedFile,
        seen: &[(&str, Seen)],
        alone: &[(&str, Seen)],
    ) -> Vec<Finding> {
        let listed: Vec<String> = expected.tests.keys().cloned().collect();
        judge(&Inputs {
            mode: Mode::Full,
            expected,
            expected_path: "expected.txt",
            listed: &listed,
            runs: &runs(seen),
            alone: &runs(alone),
            failures: &[],
            stop: None,
        })
    }

    fn rules(findings: &[Finding]) -> Vec<(Rule, Option<&str>)> {
        findings
            .iter()
            .map(|f| (f.rule, f.test.as_deref()))
            .collect()
    }

    fn fails(findings: &[Finding]) -> bool {
        findings.iter().any(|f| f.rule.judged())
    }

    #[test]
    fn a_pass_test_must_pass() {
        let expected = file(&[("A.a", Expectation::Pass)]);
        assert!(judged_full(&expected, &[("A.a", Seen::Pass)], &[]).is_empty());
        for seen in [Seen::Fail, Seen::Skip, Seen::Unsupported, Seen::NotRun] {
            let findings = judged_full(&expected, &[("A.a", seen)], &[]);
            assert_eq!(rules(&findings), [(Rule::Pass, Some("A.a"))], "{seen:?}");
            assert!(fails(&findings));
        }
        // Not run at all, as when its batch never began.
        let findings = judged_full(&expected, &[], &[]);
        assert_eq!(rules(&findings), [(Rule::Pass, Some("A.a"))]);
        assert_eq!(findings[0].text, "expected pass, not run");
        let findings = judged_full(&expected, &[("A.a", Seen::Fail)], &[]);
        assert_eq!(
            findings[0].text,
            "expected pass, saw fail (log: /work/logs/0002-Suite.log)"
        );
    }

    #[test]
    fn any_crash_or_hang_fails_whatever_was_expected() {
        for expectation in [
            Expectation::Pass,
            Expectation::Fail,
            Expectation::Skip,
            Expectation::Unsupported,
            Expectation::Flaky("varied: pass 1, fail 1".to_owned()),
        ] {
            let expected = file(&[("A.a", expectation.clone())]);
            let findings = judged_full(&expected, &[("A.a", Seen::Crash)], &[]);
            assert_eq!(
                rules(&findings),
                [(Rule::Crash, Some("A.a"))],
                "{expectation:?}"
            );
            assert!(fails(&findings));
            let findings = judged_full(&expected, &[("A.a", Seen::Hang)], &[]);
            assert_eq!(
                rules(&findings),
                [(Rule::Hang, Some("A.a"))],
                "{expectation:?}"
            );
        }
        // A process that failed with no test to blame fails too.
        let expected = file(&[("A.a", Expectation::Fail)]);
        let listed = ["A.a".to_owned()];
        let failure = ProcessFailure {
            batch: "A".to_owned(),
            seen: Seen::Crash,
            text: "signal 11, SIGSEGV, before any test began".to_owned(),
            log: PathBuf::from("/work/logs/0003-A.log"),
        };
        let findings = judge(&Inputs {
            mode: Mode::Full,
            expected: &expected,
            expected_path: "expected.txt",
            listed: &listed,
            runs: &runs(&[("A.a", Seen::NotRun)]),
            alone: &BTreeMap::new(),
            failures: std::slice::from_ref(&failure),
            stop: None,
        });
        assert_eq!(
            rules(&findings),
            [(Rule::Crash, None), (Rule::NotRun, Some("A.a"))]
        );
        assert!(fails(&findings));
        assert_eq!(
            findings[0].text,
            "A: signal 11, SIGSEGV, before any test began (log: /work/logs/0003-A.log)"
        );
    }

    #[test]
    fn the_file_and_the_list_must_name_the_same_tests() {
        let expected = file(&[("A.a", Expectation::Pass), ("A.gone", Expectation::Fail)]);
        let listed = ["A.a".to_owned(), "A.new".to_owned()];
        let inputs = |runs: &BTreeMap<String, TestRun>| {
            judge(&Inputs {
                mode: Mode::Full,
                expected: &expected,
                expected_path: "crates/yuna-wlcs/expected.txt",
                listed: &listed,
                runs,
                alone: &BTreeMap::new(),
                failures: &[],
                stop: None,
            })
        };
        let findings = inputs(&runs(&[("A.a", Seen::Pass), ("A.new", Seen::Fail)]));
        assert_eq!(
            rules(&findings),
            [
                (Rule::Gone, Some("A.gone")),
                (Rule::Unlisted, Some("A.new"))
            ]
        );
        assert!(fails(&findings));
        assert_eq!(
            findings[0].text,
            "crates/yuna-wlcs/expected.txt names it, WLCS does not list it: remove its line"
        );
        assert_eq!(
            findings[1].text,
            "WLCS lists it, crates/yuna-wlcs/expected.txt does not: give it a line; it ran: fail"
        );
        // An unlisted test that crashes fails twice over.
        let findings = inputs(&runs(&[("A.a", Seen::Pass), ("A.new", Seen::Crash)]));
        assert_eq!(
            rules(&findings),
            [
                (Rule::Gone, Some("A.gone")),
                (Rule::Unlisted, Some("A.new")),
                (Rule::Crash, Some("A.new"))
            ]
        );
        // The same file and list: nothing found.
        let same = file(&[("A.a", Expectation::Pass)]);
        assert!(judged_full(&same, &[("A.a", Seen::Pass)], &[]).is_empty());
    }

    #[test]
    fn the_files_version_must_be_the_runners() {
        let expected = file(&[]);
        assert_eq!(version(&expected, "1.8.1"), None);
        let finding = version(&expected, "1.9.0").unwrap();
        assert_eq!(finding.rule, Rule::Version);
        assert!(finding.rule.judged());
        assert!(
            finding
                .text
                .starts_with("the file is for WLCS 1.8.1, the runner is WLCS 1.9.0"),
            "{}",
            finding.text
        );
    }

    #[test]
    fn a_new_pass_fails_only_once_confirmed_alone() {
        for expectation in [
            Expectation::Fail,
            Expectation::Skip,
            Expectation::Unsupported,
        ] {
            let expected = file(&[("A.a", expectation.clone())]);
            let findings = judged_full(&expected, &[("A.a", Seen::Pass)], &[("A.a", Seen::Pass)]);
            assert_eq!(rules(&findings), [(Rule::PassesNow, Some("A.a"))]);
            assert!(fails(&findings));
            assert_eq!(
                findings[0].text,
                format!(
                    "expected {}, passed in its batch and again alone: list it as pass",
                    expectation.name()
                )
            );
            // Passed once, then as expected alone: reported only.
            let findings = judged_full(&expected, &[("A.a", Seen::Pass)], &[("A.a", Seen::Fail)]);
            assert_eq!(rules(&findings), [(Rule::PassedOnce, Some("A.a"))]);
            assert!(!fails(&findings));
            // Not run again alone: reported only.
            let findings = judged_full(&expected, &[("A.a", Seen::Pass)], &[]);
            assert_eq!(rules(&findings), [(Rule::PassedOnce, Some("A.a"))]);
            assert!(!fails(&findings));
            // A crash alone is a crash.
            let findings = judged_full(&expected, &[("A.a", Seen::Pass)], &[("A.a", Seen::Crash)]);
            assert_eq!(rules(&findings), [(Rule::Crash, Some("A.a"))]);
            assert!(fails(&findings));
        }
    }

    #[test]
    fn a_flaky_test_is_reported_never_judged() {
        let expected = file(&[(
            "A.a",
            Expectation::Flaky("varied: pass 3, fail 1".to_owned()),
        )]);
        for seen in [Seen::Pass, Seen::Fail, Seen::Skip, Seen::Unsupported] {
            let findings = judged_full(&expected, &[("A.a", seen)], &[]);
            assert_eq!(rules(&findings), [(Rule::Flaky, Some("A.a"))], "{seen:?}");
            assert!(!fails(&findings));
        }
        let findings = judged_full(&expected, &[("A.a", Seen::Fail)], &[]);
        assert_eq!(findings[0].text, "flaky (varied: pass 3, fail 1): saw fail");
    }

    #[test]
    fn other_changes_are_reported_not_judged() {
        let expected = file(&[
            ("A.fail", Expectation::Fail),
            ("A.skip", Expectation::Skip),
            ("A.unsupported", Expectation::Unsupported),
        ]);
        let findings = judged_full(
            &expected,
            &[
                ("A.fail", Seen::Skip),
                ("A.skip", Seen::Fail),
                ("A.unsupported", Seen::Fail),
            ],
            &[],
        );
        assert_eq!(
            rules(&findings),
            [
                (Rule::Changed, Some("A.fail")),
                (Rule::Changed, Some("A.skip")),
                (Rule::Changed, Some("A.unsupported"))
            ]
        );
        assert!(!fails(&findings));
        // Each as expected: nothing.
        let findings = judged_full(
            &expected,
            &[
                ("A.fail", Seen::Fail),
                ("A.skip", Seen::Skip),
                ("A.unsupported", Seen::Unsupported),
            ],
            &[],
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn listed_judges_the_pass_tests_alone() {
        let expected = file(&[
            ("A.a", Expectation::Pass),
            ("A.b", Expectation::Fail),
            ("A.c", Expectation::Flaky("varied".to_owned())),
        ]);
        let listed: Vec<String> = expected.tests.keys().cloned().collect();
        let listed_run = |seen: Seen| {
            judge(&Inputs {
                mode: Mode::Listed,
                expected: &expected,
                expected_path: "expected.txt",
                listed: &listed,
                runs: &runs(&[("A.a", seen)]),
                alone: &BTreeMap::new(),
                failures: &[],
                stop: None,
            })
        };
        assert!(listed_run(Seen::Pass).is_empty());
        assert_eq!(rules(&listed_run(Seen::Fail)), [(Rule::Pass, Some("A.a"))]);
    }

    #[test]
    fn a_stop_fails_and_counts_what_it_left_unrun() {
        let expected = file(&[
            ("A.a", Expectation::Pass),
            ("B.a", Expectation::Pass),
            ("B.b", Expectation::Fail),
        ]);
        let listed: Vec<String> = expected.tests.keys().cloned().collect();
        let stop = Stop::Sentinel("A.a, expected to pass, saw fail".to_owned());
        let findings = judge(&Inputs {
            mode: Mode::Full,
            expected: &expected,
            expected_path: "expected.txt",
            listed: &listed,
            runs: &runs(&[("A.a", Seen::Fail)]),
            alone: &BTreeMap::new(),
            failures: &[],
            stop: Some(&stop),
        });
        assert_eq!(
            rules(&findings),
            [(Rule::Stopped, None), (Rule::Pass, Some("A.a"))]
        );
        assert!(fails(&findings));
        assert_eq!(
            findings[0].text,
            "the sentinel: A.a, expected to pass, saw fail; 1 tests expected to pass did not run"
        );
        let stop = Stop::Deadline(Duration::from_secs(900));
        let findings = judge(&Inputs {
            mode: Mode::Full,
            expected: &expected,
            expected_path: "expected.txt",
            listed: &listed,
            runs: &runs(&[
                ("A.a", Seen::Pass),
                ("B.a", Seen::Pass),
                ("B.b", Seen::Fail),
            ]),
            alone: &BTreeMap::new(),
            failures: &[],
            stop: Some(&stop),
        });
        assert_eq!(rules(&findings), [(Rule::Stopped, None)]);
        assert_eq!(findings[0].text, "the deadline: 15 min passed");
    }
}
