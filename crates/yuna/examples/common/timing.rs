//! What the examples that time one step of Yuna's share: their `--rounds` argument, the summary
//! of their times, and the machine's load, which a figure taken while other programs load the
//! processor reports beside it. `events_look` and `rest_cost` include it by its path.

use std::time::{Duration, Instant};

/// `--rounds <n>`, `default` without it; `example` names the program in its usage.
pub fn read_rounds(
    mut arguments: impl Iterator<Item = String>,
    default: usize,
    example: &str,
) -> Result<usize, String> {
    match (arguments.next().as_deref(), arguments.next()) {
        (None, _) => Ok(default),
        (Some("--rounds"), Some(value)) => value
            .parse()
            .ok()
            .filter(|&rounds| rounds > 0)
            .ok_or_else(|| "--rounds takes a whole number above 0".to_owned()),
        _ => Err(format!("usage: {example} [--rounds <n>]")),
    }
}

/// The machine's load average over the last minute, as `/proc/loadavg` gives it.
pub fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|text| text.split_whitespace().next().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// How long `step` takes.
pub fn time(step: impl FnOnce()) -> Duration {
    let began = Instant::now();
    step();
    began.elapsed()
}

/// The median, the 99th percentile and the longest of `times`, in microseconds.
pub fn summary(mut times: Vec<Duration>) -> [f64; 3] {
    times.sort();
    let at = |share: f64| {
        let index = ((times.len() - 1) as f64 * share).round() as usize;
        times[index].as_secs_f64() * 1e6
    };
    [at(0.5), at(0.99), at(1.0)]
}
