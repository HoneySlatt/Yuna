//! Curves: [`Easing`], [`CurveParams`] and [`Curve`].

use std::ops::RangeInclusive;
use std::time::Duration;

use crate::{InvalidParams, finite_target, saturate, seconds_between};

/// The durations a curve may be given.
const DURATIONS: RangeInclusive<Duration> = Duration::from_millis(1)..=Duration::from_secs(10);

/// How finely a cubic Bézier's parameter is solved: far under the 1e-7 its output needs.
const BEZIER_PRECISION: f64 = 1e-12;

/// The largest |y1| or |y2| a Bézier may have: far beyond any curve worth drawing, and far below
/// where its polynomial's terms overflow into infinities of opposite signs, whose sum is NaN.
const MAX_BEZIER_Y: f64 = 1e6;

/// The most Newton steps a cubic Bézier takes before bisection takes over.
const BEZIER_NEWTON_STEPS: usize = 8;

/// How a curve's progress maps to its output: a function from 0 at p = 0 to 1 at p = 1, where p
/// is the share of the duration gone; [`Easing::EaseOutExpo`] alone falls short of 1 at p = 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Easing {
    /// At constant speed: p.
    Linear,
    /// Slows to a stop: 1 − (1 − p)².
    EaseOutQuad,
    /// Slows to a stop, later than the quadratic: 1 − (1 − p)³.
    EaseOutCubic,
    /// Covers most of the way at once, then a long tail: 1 − 2^(−10p). At p = 1 it is short of 1
    /// by 2^(−10), about a thousandth, and the curve then steps to its target, where it ends
    /// exactly: a step under half a level of an 8-bit alpha, but about a pixel on a 1,000-unit
    /// move.
    EaseOutExpo,
    /// CSS's `cubic-bezier(x1, y1, x2, y2)`: the curve through the control points (0, 0),
    /// (x1, y1), (x2, y2) and (1, 1), whose x is the progress and y the output. y may leave
    /// 0..=1, so that the motion passes its target and comes back.
    CubicBezier {
        /// The first control point's x, in 0..=1.
        x1: f64,
        /// The first control point's y.
        y1: f64,
        /// The second control point's x, in 0..=1.
        x2: f64,
        /// The second control point's y.
        y2: f64,
    },
}

impl Easing {
    /// The output at progress `p`, in 0..=1.
    fn output(&self, p: f64) -> f64 {
        match *self {
            Easing::Linear => p,
            Easing::EaseOutQuad => 1.0 - (1.0 - p).powi(2),
            Easing::EaseOutCubic => 1.0 - (1.0 - p).powi(3),
            Easing::EaseOutExpo => 1.0 - (-10.0 * p).exp2(),
            Easing::CubicBezier { x1, y1, x2, y2 } => bezier(y1, y2, bezier_parameter(x1, x2, p)),
        }
    }

    /// Whether CSS accepts it: a Bézier's x1 and x2 in 0..=1, so that its x rises from 0 to 1 and
    /// gives each progress one output; and Yuna, its y1 and y2 within ±[`MAX_BEZIER_Y`], so that
    /// its output stays finite. A NaN is refused by both comparisons.
    fn is_valid(&self) -> bool {
        match *self {
            Easing::CubicBezier { x1, y1, x2, y2 } => {
                (0.0..=1.0).contains(&x1)
                    && (0.0..=1.0).contains(&x2)
                    && y1.abs() <= MAX_BEZIER_Y
                    && y2.abs() <= MAX_BEZIER_Y
            }
            Easing::Linear | Easing::EaseOutQuad | Easing::EaseOutCubic | Easing::EaseOutExpo => {
                true
            }
        }
    }
}

/// What sets a curve's motion: its easing and its duration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveParams {
    easing: Easing,
    duration: Duration,
}

impl CurveParams {
    /// Parameters known to be in their range, for the crate's defaults, which a test checks
    /// against [`CurveParams::new`]: a constant cannot call it.
    pub(crate) const fn known_valid(easing: Easing, duration: Duration) -> CurveParams {
        CurveParams { easing, duration }
    }

    /// The same easing over half the duration, at least 1 ms: a fade under reduced motion
    /// ([ADR-0059](../../../docs/adr/0059-reduced-motion-nothing-slides-fades-stay.md)).
    pub fn halved(self) -> CurveParams {
        CurveParams {
            duration: (self.duration / 2).max(*DURATIONS.start()),
            ..self
        }
    }

    /// A curve's parameters. Refused (Err) unless the duration is 1 ms..=10 s, a Bézier's x1 and
    /// x2 are in 0..=1, and its y1 and y2 within ±1,000,000.
    pub fn new(easing: Easing, duration: Duration) -> Result<CurveParams, InvalidParams> {
        if easing.is_valid() && DURATIONS.contains(&duration) {
            Ok(CurveParams { easing, duration })
        } else {
            Err(InvalidParams)
        }
    }
}

/// A value that moves along an easing over a fixed duration: a fade's alpha.
///
/// Its output is not clamped: a Bézier whose y leaves 0..=1 takes the value past its target and
/// back, and a caller that needs a range, an alpha's 0..=1, clamps it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    easing: Easing,
    /// The pace that [`Curve::retarget`] keeps through a chain of retargets.
    pace: Pace,
    /// This curve's duration: the full duration, or less after a retarget.
    duration: Duration,
    from: f64,
    to: f64,
    /// `to − from`, finite.
    distance: f64,
    start: Duration,
    end: Duration,
}

/// A curve's pace: the distance a whole move covers in the full duration.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pace {
    /// The duration of a whole move, the one [`CurveParams`] gave.
    full_duration: Duration,
    /// The distance a whole move covers: the first curve's |to − from|, or, when that one did
    /// not move, the distance of the first retarget that does; 0 until then.
    full_distance: f64,
}

impl Curve {
    /// A curve from `from` to `to`, starting at `start`, over the duration of `params`; at once
    /// if `from == to`, since it has nothing to draw.
    ///
    /// A start or target that is not finite, or so far apart that their distance overflows,
    /// gives a curve at its target at once (`end() == start`): never a panic, never NaN out. A
    /// target that is not finite is replaced by `from`, or by 0 when neither is.
    pub fn new(params: CurveParams, from: f64, to: f64, start: Duration) -> Curve {
        let pace = Pace {
            full_duration: params.duration,
            full_distance: 0.0,
        };
        Curve::over(params.easing, pace, params.duration, from, to, start)
    }

    /// A curve over `duration`, which [`Curve::retarget`] gives below the shortest that
    /// [`CurveParams`] accepts, or at once if it does not move. A pace without a distance takes
    /// this curve's.
    fn over(
        easing: Easing,
        pace: Pace,
        duration: Duration,
        from: f64,
        to: f64,
        start: Duration,
    ) -> Curve {
        let distance = to - from;
        let (from, to, distance) = if distance.is_finite() {
            (from, to, distance)
        } else {
            let target = finite_target(from, to);
            (target, target, 0.0)
        };
        // A curve that does not move has nothing to draw: it ends at once.
        let duration = if distance == 0.0 {
            Duration::ZERO
        } else {
            duration
        };
        let pace = if pace.full_distance == 0.0 {
            Pace {
                full_distance: distance.abs(),
                ..pace
            }
        } else {
            pace
        };
        Curve {
            easing,
            pace,
            duration,
            from,
            to,
            distance,
            start,
            end: start.saturating_add(duration),
        }
    }

    /// The value at `at`: `from` up to the start, exactly the target from [`Curve::end`] on, and
    /// `from + (to − from)·easing(p)` between, p being the share of the duration gone. Not
    /// clamped; values beyond `f64`'s range, which only absurd inputs reach, stop at its largest
    /// finite number.
    pub fn value(&self, at: Duration) -> f64 {
        if at >= self.end {
            self.to
        } else if at <= self.start {
            self.from
        } else {
            let progress = seconds_between(self.start, at) / self.duration.as_secs_f64();
            saturate(self.from + self.distance * self.easing.output(progress))
        }
    }

    /// The value the curve moves to.
    pub fn target(&self) -> f64 {
        self.to
    }

    /// When it ends: from then on it is exactly at its target. The start plus the duration; the
    /// start itself for a curve that does not move; [`Duration::MAX`] when the sum would
    /// overflow.
    pub fn end(&self) -> Duration {
        self.end
    }

    /// Whether it has ended at `at`: `at >= end()`.
    pub fn is_done(&self, at: Duration) -> bool {
        at >= self.end
    }

    /// Goes on from this curve's value at `at` toward `to`, with the same easing, at the pace
    /// its parameters set
    /// ([ADR-0053](../../../docs/adr/0053-springs-in-closed-form-keeping-their-velocity.md)):
    /// over the full duration that [`CurveParams`] gave, times the share of the original
    /// distance that |to − value(at)| is, and at most the full duration.
    ///
    /// The original distance is |to − from| of the curve [`Curve::new`] made, kept through any
    /// chain of retargets, so that a fade toggled again and again neither speeds up nor slows
    /// down. A curve made without moving (from = to) has no pace: its first retarget takes the
    /// full duration, and the distance that one covers is the original distance from then on.
    ///
    /// A fade reversed half way back thus takes half its duration to return. The value does not
    /// jump at `at`; its speed may, since the easing starts again. A time before this curve's
    /// start counts as its start; a `to` that is not finite gives a curve at rest at once, as
    /// [`Curve::new`] does.
    pub fn retarget(&self, at: Duration, to: f64) -> Curve {
        let at = at.max(self.start);
        let value = self.value(at);
        let distance_left = (to - value).abs();
        let share = if self.pace.full_distance == 0.0 || !distance_left.is_finite() {
            1.0
        } else {
            (distance_left / self.pace.full_distance).min(1.0)
        };
        let duration = self.pace.full_duration.mul_f64(share);
        Curve::over(self.easing, self.pace, duration, value, to, at)
    }
}

/// One coordinate of the cubic Bézier from 0 to 1 whose two middle control points have this
/// coordinate at `a` and `b`, at parameter `t`: 3a·(1 − t)²·t + 3b·(1 − t)·t² + t³, in Horner's
/// form.
fn bezier(a: f64, b: f64, t: f64) -> f64 {
    (((1.0 + 3.0 * (a - b)) * t + 3.0 * (b - 2.0 * a)) * t + 3.0 * a) * t
}

/// The slope of [`bezier`] in `t`.
fn bezier_slope(a: f64, b: f64, t: f64) -> f64 {
    (3.0 * (1.0 + 3.0 * (a - b)) * t + 6.0 * (b - 2.0 * a)) * t + 3.0 * a
}

/// The parameter t in 0..=1 at which the Bézier's x, through `x1` and `x2` in 0..=1, is `p`.
///
/// With x1 and x2 in 0..=1, x rises from 0 to 1, so there is one such t. Newton's method from
/// t = p finds it in a few steps; where x is too flat for it (its slope vanishes at an end when
/// x1 or x2 is 0 or 1), bisection does.
///
/// Where x's slope vanishes inside the curve (x1 = 1 and x2 = 0, at p = 0.5), t is as precise as
/// `f64` allows, not more: x there is 0.5 + 4(t − 0.5)³, so a rounding of x by 10⁻¹⁶ moves t by
/// up to about 5·10⁻⁶. The output is exact at p = 0.5 itself, and within 10⁻⁷ from about 10⁻¹²
/// of progress away from it.
fn bezier_parameter(x1: f64, x2: f64, p: f64) -> f64 {
    let mut t = p;
    for _ in 0..BEZIER_NEWTON_STEPS {
        let error = bezier(x1, x2, t) - p;
        if error == 0.0 {
            return t;
        }
        let step = error / bezier_slope(x1, x2, t);
        // A slope of 0 gives an infinite step.
        if step.is_nan() || step.abs() >= 1.0 {
            break;
        }
        t -= step;
        if !(0.0..=1.0).contains(&t) {
            break;
        }
        if step.abs() <= BEZIER_PRECISION {
            return t;
        }
    }
    let (mut low, mut high) = (0.0, 1.0);
    while high - low > BEZIER_PRECISION {
        let middle = low + (high - low) / 2.0;
        if bezier(x1, x2, middle) < p {
            low = middle;
        } else {
            high = middle;
        }
    }
    low + (high - low) / 2.0
}
