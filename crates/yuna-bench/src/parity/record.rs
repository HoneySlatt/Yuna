//! The parity run's file: one JSON file per run of `yuna-bench parity run`, versioned by
//! [`FORMAT`], written again after each measurement through a temporary file and a rename, so
//! that a run cut short keeps what it measured.
//!
//! A figure's value is `null` when it could not be measured, never 0; its verdict says why.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::build::SideInfo;
use super::figures::{Finished, Judged, What};
use crate::machine::MachineInfo;

/// The format of the parity run's file; raised when a field changes its meaning.
pub const FORMAT: u32 = 1;

/// One parity run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunFile {
    /// [`FORMAT`].
    pub format: u32,
    /// The commit of the script that ran, and whether its tree differed from it.
    pub script_commit: Option<String>,
    /// When it started, in UTC.
    pub started_utc: String,
    /// When the file was last written, in UTC.
    pub written_utc: Option<String>,
    /// The script's arguments.
    pub arguments: Vec<String>,
    /// The tiers: `ci`, `gpu`, `timed` or `timed,gpu`.
    pub tier: String,
    /// The seed of the runs' random order.
    pub seed: u64,
    /// The machine, the same for both builds: figures from two machines are never compared.
    pub machine: MachineInfo,
    /// valgrind's version.
    pub valgrind: Option<String>,
    /// The base, then the head.
    pub sides: Vec<SideInfo>,
    /// The runs' labels, in the order they were drawn.
    pub order: Vec<String>,
    /// Each run made, in the order it ended.
    pub runs: Vec<RunRecord>,
    /// Each figure, judged, once every run ended.
    pub figures: Vec<FigureRecord>,
    /// The check of a move of the baseline: the head against the old baseline, at the push that
    /// moves it.
    #[serde(default)]
    pub move_check: Option<MoveCheck>,
    /// For each figure worse, whether the push moved it: the push before against the head.
    #[serde(default)]
    pub pushes: Vec<PushNote>,
    /// The timed tier's soak: each side's readings.
    #[serde(default)]
    pub soak: Option<SoakRecord>,
    /// The timed cases some of whose runs could not read the other programs' GPU load, as
    /// `<group>: <n> of <m> runs`: a game on the GPU would not have marked them disturbed.
    #[serde(default)]
    pub gpu_load_unread: Vec<String>,
    /// Facts a reader needs beside the figures.
    pub notes: Vec<String>,
    /// The program's exit status: 0 no figure worse, 1 a figure worse, 2 a figure void or an
    /// error; none until the run ends.
    pub exit: Option<u8>,
}

/// The check of a move of the baseline.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MoveCheck {
    /// The old baseline's commit.
    pub old: String,
    /// What the new moves name.
    pub named: Vec<String>,
    /// The figures worse than the old baseline that a move names: moved on purpose.
    pub worse: Vec<String>,
    /// Those no move names, or not measured against the old baseline: the run is void.
    pub unnamed: Vec<String>,
}

/// What the soak read of each side.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SoakRecord {
    /// Its length after the instances settled, in seconds.
    pub seconds: u64,
    /// The instances of each side.
    pub instances: usize,
    /// The base's readings: minutes from the first, and the `Pss_Anon` of its instances, summed,
    /// in KiB.
    pub base: Vec<(f64, f64)>,
    /// The head's.
    pub head: Vec<(f64, f64)>,
    /// Why a side could not be read to the end.
    pub errors: Vec<String>,
}

/// Whether a push moved a worse figure.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PushNote {
    /// The figure.
    pub figure: String,
    /// What the push before against the head says.
    pub said: String,
}

/// One run made.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    /// Its label: `head/partial/dhat-80/1`.
    pub label: String,
    /// Its real time, in seconds.
    pub seconds: f64,
    /// Why it failed.
    pub error: Option<String>,
    /// Its readings, raw: see `scenario::run`.
    pub readings: BTreeMap<String, f64>,
    /// A scenario's work in its measured periods: frames, frames by cause, commits counted by
    /// Yuna, and what the client counted.
    pub work: BTreeMap<String, u64>,
    /// An operation's digest.
    pub digest: Option<String>,
    /// Facts a reader needs beside it.
    pub notes: Vec<String>,
    /// For a timed run, why the machine's load disturbed it: its pair was dropped.
    #[serde(default)]
    pub disturbed: Option<String>,
}

impl RunRecord {
    /// The record of a run made.
    pub fn of(run: &Finished) -> RunRecord {
        let outcome = &run.outcome;
        let mut work = BTreeMap::new();
        if let What::Scenario { .. } = run.job.what
            && outcome.error.is_none()
        {
            let w = &outcome.work;
            work.insert("frames".to_owned(), w.frames);
            for (cause, frames) in &w.causes {
                work.insert(format!("frames:{cause}"), *frames);
            }
            work.insert("commits".to_owned(), w.commits);
            work.insert("client:commits".to_owned(), w.client.commits);
            work.insert("client:callbacks".to_owned(), w.client.callbacks);
            work.insert("client:pointer".to_owned(), w.client.pointer);
            work.insert("client:configures".to_owned(), w.client.configures);
            work.insert("client:buffers".to_owned(), w.client.buffers);
        }
        RunRecord {
            label: run.job.label(),
            seconds: outcome.seconds,
            error: outcome.error.clone(),
            readings: outcome.readings.clone(),
            work,
            digest: outcome.digest.clone(),
            notes: outcome.notes.clone(),
            disturbed: outcome.disturbed.clone(),
        }
    }
}

/// One figure, judged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FigureRecord {
    /// Its name: `partial: instructions a frame, yuna`.
    pub name: String,
    /// Its unit.
    pub unit: String,
    /// `parity`, `budget` or `recorded`.
    pub class: String,
    /// Its budget, for a budget figure.
    pub budget: Option<f64>,
    /// Its threshold's share of the base's value.
    pub threshold_relative: f64,
    /// Its threshold's floor, in its unit.
    pub threshold_absolute: f64,
    /// Where the threshold comes from.
    pub threshold_basis: String,
    /// The base's median, `null` if not measured.
    pub base: Option<f64>,
    /// The head's median.
    pub head: Option<f64>,
    /// The base's measurements.
    pub base_values: Vec<f64>,
    /// The head's measurements.
    pub head_values: Vec<f64>,
    /// The larger of the two sides' spreads: range over median.
    pub spread: Option<f64>,
    /// `no worse`, `worse`, `inconclusive`, `recorded` or `void`.
    pub verdict: String,
    /// Why it is void.
    pub reason: Option<String>,
    /// Whether it is the timed tier's: judged over pairs, or the soak's.
    #[serde(default)]
    pub timed: bool,
    /// For a timed figure, the interval it was judged on.
    #[serde(default)]
    pub judgement: Option<JudgementRecord>,
}

/// The interval a timed figure was judged on (`rule.rs`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JudgementRecord {
    /// What it bounds: `ln(head / base)` over the quiet pairs, or the head's soak slope.
    pub on: String,
    /// The quiet pairs, or the soak's readings.
    pub count: usize,
    /// The pairs dropped as disturbed.
    pub dropped: usize,
    /// The mean.
    pub mean: f64,
    /// The values' standard deviation: the spread a threshold is set from.
    pub deviation: f64,
    /// The one-sided 95 % lower bound on the mean.
    pub lower: f64,
    /// The one-sided 95 % upper bound.
    pub upper: f64,
}

impl FigureRecord {
    /// The record of a judged figure.
    pub fn of(figure: &Judged) -> FigureRecord {
        FigureRecord {
            name: figure.name.clone(),
            unit: figure.kind.unit.to_owned(),
            class: figure.class.name().to_owned(),
            budget: match figure.class {
                super::rule::Class::Budget(budget) => Some(budget),
                _ => None,
            },
            threshold_relative: figure.threshold.as_ref().map_or(0.0, |(t, _)| t.relative),
            threshold_absolute: figure.threshold.as_ref().map_or(0.0, |(t, _)| t.absolute),
            threshold_basis: figure
                .threshold
                .as_ref()
                .map_or_else(String::new, |(_, basis)| basis.clone()),
            base: figure.medians[0],
            head: figure.medians[1],
            base_values: figure.values[0].clone(),
            head_values: figure.values[1].clone(),
            spread: figure.spread,
            verdict: figure.verdict.name().to_owned(),
            reason: figure.verdict.reason().map(str::to_owned),
            timed: figure.kind.is_timed(),
            judgement: figure.judgement.map(|judgement| JudgementRecord {
                on: judgement.on.to_owned(),
                count: judgement.interval.count,
                dropped: judgement.dropped,
                mean: judgement.interval.mean,
                deviation: judgement.interval.deviation,
                lower: judgement.interval.lower,
                upper: judgement.interval.upper,
            }),
        }
    }
}

/// Writes `file` to `path`, atomically, the time it is written in it.
pub fn write(path: &Path, file: &mut RunFile) -> Result<(), String> {
    file.written_utc = Some(crate::cli::utc(std::time::SystemTime::now()));
    crate::cli::write_atomically(path, |writer| {
        serde_json::to_writer_pretty(writer, file).map_err(std::io::Error::from)
    })
}

/// Reads a parity run's file; refuses another format.
pub fn read(text: &str) -> Result<RunFile, String> {
    let file: RunFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if file.format != FORMAT {
        return Err(format!(
            "format {}: this script reads format {FORMAT}",
            file.format
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_file_is_written_whole_and_read_back() {
        let folder = crate::testing::folder("parity-run-file");
        let path = folder.join("parity.json");
        let mut file = RunFile {
            format: FORMAT,
            started_utc: "2026-11-19T15:00:00Z".to_owned(),
            tier: "ci".to_owned(),
            seed: 7,
            figures: vec![FigureRecord {
                name: "partial: Pss_Anon".to_owned(),
                base: Some(1.0),
                head: None,
                verdict: "void".to_owned(),
                reason: Some("head: Yuna ended with 101".to_owned()),
                ..FigureRecord::default()
            }],
            ..RunFile::default()
        };
        write(&path, &mut file).unwrap();
        let read_back = read(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(read_back.written_utc.is_some());
        assert_eq!(read_back, file);
        // A void figure's value is null, never 0.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"head\": null"), "{text}");
        let left: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(left, ["parity.json"]);
        assert!(read(&text.replace("\"format\": 1", "\"format\": 2")).is_err());
    }
}
