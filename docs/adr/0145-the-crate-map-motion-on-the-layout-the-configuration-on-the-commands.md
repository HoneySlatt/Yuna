# 0145. The crate map: motion on the layout, the configuration on the commands

- Status: Accepted; supersedes in part [0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) (the crate map)
- Date: 2026-11-20

## Context

[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) sets the map
of Stage 2's crates and says that `scripts/check.sh` checks its edges, from M2.4. Within level 1 it
allows only scene → colour and output → clock and scene. Two edges it forbids are needed:

- **`yuna-motion` → `yuna-layout`.** Motion turns the layout's jumps into frames
  ([ADR-0052](0052-motion-in-a-pure-crate.md)): `Motion::update` takes the whole `Layout`, and
  motion names the layout's `Placed`, `Layer`, `ShownWorkspaces`, `ColumnId` and geometry. The edge
  exists since M1.6, and `docs/ARCHITECTURE.md` states it. M2.4's check would fail on it.
- **`yuna-config` → `yuna-ipc`.** ADR-0082's item 14 makes the configuration's actions (key
  bindings, gestures) commands of the one command type, `yuna-ipc`'s. The configuration must then
  name that type. niri's configuration crate depends on its IPC crate the same way
  (`niri-config/Cargo.toml`, `impl From<niri_ipc::Action> for Action`, read 2026-11-20).

The challenge of M2.2's design, on 2026-11-20, found both against the code at `272bc8d`.

## Decision

The owner chose, on 2026-11-20, to **amend the map**: within level 1, motion → layout and
config → ipc are allowed beside scene → colour and output → clock and scene. The map, whole:

"→" reads "depends on". Edges go only down a level, and within level 1 only along the edges the
table names; the crates of level 2 never depend on each other and meet only in `yuna-core`. The
session opens the input devices, and the core hands their descriptors to `yuna-input` ahead of
use: libinput's opening of a device answers from them, never through `yuna-session` nor a D-Bus
call that would make the state thread wait.

| Level | Crates | Depend on |
|---|---|---|
| 0 | `yuna-types`: ids, coordinate spaces as distinct types, transforms, scale in 120ths, time values, dma-buf descriptors | nothing |
| 1 | `yuna-layout`, `yuna-motion`, `yuna-clock`, `yuna-colour`, `yuna-scene`, `yuna-output`, `yuna-config`, `yuna-ipc` (the commands and events, the crate Hollow uses, under semver) | level 0, and within level 1 only motion → layout, config → ipc, scene → colour, output → clock and scene |
| 2 | one crate per outside library: `yuna-wayland`, `yuna-render` (Vulkan, uwui's hosting), `yuna-kms`, `yuna-headless`, `yuna-input`, `yuna-session`, `yuna-xwm`, `yuna-process` | levels 0 and 1 |
| 3 | `yuna-core`: the state, the loop, the frame scheduler, the command dispatcher, the `Compositor` interface the tests drive | everything below |
| 4 | `yuna`, `yuna-mcp`, `yuna-keeper` (types and IPC only), `yuna-portal`, `yuna-wlcs`, fuzz targets | the core's interface |

`yuna-bench` stands outside the map, as ADR-0082 says. Each crate is created when its code exists,
except `yuna-ipc`, which Hollow depends on. `scripts/check.sh` checks the edges, one owning crate
per outside library, `forbid(unsafe_code)` outside those crates, and no `static` with interior
mutability nor `thread_local!` outside logging. Each crate's documentation states what it
promises, its bounds, its failure behaviour, and what it never does.

## Alternatives considered

- **The map kept strict**: what motion takes from the layout moved to level 0 (`Placed`, `Layer`,
  `ShownWorkspaces`, `Scale`, geometry), and the commands' vocabulary too. Level 1 stays flat, but
  `yuna-types` grows with the layout's vocabulary, and motion, which takes the whole layout, would
  need an interface of its own: a large change outside M2.2.
- **Motion merged into the layout**: removes the edge, but undoes ADR-0052's pure crate tested
  alone, and leaves config → ipc to decide.

## Consequences

- M2.4's check of the edges holds the edges of this table.
- A change of the layout rebuilds motion, as it does today.
- `yuna-config`, when it is created (Stage 2b), names `yuna-ipc`'s command type for its actions.

## Validation and reconsideration

Evidence: the code at `272bc8d` (`crates/yuna-motion/Cargo.toml`, `arrangement.rs`), the
challenge of 2026-11-20 and niri's configuration crate read that day. Proposed again if another
level-1 edge turns out needed, or if motion can take a view of the layout rather than the layout.
