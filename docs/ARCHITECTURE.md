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
├── windows: what is known of each window beyond its place (its sizes, its parent, fake
│   fullscreen, last floating place)
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
   workspaces it must have, brings each output's pinned windows to the workspace it shows, drops
   the promise to float back of windows no longer fullscreen, and puts each strip's view back on
   its focused column.

The compositor then reads `Layout::arrange(output)`: the windows that output shows, bottom to top,
each with the size its client is asked, its rectangle in logical coordinates on the output's
physical pixel grid, whether it is told it is fullscreen, and whether it has its seat's focus.
Nothing in the model runs between operations.

Clients do not always take the size they are given
([ADR-0026](adr/0026-real-clients-in-the-layout.md)). The compositor passes on each client's
minimum and maximum sizes (`Layout::set_size_limits`) and the window geometry it commits
(`Layout::commit_size`); each window's record keeps them (`sizing.rs`). A window then has two
sizes:

- **Asked**: its configure, in whole logical units, never 0 for a tiled window. In scrolling
  columns, by niri's rules: its column's width within its windows' limits, floored so that
  proportional columns fit the screen exactly, and a share of the column's height, rounded. In
  dwindle, its slot, whatever its minimum; fullscreen, the output's size; both rounded. Floating,
  0 on each axis its minimum and maximum leave free, to let the client choose, unless a rule or
  the compositor set its size.
- **Drawn**: what its client committed, or what it is asked before its first commit. Scrolling
  columns are as wide as their widest window and stack windows by their drawn heights; minimums
  that do not fit overflow the column. When a column changes width, for a commit or any other
  reason, the focused column stays where it is on screen and the columns on its left move, as in
  niri. A dwindle window is drawn from its slot's top-left corner, clipped to the slot; a
  fullscreen one is centred on its output; a floating one keeps its top-left corner.

Dialogs (`OpenOptions::parent`) open floating, centred on their parent, and take the focus only
from it; a fullscreen game's own dialog shows over it. `Layout::size_on_open` gives a new window's
first configure, by opening it on a copy of its seat.

Coordinates are logical and shared by all outputs, as `f64`; rectangles are snapped to the
physical pixels of their output, so that they stay crisp at scale 1.5
([ADR-0020](adr/0020-the-layout-models-shape.md)).

`Layout::check_invariants` checks every rule the model keeps. The tests call it after every
operation, and an example measures the model:

- `crates/yuna-layout/tests/scenarios/`: one test per behaviour; those that rewrite niri's or
  Hyprland's name their source ([ADR-0019](adr/0019-the-layouts-behaviours.md));
- `crates/yuna-layout/tests/properties.rs`: random sequences of operations over two seats and up
  to three outputs, with clients that ask limits, commit sizes of their own and open dialogs,
  checking the invariants, that one seat never changes another's, that a refused operation
  changes nothing, and that `Layout::size_on_open` foretells a new window's size;
- `crates/yuna-layout/examples/timing.rs`: an operation's time on workspaces of 100 and 1,000
  windows.

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
