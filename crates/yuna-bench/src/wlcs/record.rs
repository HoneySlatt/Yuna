//! `wlcs record`: every test run in its own process, several times ([`record`]), the outcomes
//! seen written to a JSON file ([`RecordFile`]); and records merged into the file of expected
//! outcomes ([`merge`], [`merge_files`]). The parent module's documentation gives the record's
//! format.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use serde::{Deserialize, Serialize};

use super::expected::{self, Expectation, ExpectedFile};
use super::output::Seen;
use super::running::{Batch, ListFailure, ProcessFailure, Supervisor, TestRun, Verdict, Watch};
use super::{RunOptions, counts_text, duration, print_line};
use crate::cli::{utc, write_atomically};

/// The record's format; raised when a field changes its meaning.
pub const FORMAT: u32 = 1;

/// One `wlcs record`'s outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordFile {
    /// [`FORMAT`].
    pub format: u32,
    /// The version of WLCS that ran.
    pub wlcs: String,
    /// The integration module.
    pub module: String,
    /// The workspace's commit, `-dirty` when its tree differed from it.
    pub commit: Option<String>,
    /// The runs asked of each test.
    pub runs: u32,
    /// When it started, in UTC.
    pub started_utc: String,
    /// When it was written, in UTC.
    pub written_utc: String,
    /// Why it stopped before its end, if it did: the breaker. A stopped record is never merged.
    pub stopped: Option<String>,
    /// The processes that failed with no test to blame: never merged either.
    pub failures: Vec<String>,
    /// Each test WLCS lists, by name, with the outcome of each of its runs, in the order they
    /// ended: `pass`, `fail`, `skip`, `unsupported`, `shim-error`, `crash` or `hang`.
    pub tests: BTreeMap<String, Vec<Seen>>,
}

/// Reads the record at `path`.
pub fn read(path: &Path) -> Result<RecordFile, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: RecordFile =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.format != FORMAT {
        return Err(format!(
            "{}: format {} is not one this program reads (format {FORMAT})",
            path.display(),
            file.format
        ));
    }
    Ok(file)
}

/// The file of expected outcomes from `records`, each with its path: a test with one outcome over
/// every run of every record gets it; one whose outcome varied is flaky, `# varied: <counts>`.
/// Refuses, giving every reason: a record stopped, or with a process that failed; records of
/// other WLCS versions, or of other tests; a test never run; a test that crashed or hung in any
/// run, or met the module's error.
pub fn merge(records: &[(String, RecordFile)]) -> Result<ExpectedFile, Vec<String>> {
    let mut refusals = Vec::new();
    let Some((first_path, first)) = records.first() else {
        return Err(vec!["no record to merge".to_owned()]);
    };
    for (path, record) in records {
        if let Some(stopped) = &record.stopped {
            refusals.push(format!(
                "{path} stopped before its end ({stopped}): record again"
            ));
        }
        for failure in &record.failures {
            refusals.push(format!("{path}: a process failed: {failure}"));
        }
        if record.wlcs != first.wlcs {
            refusals.push(format!(
                "{path} ran WLCS {}, {first_path} WLCS {}",
                record.wlcs, first.wlcs
            ));
        }
        for name in record.tests.keys() {
            if !first.tests.contains_key(name) {
                refusals.push(format!("{path} names {name}, {first_path} does not"));
            }
        }
        for name in first.tests.keys() {
            if !record.tests.contains_key(name) {
                refusals.push(format!("{first_path} names {name}, {path} does not"));
            }
        }
    }
    let mut tests = BTreeMap::new();
    for name in first.tests.keys() {
        let mut counts: BTreeMap<Seen, usize> = BTreeMap::new();
        for (_, record) in records {
            for seen in record.tests.get(name).into_iter().flatten() {
                *counts.entry(*seen).or_default() += 1;
            }
        }
        if counts.is_empty() {
            refusals.push(format!("{name} never ran"));
            continue;
        }
        if counts
            .keys()
            .any(|seen| matches!(seen, Seen::Crash | Seen::Hang | Seen::NotRun))
        {
            refusals.push(format!(
                "{name} crashed or hung: {}; fix it, then record again",
                counts_text(&counts)
            ));
            continue;
        }
        if counts.contains_key(&Seen::ShimError) {
            refusals.push(format!(
                "{name} met the module's error: {}; fix the module, then record again",
                counts_text(&counts)
            ));
            continue;
        }
        let expectation = if counts.len() == 1 {
            match counts.keys().next() {
                Some(Seen::Pass) => Expectation::Pass,
                Some(Seen::Fail) => Expectation::Fail,
                Some(Seen::Skip) => Expectation::Skip,
                _ => Expectation::Unsupported,
            }
        } else {
            Expectation::Flaky(format!("varied: {}", counts_text(&counts)))
        };
        tests.insert(name.clone(), expectation);
    }
    if refusals.is_empty() {
        Ok(ExpectedFile {
            wlcs: first.wlcs.clone(),
            tests,
        })
    } else {
        Err(refusals)
    }
}

/// `wlcs record --merge`: merges the records at `inputs` and writes the file of expected
/// outcomes to `write`, atomically. Gives 0 written, 1 refused; an error is an input unreadable
/// or the file unwritable.
pub fn merge_files(inputs: &[PathBuf], write: &Path) -> Result<u8, String> {
    let mut records = Vec::new();
    for path in inputs {
        records.push((path.display().to_string(), read(path)?));
    }
    match merge(&records) {
        Ok(file) => {
            let text = expected::write(&file);
            write_atomically(write, |writer| writer.write_all(text.as_bytes()))?;
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for expectation in file.tests.values() {
                *counts.entry(expectation.name()).or_default() += 1;
            }
            let counts: Vec<String> = counts.iter().map(|(n, c)| format!("{n} {c}")).collect();
            println!(
                "yuna-bench wlcs: {} written, {} tests: {}",
                write.display(),
                file.tests.len(),
                counts.join(", ")
            );
            Ok(0)
        }
        Err(refusals) => {
            println!(
                "yuna-bench wlcs: nothing written; the records are refused, each for its reason:"
            );
            for refusal in refusals {
                println!("  {refusal}");
            }
            Ok(1)
        }
    }
}

/// What `wlcs record` does as each run ends: prints it; a hang counts toward the breaker, a
/// timeout does not, since no outcome is expected yet.
struct RecordWatch;

impl Watch for RecordWatch {
    fn ended(&self, run: &TestRun) -> Verdict {
        print_line(run.ended.seen, &run.batch, run.ended.note.as_deref(), None);
        Verdict {
            toward_breaker: run.ended.seen == Seen::Hang,
            stop: None,
        }
    }

    fn failed(&self, failure: &ProcessFailure) {
        print_line(failure.seen, &failure.batch, Some(&failure.text), None);
    }
}

/// `wlcs record --runs <n> --out <file>`: every test WLCS lists, each in its own process, `runs`
/// times, `--jobs` at once; writes the outcomes to `out`. Gives 0 when no test crashed, hung or
/// met the module's error and the record is whole, 1 otherwise, and 1 with nothing written when
/// WLCS crashed or hung while it listed its tests; an error is a run that could not start.
pub fn record(options: &RunOptions, runs: u32, out: &Path) -> Result<u8, String> {
    let started = Instant::now();
    let started_utc = utc(SystemTime::now());
    let supervisor = Supervisor::new(options.settings(None))?;
    let listing = match supervisor.list() {
        Ok(listing) => listing,
        Err(ListFailure::Unusable(text)) => return Err(text),
        Err(ListFailure::Broken(failure)) => {
            println!(
                "yuna-bench wlcs record: {}: {}; nothing recorded; its log is {}",
                failure.batch,
                failure.text,
                failure.log.display()
            );
            return Ok(1);
        }
    };
    let names: Vec<&str> = listing.names().collect();
    println!(
        "yuna-bench wlcs record: WLCS {}, {} tests, {runs} runs each, {} at once; module {}",
        options.version,
        names.len(),
        options.jobs,
        options.module.display()
    );
    let batches = (1..=runs)
        .flat_map(|round| {
            names.iter().map(move |name| Batch {
                label: format!("{name} run {round}"),
                patterns: vec![(*name).to_owned()],
                tests: vec![(*name).to_owned()],
            })
        })
        .collect();
    let ran = supervisor.run(batches, &RecordWatch);
    let mut tests: BTreeMap<String, Vec<Seen>> = names
        .iter()
        .map(|name| ((*name).to_owned(), Vec::new()))
        .collect();
    for run in &ran.runs {
        if run.ended.seen != Seen::NotRun
            && let Some(outcomes) = tests.get_mut(&run.ended.name)
        {
            outcomes.push(run.ended.seen);
        }
    }
    let stopped = supervisor.stop().map(|stop| stop.describe());
    let failures: Vec<String> = ran
        .failures
        .iter()
        .map(|f| format!("{}: {} (log: {})", f.batch, f.text, f.log.display()))
        .collect();
    let file = RecordFile {
        format: FORMAT,
        wlcs: options.version.clone(),
        module: options.module.display().to_string(),
        commit: crate::scenarios::commit(&[]),
        runs,
        started_utc,
        written_utc: utc(SystemTime::now()),
        stopped: stopped.clone(),
        failures: failures.clone(),
        tests,
    };
    if let Some(folder) = out.parent().filter(|f| !f.as_os_str().is_empty()) {
        fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    write_atomically(out, |writer| {
        serde_json::to_writer_pretty(&mut *writer, &file).map_err(io::Error::from)?;
        writer.write_all(b"\n")
    })?;
    let mut counts: BTreeMap<Seen, usize> = BTreeMap::new();
    let mut broken = Vec::new();
    let mut varied = 0;
    for (name, outcomes) in &file.tests {
        let mut own: BTreeMap<Seen, usize> = BTreeMap::new();
        for seen in outcomes {
            *counts.entry(*seen).or_default() += 1;
            *own.entry(*seen).or_default() += 1;
        }
        if [Seen::Crash, Seen::Hang, Seen::ShimError]
            .iter()
            .any(|seen| own.contains_key(seen))
        {
            broken.push(format!("{name}: {}", counts_text(&own)));
        }
        varied += usize::from(own.len() > 1);
    }
    println!();
    println!(
        "yuna-bench wlcs record: {} tests in {}: runs {}; {varied} varied",
        file.tests.len(),
        duration(started.elapsed()),
        counts_text(&counts)
    );
    for line in broken.iter().chain(&failures) {
        println!("  crashed, hung or met the module's error: {line}");
    }
    if let Some(stopped) = &stopped {
        println!("  stopped: {stopped}");
    }
    println!("{}", out.display());
    let whole = broken.is_empty() && failures.is_empty() && stopped.is_none();
    if whole {
        super::remove_work(options);
    } else {
        println!(
            "yuna-bench wlcs record: the merge will refuse this record; the logs are in {}",
            options.work.display()
        );
    }
    Ok(u8::from(!whole))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(wlcs: &str, tests: &[(&str, &[Seen])]) -> RecordFile {
        RecordFile {
            format: FORMAT,
            wlcs: wlcs.to_owned(),
            module: "/target/debug/libyuna_wlcs.so".to_owned(),
            commit: None,
            runs: 2,
            started_utc: "2026-11-21T20:00:00Z".to_owned(),
            written_utc: "2026-11-21T20:10:00Z".to_owned(),
            stopped: None,
            failures: Vec::new(),
            tests: tests
                .iter()
                .map(|(name, seen)| ((*name).to_owned(), seen.to_vec()))
                .collect(),
        }
    }

    #[test]
    fn agreeing_runs_give_their_outcome() {
        use Seen::*;
        let a = record(
            "1.8.1",
            &[
                ("A.a", &[Pass, Pass]),
                ("A.b", &[Fail, Fail]),
                ("A.c", &[Skip, Skip]),
                ("A.d", &[Unsupported, Unsupported]),
            ],
        );
        let b = record(
            "1.8.1",
            &[
                ("A.a", &[Pass]),
                ("A.b", &[Fail]),
                ("A.c", &[Skip]),
                ("A.d", &[Unsupported]),
            ],
        );
        let file = merge(&[("a.json".to_owned(), a), ("b.json".to_owned(), b)]).unwrap();
        assert_eq!(file.wlcs, "1.8.1");
        assert_eq!(file.tests["A.a"], Expectation::Pass);
        assert_eq!(file.tests["A.b"], Expectation::Fail);
        assert_eq!(file.tests["A.c"], Expectation::Skip);
        assert_eq!(file.tests["A.d"], Expectation::Unsupported);
    }

    #[test]
    fn varying_runs_make_a_test_flaky_with_their_counts() {
        use Seen::*;
        let a = record("1.8.1", &[("A.a", &[Pass, Fail]), ("A.b", &[Skip, Skip])]);
        let b = record(
            "1.8.1",
            &[("A.a", &[Pass, Pass]), ("A.b", &[Skip, Unsupported])],
        );
        let file = merge(&[("a.json".to_owned(), a), ("b.json".to_owned(), b)]).unwrap();
        assert_eq!(
            file.tests["A.a"],
            Expectation::Flaky("varied: pass 3, fail 1".to_owned())
        );
        assert_eq!(
            file.tests["A.b"],
            Expectation::Flaky("varied: skip 3, unsupported 1".to_owned())
        );
        // The file it gives reads back.
        assert_eq!(expected::parse(&expected::write(&file)).unwrap(), file);
    }

    #[test]
    fn a_crash_or_a_hang_is_refused_and_named() {
        use Seen::*;
        let a = record("1.8.1", &[("A.a", &[Pass, Pass]), ("A.b", &[Fail, Crash])]);
        let b = record("1.8.1", &[("A.a", &[Pass, Hang]), ("A.b", &[Fail, Fail])]);
        let refusals = merge(&[("a.json".to_owned(), a), ("b.json".to_owned(), b)]).unwrap_err();
        assert_eq!(
            refusals,
            [
                "A.a crashed or hung: pass 3, hang 1; fix it, then record again",
                "A.b crashed or hung: fail 3, crash 1; fix it, then record again",
            ]
        );
    }

    #[test]
    fn a_shim_error_is_refused_and_named() {
        use Seen::*;
        let a = record(
            "1.8.1",
            &[("A.a", &[Pass, Pass]), ("A.b", &[Pass, ShimError])],
        );
        let refusals = merge(&[("a.json".to_owned(), a)]).unwrap_err();
        assert_eq!(
            refusals,
            ["A.b met the module's error: pass 1, shim-error 1; fix the module, then record again"]
        );
        // The same runs without it merge.
        let a = record("1.8.1", &[("A.a", &[Pass, Pass]), ("A.b", &[Pass, Pass])]);
        assert!(merge(&[("a.json".to_owned(), a)]).is_ok());
    }

    #[test]
    fn records_that_disagree_or_stopped_are_refused() {
        use Seen::*;
        let a = record("1.8.1", &[("A.a", &[Pass]), ("A.b", &[Pass])]);
        let mut b = record("1.9.0", &[("A.a", &[Pass]), ("A.c", &[Pass])]);
        b.stopped = Some("the breaker".to_owned());
        b.failures = vec!["A.b run 1: exit 1".to_owned()];
        let refusals =
            merge(&[("a.json".to_owned(), a.clone()), ("b.json".to_owned(), b)]).unwrap_err();
        assert_eq!(
            refusals,
            [
                "b.json stopped before its end (the breaker): record again",
                "b.json: a process failed: A.b run 1: exit 1",
                "b.json ran WLCS 1.9.0, a.json WLCS 1.8.1",
                "b.json names A.c, a.json does not",
                "a.json names A.b, b.json does not",
            ]
        );
        let never = record("1.8.1", &[("A.a", &[Pass]), ("A.b", &[])]);
        assert_eq!(
            merge(&[("a.json".to_owned(), never)]).unwrap_err(),
            ["A.b never ran"]
        );
        assert_eq!(merge(&[]).unwrap_err(), ["no record to merge"]);
    }

    #[test]
    fn a_record_reads_back_and_refuses_another_format() {
        use Seen::*;
        let folder = crate::testing::folder("wlcs-record-read");
        let path = folder.join("record.json");
        let file = record("1.8.1", &[("A.a", &[Pass, Fail])]);
        fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).unwrap();
        assert_eq!(read(&path).unwrap(), file);
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"A.a\": [\n      \"pass\",\n      \"fail\"\n    ]"),
            "{text}"
        );
        fs::write(&path, text.replace("\"format\": 1", "\"format\": 2")).unwrap();
        assert!(read(&path).unwrap_err().contains("format 2 is not one"));
        fs::write(
            &path,
            text.replace("\"runs\": 2", "\"runs\": 2, \"extra\": 1"),
        )
        .unwrap();
        assert!(read(&path).unwrap_err().contains("unknown field `extra`"));
    }
}
