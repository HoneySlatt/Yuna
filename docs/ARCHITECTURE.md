# Architecture

How the code is organised now, for a developer who has never seen it. Decisions and their reasons
are in [`adr/`](adr/README.md); the contracts of each item are in the code's documentation; what
comes next is in [`PLAN.md`](PLAN.md).

## Parts

One Cargo workspace; its crates live in `crates/`.

- **`yuna`** (`crates/yuna`): the program. Today it reads its command line, answers `--help` and
  `--version`, and refuses to start a session, since no backend exists yet. It depends on Smithay
  without using it, so that the pinned revision is locked and checked
  ([ADR-0017](adr/0017-smithays-revision.md)).
- **`yuna-layout`** (`crates/yuna-layout`): the layout model. It knows seats, outputs, workspaces
  and windows by plain ids, decides where every window goes, and holds no Wayland or Smithay
  object, so that every behaviour is a test without a compositor
  ([ADR-0005](adr/0005-the-layouts.md), [ADR-0020](adr/0020-the-layout-models-shape.md)). The
  program does not use it yet; the headless compositor (M1.4) puts it behind the protocols.

## Dependencies

- `yuna` → Smithay, at a pinned git revision, without its default features
  ([ADR-0011](adr/0011-built-on-smithay.md)).
- `yuna-layout` → nothing. Its tests use proptest, without its default features.

## The layout model

`yuna-layout` is a tree that each seat owns a branch of:

```text
Layout
├── options: gaps, widths, default tiling, dynamic workspaces
├── windows: what is known of each window beyond its place (fake fullscreen, last floating place)
└── seats
    └── Seat (the owner's, or an agent's)
        ├── outputs
        │   └── Output: rectangle, scale
        │       ├── workspaces, one shown
        │       │   └── Workspace: name, home output
        │       │       ├── tiling: scrolling columns or a dwindle tree
        │       │       └── floating windows, pinned ones among them
        │       └── the special workspace shown over it, if any
        ├── workspaces without an output (only while the seat has none)
        └── hidden special workspaces
```

An operation names a seat (a key binding: it acts on what that seat focuses) or a window (a
client's request). It reaches only its seat's branch, so an agent's seat never touches the owner's
([ADR-0015](adr/0015-agents-workspaces.md)).

The flow of an operation:

1. `Layout` finds the seat, the workspace and the window, and refuses unknown ids or nonsense
   numbers with an error, changing nothing.
2. The workspace applies the change to its tiling (`scrolling.rs` or `dwindle.rs`) or its floating
   windows (`floating.rs`). Each keeps its own focus: the focused column, the focused leaf, the
   focused floating window.
3. `Layout::tidy` removes the workspaces that have no reason to exist, gives every output the
   workspaces it must have, and puts each strip's view back on its focused column.

The compositor then reads `Layout::arrange(output)`: the windows that output shows, bottom to top,
each with its rectangle in logical coordinates on the output's physical pixel grid, whether it is
told it is fullscreen, and whether it has its seat's focus. Nothing in the model runs between
operations.

Coordinates are logical and shared by all outputs, as `f64`; rectangles are snapped to the
physical pixels of their output, so that they stay crisp at scale 1.5
([ADR-0020](adr/0020-the-layout-models-shape.md)).

`Layout::check_invariants` checks every rule the model keeps. The tests call it after every
operation:

- `crates/yuna-layout/tests/scenarios/`: one test per behaviour, each naming its source in niri or
  Hyprland ([ADR-0019](adr/0019-the-layouts-behaviours.md));
- `crates/yuna-layout/tests/properties.rs`: random sequences of operations over two seats and up
  to three outputs, checking the invariants, that one seat never changes another's, and that a
  refused operation changes nothing;
- `crates/yuna-layout/examples/timing.rs`: an operation's time on a workspace of 100 windows.

## Main flows

The program itself has none yet: it starts, reads its arguments and exits.

## The tools around the code

- `flake.nix`: the development shell; `rust-toolchain.toml`: the toolchain
  ([ADR-0016](adr/0016-the-rust-toolchain.md)).
- `scripts/check.sh`: every automated check, which CI runs (`.github/workflows/ci.yml`).
- `deny.toml`: the dependency policy.

## Where to start reading

1. [`../crates/yuna-layout/src/lib.rs`](../crates/yuna-layout/src/lib.rs), then
   [`layout.rs`](../crates/yuna-layout/src/layout.rs) for the operations.
2. A few scenarios, for example
   [`scrolling.rs`](../crates/yuna-layout/tests/scenarios/scrolling.rs).
3. [`../crates/yuna/src/main.rs`](../crates/yuna/src/main.rs).
4. [`../scripts/check.sh`](../scripts/check.sh).
