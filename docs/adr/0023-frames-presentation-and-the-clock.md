# 0023. Frames, presentation times and the clock

- Status: Accepted; amended by [0031](0031-frames-drawn-ahead-of-their-boundary.md) (when a frame
  is drawn, when its feedback is sent)
- Date: 2026-11-12

## Context

A virtual output has no vertical blank: Yuna decides when it refreshes. The quality bar measures
motion and input-to-present from presentation times read headless ([ROADMAP](../ROADMAP.md#3-quality-bar)),
so those times must be true. Nothing may run, poll or draw at rest. M1.5 steps the clock from MCP,
and M1.7 checks animations frame by frame on it. Compositors treat hidden windows differently: niri
sends them a frame callback about once a second, Hyprland none
([survey](../survey.md#hidden-workspaces)). xdg-shell has a `suspended` state that tells a client
its window cannot be seen.

## Decision

- **Frames follow damage.** A client's commit, or a change of the layout, arms one timer at the
  output's next refresh boundary. There, Yuna draws what changed, then sends the frame callbacks.
  The timer is armed again only while something waits. At rest, no timer is armed.
- **A presentation time is when the frame could have shown**: the first refresh boundary after its
  drawing finished, not the boundary where it began. The sequence number counts boundaries, so a
  frame that took too long shows as missed. Times are `CLOCK_MONOTONIC`.
- **The clock owns the deadlines.** Every timer of Yuna's frames asks the clock. In the program,
  the clock is the monotonic one, and it arms one calloop timer at the earliest deadline. In tests,
  time moves only when the test advances it, and every deadline passed then runs, in order. M1.5
  steps the same clock from MCP, and M1.6's variable refresh is another rule for the next deadline.
  Virtual time starts at the real monotonic time and never runs ahead of it while real clients are
  connected.
- **Hidden windows** get at most one frame callback a second and xdg-shell's `suspended` state.
  Hidden means on a workspace not shown, or in a column scrolled out of view. Windows on an agent's
  virtual output are shown, at full rate ([ADR-0015](0015-agents-workspaces.md)). The owner chose
  this on 2026-11-12.
- **The layout is arranged once per output frame**, and only after it changed, not at each commit.

## Alternatives considered

- **A free-running timer at the output's rate**: simple, but it wakes 240 times a second at rest.
- **Presentation at the boundary where drawing starts**: never shows a missed frame, so the
  motion budget measured headless would always pass.
- **A clock that only says the time**, its timers left to calloop: a test could move time but fire
  nothing, and M1.5 would rewrite the frame loop.
- **No callback for hidden windows** (Hyprland): no wake-up, but games and Vulkan clients that block
  on their next frame freeze, network and audio included.
- **Full rate for hidden windows**: frames drawn for nobody.

## Consequences

- A test can run the frame loop to an exact time and read what was drawn and presented.
- A hidden window that keeps asking for frames wakes Yuna once a second.
- llvmpipe at 4K may miss frames at 240 Hz; the presentation times say so, and the measures
  record it.
