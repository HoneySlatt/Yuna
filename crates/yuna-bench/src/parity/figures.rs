//! The figures, from the runs, and their verdicts: what each figure is; its class and its
//! threshold come from `parity-thresholds.toml` (`thresholds.rs`).
//!
//! A figure *a frame* is a cost over one period of the output's refresh, one advance of the
//! manual clock: in `partial`, `full`, `scaled` and `many`, each period draws one frame; in
//! `motion`, the frames of the motion; in `idle` and `input`, none, the period's work being the
//! advance and the input. valgrind's figures a frame are differences: the run of `2n` periods less
//! the run of `n`, over `n`, so that Yuna's start, its warm-up and its stop cancel, but for what
//! counts differently from run to run: LLVM compiling llvmpipe's shaders at Yuna's start, some
//! tens of thousands of instructions, which each scenario's floor of the total absorbs. Cutting the
//! measured periods out of one run instead would need a mark inside Yuna: callgrind's
//! `--dump-before` or `--toggle-collect` name a function, and only Yuna's own functions (its
//! `rest` tool's) are entered at the periods' ends alone, names Stage 2 changes; DHAT cuts nothing.
//! A figure is compared only where both builds did the same work ([`equal_work`]).
//!
//! The GPU tier's figures ([`GPU_KINDS`]) are judged here as the CI tier's are ([`judge_gpu`]);
//! the timed tier's kinds ([`TIMED_KINDS`]) are named here, and judged over pairs in `timed.rs`.

use std::collections::{BTreeMap, BTreeSet};

use super::build::Built;
use super::rule::{self, Class, Interval, Measured, Threshold, Verdict};
use super::scenario::{Device, Instrument, Outcome, SCENARIOS, Scenario};
use super::start::{Start, Tmpdir};
use super::thresholds::{Entry, Thresholds};
use super::valgrind::Tool;

/// Which build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    /// The baseline, or the revision compared with.
    Base,
    /// The revision under test.
    Head,
    /// The baseline before a move: the head against it flags only what the move names.
    Old,
    /// The push before the head's: compared with it to say which push moved a worse figure.
    Before,
}

impl Side {
    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Side::Base => "base",
            Side::Head => "head",
            Side::Old => "old",
            Side::Before => "before",
        }
    }
}

/// The runs of `other` and of the head among `finished`, `other`'s as the base's, to judge the
/// head against `other` with [`judge_all`].
pub fn against(finished: &[Finished], other: Side) -> Vec<Finished> {
    finished
        .iter()
        .filter(|run| run.job.side == other || run.job.side == Side::Head)
        .map(|run| {
            let mut run = run.clone();
            if run.job.side == other {
                run.job.side = Side::Base;
            }
            run
        })
        .collect()
}

/// What a run measures.
#[derive(Debug, Clone, PartialEq)]
pub enum What {
    /// A scenario, under an instrument, for a number of measured periods.
    Scenario {
        /// The scenario.
        scenario: &'static Scenario,
        /// How.
        instrument: Instrument,
        /// Its measured periods.
        periods: u32,
    },
    /// A case of an operations program, repeated.
    Operation {
        /// The program: `operations` or `motion_operations`.
        program: String,
        /// The case.
        case: String,
        /// Its repeats.
        repeats: u64,
    },
    /// A timed start of `yuna mcp` (`start.rs`).
    Start {
        /// Its output's mode.
        start: &'static Start,
        /// What Yuna draws with.
        device: Device,
        /// Its temporary folder: full for the parity figures, empty for the budgets.
        tmpdir: Tmpdir,
    },
}

/// One run to make.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// On which build.
    pub side: Side,
    /// What.
    pub what: What,
    /// Which measurement of it, from 1, when each is measured several times (`--repeat`).
    pub repeat: u32,
}

impl Job {
    /// Its label, unique among a run's: `head/partial/dhat-80/1`.
    pub fn label(&self) -> String {
        let what = match &self.what {
            What::Scenario {
                scenario,
                instrument,
                periods,
            } => format!("{}/{}-{periods}", scenario.name, instrument.name()),
            What::Operation {
                program,
                case,
                repeats,
            } => format!("{program}/{case}-{repeats}"),
            What::Start {
                start,
                device,
                tmpdir,
            } => {
                let empty = if *tmpdir == Tmpdir::Empty {
                    "-empty-tmp"
                } else {
                    ""
                };
                format!(
                    "{}/{}{empty}",
                    start.name,
                    Instrument::Timed(*device).name()
                )
            }
        };
        format!("{}/{what}/{}", self.side.name(), self.repeat)
    }
}

/// A run made.
#[derive(Debug, Clone)]
pub struct Finished {
    /// What it was.
    pub job: Job,
    /// What it measured.
    pub outcome: Outcome,
}

/// How a figure's value comes from the runs.
#[derive(Debug, Clone, Copy, PartialEq)]
enum From {
    /// `tool`'s reading of the run of `2n` periods less that of `n`, over `n`; with `idle`, less
    /// the same of the `idle` scenario: what an advance costs on its own.
    Difference {
        tool: Tool,
        reading: &'static str,
        less_idle: bool,
    },
    /// A native run's reading, over its periods when `per_period`.
    Native {
        reading: &'static str,
        per_period: bool,
    },
    /// A GPU tier's run's DRM memory: its own, resident less shared, when `own`; else shared.
    Gpu { own: bool },
    /// A timed tier's figure, judged over pairs or on the soak's series (`timed.rs`).
    Timed,
}

/// A kind of figure each scenario has.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kind {
    /// Its name, after the scenario's: `partial: instructions a frame, yuna`.
    pub name: &'static str,
    /// Its unit.
    pub unit: &'static str,
    from: From,
}

/// Every scenario's figures. Their classes and thresholds are data, in `parity-thresholds.toml`
/// (`thresholds.rs`), each with the spread it came from.
pub const KINDS: [Kind; 11] = [
    Kind {
        name: "instructions a frame, yuna",
        unit: "instructions",
        from: From::Difference {
            tool: Tool::Callgrind,
            reading: "instructions_yuna",
            less_idle: false,
        },
    },
    Kind {
        name: "instructions a frame",
        unit: "instructions",
        from: From::Difference {
            tool: Tool::Callgrind,
            reading: "instructions",
            less_idle: false,
        },
    },
    Kind {
        name: "allocations a frame",
        unit: "blocks",
        from: From::Difference {
            tool: Tool::Dhat,
            reading: "blocks",
            less_idle: true,
        },
    },
    Kind {
        name: "allocations a frame, yuna",
        unit: "blocks",
        from: From::Difference {
            tool: Tool::Dhat,
            reading: "blocks_own",
            less_idle: true,
        },
    },
    Kind {
        name: "heap growth a frame",
        unit: "bytes",
        from: From::Difference {
            tool: Tool::Dhat,
            reading: "end_bytes",
            less_idle: false,
        },
    },
    Kind {
        name: "Pss_Anon",
        unit: "KiB",
        from: From::Native {
            reading: "pss_anon_kib",
            per_period: false,
        },
    },
    Kind {
        name: "Pss_File",
        unit: "KiB",
        from: From::Native {
            reading: "pss_file_kib",
            per_period: false,
        },
    },
    Kind {
        name: "wakes a frame",
        unit: "wakes",
        from: From::Native {
            reading: "wakes",
            per_period: true,
        },
    },
    Kind {
        name: "context switches a frame",
        unit: "switches",
        from: From::Native {
            reading: "switches",
            per_period: true,
        },
    },
    Kind {
        name: "wakes at rest",
        unit: "wakes",
        from: From::Native {
            reading: "rest_wakes",
            per_period: false,
        },
    },
    Kind {
        name: "context switches at rest",
        unit: "switches",
        from: From::Native {
            reading: "rest_switches",
            per_period: false,
        },
    },
];

/// The wakes a frame of each source, from `rest`: a source that woke Yuna in one build and not the
/// other is 0 in the other.
pub const WAKES_BY_SOURCE: Kind = Kind {
    name: "wakes a frame, by source",
    unit: "wakes",
    from: From::Native {
        reading: "wakes:",
        per_period: true,
    },
};

/// An operation's instructions, from the operations programs.
pub const OPERATION: Kind = Kind {
    name: "instructions an operation",
    unit: "instructions",
    from: From::Difference {
        tool: Tool::Callgrind,
        reading: "instructions",
        less_idle: false,
    },
};

/// An animated update's instructions, from the motion operations program.
pub const MOTION_OPERATION: Kind = Kind {
    name: "instructions an update",
    unit: "instructions",
    from: From::Difference {
        tool: Tool::Callgrind,
        reading: "instructions",
        less_idle: false,
    },
};

/// The GPU tier's figures, of each scenario run natively on the GPU, at rest after its measured
/// periods: the kernel's DRM memory of Yuna's clients, each client once; its own, resident less
/// shared, parity, and its shared part, recorded, as the thresholds file says.
pub const GPU_KINDS: [Kind; 2] = [
    Kind {
        name: "GPU memory",
        unit: "KiB",
        from: From::Gpu { own: true },
    },
    Kind {
        name: "GPU memory, shared",
        unit: "KiB",
        from: From::Gpu { own: false },
    },
];

/// A frame's drawing time, the median of a timed run's frames' `took_us`.
pub const DRAWING_TIME: Kind = Kind {
    name: "a frame's drawing time",
    unit: "µs",
    from: From::Timed,
};

/// Yuna's CPU time a frame, every thread of its process, llvmpipe's included.
pub const FRAME_CPU: Kind = Kind {
    name: "CPU time a frame",
    unit: "µs",
    from: From::Timed,
};

/// From `yuna mcp`'s spawn to its `seat create`'s reply.
pub const REPLY: Kind = Kind {
    name: "seat create's reply",
    unit: "ms",
    from: From::Timed,
};

/// From `yuna mcp`'s spawn to its seat's first frame.
pub const FIRST_FRAME: Kind = Kind {
    name: "first frame",
    unit: "ms",
    from: From::Timed,
};

/// The longest reply of the head's quiet starts: a budget, 100 ms (ADR-0100), in the thresholds
/// file.
pub const REPLY_LONGEST: Kind = Kind {
    name: "seat create's reply, longest",
    unit: "ms",
    from: From::Timed,
};

/// The longest first frame of the head's quiet starts: a budget, 150 ms on llvmpipe, 100 ms on the
/// GPU (ADR-0100), in the thresholds file.
pub const FIRST_FRAME_LONGEST: Kind = Kind {
    name: "first frame, longest",
    unit: "ms",
    from: From::Timed,
};

/// The soak's growth: the slope of the ten instances' `Pss_Anon`, against no growth.
pub const SOAK_GROWTH: Kind = Kind {
    name: "Pss_Anon growth",
    unit: "KiB a minute",
    from: From::Timed,
};

/// Every kind of the timed tier.
pub const TIMED_KINDS: [Kind; 7] = [
    DRAWING_TIME,
    FRAME_CPU,
    REPLY,
    FIRST_FRAME,
    REPLY_LONGEST,
    FIRST_FRAME_LONGEST,
    SOAK_GROWTH,
];

impl Kind {
    /// Whether it is the timed tier's.
    pub fn is_timed(&self) -> bool {
        self.from == From::Timed
    }
}

/// The interval a timed figure was judged on: on `ln(head / base)` over its quiet pairs, or on
/// the slope of the head's soak.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Judgement {
    /// The interval.
    pub interval: Interval,
    /// The pairs dropped as disturbed.
    pub dropped: usize,
    /// What it bounds: `ln(head / base)`, or the soak's slope in its figure's unit.
    pub on: &'static str,
}

/// A figure, judged.
#[derive(Debug, Clone, PartialEq)]
pub struct Judged {
    /// Its name: `partial: instructions a frame, yuna`.
    pub name: String,
    /// Its scenario, or its program.
    pub group: String,
    /// Its kind.
    pub kind: Kind,
    /// Its class, from the head's thresholds file; [`Class::Parity`] when the file has none for it.
    pub class: Class,
    /// Its threshold, and where it came from; none when the thresholds file has none for it.
    pub threshold: Option<(Threshold, String)>,
    /// Each side's values, one a measurement; void ones left out.
    pub values: [Vec<f64>; 2],
    /// Each side's median, if measured.
    pub medians: [Option<f64>; 2],
    /// The larger of the two sides' spreads over their measurements; none with one a side.
    pub spread: Option<f64>,
    /// Its verdict.
    pub verdict: Verdict,
    /// For a timed figure, the interval it was judged on.
    pub judgement: Option<Judgement>,
}

/// What judging takes beside the runs: the head's classes and thresholds, which judge, and the
/// base's, which the head may not loosen unless a new move of the baseline names the figure.
#[derive(Debug, Clone, Copy)]
pub struct Judging<'a> {
    /// The head's classes and thresholds.
    pub head: &'a Thresholds,
    /// The base's, when its tree has the file.
    pub base: Option<&'a Thresholds>,
    /// The figures each new move of the baseline names: each move the head's baseline file holds
    /// and the push before's lacks.
    pub moves: &'a [Vec<String>],
}

/// The name of every kind of figure, as the thresholds file names them.
pub fn kind_names() -> Vec<&'static str> {
    KINDS
        .iter()
        .chain([&WAKES_BY_SOURCE, &OPERATION, &MOTION_OPERATION])
        .chain(&GPU_KINDS)
        .chain(&TIMED_KINDS)
        .map(|kind| kind.name)
        .collect()
}

/// Every figure of the GPU tier's runs `finished` on the builds `built` (base then head), judged
/// as the CI tier's are: each side's median against the threshold.
pub fn judge_gpu(finished: &[Finished], built: &[&Built; 2], judging: Judging) -> Vec<Judged> {
    let scenarios: BTreeSet<&str> = finished
        .iter()
        .filter_map(|run| match &run.job.what {
            What::Scenario {
                scenario,
                instrument: Instrument::Gpu,
                ..
            } => Some(scenario.name),
            _ => None,
        })
        .collect();
    let mut judged = Vec::new();
    for scenario in SCENARIOS.iter().filter(|s| scenarios.contains(s.name)) {
        let unequal = equal_work(finished, scenario.name);
        for kind in GPU_KINDS {
            let measure = |side: Side| -> Result<Vec<f64>, String> {
                if let Some(error) = &built[side as usize].info.error {
                    return Err(format!("its build failed: {error}"));
                }
                if let Some(reason) = &unequal {
                    return Err(reason.clone());
                }
                values(finished, side, scenario, kind.from, None)
            };
            judged.push(judge_one(
                format!("{}: {}", scenario.name, kind.name),
                (scenario.name, scenario.name),
                kind,
                [measure(Side::Base), measure(Side::Head)],
                judging,
            ));
        }
    }
    judged
}

/// Every figure of the runs `finished` on the builds `built` (base then head), judged.
pub fn judge_all(finished: &[Finished], built: &[&Built; 2], judging: Judging) -> Vec<Judged> {
    let mut judged = Vec::new();
    let scenarios: BTreeSet<&str> = finished
        .iter()
        .filter_map(|run| match &run.job.what {
            What::Scenario { scenario, .. } => Some(scenario.name),
            What::Operation { .. } | What::Start { .. } => None,
        })
        .collect();
    for scenario in SCENARIOS.iter().filter(|s| scenarios.contains(s.name)) {
        let unequal = equal_work(finished, scenario.name);
        let idle_unequal = equal_work(finished, "idle");
        let mut kinds: Vec<(String, Kind, Option<String>)> = KINDS
            .iter()
            .map(|kind| (kind.name.to_owned(), *kind, None))
            .collect();
        for source in wake_sources(finished, scenario.name) {
            kinds.push((
                format!("wakes a frame, {source}"),
                WAKES_BY_SOURCE,
                Some(format!("wakes:{source}")),
            ));
        }
        for (name, kind, source) in kinds {
            let from = kind.from;
            let measure = |side: Side| -> Result<Vec<f64>, String> {
                if let Some(error) = &built[side as usize].info.error {
                    return Err(format!("its build failed: {error}"));
                }
                if let Some(reason) = &unequal {
                    return Err(reason.clone());
                }
                let less_idle = matches!(
                    from,
                    From::Difference {
                        less_idle: true,
                        ..
                    }
                ) && scenario.name != "idle";
                if less_idle && let Some(reason) = &idle_unequal {
                    return Err(format!("idle, which it subtracts: {reason}"));
                }
                values(finished, side, scenario, from, source.as_deref())
            };
            judged.push(judge_one(
                format!("{}: {name}", scenario.name),
                (scenario.name, scenario.name),
                kind,
                [measure(Side::Base), measure(Side::Head)],
                judging,
            ));
        }
    }
    judged.extend(judge_operations(finished, built, judging));
    judged
}

/// Judges one figure named `name` from each side's values or why it has none. `groups` are the
/// group the report gives it and the one its threshold is looked up by: a scenario for both, or
/// a program and its case.
fn judge_one(
    name: String,
    (group, threshold_group): (&str, &str),
    kind: Kind,
    sides: [Result<Vec<f64>, String>; 2],
    judging: Judging,
) -> Judged {
    let medians = [
        sides[0].as_ref().ok().and_then(|v| rule::median(v)),
        sides[1].as_ref().ok().and_then(|v| rule::median(v)),
    ];
    let measured = |side: &Result<Vec<f64>, String>, median: Option<f64>| match (side, median) {
        (Ok(_), Some(value)) => Measured::Value(value),
        (Ok(_), None) => Measured::Void("no measurement".to_owned()),
        (Err(reason), _) => Measured::Void(reason.clone()),
    };
    let entry = judging.head.lookup(kind.name, threshold_group);
    let verdict = match entry_of(kind, threshold_group, &name, judging) {
        Err(reason) => Verdict::Void(reason),
        Ok(entry) => rule::judge(
            entry.class,
            entry.threshold,
            &measured(&sides[0], medians[0]),
            &measured(&sides[1], medians[1]),
        ),
    };
    let values = [
        sides[0].clone().unwrap_or_default(),
        sides[1].clone().unwrap_or_default(),
    ];
    // Over a side's measurements, so none with one measurement a side.
    let measured: Vec<&Vec<f64>> = values.iter().filter(|v| v.len() >= 2).collect();
    let spread = if measured.is_empty() {
        None
    } else {
        measured
            .iter()
            .map(|v| rule::spread(v))
            .try_fold(0.0_f64, |high, spread| spread.map(|s| high.max(s)))
    };
    Judged {
        name,
        group: group.to_owned(),
        kind,
        class: entry.map_or(Class::Parity, |e| e.class),
        // Shown even when it made the figure void.
        threshold: entry.map(|e| (e.threshold, e.basis.clone())),
        values,
        medians,
        spread,
        verdict,
        judgement: None,
    }
}

/// The head's entry for the figure `name`, of `kind`, in `group`: its class, its threshold and
/// their basis. Why the figure is void instead: no entry, which every figure needs for its class,
/// or one the head made looser than the base's without a new move of the baseline naming it.
pub fn entry_of<'a>(
    kind: Kind,
    group: &str,
    name: &str,
    judging: Judging<'a>,
) -> Result<&'a Entry, String> {
    let entry = judging.head.lookup(kind.name, group);
    let loosened = judging.base.and_then(|base| {
        let moves: Vec<&[String]> = judging.moves.iter().map(Vec::as_slice).collect();
        super::thresholds::loosened(base, judging.head, kind.name, group, name, &moves)
    });
    match (entry, loosened) {
        (None, _) => Err(format!(
            "no threshold for {:?} in {}, which gives each figure its class too",
            kind.name,
            super::thresholds::PATH
        )),
        (_, Some(reason)) => Err(reason),
        (Some(entry), None) => Ok(entry),
    }
}

/// The figures of the operations programs.
fn judge_operations(finished: &[Finished], built: &[&Built; 2], judging: Judging) -> Vec<Judged> {
    let mut cases: BTreeMap<(String, String), ()> = BTreeMap::new();
    for run in finished {
        if let What::Operation { program, case, .. } = &run.job.what {
            cases.insert((program.clone(), case.clone()), ());
        }
    }
    // A program in neither tree, or whose cases a side could not list, is one void figure: its
    // cases are not known.
    for example in super::build::EXAMPLES {
        let anywhere = built
            .iter()
            .any(|side| side.programs.contains_key(example.name));
        let unlisted = built
            .iter()
            .any(|side| matches!(side.cases.get(example.name), Some(Err(_))));
        if !anywhere || unlisted {
            cases.insert((example.name.to_owned(), "*".to_owned()), ());
        }
    }
    let mut judged = Vec::new();
    for (program, case) in cases.into_keys() {
        let measure = |side: Side| -> Result<Vec<f64>, String> {
            let build = built[side as usize];
            if let Some(error) = &build.info.error {
                return Err(format!("its build failed: {error}"));
            }
            if let Some(Err(error)) = build.cases.get(&program) {
                return Err(format!("its cases could not be listed: {error}"));
            }
            if !build.programs.contains_key(&program) {
                let source = super::build::EXAMPLES
                    .iter()
                    .find(|e| e.name == program)
                    .map_or("", |e| e.source);
                return Err(format!("its tree has no {source}"));
            }
            if let Some(reason) = digests_differ(finished, &program, &case) {
                return Err(reason);
            }
            operation_values(finished, side, &program, &case)
        };
        let group = program.clone();
        let kind = if program == "motion_operations" {
            MOTION_OPERATION
        } else {
            OPERATION
        };
        judged.push(judge_one(
            format!("{program}: {case}"),
            (&group, &case),
            kind,
            [measure(Side::Base), measure(Side::Head)],
            judging,
        ));
    }
    judged
}

/// One value a measurement of `side` gives for `case` of `program`.
fn operation_values(
    finished: &[Finished],
    side: Side,
    program: &str,
    case: &str,
) -> Result<Vec<f64>, String> {
    let runs = |side: Side| {
        finished.iter().filter(move |run| {
            run.job.side == side
                && matches!(&run.job.what, What::Operation { program: p, case: c, .. }
                    if p == program && c == case)
        })
    };
    let repeats = |run: &Finished| match run.job.what {
        What::Operation { repeats, .. } => repeats,
        What::Scenario { .. } | What::Start { .. } => 0,
    };
    let shorter = runs(side).map(repeats).min().ok_or("not run")?;
    let mut values = Vec::new();
    for measurement in measurements(runs(side).map(|run| run.job.repeat)) {
        let reading = |count: u64| -> Result<f64, String> {
            let run = runs(side)
                .find(|run| repeats(run) == count && run.job.repeat == measurement)
                .ok_or_else(|| format!("no run of {count} repeats"))?;
            reading_of(&run.outcome, "instructions")
        };
        values.push((reading(2 * shorter)? - reading(shorter)?) / shorter as f64);
    }
    Ok(values)
}

/// Why `case`'s runs did not all do the same work: their digests differ.
fn digests_differ(finished: &[Finished], program: &str, case: &str) -> Option<String> {
    let mut by_repeats: BTreeMap<u64, BTreeSet<(Side, String)>> = BTreeMap::new();
    for run in finished {
        if let What::Operation {
            program: p,
            case: c,
            repeats,
        } = &run.job.what
            && p == program
            && c == case
            && let Some(digest) = &run.outcome.digest
        {
            by_repeats
                .entry(*repeats)
                .or_default()
                .insert((run.job.side, digest.clone()));
        }
    }
    by_repeats.into_iter().find_map(|(repeats, digests)| {
        let distinct: BTreeSet<&String> = digests.iter().map(|(_, d)| d).collect();
        (distinct.len() > 1).then(|| {
            let said: Vec<String> = digests
                .iter()
                .map(|(side, digest)| format!("{} {digest}", side.name()))
                .collect();
            format!(
                "unequal work: the digests of {repeats} repeats differ: {}",
                said.join(", ")
            )
        })
    })
}

/// The measurements' numbers among `repeats`, in order.
fn measurements(repeats: impl Iterator<Item = u32>) -> BTreeSet<u32> {
    repeats.collect()
}

/// The sources that woke Yuna in a native run of `scenario`, on either side.
fn wake_sources(finished: &[Finished], scenario: &str) -> BTreeSet<String> {
    finished
        .iter()
        .filter(|run| {
            matches!(&run.job.what, What::Scenario { scenario: s, instrument: Instrument::Native, .. }
                if s.name == scenario)
        })
        .flat_map(|run| run.outcome.readings.keys())
        .filter_map(|key| key.strip_prefix("wakes:"))
        .map(str::to_owned)
        .collect()
}

/// A reading of a run's `outcome`, or why there is none.
fn reading_of(outcome: &Outcome, reading: &str) -> Result<f64, String> {
    if let Some(error) = &outcome.error {
        return Err(error.clone());
    }
    outcome
        .readings
        .get(reading)
        .copied()
        .ok_or_else(|| format!("the run read no {reading}"))
}

/// The runs of `scenario` on `side` under `instrument`.
fn scenario_runs<'a>(
    finished: &'a [Finished],
    side: Side,
    scenario: &'a str,
    instrument: Instrument,
) -> impl Iterator<Item = &'a Finished> + 'a {
    finished.iter().filter(move |run| {
        run.job.side == side
            && matches!(&run.job.what, What::Scenario { scenario: s, instrument: i, .. }
                if s.name == scenario && *i == instrument)
    })
}

fn periods_of(run: &Finished) -> u32 {
    match run.job.what {
        What::Scenario { periods, .. } => periods,
        What::Operation { .. } | What::Start { .. } => 0,
    }
}

/// One value a measurement of `side` gives for a figure of `scenario` taken `from` its runs;
/// `source` names the wake source of a figure of [`WAKES_BY_SOURCE`].
fn values(
    finished: &[Finished],
    side: Side,
    scenario: &Scenario,
    from: From,
    source: Option<&str>,
) -> Result<Vec<f64>, String> {
    match from {
        From::Timed => Err("a timed figure, judged over pairs (`timed.rs`)".to_owned()),
        From::Gpu { own } => {
            let runs: Vec<&Finished> =
                scenario_runs(finished, side, scenario.name, Instrument::Gpu).collect();
            if runs.is_empty() {
                return Err("no run on the GPU".to_owned());
            }
            runs.iter()
                .map(|run| {
                    let shared = reading_of(&run.outcome, "gpu_shared_kib")?;
                    if own {
                        Ok(reading_of(&run.outcome, "gpu_resident_kib")? - shared)
                    } else {
                        Ok(shared)
                    }
                })
                .collect()
        }
        From::Native {
            reading,
            per_period,
        } => {
            let runs: Vec<&Finished> =
                scenario_runs(finished, side, scenario.name, Instrument::Native).collect();
            if runs.is_empty() {
                return Err("no native run".to_owned());
            }
            runs.iter()
                .map(|run| {
                    let value = match source {
                        // A source that never woke Yuna in this run is not listed by `rest`.
                        Some(source) => {
                            reading_of(&run.outcome, "wakes")?;
                            run.outcome.readings.get(source).copied().unwrap_or(0.0)
                        }
                        None => reading_of(&run.outcome, reading)?,
                    };
                    Ok(if per_period {
                        value / f64::from(periods_of(run))
                    } else {
                        value
                    })
                })
                .collect()
        }
        From::Difference {
            tool,
            reading,
            less_idle,
        } => {
            let per_frame = |name: &str| -> Result<Vec<f64>, String> {
                let instrument = Instrument::Valgrind(tool);
                let runs: Vec<&Finished> =
                    scenario_runs(finished, side, name, instrument).collect();
                let shorter = runs
                    .iter()
                    .map(|run| periods_of(run))
                    .min()
                    .ok_or_else(|| format!("no {} run of {name}", tool.name()))?;
                measurements(runs.iter().map(|run| run.job.repeat))
                    .into_iter()
                    .map(|measurement| {
                        let at = |periods: u32| -> Result<f64, String> {
                            let run = runs
                                .iter()
                                .find(|r| periods_of(r) == periods && r.job.repeat == measurement)
                                .ok_or_else(|| {
                                    format!("no {} run of {name} of {periods} periods", tool.name())
                                })?;
                            reading_of(&run.outcome, reading)
                        };
                        Ok((at(2 * shorter)? - at(shorter)?) / f64::from(shorter))
                    })
                    .collect()
            };
            let mut values = per_frame(scenario.name)?;
            if less_idle && scenario.name != "idle" {
                let idle = per_frame("idle")?;
                if idle.len() != values.len() {
                    return Err("idle was not measured as often".to_owned());
                }
                for (value, advance) in values.iter_mut().zip(idle) {
                    *value -= advance;
                }
            }
            Ok(values)
        }
    }
}

/// Why the runs of `scenario` did not all do the same work, if they did not: among the runs of
/// each instrument and length, on both sides, the frames drawn, their causes and the client's
/// commits must be the same, or no cost of the scenario is compared.
pub fn equal_work(finished: &[Finished], scenario: &str) -> Option<String> {
    let mut groups: BTreeMap<(Instrument, u32), Vec<&Finished>> = BTreeMap::new();
    for run in finished {
        if let What::Scenario {
            scenario: s,
            instrument,
            periods,
        } = &run.job.what
            && s.name == scenario
            && run.outcome.error.is_none()
        {
            groups.entry((*instrument, *periods)).or_default().push(run);
        }
    }
    for ((instrument, periods), runs) in groups {
        // What Yuna did, by its own counts, and what the client received, by the client's: a
        // build that sends a client less (no pointer motion, no frame callback) does less work,
        // and is not cheaper for it.
        let shared = |run: &Finished| {
            let work = &run.outcome.work;
            let causes: BTreeMap<&String, &u64> = work
                .causes
                .iter()
                .filter(|(_, frames)| **frames > 0)
                .collect();
            format!(
                "{} frames {causes:?}, {} commits counted; the client: {} commits, {} frame \
                 callbacks, {} pointer events, {} configures, {} buffers",
                work.frames,
                work.commits,
                work.client.commits,
                work.client.callbacks,
                work.client.pointer,
                work.client.configures,
                work.client.buffers
            )
        };
        let first = shared(runs[0]);
        if let Some(other) = runs.iter().find(|run| shared(run) != first) {
            return Some(format!(
                "unequal work, {} of {periods} periods: {}: {first}; {}: {}",
                instrument.name(),
                runs[0].job.label(),
                other.job.label(),
                shared(other)
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parity::build::SideInfo;
    use crate::parity::lockstep::Counts;
    use crate::parity::scenario::{Work, named};

    /// The tests' thresholds: every kind's `*`, 0.5 % and at least 0.5, operations' at least 100.
    fn thresholds() -> Thresholds {
        Thresholds {
            entries: kind_names()
                .into_iter()
                .map(|figure| super::super::thresholds::Entry {
                    figure: figure.to_owned(),
                    group: "*".to_owned(),
                    class: if figure == "Pss_File" {
                        Class::Recorded
                    } else {
                        Class::Parity
                    },
                    threshold: Threshold {
                        relative: 0.005,
                        absolute: if figure.starts_with("instructions an") {
                            100.0
                        } else {
                            0.5
                        },
                    },
                    basis: "the tests'".to_owned(),
                })
                .collect(),
        }
    }

    fn judge(finished: &[Finished], built: &[&Built; 2]) -> Vec<Judged> {
        let thresholds = thresholds();
        judge_all(
            finished,
            built,
            Judging {
                head: &thresholds,
                base: None,
                moves: &[],
            },
        )
    }

    #[test]
    fn a_figure_without_a_threshold_or_whose_threshold_loosened_is_void() {
        let finished = vec![
            dhat(Side::Base, "partial", 40, 5000.0),
            dhat(Side::Base, "partial", 80, 6000.0),
            dhat(Side::Head, "partial", 40, 5000.0),
            dhat(Side::Head, "partial", 80, 6000.0),
        ];
        let (base, head) = (built(), built());
        let mut none = thresholds();
        none.entries.retain(|e| e.figure != "heap growth a frame");
        let judged = judge_all(
            &finished,
            &[&base, &head],
            Judging {
                head: &none,
                base: None,
                moves: &[],
            },
        );
        let verdict = &figure(&judged, "partial: heap growth a frame").verdict;
        assert!(
            verdict.reason().unwrap().contains("no threshold"),
            "{verdict:?}"
        );
        // The head loosens the threshold of partial's heap growth: void, unless a new move names it.
        let was = thresholds();
        let mut looser = thresholds();
        looser.entries.push(super::super::thresholds::Entry {
            figure: "heap growth a frame".to_owned(),
            group: "partial".to_owned(),
            class: Class::Parity,
            threshold: Threshold {
                relative: 0.0,
                absolute: 64.0,
            },
            basis: "a leak allowed".to_owned(),
        });
        let judge_with = |moves: &[Vec<String>]| {
            judge_all(
                &finished,
                &[&base, &head],
                Judging {
                    head: &looser,
                    base: Some(&was),
                    moves,
                },
            )
        };
        let judged = judge_with(&[]);
        let verdict = &figure(&judged, "partial: heap growth a frame").verdict;
        assert!(
            verdict.reason().unwrap().contains("loosened"),
            "{verdict:?}"
        );
        // Another scenario's, which kept its threshold, is judged as ever.
        let judged = judge_with(&[vec!["partial: heap growth a frame".to_owned()]]);
        assert_eq!(
            figure(&judged, "partial: heap growth a frame").verdict,
            Verdict::NoWorse
        );
    }

    fn built() -> Built {
        Built {
            info: SideInfo::default(),
            programs: BTreeMap::from([("yuna".to_owned(), "/w/yuna".into())]),
            cases: BTreeMap::new(),
            thresholds: None,
            run_environment: Vec::new(),
        }
    }

    fn run(
        side: Side,
        scenario: &str,
        instrument: Instrument,
        periods: u32,
        readings: &[(&str, f64)],
    ) -> Finished {
        Finished {
            job: Job {
                side,
                what: What::Scenario {
                    scenario: named(scenario).unwrap(),
                    instrument,
                    periods,
                },
                repeat: 1,
            },
            outcome: Outcome {
                work: Work {
                    frames: u64::from(periods),
                    commits: u64::from(periods),
                    ..Work::default()
                },
                readings: readings
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), *v))
                    .collect(),
                ..Outcome::default()
            },
        }
    }

    fn figure<'a>(judged: &'a [Judged], name: &str) -> &'a Judged {
        judged
            .iter()
            .find(|figure| figure.name == name)
            .unwrap_or_else(|| panic!("no figure {name}"))
    }

    fn dhat(side: Side, scenario: &str, periods: u32, blocks: f64) -> Finished {
        run(
            side,
            scenario,
            Instrument::Valgrind(Tool::Dhat),
            periods,
            &[
                ("blocks", blocks),
                ("blocks_own", blocks),
                ("end_bytes", 0.0),
            ],
        )
    }

    #[test]
    fn a_frames_allocations_leave_out_the_advances_own() {
        // idle: 10 blocks an advance; partial: 25 a frame, 15 of them the frame's own.
        let mut finished = Vec::new();
        for side in [Side::Base, Side::Head] {
            finished.push(dhat(side, "idle", 40, 1000.0));
            finished.push(dhat(side, "idle", 80, 1400.0));
            finished.push(dhat(side, "partial", 40, 5000.0));
        }
        finished.push(dhat(Side::Base, "partial", 80, 6000.0));
        // The head allocates one more block each frame: 26 a frame.
        finished.push(dhat(Side::Head, "partial", 80, 6040.0));
        let (base, head) = (built(), built());
        let judged = judge(&finished, &[&base, &head]);
        let idle = figure(&judged, "idle: allocations a frame");
        assert_eq!(idle.medians, [Some(10.0), Some(10.0)]);
        assert_eq!(idle.verdict, Verdict::NoWorse);
        let partial = figure(&judged, "partial: allocations a frame");
        assert_eq!(partial.medians, [Some(15.0), Some(16.0)]);
        assert_eq!(partial.verdict, Verdict::Worse);
        // No callgrind run: those figures are void, never passed.
        assert!(matches!(
            figure(&judged, "partial: instructions a frame").verdict,
            Verdict::Void(_)
        ));
    }

    #[test]
    fn unequal_work_voids_every_figure_of_its_scenario() {
        let mut finished = vec![
            dhat(Side::Base, "partial", 40, 5000.0),
            dhat(Side::Base, "partial", 80, 6000.0),
            dhat(Side::Head, "partial", 40, 5000.0),
            dhat(Side::Head, "partial", 80, 6000.0),
        ];
        assert_eq!(equal_work(&finished, "partial"), None);
        // The head drew a frame less in its longer run.
        finished[3].outcome.work.frames -= 1;
        let reason = equal_work(&finished, "partial").unwrap();
        assert!(
            reason.contains("unequal work, dhat of 80 periods"),
            "{reason}"
        );
        let (base, head) = (built(), built());
        let judged = judge(&finished, &[&base, &head]);
        let verdict = &figure(&judged, "partial: heap growth a frame").verdict;
        assert!(
            verdict.reason().unwrap().contains("unequal work"),
            "{verdict:?}"
        );
        // What the client received counts too, each of its counts: a head that sends no pointer
        // motion does less, and is not cheaper for it.
        finished[3].outcome.work.frames += 1;
        assert_eq!(equal_work(&finished, "partial"), None);
        let differ: [fn(&mut Counts); 5] = [
            |c| c.commits += 1,
            |c| c.callbacks += 1,
            |c| c.pointer += 1,
            |c| c.configures += 1,
            |c| c.buffers += 1,
        ];
        for change in differ {
            let mut one = finished.clone();
            change(&mut one[3].outcome.work.client);
            assert!(
                equal_work(&one, "partial").is_some(),
                "{:?}",
                one[3].outcome.work
            );
        }
        // A run that failed is void on its own, and does not break the others' equal work.
        finished[2].outcome.error = Some("Yuna ended with 101".to_owned());
        finished[2].outcome.work = Work::default();
        assert_eq!(equal_work(&finished, "partial"), None);
    }

    #[test]
    fn wakes_by_source_and_at_rest() {
        let native = |side, wakes: f64, clients: f64, rest: f64| {
            run(
                side,
                "partial",
                Instrument::Native,
                40,
                &[
                    ("wakes", wakes),
                    ("wakes:mcp_input", 40.0),
                    ("wakes:clients", clients),
                    ("rest_wakes", rest),
                ],
            )
        };
        let mut head = native(Side::Head, 80.0, 40.0, 2.0);
        head.outcome.readings.remove("wakes:clients");
        head.outcome
            .readings
            .insert("wakes:clock_refresh".to_owned(), 40.0);
        let finished = vec![native(Side::Base, 80.0, 40.0, 0.0), head];
        let (base, head) = (built(), built());
        let judged = judge(&finished, &[&base, &head]);
        assert_eq!(
            figure(&judged, "partial: wakes a frame").verdict,
            Verdict::NoWorse
        );
        let clients = figure(&judged, "partial: wakes a frame, clients");
        assert_eq!(clients.medians, [Some(1.0), Some(0.0)]);
        let refresh = figure(&judged, "partial: wakes a frame, clock_refresh");
        assert_eq!(refresh.medians, [Some(0.0), Some(1.0)]);
        assert_eq!(refresh.verdict, Verdict::Worse);
        assert_eq!(
            figure(&judged, "partial: wakes at rest").verdict,
            Verdict::Worse
        );
    }

    fn operation(side: Side, repeats: u64, instructions: f64, digest: &str) -> Finished {
        Finished {
            job: Job {
                side,
                what: What::Operation {
                    program: "operations".to_owned(),
                    case: "focus-left-right@100-scrolling".to_owned(),
                    repeats,
                },
                repeat: 1,
            },
            outcome: Outcome {
                readings: BTreeMap::from([("instructions".to_owned(), instructions)]),
                digest: Some(digest.to_owned()),
                ..Outcome::default()
            },
        }
    }

    #[test]
    fn an_operations_figure_needs_the_program_on_both_sides_and_the_same_digests() {
        let mut with = built();
        with.programs
            .insert("operations".to_owned(), "/w/operations".into());
        let finished = vec![
            operation(Side::Base, 20, 100_000.0, "aa"),
            operation(Side::Base, 40, 140_000.0, "bb"),
            operation(Side::Head, 20, 100_000.0, "aa"),
            operation(Side::Head, 40, 141_000.0, "bb"),
        ];
        let judged = judge(&finished, &[&with, &with]);
        let focus = figure(&judged, "operations: focus-left-right@100-scrolling");
        assert_eq!(focus.medians, [Some(2000.0), Some(2050.0)]);
        // 50 more is within the floor of 100.
        assert_eq!(focus.verdict, Verdict::NoWorse);
        // Different work: void.
        let mut unequal = finished.clone();
        unequal[3].outcome.digest = Some("cc".to_owned());
        let judged = judge(&unequal, &[&with, &with]);
        let verdict = &figure(&judged, "operations: focus-left-right@100-scrolling").verdict;
        assert!(verdict.reason().unwrap().contains("digests"), "{verdict:?}");
        // The base lacks the program: void; neither has it: void too, never a pass.
        let without = built();
        let judged = judge(&finished[2..], &[&without, &with]);
        let verdict = &figure(&judged, "operations: focus-left-right@100-scrolling").verdict;
        assert!(
            verdict.reason().unwrap().contains("its tree has no"),
            "{verdict:?}"
        );
        let judged = judge(&[], &[&without, &without]);
        let neither = figure(&judged, "operations: *");
        assert!(matches!(neither.verdict, Verdict::Void(_)), "{neither:?}");
        assert_eq!(rule::exit_status(judged.iter().map(|j| &j.verdict)), 2);
        // A side whose program could not list its cases: void, never dropped.
        let mut unlisted = with.clone();
        unlisted
            .cases
            .insert("operations".to_owned(), Err("exit 101".to_owned()));
        let judged = judge(&finished, &[&unlisted, &with]);
        let verdict = &figure(&judged, "operations: *").verdict;
        assert!(
            verdict.reason().unwrap().contains("could not be listed"),
            "{verdict:?}"
        );
    }
}
