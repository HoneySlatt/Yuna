//! What the machine's *other* programs used over a phase of a measure, read every second with the
//! readers of `yuna_bench::machine`
//! ([ADR-0060](../../../../docs/adr/0060-development-first-agents-in-their-own-yuna.md)): their
//! processor time, the machine's busy time less the measure's and its descendants', every Yuna and
//! client it started among them, those reaped included; and their GPU time on the engines Yuna
//! draws on, by the DRM statistics of every process this user may read but those. A sandbox that
//! hides other processes' files leaves it unknown, and says so. A phase is marked disturbed by
//! that module's rule (`OTHERS_CPU_LIMIT`, `OTHERS_GPU_LIMIT`); their peak over one reading, the
//! measure's own processor time and the busy percentage of the GPU behind `/dev/dri/renderD128`
//! are given beside it. `xwayland_time` and `instances` include this file beside `common`.

use yuna_bench::machine::{self, MachineState, MachineWatch};
use yuna_bench::sandbox::RENDER_NODE;

/// What the machine's other programs use over a phase: the bench's [`MachineWatch`], and the
/// GPU's busy percentage before the phase, which it keeps only from the first second on.
pub struct LoadWatch {
    watch: MachineWatch,
    gpu_busy_before: Option<u32>,
}

impl LoadWatch {
    pub fn start() -> LoadWatch {
        LoadWatch {
            gpu_busy_before: MachineState::read(RENDER_NODE).gpu_busy_percent,
            watch: MachineWatch::start(RENDER_NODE),
        }
    }

    pub fn finish(self) -> Load {
        let others = self.watch.finish();
        let busy_after = others
            .seconds
            .last()
            .and_then(|second| second.state.gpu_busy_percent);
        let busy_at_most = others
            .seconds
            .iter()
            .filter_map(|second| second.state.gpu_busy_percent)
            .chain(self.gpu_busy_before)
            .max();
        Load {
            others,
            gpu_busy: (self.gpu_busy_before, busy_after, busy_at_most),
        }
    }
}

/// What the other programs used over a phase, as `yuna_bench::machine` reads it, and the busy
/// percentage of the GPU behind `/dev/dri/renderD128`, at the phase's start, its end, and at most.
pub struct Load {
    others: machine::Load,
    gpu_busy: (Option<u32>, Option<u32>, Option<u32>),
}

impl Load {
    /// What the other programs' GPU time was, or why it is not known: no process's GPU files
    /// could be read, which is not the same as nil.
    fn others_gpu(&self) -> String {
        match (
            self.others.others_gpu_percent_mean,
            self.others.others_gpu_percent_max,
        ) {
            (Some(mean), Some(max)) => format!(
                "{mean:.1} % of the GPU's drawing on average, {max:.1} % at most, by {} processes",
                self.others.others_gpu_processes
            ),
            _ => "no other process's GPU time could be read".to_owned(),
        }
    }
}

impl std::fmt::Display for Load {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cores = |cores: Option<f64>| cores.map_or("?".to_owned(), |c| format!("{c:.2}"));
        let percent = |busy: Option<u32>| busy.map_or("?".to_owned(), |b| b.to_string());
        write!(
            f,
            "other programs: {} cores on average, {} at most, and {}; this measure: {:.2} \
             cores; the GPU {} % busy before, {} % after, {} % at most: {}",
            cores(self.others.others_cpu_cores_mean),
            cores(self.others.others_cpu_cores_max),
            self.others_gpu(),
            self.others.bench_cpu_cores_mean,
            percent(self.gpu_busy.0),
            percent(self.gpu_busy.1),
            percent(self.gpu_busy.2),
            match self.others.disturbed {
                Some(true) => "disturbed",
                Some(false) => "quiet",
                None => "unknown",
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_load_says_what_the_other_programs_used() {
        let others = machine::Load {
            others_cpu_cores_mean: Some(1.0),
            others_cpu_cores_max: Some(1.5),
            others_gpu_percent_mean: Some(5.0),
            others_gpu_percent_max: Some(10.0),
            others_gpu_processes: 1,
            bench_cpu_cores_mean: 1.5,
            disturbed: Some(false),
            ..machine::Load::default()
        };
        let load = Load {
            others,
            gpu_busy: (Some(5), Some(6), Some(7)),
        };
        let text = load.to_string();
        assert!(
            text.contains("1.00 cores on average, 1.50 at most"),
            "{text}"
        );
        assert!(
            text.contains("5.0 % of the GPU's drawing on average, 10.0 % at most, by 1 processes"),
            "{text}"
        );
        assert!(text.contains("this measure: 1.50 cores"), "{text}");
        assert!(
            text.contains("the GPU 5 % busy before, 6 % after, 7 % at most: quiet"),
            "{text}"
        );
    }

    #[test]
    fn a_load_says_when_nothing_could_be_read() {
        // No other process's GPU files read: unknown, not nil; and a verdict of none is unknown.
        let load = Load {
            others: machine::Load::default(),
            gpu_busy: (None, None, None),
        };
        let text = load.to_string();
        assert!(
            text.contains("no other process's GPU time could be read"),
            "{text}"
        );
        assert!(
            text.contains("the GPU ? % busy before, ? % after, ? % at most: unknown"),
            "{text}"
        );
        let disturbed = Load {
            others: machine::Load {
                disturbed: Some(true),
                ..machine::Load::default()
            },
            gpu_busy: (None, None, None),
        };
        assert!(disturbed.to_string().ends_with(": disturbed"));
    }
}
