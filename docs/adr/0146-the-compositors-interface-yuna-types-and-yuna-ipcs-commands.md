# 0146. The compositor's interface: `yuna-types`, and `yuna-ipc`'s commands for a caller

- Status: Accepted; superseded in part by [0157](0157-yuna-types-takes-the-logical-geometry-and-a-windows-shell.md) (what `yuna-types` holds)
- Date: 2026-11-20

## Context

Step 2 of [ADR-0139](0139-stage-2-in-three-gated-parts-2as-path-revised.md)'s path frees the
`Compositor` interface, which tests, MCP and the measuring examples drive, from Smithay's types,
and puts typed commands, `yuna-ipc`'s, in place of `change_layout`.
[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) asks for one
command type for key bindings, gestures, the configuration's actions, Hollow, MCP and the tests,
"dispatched knowing the caller" (item 14), and places `yuna-types` at level 0 and `yuna-ipc` at
level 1 of its map ([ADR-0145](0145-the-crate-map-motion-on-the-layout-the-configuration-on-the-commands.md)).

What the code held at `272bc8d`, surveyed on 2026-11-20:

- One public item named a Smithay type: `Compositor::read_pixels(OutputId, Rectangle<i32,
  Physical>)`, which only tests call. Tests also compiled their clients' keymaps with Smithay's
  re-export of xkbcommon.
- `change_layout(FnOnce(&mut Layout) -> R)` had 79 callers (74 in tests, 4 in MCP's `workspaces`
  tool, 1 example) over 15 of the layout's operations, each returning `Result<(), LayoutError>`.
  Three tests used it to put a window with no client into the layout, so that the layout refuses
  the next one; one used it only to make every output arrange again. Yuna has no key bindings yet.
- The ids, `Direction` and `Tiling` lived in `yuna-layout`, which `yuna` re-exported whole.

The rivals, read on 2026-11-20: niri's dispatch takes the caller's context beside the action, and
29 of its 149 actions take an optional window, the focused one when none; sway runs a command on a
seat given beside it; niri, sway and Hyprland answer a refusal with text. The challenge of the
design found that three planned changes add to what a dispatch must know of its caller: the lock
(Stage 2b), Hollow's role (2c), and one seat per Yuna (M2.12).

## Decision

The owner chose, on 2026-11-20:

- **`yuna-types` holds what more than one crate needs now**: the ids `WindowId`, `OutputId`,
  `SeatId`, `WorkspaceId` and `ColumnId`, `Direction`, `Tiling`, and `PixelRect`, a region of an
  output's buffer in physical pixels, unsigned. The layout's geometry stays in `yuna-layout` until
  the scene (M2.7) sets the coordinate spaces; the rest of the map's level 0 comes with its code.
  No crate re-exports another's types: each names `yuna_types` itself, and `yuna` re-exports
  neither `yuna_layout` nor `yuna_motion`.
- **`yuna-ipc`'s `Command`**: one variant per operation a caller may ask, relative to the caller's
  seat. A command on a window takes `Option<WindowId>`: none means the caller's focused window, as
  a key binding asks. `CloseWindow` is one of them. `Refusal` says why a command was refused, a
  variant per reason, its text the one MCP gives today. Both enums are `#[non_exhaustive]`, so
  that a variant added is not a break for Hollow. No wire format and no events yet: Hollow's
  protocol is decided with Hollow's interface (Stage 2c), the event stream at M2.4.
- **The dispatch knows its caller**: `Compositor::command(Caller, Command) -> Result<(), Refusal>`.
  `Caller` holds the caller's seat now; the lock's permission and Hollow's role become fields of
  it when they come. A command that names a window or an output of another seat is refused there,
  in one place, so that an agent's seat never reaches another's
  ([ADR-0138](0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md)). A command applied
  changes the layout as a key binding would, animated; a command refused changes nothing and
  draws nothing.
- **Test seams**, hidden from the documentation (ADR-0082 item 22): a window id held in the layout
  with no client behind it, for the tests of the layout's refusals, and an arrangement asked again
  of every output. Each says which invariant it may break.
- **`read_pixels`** takes a `PixelRect`. Tests and examples name no Smithay type; the tests'
  clients use xkbcommon directly.

## Alternatives considered

- **Each command carrying its seat**: a simpler signature, but M2.12 makes the field useless
  everywhere, and nothing would carry the caller's role for the lock or Hollow.
- **Refusals as text**, as the rivals do: the least code, but tests, MCP and Hollow would compare
  texts, and Yuna's sentences would leave the one module ADR-0082 item 23 keeps them in.
- **Window commands that require a window**: what tests and MCP use today, but key bindings act
  on the focused window, and the type would change at Stage 2b.
- **`yuna-ipc`'s own `Tiling`, mapped to the layout's**: two types for one thing.
- **The geometry moved to `yuna-types` now**: nothing new needs it, three of `Rect`'s methods
  would become public, the scene will set the coordinate spaces again at M2.7, and moving code
  across crates can move the parity script's operation counts.

## Consequences

- `change_layout` is gone; a closure of two operations is two commands, or one command whose
  dispatch does both (moving a window to a workspace focuses it first, as MCP did).
- `yuna-ipc` is a contract in 0.x
  ([ADR-0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)): once Hollow uses it,
  its changes are announced.
- A new command is a variant and its dispatch, tested through `command`.
- MCP no longer checks a window's seat itself; the dispatch does, with MCP's text.

## Validation and reconsideration

Evidence: the survey of the code at `272bc8d` (`.delegation/m2.2-*`, local), the rivals' sources
read on 2026-11-20, the challenge of the design. Checked by the tests driving every command, the
refusal of another seat's window and output, a refused command leaving no deadline armed, and the
parity script, which drives MCP's `workspaces` tool. Proposed again if Hollow's interface needs
another shape, or if key bindings need a command the shape cannot hold.
