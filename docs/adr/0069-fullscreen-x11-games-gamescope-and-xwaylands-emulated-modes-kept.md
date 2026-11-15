# 0069. Fullscreen X11 games: gamescope, and Xwayland's emulated modes kept

- Status: Accepted; answers [0044](0044-xwayland-across-outputs-of-different-scales.md)'s question on games
- Date: 2026-11-14

## Context

[ADR-0044](0044-xwayland-across-outputs-of-different-scales.md) asked M1.8's design how a
fullscreen X11 game on an output of a smaller scale draws at that output's mode. With the main
output at 1.5 and another of 1920 × 1080 at 1, X11 sees the second as 2880 × 1620; a game at
that size draws 2.25 times the pixels the output shows.

Xwayland emulates mode changes: a fullscreen client that sets a smaller RandR or vidmode mode
gets a `wp_viewport` scaling its surface to the output, and its pointer converted. Xwayland 24.1
lists the output's current size first, then common modes no larger, 1920 × 1080 among them;
26.1 lists the native mode first. A surface gets its viewport only when its window sits at the
output's origin with exactly the emulated size; Xwayland puts the emulated rectangles in
`_XWAYLAND_RANDR_EMU_MONITOR_RECTS` on the client's windows. Upstream Wine sets real RandR modes
under X11; Proton has fullscreen upscaling of its own, not checked. gamescope runs a game on a
nested Xwayland whose only output is the game's resolution, and scales the result itself.

## Decision

The owner chose, on 2026-11-14:

- **gamescope, run inside Yuna, is the documented answer for a fullscreen X11 game on an output
  of a smaller scale**: the game draws at the resolution given to gamescope.
- **Yuna does not break Xwayland's emulation.** Its window manager configures a fullscreen window
  to the rectangle `_XWAYLAND_RANDR_EMU_MONITOR_RECTS` gives for its output, rather than to the
  output, and never moves a hidden fullscreen window. A game run without gamescope that sets the
  output's mode is then drawn pixel for pixel.

## Alternatives considered

- **Xwayland's emulation as the answer**: no cost when a game picks the output's mode, but a game
  left at the desktop's size pays the 2.25 times; kept as what Yuna does not break, not as the
  answer.
- **A second Xwayland at scale 1 for games**: games always at the output's mode, but small on the
  output at 1.5, Steam's windows too, two servers and a rule per application. In the
  [PLAN's ideas](../PLAN.md#ideas).

## Consequences

- A test sets the output's mode from an X11 client and checks the window drawn pixel for pixel.
- A game run without gamescope at the desktop's size costs 2.25 times the output's pixels at
  1.5 beside 1: stated in the documentation.
- Proton's own scaling with Yuna goes on the owner's checklist: it cannot be run headless.
