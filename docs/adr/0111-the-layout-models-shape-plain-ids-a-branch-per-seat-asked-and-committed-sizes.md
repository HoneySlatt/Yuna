# 0111. The layout model's shape: plain ids, a branch per seat, sizes asked and committed

- Status: Accepted; supersedes [0005](0005-the-layouts.md) (the layout model, pure),
  [0020](0020-the-layout-models-shape.md) and [0026](0026-real-clients-in-the-layout.md) (sizes
  asked and committed); supersedes in part
  [0052](0052-motion-in-a-pure-crate.md) (workspace ids, the camera the layout gives out)
- Date: 2026-11-18

## Context

[ADR-0005](0005-the-layouts.md) made the layout model a pure part of Yuna, tested alone. Its
shape was set by [ADR-0020](0020-the-layout-models-shape.md), which left several parts to later
milestones.
[ADR-0026](0026-real-clients-in-the-layout.md) gave it the sizes clients ask and commit, and
[ADR-0052](0052-motion-in-a-pure-crate.md) gave workspaces ids and the layout a camera to give
out. A reader needed all three, and ADR-0020 still described a window as taking the size it is
given.

ADR-0020 also left for later the arranging of a hidden workspace, for the owner's peek at an
agent's workspace; that peek went with Stage 3
([ADR-0060](0060-development-first-agents-in-their-own-yuna.md)). It assumed every window takes
the size it is given, until ADR-0026 replaced that.

This ADR gathers the model's shape as it stands on 2026-11-18. It decides nothing new. How the
layouts behave is [ADR-0110](0110-the-layouts-and-their-behaviours.md)'s; how motion turns the
layout's jumps into springs, ADR-0052's.

## Decision

- **The layout model is a pure part of Yuna**, with no Wayland or Smithay object in it, so that
  every behaviour is a property test or a scenario run without a compositor (ADR-0005,
  2026-11-11). It serves several layouts and several seats from its first line.
- **One crate, `yuna-layout`, with no dependency** (ADR-0020, 2026-11-11). Its tests use proptest,
  which the owner accepted for tests only.
- **Plain ids** (ADR-0020). The caller names windows, outputs and seats with `u64` ids. It gives
  the same output id to the same monitor each time it is plugged in (from its connector and EDID,
  or the name the configuration uses), so that workspaces find their home again; it never reuses
  a window id. The model knows nothing of a window but its id and its layout state. **Workspaces
  get ids from the model**, from a counter, never reused (ADR-0052, 2026-11-14): the model creates
  workspaces, so it names them.
- **A tree that each seat owns a branch of** (ADR-0020). A seat holds its outputs, its workspaces
  without an output and its hidden special workspaces; an output holds its workspaces and the
  special one shown over it. Workspace names are per seat. An operation names a seat (a key
  binding) or a window (a client's request) and reaches only that seat's branch, so that isolation
  is the shape of the data, not a check. The refusal is mutual: the owner cannot pull an agent's
  window onto an owner's output either, until the owner asks for it.
- **Logical coordinates in `f64`**, shared by all outputs, as niri (ADR-0020). Gaps, the working
  area and every rectangle given out are snapped to the output's physical pixels, and snapped again
  when a workspace moves to another scale.
- **Sizes asked and committed** (ADR-0026, 2026-11-12). The model keeps each window's minimum,
  maximum and committed size, the latter its xdg window geometry. `Layout::arrange` gives each
  window the size it is asked, in whole logical units, heights rounded and widths floored so that
  proportional columns fit the screen exactly, both at least 1, and the rectangle it is drawn in,
  clipped to it. A size set by a rule or by the compositor is the size the window is asked, and
  stays asked. A tile is never asked 0, which would tell its client to choose its own size. The
  first configure carries the size the window will have, from `Layout::size_on_open`.
- **The layout jumps; it gives out what motion needs** (ADR-0052). An operation moves the model to
  its new state at once. With each shown scrolling workspace the layout gives out its camera, the
  view's position on the strip, `view_x`; with each placed window, its workspace and its layer
  (strip, tiles, floating, pinned). Dwindle and floating have no camera.
- **Nothing runs between operations** (ADR-0020). There is no clock and no animation state. An
  operation and `arrange` take time linear in the seat's windows: under 25 µs for 100, under
  300 µs for 1,000 ([measurements](../measurements.md)). The dwindle tree is walked without
  recursion (commit `62568b3`, 2026-11-12), and the windows a client may have are capped
  ([ADR-0028](0028-bounds-on-what-one-client-may-ask.md)).
- **Bad input is an error, never a panic, and a refused operation changes nothing** (ADR-0020).
  `check_invariants` states every rule; the tests call it after every operation.
- **An output's rectangle and scale change whole**: `configure_output`, and a window whose asked
  size the change altered counts at that size until its client answers
  ([ADR-0090](0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md)).
- **Left for later, without changing the tree** (ADR-0020): windows being dragged (Stage 2), and
  options changed live (with the configuration).

## Alternatives considered

- **Generic over a window trait, as niri** ([ADR-0020](0020-the-layout-models-shape.md)): the
  model would call into clients and hold their state, and tests would need fake windows.
- **Integer physical pixels per output** ([ADR-0020](0020-the-layout-models-shape.md)): exact
  arithmetic, but conversions everywhere between outputs of different scales.
- **Seats sharing outputs** ([ADR-0020](0020-the-layout-models-shape.md)): nothing needs it, and
  whose focus a workspace holds becomes unclear.
- **Animation state in the layout model, as niri** ([ADR-0052](0052-motion-in-a-pure-crate.md)):
  time would enter the pure model, its tests and its budgets.
- **Places from the sizes asked alone** ([ADR-0026](0026-real-clients-in-the-layout.md)): a window
  that commits more covers its neighbour, and one that commits less leaves a hole.

## Consequences

- The compositor keeps the map from Wayland objects to ids, and from monitors to stable output
  ids. It reads each window's limits and geometry on every commit and passes them on, and opens a
  window at its first buffer, its size known.

## Validation and reconsideration

Evidence: the model's scenarios and property tests, `check_invariants` after every operation, and
the timings in the [measurements](../measurements.md). Proposed again if a later need cannot be
added without changing the tree.
