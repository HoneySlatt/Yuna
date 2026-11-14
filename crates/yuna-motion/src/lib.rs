//! Yuna's motion: values that move over time, on springs for positions and on curves for fades,
//! each computed in closed form from its start alone, at any time. It holds no Wayland or Smithay
//! object and reads no clock: time is a [`Duration`] since any origin (Yuna's clock gives one),
//! and a value is an `f64` in any unit, logical units or an alpha.
//! [ADR-0052](../../../docs/adr/0052-motion-in-a-pure-crate.md) makes this crate the home of
//! Yuna's motion; today it holds its two pieces, [`Spring`] and [`Curve`].
//!
//! # Springs
//!
//! A [`Spring`] moves a value toward a target as a mass of 1 on a damped spring
//! ([ADR-0053](../../../docs/adr/0053-springs-in-closed-form-keeping-their-velocity.md)). Its
//! offset from the target, x, solves
//!
//! ```text
//! x'' + 2ζω·x' + ω²·x = 0
//! ```
//!
//! where ζ is the damping ratio and ω² the stiffness ([`SpringParams`]): ADR-0053's
//! `x'' + c·x' + k·x = 0`, with k = ω² and c = 2ζω. Below ζ = 1 the value passes its target and
//! rings; at 1 it returns as fast as it can without passing it; above 1 it creeps in. Each regime
//! has a closed form for the value and the velocity, written so that it keeps its precision as ζ
//! nears 1 and never overflows however long the spring runs; the private `oscillator` module
//! derives them. A ratio near 1 is computed as itself, not rounded to 1: rounding 1 + 10⁻⁶ to 1
//! moves a spring of stiffness 50 launched at 3,000 units per second by about 10⁻⁴ units, a
//! hundred times what the test against a numerical integration (`tests/spring.rs`) allows.
//!
//! ## When a spring ends
//!
//! A spring never reaches its target in finite time, so it ends when its motion can no longer be
//! seen: when its distance to the target stays under a tolerance the caller gives, half a
//! physical pixel of the output it is drawn on, in the value's units. From [`Spring::end`] on, it
//! is exactly at its target, and nothing more needs drawing: at rest, nothing runs. Half a pixel
//! rather than a share of the distance, which has no unit: a share is too fine for short moves
//! and too coarse for long ones, while half a pixel is what can be seen.
//!
//! The end is solved once, when the spring is made, from bounds of its distance to the target
//! that hold at every later time (envelopes):
//!
//! - in every regime, `(|x0| + |v0 + λ·x0|·t)·e^(−λt)`, from the starting offset x0 and velocity
//!   v0, with λ the slowest decay rate: the distance itself for a critical spring from rest,
//!   close to it near ζ = 1;
//! - below critical damping, the oscillation's amplitude, decaying as `e^(−ζωt)`;
//! - above it, the sum of the two decaying terms' sizes.
//!
//! The end is the first time from which one of them stays under the tolerance. The true motion
//! is thus never more than the tolerance from the target after the end, and the jump to it
//! cannot be seen. A bound that ignores a factor ends too early: the envelope `e^(−ωt)` alone,
//! which niri and libadwaita use, leaves a critical spring about 2 physical pixels short of a
//! 1,280-unit move at scale 1.5, then jumps ([survey](../../../docs/survey.md#animations)).
//!
//! Where a bound has no closed form, Newton's method solves it, kept within a bracket of times by
//! bisection. If it has not converged in 100 steps, the end is the last time known to be safe:
//! never zero, which would jump, and never infinite, which would never rest. A spring whose bound
//! stays under the tolerance ends at once; a spring that starts at its target with a velocity
//! still moves, and ends later, when that velocity takes it farther than the tolerance.
//!
//! The bounds are tight from rest at and above critical damping; a spring launched toward its
//! target, above critical damping, may end several times later than its last visible move. They
//! are loose by up to about half an
//! oscillation below it, where the amplitude decays between peaks that the motion does not
//! reach. Over the grid of `tests/spring.rs`, the end comes at most 1.3 times as late as the last
//! time the motion is farther than the tolerance (ζ = 0.5), and at most 1.005 times as late
//! from ζ = 1 on. [`SpringParams::time_to_rest`] gives the end of a spring from rest, for
//! ADR-0053's check that refuses a spring taking more than 1.5 s across the largest output's
//! diagonal.
//!
//! ## Interrupted
//!
//! [`Spring::retarget`] starts a spring from the old one's value and velocity at that time,
//! toward the new target: the exact motion of the spring's equation with its target switched
//! then, which neither jumps nor changes speed. A spring with ζ ≥ 1 never passes its target from
//! rest; after a change of mind it may, by the velocity it carried.
//!
//! # Curves
//!
//! A [`Curve`] moves a value from a start to a target over a fixed duration, along an
//! [`Easing`]: linear, three named ease-outs, or a cubic Bézier in CSS's convention, solved by
//! Newton's method with bisection behind it, with no table. Interrupted, it goes on from its
//! value at the pace its parameters set: the full duration times the share of the original
//! distance that is left, however often it is interrupted ([`Curve::retarget`]). A curve that
//! does not move ends at once. Its value is not clamped: a caller clamps an alpha to 0..=1.
//!
//! # Bad input
//!
//! Parameters out of their range are refused with [`InvalidParams`]. Values that are not finite
//! give a motion at its target at once. Nothing here panics or returns NaN.
//!
//! [`Duration`]: std::time::Duration

mod curve;
mod oscillator;
mod spring;

use std::fmt;
use std::time::Duration;

pub use curve::{Curve, CurveParams, Easing};
pub use spring::{Spring, SpringParams};

/// Parameters out of their range, or not finite: what [`SpringParams::new`] and
/// [`CurveParams::new`] refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidParams;

impl fmt::Display for InvalidParams {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("motion parameters out of their range")
    }
}

impl std::error::Error for InvalidParams {}

/// What stands for a target that is not finite: the start if it is finite, else 0.
fn finite_target(from: f64, to: f64) -> f64 {
    if to.is_finite() {
        to
    } else if from.is_finite() {
        from
    } else {
        0.0
    }
}

/// The seconds from `start` to `at`; 0 if `at` is earlier.
fn seconds_between(start: Duration, at: Duration) -> f64 {
    at.saturating_sub(start).as_secs_f64()
}

/// `value` with an infinity replaced by the largest finite number of its sign. Never given NaN.
fn saturate(value: f64) -> f64 {
    value.clamp(f64::MIN, f64::MAX)
}
