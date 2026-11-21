//! The timed tier
//! ([ADR-0142](../../../../docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)):
//! figures the machine's load moves, run once per milestone by an agent under
//! [ADR-0115](../../../../docs/adr/0115-priorities-uwui-first-the-owners-activity-measures-that-check-their-load.md)'s
//! rules, and judged over *pairs* by the rule of `rule.rs` ([`rule::decide`]).
//!
//! **The cases.** Each is run natively, one run at a time, never beside another of the script's:
//!
//! - *frames*: `partial` (1080p, a 64 × 64 square a frame), `scaled` (4K at scale 1.5, the same
//!   square) and `full-4k` (4K at scale 1.5, two windows whole each frame), on the manual clock
//!   with the lockstep client, as the CI tier runs them, but with llvmpipe on its own threads and
//!   Yuna logging each frame at debug. Their figures: *a frame's drawing time*, the median of the
//!   measured frames' `took_us`, and Yuna's *CPU time a frame*, every thread of its process. On
//!   llvmpipe, `full-4k`'s are *recorded*: ADR-0139 judges the processor tier on its own budget,
//!   which no ROADMAP row or ADR sets yet (ADR-0082 item 26 names none); `partial`'s and
//!   `scaled`'s are *parity*.
//! - *starts* (`start.rs`): `start-1080p` and `start-4k`, cold, timed to `seat create`'s reply and
//!   to the seat's first frame. As the owner chose on 2026-11-21: their *parity* figures are
//!   judged over pairs whose `TMPDIR` is full, 40,000 files, which guards `58f2e75`; their
//!   *budget* figures, ADR-0100's, at that ADR's conditions, on an empty `TMPDIR`: after the
//!   pairs, the head alone starts until it has 20 quiet starts (the starts' least pairs), and the
//!   longest of exactly those 20 is judged against 100 ms to the reply, and 150 ms to the first
//!   frame on llvmpipe, 100 on the GPU. A disturbed one is replaced until 20 were dropped; fewer
//!   than 20 quiet ones make the budgets void.
//! - the *soak* (`soak.rs`), once, after the pairs: its slope, against no growth.
//!
//! With `--tier timed,gpu`, the frames and the starts run on the GPU too, their groups named
//! `<case>-gpu`; there every figure is *parity* but the budgets.
//!
//! **The order.** One warm-up run of each build comes first, discarded: the page cache then holds
//! both builds and their Mesa. Then each case's least pairs, every case's mixed in an order drawn
//! from the run's seed, each pair's two runs in an order drawn from it too: a fixed order would
//! bias the ratio by whatever the first run of a pair leaves to the second. After each batch, each
//! figure is decided again; one that needs more pairs gets them, a batch at a time, up to its cap.
//!
//! **Disturbed runs** ([`disturbance`]): the machine's state is read each second of each run
//! (`machine.rs`). A run is disturbed when the other programs used more than ADR-0115's limits,
//! [`OTHERS_CPU_LIMIT`] cores or [`OTHERS_GPU_LIMIT`] % of the GPU's drawing engines, on average
//! or in any second; or when Yuna's own threads (its main thread and its `yuna-…` ones) waited on
//! the run queue for more than [`RUN_QUEUE_LIMIT`] of the time they ran: a busy machine delays
//! them even when the share of its other programs looks low. A pair with a disturbed run is
//! dropped and replaced, up to the cap; a figure left with fewer quiet pairs than its least is
//! void. A run that fails voids its case's figures: a failure is never taken for noise.
//!
//! **The GPU's load unread** ([`gpu_load_unknown`]): inside an agent's sandbox the other
//! programs' DRM files are hidden, so their share of the GPU is unknown. On llvmpipe the run file
//! and the report's headline say so for each case it touched ([`gpu_load_unread`]); a case on the
//! GPU, which a game there would slow unseen, is void then, unless `--gpu-load-unknown-ok` says
//! by hand that the GPU was free.
//!
//! **Equal work**: the frames' runs of a case, on both sides, must draw the same frames for the
//! same causes and give the client the same events, as in the CI tier (`figures::equal_work`).

use std::collections::BTreeMap;

use super::figures::{
    self, DRAWING_TIME, FIRST_FRAME, FIRST_FRAME_LONGEST, FRAME_CPU, Finished, Job, Judged,
    Judgement, Judging, Kind, REPLY, REPLY_LONGEST, SOAK_GROWTH, Side, What,
};
use super::rule::{self, Class, Decision, Interval, PairLimits, Verdict};
use super::scenario::{self, Device, FULL_4K, Instrument, Scenario};
use super::soak::{self, Series};
use super::start::{STARTS, Start, Tmpdir};
use super::thresholds::Entry;
use crate::cli::SplitMix;
use crate::machine::{OTHERS_CPU_LIMIT, OTHERS_GPU_LIMIT};

/// Above this share of the time Yuna's own threads ran, their wait on the run queue disturbs a
/// run. A quiet machine keeps it near nothing; a build of the workspace beside a run takes it
/// far above.
pub const RUN_QUEUE_LIMIT: f64 = 0.05;

/// The measured periods of a timed run of `partial` and `scaled`: one second at 240 Hz.
pub const FRAME_PERIODS: u32 = 240;
/// Those of `full-4k`, whose frames take about 8 ms each on llvmpipe.
pub const FULL_FRAME_PERIODS: u32 = 120;

/// The frames' pairs: at least 10, at most 30, 5 more at a time.
pub const FRAME_PAIRS: PairLimits = PairLimits {
    least: 10,
    most: 30,
    batch: 5,
};
/// The starts' pairs: at least 20, so that the budget is the longest of 20 starts, as ADR-0100
/// states it; at most 60, 10 more at a time.
pub const START_PAIRS: PairLimits = PairLimits {
    least: 20,
    most: 60,
    batch: 10,
};

/// The timed tier's parts, as `--scenario` names them.
pub const PARTS: [&str; 6] = [
    "partial",
    "scaled",
    "full-4k",
    "start-1080p",
    "start-4k",
    "soak",
];

/// What one pair runs on each side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Case {
    /// A scenario's frames.
    Frames {
        /// The scenario.
        scenario: &'static Scenario,
        /// What Yuna draws with.
        device: Device,
    },
    /// A cold start.
    Start {
        /// Its output's mode.
        start: &'static Start,
        /// What Yuna draws with.
        device: Device,
    },
}

impl Case {
    /// Its group, which its figures are named and their thresholds looked up by: its scenario's or
    /// start's name, `-gpu` after it on the GPU.
    pub fn group(self) -> String {
        let (name, device) = match self {
            Case::Frames { scenario, device } => (scenario.name, device),
            Case::Start { start, device } => (start.name, device),
        };
        match device {
            Device::Llvmpipe => name.to_owned(),
            Device::Gpu => format!("{name}-gpu"),
        }
    }

    /// The job of `side`'s run of pair `pair`, 0 for a warm-up.
    pub fn job(self, side: Side, pair: u32) -> Job {
        let what = match self {
            Case::Frames { scenario, device } => What::Scenario {
                scenario,
                instrument: Instrument::Timed(device),
                periods: if scenario.name == FULL_4K.name {
                    FULL_FRAME_PERIODS
                } else {
                    FRAME_PERIODS
                },
            },
            Case::Start { start, device } => What::Start {
                start,
                device,
                tmpdir: Tmpdir::Full,
            },
        };
        Job {
            side,
            what,
            repeat: pair,
        }
    }

    /// The job of the head's `number`th start for ADR-0100's budgets, from 1, on an empty
    /// `TMPDIR`; none for a case that has no budget.
    pub fn budget_job(self, number: u32) -> Option<Job> {
        match self {
            Case::Frames { .. } => None,
            Case::Start { start, device } => Some(Job {
                side: Side::Head,
                what: What::Start {
                    start,
                    device,
                    tmpdir: Tmpdir::Empty,
                },
                repeat: number,
            }),
        }
    }

    /// Whether `job` is one of the head's starts for its budgets.
    fn budget_runs(self, job: &Job) -> bool {
        match (self, &job.what) {
            (
                Case::Start { start, device },
                What::Start {
                    start: s,
                    device: d,
                    tmpdir: Tmpdir::Empty,
                },
            ) => s.name == start.name && *d == device && job.side == Side::Head,
            _ => false,
        }
    }

    /// What Yuna draws with.
    fn device(self) -> Device {
        match self {
            Case::Frames { device, .. } | Case::Start { device, .. } => device,
        }
    }

    /// Whether `job` is one of its runs.
    fn runs(self, job: &Job) -> bool {
        match (self, &job.what) {
            (
                Case::Frames { scenario, device },
                What::Scenario {
                    scenario: s,
                    instrument,
                    ..
                },
            ) => s.name == scenario.name && *instrument == Instrument::Timed(device),
            (
                Case::Start { start, device },
                What::Start {
                    start: s,
                    device: d,
                    tmpdir: Tmpdir::Full,
                },
            ) => s.name == start.name && *d == device,
            _ => false,
        }
    }

    /// Its figures judged over pairs: each kind, and how a run gives its value. Their classes
    /// are the thresholds file's, by the case's group: `full-4k`'s recorded on llvmpipe
    /// (ADR-0139).
    fn figures(self) -> Vec<(Kind, Value)> {
        match self {
            Case::Frames { .. } => vec![
                (DRAWING_TIME, Value::Reading("took_us")),
                (FRAME_CPU, Value::CpuAFrame),
            ],
            Case::Start { .. } => vec![
                (REPLY, Value::Reading("reply_ms")),
                (FIRST_FRAME, Value::Reading("first_frame_ms")),
            ],
        }
    }

    /// Its budget figures, judged on the head's longest start on an empty `TMPDIR`: the
    /// thresholds file gives their budgets, the GPU's first frame its own.
    fn budgets(self) -> Vec<(Kind, Value)> {
        match self {
            Case::Frames { .. } => Vec::new(),
            Case::Start { .. } => vec![
                (REPLY_LONGEST, Value::Reading("reply_ms")),
                (FIRST_FRAME_LONGEST, Value::Reading("first_frame_ms")),
            ],
        }
    }

    fn limits(self, settings: &Settings) -> PairLimits {
        match self {
            Case::Frames { .. } => settings.frame_pairs,
            Case::Start { .. } => settings.start_pairs,
        }
    }
}

/// How a run gives a figure's value.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Value {
    /// One of its readings.
    Reading(&'static str),
    /// Its CPU time over its frames, in µs.
    CpuAFrame,
}

impl Value {
    fn of(self, run: &Finished) -> Result<f64, String> {
        let reading = |name: &str| {
            run.outcome
                .readings
                .get(name)
                .copied()
                .ok_or_else(|| format!("{} read no {name}", run.job.label()))
        };
        match self {
            Value::Reading(name) => reading(name),
            Value::CpuAFrame => {
                let frames = run.outcome.work.frames;
                if frames == 0 {
                    return Err(format!("{} drew no frame", run.job.label()));
                }
                Ok(reading("cpu_ns")? / frames as f64 / 1e3)
            }
        }
    }
}

/// The timed tier's cases among `parts`, on llvmpipe, and on the GPU too when `gpu`.
pub fn cases(parts: &[String], gpu: bool) -> Vec<Case> {
    let asked = |name: &str| parts.iter().any(|part| part == name);
    let devices: &[Device] = if gpu {
        &[Device::Llvmpipe, Device::Gpu]
    } else {
        &[Device::Llvmpipe]
    };
    let mut cases = Vec::new();
    for &device in devices {
        for scenario in [
            scenario::named("partial").expect("partial"),
            scenario::named("scaled").expect("scaled"),
            &FULL_4K,
        ] {
            if asked(scenario.name) {
                cases.push(Case::Frames { scenario, device });
            }
        }
        for start in &STARTS {
            if asked(start.name) {
                cases.push(Case::Start { start, device });
            }
        }
    }
    cases
}

/// What the timed tier takes beside its cases.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// The frames' pairs.
    pub frame_pairs: PairLimits,
    /// The starts'.
    pub start_pairs: PairLimits,
    /// The seed of the pairs' order.
    pub seed: u64,
    /// Whether a case on the GPU is judged although the other programs' GPU load could not be
    /// read (`--gpu-load-unknown-ok`, by hand alone).
    pub gpu_load_unknown_ok: bool,
}

/// Why a timed run's `readings` say the machine disturbed it, if they do: the other programs'
/// load beyond ADR-0115's limits, on average or in any second, or Yuna's own threads' wait on the
/// run queue beyond [`RUN_QUEUE_LIMIT`] of their time on a core.
pub fn disturbance(readings: &BTreeMap<String, f64>) -> Option<String> {
    let mut reasons = Vec::new();
    let mut over = |key: &str, limit: f64, said: &str| {
        if let Some(value) = readings.get(key)
            && *value > limit
        {
            reasons.push(format!("{said} {value:.2}, above {limit}"));
        }
    };
    over(
        "others_cpu_cores_mean",
        OTHERS_CPU_LIMIT,
        "the other programs' cores on average",
    );
    over(
        "others_cpu_cores_max",
        OTHERS_CPU_LIMIT,
        "the other programs' cores in a second",
    );
    over(
        "others_gpu_percent_mean",
        OTHERS_GPU_LIMIT,
        "the other programs' % of the GPU on average",
    );
    over(
        "others_gpu_percent_max",
        OTHERS_GPU_LIMIT,
        "the other programs' % of the GPU in a second",
    );
    if let (Some(wait), Some(ran)) = (readings.get("own_wait_ns"), readings.get("own_cpu_ns"))
        && *ran > 0.0
        && wait / ran > RUN_QUEUE_LIMIT
    {
        reasons.push(format!(
            "Yuna's own threads waited on the run queue {:.1} % of their time on a core, above \
             {:.0} %",
            wait / ran * 100.0,
            RUN_QUEUE_LIMIT * 100.0
        ));
    }
    (!reasons.is_empty()).then(|| reasons.join("; "))
}

/// Runs the pairs of `cases` by `run`, which makes one run of a job and gives it, its load read,
/// and gives each run made to `made` once its disturbance is set; gives every run, the warm-ups
/// first.
pub fn run_pairs(
    cases: &[Case],
    settings: &Settings,
    judging: Judging,
    mut run: impl FnMut(&Job) -> Finished,
    mut made: impl FnMut(&Finished),
) -> Vec<Finished> {
    let mut random = SplitMix(settings.seed);
    let mut finished = Vec::new();
    let mut make = |job: &Job, finished: &mut Vec<Finished>| {
        let mut done = run(job);
        if done.outcome.error.is_none() {
            done.outcome.disturbed = disturbance(&done.outcome.readings);
        }
        made(&done);
        finished.push(done);
    };
    if let Some(first) = cases.first() {
        for side in [Side::Base, Side::Head] {
            make(&first.job(side, 0), &mut finished);
        }
    }
    let mut wanted: Vec<usize> = cases
        .iter()
        .map(|case| case.limits(settings).least)
        .collect();
    let mut last_pair = vec![0_u32; cases.len()];
    loop {
        let mut batch: Vec<(usize, u32)> = Vec::new();
        for (index, more) in wanted.iter().enumerate() {
            for _ in 0..*more {
                last_pair[index] += 1;
                batch.push((index, last_pair[index]));
            }
        }
        if batch.is_empty() {
            break;
        }
        crate::cli::shuffle_with(&mut batch, &mut random);
        for (index, pair) in batch {
            let mut order = [Side::Base, Side::Head];
            if random.next() % 2 == 1 {
                order.reverse();
            }
            for side in order {
                make(&cases[index].job(side, pair), &mut finished);
            }
        }
        wanted = cases
            .iter()
            .map(|case| {
                assess(*case, &finished, settings, judging)
                    .iter()
                    .map(|(_, more)| *more)
                    .max()
                    .unwrap_or(0)
            })
            .collect();
    }
    // Then the head's starts for the budgets, on an empty `TMPDIR`, until each case has its
    // least quiet ones.
    let mut last_start = vec![0_u32; cases.len()];
    loop {
        let mut batch: Vec<Job> = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            for _ in 0..budget_starts_needed(*case, &finished, settings) {
                last_start[index] += 1;
                batch.extend(case.budget_job(last_start[index]));
            }
        }
        if batch.is_empty() {
            break;
        }
        crate::cli::shuffle_with(&mut batch, &mut random);
        for job in &batch {
            make(job, &mut finished);
        }
    }
    finished
}

/// The head's starts on an empty `TMPDIR` of `case` among `finished`: the quiet ones' values in
/// the order they ran, and how many were dropped as disturbed; or why its budgets are void.
fn budget_starts(case: Case, finished: &[Finished]) -> Result<(Vec<&Finished>, usize), String> {
    let runs: Vec<&Finished> = finished
        .iter()
        .filter(|run| case.budget_runs(&run.job))
        .collect();
    if let Some(failed) = runs.iter().find(|run| run.outcome.error.is_some()) {
        return Err(format!(
            "{} failed: {}",
            failed.job.label(),
            failed.outcome.error.as_deref().unwrap_or_default()
        ));
    }
    let dropped = runs
        .iter()
        .filter(|run| run.outcome.disturbed.is_some())
        .count();
    let quiet = runs
        .into_iter()
        .filter(|run| run.outcome.disturbed.is_none())
        .collect();
    Ok((quiet, dropped))
}

/// How many more of the head's starts `case`'s budgets need: up to its least, the starts' least
/// pairs, 20 by default; each disturbed one is replaced until as many were dropped.
fn budget_starts_needed(case: Case, finished: &[Finished], settings: &Settings) -> usize {
    if case.budgets().is_empty() {
        return 0;
    }
    let least = settings.start_pairs.least;
    match budget_starts(case, finished) {
        Ok((quiet, dropped)) if dropped < least => least.saturating_sub(quiet.len()),
        _ => 0,
    }
}

/// Every figure of `cases`, judged from the runs `finished`.
pub fn judge(
    cases: &[Case],
    finished: &[Finished],
    settings: &Settings,
    judging: Judging,
) -> Vec<Judged> {
    cases
        .iter()
        .flat_map(|case| {
            assess(*case, finished, settings, judging)
                .into_iter()
                .map(|(mut judged, more)| {
                    if more > 0 {
                        judged.verdict = Verdict::Void("its pairs were not all run".to_owned());
                    }
                    judged
                })
                .chain(judge_budgets(*case, finished, settings, judging))
        })
        .collect()
}

/// Whether the other programs' GPU load could not be read over `run`: inside an agent's
/// sandbox, their DRM files are hidden.
pub fn gpu_load_unknown(run: &Finished) -> bool {
    run.outcome.error.is_none() && !run.outcome.readings.contains_key("others_gpu_percent_mean")
}

/// Each case some of whose runs among `finished` could not read the other programs' GPU load:
/// `<group>: <n> of <m> runs`, for the run file and the report's headline.
pub fn gpu_load_unread(cases: &[Case], finished: &[Finished]) -> Vec<String> {
    cases
        .iter()
        .filter_map(|case| {
            let runs: Vec<&Finished> = finished
                .iter()
                .filter(|run| case.runs(&run.job) || case.budget_runs(&run.job))
                .collect();
            let unread = runs.iter().filter(|run| gpu_load_unknown(run)).count();
            (unread > 0).then(|| format!("{}: {unread} of {} runs", case.group(), runs.len()))
        })
        .collect()
}

/// Why `case`'s `runs` cannot be judged for the GPU's load: on the GPU, a run whose other
/// programs' GPU load could not be read might share the GPU with a game unseen, unless
/// `--gpu-load-unknown-ok` says so by hand. On llvmpipe the report's headline carries it alone.
fn gpu_load_refused(case: Case, runs: &[&Finished], settings: &Settings) -> Option<String> {
    let unknown = runs.iter().filter(|run| gpu_load_unknown(run)).count();
    (case.device() == Device::Gpu && unknown > 0 && !settings.gpu_load_unknown_ok).then(|| {
        format!(
            "the other programs' GPU load could not be read in {unknown} of its runs, so a game \
             on the GPU would go unseen: run where it can be read, or give \
             --gpu-load-unknown-ok"
        )
    })
}

/// The budget figures of `case`: the longest of the head's first quiet starts on an empty
/// `TMPDIR`, exactly the starts' least, 20 by default, against the thresholds file's budget.
fn judge_budgets(
    case: Case,
    finished: &[Finished],
    settings: &Settings,
    judging: Judging,
) -> Vec<Judged> {
    let group = case.group();
    let least = settings.start_pairs.least;
    let starts = budget_starts(case, finished).and_then(|(quiet, dropped)| {
        if let Some(reason) = gpu_load_refused(case, &quiet, settings) {
            return Err(reason);
        }
        if quiet.len() < least {
            return Err(format!(
                "{} quiet starts of the {least} needed, {dropped} dropped as disturbed",
                quiet.len()
            ));
        }
        Ok((quiet, dropped))
    });
    case.budgets()
        .into_iter()
        .map(|(kind, value)| {
            let name = format!("{group}: {}", kind.name);
            let entry = figures::entry_of(kind, &group, &name, judging);
            let mut values: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
            let mut judgement = None;
            let verdict = (|| -> Result<Verdict, String> {
                let (quiet, dropped) = starts.as_ref().map_err(Clone::clone)?;
                let entry = entry.clone()?;
                let taken: Vec<f64> = quiet[..least]
                    .iter()
                    .map(|run| value.of(run))
                    .collect::<Result<_, _>>()?;
                let longest = taken.iter().copied().fold(0.0, f64::max);
                values[1] = vec![longest];
                judgement = Interval::of(&taken).map(|interval| Judgement {
                    interval,
                    dropped: *dropped,
                    on: "the head's starts on an empty TMPDIR",
                });
                match entry.class {
                    Class::Budget(budget) if longest > budget => Ok(Verdict::Worse),
                    Class::Budget(_) => Ok(Verdict::NoWorse),
                    Class::Recorded => Ok(Verdict::Recorded),
                    Class::Parity => Err(format!(
                        "a figure of the head's starts alone needs a budget or no verdict, not \
                         parity: {}",
                        super::thresholds::PATH
                    )),
                }
            })()
            .unwrap_or_else(Verdict::Void);
            timed_figure(name, &group, kind, entry.ok(), values, verdict, judgement)
        })
        .collect()
}

/// A case's pairs.
struct Pairs<'a> {
    /// Its quiet pairs: base, then head.
    quiet: Vec<[&'a Finished; 2]>,
    /// Its pairs dropped as disturbed.
    dropped: usize,
}

/// The pairs of `case` among `finished`, or why its figures are void: a run failed, or its frames
/// did unequal work.
fn pairs_of<'a>(
    case: Case,
    finished: &'a [Finished],
    settings: &Settings,
) -> Result<Pairs<'a>, String> {
    let runs: Vec<&Finished> = finished
        .iter()
        .filter(|run| case.runs(&run.job) && run.job.repeat > 0)
        .collect();
    if let Some(failed) = runs.iter().find(|run| run.outcome.error.is_some()) {
        return Err(format!(
            "{} failed: {}",
            failed.job.label(),
            failed.outcome.error.as_deref().unwrap_or_default()
        ));
    }
    if let Some(reason) = gpu_load_refused(case, &runs, settings) {
        return Err(reason);
    }
    if let Case::Frames { scenario, .. } = case {
        let measured: Vec<Finished> = runs.iter().map(|run| (*run).clone()).collect();
        if let Some(reason) = figures::equal_work(&measured, scenario.name) {
            return Err(reason);
        }
    }
    let mut by_pair: BTreeMap<u32, [Option<&Finished>; 2]> = BTreeMap::new();
    for run in runs {
        let at = match run.job.side {
            Side::Head => 1,
            _ => 0,
        };
        by_pair.entry(run.job.repeat).or_default()[at] = Some(run);
    }
    let mut pairs = Pairs {
        quiet: Vec::new(),
        dropped: 0,
    };
    for pair in by_pair.into_values() {
        let [Some(base), Some(head)] = pair else {
            continue;
        };
        if base.outcome.disturbed.is_some() || head.outcome.disturbed.is_some() {
            pairs.dropped += 1;
        } else {
            pairs.quiet.push([base, head]);
        }
    }
    Ok(pairs)
}

/// Each figure of `case`, judged as far as `finished` goes, and how many more pairs it asks.
fn assess(
    case: Case,
    finished: &[Finished],
    settings: &Settings,
    judging: Judging,
) -> Vec<(Judged, usize)> {
    let group = case.group();
    let limits = case.limits(settings);
    let pairs = pairs_of(case, finished, settings);
    case.figures()
        .into_iter()
        .map(|(kind, value)| {
            let name = format!("{group}: {}", kind.name);
            let entry = figures::entry_of(kind, &group, &name, judging);
            let mut values: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
            let mut judgement = None;
            let mut more = 0;
            let verdict = (|| -> Result<Verdict, String> {
                let pairs = pairs.as_ref().map_err(Clone::clone)?;
                let entry = entry.clone()?;
                let mut logs = Vec::new();
                for [base, head] in &pairs.quiet {
                    let (b, h) = (value.of(base)?, value.of(head)?);
                    if !(b > 0.0 && h > 0.0) {
                        return Err(format!("a value not above 0: base {b}, head {h}"));
                    }
                    values[0].push(b);
                    values[1].push(h);
                    logs.push((h / b).ln());
                }
                match rule::decide(
                    entry.class,
                    entry.threshold.relative,
                    &logs,
                    pairs.dropped,
                    limits,
                ) {
                    Decision::More(n) => {
                        more = n;
                        Ok(Verdict::Inconclusive)
                    }
                    Decision::Judged(verdict, interval) => {
                        judgement = interval.map(|interval| Judgement {
                            interval,
                            dropped: pairs.dropped,
                            on: "ln(head / base)",
                        });
                        Ok(verdict)
                    }
                }
            })()
            .unwrap_or_else(Verdict::Void);
            let judged = timed_figure(name, &group, kind, entry.ok(), values, verdict, judgement);
            (judged, more)
        })
        .collect()
}

/// A timed figure, judged: its class and threshold from its entry, its medians and spread from
/// each side's values.
fn timed_figure(
    name: String,
    group: &str,
    kind: Kind,
    entry: Option<&Entry>,
    values: [Vec<f64>; 2],
    verdict: Verdict,
    judgement: Option<Judgement>,
) -> Judged {
    let medians = [rule::median(&values[0]), rule::median(&values[1])];
    // The wider side's, none if a side's cannot be told.
    let spreads: Option<Vec<f64>> = values
        .iter()
        .filter(|side| side.len() >= 2)
        .map(|side| rule::spread(side))
        .collect();
    let spread = spreads.and_then(|spreads| spreads.into_iter().reduce(f64::max));
    Judged {
        name,
        group: group.to_owned(),
        kind,
        class: entry.map_or(Class::Parity, |e| e.class),
        threshold: entry.map(|e| (e.threshold, e.basis.clone())),
        values,
        medians,
        spread,
        verdict,
        judgement,
    }
}

/// The soak's figure: the head's slope, against no growth beyond its threshold's floor, in KiB a
/// minute (or a budget, if the file gives it one); the base's beside it. Inconclusive when the
/// head's interval shows neither: a soak runs once.
pub fn judge_soak(series: &[Series; 2], judging: Judging) -> Judged {
    let group = "soak";
    let name = format!("{group}: {}", SOAK_GROWTH.name);
    let entry = figures::entry_of(SOAK_GROWTH, group, &name, judging);
    let slopes = series.clone().map(|side| match &side.error {
        Some(error) => Err(error.clone()),
        None => soak::slope(&side.readings)
            .ok_or_else(|| format!("{} readings: no slope", side.readings.len())),
    });
    let values = [
        slopes[0].as_ref().map(|s| vec![s.mean]).unwrap_or_default(),
        slopes[1].as_ref().map(|s| vec![s.mean]).unwrap_or_default(),
    ];
    let head: Result<&Interval, String> =
        slopes[1].as_ref().map_err(|error| format!("head: {error}"));
    let verdict = match (&entry, &head) {
        (Err(reason), _) | (_, Err(reason)) => Verdict::Void(reason.clone()),
        (Ok(entry), Ok(interval)) => {
            let allowed = match entry.class {
                Class::Recorded => None,
                Class::Budget(budget) => Some(budget),
                Class::Parity => Some(entry.threshold.absolute),
            };
            allowed.map_or(Verdict::Recorded, |allowed| {
                rule::shown(interval, allowed).unwrap_or(Verdict::Inconclusive)
            })
        }
    };
    let judgement = slopes[1].as_ref().ok().map(|interval| Judgement {
        interval: *interval,
        dropped: 0,
        on: "the head's slope",
    });
    timed_figure(
        name,
        group,
        SOAK_GROWTH,
        entry.ok(),
        values,
        verdict,
        judgement,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parity::rule::Threshold;
    use crate::parity::scenario::{Outcome, Work};
    use crate::parity::thresholds::Thresholds;

    fn thresholds(t: f64) -> Thresholds {
        Thresholds {
            entries: figures::kind_names()
                .into_iter()
                .map(|figure| Entry {
                    figure: figure.to_owned(),
                    group: "*".to_owned(),
                    // ADR-0100's budgets, as the tracked file gives them.
                    class: match figure {
                        "seat create's reply, longest" => Class::Budget(100.0),
                        "first frame, longest" => Class::Budget(150.0),
                        _ => Class::Parity,
                    },
                    threshold: Threshold {
                        relative: t,
                        absolute: if figure == SOAK_GROWTH.name { 8.0 } else { 0.0 },
                    },
                    basis: "the tests'".to_owned(),
                })
                .collect(),
        }
    }

    const SETTINGS: Settings = Settings {
        frame_pairs: PairLimits {
            least: 10,
            most: 30,
            batch: 5,
        },
        start_pairs: PairLimits {
            least: 20,
            most: 60,
            batch: 10,
        },
        seed: 7,
        gpu_load_unknown_ok: false,
    };

    #[test]
    fn a_run_is_disturbed_by_others_on_average_or_in_a_second_or_by_its_run_queue() {
        let readings = |pairs: &[(&str, f64)]| -> BTreeMap<String, f64> {
            pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
        };
        let quiet = readings(&[
            ("others_cpu_cores_mean", 0.8),
            ("others_cpu_cores_max", 1.6),
            ("others_gpu_percent_mean", 0.0),
            ("own_cpu_ns", 100_000_000.0),
            ("own_wait_ns", 2_000_000.0),
        ]);
        assert_eq!(disturbance(&quiet), None);
        // Two cores on average, the limit itself, is not above it.
        let mut at_limit = quiet.clone();
        at_limit.insert("others_cpu_cores_mean".to_owned(), 2.0);
        assert_eq!(disturbance(&at_limit), None);
        // One second of a build: quiet on average, disturbed in that second.
        let mut burst = quiet.clone();
        burst.insert("others_cpu_cores_max".to_owned(), 5.5);
        let reason = disturbance(&burst).expect("a second above the limit");
        assert!(reason.contains("in a second 5.50"), "{reason}");
        let mut game = quiet.clone();
        game.insert("others_gpu_percent_mean".to_owned(), 40.0);
        assert!(disturbance(&game).is_some());
        // The others' load below its limits, but Yuna's own threads kept waiting for a core: 12 %
        // of their time on one.
        let mut queued = quiet.clone();
        queued.insert("own_wait_ns".to_owned(), 12_000_000.0);
        let reason = disturbance(&queued).expect("the run queue's wait");
        assert!(
            reason.contains("waited on the run queue 12.0 %"),
            "{reason}"
        );
        // Unknown loads are not taken for disturbance: the GPU's share, unread, is left out.
        assert_eq!(disturbance(&BTreeMap::new()), None);
    }

    /// A made run of `job`, whose value of a frame's drawing time, and of its start's times, is
    /// `took_us`, and whose CPU time a frame is twice it; the GPU's load read, nil.
    fn made(job: &Job, took_us: f64, disturbed: bool) -> Finished {
        let frames = 240;
        let readings = BTreeMap::from([
            ("took_us".to_owned(), took_us),
            ("cpu_ns".to_owned(), 2.0 * took_us * 1e3 * frames as f64),
            ("reply_ms".to_owned(), took_us),
            ("first_frame_ms".to_owned(), took_us),
            (
                "others_cpu_cores_mean".to_owned(),
                if disturbed { 4.0 } else { 0.5 },
            ),
            ("others_gpu_percent_mean".to_owned(), 0.0),
        ]);
        Finished {
            job: job.clone(),
            outcome: Outcome {
                work: Work {
                    frames,
                    commits: frames,
                    ..Work::default()
                },
                readings,
                ..Outcome::default()
            },
        }
    }

    fn partial() -> Case {
        Case::Frames {
            scenario: scenario::named("partial").unwrap(),
            device: Device::Llvmpipe,
        }
    }

    /// Runs the pairs of `cases` with made runs: the head `slowdown` times the base, ±`noise` in
    /// turn, by its pair's number, every run of a pair numbered in `disturbed` disturbed.
    fn simulate(
        cases: &[Case],
        slowdown: f64,
        noise: f64,
        disturbed: impl Fn(u32) -> bool,
        t: f64,
    ) -> (Vec<Finished>, Vec<Judged>) {
        let thresholds = thresholds(t);
        let judging = Judging {
            head: &thresholds,
            base: None,
            moves: &[],
        };
        let finished = run_pairs(
            cases,
            &SETTINGS,
            judging,
            |job| {
                let wiggle = if job.repeat % 2 == 0 { noise } else { -noise };
                let value = match job.side {
                    Side::Head => 200.0 * slowdown * (1.0 + wiggle),
                    _ => 200.0,
                };
                made(job, value, disturbed(job.repeat))
            },
            |_| {},
        );
        let judged = judge(cases, &finished, &SETTINGS, judging);
        (finished, judged)
    }

    fn verdict_of<'a>(judged: &'a [Judged], name: &str) -> &'a Verdict {
        &judged
            .iter()
            .find(|figure| figure.name == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .verdict
    }

    #[test]
    fn the_same_build_twice_is_no_worse_on_its_least_pairs() {
        let (finished, judged) = simulate(&[partial()], 1.0, 0.01, |_| false, 0.05);
        // Two warm-ups, then 10 pairs.
        assert_eq!(finished.len(), 2 + 20);
        assert_eq!(finished[0].job.repeat, 0);
        assert_eq!(
            verdict_of(&judged, "partial: a frame's drawing time"),
            &Verdict::NoWorse
        );
        assert_eq!(
            verdict_of(&judged, "partial: CPU time a frame"),
            &Verdict::NoWorse
        );
        let figure = judged.iter().find(|f| f.group == "partial").unwrap();
        assert_eq!(figure.judgement.unwrap().interval.count, 10);
        // Each pair's two runs come in either order, drawn from the seed.
        let firsts: Vec<Side> = finished[2..]
            .chunks(2)
            .map(|pair| pair[0].job.side)
            .collect();
        assert!(firsts.contains(&Side::Base) && firsts.contains(&Side::Head));
    }

    #[test]
    fn a_head_twice_its_threshold_slower_is_worse() {
        let (_, judged) = simulate(&[partial()], 1.10, 0.01, |_| false, 0.05);
        assert_eq!(
            verdict_of(&judged, "partial: a frame's drawing time"),
            &Verdict::Worse
        );
    }

    #[test]
    fn a_head_at_its_threshold_runs_up_to_the_cap_then_is_inconclusive() {
        let (finished, judged) = simulate(&[partial()], 1.045, 0.05, |_| false, 0.05);
        assert_eq!(finished.len(), 2 + 2 * 30, "every pair up to the cap");
        assert_eq!(
            verdict_of(&judged, "partial: a frame's drawing time"),
            &Verdict::Inconclusive
        );
    }

    #[test]
    fn disturbed_pairs_are_dropped_and_replaced_or_the_figure_is_void() {
        // Pairs 3 and 4 disturbed: two more run, ten quiet ones judged.
        let (finished, judged) =
            simulate(&[partial()], 1.0, 0.01, |pair| pair == 3 || pair == 4, 0.05);
        assert_eq!(finished.len(), 2 + 2 * 12);
        assert_eq!(
            verdict_of(&judged, "partial: a frame's drawing time"),
            &Verdict::NoWorse
        );
        let figure = judged.iter().find(|f| f.group == "partial").unwrap();
        assert_eq!(figure.judgement.unwrap().dropped, 2);
        // Every pair disturbed: replaced up to the cap, then void.
        let (finished, judged) = simulate(&[partial()], 1.0, 0.01, |_| true, 0.05);
        assert_eq!(finished.len(), 2 + 2 * 30);
        let Verdict::Void(reason) = verdict_of(&judged, "partial: a frame's drawing time") else {
            panic!("{judged:?}");
        };
        assert!(reason.contains("0 quiet pairs of the 10"), "{reason}");
    }

    #[test]
    fn a_failed_run_voids_its_case_and_runs_no_more_of_it() {
        let thresholds = thresholds(0.05);
        let judging = Judging {
            head: &thresholds,
            base: None,
            moves: &[],
        };
        let case = partial();
        let finished = run_pairs(
            &[case],
            &SETTINGS,
            judging,
            |job| {
                let mut run = made(job, 200.0, false);
                if job.repeat == 2 && job.side == Side::Head {
                    run.outcome.error = Some("Yuna ended with 101".to_owned());
                }
                run
            },
            |_| {},
        );
        assert_eq!(finished.len(), 2 + 20);
        let judged = judge(&[case], &finished, &SETTINGS, judging);
        let Verdict::Void(reason) = verdict_of(&judged, "partial: a frame's drawing time") else {
            panic!("{judged:?}");
        };
        assert!(reason.contains("Yuna ended with 101"), "{reason}");
    }

    #[test]
    fn starts_are_judged_by_pairs_on_a_full_tmpdir_and_their_budgets_on_20_empty_ones() {
        let start = Case::Start {
            start: crate::parity::start::named("start-4k").unwrap(),
            device: Device::Llvmpipe,
        };
        let thresholds = thresholds(0.05);
        let judging = Judging {
            head: &thresholds,
            base: None,
            moves: &[],
        };
        let finished = run_pairs(
            &[start],
            &SETTINGS,
            judging,
            |job| {
                let empty = matches!(
                    job.what,
                    What::Start {
                        tmpdir: Tmpdir::Empty,
                        ..
                    }
                );
                // Pairs of 110 ms on the full folder. On the empty one, the head's starts of 90
                // ms, the 7th at 140 ms; the 3rd disturbed, at 400 ms, which no budget sees.
                let value = match (empty, job.repeat) {
                    (false, _) => 110.0,
                    (true, 3) => 400.0,
                    (true, 7) => 140.0,
                    (true, _) => 90.0,
                };
                made(job, value, empty && job.repeat == 3)
            },
            |_| {},
        );
        let judged = judge(&[start], &finished, &SETTINGS, judging);
        let empty: Vec<&Finished> = finished
            .iter()
            .filter(|run| start.budget_runs(&run.job))
            .collect();
        // Two warm-ups, 20 pairs, then 21 of the head's starts on an empty folder: the
        // disturbed one replaced.
        assert_eq!(finished.len(), 2 + 2 * 20 + 21);
        assert_eq!(empty.len(), 21);
        assert!(empty.iter().all(|run| run.job.side == Side::Head));
        assert_eq!(
            empty[0].job.label(),
            format!("head/start-4k/timed-empty-tmp/{}", empty[0].job.repeat)
        );
        assert_eq!(
            verdict_of(&judged, "start-4k: first frame"),
            &Verdict::NoWorse
        );
        assert_eq!(
            verdict_of(&judged, "start-4k: seat create's reply, longest"),
            &Verdict::Worse,
            "140 ms is beyond the reply's 100"
        );
        assert_eq!(
            verdict_of(&judged, "start-4k: first frame, longest"),
            &Verdict::NoWorse,
            "140 ms is within the first frame's 150"
        );
        let longest = judged
            .iter()
            .find(|f| f.name == "start-4k: first frame, longest")
            .unwrap();
        // The head's alone, the longest of exactly 20 quiet ones, the disturbed one left out.
        assert_eq!(longest.values, [vec![], vec![140.0]]);
        let judgement = longest.judgement.unwrap();
        assert_eq!((judgement.interval.count, judgement.dropped), (20, 1));
    }

    #[test]
    fn too_few_quiet_budget_starts_void_the_budgets() {
        let start = Case::Start {
            start: crate::parity::start::named("start-1080p").unwrap(),
            device: Device::Llvmpipe,
        };
        let thresholds = thresholds(0.05);
        let judging = Judging {
            head: &thresholds,
            base: None,
            moves: &[],
        };
        let finished = run_pairs(
            &[start],
            &SETTINGS,
            judging,
            |job| {
                let empty = start.budget_runs(job);
                made(job, 80.0, empty)
            },
            |_| {},
        );
        // Every budget start disturbed: replaced until 20 were dropped, then void.
        assert_eq!(
            finished
                .iter()
                .filter(|run| start.budget_runs(&run.job))
                .count(),
            20
        );
        let judged = judge(&[start], &finished, &SETTINGS, judging);
        let Verdict::Void(reason) = verdict_of(&judged, "start-1080p: first frame, longest") else {
            panic!("{judged:?}");
        };
        assert!(reason.contains("0 quiet starts of the 20"), "{reason}");
        assert_eq!(
            verdict_of(&judged, "start-1080p: first frame"),
            &Verdict::NoWorse
        );
    }

    #[test]
    fn a_case_on_the_gpu_whose_gpu_load_is_unknown_is_void_unless_asked() {
        let thresholds = thresholds(0.05);
        let judging = Judging {
            head: &thresholds,
            base: None,
            moves: &[],
        };
        let on = |device| Case::Frames {
            scenario: scenario::named("partial").unwrap(),
            device,
        };
        let unread = |job: &Job| {
            let mut run = made(job, 200.0, false);
            run.outcome.readings.remove("others_gpu_percent_mean");
            run
        };
        let figure = "partial-gpu: a frame's drawing time";
        let finished = run_pairs(&[on(Device::Gpu)], &SETTINGS, judging, unread, |_| {});
        let judged = judge(&[on(Device::Gpu)], &finished, &SETTINGS, judging);
        let Verdict::Void(reason) = verdict_of(&judged, figure) else {
            panic!("{judged:?}");
        };
        assert!(reason.contains("--gpu-load-unknown-ok"), "{reason}");
        assert!(finished.iter().all(gpu_load_unknown));
        assert_eq!(
            gpu_load_unread(&[on(Device::Gpu)], &finished),
            // Its two warm-ups included.
            ["partial-gpu: 22 of 22 runs".to_owned()]
        );
        // Asked by hand: judged.
        let asked = Settings {
            gpu_load_unknown_ok: true,
            ..SETTINGS
        };
        let finished = run_pairs(&[on(Device::Gpu)], &asked, judging, unread, |_| {});
        let judged = judge(&[on(Device::Gpu)], &finished, &asked, judging);
        assert_eq!(verdict_of(&judged, figure), &Verdict::NoWorse);
        // On llvmpipe, the GPU's load judges nothing: the report's headline says it alone.
        let finished = run_pairs(&[on(Device::Llvmpipe)], &SETTINGS, judging, unread, |_| {});
        let judged = judge(&[on(Device::Llvmpipe)], &finished, &SETTINGS, judging);
        assert_eq!(
            verdict_of(&judged, "partial: a frame's drawing time"),
            &Verdict::NoWorse
        );
    }

    #[test]
    fn cases_follow_the_parts_and_the_devices() {
        let parts: Vec<String> = PARTS.iter().map(|p| (*p).to_owned()).collect();
        let llvmpipe = cases(&parts, false);
        assert_eq!(llvmpipe.len(), 5);
        assert_eq!(
            llvmpipe.iter().map(|c| c.group()).collect::<Vec<_>>(),
            ["partial", "scaled", "full-4k", "start-1080p", "start-4k"]
        );
        let both = cases(&parts, true);
        assert_eq!(both.len(), 10);
        assert_eq!(both[5].group(), "partial-gpu");
        let full = |device| Case::Frames {
            scenario: &FULL_4K,
            device,
        };
        // A full frame on the processor is recorded by the tracked file (ADR-0139); on the GPU,
        // parity; a first frame on the GPU has its own budget.
        let tracked = crate::parity::thresholds::parse(
            &std::fs::read_to_string(
                crate::scenarios::workspace().join(crate::parity::thresholds::PATH),
            )
            .unwrap(),
        )
        .unwrap();
        let class =
            |figure: &Kind, case: Case| tracked.lookup(figure.name, &case.group()).unwrap().class;
        assert_eq!(
            class(&DRAWING_TIME, full(Device::Llvmpipe)),
            Class::Recorded
        );
        assert_eq!(class(&FRAME_CPU, full(Device::Llvmpipe)), Class::Recorded);
        assert_eq!(class(&DRAWING_TIME, full(Device::Gpu)), Class::Parity);
        let start = |device| Case::Start {
            start: crate::parity::start::named("start-4k").unwrap(),
            device,
        };
        assert_eq!(
            class(&FIRST_FRAME_LONGEST, start(Device::Llvmpipe)),
            Class::Budget(150.0)
        );
        assert_eq!(
            class(&FIRST_FRAME_LONGEST, start(Device::Gpu)),
            Class::Budget(100.0)
        );
        assert_eq!(
            class(&REPLY_LONGEST, start(Device::Gpu)),
            Class::Budget(100.0)
        );
        assert_eq!(
            full(Device::Llvmpipe).job(Side::Head, 3).label(),
            "head/full-4k/timed-120/3"
        );
    }

    #[test]
    fn the_soak_is_judged_on_the_heads_slope_against_no_growth() {
        let thresholds = thresholds(0.05);
        let judging = Judging {
            head: &thresholds,
            base: None,
            moves: &[],
        };
        let series = |per_minute: f64| Series {
            readings: (0..21)
                .map(|i| {
                    let minutes = f64::from(i) * 0.5;
                    (minutes, 500_000.0 + per_minute * minutes)
                })
                .collect(),
            error: None,
        };
        let judged = judge_soak(&[series(0.0), series(0.0)], judging);
        assert_eq!(judged.verdict, Verdict::NoWorse);
        // 64 KiB a minute, beyond the tests' floor of 8: worse.
        let judged = judge_soak(&[series(0.0), series(64.0)], judging);
        assert_eq!(judged.verdict, Verdict::Worse);
        assert!((judged.medians[1].unwrap() - 64.0).abs() < 1e-9);
        // A base that grows does not excuse the head, nor does it fail it.
        let judged = judge_soak(&[series(64.0), series(0.0)], judging);
        assert_eq!(judged.verdict, Verdict::NoWorse);
        let mut ended = series(0.0);
        ended.error = Some("one ended".to_owned());
        assert!(matches!(
            judge_soak(&[series(0.0), ended], judging).verdict,
            Verdict::Void(_)
        ));
    }
}
