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
/// if any is void, else 0. A worse figure comes first: it is what a canary must show, and what a
/// reader looks for first.
pub fn exit_status<'a>(verdicts: impl IntoIterator<Item = &'a Verdict>) -> u8 {
    let mut void = false;
    for verdict in verdicts {
        match verdict {
            Verdict::Worse => return 1,
            Verdict::Void(_) => void = true,
            _ => {}
        }
    }
    if void { 2 } else { 0 }
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
