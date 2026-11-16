//! The run's file: one JSON file per run of the bench, versioned by [`FORMAT`].
//!
//! Every figure is a [`Figure`], a value and its unit, the value `null` when it could not be
//! measured, never 0. In the readings kept raw (the machine's seconds, the raw `fdinfo`), the
//! unit is the end of the field's name: `_ns`, `_kib`, `_mhz`, `_w`. A reader of a later format
//! refuses this one rather than guess.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cgroup::CpuStat;
use crate::machine::{Load, MachineInfo};
use crate::sampler::Window;

/// The format of the run's file; raised when a field changes its meaning.
pub const FORMAT: u32 = 1;

/// Below this share of a group's CPU time in the threads sampled, its wake-ups are a lower bound.
pub const COVERAGE: f64 = 0.9;

/// A measured value and its unit; the value is `None` when it could not be measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Figure {
    /// The value.
    pub value: Option<f64>,
    /// Its unit: `ns`, `us`, `ms`, `s`, `KiB`, `count`, `per_s`, `cores`, `percent`, …
    pub unit: String,
}

impl Figure {
    /// A figure of `value` in `unit`.
    pub fn new(value: Option<f64>, unit: &str) -> Figure {
        Figure {
            value,
            unit: unit.to_owned(),
        }
    }

    /// A count.
    pub fn count(value: u64) -> Figure {
        Figure::new(Some(value as f64), "count")
    }
}

/// Named figures.
pub type Figures = BTreeMap<String, Figure>;

/// One run of the bench: what it ran on, in which order, and each case's figures.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunFile {
    /// [`FORMAT`].
    pub format: u32,
    /// The bench's own commit, and whether its tree differed from it.
    pub commit: Option<String>,
    /// Whether the tree had changes beside its commit.
    pub dirty: Option<bool>,
    /// When it started, in UTC.
    pub started_utc: String,
    /// The bench's arguments.
    pub arguments: Vec<String>,
    /// The seed of the cases' random order.
    pub seed: u64,
    /// The measured windows' length, when `--seconds` changed it: then the figures are not
    /// comparable to the scenarios' own.
    pub seconds_override: Option<f64>,
    /// How the agents' exclusive lock was held.
    pub exclusive_lock: String,
    /// The machine.
    pub machine: MachineInfo,
    /// What the bench could not prepare, and facts a reader needs beside the figures.
    pub notes: Vec<String>,
    /// The cases' labels, in the order they ran.
    pub order: Vec<String>,
    /// Each case's run, in that order.
    pub runs: Vec<CaseRun>,
}

/// A case: one compositor, one of its configurations, one scenario.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Case {
    /// The recipe's name: `sway`, `kwin`, `dwl`, … (`recipes::RECIPES`).
    pub compositor: String,
    /// `as-shipped` or `equal-work`.
    pub configuration: String,
    /// `rest`, `game`, `film`, `pacing` or `motion`.
    pub scenario: String,
    /// The still windows, for `rest`.
    pub windows: Option<u32>,
}

impl Case {
    /// Its label, such as `sway/equal-work/rest-15`.
    pub fn label(&self) -> String {
        let scenario = match self.windows {
            Some(windows) => format!("{}-{windows}", self.scenario),
            None => self.scenario.clone(),
        };
        format!("{}/{}/{scenario}", self.compositor, self.configuration)
    }
}

/// What ran as the compositor.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompositorInfo {
    /// Its name.
    pub name: String,
    /// Its version, from its store path, or Yuna's commit.
    pub version: Option<String>,
    /// Its program.
    pub program: String,
    /// The backend it ran on: frames are compared only on the same one (ADR-0075).
    pub backend: String,
    /// What it drew with.
    pub renderer: String,
    /// The settings of its configuration that differ between `as-shipped` and `equal-work`.
    pub settings: BTreeMap<String, String>,
    /// The output asked for.
    pub output_asked: String,
    /// The output it reported, when it reports one.
    pub output: Option<String>,
    /// How its frames are counted.
    pub frames_counted_by: String,
    /// The Mesa store folders its processes mapped: what it really drew with.
    pub mesa_mapped: Vec<String>,
}

/// One case's run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CaseRun {
    /// The case.
    pub case: Case,
    /// Which of its runs: 1, 2, 3.
    pub repeat: u32,
    /// The compositor.
    pub compositor: Option<CompositorInfo>,
    /// From its start to its being ready.
    pub ready_after: Figure,
    /// Each measured window.
    pub phases: Vec<Phase>,
    /// The memory, read between windows.
    pub memory: Vec<Memory>,
    /// What the sandbox's checks found: the case was then stopped and not measured.
    pub refused: Vec<String>,
    /// What went wrong, if anything did.
    pub error: Option<String>,
    /// How the compositor ended.
    pub stopped: Option<String>,
    /// Whether other programs disturbed any of its windows; none if it could not be told.
    pub disturbed: Option<bool>,
    /// Facts a reader needs beside the figures.
    pub notes: Vec<String>,
}

/// One measured window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Phase {
    /// What it measured: `cost`, the compositor's costs; `frames`, its frames counted apart.
    pub name: String,
    /// Its length.
    pub seconds: Figure,
    /// Its figures, derived from what follows: per second, per frame.
    pub figures: Figures,
    /// The compositor's own process and its descendants.
    pub core: Usage,
    /// Every process of its sandbox: the compositor, its session bus and its helpers.
    pub stack: Usage,
    /// Each of the bench's clients, apart.
    pub clients: Vec<ClientUsage>,
    /// The machine meanwhile.
    pub load: Load,
    /// How the window's frames were counted: by the compositor's own means or the client's,
    /// in this window. The figures a frame exist only when the count is exact.
    pub frames_counted_by: Option<String>,
    /// The figures of `figures` that are lower bounds, not exact: never ranked as exact.
    pub lower_bounds: Vec<String>,
    /// The `fdinfo` of the compositor's DRM files at the window's start and end, as the kernel
    /// wrote them.
    pub fdinfo_raw: Vec<RawFdinfo>,
}

/// One DRM file's `fdinfo`, raw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawFdinfo {
    /// `start` or `end`.
    pub when: String,
    /// The process.
    pub pid: u32,
    /// The descriptor.
    pub fd: u32,
    /// The node.
    pub node: String,
    /// The text.
    pub text: String,
}

/// What a set of processes used over a window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// The CPU time of every process that ran in the group, from its `cpu.stat`, those that
    /// ended included: exact.
    pub cpu: Figure,
    /// Of it, in user space.
    pub cpu_user: Figure,
    /// Of it, in the kernel.
    pub cpu_system: Figure,
    /// The CPU time of the threads sampled: a thread that ended counts up to its last reading.
    pub threads_cpu: Figure,
    /// The share of the group's exact CPU time the threads sampled account for, from 0 to 1;
    /// none without CPU time.
    pub threads_coverage: Option<f64>,
    /// Voluntary context switches, every thread together: its wake-ups. Those of processes
    /// that lived between two readings are missed: see `wakeups_lower_bound`.
    pub wakeups: Figure,
    /// Whether `wakeups` and `preemptions` are lower bounds: the threads sampled account for
    /// less than [`COVERAGE`] of the group's CPU time, so processes escaped the readings, or
    /// that share is unknown, its `cpu.stat` unread or nil.
    pub wakeups_lower_bound: bool,
    /// Involuntary context switches: its preemptions.
    pub preemptions: Figure,
    /// By process and kind of thread.
    pub threads: Vec<ThreadUsage>,
    /// Each GPU engine's busy time, every DRM client counted once.
    pub gpu: BTreeMap<String, Figure>,
    /// The DRM clients seen.
    pub drm_clients: usize,
    /// The readings the window took.
    pub readings: usize,
}

/// One kind of thread of one process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadUsage {
    /// The process's name.
    pub process: String,
    /// The thread's name, its number left out: `llvmpipe-0` to `llvmpipe-15` count as one.
    pub thread: String,
    /// How many such threads.
    pub threads: usize,
    /// How many of them ended before the window's end.
    pub ended: usize,
    /// Their CPU time.
    pub cpu: Figure,
    /// Their wake-ups.
    pub wakeups: Figure,
    /// Their preemptions.
    pub preemptions: Figure,
}

/// One client's usage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientUsage {
    /// The client.
    pub name: String,
    /// What it used.
    pub usage: Usage,
}

/// The memory of the sandbox's processes at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    /// When: `ready`, before any client; `after`, at the end of the scenario.
    pub when: String,
    /// The compositor and its descendants together.
    pub core: MemorySum,
    /// Every process of the sandbox together.
    pub stack: MemorySum,
    /// Each process.
    pub processes: Vec<ProcessMemory>,
}

/// Memory summed over processes, from `smaps_rollup` and the DRM statistics.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemorySum {
    /// The proportional set size, its three parts together.
    pub pss: Figure,
    /// Of it, anonymous memory.
    pub pss_anon: Figure,
    /// Of it, files mapped.
    pub pss_file: Figure,
    /// Of it, shared memory.
    pub pss_shmem: Figure,
    /// The resident set, shared pages counted in each process.
    pub rss: Figure,
    /// Each DRM memory key, each client counted once: `memory-vram`, `resident-gtt`, …
    pub drm: BTreeMap<String, Figure>,
}

impl Default for Figure {
    fn default() -> Figure {
        Figure::new(None, "")
    }
}

/// One process's memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessMemory {
    /// Its id.
    pub pid: u32,
    /// Its name.
    pub name: String,
    /// Whether it is the compositor or one of its descendants.
    pub core: bool,
    /// Its memory.
    pub memory: MemorySum,
}

/// The usage of a group over a window: its `cpu.stat` at both ends, and its window.
pub fn usage(before: Option<CpuStat>, after: Option<CpuStat>, window: &Window) -> Usage {
    let microseconds = |pick: fn(&CpuStat) -> u64| {
        let value = match (before, after) {
            (Some(b), Some(a)) => Some(pick(&a).saturating_sub(pick(&b)) as f64 * 1000.0),
            _ => None,
        };
        Figure::new(value, "ns")
    };
    let mut kinds: BTreeMap<(String, String), ThreadUsage> = BTreeMap::new();
    for thread in window.threads() {
        let kind = thread_kind(&thread.counts.name).to_owned();
        let entry = kinds
            .entry((thread.counts.process.clone(), kind.clone()))
            .or_insert_with(|| ThreadUsage {
                process: thread.counts.process.clone(),
                thread: kind,
                threads: 0,
                ended: 0,
                cpu: Figure::new(Some(0.0), "ns"),
                wakeups: Figure::count(0),
                preemptions: Figure::count(0),
            });
        entry.threads += 1;
        entry.ended += usize::from(thread.ended);
        add(&mut entry.cpu, thread.counts.cpu_ns);
        add(&mut entry.wakeups, thread.counts.voluntary);
        add(&mut entry.preemptions, thread.counts.involuntary);
    }
    let total = window.total();
    // No thread read at all: the counts are unknown, not nil.
    let seen = !window.threads().is_empty();
    let counted = |value: u64| Figure::new(seen.then_some(value as f64), "count");
    let cpu = microseconds(|stat| stat.usage_us);
    let threads_coverage = cpu
        .value
        .filter(|cpu| *cpu > 0.0)
        .map(|cpu| if seen { total.cpu_ns as f64 / cpu } else { 0.0 });
    Usage {
        cpu,
        cpu_user: microseconds(|stat| stat.user_us),
        cpu_system: microseconds(|stat| stat.system_us),
        threads_cpu: Figure::new(seen.then_some(total.cpu_ns as f64), "ns"),
        threads_coverage,
        wakeups: counted(total.voluntary),
        // Exact only where the threads are known to account for the group's time.
        wakeups_lower_bound: threads_coverage.is_none_or(|share| share < COVERAGE),
        preemptions: counted(total.involuntary),
        threads: kinds.into_values().collect(),
        gpu: window
            .engines()
            .into_iter()
            .map(|(engine, ns)| (engine, Figure::new(Some(ns as f64), "ns")))
            .collect(),
        drm_clients: window.drm_clients(),
        readings: window.readings(),
    }
}

fn add(figure: &mut Figure, more: u64) {
    figure.value = Some(figure.value.unwrap_or(0.0) + more as f64);
}

/// A thread's name without its number, so that `llvmpipe-0` to `llvmpipe-15` count as one.
pub fn thread_kind(name: &str) -> &str {
    let kind = name
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches(['-', ':', '_']);
    if kind.is_empty() { name } else { kind }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler::{Sample, ThreadCounts, ThreadId};

    #[test]
    fn a_case_is_labelled_by_its_parts() {
        let case = Case {
            compositor: "sway".to_owned(),
            configuration: "equal-work".to_owned(),
            scenario: "rest".to_owned(),
            windows: Some(15),
        };
        assert_eq!(case.label(), "sway/equal-work/rest-15");
    }

    #[test]
    fn usage_groups_threads_by_kind_and_keeps_the_groups_exact_time() {
        let counts = |name: &str, cpu_ns: u64, voluntary: u64| ThreadCounts {
            process: "sway".to_owned(),
            name: name.to_owned(),
            cpu_ns,
            voluntary,
            involuntary: 1,
        };
        let mut first = Sample::default();
        first
            .threads
            .insert(ThreadId { pid: 1, tid: 1 }, counts("sway", 0, 0));
        first
            .threads
            .insert(ThreadId { pid: 1, tid: 2 }, counts("sway:gdrv0", 0, 0));
        let mut window = Window::start(first);
        let mut second = Sample::default();
        second
            .threads
            .insert(ThreadId { pid: 1, tid: 1 }, counts("sway", 2_000, 4));
        second
            .threads
            .insert(ThreadId { pid: 1, tid: 3 }, counts("sway:gdrv1", 500, 2));
        window.observe(second);
        let before = CpuStat {
            usage_us: 10,
            user_us: 6,
            system_us: 4,
        };
        let after = CpuStat {
            usage_us: 13,
            user_us: 8,
            system_us: 5,
        };
        let usage = usage(Some(before), Some(after), &window);
        assert_eq!(usage.cpu, Figure::new(Some(3_000.0), "ns"));
        assert_eq!(usage.wakeups, Figure::count(6));
        let gdrv = usage
            .threads
            .iter()
            .find(|t| t.thread == "sway:gdrv")
            .unwrap();
        assert_eq!((gdrv.threads, gdrv.ended), (2, 1));
        assert_eq!(gdrv.cpu.value, Some(500.0));
        assert_eq!(super::usage(None, Some(after), &window).cpu.value, None);
        // The threads account for 2,500 of the group's 3,000 ns: 83 %, the wake-ups a lower
        // bound.
        assert!(usage.wakeups_lower_bound, "{:?}", usage.threads_coverage);
        // 2,500 ns of threads against the group's 2,000: the readings' ends differ by a little,
        // and every thread was seen. Exact.
        let covered = CpuStat {
            usage_us: 12,
            ..before
        };
        let exact = super::usage(Some(before), Some(covered), &window);
        assert!(!exact.wakeups_lower_bound, "{:?}", exact.threads_coverage);
        // Without the group's time, or with none, the share is unknown: not exact.
        assert!(super::usage(None, Some(after), &window).wakeups_lower_bound);
        assert!(super::usage(Some(before), Some(before), &window).wakeups_lower_bound);
        // A window that saw no thread knows no wake-up.
        let empty = Window::start(Sample::default());
        let unseen = super::usage(Some(before), Some(after), &empty);
        assert_eq!(unseen.wakeups.value, None);
        assert_eq!(unseen.threads_cpu.value, None);
        assert!(unseen.wakeups_lower_bound);
        assert_eq!(thread_kind("llvmpipe-12"), "llvmpipe");
        assert_eq!(thread_kind("1234"), "1234");
    }
}
