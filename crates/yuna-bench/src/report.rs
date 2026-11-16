//! The summary of one or more run files: a Markdown table a scenario, in the form of
//! `docs/measurements.md`, each cell the lowest and highest of a case's runs; `≥` marks a lower
//! bound.
//!
//! A cost is ranked only where the difference is larger than the runs' spread: two compositors
//! whose ranges of runs overlap share a rank, and nothing is ranked with fewer than two runs of
//! each. Only *equal work* runs that other programs did not disturb are ranked; the rest are
//! shown, unranked, and a compositor left out so is named. A lower bound is never ranked.
//!
//! In `game` and `pacing`, whose client draws as fast as the compositor takes its frames, a cost
//! a second scales with the backend's pace (`recipes::Pace`): it is ranked only between
//! compositors on the same pace. wlroots' headless backend draws a frame 4 ms after each commit,
//! up to 250 frames a second for a 240 Hz mode, fewer under load; the others draw at the mode's
//! rate, 240 at most. In `rest`, `motion` and `film`, where equal work draws only when something
//! changes, no pace sets the costs: every compositor ranks together. No measured rate decides
//! where a compositor ranks. A figure of the stack is ranked apart for a compositor whose stack
//! holds its parent (Hyprland's dwl). A cost a frame is ranked only between compositors on the
//! same backend whose frames were counted the same way (ADR-0075); frames themselves are never
//! ranked.
//!
//! The summary ends with every rival of ADR-0075: those the files measured, those they did not,
//! and those that run on the owner's tour only, with why.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::recipes;
use crate::record::{CaseRun, FORMAT, RunFile};

/// Reads a run file, refusing another format than this bench's.
pub fn read(text: &str) -> Result<RunFile, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    match value["format"].as_u64() {
        Some(format) if format == u64::from(FORMAT) => {
            serde_json::from_value(value).map_err(|e| e.to_string())
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
    /// Only between compositors on the same backend whose frames were counted the same way.
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

/// Each scenario's columns.
fn columns(scenario: &str) -> Vec<Column> {
    match scenario {
        "rest" => vec![
            cost("Wake-ups a second, core", "core_wakeups"),
            cost("CPU, core (ms/s)", "core_cpu"),
            cost("CPU, stack (ms/s)", "stack_cpu"),
            cost("GPU, core (ms/s)", "core_gpu"),
            frames("Frames a second", "frames_per_s"),
            PSS_CORE,
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
            let after = run.memory.iter().rev().find(|m| m.when == "after")?;
            let sum = if core { &after.core } else { &after.stack };
            Some(Value {
                value: sum.pss.value? / 1024.0,
                lower_bound: false,
            })
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

/// A compositor's name and its runs' values in one column.
type Values = (String, Vec<f64>);

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

/// The ranking of `column` among `scenario`'s `groups`, by compositor and configuration, as the
/// summary writes it: over *equal work* runs not disturbed alone, lower bounds left out, a cost a
/// second of a paced scenario only within one pace, a figure of the stack apart for a compositor
/// with a parent, a cost a frame only within one backend and one way of counting frames.
fn ranking(
    column: &Column,
    scenario: &str,
    groups: &BTreeMap<(String, String), Vec<&CaseRun>>,
) -> String {
    // Each set is ranked apart, named by the pace, the parent or the backend it holds.
    let mut sets: BTreeMap<String, Vec<Values>> = BTreeMap::new();
    let mut bounds = Vec::new();
    let mut all_disturbed = Vec::new();
    for ((compositor, configuration), runs) in groups {
        if configuration != "equal-work" {
            continue;
        }
        let quiet: Vec<&&CaseRun> = runs
            .iter()
            .filter(|run| run.disturbed == Some(false))
            .collect();
        let values: Vec<Value> = quiet.iter().filter_map(|run| value(run, column)).collect();
        if values.is_empty() {
            if quiet.is_empty() && runs.iter().any(|run| value(run, column).is_some()) {
                all_disturbed.push(compositor.clone());
            }
            continue;
        }
        if values.iter().any(|value| value.lower_bound) {
            bounds.push(compositor.clone());
            continue;
        }
        let values: Vec<f64> = values.iter().map(|v| v.value).collect();
        let set = if of_stack(column) && has_parent(runs) {
            "with its parent in its stack".to_owned()
        } else {
            match column.rank {
                Rank::PerSecond if PACED.contains(&scenario) => recipes::recipe(compositor)
                    .map_or("on a pace not known", |recipe| recipe.pace.describe())
                    .to_owned(),
                Rank::PerFrame => format!(
                    "on {}, frames counted by {}",
                    runs.iter()
                        .find_map(|run| run.compositor.as_ref())
                        .map_or("?", |info| info.backend.as_str()),
                    counted_by(runs).unwrap_or_else(|| "?".to_owned())
                ),
                Rank::PerSecond | Rank::Total | Rank::No => String::new(),
            }
        };
        sets.entry(set)
            .or_default()
            .push((compositor.clone(), values));
    }
    let line = |groups: &[Values]| match ranks(groups) {
        Ok(tiers) => tiers
            .iter()
            .map(|tier| tier.join(" = "))
            .collect::<Vec<_>>()
            .join(" < "),
        Err(reason) => format!("no ranking: {reason}"),
    };
    // The set of no pace, parent or backend of its own, beside the others.
    let named = |name: &str| {
        if name.is_empty() {
            "the others".to_owned()
        } else {
            name.to_owned()
        }
    };
    let mut text = if column.rank == Rank::PerFrame
        && !sets.is_empty()
        && sets.values().all(|set| set.len() < 2)
    {
        "no ranking: no two compositors share a backend and a way of counting frames".to_owned()
    } else if sets.len() > 1 {
        let mut parts: Vec<String> = sets
            .iter()
            .filter(|(_, set)| set.len() >= 2)
            .map(|(name, set)| format!("{}: {}", named(name), line(set)))
            .collect();
        let alone: Vec<String> = sets
            .iter()
            .filter(|(_, set)| set.len() < 2)
            .map(|(name, set)| format!("{} ({})", set[0].0, named(name)))
            .collect();
        if !alone.is_empty() {
            parts.push(format!("alone: {}", alone.join(", ")));
        }
        parts.join("; ")
    } else {
        match sets.iter().next() {
            Some((name, set)) if name.is_empty() => line(set),
            Some((name, set)) => format!("{name}: {}", line(set)),
            None => "no ranking: no quiet equal-work run".to_owned(),
        }
    };
    if !bounds.is_empty() {
        let _ = write!(text, "; unranked, lower bounds: {}", bounds.join(", "));
    }
    if !all_disturbed.is_empty() {
        let _ = write!(
            text,
            "; left out, every equal-work run disturbed: {}",
            all_disturbed.join(", ")
        );
    }
    text
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

/// The summary of `files`, each its name and its run.
pub fn summary(files: &[(String, RunFile)]) -> String {
    let mut out = String::from("# The rivals' bench\n\n");
    for (name, file) in files {
        let machine = &file.machine;
        let gpu = machine.gpu.as_ref();
        let first_state = file
            .runs
            .iter()
            .flat_map(|run| &run.phases)
            .flat_map(|phase| &phase.load.seconds)
            .next()
            .map(|second| &second.state);
        let _ = writeln!(
            out,
            "`{name}`: started {} UTC, commit `{}`{}, seed {}.",
            file.started_utc,
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
            first_state
                .and_then(|s| s.performance_level.as_deref())
                .unwrap_or("?"),
            file.exclusive_lock,
            match file.seconds_override {
                Some(seconds) => format!(
                    " Every window lasted {seconds} s, not the scenarios' own: not comparable."
                ),
                None => String::new(),
            }
        );
        out.push('\n');
    }
    let runs: Vec<&CaseRun> = files.iter().flat_map(|(_, file)| &file.runs).collect();
    let scenarios: BTreeSet<(String, Option<u32>)> = runs
        .iter()
        .map(|run| (run.case.scenario.clone(), run.case.windows))
        .collect();
    for (scenario, windows) in scenarios {
        let title = match windows {
            Some(windows) => format!("{scenario}, {windows} windows"),
            None => scenario.clone(),
        };
        let _ = writeln!(out, "## {title}\n");
        let mut groups: BTreeMap<(String, String), Vec<&CaseRun>> = BTreeMap::new();
        for run in runs
            .iter()
            .filter(|run| run.case.scenario == scenario && run.case.windows == windows)
        {
            groups
                .entry((run.case.compositor.clone(), run.case.configuration.clone()))
                .or_default()
                .push(run);
        }
        let columns = columns(&scenario);
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
        for ((compositor, configuration), runs) in &groups {
            let info = runs.iter().find_map(|run| run.compositor.as_ref());
            let disturbed = runs.iter().any(|run| run.disturbed == Some(true));
            let failed = runs.iter().filter(|run| run.error.is_some()).count();
            let refused = runs.iter().filter(|run| !run.refused.is_empty()).count();
            let name = match info.and_then(|info| info.version.as_deref()) {
                Some(version) => format!("{compositor} {}", short_version(version)),
                None => compositor.clone(),
            };
            let backend = info
                .map(|info| info.backend.as_str())
                .or_else(|| recipes::recipe(compositor).map(|recipe| recipe.backend));
            let _ = write!(
                out,
                "| {name}{} | {configuration} | {} | {}{}{} |",
                if disturbed { " †" } else { "" },
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
                        "{compositor}'s stack holds its parent, and its figures of the stack \
                         rank apart: {parent}"
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
                ranking(column, &scenario, &groups)
            );
        }
        let _ = writeln!(
            out,
            "- {}",
            if PACED.contains(&scenario.as_str()) {
                "Its client draws as fast as the compositor takes its frames: a cost a second \
                 ranks only between compositors on the same pace, wlroots' headless timer or \
                 the mode's rate."
            } else {
                "At equal work it draws only when something changes, on no pace: a cost a \
                 second ranks across every compositor."
            }
        );
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
    }
    out.push_str(&rivals(&runs));
    out.push_str(
        "† Other programs disturbed a run: more than 2 cores or 10 % of the GPU's drawing on \
         average.\n",
    );
    out
}

/// The summary's closing section: every rival of ADR-0075, measured in `runs` or not, those that
/// run on the tour only and why, the scenarios a recipe cannot run, and why the rivals on
/// wlroots draw at 250 Hz.
fn rivals(runs: &[&CaseRun]) -> String {
    let ran: BTreeSet<&str> = runs
        .iter()
        .map(|run| run.case.compositor.as_str())
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
        .filter(|recipe| recipe.backend == "wlroots-headless")
        .map(|recipe| recipe.name)
        .collect();
    let _ = writeln!(
        out,
        "- The rivals on wlroots' headless backend ({}) draw a frame 4 ms after each commit: \
         it counts whole milliseconds, 1000000 / 240000 = 4 (wlroots 0.20.2, \
         backend/headless/output.c:31 and :76). That is up to {} frames a second for a 240 Hz \
         mode, fewer as each frame takes time to draw: 241.7 to 243.9 in `game` on 2026-11-16. \
         The others draw at the mode's rate, 240 frames a second at most. In `game` and \
         `pacing`, costs a second rank within one of these paces.\n",
        wlroots.join(", "),
        number(recipes::WLROOTS_HEADLESS_HZ)
    );
    out
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
        let exact = |value| Value {
            value,
            lower_bound: false,
        };
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
    /// `value`.
    fn run(
        (compositor, configuration, backend): (&str, &str, &str),
        key: &str,
        value: f64,
        lower_bound: bool,
        disturbed: Option<bool>,
    ) -> CaseRun {
        let mut phase = crate::record::Phase {
            name: "cost".to_owned(),
            frames_counted_by: Some("mpv".to_owned()),
            ..Default::default()
        };
        phase.figures.insert(
            key.to_owned(),
            crate::record::Figure::new(Some(value), "us"),
        );
        if lower_bound {
            phase.lower_bounds.push(key.to_owned());
        }
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

    fn grouped(runs: &[CaseRun]) -> BTreeMap<(String, String), Vec<&CaseRun>> {
        let mut groups: BTreeMap<(String, String), Vec<&CaseRun>> = BTreeMap::new();
        for run in runs {
            groups
                .entry((run.case.compositor.clone(), run.case.configuration.clone()))
                .or_default()
                .push(run);
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
        let runs = [
            run(
                ("sway", "equal-work", "w"),
                "core_wakeups",
                1.0,
                false,
                quiet,
            ),
            run(
                ("sway", "equal-work", "w"),
                "core_wakeups",
                1.1,
                false,
                quiet,
            ),
            run(
                ("yuna", "equal-work", "y"),
                "core_wakeups",
                0.0,
                false,
                quiet,
            ),
            run(
                ("yuna", "equal-work", "y"),
                "core_wakeups",
                0.0,
                false,
                quiet,
            ),
            run(
                ("kwin", "equal-work", "k"),
                "core_wakeups",
                0.1,
                true,
                quiet,
            ),
            run(
                ("kwin", "equal-work", "k"),
                "core_wakeups",
                0.1,
                true,
                quiet,
            ),
        ];
        let column = cost("Wake-ups", "core_wakeups");
        assert_eq!(
            ranking(&column, "rest", &grouped(&runs)),
            "yuna < sway; unranked, lower bounds: kwin"
        );
    }

    #[test]
    fn costs_a_second_rank_within_one_pace_where_the_client_paces_the_compositor() {
        let runs: Vec<CaseRun> = [
            ("kwin", [3.0, 3.1]),
            ("yuna", [2.0, 2.1]),
            ("hyprland", [4.0, 4.1]),
            // On wlroots' headless timer, whatever the rate each reached: Umbriel's 241.7 too.
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
    fn costs_a_frame_rank_only_within_a_backend_and_a_way_of_counting() {
        let quiet = Some(false);
        let mut runs = vec![
            run(
                ("sway", "equal-work", "wlroots"),
                "core_cpu_per_frame",
                150.0,
                false,
                quiet,
            ),
            run(
                ("sway", "equal-work", "wlroots"),
                "core_cpu_per_frame",
                151.0,
                false,
                quiet,
            ),
            run(
                ("kwin", "equal-work", "kwin"),
                "core_cpu_per_frame",
                300.0,
                false,
                quiet,
            ),
            run(
                ("kwin", "equal-work", "kwin"),
                "core_cpu_per_frame",
                301.0,
                false,
                quiet,
            ),
        ];
        let column = per_frame("CPU a frame", "core_cpu_per_frame");
        assert_eq!(
            ranking(&column, "game", &grouped(&runs)),
            "no ranking: no two compositors share a backend and a way of counting frames"
        );
        for value in [400.0, 401.0] {
            runs.push(run(
                ("sway-vulkan", "equal-work", "wlroots"),
                "core_cpu_per_frame",
                value,
                false,
                quiet,
            ));
        }
        assert_eq!(
            ranking(&column, "game", &grouped(&runs)),
            "on wlroots, frames counted by mpv: sway < sway-vulkan; alone: kwin (on kwin, frames \
             counted by mpv)"
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
