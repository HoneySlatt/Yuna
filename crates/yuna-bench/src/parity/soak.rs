//! The soak, for the timed tier: ten `yuna mcp` of each side, a seat and a lockstep client each,
//! held at rest on the real clock, their `Pss_Anon` read every [`READING`]; its figure is the
//! slope of each side's sum, which must show no growth
//! ([ADR-0082](../../../../docs/adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
//! item 25, [ADR-0100](../../../../docs/adr/0100-the-budgets-of-yuna-mcp-at-their-conditions.md)).
//!
//! Both sides' instances run at once, started in turn: an instance's anonymous memory is its own,
//! which another process beside it does not move (`Pss_File` would, which the soak leaves out),
//! and the soak then takes its time once. The load is read, not judged: memory at rest does not
//! follow it.

use std::path::Path;
use std::time::{Duration, Instant};

use super::figures::Side;
use super::scenario::{Build, Instrument, Run, SOAK, Tools};

/// The instances of each side.
pub const INSTANCES: usize = 10;
/// How often the memory is read.
pub const READING: Duration = Duration::from_secs(30);
/// How long after every instance is ready the first reading waits: their first frames drawn, the
/// processor's work of their start over.
const SETTLE: Duration = Duration::from_secs(10);

/// What the soak read of one side.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Series {
    /// Each reading: its time from the first, in minutes, and the side's instances' `Pss_Anon`
    /// summed, in KiB.
    pub readings: Vec<(f64, f64)>,
    /// Why it could not be read to the end.
    pub error: Option<String>,
}

/// Holds [`INSTANCES`] instances of each of `builds`, base then head, for `length` after they
/// settle, their folders and sockets under `work`; the series of each side.
pub fn run(builds: [&Build; 2], tools: &Tools, work: &Path, length: Duration) -> [Series; 2] {
    let mut series = [Series::default(), Series::default()];
    let mut runs: Vec<(Side, Run)> = Vec::new();
    for index in 0..INSTANCES {
        for (side, build) in [(Side::Base, builds[0]), (Side::Head, builds[1])] {
            let at = side as usize;
            if series[at].error.is_some() {
                continue;
            }
            let folder = work
                .join("runs")
                .join(format!("{}_soak_{index}", side.name()));
            let _ = std::fs::remove_dir_all(&folder);
            // The runtime folders of the timed runs are r0, r1; the soak's follow.
            let runtime = 2 + 2 * index + at;
            match Run::start(&SOAK, Instrument::Soak, build, tools, &folder, runtime) {
                Ok(run) => runs.push((side, run)),
                Err(error) => {
                    series[at].error = Some(format!("instance {index} did not start: {error}"));
                }
            }
        }
    }
    std::thread::sleep(SETTLE);
    let began = Instant::now();
    let mut next = Duration::ZERO;
    while next <= length {
        std::thread::sleep((began + next).saturating_duration_since(Instant::now()));
        let minutes = began.elapsed().as_secs_f64() / 60.0;
        for (at, side) in [Side::Base, Side::Head].into_iter().enumerate() {
            if series[at].error.is_some() {
                continue;
            }
            let mut total = 0;
            let mut read = 0;
            for (_, run) in runs.iter().filter(|(of, _)| *of == side) {
                let (anon, _) = run.memory();
                if anon > 0 {
                    read += 1;
                }
                total += anon;
            }
            if read < INSTANCES {
                series[at].error = Some(format!(
                    "{read} of its {INSTANCES} instances read at {minutes:.1} min: one ended"
                ));
            } else {
                series[at].readings.push((minutes, total as f64));
            }
        }
        next += READING;
    }
    for (side, run) in runs {
        if let Err(error) = run.stop() {
            series[side as usize].error.get_or_insert(error);
        }
    }
    series
}

/// The least-squares slope of `readings`, `(minutes, KiB)`, in KiB a minute, with Student's
/// one-sided 95 % bounds on it, from the residuals' spread over `n - 2`; none for fewer than three
/// readings, or readings all at one time.
pub fn slope(readings: &[(f64, f64)]) -> Option<super::rule::Interval> {
    let count = readings.len();
    if count < 3 {
        return None;
    }
    let n = count as f64;
    let mean_t = readings.iter().map(|(t, _)| t).sum::<f64>() / n;
    let mean_y = readings.iter().map(|(_, y)| y).sum::<f64>() / n;
    let spread_t: f64 = readings.iter().map(|(t, _)| (t - mean_t).powi(2)).sum();
    if spread_t == 0.0 {
        return None;
    }
    let slope = readings
        .iter()
        .map(|(t, y)| (t - mean_t) * (y - mean_y))
        .sum::<f64>()
        / spread_t;
    let residuals: f64 = readings
        .iter()
        .map(|(t, y)| (y - mean_y - slope * (t - mean_t)).powi(2))
        .sum();
    let deviation = (residuals / (n - 2.0) / spread_t).sqrt();
    let half = super::rule::student_95(count - 2) * deviation;
    Some(super::rule::Interval {
        count,
        mean: slope,
        deviation,
        lower: slope - half,
        upper: slope + half,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_series_has_no_slope_and_a_growing_one_its_own() {
        let flat: Vec<(f64, f64)> = (0..21).map(|i| (f64::from(i) * 0.5, 480_000.0)).collect();
        let interval = slope(&flat).unwrap();
        assert_eq!(
            (interval.mean, interval.lower, interval.upper),
            (0.0, 0.0, 0.0)
        );
        // 64 KiB a minute, exactly.
        let growing: Vec<(f64, f64)> = (0..21)
            .map(|i| (f64::from(i) * 0.5, 480_000.0 + 32.0 * f64::from(i)))
            .collect();
        let interval = slope(&growing).unwrap();
        assert!((interval.mean - 64.0).abs() < 1e-9, "{interval:?}");
        assert!(interval.upper - interval.lower < 1e-6);
        // One step of a page, halfway: a small slope, with bounds around it.
        let step: Vec<(f64, f64)> = (0..21)
            .map(|i| {
                (
                    f64::from(i) * 0.5,
                    480_000.0 + if i > 10 { 4.0 } else { 0.0 },
                )
            })
            .collect();
        let interval = slope(&step).unwrap();
        assert!(interval.mean > 0.0 && interval.mean < 1.0, "{interval:?}");
        assert!(interval.lower < interval.mean && interval.mean < interval.upper);
        assert_eq!(slope(&flat[..2]), None);
        assert_eq!(slope(&[(1.0, 1.0), (1.0, 2.0), (1.0, 3.0)]), None);
    }
}
