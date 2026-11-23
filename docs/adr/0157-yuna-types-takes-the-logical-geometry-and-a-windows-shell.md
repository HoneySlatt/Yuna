# 0157. `yuna-types` takes the logical geometry and a window's shell

- Status: Accepted; supersedes in part
  [0146](0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md) (what `yuna-types`
  holds)
- Date: 2026-11-23

## Context

[ADR-0146](0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md) put in
`yuna-types` what more than one crate needed then: the ids, `Direction`, `Tiling` and
`PixelRect`. It left the layout's geometry in `yuna-layout` until the scene (M2.7) sets the
coordinate spaces, for three reasons: nothing new needed it, three of `Rect`'s methods would
become public, and moving code across crates can move the parity script's operation counts.

[ADR-0153](0153-one-event-stream-marks-at-the-change-mcps-log-per-seat-the-bound.md) puts the
compositor's events in `yuna_ipc::Event`. Two of them need types that live elsewhere today:

- **A window placed carries its place**, a rectangle in logical coordinates: `yuna_layout`'s
  `geometry::Rect`, whose methods return its `Point` and `Size`.
- **A window opened carries its shell**, xdg or X11: `yuna`'s `WindowShell`.

`yuna-ipc` is of level 1 on `yuna-types` alone
([ADR-0151](0151-the-crate-map-checked-by-rustc-and-a-script-yuna-transitional.md)): it can reach
neither. About fifty files name `Rect`, `Point` or `Size`.

## Decision

The owner chose, on 2026-11-23:

- **`yuna-types` holds what more than one crate needs now**: the ids `WindowId`, `OutputId`,
  `SeatId`, `WorkspaceId` and `ColumnId`; `Direction` and `Tiling`; `PixelRect`, a region of an
  output's buffer in physical pixels, unsigned; the logical geometry, `Point`, `Size` and `Rect`
  with their methods and the tolerance they compare to, `EPSILON`; and `WindowShell`, the shell
  behind a window. The rest of the map's level 0 comes with its code.
- **The geometry moves whole, its methods marked `#[inline]`**, so that a call across the crates'
  boundary costs what it cost inside `yuna-layout`. `clipped_to`, `is_inside` and `has_area`
  become public with the others.
- **What is the layout's own stays there**: `Scale` and its snapping to the physical grid, and
  the choice of the nearest rectangle in a direction.
- **No crate re-exports another's types**: each names `yuna_types` itself, and `yuna` re-exports
  neither `yuna_layout` nor `yuna_motion`.
- **The parity script judges the move.** It is a commit of its own; if its figures move, it is
  reverted and put to the owner again
  ([ADR-0139](0139-stage-2-in-three-gated-parts-2as-path-revised.md)).

The rest of ADR-0146 stands: the commands, the refusals, the dispatch knowing its caller, the test
seams and `read_pixels`.

## Alternatives considered

- **`yuna-ipc`'s own plain rectangle**, converted in one place of the core: no risk to the parity
  figures and nothing else moves, but two types for one thing, as ADR-0146 refused for `Tiling`,
  and a debt until M2.7.
- **`yuna-ipc` allowed to depend on `yuna-layout`**: Hollow's interface would bring the layout
  model with it, against the map.
- **The geometry left until M2.7**, the events carrying no place: MCP's `window_placed` gives
  one, and the oracle test folds it.

## Consequences

- `yuna-types`' documentation lists what it holds; `docs/ARCHITECTURE.md`'s table of crates
  says so.
- The scene (M2.7) sets the coordinate spaces from this geometry: it may rename or split it, by a
  new ADR.
- `yuna-ipc`'s events, a contract in 0.x
  ([ADR-0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)), name `Rect` and
  `WindowShell`: their changes are the contract's.

## Validation and reconsideration

Evidence: ADR-0146, the crate map, and a count of the files that name the geometry at `d43c648`.
Checked by the map's check, the build, and the parity script's figures on the commit that moves
it. Proposed again if those figures move, or if the scene needs coordinate types this one cannot
become.
