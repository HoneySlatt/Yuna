//! Yuna's motion: values that move over time, on springs for positions and on curves for fades,
//! each computed in closed form from its start alone, at any time; and the motion of the
//! layout's arrangements built on them, [`Motion`]. It holds no Wayland or Smithay object and
//! reads no clock: time is a [`Duration`] since any origin (Yuna's clock gives one), and a value
//! is an `f64` in any unit, logical units or an alpha.
//! [ADR-0052](../../../docs/adr/0052-motion-in-a-pure-crate.md) makes this crate the home of
//! Yuna's motion. Its pieces: [`Spring`] and [`Curve`], then [`Motion`], which the [last
//! section](#the-motion-of-an-arrangement) explains.
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
//! # The motion of an arrangement
//!
//! The layout model ([`yuna_layout`]) jumps: an operation moves it to its new state at once, and
//! [`Layout::arrange`] gives each output's windows at their new places. [`Motion`] turns those
//! jumps into motion ([ADR-0052](../../../docs/adr/0052-motion-in-a-pure-crate.md)). The caller
//! gives it the layout after each change ([`Motion::update`]), and at each frame asks what an
//! output draws at the time that frame is presented ([`Motion::frame`],
//! [ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)),
//! until the output's motions end ([`Motion::end`]): then the frame is exactly the arrangement,
//! and nothing more is drawn. What moves, and on which spring or curve, is
//! [ADR-0055](../../../docs/adr/0055-what-moves-first-and-animations-off-headless.md)'s, set by
//! [`Settings`].
//!
//! ## A jump becomes an offset
//!
//! At each update, a window drawn before and after gets an offset: where it was drawn at the
//! update's time minus where its new place, and its camera, draw it. The offset decays to 0 on
//! the windows' spring, from the velocity the window had, so that nothing jumps on screen at an
//! operation and a motion interrupted goes on without a break. A window keeps its motion while
//! it stays in its container: its workspace and its layer ([`Layer`]); going from floating to
//! tiled or back within its workspace keeps it too, through screen positions; a pinned window
//! stays pinned whatever workspace a switch gives it. A window that changes workspace, output or
//! layer otherwise, or goes fullscreen or leaves it, is drawn at its new place at once, and so
//! is a new window; a window gone is dropped. Only positions move: a window is drawn at the size
//! of its place, which jumps.
//!
//! ## The camera and its anchor
//!
//! A scrolling workspace's strip is seen through a camera, the strip position shown at the
//! working area's left edge, which moves on its own spring toward the layout's view
//! ([`WorkspaceView`]): focusing another column moves the camera alone. A strip window is drawn
//! at its place plus its offset plus the camera's lag, the view's place minus the camera.
//!
//! An operation may also move the strip under the view: a column left of the focused one that
//! widens or closes shifts every column on its right, and the view by as much, so that they stay
//! on screen. The camera is first moved by the same shift, keeping its velocity, so that it
//! keeps looking at the same columns; then it moves toward the new view. The shift is read from
//! an anchor, a column there before and after that the change left in place on screen: the
//! focused one, else the one nearest it. The camera then stays still, so that what the change
//! left in place does not move, whatever the springs of the camera and of the windows. When no
//! column stayed in place, as when the view moves to another column, the anchor is the focused
//! column after the change, if it was there before; else the one focused before, if it is there
//! after; else there is no anchor, and no shift. A focused column is the column of the strip's window that has the seat's focus,
//! kept while the focus is elsewhere. Whatever the camera does not take up, the offsets do: the
//! split between them decides how things move, never whether they jump.
//!
//! ## Rounded once
//!
//! A drawn rectangle is the place moved by every motion, its corner rounded once to the output's
//! physical grid, half a pixel going up, as the layout rounds its places. Still columns thus move
//! as one rigid strip, and a window moving over a moving camera errs by half a pixel at most. An
//! axis that does not move is exactly the place's. Every spring ends within half a physical
//! pixel of the output, so that the last steps move one pixel at most.
//!
//! ## The workspace switch
//!
//! When an output's main workspace changes, by its id, and the one left is still on the output,
//! the output slides along [ADR-0018](../../../docs/adr/0018-workspaces.md)'s vertical stack: up
//! is the previous workspace, down the next. The switch keeps a stack of its own, the
//! workspaces it involves in the output's order, a height apart: the one left and the one shown,
//! not those between, so that a far switch shows only those two. Retargeted mid-way, it inserts
//! the newly shown workspace into its stack and slides on from where it is, keeping its
//! velocity; the workspace on screen nearest the view keeps its place. A workspace gone from the
//! output leaves the stack, the position re-expressed so that the nearest of those left keeps
//! its place, and both on screen when the one gone was not between them. A workspace removed
//! elsewhere changes no id, and starts no switch. While a switch is drawn, a frame lists
//! the windows of its workspaces that show on the output; it is drawn until its slide ends and
//! the windows of the workspaces sliding out stop moving. Pinned windows stay above it, unmoved.
//!
//! ## The special workspace's fade
//!
//! A special workspace shown fades in, from alpha 0 to 1, on [`Settings::special_fade`], and
//! fades out when hidden, its windows drawn where they last were, until it ends or they close
//! or are shown elsewhere. A toggle mid-fade turns the curve from where it is. It is drawn above
//! the main workspace and below the pinned windows.
//!
//! ## Drawn and told
//!
//! What is drawn follows the motion; what a client is told follows its place
//! ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
//! [`Motion::told_visible`] gives the windows whose place overlaps the output, and while its
//! motions run, every window told since they began that it still draws: a window is told it is
//! visible once per operation, and hidden only once the motion has ended. A window that only
//! passes through the view, its place never on the output, is drawn but never told.
//!
//! ## Instant changes
//!
//! A change made [`Change::Instant`], or any change while [`Settings::off`] is set, ends every
//! motion of every output: its next frame is the arrangement. A change made
//! [`Change::InstantFor`] some seats ends the motions of their outputs alone, and is animated on
//! the others': one seat's outputs coming and going never cut another seat's motion short.
//! Changes made before one update combine ([`Change::and`]): instant wherever one of them is.
//! Whatever the change, an output added, moved or rescaled starts at rest, and one removed is
//! forgotten; a window going fullscreen or leaving it ends the motions of its workspace's camera
//! and offsets, pinned windows aside: a game wants its output at once.
//!
//! Under [`Settings::reduced_motion`], nothing crosses the screen: the switch, cameras and
//! offsets show their result at once, while the special workspace's fade stays, over half its
//! duration ([ADR-0059](../../../docs/adr/0059-reduced-motion-nothing-slides-fades-stay.md)).
//!
//! ## Cost
//!
//! [`Motion::update`] is linear in the windows of the workspaces each output draws: it arranges
//! them once, and finds each window's motion by hashing. While a special workspace fades out, it
//! also asks the layout where each of that workspace's windows is ([`Layout::window`]), so that
//! a window closed or shown elsewhere is no longer drawn; each answer costs a search of the
//! seat's workspaces. [`Motion::frame`] is linear in the output's drawn windows and allocates
//! nothing but the list it returns.
//!
//! ## Limits
//!
//! - A workspace sliding out of a switch, or fading out, keeps its camera as it was: the layout
//!   gives no view for a workspace it does not show. If the layout moves that view meanwhile,
//!   the windows whose places change reach them on their offsets.
//! - A workspace that leaves the output while it fills the view, an empty one that the layout
//!   removes or one that goes to another output, closes up the switch's stack: the workspace
//!   beside it comes into view at once.
//! - A switch slides by the output's height, so a window taller than its place, whose client
//!   committed more than it was asked, shows that much past its workspace's edge until the
//!   switch ends.
//! - Windows do not fade in when they open nor out when they close: that comes with the effects
//!   ([ADR-0055](../../../docs/adr/0055-what-moves-first-and-animations-off-headless.md)).
//!
//! [`Duration`]: std::time::Duration
//! [`Layout::arrange`]: yuna_layout::Layout::arrange
//! [`Layout::window`]: yuna_layout::Layout::window
//! [`Layer`]: yuna_layout::Layer
//! [`WorkspaceView`]: yuna_layout::WorkspaceView

mod arrangement;
mod curve;
mod oscillator;
mod spring;

use std::fmt;
use std::time::Duration;

pub use arrangement::{Change, Drawn, Frame, Motion, Settings};
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
