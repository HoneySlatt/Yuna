# 0090. An output's mode and scale changed whole, through MCP

- Status: Accepted
- Date: 2026-11-18

## Context

M1.11 lets an agent make the situations it tests, among them a scale or a mode changed on a
monitor ([ADR-0061](0061-yunas-uses.md), [PLAN](../PLAN.md#m111--the-agents-toolbox)). Until
then a virtual output kept the mode it was created with: the layout model had no way to change
an output's rectangle or scale, and the output's buffer, damage tracker and refresh timing were
made once ([ADR-0023](0023-frames-presentation-and-the-clock.md)).
[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 7
asks Stage 2's core to apply an output change whole or not at all.

Two behaviours showed up while building it:

- After an output shrank, the layout fitted the strip to the clients' old committed widths, and
  its rule that the focused column stays put left a window at x = −304 though both columns fitted.
- Yuna passes a client's commit to the layout only when its size changes. A client that keeps
  its size, a fullscreen game at 1280 × 720 among them, would then never tell the layout that it
  answered the new mode.

## Decision

Chosen while building M1.11, as implementation within the milestone:

- **`outputs configure`** changes an output's physical size, rate, scale, VRR range and HDR; each
  value left out is the output's own; `vrr_min: 0` turns VRR off. `outputs create` is unchanged.
- **Applied whole or not at all.** Every check runs first: the output, the renderer's limit, the
  new buffer made, the layout's own bounds. Nothing can fail after them. Then the buffer, damage
  tracker, refresh timing and colour description are replaced, `wl_output` and `xdg_output` are
  told, every surface shown on the output learns its preferred scale, and captures waiting on it
  fail with a reason. A mode equal to the output's own changes nothing. Refresh numbers keep
  counting across the change.
- **The seat's outputs to the right move with its right edge**, so that none overlaps it. Other
  seats' outputs never move.
- **A window whose asked size the change altered counts at that asked size until its client
  answers.** The answer is the commit that acknowledges the new configure, passed on whatever its
  size; for X11, which acknowledges nothing, the first commit after the size is told. A client
  that never answers stays placed at its asked size.
- **X11**: Xwayland keeps its scale for its life
  ([ADR-0068](0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)); its windows are placed
  again in X11 pixels.

## Alternatives considered

- **Remove the output and create another**: no new code, but every window would move to another
  output and back, and clients would see an output leave, which no monitor's mode change does.
- **Keep the committed sizes, and recompute the view from the focused column**: no new rule, but
  the view would rest on sizes the clients are about to replace.
- **Pass every commit to the layout**: simplest, but a layout pass at every frame of every client.
- **Leave neighbouring outputs in place**: outputs would overlap, and the pointer's output would
  depend on their order.

## Consequences

- The layout model gains `configure_output` and `awaits_commit`, with their scenarios and property
  tests.
- `outputs configure` is part of the MCP contract
  ([ADR-0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)).
- The pointer keeps its logical place, as after a removal: it may sit on no output after a shrink.
  No X11 test drives a window that keeps its size. Both are in [debt](../debt.md).
- `State::configure_output` lives on today's shared `State`, beside `add_output`; Stage 2's core
  replaces it with an output configuration applied as one
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 7).

## Validation and reconsideration

Evidence: the layout's scenarios (a change smaller, larger, of scale, with fullscreen and floating
windows, refused); headless tests of a resize with a capture after it, a new rate on the manual
clock, VRR and HDR turned on and off, a refusal by the layout after the buffer was made, and a
fullscreen game that keeps its size centred again once it answers. Proposed again when Stage 2's
core configures real monitors, whose changes come from the kernel and can fail after a commit.
