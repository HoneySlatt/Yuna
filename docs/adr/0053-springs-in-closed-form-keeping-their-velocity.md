# 0053. Springs in closed form, keeping their velocity; curves

- Status: Accepted
- Date: 2026-11-14

## Context

[ADR-0010](0010-animations-and-effects.md) puts motion on springs that keep their velocity when
interrupted, and fades on curves. niri and libadwaita evaluate a spring in closed form and end
it on an envelope that ignores a term, so niri stops long moves pixels short and jumps; niri
restarts an interrupted camera at velocity 0. Hyprland and SwiftUI keep the velocity
([survey](../survey.md#animations)). A spring that bounces near 1 never rests, against
[AGENTS.md](../../AGENTS.md)'s "at rest, nothing runs".

## Decision

The owner chose, on 2026-11-14, to keep the velocity; the rest follows from it:

- **The spring** is `x'' + c x' + k x = 0`, mass 1, set by its damping ratio and stiffness,
  evaluated in closed form at any time from its start, value and velocity alike. Overdamped
  springs are written with decaying exponentials only, so that nothing overflows, and every
  form stays precise as the ratio nears 1, without rounding it to 1.
- **Interrupted**, a spring starts again from its value and velocity at that time, toward the new
  target. A spring without bounce does not overshoot from rest; after a change of mind it may,
  by the velocity it carried.
- **It ends when it can no longer move a pixel**: the time after which a bound of its distance to
  the target stays under half a physical pixel of its output, solved once when it starts, with
  Newton's method where there is no closed form, and an upper bound, never zero, if that fails.
  At that time it is exactly at its target, and nothing more is drawn.
- **A spring that would take more than 1.5 s** across the largest output's diagonal is refused
  as a configuration error, which never stops Yuna
  ([ADR-0008](0008-configuration-in-kdl.md)).
- **Curves** are cubic Béziers in CSS's convention, with `linear`, `ease-out-quad`,
  `ease-out-cubic` and `ease-out-expo` named, over a duration; solved by Newton's method kept
  within a bracket, no table. An interrupted curve starts again from its value, over the share of its duration that the
  distance left takes. An alpha stays within 0 and 1.
- **What users write** is decided with the configuration file (Stage 2): damping ratio and
  stiffness, SwiftUI's duration and bounce, or both.

## Alternatives considered

- **Restarting at velocity 0** (niri): simpler, no overshoot, but motion breaks at each
  interruption and a held key lags further behind: 3.2 columns at 25 repeats a second, against
  2.3 keeping the velocity.
- **Stopping at the first arrival** (libadwaita's clamp): no overshoot, but a visible stop.
- **Stepping by the elapsed time** (Hyprland, KWin): as exact when stepped exactly, but a value
  depends on the frames before it; closed form gives any frame's value from the start alone.
- **Ending on an epsilon of the distance**: a number with no unit, too fine for short moves and
  too coarse for long ones; half a pixel is what can be seen.

## Consequences

- Change your mind quickly, and the camera passes its last target, by up to about a fifth of a
  column, then comes back.
- Each spring is checked against a numerical integration of its equation, retargets included.
