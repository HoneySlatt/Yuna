//! A spring's motion around its target, in closed form, and the time from which a bound of its
//! distance to the target stays under a tolerance.

use std::f64::consts::{E, LN_2};

/// The share of the tolerance that ends are solved under, so that rounding in a bound never makes
/// an end early: the end is computed for `tolerance · (1 − TOLERANCE_MARGIN)`.
const TOLERANCE_MARGIN: f64 = 1e-9;

/// The most steps Newton's method takes; past them, the last time known to be safe is kept.
const MAX_ITERATIONS: usize = 100;

/// How finely an end is solved, in seconds: a tenth of a nanosecond, finer than
/// [`std::time::Duration`]'s nanoseconds.
const TIME_PRECISION: f64 = 1e-10;

/// A spring's motion around its target. Its offset `x` from the target solves
/// `x'' + 2ζω·x' + ω²·x = 0`, mass 1, from `x(0) = x0` and `x'(0) = v0`, time in seconds.
///
/// Every regime is written `x(t) = x0·c(t) + (v0 + β·x0)·s(t)`, with β = ζω, where `c` and `s`
/// are the solutions from `(c, c') = (1, −β)` and `(s, s') = (0, 1)`:
///
/// | Regime | c(t) | s(t) |
/// |---|---|---|
/// | underdamped, ζ < 1 | e^(−βt)·cos(ω_d·t) | e^(−βt)·sin(ω_d·t)/ω_d |
/// | critical, ζ = 1 | e^(−ωt) | t·e^(−ωt) |
/// | overdamped, ζ > 1 | (e^(−λ₁t) + e^(−λ₂t))/2 | e^(−λ₁t)·(1 − e^(−2ω_o·t))/(2ω_o) |
///
/// with ω_d = ω·√(1 − ζ²), ω_o = ω·√(ζ² − 1), and the two decay rates λ₁ = β − ω_o and
/// λ₂ = β + ω_o. The velocity is itself a solution, from `v0` and `x''(0) = −2β·v0 − ω²·x0`, so
/// `x'(t) = v0·c(t) − (ω²·x0 + β·v0)·s(t)`.
///
/// None of these forms divides by a frequency that vanishes or subtracts two large numbers as ζ
/// nears 1: `sin(ω_d·t)/ω_d` and `(1 − e^(−2ω_o·t))/(2ω_o)` (through `exp_m1`) both tend to `t`
/// with all their digits. The overdamped form holds only decaying exponentials, so that nothing
/// overflows however long the spring runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Oscillator {
    /// The offset from the target at time 0.
    x0: f64,
    /// The velocity at time 0.
    v0: f64,
    /// β = ζω.
    beta: f64,
    /// `v0 + β·x0`: the weight of `s` in the offset.
    offset_weight: f64,
    /// `ω²·x0 + β·v0`: the weight of `s` in the velocity, with its sign changed.
    velocity_weight: f64,
    regime: Regime,
}

/// What `c` and `s` are made of; see [`Oscillator`].
#[derive(Debug, Clone, Copy, PartialEq)]
enum Regime {
    /// ζ < 1: it oscillates at `frequency`, ω_d.
    Under { frequency: f64 },
    /// ζ = 1.
    Critical,
    /// ζ > 1: `spread` is ω_o; `slow` and `fast` are the decay rates λ₁ and λ₂.
    Over { spread: f64, slow: f64, fast: f64 },
}

impl Oscillator {
    /// The motion of a spring of `damping_ratio` and `stiffness` from offset `x0` and velocity
    /// `v0`. The caller checks [`Oscillator::is_finite`] before using it.
    pub(crate) fn new(damping_ratio: f64, stiffness: f64, x0: f64, v0: f64) -> Oscillator {
        let omega = stiffness.sqrt();
        let beta = damping_ratio * omega;
        // (1 − ζ)(1 + ζ) rather than 1 − ζ², which loses digits as ζ nears 1.
        let regime = if damping_ratio < 1.0 {
            let frequency = omega * ((1.0 - damping_ratio) * (1.0 + damping_ratio)).sqrt();
            Regime::Under { frequency }
        } else if damping_ratio > 1.0 {
            let spread = omega * ((damping_ratio - 1.0) * (damping_ratio + 1.0)).sqrt();
            let fast = beta + spread;
            // λ₁·λ₂ = β² − ω_o² = ω², so λ₁ = ω²/λ₂: no digits lost subtracting ω_o from β when
            // ζ is large.
            let slow = stiffness / fast;
            Regime::Over { spread, slow, fast }
        } else {
            Regime::Critical
        };
        Oscillator {
            x0,
            v0,
            beta,
            offset_weight: v0 + beta * x0,
            velocity_weight: stiffness * x0 + beta * v0,
            regime,
        }
    }

    /// Whether every number the motion is computed from is finite: false for inputs that are
    /// not, or so large that they overflow.
    pub(crate) fn is_finite(&self) -> bool {
        [self.x0, self.v0, self.offset_weight, self.velocity_weight]
            .iter()
            .all(|number| number.is_finite())
    }

    /// The offset from the target at `t` seconds, t ≥ 0. It may overflow to an infinity for
    /// inputs near `f64`'s limits, never to NaN: only its second term can.
    pub(crate) fn offset(&self, t: f64) -> f64 {
        let (c, s) = self.basis(t);
        self.x0 * c + self.offset_weight * s
    }

    /// The velocity at `t` seconds, t ≥ 0; like [`Oscillator::offset`], never NaN.
    pub(crate) fn velocity(&self, t: f64) -> f64 {
        let (c, s) = self.basis(t);
        self.v0 * c - self.velocity_weight * s
    }

    /// `c(t)` and `s(t)`; see [`Oscillator`]. Both stay finite: |c| ≤ 1 and 0 ≤ s ≤ t.
    fn basis(&self, t: f64) -> (f64, f64) {
        match self.regime {
            Regime::Under { frequency } => {
                let decay = (-self.beta * t).exp();
                let (sin, cos) = (frequency * t).sin_cos();
                (decay * cos, decay * (sin / frequency))
            }
            Regime::Critical => {
                let decay = (-self.beta * t).exp();
                (decay, decay * t)
            }
            Regime::Over { spread, slow, fast } => {
                let slow_decay = (-slow * t).exp();
                let fast_decay = (-fast * t).exp();
                let rise = -(-2.0 * spread * t).exp_m1() / (2.0 * spread);
                ((slow_decay + fast_decay) / 2.0, slow_decay * rise)
            }
        }
    }

    /// The time, in seconds, from which a bound of |x| stays at or under `tolerance`: 0 when the
    /// bound never exceeds it, finite always. `tolerance` is positive and finite.
    ///
    /// Two bounds hold in each regime, and the earlier of their times is kept, each being safe:
    ///
    /// - `(|x0| + |v0 + λ·x0|·t)·e^(−λt)`, with λ = β below and at critical damping and λ = λ₁
    ///   above it. It is exact for a critical spring whose two terms share a sign, and close to
    ///   it near ζ = 1. Below: |cos| ≤ 1 and |sin u| ≤ |u|. Above: `x = e^(−λ₁t)·(x0 + u·(v0 +
    ///   λ₁·x0)/(2ω_o))` with `u = 1 − e^(−2ω_o·t)`, and 0 ≤ u ≤ 2ω_o·t.
    /// - Underdamped, `√(x0² + ((v0 + β·x0)/ω_d)²)·e^(−βt)`, the oscillation's amplitude;
    ///   overdamped, `|c₁|·e^(−λ₁t) + |c₂|·e^(−λ₂t)`, with `x = c₁·e^(−λ₁t) + c₂·e^(−λ₂t)`. These
    ///   are tight away from ζ = 1, where the first one's factor `t` is loose; near it, they grow
    ///   without limit (ω_d and ω_o tend to 0) and the first one is kept.
    pub(crate) fn settle_time(&self, tolerance: f64) -> f64 {
        let level = tolerance * (1.0 - TOLERANCE_MARGIN);
        let (slowest_rate, slowest_weight) = match self.regime {
            Regime::Over { slow, .. } => (slow, self.v0 + slow * self.x0),
            Regime::Under { .. } | Regime::Critical => (self.beta, self.offset_weight),
        };
        let near_critical =
            linear_exponential_settle(self.x0.abs(), slowest_weight.abs(), slowest_rate, level);
        let away_from_critical = match self.regime {
            Regime::Under { frequency } => {
                let amplitude = self.x0.hypot(self.offset_weight / frequency);
                exponential_settle(amplitude, self.beta, level)
            }
            Regime::Critical => f64::INFINITY,
            Regime::Over { spread, slow, fast } => {
                let slow_weight = (self.x0 + self.offset_weight / spread) / 2.0;
                let fast_weight = (self.x0 - self.offset_weight / spread) / 2.0;
                two_exponentials_settle((slow_weight, slow), (fast_weight, fast), level)
            }
        };
        near_critical.min(away_from_critical)
    }
}

/// The time from which `amplitude·e^(−rate·t)` stays at or under `level`: 0 if it starts there,
/// infinite if the amplitude is (a bound that does not help).
fn exponential_settle(amplitude: f64, rate: f64, level: f64) -> f64 {
    if amplitude <= level {
        0.0
    } else {
        // Logarithms apart, so that amplitude / level cannot overflow.
        (amplitude.ln() - level.ln()) / rate
    }
}

/// The time from which `(a + b·t)·e^(−rate·t)` stays at or under `level`: 0 if it never exceeds
/// it. a, b ≥ 0 and finite; rate and level > 0.
///
/// The bound rises to its peak at `1/rate − a/b`, then falls. Its logarithm, `ln(a + b·t) −
/// rate·t`, is concave, so that Newton's method started right of the crossing stays right of it:
/// each step is a time already safe.
fn linear_exponential_settle(a: f64, b: f64, rate: f64, level: f64) -> f64 {
    if b == 0.0 {
        return exponential_settle(a, rate, level);
    }
    let ln_level = level.ln();
    let excess_and_slope = |t: f64| {
        let excess = ln_affine(a, b, t) - rate * t - ln_level;
        // d/dt ln(a + b·t) = b/(a + b·t), written so that a + b·t is never formed.
        let slope = 1.0 / (a / b + t) - rate;
        (excess, slope)
    };
    let peak = (1.0 / rate - a / b).max(0.0);
    if excess_and_slope(peak).0 <= 0.0 {
        return 0.0;
    }
    // t ≤ 2/(rate·e)·e^(rate·t/2), so the bound is under (a + 2b/(rate·e))·e^(−rate·t/2), which
    // reaches the level at `safe`: a time past the crossing.
    let safe = 2.0 / rate * (ln_affine(a, b, 2.0 / (rate * E)) - ln_level);
    first_time_under(excess_and_slope, peak, safe, safe)
}

/// The time from which `|c₁|·e^(−r₁t) + |c₂|·e^(−r₂t)` stays at or under `level`, given each
/// term as `(c, r)`; infinite if a weight is (a bound that does not help).
///
/// The sum falls from t = 0 and its logarithm is convex, so that Newton's method started at 0
/// approaches the crossing from below; [`first_time_under`] then steps just past it, to a time
/// it checks.
fn two_exponentials_settle(first: (f64, f64), second: (f64, f64), level: f64) -> f64 {
    let ((c1, r1), (c2, r2)) = (first, second);
    if !(c1.is_finite() && c2.is_finite()) {
        return f64::INFINITY;
    }
    let (ln_c1, ln_c2) = (c1.abs().ln(), c2.abs().ln());
    let ln_level = level.ln();
    let excess_and_slope = |t: f64| {
        let (ln_term1, ln_term2) = (ln_c1 - r1 * t, ln_c2 - r2 * t);
        let ln_sum = ln_sum_exp(ln_term1, ln_term2);
        // The slope of ln(sum): each rate, weighted by its term's share of the sum.
        let slope = -(r1 * (ln_term1 - ln_sum).exp() + r2 * (ln_term2 - ln_sum).exp());
        (ln_sum - ln_level, slope)
    };
    if excess_and_slope(0.0).0 <= 0.0 {
        return 0.0;
    }
    // Once each term is under half the level, the sum is under it. A weight of 0 gives −∞.
    let safe = ((ln_c1 - ln_level + LN_2) / r1)
        .max((ln_c2 - ln_level + LN_2) / r2)
        .max(0.0);
    first_time_under(excess_and_slope, 0.0, safe, 0.0)
}

/// The first time from which a falling quantity stays at or under 0. `excess_and_slope` gives
/// the quantity and its slope at a time; it is above 0 at `low`, at or under 0 at `high`, and
/// the search starts at `start`, one of the two.
///
/// Newton's method, kept within `low..high` by bisection. Each time tried is sorted by the sign
/// found there, so that `high` is always a time seen at or under 0, and `high` is the answer:
/// never early, never infinite, within [`TIME_PRECISION`] of the crossing unless
/// [`MAX_ITERATIONS`] run out first.
fn first_time_under(
    excess_and_slope: impl Fn(f64) -> (f64, f64),
    mut low: f64,
    mut high: f64,
    start: f64,
) -> f64 {
    let mut t = start;
    for _ in 0..MAX_ITERATIONS {
        let (excess, slope) = excess_and_slope(t);
        // A NaN is sorted to neither side; the bisection below moves on from it.
        if excess > 0.0 {
            low = t;
        } else if excess <= 0.0 {
            high = t;
        }
        // Finer than this, a time in seconds runs out of digits.
        let precision = TIME_PRECISION + t * 4.0 * f64::EPSILON;
        if high - low <= precision {
            break;
        }
        let step = -excess / slope;
        let next = if step.abs() <= precision / 2.0 {
            if excess <= 0.0 {
                // `t` is `high`, and the crossing a negligible step away.
                break;
            }
            // Newton converging from below never brings `high` down: try just past the
            // crossing, which then lies within the precision.
            t + step + precision / 2.0
        } else {
            t + step
        };
        // A step out of the bracket, or NaN, falls back on bisection.
        t = if low < next && next < high {
            next
        } else {
            low + (high - low) / 2.0
        };
    }
    high
}

/// `ln(a + b·t)` for a, b, t ≥ 0, without forming `a + b·t`, which may overflow.
fn ln_affine(a: f64, b: f64, t: f64) -> f64 {
    let bt = b * t;
    if bt > a {
        b.ln() + t.ln() + (a / bt).ln_1p()
    } else if a > 0.0 {
        a.ln() + (bt / a).ln_1p()
    } else {
        f64::NEG_INFINITY
    }
}

/// `ln(e^u + e^v)`, without overflow; −∞ when both are.
fn ln_sum_exp(u: f64, v: f64) -> f64 {
    let (high, low) = if u >= v { (u, v) } else { (v, u) };
    if low == f64::NEG_INFINITY {
        high
    } else {
        high + (low - high).exp().ln_1p()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 4096,
            ..ProptestConfig::default()
        })]

        /// The end is a true bound: from the settle time on, the motion itself, before any
        /// rounding to the target, stays within the tolerance; and a motion that starts farther
        /// than the tolerance never settles at 0.
        #[test]
        fn the_motion_stays_within_the_tolerance_from_its_settle_time(
            damping_ratio in prop_oneof![
                0.05..=20.0,
                0.99..1.01,
                (-1e-5..1e-5).prop_map(|nudge: f64| 1.0 + nudge),
                Just(1.0),
            ],
            stiffness in (0.0..=5.0_f64).prop_map(|decades| 10.0_f64.powf(decades)),
            x0 in prop_oneof![-2e5..2e5, -1.0..1.0, Just(0.0)],
            v0 in prop_oneof![-1e4..1e4, Just(0.0)],
            tolerance in prop_oneof![Just(0.5), Just(0.25), Just(0.5 / 255.0), 1e-3..1.0],
            shares in prop::collection::vec(0.0..3.0, 1..32),
        ) {
            let motion = Oscillator::new(damping_ratio, stiffness, x0, v0);
            let settle = motion.settle_time(tolerance);
            prop_assert!(settle.is_finite() && settle >= 0.0, "{settle}");
            prop_assert!(x0.abs() <= tolerance || settle > 0.0, "{settle}");
            // Up to three times the settle time past it, or 3/(ζω) for a motion that settles at
            // once.
            let span = settle.max(1.0 / (damping_ratio * stiffness.sqrt()));
            for t in shares.iter().map(|share| settle + share * span).chain([settle]) {
                let offset = motion.offset(t);
                prop_assert!(offset.abs() <= tolerance, "{offset} at {t}, settle {settle}");
            }
        }
    }

    /// The bounds' crossings are safe and tight: the bound is at or under the level there, and
    /// above it a microsecond earlier.
    #[test]
    fn crossings_are_safe_and_tight() {
        let level = 0.5;
        let cases = [
            (1.0, 0.0, 5.0),
            (0.0, 3000.0, 28.0),
            (2e5, 2e5 * 7.0, 7.0),
            (3.0, 1e-9, 2.0),
        ];
        for (a, b, rate) in cases {
            let bound = |t: f64| (a + b * t) * (-rate * t).exp();
            let t = linear_exponential_settle(a, b, rate, level);
            assert!(t > 0.0, "{a} {b} {rate}");
            assert!(bound(t) <= level, "{a} {b} {rate}: {}", bound(t));
            assert!(bound(t - 1e-6) > level * (1.0 - 2e-9), "{a} {b} {rate}");
        }
        let cases: [((f64, f64), (f64, f64)); 3] = [
            ((1.0, 1.0), (-2.0, 30.0)),
            ((5e4, 0.6), (0.0, 80.0)),
            ((-3.0, 2.0), (4.0, 2.5)),
        ];
        for ((c1, r1), (c2, r2)) in cases {
            let bound = |t: f64| c1.abs() * (-r1 * t).exp() + c2.abs() * (-r2 * t).exp();
            let t = two_exponentials_settle((c1, r1), (c2, r2), level);
            assert!(t > 0.0);
            assert!(bound(t) <= level, "{c1} {c2}: {}", bound(t));
            assert!(bound(t - 1e-6) > level * (1.0 - 2e-9), "{c1} {c2}");
        }
    }

    /// A bound that never reaches the level gives 0, never a negative time.
    #[test]
    fn a_bound_under_the_level_settles_at_once() {
        assert_eq!(linear_exponential_settle(0.1, 0.0, 3.0, 0.5), 0.0);
        assert_eq!(linear_exponential_settle(0.0, 1.0, 28.0, 0.5), 0.0);
        assert_eq!(two_exponentials_settle((0.2, 1.0), (0.2, 9.0), 0.5), 0.0);
    }

    /// When Newton's steps say nothing, bisection alone still ends on the safe side of the
    /// crossing.
    #[test]
    fn a_search_without_a_slope_bisects_to_the_safe_side() {
        let t = first_time_under(|t| (1.0 - t, 0.0), 0.0, 1e6, 0.0);
        assert!((1.0..1.0 + 1e-9).contains(&t), "{t}");
    }

    /// When the iterations run out, the search keeps its safe side, however wide the bracket.
    #[test]
    fn a_search_that_runs_out_keeps_its_safe_side() {
        // NaN everywhere: every step bisects, and no time is ever seen at or under 0.
        let t = first_time_under(|_| (f64::NAN, f64::NAN), 0.0, 7.0, 0.0);
        assert_eq!(t, 7.0);
    }
}
