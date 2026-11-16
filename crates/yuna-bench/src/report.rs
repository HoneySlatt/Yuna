//! The summary of one or more run files: a Markdown table a scenario, in the form of
//! `docs/measurements.md`, each cell the lowest and highest of a case's runs, `≥` before a lower
//! bound; then the compositors' memory at rest, and every rival of ADR-0075.
//!
//! A ranking compares the runs of one day and one machine state alone
//! ([ADR-0072](../../../docs/adr/0072-budgets-set-against-the-peers-the-same-day.md)): the UTC
//! day their run file started, one machine whole (its processor and memory, its GPU, VBIOS and
//! firmware, its kernel and Mesa), and the GPU's performance level their windows read. The first
//! file named sets them. Runs of another day, machine or level are shown on rows of their own,
//! marked `‡`, unranked, with why.
//!
//! A figure is ranked only where the difference is larger than the runs' spread: two compositors
//! whose ranges of runs overlap share a rank, and nothing is ranked with fewer than two runs of
//! each. Only *equal work* runs that other programs did not disturb are ranked; the rest are
//! shown, unranked, and a compositor left out so is named. A lower bound is never ranked.
//!
//! In `game` and `pacing`, whose client draws as fast as the compositor takes its frames, a cost
//! a second scales with the backend's pace (`recipes::Pace`): it is ranked only between
//! compositors on the same pace. In `rest`, `motion` and `film`, where equal work draws only when
//! something changes, no pace sets the costs: every compositor ranks together. A cost a frame
//! does not scale with the pace: it ranks across backends, between compositors whose frames were
//! counted the same exact way, by mpv in `game` or by one compositor's own count. Frames
//! themselves, counted headless, compare only on one backend (ADR-0075): they are never ranked.
//! No measured rate decides where a compositor ranks. A figure of the stack is ranked apart for a
//! compositor whose stack holds its parent (Hyprland's dwl).
//!
//! The memory at rest is the core's, after `rest`'s cost window, at each count of still windows:
//! its own memory (`Pss_Anon` + `Pss_File`) and its GPU memory (`resident-vram` +
//! `resident-gtt`), both ranked, and the clients' buffers it holds (`Pss_Shmem`), shown and not
//! ranked; and each one's growth a window, from the fewest windows to the most.
//!
//! A run file is read under the rule that a group which ran nothing woke exactly 0 times
//! ([`crate::record::Usage::ran_nothing`]), files written before it included.
//!
//! The summary ends with every rival of ADR-0075: those the files measured, those they did not,
//! and those that run on the owner's tour only, with why.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::machine::MachineInfo;
use crate::recipes::{self, Pace};
use crate::record::{self, CaseRun, FORMAT, MemorySum, RunFile};

/// Reads a run file, refusing another format than this bench's.
pub fn read(text: &str) -> Result<RunFile, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    match value["format"].as_u64() {
        Some(format) if format == u64::from(FORMAT) => {
            let mut file: RunFile = serde_json::from_value(value).map_err(|e| e.to_string())?;
            for phase in file.runs.iter_mut().flat_map(|run| &mut run.phases) {
                phase.settle_idle_groups();
            }
            Ok(file)
        }
        Some(format) => Err(format!("format {format}: this bench reads format {FORMAT}")),
        None => Err("not a run file: no format".to_owned()),
    }
}

/// Where a column's figure comes from.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// A figure of the cost window.
    Cost(&'static str),
    /// A figure of frames: the cost window's when it counted frames, else the frames window's,
    /// a lower bound.
    Frames(&'static str),
    /// The proportional set size after the scenario: the core's or the stack's.
    Pss(bool),
}

/// How a column is ranked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rank {
    /// Never: frames, and the clients' own figures.
    No,
    /// Only between compositors that drew at the same rate: a cost a second.
    PerSecond,
    /// Across every compositor: a count of drops, a memory.
    Total,
    /// Across backends, between compositors whose frames were counted the same exact way: a cost
    /// a frame.
    PerFrame,
}

/// A column of a scenario's table.
#[derive(Debug, Clone, Copy)]
struct Column {
    title: &'static str,
    source: Source,
    rank: Rank,
}

const fn cost(title: &'static str, key: &'static str) -> Column {
    Column {
        title,
        source: Source::Cost(key),
        rank: Rank::PerSecond,
    }
}

const fn total(title: &'static str, key: &'static str) -> Column {
    Column {
        title,
        source: Source::Cost(key),
        rank: Rank::Total,
    }
}

const fn per_frame(title: &'static str, key: &'static str) -> Column {
    Column {
        title,
        source: Source::Cost(key),
        rank: Rank::PerFrame,
    }
}

const fn shown(title: &'static str, key: &'static str) -> Column {
    Column {
        title,
        source: Source::Cost(key),
        rank: Rank::No,
    }
}

const fn frames(title: &'static str, key: &'static str) -> Column {
    Column {
        title,
        source: Source::Frames(key),
        rank: Rank::No,
    }
}

const PSS_CORE: Column = Column {
    title: "PSS after, core (MiB)",
    source: Source::Pss(true),
    rank: Rank::Total,
};
const PSS_STACK: Column = Column {
    title: "PSS after, stack (MiB)",
    source: Source::Pss(false),
    rank: Rank::Total,
};

/// Each scenario's columns. `rest`'s core memory is in the memory at rest, split into its parts.
fn columns(scenario: &str) -> Vec<Column> {
    match scenario {
        "rest" => vec![
            cost("Wake-ups a second, core", "core_wakeups"),
            cost("CPU, core (ms/s)", "core_cpu"),
            cost("CPU, stack (ms/s)", "stack_cpu"),
            cost("GPU, core (ms/s)", "core_gpu"),
            frames("Frames a second", "frames_per_s"),
            PSS_STACK,
            shown("Client's CPU (ms/s)", "client_still_cpu"),
        ],
        "game" => vec![
            frames("Frames a second", "frames_per_s"),
            per_frame("CPU a frame, core (µs)", "core_cpu_per_frame"),
            per_frame("GPU a frame, core (µs)", "core_gpu_per_frame"),
            per_frame("CPU a frame, stack (µs)", "stack_cpu_per_frame"),
            cost("CPU, core (ms/s)", "core_cpu"),
            cost("GPU, core (ms/s)", "core_gpu"),
            shown("mpv's CPU (ms/s)", "client_mpv_cpu"),
            shown("mpv's GPU (ms/s)", "client_mpv_gpu"),
            PSS_CORE,
        ],
        "film" => vec![
            frames("Frames a second", "frames_per_s"),
            shown("mpv's frames shown", "mpv_shown"),
            total("mpv's frames dropped", "mpv_frame_drop_count"),
            total("mpv's frames delayed", "mpv_vo_delayed_frame_count"),
            cost("CPU, core (ms/s)", "core_cpu"),
            cost("GPU, core (ms/s)", "core_gpu"),
            per_frame("CPU a frame, core (µs)", "core_cpu_per_frame"),
            shown("mpv's CPU (ms/s)", "client_mpv_cpu"),
        ],
        "pacing" => vec![
            shown("Presented", "presented"),
            shown("Between presentations, median (µs)", "p2p_median"),
            shown("p99 (µs)", "p2p_p99"),
            shown("Over 1.5 × the median", "p2p_over_1.5_median"),
            cost("CPU, core (ms/s)", "core_cpu"),
            cost("GPU, core (ms/s)", "core_gpu"),
        ],
        "motion" => vec![
            frames("Frames", "frames"),
            cost("CPU, core (ms/s)", "core_cpu"),
            cost("GPU, core (ms/s)", "core_gpu"),
            cost("Wake-ups a second, core", "core_wakeups"),
            per_frame("CPU a frame, core (µs)", "core_cpu_per_frame"),
        ],
        _ => Vec::new(),
    }
}

/// One run's value in a column.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Value {
    value: f64,
    /// Whether the run's file marks it a lower bound.
    lower_bound: bool,
}

impl Value {
    /// An exact value.
    fn exact(value: f64) -> Value {
        Value {
            value,
            lower_bound: false,
        }
    }
}

/// The memory a run read at the end of its scenario; none for a failed or refused run.
fn memory_after(run: &CaseRun) -> Option<&record::Memory> {
    if run.error.is_some() || !run.refused.is_empty() {
        return None;
    }
    run.memory.iter().rev().find(|m| m.when == "after")
}

/// A run's value in `column`, when it has one; none for a failed or refused run.
fn value(run: &CaseRun, column: &Column) -> Option<Value> {
    if run.error.is_some() || !run.refused.is_empty() {
        return None;
    }
    let figure = |phase: &str, key: &str| {
        let phase = run.phases.iter().find(|p| p.name == phase)?;
        Some(Value {
            value: phase.figures.get(key)?.value?,
            lower_bound: phase.lower_bounds.iter().any(|bound| bound == key),
        })
    };
    match column.source {
        Source::Cost(key) => figure("cost", key),
        Source::Frames(key) => figure("cost", key).or_else(|| figure("frames", key)),
        Source::Pss(core) => {
            let after = memory_after(run)?;
            let sum = if core { &after.core } else { &after.stack };
            Some(Value::exact(sum.pss.value? / 1024.0))
        }
    }
}

/// How the cost window of `runs` counted its frames, the first that says.
fn counted_by(runs: &[&CaseRun]) -> Option<String> {
    runs.iter().find_map(|run| {
        run.phases
            .iter()
            .find(|phase| phase.name == "cost")?
            .frames_counted_by
            .clone()
    })
}

/// The scenarios whose client draws as fast as the compositor takes its frames: their costs a
/// second scale with the backend's pace.
pub const PACED: [&str; 2] = ["game", "pacing"];

/// Whether `column` is a figure of the stack, which holds a compositor's parent where it has one.
fn of_stack(column: &Column) -> bool {
    match column.source {
        Source::Cost(key) | Source::Frames(key) => key.starts_with("stack_"),
        Source::Pss(core) => !core,
    }
}

/// Whether `runs` ran with a parent in their stack, as their settings record it.
fn has_parent(runs: &[&CaseRun]) -> bool {
    runs.iter()
        .find_map(|run| run.compositor.as_ref())
        .is_some_and(|info| info.settings.contains_key("parent"))
}

/// What a ranking holds the same (ADR-0072): one day and one machine state.
#[derive(Debug, Clone, PartialEq)]
struct State<'a> {
    /// The UTC day its run file started, such as `2026-11-16`.
    day: &'a str,
    /// The machine, whole: processor, memory, GPU, VBIOS, firmware, kernel and Mesa.
    machine: &'a MachineInfo,
    /// The GPU's performance levels its windows read, `None` for a reading that found none;
    /// empty for a run that measured no window.
    levels: BTreeSet<Option<&'a str>>,
}

impl<'a> State<'a> {
    /// The state `run` of `file` ran in.
    fn of(file: &'a RunFile, run: &'a CaseRun) -> State<'a> {
        State {
            day: day(file),
            machine: &file.machine,
            levels: levels(run),
        }
    }

    /// The rankings' state: that of `file`, the first named, at the levels of its first run that
    /// measured a window.
    fn reference(file: &'a RunFile) -> State<'a> {
        State {
            day: day(file),
            machine: &file.machine,
            levels: file
                .runs
                .iter()
                .map(levels)
                .find(|levels| !levels.is_empty())
                .unwrap_or_default(),
        }
    }

    /// Why a run in this state is not ranked with `reference`'s; none if it is. A run that
    /// measured no window has no level of its own, and no figure to rank.
    fn apart_from(&self, reference: &State) -> Option<String> {
        let mut why = Vec::new();
        if self.day != reference.day {
            why.push(format!("run on {}, not {}", self.day, reference.day));
        }
        if self.machine != reference.machine {
            why.extend(machine_differences(self.machine, reference.machine));
        }
        if !self.levels.is_empty() && self.levels != reference.levels {
            why.push(format!(
                "at GPU level {}, not {}",
                described(&self.levels),
                described(&reference.levels)
            ));
        }
        (!why.is_empty()).then(|| why.join("; "))
    }
}

/// The UTC day `file` started.
fn day(file: &RunFile) -> &str {
    file.started_utc
        .split('T')
        .next()
        .unwrap_or(&file.started_utc)
}

/// The GPU's performance levels `run`'s windows read, each second.
fn levels(run: &CaseRun) -> BTreeSet<Option<&str>> {
    run.phases
        .iter()
        .flat_map(|phase| &phase.load.seconds)
        .map(|second| second.state.performance_level.as_deref())
        .collect()
}

/// `levels` in words: `high`, or `auto and high` for a run during which it changed.
fn described(levels: &BTreeSet<Option<&str>>) -> String {
    if levels.is_empty() {
        return "unknown".to_owned();
    }
    levels
        .iter()
        .map(|level| level.unwrap_or("unknown"))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// What differs between `machine` and `reference`, in words: something, whenever they differ.
fn machine_differences(machine: &MachineInfo, reference: &MachineInfo) -> Vec<String> {
    let text = |value: &Option<String>| value.as_deref().unwrap_or("unknown").to_owned();
    let mut found = Vec::new();
    if machine.kernel != reference.kernel {
        found.push(format!(
            "Linux {}, not {}",
            text(&machine.kernel),
            text(&reference.kernel)
        ));
    }
    if machine.mesa_version != reference.mesa_version {
        found.push(format!(
            "Mesa {}, not {}",
            text(&machine.mesa_version),
            text(&reference.mesa_version)
        ));
    } else if machine.mesa_folder != reference.mesa_folder {
        found.push("another build of the same Mesa".to_owned());
    }
    if (&machine.cpu_model, machine.logical_cpus, machine.memory_kib)
        != (
            &reference.cpu_model,
            reference.logical_cpus,
            reference.memory_kib,
        )
    {
        found.push("another processor or memory".to_owned());
    }
    if machine.gpu != reference.gpu {
        found.push("another GPU, VBIOS or GPU firmware".to_owned());
    }
    if found.is_empty() {
        found.push("another machine".to_owned());
    }
    found
}

/// A row of a table: one compositor in one configuration, and, for runs outside the rankings'
/// day and machine state, why.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    compositor: String,
    configuration: String,
    /// Why its runs are not ranked: another day, machine or GPU level than the rankings'.
    apart: Option<String>,
}

impl Row {
    /// The row of `run`, apart for the reason given, if any.
    fn of(run: &CaseRun, apart: &Option<String>) -> Row {
        Row {
            compositor: run.case.compositor.clone(),
            configuration: run.case.configuration.clone(),
            apart: apart.clone(),
        }
    }

    /// The compositor's name in a table: its version, `†` when other programs disturbed one of
    /// `runs`, `‡` when the row is apart.
    fn name(&self, runs: &[&CaseRun]) -> String {
        let info = runs.iter().find_map(|run| run.compositor.as_ref());
        let mut name = match info.and_then(|info| info.version.as_deref()) {
            Some(version) => format!("{} {}", self.compositor, short_version(version)),
            None => self.compositor.clone(),
        };
        if runs.iter().any(|run| run.disturbed == Some(true)) {
            name.push_str(" †");
        }
        if self.apart.is_some() {
            name.push_str(" ‡");
        }
        name
    }
}

/// Each row's runs.
type Groups<'a> = BTreeMap<Row, Vec<&'a CaseRun>>;

/// A compositor's name and its runs' values in one column.
type Values = (String, Vec<f64>);

/// What a ranking line compares: each set of compositors ranked together, and those left out,
/// by why.
#[derive(Debug, Default)]
struct Ranking {
    /// By the pace, the parent or the way of counting frames they share; `""` for the others.
    sets: BTreeMap<String, Vec<Values>>,
    lower_bounds: Vec<String>,
    all_disturbed: Vec<String>,
    other_state: Vec<String>,
}

impl Ranking {
    /// Adds `row` to the set `set` names, with `values`, its quiet runs'; `disturbed_only` says
    /// that it has values from disturbed runs alone. A row not at equal work is left out; a row
    /// apart, one whose runs were all disturbed, and one with a lower bound are named, not
    /// ranked.
    fn add(
        &mut self,
        row: &Row,
        set: impl FnOnce() -> String,
        values: Vec<Value>,
        disturbed_only: bool,
    ) {
        if row.configuration != "equal-work" {
            return;
        }
        if row.apart.is_some() {
            if !values.is_empty() || disturbed_only {
                self.other_state.push(row.compositor.clone());
            }
            return;
        }
        if values.is_empty() {
            if disturbed_only {
                self.all_disturbed.push(row.compositor.clone());
            }
            return;
        }
        if values.iter().any(|value| value.lower_bound) {
            self.lower_bounds.push(row.compositor.clone());
            return;
        }
        self.sets.entry(set()).or_default().push((
            row.compositor.clone(),
            values.iter().map(|v| v.value).collect(),
        ));
    }

    /// The ranking as the summary writes it; `unshared`, when given, is what it says when no two
    /// compositors fall in one set.
    fn line(&self, unshared: Option<&str>) -> String {
        let ranked = |set: &[Values]| match ranks(set) {
            Ok(tiers) => tiers
                .iter()
                .map(|tier| tier.join(" = "))
                .collect::<Vec<_>>()
                .join(" < "),
            Err(reason) => format!("no ranking: {reason}"),
        };
        // A way of counting frames says what it is before its first colon, and how after it.
        let named = |name: &str| {
            if name.is_empty() {
                "the others".to_owned()
            } else {
                name.split(':').next().unwrap_or(name).to_owned()
            }
        };
        let mut text = match unshared {
            Some(unshared)
                if !self.sets.is_empty() && self.sets.values().all(|set| set.len() < 2) =>
            {
                format!("no ranking: {unshared}")
            }
            _ if self.sets.len() > 1 => {
                let mut parts: Vec<String> = self
                    .sets
                    .iter()
                    .filter(|(_, set)| set.len() >= 2)
                    .map(|(name, set)| format!("{}: {}", named(name), ranked(set)))
                    .collect();
                let alone: Vec<String> = self
                    .sets
                    .iter()
                    .filter(|(_, set)| set.len() < 2)
                    .map(|(name, set)| format!("{} ({})", set[0].0, named(name)))
                    .collect();
                if !alone.is_empty() {
                    parts.push(format!("alone: {}", alone.join(", ")));
                }
                parts.join("; ")
            }
            _ => match self.sets.iter().next() {
                Some((name, set)) if name.is_empty() => ranked(set),
                Some((name, set)) => format!("{}: {}", named(name), ranked(set)),
                None => "no ranking: no quiet equal-work run".to_owned(),
            },
        };
        for (left_out, names) in [
            ("unranked, lower bounds", &self.lower_bounds),
            (
                "left out, every equal-work run disturbed",
                &self.all_disturbed,
            ),
            ("unranked, another day or machine state", &self.other_state),
        ] {
            if !names.is_empty() {
                let _ = write!(text, "; {left_out}: {}", names.join(", "));
            }
        }
        text
    }
}

/// The values `value` gives the quiet runs of `runs`, and whether disturbed runs alone had one.
fn gather(runs: &[&CaseRun], value: impl Fn(&CaseRun) -> Option<Value>) -> (Vec<Value>, bool) {
    let quiet: Vec<&CaseRun> = runs
        .iter()
        .copied()
        .filter(|run| run.disturbed == Some(false))
        .collect();
    let values = quiet.iter().filter_map(|run| value(run)).collect();
    let disturbed_only = quiet.is_empty() && runs.iter().any(|run| value(run).is_some());
    (values, disturbed_only)
}

/// The ranking of `column` among `scenario`'s `groups`, as the summary writes it: over *equal
/// work* runs not disturbed, of the rankings' day and state, lower bounds left out; a cost a
/// second of a paced scenario only within one pace, a figure of the stack apart for a
/// compositor with a parent, a cost a frame only within one way of counting frames.
fn ranking(column: &Column, scenario: &str, groups: &Groups) -> String {
    let mut ranking = Ranking::default();
    for (row, runs) in groups {
        let (values, disturbed_only) = gather(runs, |run| value(run, column));
        let set = || {
            if of_stack(column) && has_parent(runs) {
                return "with its parent in its stack".to_owned();
            }
            match column.rank {
                Rank::PerSecond if PACED.contains(&scenario) => recipes::recipe(&row.compositor)
                    .map_or("on a pace not known", |recipe| recipe.pace.describe())
                    .to_owned(),
                Rank::PerFrame => format!(
                    "frames counted by {}",
                    counted_by(runs).unwrap_or_else(|| "?".to_owned())
                ),
                Rank::PerSecond | Rank::Total | Rank::No => String::new(),
            }
        };
        ranking.add(row, set, values, disturbed_only);
    }
    ranking.line(
        (column.rank == Rank::PerFrame)
            .then_some("no two compositors' frames were counted the same way"),
    )
}

/// A number as `docs/measurements.md` writes them: three figures or so, thousands apart.
pub fn number(value: f64) -> String {
    let text = if value.abs() >= 100.0 || value.fract() == 0.0 {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    };
    let (integer, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let (sign, digits) = integer
        .strip_prefix('-')
        .map_or(("", integer), |digits| ("-", digits));
    let mut grouped = String::new();
    for (n, digit) in digits.chars().enumerate() {
        if n > 0 && (digits.len() - n) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if fraction.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{fraction}")
    }
}

/// The lowest and highest of `values`, as a cell, `≥` before it when any is a lower bound.
fn cell(values: &[Value]) -> String {
    let low = values.iter().map(|v| v.value).fold(f64::INFINITY, f64::min);
    let high = values
        .iter()
        .map(|v| v.value)
        .fold(f64::NEG_INFINITY, f64::max);
    let bound = if values.iter().any(|v| v.lower_bound) {
        "≥ "
    } else {
        ""
    };
    match values.len() {
        0 => "—".to_owned(),
        _ if number(low) == number(high) => format!("{bound}{}", number(low)),
        _ => format!("{bound}{}–{}", number(low), number(high)),
    }
}

/// The ranks of `groups`, each a name and its runs' values, lower first: groups whose ranges of
/// runs overlap, directly or through another, share a rank. Refuses with fewer than two groups,
/// or fewer than two runs in any: the spread is then unknown.
pub fn ranks(groups: &[(String, Vec<f64>)]) -> Result<Vec<Vec<String>>, &'static str> {
    if groups.len() < 2 {
        return Err("one case alone");
    }
    if groups.iter().any(|(_, values)| values.len() < 2) {
        return Err("fewer than two runs of a case: their spread is unknown");
    }
    let range = |values: &[f64]| {
        (
            values.iter().copied().fold(f64::INFINITY, f64::min),
            values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        )
    };
    let mut sorted: Vec<(&str, (f64, f64))> = groups
        .iter()
        .map(|(name, values)| (name.as_str(), range(values)))
        .collect();
    sorted.sort_by(|a, b| a.1.0.total_cmp(&b.1.0));
    let mut tiers: Vec<Vec<String>> = Vec::new();
    // The highest value of the current rank's groups: a group starting at or below it overlaps.
    let mut tier_high = f64::NEG_INFINITY;
    for (name, (low, high)) in sorted {
        match tiers.last_mut() {
            Some(tier) if low <= tier_high => {
                tier.push(name.to_owned());
                tier_high = tier_high.max(high);
            }
            _ => {
                tiers.push(vec![name.to_owned()]);
                tier_high = high;
            }
        }
    }
    Ok(tiers)
}

/// The lines of `file`, named `name`, at the summary's head: what ran, when, where.
fn file_header(name: &str, file: &RunFile) -> String {
    let machine = &file.machine;
    let gpu = machine.gpu.as_ref();
    let levels: BTreeSet<Option<&str>> = file.runs.iter().flat_map(levels).collect();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "`{name}`: started {} UTC{}, commit `{}`{}, seed {}.",
        file.started_utc,
        match &file.written_utc {
            Some(written) => format!(", written {written} UTC after its last case"),
            None => String::new(),
        },
        file.commit.as_deref().unwrap_or("?"),
        if file.dirty == Some(true) {
            " with changes"
        } else {
            ""
        },
        file.seed
    );
    let _ = writeln!(
        out,
        "Machine: {}, {} processors; GPU `{}` ({}, VBIOS {}); Linux {}; Mesa {}; GPU \
         performance level {}. Lock: {}.{}",
        machine.cpu_model.as_deref().unwrap_or("?"),
        machine
            .logical_cpus
            .map_or("?".to_owned(), |n| n.to_string()),
        gpu.and_then(|g| g.pci_slot.as_deref()).unwrap_or("?"),
        gpu.and_then(|g| g.driver.as_deref()).unwrap_or("?"),
        gpu.and_then(|g| g.vbios.as_deref()).unwrap_or("?"),
        machine.kernel.as_deref().unwrap_or("?"),
        machine.mesa_version.as_deref().unwrap_or("?"),
        described(&levels),
        file.exclusive_lock,
        match file.seconds_override {
            Some(seconds) =>
                format!(" Every window lasted {seconds} s, not the scenarios' own: not comparable."),
            None => String::new(),
        }
    );
    out
}

/// The summary of `files`, each its name and its run.
pub fn summary(files: &[(String, RunFile)]) -> String {
    let mut out = String::from("# The rivals' bench\n\n");
    for (name, file) in files {
        out.push_str(&file_header(name, file));
        out.push('\n');
    }
    let reference = files.first().map(|(_, file)| State::reference(file));
    // Each run, with why it is apart from the rankings' day and state, if it is.
    let mut runs: Vec<(&CaseRun, Option<String>)> = Vec::new();
    for (_, file) in files {
        for run in &file.runs {
            let apart = reference
                .as_ref()
                .and_then(|reference| State::of(file, run).apart_from(reference));
            runs.push((run, apart));
        }
    }
    if let (Some(reference), Some((first, _))) = (&reference, files.first()) {
        let _ = writeln!(
            out,
            "Rankings compare runs of {} UTC on the machine of `{first}` at GPU level {} \
             (ADR-0072); runs of another day, machine or level are shown on rows of their own, \
             unranked.\n",
            reference.day,
            described(&reference.levels)
        );
    }
    let scenarios: BTreeSet<(String, Option<u32>)> = runs
        .iter()
        .map(|(run, _)| (run.case.scenario.clone(), run.case.windows))
        .collect();
    for (scenario, windows) in scenarios {
        let title = match windows {
            Some(windows) => format!("{scenario}, {windows} windows"),
            None => scenario.clone(),
        };
        let _ = writeln!(out, "## {title}\n");
        let mut groups: Groups = BTreeMap::new();
        for (run, apart) in runs
            .iter()
            .filter(|(run, _)| run.case.scenario == scenario && run.case.windows == windows)
        {
            groups.entry(Row::of(run, apart)).or_default().push(run);
        }
        out.push_str(&scenario_table(&scenario, &groups));
    }
    out.push_str(&memory_at_rest(&runs));
    out.push_str(&rivals(&runs));
    out.push_str(
        "† Other programs disturbed a run: more than 2 cores or 10 % of the GPU's drawing on \
         average.\n",
    );
    if runs.iter().any(|(_, apart)| apart.is_some()) {
        out.push_str(
            "‡ Runs of another day, machine or GPU level than the rankings': shown, never \
             ranked.\n",
        );
    }
    out
}

/// `scenario`'s table of `groups`, its rankings and its notes.
fn scenario_table(scenario: &str, groups: &Groups) -> String {
    let mut out = String::new();
    let columns = columns(scenario);
    let _ = write!(out, "| Compositor | Configuration | Backend | Runs |");
    for column in &columns {
        let _ = write!(out, " {} |", column.title);
    }
    let _ = write!(out, "\n|---|---|---|---|");
    for _ in &columns {
        out.push_str("---|");
    }
    out.push('\n');
    let mut notes = BTreeSet::new();
    for (row, runs) in groups {
        let (compositor, configuration) = (&row.compositor, &row.configuration);
        let failed = runs.iter().filter(|run| run.error.is_some()).count();
        let refused = runs.iter().filter(|run| !run.refused.is_empty()).count();
        let backend = runs
            .iter()
            .find_map(|run| run.compositor.as_ref())
            .map(|info| info.backend.as_str())
            .or_else(|| recipes::recipe(compositor).map(|recipe| recipe.backend));
        let _ = write!(
            out,
            "| {} | {configuration} | {} | {}{}{} |",
            row.name(runs),
            backend.unwrap_or("?"),
            runs.len(),
            if failed > 0 {
                format!(", {failed} failed")
            } else {
                String::new()
            },
            if refused > 0 {
                format!(", {refused} refused")
            } else {
                String::new()
            }
        );
        for column in &columns {
            let values: Vec<Value> = runs.iter().filter_map(|run| value(run, column)).collect();
            let _ = write!(out, " {} |", cell(&values));
        }
        out.push('\n');
        if let Some(apart) = &row.apart {
            notes.insert(format!(
                "{compositor}, {configuration} ‡: unranked: {apart}"
            ));
        }
        for run in runs {
            for note in &run.notes {
                notes.insert(format!("{compositor}: {note}"));
            }
            if let Some(error) = &run.error {
                notes.insert(format!(
                    "{compositor}, {configuration}, run {}: failed: {}",
                    run.repeat,
                    error.lines().next().unwrap_or("")
                ));
            }
            for refused in &run.refused {
                notes.insert(format!(
                    "{compositor}, {configuration}, run {}: refused: {refused}",
                    run.repeat
                ));
            }
            if let Some(output) = run
                .compositor
                .as_ref()
                .and_then(|info| info.output.as_ref())
            {
                notes.insert(format!("{compositor}'s output: {output}"));
            }
            if let Some(parent) = run
                .compositor
                .as_ref()
                .and_then(|info| info.settings.get("parent"))
            {
                notes.insert(format!(
                    "{compositor}'s stack holds its parent, and its figures of the stack rank \
                     apart: {parent}"
                ));
            }
        }
    }
    out.push('\n');
    for column in columns.iter().filter(|column| column.rank != Rank::No) {
        let _ = writeln!(
            out,
            "- {}, lowest first, among quiet equal-work runs: {}.",
            column.title,
            ranking(column, scenario, groups)
        );
    }
    let _ = writeln!(
        out,
        "- {}",
        if PACED.contains(&scenario) {
            "Its client draws as fast as the compositor takes its frames: a cost a second \
             ranks only between compositors on the same pace, wlroots' headless timer or \
             the mode's rate."
        } else {
            "At equal work it draws only when something changes, on no pace: a cost a \
             second ranks across every compositor."
        }
    );
    if columns.iter().any(|column| column.rank == Rank::PerFrame) {
        let _ = writeln!(
            out,
            "- A cost a frame does not scale with the pace: it ranks across backends, between \
             compositors whose frames were counted the same exact way."
        );
    }
    let _ = writeln!(
        out,
        "- Frames counted headless compare only between compositors on the same backend \
         (ADR-0075); none is ranked. `≥` marks a lower bound: frames counted by capture, \
         which misses some, or wake-ups of processes that ended between readings."
    );
    for note in notes {
        let _ = writeln!(out, "- {note}");
    }
    out.push('\n');
    out
}

/// A part of a compositor's memory at rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryPart {
    /// Its own: what it allocated and the files it mapped.
    Own,
    /// What it holds on the GPU.
    Gpu,
    /// The shared memory it maps: its clients' `wl_shm` buffers, whose size each layout sets.
    Buffers,
}

impl MemoryPart {
    /// Each, in the order the summary shows them.
    const ALL: [MemoryPart; 3] = [MemoryPart::Own, MemoryPart::Gpu, MemoryPart::Buffers];

    /// Its name, and where it is read.
    fn title(self) -> &'static str {
        match self {
            MemoryPart::Own => "Its own memory, `Pss_Anon` + `Pss_File`",
            MemoryPart::Gpu => "Its GPU memory, `resident-vram` + `resident-gtt`",
            MemoryPart::Buffers => "The clients' buffers it holds, `Pss_Shmem`",
        }
    }

    /// Whether it ranks: the clients' buffers are the clients' pixels, which the layout sizes.
    fn ranked(self) -> bool {
        self != MemoryPart::Buffers
    }

    /// Its value in `sum`, in KiB.
    fn kib(self, sum: &MemorySum) -> Option<f64> {
        match self {
            MemoryPart::Own => Some(sum.pss_anon.value? + sum.pss_file.value?),
            MemoryPart::Gpu => {
                Some(sum.drm.get("resident-vram")?.value? + sum.drm.get("resident-gtt")?.value?)
            }
            MemoryPart::Buffers => sum.pss_shmem.value,
        }
    }

    /// `run`'s value, in MiB: its core's at the end of its scenario.
    fn of(self, run: &CaseRun) -> Option<Value> {
        Some(Value::exact(self.kib(&memory_after(run)?.core)? / 1024.0))
    }
}

/// The growth a window from `fewest`, values at the fewest windows, to `most`, at the most, in
/// KiB over `span` windows: every run at the most against every run at the fewest, so that the
/// range holds every pairing of them.
fn growth(fewest: &[Value], most: &[Value], span: u32) -> Vec<Value> {
    most.iter()
        .flat_map(|most| {
            fewest.iter().map(move |fewest| {
                Value::exact((most.value - fewest.value) * 1024.0 / f64::from(span))
            })
        })
        .collect()
}

/// The runs of `by_count` at `count` windows; none if it ran none there.
fn runs_at<'a>(by_count: &BTreeMap<u32, Vec<&'a CaseRun>>, count: u32) -> Vec<&'a CaseRun> {
    by_count.get(&count).cloned().unwrap_or_default()
}

/// `n` windows, in words.
fn windows_named(n: u32) -> String {
    if n == 1 {
        "1 window".to_owned()
    } else {
        format!("{n} windows")
    }
}

/// The memory at rest: each row's core after `rest`'s cost window, at each count of windows the
/// files ran, in three tables, and its growth a window; empty without a `rest` run.
fn memory_at_rest(runs: &[(&CaseRun, Option<String>)]) -> String {
    let mut rows: BTreeMap<Row, BTreeMap<u32, Vec<&CaseRun>>> = BTreeMap::new();
    for (run, apart) in runs {
        if run.case.scenario == "rest"
            && let Some(windows) = run.case.windows
        {
            rows.entry(Row::of(run, apart))
                .or_default()
                .entry(windows)
                .or_default()
                .push(run);
        }
    }
    let counts: Vec<u32> = rows
        .values()
        .flat_map(|by_count| by_count.keys().copied())
        .collect::<BTreeSet<u32>>()
        .into_iter()
        .collect();
    let (Some(&fewest), Some(&most)) = (counts.first(), counts.last()) else {
        return String::new();
    };
    let span = (fewest < most).then_some(most - fewest);
    let mut out = String::from("## Memory at rest\n\n");
    let _ = writeln!(
        out,
        "The core's memory after `rest`'s cost window, its still windows shown: the compositor's \
         process and its descendants, from their `smaps_rollup` and the DRM `fdinfo` of their \
         GPU files, each DRM client counted once. In MiB; a window's growth, in KiB, from {} to \
         {}, every run at {most} against every run at {fewest}. Its own memory and its GPU \
         memory rank; the clients' buffers it holds are shown, not ranked: their size is the \
         layout's.\n",
        windows_named(fewest),
        windows_named(most)
    );
    for part in MemoryPart::ALL {
        let _ = writeln!(out, "### {}\n", part.title());
        let _ = write!(out, "| Compositor | Configuration | Runs |");
        for &count in &counts {
            let _ = write!(out, " {} |", windows_named(count));
        }
        if span.is_some() {
            let _ = write!(out, " A window (KiB) |");
        }
        let _ = write!(out, "\n|---|---|---|");
        for _ in 0..counts.len() + usize::from(span.is_some()) {
            out.push_str("---|");
        }
        out.push('\n');
        for (row, by_count) in &rows {
            let every: Vec<&CaseRun> = by_count.values().flatten().copied().collect();
            let runs: Vec<String> = counts
                .iter()
                .map(|&count| runs_at(by_count, count).len().to_string())
                .collect();
            let _ = write!(
                out,
                "| {} | {} | {} |",
                row.name(&every),
                row.configuration,
                runs.join(", ")
            );
            let values = |count: u32| -> Vec<Value> {
                runs_at(by_count, count)
                    .iter()
                    .filter_map(|run| part.of(run))
                    .collect()
            };
            for &count in &counts {
                let _ = write!(out, " {} |", cell(&values(count)));
            }
            if let Some(span) = span {
                let _ = write!(
                    out,
                    " {} |",
                    cell(&growth(&values(fewest), &values(most), span))
                );
            }
            out.push('\n');
        }
        out.push('\n');
        if !part.ranked() {
            continue;
        }
        let name = part.title().split(',').next().unwrap_or_default();
        for &count in &counts {
            let mut ranking = Ranking::default();
            for (row, by_count) in &rows {
                let (values, disturbed_only) =
                    gather(&runs_at(by_count, count), |run| part.of(run));
                ranking.add(row, String::new, values, disturbed_only);
            }
            let _ = writeln!(
                out,
                "- {name} at {}, lowest first, among quiet equal-work runs: {}.",
                windows_named(count),
                ranking.line(None)
            );
        }
        if let Some(span) = span {
            let mut ranking = Ranking::default();
            for (row, by_count) in &rows {
                let (at_fewest, fewest_disturbed) =
                    gather(&runs_at(by_count, fewest), |run| part.of(run));
                let (at_most, most_disturbed) =
                    gather(&runs_at(by_count, most), |run| part.of(run));
                let mut values = growth(&at_fewest, &at_most, span);
                // With fewer than two quiet runs at either end, the growth's spread is unknown:
                // one pairing alone, and the ranking says so.
                if at_fewest.len() < 2 || at_most.len() < 2 {
                    values.truncate(1);
                }
                ranking.add(row, String::new, values, fewest_disturbed || most_disturbed);
            }
            let _ = writeln!(
                out,
                "- {name}, its growth a window, lowest first, among quiet equal-work runs: {}.",
                ranking.line(None)
            );
        }
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "- Window sizes differ by layout: a tiling compositor shares the output between the \
         still windows, so that each buffer shrinks as windows open; a floating one leaves each \
         the 640 × 480 it chooses. Each run's file records each still window's configured size \
         and buffer (`still_windows`), `null` in files written before them."
    );
    for (row, by_count) in &rows {
        let drawn: Vec<String> = counts
            .iter()
            .filter_map(|&count| {
                let values: Vec<Value> = runs_at(by_count, count)
                    .iter()
                    .filter_map(|run| buffers_drawn(run))
                    .collect();
                (!values.is_empty())
                    .then(|| format!("{} MiB at {}", cell(&values), windows_named(count)))
            })
            .collect();
        if !drawn.is_empty() {
            let _ = writeln!(
                out,
                "- {}, {}: its still windows' buffers, by their recorded sizes: {}.",
                row.compositor,
                row.configuration,
                drawn.join(", ")
            );
        }
    }
    out.push('\n');
    out
}

/// The size of the buffers `run`'s still windows last drew, in MiB, four bytes a pixel; none
/// where its file does not record them.
fn buffers_drawn(run: &CaseRun) -> Option<Value> {
    let windows = run.still_windows.as_ref()?;
    let bytes: f64 = windows
        .iter()
        .map(|window| f64::from(window.buffer[0]) * f64::from(window.buffer[1]) * 4.0)
        .sum();
    Some(Value::exact(bytes / (1024.0 * 1024.0)))
}

/// The summary's closing section: every rival of ADR-0075, measured in `runs` or not, those that
/// run on the tour only and why, the scenarios a recipe cannot run, and how wlroots' headless
/// backend paces the rivals on it, with the rates their runs of `game` reached.
fn rivals(runs: &[(&CaseRun, Option<String>)]) -> String {
    let ran: BTreeSet<&str> = runs
        .iter()
        .map(|(run, _)| run.case.compositor.as_str())
        .collect();
    let (measured, not_run): (Vec<&str>, Vec<&str>) = recipes::ADR_0075_RIVALS
        .iter()
        .filter(|rival| recipes::tour_only(rival).is_none())
        .partition(|rival| ran.contains(*rival));
    let none = |names: &[&str]| {
        if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(", ")
        }
    };
    let mut out = String::from("## Every rival of ADR-0075\n\n");
    let _ = writeln!(out, "- Measured in these files: {}.", none(&measured));
    let _ = writeln!(out, "- Not run in these files: {}.", none(&not_run));
    for rival in &recipes::TOUR_ONLY {
        let _ = writeln!(out, "- {}: tour only: {}.", rival.name, rival.why);
    }
    for recipe in &recipes::RECIPES {
        if let Some(why) = recipe.no_actions {
            let _ = writeln!(out, "- {}: `motion` not run: {why}.", recipe.name);
        }
    }
    let wlroots: Vec<&str> = recipes::RECIPES
        .iter()
        .filter(|recipe| recipe.pace == Pace::WlrootsTimer)
        .map(|recipe| recipe.name)
        .collect();
    let reached = match wlroots_game_rates(runs) {
        Some((low, high)) => format!(": {low:.1} to {high:.1} in `game` in these files"),
        None => String::new(),
    };
    let _ = writeln!(
        out,
        "- The rivals on wlroots' headless backend ({}) draw a frame 4 ms after each commit: \
         it counts whole milliseconds, 1000000 / 240000 = 4 (wlroots 0.20.2, \
         backend/headless/output.c:31 and :76). That is up to {} frames a second for a 240 Hz \
         mode, fewer as each frame takes time to draw{reached}. The others draw at the mode's \
         rate, 240 frames a second at most. In `game` and `pacing`, costs a second rank within \
         one of these paces.\n",
        wlroots.join(", "),
        number(recipes::WLROOTS_HEADLESS_HZ)
    );
    out
}

/// The lowest and highest rate the rivals on wlroots' headless timer reached in `runs` of
/// `game`, exact rates alone; none without such a run.
fn wlroots_game_rates(runs: &[(&CaseRun, Option<String>)]) -> Option<(f64, f64)> {
    let rates: Vec<f64> = runs
        .iter()
        .filter(|(run, _)| {
            run.case.scenario == "game"
                && run.error.is_none()
                && run.refused.is_empty()
                && recipes::recipe(&run.case.compositor)
                    .is_some_and(|recipe| recipe.pace == Pace::WlrootsTimer)
        })
        .filter_map(|(run, _)| {
            let cost = run.phases.iter().find(|phase| phase.name == "cost")?;
            let key = "achieved_frames_per_s";
            let rate = cost.figures.get(key)?.value?;
            (!cost.lower_bounds.iter().any(|bound| bound == key)).then_some(rate)
        })
        .collect();
    let low = rates.iter().copied().reduce(f64::min)?;
    let high = rates.iter().copied().reduce(f64::max)?;
    Some((low, high))
}

/// A version short enough for a table: a commit's first 7 characters, its `-dirty` kept.
fn short_version(version: &str) -> String {
    let (commit, dirty) = version
        .strip_suffix("-dirty")
        .map_or((version, ""), |commit| (commit, "-dirty"));
    if commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("`{}`{dirty}", &commit[..7])
    } else {
        version.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{MachineState, Second};
    use crate::record::{Figure, Memory, Phase, StillWindow};

    fn group(name: &str, values: &[f64]) -> (String, Vec<f64>) {
        (name.to_owned(), values.to_vec())
    }

    #[test]
    fn no_ranking_where_the_difference_is_under_the_spread() {
        // sway's runs and KWin's overlap: one rank; Yuna's are above both.
        let groups = [
            group("yuna", &[9.0, 9.5, 10.0]),
            group("kwin", &[3.0, 4.2, 3.5]),
            group("sway", &[4.0, 4.5, 4.1]),
        ];
        assert_eq!(
            ranks(&groups),
            Ok(vec![
                vec!["kwin".to_owned(), "sway".to_owned()],
                vec!["yuna".to_owned()]
            ])
        );
        // Apart: three ranks.
        let apart = [
            group("a", &[1.0, 1.1]),
            group("b", &[2.0, 2.1]),
            group("c", &[3.0, 3.1]),
        ];
        assert_eq!(ranks(&apart).unwrap().len(), 3);
        // A chain of overlaps is one rank.
        let chain = [
            group("a", &[1.0, 2.0]),
            group("b", &[1.9, 3.0]),
            group("c", &[2.9, 4.0]),
        ];
        assert_eq!(ranks(&chain).unwrap().len(), 1);
        // One run: the spread is unknown.
        assert!(ranks(&[group("a", &[1.0]), group("b", &[5.0, 6.0])]).is_err());
        assert!(ranks(&[group("a", &[1.0, 2.0])]).is_err());
    }

    #[test]
    fn numbers_are_written_as_the_measurements_write_them() {
        assert_eq!(number(1375.4), "1,375");
        assert_eq!(number(1234567.0), "1,234,567");
        assert_eq!(number(42.25), "42.2");
        assert_eq!(number(0.123), "0.12");
        assert_eq!(number(-2500.0), "-2,500");
        let exact = Value::exact;
        assert_eq!(cell(&[exact(1375.0), exact(1393.0)]), "1,375–1,393");
        assert_eq!(cell(&[exact(7.0)]), "7");
        assert_eq!(cell(&[exact(7.5)]), "7.50");
        assert_eq!(cell(&[]), "—");
        let bound = Value {
            value: 222.0,
            lower_bound: true,
        };
        assert_eq!(cell(&[bound]), "≥ 222");
    }

    /// A run of `compositor` in `configuration` on `backend`, whose cost window holds `key` at
    /// `value`, its frames counted by mpv, at the GPU level `high`.
    fn run(
        (compositor, configuration, backend): (&str, &str, &str),
        key: &str,
        value: f64,
        lower_bound: bool,
        disturbed: Option<bool>,
    ) -> CaseRun {
        let mut phase = Phase {
            name: "cost".to_owned(),
            frames_counted_by: Some("mpv's frames shown: over its IPC".to_owned()),
            ..Default::default()
        };
        phase
            .figures
            .insert(key.to_owned(), Figure::new(Some(value), "us"));
        if lower_bound {
            phase.lower_bounds.push(key.to_owned());
        }
        phase.load.seconds.push(at_level("high"));
        CaseRun {
            case: crate::record::Case {
                compositor: compositor.to_owned(),
                configuration: configuration.to_owned(),
                scenario: "game".to_owned(),
                windows: None,
            },
            compositor: Some(crate::record::CompositorInfo {
                backend: backend.to_owned(),
                ..Default::default()
            }),
            phases: vec![phase],
            disturbed,
            ..Default::default()
        }
    }

    /// A second of a window that read the GPU at `level`.
    fn at_level(level: &str) -> Second {
        Second {
            at_s: 1.0,
            others_cpu_cores: Some(0.0),
            others_gpu_percent: Some(0.0),
            bench_cpu_cores: 0.0,
            state: MachineState {
                performance_level: Some(level.to_owned()),
                ..MachineState::default()
            },
        }
    }

    /// Two quiet equal-work runs of `compositor` whose `key` is `values`.
    fn quiet_pair(compositor: &str, key: &str, values: [f64; 2]) -> [CaseRun; 2] {
        values.map(|value| {
            run(
                (compositor, "equal-work", "b"),
                key,
                value,
                false,
                Some(false),
            )
        })
    }

    fn grouped(runs: &[CaseRun]) -> Groups<'_> {
        let mut groups: Groups = BTreeMap::new();
        for run in runs {
            groups.entry(Row::of(run, &None)).or_default().push(run);
        }
        groups
    }

    #[test]
    fn rankings_use_quiet_equal_work_runs_and_no_lower_bound() {
        let quiet = Some(false);
        let runs = [
            run(("sway", "equal-work", "w"), "core_cpu", 1.0, false, quiet),
            run(("sway", "equal-work", "w"), "core_cpu", 1.1, false, quiet),
            run(("kwin", "equal-work", "k"), "core_cpu", 3.0, false, quiet),
            run(("kwin", "equal-work", "k"), "core_cpu", 3.1, false, quiet),
            run(("yuna", "equal-work", "y"), "core_cpu", 0.5, false, quiet),
            run(("yuna", "equal-work", "y"), "core_cpu", 0.6, false, quiet),
            // A disturbed run, and runs as shipped, are shown, not ranked.
            run(
                ("yuna", "equal-work", "y"),
                "core_cpu",
                9.0,
                false,
                Some(true),
            ),
            run(("sway", "as-shipped", "w"), "core_cpu", 0.01, false, quiet),
            run(("sway", "as-shipped", "w"), "core_cpu", 0.02, false, quiet),
        ];
        let column = cost("CPU", "core_cpu");
        assert_eq!(
            ranking(&column, "rest", &grouped(&runs)),
            "yuna < sway < kwin"
        );
        // KWin's wake-ups are a lower bound: it is left out of the ranking, and said so.
        let wakeups = |compositor: &str, value: f64, lower_bound: bool| {
            run(
                (compositor, "equal-work", "w"),
                "core_wakeups",
                value,
                lower_bound,
                quiet,
            )
        };
        let runs = [
            wakeups("sway", 1.0, false),
            wakeups("sway", 1.1, false),
            wakeups("yuna", 0.0, false),
            wakeups("yuna", 0.0, false),
            wakeups("kwin", 0.1, true),
            wakeups("kwin", 0.1, true),
        ];
        let column = cost("Wake-ups", "core_wakeups");
        assert_eq!(
            ranking(&column, "rest", &grouped(&runs)),
            "yuna < sway; unranked, lower bounds: kwin"
        );
    }

    #[test]
    fn a_core_that_ran_nothing_ranks_with_exactly_zero_wake_ups_even_from_an_old_file() {
        // dwl at rest ran no CPU time; a file written before the rule marked its wake-ups a lower
        // bound, as KWin's are, which ran.
        let idle = |compositor: &str, cpu_ns: f64, value: f64| {
            let mut run = run(
                (compositor, "equal-work", "w"),
                "core_wakeups",
                value,
                true,
                Some(false),
            );
            run.case.scenario = "rest".to_owned();
            run.case.windows = Some(15);
            run.phases[0].core.cpu = Figure::new(Some(cpu_ns), "ns");
            run.phases[0].core.wakeups_lower_bound = true;
            run
        };
        let file = RunFile {
            format: FORMAT,
            started_utc: "2026-11-16T06:14:01Z".to_owned(),
            runs: vec![
                idle("dwl", 0.0, 0.0),
                idle("dwl", 0.0, 0.0),
                idle("kwin", 2_000.0, 0.1),
                idle("kwin", 2_000.0, 0.2),
            ],
            ..RunFile::default()
        };
        let file = read(&serde_json::to_string(&file).unwrap()).unwrap();
        assert!(!file.runs[0].phases[0].core.wakeups_lower_bound);
        let text = summary(&[("run.json".to_owned(), file)]);
        assert!(
            text.contains(
                "- Wake-ups a second, core, lowest first, among quiet equal-work runs: no \
                 ranking: one case alone; unranked, lower bounds: kwin."
            ),
            "{text}"
        );
        assert!(text.contains("| dwl | equal-work | w | 2 | 0 |"), "{text}");
    }

    #[test]
    fn costs_a_second_rank_within_one_pace_where_the_client_paces_the_compositor() {
        let runs: Vec<CaseRun> = [
            ("kwin", [3.0, 3.1]),
            ("yuna", [2.0, 2.1]),
            ("hyprland", [4.0, 4.1]),
            // On wlroots' headless timer, whatever the rate each reached.
            ("sway", [1.0, 1.1]),
            ("dwl", [0.5, 0.6]),
            ("umbriel", [0.8, 0.85]),
        ]
        .into_iter()
        .flat_map(|(compositor, values)| quiet_pair(compositor, "core_cpu", values))
        .collect();
        let column = cost("CPU", "core_cpu");
        assert_eq!(
            ranking(&column, "game", &grouped(&runs)),
            "at the mode's rate, 240 frames a second at most: yuna < kwin < hyprland; on \
             wlroots' headless timer, a frame 4 ms after each commit: dwl < umbriel < sway"
        );
        // At rest, and in motion, no pace sets the costs: every compositor ranks together.
        assert_eq!(
            ranking(&column, "rest", &grouped(&runs)),
            "dwl < umbriel < sway < yuna < kwin < hyprland"
        );
        // A memory is no cost a second: every compositor ranks together, in game too.
        let memory = total("Memory", "core_cpu");
        assert_eq!(
            ranking(&memory, "game", &grouped(&runs)),
            "dwl < umbriel < sway < yuna < kwin < hyprland"
        );
    }

    #[test]
    fn a_stack_that_holds_a_parent_ranks_apart() {
        let mut runs: Vec<CaseRun> = [("kwin", [3.0, 3.1]), ("sway", [1.0, 1.1])]
            .into_iter()
            .flat_map(|(compositor, values)| quiet_pair(compositor, "stack_cpu", values))
            .collect();
        for mut hyprland in quiet_pair("hyprland", "stack_cpu", [0.2, 0.3]) {
            hyprland
                .compositor
                .as_mut()
                .unwrap()
                .settings
                .insert("parent".to_owned(), "dwl 0.9".to_owned());
            runs.push(hyprland);
        }
        let stack = cost("CPU, stack", "stack_cpu");
        assert_eq!(
            ranking(&stack, "rest", &grouped(&runs)),
            "the others: sway < kwin; alone: hyprland (with its parent in its stack)"
        );
        // Its core's figures rank with the others'.
        for run in &mut runs {
            let value = run.phases[0].figures.remove("stack_cpu").unwrap();
            run.phases[0].figures.insert("core_cpu".to_owned(), value);
        }
        assert_eq!(
            ranking(&cost("CPU", "core_cpu"), "rest", &grouped(&runs)),
            "hyprland < sway < kwin"
        );
    }

    #[test]
    fn a_compositor_without_a_quiet_run_is_named_not_dropped() {
        let mut runs: Vec<CaseRun> = [("sway", [1.0, 1.1]), ("kwin", [3.0, 3.1])]
            .into_iter()
            .flat_map(|(compositor, values)| quiet_pair(compositor, "core_cpu", values))
            .collect();
        for value in [0.5, 0.6] {
            runs.push(run(
                ("yuna", "equal-work", "y"),
                "core_cpu",
                value,
                false,
                Some(true),
            ));
        }
        // Mutter's frames are not counted at rest: it ranks with the others all the same.
        runs.extend(quiet_pair("mutter", "core_cpu", [0.1, 0.1]));
        assert_eq!(
            ranking(&cost("CPU", "core_cpu"), "rest", &grouped(&runs)),
            "mutter < sway < kwin; left out, every equal-work run disturbed: yuna"
        );
    }

    #[test]
    fn costs_a_frame_rank_across_backends_within_one_way_of_counting() {
        let quiet = Some(false);
        let per_frame_run = |compositor: &str, backend: &str, value: f64| {
            run(
                (compositor, "equal-work", backend),
                "core_cpu_per_frame",
                value,
                false,
                quiet,
            )
        };
        // mpv counted every compositor's frames in `game`, whatever its backend and its pace.
        let mut runs = vec![
            per_frame_run("sway", "wlroots-headless", 150.0),
            per_frame_run("sway", "wlroots-headless", 151.0),
            per_frame_run("kwin", "kwin-virtual", 300.0),
            per_frame_run("kwin", "kwin-virtual", 301.0),
            per_frame_run("yuna", "yuna-headless", 160.0),
            per_frame_run("yuna", "yuna-headless", 166.0),
        ];
        let column = per_frame("CPU a frame", "core_cpu_per_frame");
        assert_eq!(
            ranking(&column, "game", &grouped(&runs)),
            "frames counted by mpv's frames shown: sway < yuna < kwin"
        );
        // Elsewhere, each compositor counts its own frames its own way: none shares one.
        for (run, by) in runs.iter_mut().zip([
            "KWin's frame log, in this window",
            "KWin's frame log, in this window",
            "Umbriel's count of its output's commits: read at this window's ends",
            "Umbriel's count of its output's commits: read at this window's ends",
        ]) {
            run.phases[0].frames_counted_by = Some(by.to_owned());
        }
        runs.truncate(4);
        assert_eq!(
            ranking(&column, "film", &grouped(&runs)),
            "no ranking: no two compositors' frames were counted the same way"
        );
        // One way shared by two, another by one alone.
        runs.extend([
            per_frame_run("dwl", "wlroots-headless", 140.0),
            per_frame_run("dwl", "wlroots-headless", 141.0),
        ]);
        for run in &mut runs[4..] {
            run.phases[0].frames_counted_by = Some("KWin's frame log, in this window".to_owned());
        }
        assert_eq!(
            ranking(&column, "film", &grouped(&runs)),
            "frames counted by KWin's frame log, in this window: dwl < sway; alone: kwin \
             (frames counted by Umbriel's count of its output's commits)"
        );
    }

    /// A machine as a run file records it, on `kernel`.
    fn machine(kernel: &str) -> MachineInfo {
        MachineInfo {
            kernel: Some(kernel.to_owned()),
            cpu_model: Some("AMD Ryzen 7 5800X 8-Core Processor".to_owned()),
            logical_cpus: Some(16),
            mesa_version: Some("26.2.6".to_owned()),
            ..MachineInfo::default()
        }
    }

    #[test]
    fn a_ranking_holds_the_runs_of_one_day_and_machine_state() {
        let file = |started: &str, kernel: &str, runs: Vec<CaseRun>| RunFile {
            format: FORMAT,
            started_utc: started.to_owned(),
            machine: machine(kernel),
            runs,
            ..RunFile::default()
        };
        let pair = |compositor: &str, values: [f64; 2]| -> Vec<CaseRun> {
            quiet_pair(compositor, "core_cpu", values).to_vec()
        };
        let mut at_auto = pair("mango", [0.1, 0.2]);
        for run in &mut at_auto {
            run.phases[0].load.seconds = vec![at_level("auto")];
        }
        let files = [
            (
                "a.json".to_owned(),
                file(
                    "2026-11-16T06:14:01Z",
                    "7.3.1",
                    [pair("sway", [1.0, 1.1]), pair("kwin", [3.0, 3.1]), at_auto].concat(),
                ),
            ),
            // The same day, later, on the same machine: it ranks.
            (
                "b.json".to_owned(),
                file("2026-11-16T10:21:20Z", "7.3.1", pair("dwl", [0.5, 0.6])),
            ),
            // The next day, and another kernel: shown, not ranked.
            (
                "c.json".to_owned(),
                file("2026-11-17T08:00:00Z", "7.3.2", pair("yuna", [0.2, 0.3])),
            ),
        ];
        let text = summary(&files);
        assert!(
            text.contains(
                "Rankings compare runs of 2026-11-16 UTC on the machine of `a.json` at GPU level \
                 high (ADR-0072)"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "- CPU, core (ms/s), lowest first, among quiet equal-work runs: on wlroots' \
                 headless timer, a frame 4 ms after each commit: dwl < sway; alone: kwin (at the \
                 mode's rate, 240 frames a second at most); unranked, another day or machine \
                 state: mango, yuna."
            ),
            "{text}"
        );
        assert!(
            text.contains("- mango, equal-work ‡: unranked: at GPU level auto, not high"),
            "{text}"
        );
        assert!(
            text.contains(
                "- yuna, equal-work ‡: unranked: run on 2026-11-17, not 2026-11-16; Linux \
                 7.3.2, not 7.3.1"
            ),
            "{text}"
        );
        assert!(text.contains("| yuna ‡ | equal-work |"), "{text}");
        assert!(text.contains("‡ Runs of another day"), "{text}");
        // One file alone, one state: no row apart, no mark.
        let alone = summary(&files[..1]);
        assert!(alone.contains("| mango ‡ |"), "{alone}");
        assert!(!summary(&files[1..2]).contains('‡'));
    }

    /// A run of `rest` with `windows` windows, its core's memory after it as given, in KiB.
    fn at_rest(
        compositor: &str,
        windows: u32,
        [anon, file, shmem, vram, gtt]: [f64; 5],
    ) -> CaseRun {
        let kib = |value: f64| Figure::new(Some(value), "KiB");
        let core = MemorySum {
            pss: kib(anon + file + shmem),
            pss_anon: kib(anon),
            pss_file: kib(file),
            pss_shmem: kib(shmem),
            rss: kib(anon + file + shmem),
            drm: [("resident-vram", vram), ("resident-gtt", gtt)]
                .into_iter()
                .map(|(key, value)| (key.to_owned(), kib(value)))
                .collect(),
        };
        let mut run = run(
            (compositor, "equal-work", "b"),
            "core_cpu",
            1.0,
            false,
            Some(false),
        );
        run.case.scenario = "rest".to_owned();
        run.case.windows = Some(windows);
        run.memory.push(Memory {
            when: "after".to_owned(),
            core: core.clone(),
            stack: core,
            processes: Vec::new(),
        });
        run
    }

    #[test]
    fn memory_at_rest_ranks_its_own_and_the_gpus_and_shows_the_clients_buffers() {
        let mib = 1024.0;
        // Each compositor's runs, in KiB: anonymous, files, shared memory, VRAM and GTT. Over the
        // 149 windows from 1 to 150, Umbriel's own memory grows by 30 KiB a window, Yuna's by
        // 320.
        let umbriel = [
            (1, [20.0 * mib, 80.0 * mib, mib, 4.0 * mib, 2.0 * mib]),
            (
                15,
                [20.0 * mib, 80.0 * mib, 40.0 * mib, 4.0 * mib, 2.0 * mib],
            ),
            (
                150,
                [
                    20.0 * mib + 149.0 * 30.0,
                    80.0 * mib,
                    300.0 * mib,
                    5.0 * mib,
                    2.0 * mib,
                ],
            ),
        ];
        let yuna = [
            (1, [20.0 * mib, 90.0 * mib, mib, 8.0 * mib, 2.0 * mib]),
            (
                15,
                [24.0 * mib, 90.0 * mib, 10.0 * mib, 40.0 * mib, 2.0 * mib],
            ),
            (
                150,
                [
                    20.0 * mib + 149.0 * 320.0,
                    90.0 * mib,
                    30.0 * mib,
                    300.0 * mib,
                    2.0 * mib,
                ],
            ),
        ];
        let mut runs = Vec::new();
        for _ in 0..2 {
            for (compositor, memory) in [("umbriel", umbriel), ("yuna", yuna)] {
                for (windows, kib) in memory {
                    runs.push(at_rest(compositor, windows, kib));
                }
            }
        }
        // Yuna's windows' sizes at 15 windows, recorded by a newer bench: 640 × 480 at 1.5.
        for run in runs
            .iter_mut()
            .filter(|run| run.case.compositor == "yuna" && run.case.windows == Some(15))
        {
            run.still_windows = Some(vec![
                StillWindow {
                    configured: [0, 0],
                    buffer: [960, 720],
                };
                15
            ]);
        }
        let file = RunFile {
            format: FORMAT,
            runs,
            ..RunFile::default()
        };
        let text = summary(&[("run.json".to_owned(), file)]);
        let section = &text[text.find("## Memory at rest").expect("a memory section")..];
        assert!(
            section.contains(
                "| Compositor | Configuration | Runs | 1 window | 15 windows | 150 windows | A \
                 window (KiB) |"
            ),
            "{section}"
        );
        // Its own: 100 MiB at 1 and 15 windows, 104.4 at 150, 30 KiB a window.
        assert!(
            section.contains("| umbriel | equal-work | 2, 2, 2 | 100 | 100 | 104 | 30 |"),
            "{section}"
        );
        assert!(
            section.contains("| yuna | equal-work | 2, 2, 2 | 110 | 114 | 157 | 320 |"),
            "{section}"
        );
        assert!(
            section.contains(
                "- Its own memory at 15 windows, lowest first, among quiet equal-work runs: \
                 umbriel < yuna."
            ),
            "{section}"
        );
        assert!(
            section.contains(
                "- Its own memory, its growth a window, lowest first, among quiet equal-work \
                 runs: umbriel < yuna."
            ),
            "{section}"
        );
        assert!(
            section.contains(
                "- Its GPU memory at 150 windows, lowest first, among quiet equal-work runs: \
                 umbriel < yuna."
            ),
            "{section}"
        );
        // The clients' buffers are shown, never ranked.
        assert!(section.contains("### The clients' buffers it holds, `Pss_Shmem`"));
        assert!(
            !section.contains("- The clients' buffers it holds at"),
            "{section}"
        );
        assert!(
            section.contains("| yuna | equal-work | 2, 2, 2 | 1 | 10 | 30 |"),
            "{section}"
        );
        assert!(
            section.contains("- Window sizes differ by layout"),
            "{section}"
        );
        assert!(
            section.contains(
                "- yuna, equal-work: its still windows' buffers, by their recorded sizes: 39.6 \
                 MiB at 15 windows."
            ),
            "{section}"
        );
        assert!(
            !section.contains("- umbriel, equal-work: its still windows'"),
            "{section}"
        );
    }

    #[test]
    fn a_file_without_the_fields_added_since_reads_them_as_null() {
        let file = RunFile {
            format: FORMAT,
            written_utc: Some("2026-11-16T09:21:00Z".to_owned()),
            runs: vec![CaseRun {
                still_windows: Some(Vec::new()),
                ..CaseRun::default()
            }],
            ..RunFile::default()
        };
        let mut value = serde_json::to_value(&file).unwrap();
        value.as_object_mut().unwrap().remove("written_utc");
        value["runs"][0]
            .as_object_mut()
            .unwrap()
            .remove("still_windows");
        let read = read(&value.to_string()).unwrap();
        assert_eq!(read.written_utc, None);
        assert_eq!(read.runs[0].still_windows, None);
        // Written again, they are `null`.
        let again = serde_json::to_value(&read).unwrap();
        assert!(again["written_utc"].is_null());
        assert!(again["runs"][0]["still_windows"].is_null());
    }

    #[test]
    fn the_wlroots_rate_comes_from_the_runs_not_from_a_date() {
        let rate = |compositor: &str, value: f64| {
            let mut run = run(
                (compositor, "equal-work", "wlroots-headless"),
                "achieved_frames_per_s",
                value,
                false,
                Some(false),
            );
            run.case.scenario = "game".to_owned();
            run
        };
        let runs = [
            rate("sway", 243.88),
            rate("umbriel", 242.18),
            // At the mode's rate: not wlroots'.
            rate("kwin", 239.98),
        ];
        let paired: Vec<(&CaseRun, Option<String>)> = runs.iter().map(|run| (run, None)).collect();
        assert_eq!(wlroots_game_rates(&paired), Some((242.18, 243.88)));
        assert!(rivals(&paired).contains(
            "fewer as each frame takes time to draw: 242.2 to 243.9 in `game` in these files."
        ));
        let none = rivals(&paired[2..]);
        assert!(
            none.contains("fewer as each frame takes time to draw. The others"),
            "{none}"
        );
    }

    #[test]
    fn a_refusal_is_not_a_failure_in_the_summary() {
        let mut refused = run(("sway", "equal-work", "w"), "core_cpu", 1.0, false, None);
        refused
            .refused
            .push("process 10 (sway) holds /dev/dri/card1 open".to_owned());
        let mut failed = run(("kwin", "equal-work", "k"), "core_cpu", 1.0, false, None);
        failed.error = Some("kwin was not ready".to_owned());
        let file = RunFile {
            format: FORMAT,
            runs: vec![refused, failed],
            ..Default::default()
        };
        let text = summary(&[("run.json".to_owned(), file)]);
        assert!(
            text.contains("| sway | equal-work | w | 1, 1 refused |"),
            "{text}"
        );
        assert!(
            text.contains("| kwin | equal-work | k | 1, 1 failed |"),
            "{text}"
        );
        assert!(text.contains("refused: process 10 (sway) holds /dev/dri/card1 open"));
        // Every rival of ADR-0075 is listed: those run, those not, those on the tour only.
        assert!(
            text.contains("- Measured in these files: sway, kwin."),
            "{text}"
        );
        assert!(
            text.contains("- Not run in these files: dwl, hyprland, mango, umbriel, mutter."),
            "{text}"
        );
        assert!(text.contains("- niri: tour only: "), "{text}");
        assert!(text.contains("- cosmic-comp: tour only: "), "{text}");
        assert!(text.contains("- mutter: `motion` not run: "), "{text}");
        assert!(
            text.contains(
                "(sway, sway-vulkan, dwl, mango, umbriel) draw a frame 4 ms after each commit"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_file_of_another_format_is_refused() {
        assert!(read(r#"{"format": 2}"#).unwrap_err().contains("format 2"));
        assert!(read("{}").is_err());
        assert!(read("not json").is_err());
    }
}
