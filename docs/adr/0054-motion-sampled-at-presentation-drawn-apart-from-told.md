# 0054. Motion sampled at its presentation; what is drawn apart from what a client is told

- Status: Accepted; amends [0023](0023-frames-presentation-and-the-clock.md) (frames during
  motion, hidden windows), [0038](0038-captures-coordinates-windows-and-wayland.md) (captures
  and clicks during motion), [0033](0033-the-mcp-server.md) (windows' drawn rectangles),
  [0051](0051-emulated-vrr-as-the-monitors-traces-showed.md) (a VRR refresh with nothing new during
  motion)
- Date: 2026-11-14

## Context

[ADR-0023](0023-frames-presentation-and-the-clock.md) draws frames on damage and arms nothing at
rest; [ADR-0031](0031-frames-drawn-ahead-of-their-boundary.md) draws a frame ahead of the
boundary it aims at, or at once on an idle output. ADR-0023 hides, by `suspended` and a
callback a second, windows outside their output; that decision, today, also sets `enter` and
`leave`, the pointer's focus and which feedback is discarded, from the layout's rectangles. With
motion, a window's drawn rectangle and its place differ for a few hundred milliseconds. niri
samples animations at the frame's predicted presentation and holds the pointer's focus during a
transition; Hyprland ticks on the wall clock ([survey](../survey.md#animations)).

## Decision

- **Each frame samples every motion at the boundary it aims at**, the time it is presented when
  on time, so that steps are even whatever the moment drawing starts within the period. A frame
  drawn at once on an idle output samples the boundary it can reach after its expected drawing,
  at most one period as the lead is ([ADR-0031](0031-frames-drawn-ahead-of-their-boundary.md)),
  not the first one after now. While a motion runs, each frame samples a later time than the
  frame before: a frame drawn at once that shows sooner than expected is followed by one that
  samples the boundary after the one it sampled, never an earlier one.
- **A motion starts when Yuna applies the change**, at the refresh that arranges it, so that its
  first frame shows less than two periods of motion.
- **While a motion of an output is unfinished at the time a refresh sampled**, that refresh arms
  the next one, whether it drew or found nothing changed, and a refresh whose drawing failed
  too. The last frame shows the target exactly; then nothing is armed.
- **On a VRR output, a motion's refresh that finds nothing changed shows its frame again**, so that
  the panel refreshes where the frame loop counts one and the motion keeps the shortest refresh
  to its end. At rest, such a refresh still flips nothing
  ([ADR-0051](0051-emulated-vrr-as-the-monitors-traces-showed.md)).
- **What is drawn follows the motion; what a client is told follows its place.** A window is told
  it is visible (`suspended` cleared, `enter`) when its place enters the output, and hidden only
  when its output's motions have ended with neither its place nor its drawn rectangle on it. A
  window that only passes through the view is drawn from its last buffer and told nothing. Each
  window gets at most one such change per operation. Frame callbacks go at full rate to every
  window a frame draws, told or not, so that what shows keeps updating.
- **The pointer and clicks** hit-test what is drawn, at each pointer event; between events, the
  pointer's focus is refreshed once when its output's motions end, not at each frame.
- **Captures** cut what their frame drew. MCP's `windows` gives a window's place, its drawn
  rectangle and when its motion ends ([ADR-0033](0033-the-mcp-server.md)).
- **Popups** follow their window's drawn rectangle; keeping them on the output uses its place.

## Alternatives considered

- **Sampling at the time drawing starts**: steps jitter by the drawing's start within the
  period.
- **Telling clients from the drawn rectangle**: a window crossing the edge, retargeted ten times
  a second, gets 10 to 20 configures a second; a jump across 100 columns configures nearly every
  window twice in half a second.
- **The pointer re-tested at every frame**: `enter`, `leave` and `motion` up to 240 times a second
  while windows slide under a still pointer.

## Consequences

- During a motion, the pointer's focus may name a window that has slid from under it until the
  pointer moves or the motion ends; a click always reaches what it shows.
- At Stage 2, the DRM backend commits the unchanged buffer at those refreshes: amdgpu counts a
  commit of the primary plane with a buffer as a flip (`amdgpu_dm.c`, read on 2026-11-14).
- A film on a VRR output loses its exact multiples while a motion runs there; Stage 5 runs motion
  at a multiple of the film's rate.
