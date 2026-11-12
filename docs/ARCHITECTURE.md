# Architecture

How the code is organised now, for a developer who has never seen it. Decisions and their reasons
are in [`adr/`](adr/README.md); the contracts of each item are in the code's documentation; what
comes next is in [`PLAN.md`](PLAN.md).

## Parts

One Cargo workspace; its crates live in `crates/`.

- **`yuna`** (`crates/yuna`): the compositor. Its library is the headless compositor, which the
  program runs and tests drive in their own process; its binary, `src/main.rs`, reads the command
  line and runs `yuna --headless` until SIGTERM. The desktop session does not exist yet (Stage 2).
- **`yuna-layout`** (`crates/yuna-layout`): the layout model. It knows seats, outputs, workspaces
  and windows by plain ids, decides where every window goes, and holds no Wayland or Smithay
  object, so that every behaviour is a test without a compositor
  ([ADR-0005](adr/0005-the-layouts.md), [ADR-0020](adr/0020-the-layout-models-shape.md)). The
  compositor puts it behind the protocols.

## Dependencies

- `yuna` → Smithay, at a pinned git revision, with only its `wayland_frontend`, `desktop` and
  `renderer_gl` features ([ADR-0011](adr/0011-built-on-smithay.md),
  [ADR-0022](adr/0022-the-headless-backend-in-yuna.md)); `tracing` and `tracing-subscriber` for
  its logs; `yuna-layout`. Its tests use `wayland-client` and `wayland-protocols`' client side.
- `yuna-layout` → nothing. Its tests use proptest, without its default features.
- At run time, from the development shell: libxkbcommon, linked; libglvnd's EGL and the shell's
  own Mesa, loaded, never the host's drivers.

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

## The headless compositor

`crates/yuna/src`, one job per module:

- `compositor.rs`: the public face, `Compositor`, which has at least one output. It holds
  calloop's event loop, the Wayland display and the state, and runs them: `dispatch` for one
  round, `run_until` for the program, `advance` to move the manual clock. Each round ends by
  freeing what went away, without drawing: the outputs forget the surfaces destroyed
  (`Output::cleanup`), and the renderer frees the textures of closed windows, destroyed surfaces
  and destroyed dmabufs. Then it sends the events written. Its `Counters` count frames drawn, frame callbacks,
  configures, commits and wake-ups, for tests.
- `state.rs`: `State`, everything beside the loop and the display, and the protocol handlers that
  are not the shell's: `wl_compositor` (version 6) and `wl_subcompositor`, `wl_shm`,
  `wp_viewporter`, one seat with a keyboard (the owner's), `wl_output` with `xdg-output`,
  presentation time.
- `shell.rs`: `xdg-shell` and `xdg-decoration` in front of the layout. It maps each toplevel to a
  `WindowId` and passes on what its client asks.
- `frames.rs`: the frame loop: when an output refreshes, what is arranged, drawn, called back and
  reported presented, and the hidden windows.
- `scale.rs`: the scale each surface is told: `wp_fractional_scale_v1`, and `wl_surface`'s
  preferred buffer scale and transform.
- `dmabuf.rs`: `linux-dmabuf`: its global, and the import of each client's dmabuf.
- `output.rs`: virtual outputs: their mode, checked when they are created; their buffer and
  damage tracker; their refresh boundaries.
- `render.rs`: the one GLES renderer, on Mesa's llvmpipe or, with `--gpu`, the GPU's render node;
  the only module that knows OpenGL ([ADR-0012](adr/0012-opengl-first.md)).
- `clock.rs`: the clock and its deadlines ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)).
- `main.rs` and `stop_signal.rs`: the program alone: its command line, its logs, and SIGTERM.

### Time and deadlines

The clock owns every deadline of the frame loop: one per output, its next refresh boundary, and
one for the hidden windows. In real mode, time is `CLOCK_MONOTONIC`, and the compositor keeps one
calloop timer armed at the earliest deadline, armed again before each wait; with no deadline,
no timer. In manual mode, for tests, time starts at the real monotonic time and moves only through
`Compositor::advance`, which runs each deadline it passes at its own time, in order. A draw hook
can pass manual time while a frame draws, to make drawing late.

An output's refresh boundary `k` comes `k / rate` seconds after the output was created, computed
from the rate each time, so that errors never add up.

### A window's life

1. A client creates an `xdg_toplevel`; Yuna records it and waits.
2. Its initial commit: Yuna reads its limits and parent, asks `Layout::size_on_open` and sends
   the first configure: that size, tiled on all four edges
   ([ADR-0024](adr/0024-no-title-bars.md)), decorations on the server's side.
3. Its first buffer: `Layout::open_window`, with its parent, its limits and its window geometry,
   clamped to the layout's `MAX_SIZE`; the window gets the next `WindowId`, never reused. The
   layout changed: every output's next boundary is armed. Should the layout refuse it, which a
   client's values cannot cause, the refusal is logged once, the window is not shown, and its
   client gets frame callbacks at the hidden windows' rate, never left waiting.
4. Each later commit passes its limits (`set_size_limits`) and its geometry (`commit_size`) on
   when they changed, and arms its output's next boundary when it is shown.
5. Fullscreen and maximise requests become `set_fullscreen` and `set_full_width`, and each gets a
   configure, as xdg-shell asks: at the next arrangement when the layout takes it, even if
   nothing changes; at once, with the state the window has, when the layout refuses it. Before
   the window is mapped, it will open so, and a configure says it if the first one is past.
6. A commit without a buffer, the toplevel's destruction, or its surface's, closes it in the
   layout. A client that leaves has its toplevels destroyed by the display, the same way.

Popups are not shown yet: each is dismissed at once (`xdg_popup.popup_done`).

### A frame

At an output's refresh boundary:

1. If the layout changed, it is arranged once, for every output. Each window is told its asked
   size and its states: tiled, fullscreen, activated if it has the seat's focus, suspended if it
   is shown on no output. A configure goes out only when its size or states changed, or when a
   request of its client is owed one. The keyboard focus follows `Layout::focused`.
2. The output is drawn if anything on it is damaged: each window it shows, bottom to top, its
   surface tree with its window geometry at its rectangle's corner, cut to the rectangle, over a
   plain background. Surfaces sit on whole pixels: where the geometry's offset in its surface is
   not a whole number of pixels, it is rounded up, so that the cut takes a sliver of content
   rather than show the window's margin. A surface drawn at its output's physical size is drawn
   exactly ([Scale](#scale)). Yuna waits until the drawing is done.
3. The surfaces shown there get their frame callbacks, and their presentation feedback: a frame
   whose drawing takes no time, as on the manual clock, is presented at its own boundary;
   otherwise at the first boundary after its drawing ended; never at the boundary of the frame
   before. `seq` is the boundary's number, the flags VSYNC, the refresh the output's period.

A window shown on no output, on a workspace not shown or scrolled out of view, is hidden: its
content updates are discarded, and its frame callbacks go out at most once a second, through the
hidden windows' deadline, armed only while one of them waits. A window the layout refused is
treated the same way.

### Scale

Each surface is told the scale of the output its window is shown on: `wp_fractional_scale_v1`'s
preferred scale, in 120ths, and `wl_surface.preferred_buffer_scale`, that scale's integer
ceiling. It is told when its fractional scale object is created, at its first commit, and when
its window enters an output; only a change sends anything. Before its window is shown anywhere,
it is told the scale of the owner seat's active output, where new windows open; a window shown
on no output keeps the last value.

A client that follows the fractional scale draws its buffer at its window's physical size,
rounded half away from zero, and sets its viewport's destination (`wp_viewporter`) to its
logical size. Its window geometry, its drawing and the size the layout is given all use that
logical size. One buffer pixel then covers one physical pixel: on whole pixels, at a rectangle
the layout put on the physical grid, the window is drawn exactly, with no blending at any edge.
A client that draws at an integer buffer scale is scaled when drawn ([debt](debt.md)).

### Buffers

Clients' buffers come through `wl_shm` or `linux-dmabuf`. Yuna offers `linux-dmabuf` only when
its renderer imports a format: on llvmpipe, version 3 with the formats; with `--gpu`, default
feedback naming the render node, which Smithay offers as version 6. Each dmabuf is imported when
its client creates its buffer, so that the client learns at once whether it can be used.
Smithay refuses broken parameters with a protocol error, which ends that client alone; a buffer
whose drawing fails is left out of that frame and logged by Smithay's renderer. A destroyed
dmabuf's texture is freed at the end of the round, like a closed window's.

### The program

`yuna --headless` starts the compositor with real time and a socket in `XDG_RUNTIME_DIR`, prints
`WAYLAND_DISPLAY=<socket>`, and runs until SIGTERM or SIGINT. The signal handler writes to a pipe
the event loop watches, so that the loop returns and the socket is removed.

### Tests

- `crates/yuna/tests/headless/`: the compositor in the test's process, manual clock, clients
  written with `wayland-client` (`client.rs`; viewports, fractional scale and dmabufs in
  `client/buffers.rs`), each test a short story with exact figures.
- `crates/yuna/tests/command_line.rs`: the program as a user runs it, its socket and its globals
  included.

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
3. [`../crates/yuna/src/lib.rs`](../crates/yuna/src/lib.rs), then
   [`frames.rs`](../crates/yuna/src/frames.rs) and
   [`shell.rs`](../crates/yuna/src/shell.rs).
4. A story of [`../crates/yuna/tests/headless/main.rs`](../crates/yuna/tests/headless/main.rs).
5. [`../scripts/check.sh`](../scripts/check.sh).
