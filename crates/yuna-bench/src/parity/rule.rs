//! The rule of parity
//! ([ADR-0142](../../../../docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)):
//! each figure has a class, the class says how it is judged, and a judged figure gets a verdict.
//!
//! Every figure the script measures is a cost: lower is better. A *parity* figure is worse when
//! the head's value exceeds the base's by more than its threshold, `max(relative × base,
//! absolute)`: the relative part, about three times the spread of the base measured against
//! itself, the absolute part a floor for figures whose spread is near nothing. A *budget* figure is
//! worse when the head's value exceeds its budget, whatever the base's. A *recorded* figure is
//! written and never judged.
//!
//! A figure that could not be measured on a side, its program missing from that side's tree
//! included, is *void*, with the reason; a void figure never passes.
//!
//! **Timed figures** (the timed tier) are judged over *pairs*: a run of the base and a run of the
//! head, in an order drawn from the run's seed. Each pair gives `ln(head / base)`; over the quiet
//! pairs, the mean of these logs and Student's one-sided 95 % bounds on it, below and above
//! ([`Interval`]). With `T` the figure's threshold, a share of the base's value:
//!
//! - *no worse* when the upper bound is below `ln(1 + T)`: the head is shown to cost less than
//!   `1 + T` times the base;
//! - *worse* when the lower bound is above 0 and the mean above `ln(1 + T)`: the head is shown to
//!   cost more than the base, by more than `T` as far as the pairs tell;
//! - *inconclusive* otherwise: more pairs are run, a batch at a time, up to a cap
//!   ([`PairLimits`]); still undecided at the cap, the figure is *inconclusive*, which never passes.
//!
//! The logs make a ratio's noise symmetric, and a pair compares two runs made a few seconds apart,
//! so that a slow drift of the machine cancels. The mean, not the median: its interval follows
//! from Student's law, where a bootstrap of the median would need many more pairs. Each look at
//! the pairs, after each batch, is made at 95 % with no correction for the looks before it: at
//! the boundary, a figure just at `T`, the chance of a wrong verdict is above 5 %. Fewer quiet
//! pairs than the least ([`PairLimits::least`]), once the disturbed ones may no longer be
//! replaced, make the figure *void*.

/// How a figure is judged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Class {
    /// No worse than the baseline, within its threshold.
    Parity,
    /// No worse than this budget, from the ROADMAP or an ADR, whatever the baseline.
    Budget(f64),
    /// Written, not judged.
    Recorded,
}

impl Class {
    /// Its name, as the run file and the report give it.
    pub fn name(self) -> &'static str {
        match self {
            Class::Parity => "parity",
            Class::Budget(_) => "budget",
            Class::Recorded => "recorded",
        }
    }
}

/// How far a parity figure may grow before it is worse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Threshold {
    /// A share of the base's value: 0.01 is 1 %.
    pub relative: f64,
    /// A floor, in the figure's unit.
    pub absolute: f64,
}

impl Threshold {
    /// The growth allowed above `base`.
    pub fn allowed(self, base: f64) -> f64 {
        (self.relative * base.abs()).max(self.absolute)
    }
}

/// What one side measured of a figure.
#[derive(Debug, Clone, PartialEq)]
pub enum Measured {
    /// Its value.
    Value(f64),
    /// It could not be measured: why.
    Void(String),
}

/// A figure's verdict.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Within its threshold or its budget.
    NoWorse,
    /// Beyond them.
    Worse,
    /// A timed figure whose pairs, at their cap, show it neither no worse nor worse. Never
    /// passes.
    Inconclusive,
    /// Not judged: written.
    Recorded,
    /// Not measured on a side, or measured on unequal work: why. Never passes.
    Void(String),
}

impl Verdict {
    /// Its name, as the run file and the report give it.
    pub fn name(&self) -> &'static str {
        match self {
            Verdict::NoWorse => "no worse",
            Verdict::Worse => "worse",
            Verdict::Inconclusive => "inconclusive",
            Verdict::Recorded => "recorded",
            Verdict::Void(_) => "void",
        }
    }

    /// Why it is void.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Verdict::Void(reason) => Some(reason),
            _ => None,
        }
    }
}

/// Judges a figure of `class` and `threshold` from the base's and the head's measures.
pub fn judge(class: Class, threshold: Threshold, base: &Measured, head: &Measured) -> Verdict {
    let (base, head) = match (base, head) {
        (Measured::Void(reason), _) => return Verdict::Void(format!("base: {reason}")),
        (_, Measured::Void(reason)) => return Verdict::Void(format!("head: {reason}")),
        (Measured::Value(base), Measured::Value(head)) => (*base, *head),
    };
    if !base.is_finite() || !head.is_finite() {
        return Verdict::Void(format!("not a number: base {base}, head {head}"));
    }
    match class {
        Class::Recorded => Verdict::Recorded,
        Class::Budget(budget) if head > budget => Verdict::Worse,
        Class::Budget(_) => Verdict::NoWorse,
        Class::Parity if head - base > threshold.allowed(base) => Verdict::Worse,
        Class::Parity => Verdict::NoWorse,
    }
}

/// The program's exit status for a run whose figures got `verdicts`: 1 if any is worse, else 2
/// if any is void or inconclusive, else 0. A worse figure comes first: it is what a canary must
/// show, and what a reader looks for first.
pub fn exit_status<'a>(verdicts: impl IntoIterator<Item = &'a Verdict>) -> u8 {
    let mut unknown = false;
    for verdict in verdicts {
        match verdict {
            Verdict::Worse => return 1,
            Verdict::Void(_) | Verdict::Inconclusive => unknown = true,
            _ => {}
        }
    }
    if unknown { 2 } else { 0 }
}

/// The one-sided 95 % bounds on the mean of some values: Student's, from their mean and their
/// standard deviation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    /// How many values.
    pub count: usize,
    /// Their mean.
    pub mean: f64,
    /// Their standard deviation, over `count - 1`.
    pub deviation: f64,
    /// The mean is above this, at 95 %.
    pub lower: f64,
    /// The mean is below this, at 95 %.
    pub upper: f64,
}

impl Interval {
    /// The bounds of `values`' mean; none for fewer than two, or for a value that is not a
    /// number.
    pub fn of(values: &[f64]) -> Option<Interval> {
        let count = values.len();
        if count < 2 || values.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let mean = values.iter().sum::<f64>() / count as f64;
        let squares: f64 = values.iter().map(|v| (v - mean).powi(2)).sum();
        let deviation = (squares / (count - 1) as f64).sqrt();
        let half = student_95(count - 1) * deviation / (count as f64).sqrt();
        Some(Interval {
            count,
            mean,
            deviation,
            lower: mean - half,
            upper: mean + half,
        })
    }
}

/// Student's one-sided 95 % quantile for `freedom` degrees of freedom: from its table up to 30,
/// beyond by the Cornish–Fisher expansion around the normal law's 1.6449, within 0.0002 of it.
pub fn student_95(freedom: usize) -> f64 {
    const TABLE: [f64; 30] = [
        6.314, 2.920, 2.353, 2.132, 2.015, 1.943, 1.895, 1.860, 1.833, 1.812, 1.796, 1.782, 1.771,
        1.761, 1.753, 1.746, 1.740, 1.734, 1.729, 1.725, 1.721, 1.717, 1.714, 1.711, 1.708, 1.706,
        1.703, 1.701, 1.699, 1.697,
    ];
    match freedom {
        0 => f64::INFINITY,
        1..=30 => TABLE[freedom - 1],
        _ => {
            let z = 1.644_853_6_f64;
            let v = freedom as f64;
            z + (z.powi(3) + z) / (4.0 * v)
                + (5.0 * z.powi(5) + 16.0 * z.powi(3) + 3.0 * z) / (96.0 * v * v)
        }
    }
}

/// How many quiet pairs a timed figure takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairLimits {
    /// The fewest it is judged on: fewer, once no more may run, make it void.
    pub least: usize,
    /// The most it runs, and the most disturbed pairs it drops and replaces.
    pub most: usize,
    /// How many more an undecided figure runs at a time.
    pub batch: usize,
}

/// What a timed figure's pairs decide.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Its verdict, and the interval on its logs when it has one.
    Judged(Verdict, Option<Interval>),
    /// Run this many more pairs, then decide again.
    More(usize),
}

/// What the quiet pairs' logs `ln(head / base)` decide for a timed figure of `class` whose
/// threshold is `threshold`, a share of the base's value, when `dropped` pairs were dropped as
/// disturbed. A parity figure gets the three verdicts; a recorded one is recorded once it has its
/// least pairs; a budget figure is judged on the head's own values ([`judge`]), once its pairs
/// are run, and gets [`Verdict::Recorded`] here.
pub fn decide(
    class: Class,
    threshold: f64,
    logs: &[f64],
    dropped: usize,
    limits: PairLimits,
) -> Decision {
    // A disturbed pair is replaced until `most` were dropped; quiet ones run up to `most`.
    let may_run = if dropped >= limits.most {
        0
    } else {
        limits.most.saturating_sub(logs.len())
    };
    if logs.len() < limits.least {
        return if may_run > 0 {
            Decision::More((limits.least - logs.len()).min(may_run))
        } else {
            Decision::Judged(
                Verdict::Void(format!(
                    "{} quiet pairs of the {} needed, {dropped} dropped as disturbed",
                    logs.len(),
                    limits.least
                )),
                None,
            )
        };
    }
    let Some(interval) = Interval::of(logs) else {
        return Decision::Judged(
            Verdict::Void(format!("no interval on {} pairs", logs.len())),
            None,
        );
    };
    if class != Class::Parity {
        return Decision::Judged(Verdict::Recorded, Some(interval));
    }
    match shown(&interval, threshold.ln_1p()) {
        Some(verdict) => Decision::Judged(verdict, Some(interval)),
        None if may_run > 0 => Decision::More(limits.batch.max(1).min(may_run)),
        None => Decision::Judged(Verdict::Inconclusive, Some(interval)),
    }
}

/// What `interval` shows of a cost against `allowed`, its growth allowed: no worse when its upper
/// bound is below `allowed`; worse when its lower bound is above 0 and its mean above `allowed`;
/// none when it shows neither.
pub fn shown(interval: &Interval, allowed: f64) -> Option<Verdict> {
    if interval.upper < allowed {
        Some(Verdict::NoWorse)
    } else if interval.lower > 0.0 && interval.mean > allowed {
        Some(Verdict::Worse)
    } else {
        None
    }
}

/// The spread of `values`: their range over their median, 0 for fewer than two; none when the
/// median is 0 and the values differ.
pub fn spread(values: &[f64]) -> Option<f64> {
    if values.len() < 2 {
        return Some(0.0);
    }
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let middle = median(values)?;
    if high == low {
        Some(0.0)
    } else if middle == 0.0 {
        None
    } else {
        Some((high - low) / middle.abs())
    }
}

/// The median of `values`; none for none.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let half = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[half]
    } else {
        f64::midpoint(sorted[half - 1], sorted[half])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_PERCENT: Threshold = Threshold {
        relative: 0.01,
        absolute: 2.0,
    };

    fn value(v: f64) -> Measured {
        Measured::Value(v)
    }

    #[test]
    fn a_parity_figure_is_worse_beyond_its_threshold_alone() {
        // 1 % of 1,000 is 10, above the floor of 2.
        let judge = |base, head| judge(Class::Parity, ONE_PERCENT, &value(base), &value(head));
        assert_eq!(judge(1000.0, 1010.0), Verdict::NoWorse);
        assert_eq!(judge(1000.0, 1010.5), Verdict::Worse);
        // Better is no worse.
        assert_eq!(judge(1000.0, 500.0), Verdict::NoWorse);
        // Near 0, the floor rules: 1 % of 10 is 0.1, the floor 2.
        assert_eq!(judge(10.0, 12.0), Verdict::NoWorse);
        assert_eq!(judge(10.0, 12.1), Verdict::Worse);
        // From nothing, the floor alone: a wake at rest where there was none is worse.
        let any_wake = Threshold {
            relative: 0.0,
            absolute: 0.5,
        };
        assert_eq!(
            judge_with(any_wake, 0.0, 1.0),
            Verdict::Worse,
            "one wake at rest"
        );
        assert_eq!(judge_with(any_wake, 0.0, 0.0), Verdict::NoWorse);
    }

    fn judge_with(threshold: Threshold, base: f64, head: f64) -> Verdict {
        judge(Class::Parity, threshold, &value(base), &value(head))
    }

    #[test]
    fn a_budget_figure_is_judged_against_its_budget_not_the_base() {
        let budget = Class::Budget(8.0);
        assert_eq!(
            judge(budget, ONE_PERCENT, &value(9.0), &value(7.9)),
            Verdict::NoWorse
        );
        // Far worse than the base, still within its budget.
        assert_eq!(
            judge(budget, ONE_PERCENT, &value(1.0), &value(8.0)),
            Verdict::NoWorse
        );
        assert_eq!(
            judge(budget, ONE_PERCENT, &value(1.0), &value(8.1)),
            Verdict::Worse
        );
    }

    #[test]
    fn a_recorded_figure_is_never_judged() {
        assert_eq!(
            judge(Class::Recorded, ONE_PERCENT, &value(1.0), &value(1e9)),
            Verdict::Recorded
        );
    }

    #[test]
    fn a_figure_missing_on_a_side_is_void() {
        let void = Measured::Void("unequal work: frames 40 and 39".to_owned());
        let verdict = judge(Class::Parity, ONE_PERCENT, &void, &value(1.0));
        assert_eq!(
            verdict,
            Verdict::Void("base: unequal work: frames 40 and 39".to_owned())
        );
        assert!(matches!(
            judge(Class::Recorded, ONE_PERCENT, &value(1.0), &void),
            Verdict::Void(_)
        ));
        // A tree that lacks the program: nothing to compare with, never a pass, on either side.
        let lacking = Measured::Void("its tree has no examples/operations.rs".to_owned());
        assert_eq!(
            judge(Class::Parity, ONE_PERCENT, &value(1.0), &lacking).reason(),
            Some("head: its tree has no examples/operations.rs")
        );
        assert!(matches!(
            judge(Class::Parity, ONE_PERCENT, &lacking, &lacking),
            Verdict::Void(_)
        ));
        assert!(matches!(
            judge(Class::Parity, ONE_PERCENT, &value(f64::NAN), &value(1.0)),
            Verdict::Void(_)
        ));
    }

    #[test]
    fn the_exit_status_says_worse_first_then_void() {
        let void = Verdict::Void("x".to_owned());
        assert_eq!(exit_status(&[Verdict::NoWorse, Verdict::Recorded]), 0);
        assert_eq!(exit_status(&[Verdict::NoWorse, void.clone()]), 2);
        assert_eq!(exit_status(&[void, Verdict::Worse]), 1);
        assert_eq!(exit_status(&[]), 0);
        // Inconclusive never passes.
        assert_eq!(exit_status(&[Verdict::NoWorse, Verdict::Inconclusive]), 2);
        assert_eq!(exit_status(&[Verdict::Inconclusive, Verdict::Worse]), 1);
    }

    const LIMITS: PairLimits = PairLimits {
        least: 10,
        most: 30,
        batch: 5,
    };

    /// `count` logs of ratios around `ratio`, spread ± `noise` in turn: a mean of `ln(ratio)`.
    fn logs(ratio: f64, noise: f64, count: usize) -> Vec<f64> {
        (0..count)
            .map(|i| ratio.ln() + if i % 2 == 0 { noise } else { -noise })
            .collect()
    }

    #[test]
    fn students_quantile_from_its_table_and_beyond() {
        assert_eq!(student_95(1), 6.314);
        assert_eq!(student_95(9), 1.833);
        assert_eq!(student_95(30), 1.697);
        // The published 1.684 at 40, 1.671 at 60, 1.658 at 120; the normal law's beyond.
        assert!(
            (student_95(40) - 1.684).abs() < 0.0005,
            "{}",
            student_95(40)
        );
        assert!(
            (student_95(60) - 1.671).abs() < 0.0005,
            "{}",
            student_95(60)
        );
        assert!(
            (student_95(120) - 1.658).abs() < 0.0005,
            "{}",
            student_95(120)
        );
        assert!((student_95(1_000_000) - 1.6449).abs() < 0.0001);
        // At 30, the expansion meets the table.
        let z = 1.644_853_6_f64;
        let expansion = z
            + (z.powi(3) + z) / 120.0
            + (5.0 * z.powi(5) + 16.0 * z.powi(3) + 3.0 * z) / (96.0 * 900.0);
        assert!((expansion - 1.697).abs() < 0.0005, "{expansion}");
        assert_eq!(student_95(0), f64::INFINITY);
    }

    #[test]
    fn an_interval_on_a_mean() {
        // Mean 2, deviation 1 over 4 values: 2 ± 2.353 × 1 / 2.
        let interval = Interval::of(&[1.0, 2.0, 2.0, 3.0]).unwrap();
        assert_eq!(interval.count, 4);
        assert_eq!(interval.mean, 2.0);
        assert!((interval.deviation - (2.0_f64 / 3.0).sqrt()).abs() < 1e-12);
        let half = 2.353 * (2.0_f64 / 3.0).sqrt() / 2.0;
        assert!((interval.upper - (2.0 + half)).abs() < 1e-12);
        assert!((interval.lower - (2.0 - half)).abs() < 1e-12);
        assert_eq!(Interval::of(&[1.0]), None);
        assert_eq!(Interval::of(&[1.0, f64::NAN]), None);
    }

    #[test]
    fn timed_pairs_say_no_worse_when_the_head_is_shown_within_its_threshold() {
        // The same build twice: ratios of 1 ± 2 %, a threshold of 5 %.
        let decision = decide(Class::Parity, 0.05, &logs(1.0, 0.02, 10), 0, LIMITS);
        let Decision::Judged(Verdict::NoWorse, Some(interval)) = decision else {
            panic!("{decision:?}");
        };
        assert!(interval.upper < 0.05_f64.ln_1p());
        // A head 1 % slower is still within 5 %.
        assert!(matches!(
            decide(Class::Parity, 0.05, &logs(1.01, 0.02, 10), 0, LIMITS),
            Decision::Judged(Verdict::NoWorse, _)
        ));
        // A faster head too.
        assert!(matches!(
            decide(Class::Parity, 0.05, &logs(0.8, 0.02, 10), 0, LIMITS),
            Decision::Judged(Verdict::NoWorse, _)
        ));
    }

    #[test]
    fn timed_pairs_say_worse_when_the_head_is_shown_slower_beyond_its_threshold() {
        // Three times the threshold: 15 % slower, ± 2 %.
        let decision = decide(Class::Parity, 0.05, &logs(1.15, 0.02, 10), 0, LIMITS);
        assert!(
            matches!(decision, Decision::Judged(Verdict::Worse, Some(_))),
            "{decision:?}"
        );
        // Slower, surely, but by less than the threshold on average: not worse, and not shown
        // within it either while the noise is wide.
        let decision = decide(Class::Parity, 0.05, &logs(1.04, 0.06, 10), 0, LIMITS);
        assert_eq!(decision, Decision::More(5));
    }

    #[test]
    fn undecided_pairs_run_more_up_to_the_cap_then_are_inconclusive() {
        // A head 4.5 % slower, near its threshold of 5 %, with noise: neither shown.
        let undecided = logs(1.045, 0.05, 10);
        assert_eq!(
            decide(Class::Parity, 0.05, &undecided, 0, LIMITS),
            Decision::More(5)
        );
        // Near the cap, only what is left of it.
        assert_eq!(
            decide(Class::Parity, 0.05, &logs(1.045, 0.05, 28), 0, LIMITS),
            Decision::More(2)
        );
        // At the cap: inconclusive, which never passes.
        let decision = decide(Class::Parity, 0.05, &logs(1.045, 0.05, 30), 0, LIMITS);
        assert!(
            matches!(decision, Decision::Judged(Verdict::Inconclusive, Some(_))),
            "{decision:?}"
        );
        // A threshold of 0 can never be shown: inconclusive at the cap, never no worse.
        assert!(matches!(
            decide(Class::Parity, 0.0, &logs(1.0, 0.01, 30), 0, LIMITS),
            Decision::Judged(Verdict::Inconclusive, _)
        ));
    }

    #[test]
    fn too_few_quiet_pairs_run_more_then_make_the_figure_void() {
        // 6 quiet, 4 dropped: 4 more to reach the least.
        assert_eq!(
            decide(Class::Parity, 0.05, &logs(1.0, 0.01, 6), 4, LIMITS),
            Decision::More(4)
        );
        // Once the cap of dropped pairs is reached, none is replaced: void, however clear the
        // quiet pairs are.
        let decision = decide(Class::Parity, 0.05, &logs(1.0, 0.01, 6), 30, LIMITS);
        let Decision::Judged(Verdict::Void(reason), None) = decision else {
            panic!("{decision:?}");
        };
        assert!(reason.contains("6 quiet pairs of the 10"), "{reason}");
        // Enough quiet pairs are judged, however many were dropped.
        assert!(matches!(
            decide(Class::Parity, 0.05, &logs(1.0, 0.01, 10), 30, LIMITS),
            Decision::Judged(Verdict::NoWorse, _)
        ));
        // None at all.
        assert_eq!(
            decide(Class::Parity, 0.05, &[], 0, LIMITS),
            Decision::More(10)
        );
    }

    #[test]
    fn a_recorded_timed_figure_needs_its_pairs_and_no_verdict() {
        assert!(matches!(
            decide(Class::Recorded, 0.0, &logs(3.0, 0.2, 10), 0, LIMITS),
            Decision::Judged(Verdict::Recorded, Some(_))
        ));
        assert_eq!(
            decide(Class::Recorded, 0.0, &logs(3.0, 0.2, 2), 0, LIMITS),
            Decision::More(8)
        );
    }

    #[test]
    fn spreads_and_medians() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&[]), None);
        assert_eq!(spread(&[100.0, 101.0, 99.0]), Some(0.02));
        assert_eq!(spread(&[5.0]), Some(0.0));
        assert_eq!(spread(&[0.0, 0.0]), Some(0.0));
        assert_eq!(spread(&[-1.0, 0.0, 1.0]), None);
    }
}
