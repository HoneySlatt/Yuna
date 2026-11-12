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
  its logs; `yuna-layout`. Its tests use `wayland-client` and `wayland-protocols`' client side,
  staging protocols included.
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
  (`Output::cleanup`), Smithay's popup trees forget the popups destroyed
  (`PopupManager::cleanup`), and the renderer frees the textures of closed windows, destroyed
  surfaces and destroyed dmabufs. Then it sends the events written. Its `Counters` count frames
  drawn, frame callbacks, configures, commits and wake-ups, for tests.
- `state.rs`: `State`, everything beside the loop and the display, and the protocol handlers that
  are not the shell's: `wl_compositor` (version 6) and `wl_subcompositor`, `wl_shm`,
  `wp_viewporter`, one seat with a keyboard (the owner's), `wl_output` with `xdg-output`,
  presentation time.
- `shell.rs`: `xdg-shell` and `xdg-decoration` in front of the layout. It maps each toplevel to a
  `WindowId`, passes on what its client asks, and caps the windows of each client.
- `popups.rs`: popups (`xdg_popup`), through Smithay's `PopupManager`: their placement, their
  configures, their keyboard grabs, their dismissal.
- `activation.rs`: `xdg-activation-v1`: tokens, activation through the layout, urgent windows.
- `dialog.rs`: `xdg-dialog-v1`: offered; the modal hint is read, and nothing uses it yet.
- `selection.rs`: the clipboard (`wl_data_device`), the primary selection and
  `ext-data-control-v1`, on Smithay's handlers.
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
   layout changed: every output's next boundary is armed. An activation asked before, if any, is
   applied now. A window is refused when its client has 1,000 windows already
   (`MAX_WINDOWS_PER_CLIENT`) or the layout holds 10,000 (`MAX_WINDOWS`): the layout walks a
   dwindle tree by recursion, and 10,000 windows in one tree work where 100,000 overflow the stack
   ([ADR-0020](adr/0020-the-layout-models-shape.md), [ADR-0028](adr/0028-bounds-on-what-one-client-may-ask.md)).
   A refusal for a cap is logged once per
   client. Should the layout itself refuse it, which a client's values cannot cause, that is
   logged once per window. A refused window is not shown, and its client gets frame callbacks at
   the hidden windows' rate, never left waiting; its next buffer tries again.
4. Each later commit passes its limits (`set_size_limits`) and its geometry (`commit_size`) on
   when they changed, and arms its output's next boundary when it is shown.
5. Fullscreen and maximise requests become `set_fullscreen` and `set_full_width`, and each gets a
   configure, as xdg-shell asks: at the next arrangement when the layout takes it, even if
   nothing changes; at once, with the state the window has, when the layout refuses it. Before
   the window is mapped, it will open so, and a configure says it if the first one is past.
6. A commit without a buffer, the toplevel's destruction, or its surface's, closes it in the
   layout and dismisses its popups. A client that leaves has its toplevels destroyed by the
   display, the same way.

### Popups

Popups (`popups.rs`) are not windows of the layout: they get no `WindowId` and never change it.
Smithay's `PopupManager` keeps each window's popups in a tree on its toplevel's surface, each with
its place relative to the toplevel's window geometry.

1. A client creates an `xdg_popup` on a toplevel or on another popup. Yuna places it: the
   positioner's rules, then its constraint adjustments (`PositionerState::get_unconstrained_geometry`)
   against the output its window is shown on, in the coordinates of its parent's window geometry.
   A window shown on no output places its popups without constraint. A popup is dismissed
   (`popup_done`) at once when its window is not mapped or has 64 popups already, all depths
   together (`MAX_POPUPS_PER_WINDOW`), its parent popup was dismissed, its chain of parents is
   broken, cyclic or deeper than 32 (`MAX_POPUP_DEPTH`), or its positioner holds nonsense values.
   The cap on popups bounds Smithay's walks over a window's popups, and Yuna's, which are linear
   and run at each of its commits and refresh boundaries
   ([ADR-0028](adr/0028-bounds-on-what-one-client-may-ask.md)). A new popup is told its window's scale
   at once ([Scale](#scale)).
2. Its initial commit gets its configure; its first buffer shows it. A `reposition` request
   places it again and is answered with `repositioned` and a configure, or dismisses it when it
   can no longer be placed. A commit without a buffer unmaps it: Smithay resets its role, its
   parent included, before Yuna sees the commit, so that it can never be shown again. Yuna then
   dismisses it with the popups over it, without a configure, and arms its window's output, which
   draws them away at the next boundary. Any later commit of that popup is its client's protocol
   error.
3. Its commits arm what its window's commits arm (`schedule_for_commit`), and its frame callbacks
   and presentation feedback go out with its window's.
4. An explicit grab (`xdg_popup.grab`) is granted when the owner's keyboard is on the popup's
   window, or one of its popups, and the grab's serial is the last `enter` that keyboard sent, or
   the serial of the grab a parent popup holds. Until M1.5 brings input, an `enter` is the only
   serial a client can have. A grab asked otherwise is refused by dismissing the popup, as
   xdg-shell allows. A popup dismissed already, at its creation or with its parent, takes no
   grab. A granted grab sets Smithay's keyboard grab for popups, and the keyboard goes to the
   topmost grabbing popup. The grab ends when its popups are gone, or its topmost popup is
   dismissed, its other popups then dismissed too and the keyboard returning to the window; or
   when the focus moves to another window, at a refresh boundary. A new grabbing popup on the
   window, not on the topmost grabbing one, ends the older grab first: Smithay would answer it
   with a protocol error, which xdg-shell does not ask. If the new popup hung from one of the
   older grab's popups, it is dismissed with them and takes no grab.
5. Between refresh boundaries the keyboard moves only within the window it is on: to the popup
   that takes a grab there, or back to the window when the grab ends. A move to another window,
   such as an activation asks, waits for the next boundary ([A frame](#a-frame), step 1).
6. Its window unmapping or going dismisses it. A client that destroys a parent popup before its
   child gets xdg-shell's `not_the_topmost_popup` error from Smithay, and leaves.

### Activation and dialogs

A client asks an `xdg-activation-v1` token, then asks with it that a window be activated, as a
launcher does for the program it starts (`activation.rs`). A token is valid when it was asked with
the owner's seat, for the surface that has the keyboard focus, and the serial of the `enter` the
keyboard sent that surface; it lasts 10 s of Yuna's clock and is used once. Tokens asked
otherwise are given, as the protocol requires, but are not valid. Yuna keeps the 64 newest
tokens of each client not used yet, without a timer, so that one client's tokens never push out
another's.

With a valid token, `Layout::request_activation` decides: the window takes the focus at the next
refresh boundary, unless a fullscreen window holds the screen
([ADR-0026](adr/0026-real-clients-in-the-layout.md)). Refused, or with a token that is not valid
or expired, the window is marked urgent and nothing else changes. Urgent is the compositor's
state, kept on the toplevel: it ends when the window gets the seat's focus, at the arrangement
that gives it, or closes; a window that has the focus is not marked. A request for a toplevel not
mapped yet waits for its mapping, and is judged then, by when its token was asked: a token that
expired meanwhile marks the window urgent. A request for a surface that is not a toplevel is
ignored.

`xdg-dialog-v1` is offered. Dialogs already open on their parent through `xdg_toplevel.set_parent`;
the modal hint (`dialog.rs`) is read from Smithay's record of the toplevel, and nothing uses it
yet.

### A frame

At an output's refresh boundary:

1. If the layout changed, it is arranged once, for every output. Each window is told its asked
   size and its states: tiled, fullscreen, activated if it has the seat's focus, suspended if it
   is shown on no output. A configure goes out only when its size or states changed, or when a
   request of its client is owed one. The keyboard focus follows `Layout::focused`, or the
   topmost popup of a grab on that window: only here does it change window. The selections
   follow the keyboard focus; the focused window is no longer urgent.
2. The output is drawn if anything on it is damaged: each window it shows, bottom to top, its
   surface tree with its window geometry at its rectangle's corner, cut to the rectangle, then
   its popups just above it, over a plain background. Surfaces sit on whole pixels: where the
   geometry's offset in its surface is not a whole number of pixels, it is rounded up, so that
   the cut takes a sliver of content rather than show the window's margin. A surface drawn at
   its output's physical size is drawn exactly ([Scale](#scale)). A popup's corner is rounded
   to the nearest pixel, and a popup is cut to the output, not to its window: a menu may
   overflow it. Yuna waits until the drawing is done.
3. The surfaces shown there, windows' and their popups', get their frame callbacks, and their
   presentation feedback: a frame whose drawing takes no time, as on the manual clock, is
   presented at its own boundary; otherwise at the first boundary after its drawing ended; never
   at the boundary of the frame before. `seq` is the boundary's number, the flags VSYNC, the
   refresh the output's period.

A window shown on no output, on a workspace not shown or scrolled out of view, is hidden: its
content updates are discarded, and its frame callbacks go out at most once a second, through the
hidden windows' deadline, armed only while one of them waits; its popups' too. A window the
layout refused is treated the same way.

### Scale

Each surface is told the scale of the output its window is shown on: `wp_fractional_scale_v1`'s
preferred scale, in 120ths, and `wl_surface.preferred_buffer_scale`, that scale's integer
ceiling. It is told when its fractional scale object is created, at its first commit, and when
its window enters an output; only a change sends anything. A popup's surfaces are told what
their window is told, from the popup's creation on. A surface whose window is shown on no output
keeps the last value. One never told anything learns what its window's surface was last told, so
that a subsurface or a popup added to a hidden window matches its window; a new window learns the
scale of the owner seat's active output, where it opens, else the first output's.
When a window moves to another output, it is told the new scale before the configure that gives
its size there, so that its client draws the buffer that acknowledges it at that scale.

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

Yuna takes a dmabuf only if each of its planes lies in a file it can always read: a real
dma-buf, whose size is fixed (`fstatfs` gives `DMA_BUF_MAGIC`), or a tmpfs memfd its client
sealed against shrinking (`F_SEAL_SHRINK`, which no one can take off; a hole punched in it reads
as zeros). A hugetlbfs memfd may raise SIGBUS where a hole is punched, sealed or not. Any other
file is refused: the client is told `failed`, or, for `create_immed`, gets the protocol's
error. llvmpipe would take a plain memfd, and the renderer reads a plane's file each time it
draws, while Smithay checks the plane against the file's size only when the buffer is created: a
file shrunk in between would kill Yuna with SIGBUS, which Smithay catches only for `wl_shm`
([ADR-0007](adr/0007-surviving-a-crash.md)).

### Selections

The clipboard and the primary selection belong to the owner's seat and follow its keyboard focus.
Yuna holds no selection of its own.

1. When the keyboard focus moves to another client's window, that client gets the selections
   just before its keyboard `enter`, as `wl_data_device` asks: each device it has is told what
   they hold, an offer with its MIME types, or nothing. When no window has the focus, no client
   has them. The frame loop does both where it moves the keyboard focus (`give_keyboard_to`).
2. Only the client with the focus may set them: Smithay ignores the request of any other. It does
   not check the serial the request names ([debt](debt.md)).
3. A new selection is offered to the client with the focus, and to every `ext-data-control-v1`
   client. A data-control client, such as a clipboard manager, gets the current selections as
   soon as it binds its device, and may set them without the focus. Every client may bind it for
   now ([debt](debt.md)).
4. To read a selection, a client sends the write end of a pipe with its `receive`. Yuna passes it
   to the source's client in a `send` event, then closes its own copy once that event is sent. It
   never reads, writes or waits on the pipe. For a MIME type the source does not offer, or a
   source gone, Yuna closes the pipe at once: the reader sees it end, empty.
5. A selection lasts as long as its source. When the source is destroyed, or its client leaves,
   the selection empties, and the client with the focus and the data-control clients are told.

Drag and drop waits for the pointer (M1.5). Smithay starts a drag only from a pointer or touch
grab, and the seat has neither: it refuses every `start_drag`.

### The program

`yuna --headless` starts the compositor with real time and a socket in `XDG_RUNTIME_DIR`, prints
`WAYLAND_DISPLAY=<socket>`, and runs until SIGTERM or SIGINT. The signal handler writes to a pipe
the event loop watches, so that the loop returns and the socket is removed.

### Tests

- `crates/yuna/tests/headless/`: the compositor in the test's process, manual clock, clients
  written with `wayland-client` (`client.rs` and `client/`; `selection_client.rs` for the
  selections), each test a short story with exact figures: the windows' in `main.rs`, then one
  file per subject (`popups.rs`, `activation.rs`, `dialogs.rs`, `window_cap.rs`, `scale.rs`,
  `dmabuf.rs`, `selection.rs`).
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
