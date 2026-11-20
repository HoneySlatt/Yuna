//! One measured run of the scene (`scene_definition.rs`), on either side: a row of the table that
//! compares them, its details in Markdown, and its JSON. Shared with `prototypes/llvmpipe-scene`,
//! so that both sides sum up their figures with the same code.

use crate::json::Json;
use crate::load::{self, LoadWindow, Machine};
use crate::scene_definition::describe_scene;
use crate::stats::{Summary, summary_cell, summary_json};
use crate::threads::Usage;

/// The version of a run's JSON file, raised when its fields change meaning.
// 2: `disturbed` is the scene's verdict, on average or in the busiest second
// (`LoadWindow::disturbed_in_a_second`); `pacing`, `ms_a_frame`, `steps` and `busiest_second`.
const RUN_FORMAT: i64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Damage {
    /// Each frame, both windows commit a buffer with full damage.
    Full,
    /// Each frame, 64 × 64 texels of window 0 change and are damaged, alone.
    Partial,
}

impl Damage {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "full" => Ok(Self::Full),
            "partial" => Ok(Self::Partial),
            other => Err(format!("`--damage` takes full or partial, not `{other}`")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Partial => "partial",
        }
    }
}

/// What one side measured over its window.
pub struct SceneRun {
    /// "Yuna, llvmpipe" or "prototype, lavapipe".
    pub side: &'static str,
    pub damage: Damage,
    /// `LP_NUM_THREADS` as the run had it; none for Mesa's default.
    pub rasterizer_threads: Option<String>,
    /// Each measured frame's wall time, in microseconds.
    pub wall_us: Vec<f64>,
    /// The CPU time of the drawing process's threads over the window.
    pub cpu: Usage,
    pub load: LoadWindow,
    pub machine: Machine,
    /// The output as the run left it, against the scene's definition.
    pub image: String,
    /// What starts the side's frames: a rate, or Yuna's frame loop at each frame callback.
    pub pacing: String,
    /// How many of the output's periods lie between two measured frames, and how often: from
    /// the boundaries Yuna's frames aimed at, or the prototype's frame starts.
    pub steps: Vec<(u64, usize)>,
    /// What the side adds: its driver, its commit, its upload path, what its frames held.
    pub facts: Vec<(&'static str, String)>,
}

impl SceneRun {
    /// The table's header, which `table_row` fills.
    pub fn table_header() -> &'static str {
        "| Side | Damage | Rasterizer threads | Pacing | ms a frame | Steps (periods × frames) | \
         Wall: median / p99 / longest (µs) | CPU a frame (µs) | Frames | Others' CPUs, GPU %: \
         average (busiest second) | Load |\n|---|---|---|---|---|---|---|---|---|---|---|\n"
    }

    pub fn table_row(&self) -> String {
        let others =
            |value: Option<f64>| value.map_or_else(|| "-".to_owned(), |v| format!("{v:.2}"));
        let (busiest_cores, busiest_gpu) = self.load.busiest_second();
        format!(
            "| {} | {} | {} | {} | {:.2} | {} | {} | {:.0} | {} | {} ({}), {} ({}) | {} |\n",
            self.side,
            self.damage.name(),
            self.threads_name(),
            self.pacing,
            self.ms_a_frame(),
            steps_text(&self.steps),
            summary_cell(Summary::of(&self.wall_us)),
            self.cpu_us_a_frame(),
            self.wall_us.len(),
            others(self.load.others_cores),
            others(busiest_cores),
            others(self.load.others_gpu_percent),
            others(busiest_gpu),
            if self.disturbed() {
                "disturbed"
            } else {
                "quiet"
            },
        )
    }

    /// The scene's verdict: others over a limit on average, or in the busiest second.
    pub fn disturbed(&self) -> bool {
        self.load.disturbed_in_a_second()
    }

    /// The cadence: the measured window's length over its frames, in milliseconds.
    fn ms_a_frame(&self) -> f64 {
        self.load.seconds * 1000.0 / self.wall_us.len().max(1) as f64
    }

    fn threads_name(&self) -> String {
        self.rasterizer_threads
            .as_ref()
            .map_or_else(|| "Mesa's default".to_owned(), |threads| threads.clone())
    }

    fn cpu_us_a_frame(&self) -> f64 {
        self.cpu.total_ns as f64 / 1000.0 / self.wall_us.len().max(1) as f64
    }

    pub fn to_markdown(&self) -> String {
        let mut out = format!(
            "# The scene on {}, {} damage\n\n",
            self.side,
            self.damage.name()
        );
        for (name, value) in &self.facts {
            out.push_str(&format!("- {name}: {value}\n"));
        }
        out.push_str(&format!(
            "- Rasterizer threads (`LP_NUM_THREADS`): {}\n\n",
            self.threads_name()
        ));
        out.push_str(&describe_scene());
        out.push('\n');
        out.push_str(Self::table_header());
        out.push_str(&self.table_row());
        out.push_str(&format!(
            "\nQuiet: others used at most 2 logical CPUs and 10 % of the GPU, on average and in \
             the busiest second, beyond the counters' rounding. ms a frame: the measured \
             window's length over its frames. Steps: the output's 240 Hz periods between two \
             frames.\n\nCPU a frame, by thread: {}.\n\nThe output at the end: {}.\n\n",
            self.cpu.to_text(self.wall_us.len()),
            self.image
        ));
        out.push_str("## The machine's load\n\n");
        load::to_markdown(std::slice::from_ref(&self.load), &mut out);
        out.push_str("## The machine\n\n");
        self.machine.to_markdown(&mut out);
        out
    }

    pub fn to_json(&self) -> Json {
        Json::Object(vec![
            ("format", Json::Integer(RUN_FORMAT)),
            ("side", Json::text(self.side)),
            ("damage", Json::text(self.damage.name())),
            (
                "lp_num_threads",
                Json::optional(self.rasterizer_threads.as_ref(), Json::text),
            ),
            ("frames", Json::Integer(self.wall_us.len() as i64)),
            ("wall_us", summary_json(Summary::of(&self.wall_us))),
            ("cpu", self.cpu.to_json(self.wall_us.len())),
            ("disturbed", Json::Bool(self.disturbed())),
            ("disturbed_on_average", Json::Bool(self.load.disturbed)),
            (
                "busiest_second",
                Json::Object(vec![
                    (
                        "others_logical_cpus",
                        Json::optional(self.load.busiest_second().0, Json::Number),
                    ),
                    (
                        "others_gpu_percent",
                        Json::optional(self.load.busiest_second().1, Json::Number),
                    ),
                ]),
            ),
            ("pacing", Json::text(&self.pacing)),
            ("ms_a_frame", Json::Number(self.ms_a_frame())),
            (
                "steps",
                Json::Array(
                    self.steps
                        .iter()
                        .map(|&(periods, frames)| {
                            Json::Array(vec![
                                Json::Integer(periods as i64),
                                Json::Integer(frames as i64),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("load", self.load.to_json()),
            ("image", Json::text(&self.image)),
            (
                "facts",
                Json::Object(
                    self.facts
                        .iter()
                        .map(|(name, value)| (*name, Json::text(value)))
                        .collect(),
                ),
            ),
            ("machine", self.machine.to_json()),
            (
                "wall_us_each",
                Json::Array(self.wall_us.iter().map(|&us| Json::Number(us)).collect()),
            ),
        ])
    }
}

/// How often each step occurs among `steps`, in periods, the shortest first.
pub fn step_counts(steps: impl IntoIterator<Item = u64>) -> Vec<(u64, usize)> {
    let mut counts: Vec<(u64, usize)> = Vec::new();
    for step in steps {
        match counts.iter_mut().find(|(periods, _)| *periods == step) {
            Some((_, count)) => *count += 1,
            None => counts.push((step, 1)),
        }
    }
    counts.sort_unstable();
    counts
}

/// "2×343, 3×856": each step in periods and how many frames took it.
fn steps_text(steps: &[(u64, usize)]) -> String {
    if steps.is_empty() {
        return "-".to_owned();
    }
    steps
        .iter()
        .map(|(periods, frames)| format!("{periods}×{frames}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The value of `LP_NUM_THREADS` in this process's environment, which Mesa reads.
pub fn rasterizer_threads() -> Option<String> {
    std::env::var("LP_NUM_THREADS").ok()
}

#[cfg(test)]
mod tests {
    use super::{step_counts, steps_text};

    #[test]
    fn counts_the_steps_between_frames() {
        let counts = step_counts([3, 2, 3, 3, 2]);
        assert_eq!(counts, vec![(2, 2), (3, 3)]);
        assert_eq!(steps_text(&counts), "2×2, 3×3");
        assert_eq!(steps_text(&step_counts([])), "-");
    }
}
