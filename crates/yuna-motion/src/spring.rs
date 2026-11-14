//! Springs: [`SpringParams`] and [`Spring`].

use std::ops::RangeInclusive;
use std::time::Duration;

use crate::oscillator::Oscillator;
use crate::{InvalidParams, finite_target, saturate, seconds_between};

/// The damping ratios accepted: from a spring that rings for long to one that crawls.
const DAMPING_RATIOS: RangeInclusive<f64> = 0.05..=20.0;

/// The stiffnesses accepted, in 1/s² (mass 1).
const STIFFNESSES: RangeInclusive<f64> = 1.0..=100_000.0;

/// What sets a spring's motion: its damping ratio ζ and its stiffness k, for a mass of 1.
///
/// ζ = 1 is the fastest return without passing the target; below 1 the spring passes it and
/// rings, above 1 it creeps in. The stiffness sets the speed: the spring's natural angular
/// frequency is ω = √k, in radians per second. niri's default for windows and the view is ζ = 1,
/// k = 800 ([survey](../../../docs/survey.md#animations)).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringParams {
    damping_ratio: f64,
    stiffness: f64,
}

impl SpringParams {
    /// Parameters known to be in their range, for the crate's defaults, which a test checks
    /// against [`SpringParams::new`]: a constant cannot call it.
    pub(crate) const fn known_valid(damping_ratio: f64, stiffness: f64) -> SpringParams {
        SpringParams {
            damping_ratio,
            stiffness,
        }
    }

    /// A spring's parameters. Refused (Err) unless finite, `damping_ratio` in 0.05..=20 and
    /// `stiffness` in 1..=100 000.
    pub fn new(damping_ratio: f64, stiffness: f64) -> Result<SpringParams, InvalidParams> {
        // `contains` is false for NaN, and the ranges hold no infinity.
        if DAMPING_RATIOS.contains(&damping_ratio) && STIFFNESSES.contains(&stiffness) {
            Ok(SpringParams {
                damping_ratio,
                stiffness,
            })
        } else {
            Err(InvalidParams)
        }
    }

    /// The damping ratio ζ.
    pub fn damping_ratio(&self) -> f64 {
        self.damping_ratio
    }

    /// The stiffness k, in 1/s².
    pub fn stiffness(&self) -> f64 {
        self.stiffness
    }

    /// How long a spring from rest takes to end over `distance` with `tolerance`: the life of
    /// `Spring::new(self, distance, 0.0, 0.0, start, tolerance)`. The sign of `distance` does not
    /// matter.
    ///
    /// [ADR-0053](../../../docs/adr/0053-springs-in-closed-form-keeping-their-velocity.md)
    /// refuses a spring that takes more than 1.5 s over the largest output's diagonal with a
    /// tolerance of half its physical pixel; this is the figure that check reads. Zero when the
    /// distance or the tolerance is not finite, or the tolerance not positive, as for
    /// [`Spring::new`].
    pub fn time_to_rest(&self, distance: f64, tolerance: f64) -> Duration {
        motion_and_life(*self, distance, 0.0, tolerance).map_or(Duration::ZERO, |(_, life)| life)
    }
}

/// A value that moves on a spring toward a target: a window's position, the view's.
///
/// It is computed in closed form from its start alone, at any time, value and velocity alike,
/// and ends at a time solved once, when it is made: from then on it is exactly at its target and
/// nothing more needs drawing. See the [crate's documentation](crate) for the equation, how the
/// end is decided, and what [`Spring::retarget`] keeps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    params: SpringParams,
    /// The motion around the target, from the start.
    motion: Oscillator,
    from: f64,
    target: f64,
    tolerance: f64,
    start: Duration,
    end: Duration,
}

impl Spring {
    /// A spring from `from` toward `to`, starting at `start` with `velocity`, in units per
    /// second.
    ///
    /// `tolerance` > 0 is the distance under which motion can no longer be seen: half a physical
    /// pixel, in the value's units. The spring ends once a bound of its distance to `to` stays
    /// under it; a spring whose bound never leaves it ends at once.
    ///
    /// Inputs that are not finite, a tolerance that is not positive, or numbers so large that
    /// they overflow give a spring at its target at once (`end() == start`): never a panic, never
    /// NaN out. A target that is not finite is replaced by `from`, or by 0 when neither is.
    pub fn new(
        params: SpringParams,
        from: f64,
        to: f64,
        velocity: f64,
        start: Duration,
        tolerance: f64,
    ) -> Spring {
        match motion_and_life(params, from - to, velocity, tolerance) {
            Some((motion, life)) => Spring {
                params,
                motion,
                from,
                target: to,
                tolerance,
                start,
                end: start.checked_add(life).unwrap_or(Duration::MAX),
            },
            None => {
                let target = finite_target(from, to);
                Spring {
                    params,
                    motion: Oscillator::new(params.damping_ratio, params.stiffness, 0.0, 0.0),
                    from: target,
                    target,
                    tolerance,
                    start,
                    end: start,
                }
            }
        }
    }

    /// The value at `at`: `from` up to the start, exactly the target from [`Spring::end`] on.
    /// Values beyond `f64`'s range, which only absurd inputs reach, stop at its largest finite
    /// number.
    pub fn value(&self, at: Duration) -> f64 {
        if at >= self.end {
            self.target
        } else if at <= self.start {
            self.from
        } else {
            let offset = self.motion.offset(seconds_between(self.start, at));
            saturate(self.target + offset)
        }
    }

    /// The velocity at `at`, in units per second: the starting velocity up to the start, 0 from
    /// [`Spring::end`] on.
    pub fn velocity(&self, at: Duration) -> f64 {
        if at >= self.end {
            0.0
        } else {
            saturate(self.motion.velocity(seconds_between(self.start, at)))
        }
    }

    /// The value the spring moves toward.
    pub fn target(&self) -> f64 {
        self.target
    }

    /// When it ends: from then on it is exactly at its target. Computed once, in
    /// [`Spring::new`]; never before the start, and finite: at worst [`Duration::MAX`], when the
    /// start is that close to it.
    pub fn end(&self) -> Duration {
        self.end
    }

    /// Whether it has ended at `at`: `at >= end()`.
    pub fn is_done(&self, at: Duration) -> bool {
        at >= self.end
    }

    /// A spring going on from this one's value and velocity at `at`, toward `to`, with the same
    /// parameters and tolerance
    /// ([ADR-0053](../../../docs/adr/0053-springs-in-closed-form-keeping-their-velocity.md)).
    ///
    /// The motion is the exact solution of the spring's equation with its target switched at
    /// `at`, so it does not jump or change speed there. A spring without bounce does not pass its
    /// target from rest, but it may after a retarget, by the velocity it carried. A time before
    /// this spring's start counts as its start; after its end, the spring is at rest at its
    /// target.
    pub fn retarget(&self, at: Duration, to: f64) -> Spring {
        let at = at.max(self.start);
        Spring::new(
            self.params,
            self.value(at),
            to,
            self.velocity(at),
            at,
            self.tolerance,
        )
    }

    /// The same motion moved by `by`: its start, its value at every time and its target are
    /// `by` farther, its velocity and its end are unchanged. What re-expressing a position in
    /// another frame needs, when the frame's origin moves by `by` and nothing on screen may.
    ///
    /// If `by` is not finite, or the moved values overflow, the spring is at rest at its own
    /// target at once instead: never NaN out.
    pub fn shifted(&self, by: f64) -> Spring {
        let (from, target) = (self.from + by, self.target + by);
        if from.is_finite() && target.is_finite() {
            Spring {
                from,
                target,
                ..*self
            }
        } else {
            Spring::new(
                self.params,
                self.target,
                self.target,
                0.0,
                self.start,
                self.tolerance,
            )
        }
    }
}

/// The motion from offset `x0` and velocity `v0` around a target, and how long it lasts before it
/// can no longer be seen with `tolerance`; `None` when an input is not finite, the tolerance is
/// not positive, or the numbers overflow.
fn motion_and_life(
    params: SpringParams,
    x0: f64,
    v0: f64,
    tolerance: f64,
) -> Option<(Oscillator, Duration)> {
    if !(tolerance > 0.0 && tolerance.is_finite()) {
        return None;
    }
    let motion = Oscillator::new(params.damping_ratio, params.stiffness, x0, v0);
    if !motion.is_finite() {
        return None;
    }
    Some((motion, duration_rounded_up(motion.settle_time(tolerance))))
}

/// A time in seconds, finite and not negative, rounded up to the nanosecond so that an end is
/// never early. Beyond `u64` nanoseconds (584 years), it stops there.
fn duration_rounded_up(seconds: f64) -> Duration {
    // `as` saturates a float beyond u64's range.
    Duration::from_nanos((seconds * 1e9).ceil() as u64)
}
