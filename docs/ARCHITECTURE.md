# Architecture

How the code is organised now, for a developer who has never seen it. Decisions and their reasons
are in [`adr/`](adr/README.md); the contracts of each item are in the code's documentation; what
comes next is in [`PLAN.md`](PLAN.md).

## Parts

One Cargo workspace; its crates live in `crates/`.

- **`yuna`** (`crates/yuna`): the compositor. Its library is the headless compositor, which the
  program runs and tests drive in their own process, and the MCP server around it; its binary,
  `src/main.rs`, reads the command line and runs `yuna --headless` until SIGTERM, or `yuna mcp`
  until its client closes its input. The desktop session does not exist yet (Stage 2).
- **`yuna-layout`** (`crates/yuna-layout`): the layout model. It knows seats, outputs, workspaces
  and windows by plain ids, decides where every window goes, and holds no Wayland or Smithay
  object, so that every behaviour is a test without a compositor
  ([ADR-0005](adr/0005-the-layouts.md), [ADR-0020](adr/0020-the-layout-models-shape.md)). The
  compositor puts it behind the protocols.

## Dependencies

- `yuna` → Smithay, at a pinned git revision, with only its `wayland_frontend`, `desktop` and
  `renderer_gl` features ([ADR-0011](adr/0011-built-on-smithay.md),
  [ADR-0022](adr/0022-the-headless-backend-in-yuna.md)); `tracing` and `tracing-subscriber` for
  its logs; `serde`, with its derive, and `serde_json` for MCP's messages
  ([ADR-0033](adr/0033-the-mcp-server.md)); `png` for a capture's image
  ([ADR-0034](adr/0034-captures.md)), which builds `miniz_oxide` twice, 0.8 for itself and 0.9
  for `flate2`; `yuna-layout`. rustix, which Smithay re-exports, makes its system calls: pipes,
  signals, `waitpid`, `prctl`. Its tests use `wayland-client` and `wayland-protocols`' client
  side, staging protocols included, proptest, without its default features, and `png` to decode
  captures.
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
Nothing in the model runs between operations. `Layout::arrange_workspace(output, index)`
arranges any of an output's workspaces as that output would show it alone, changing nothing:
what the owner's peek at a workspace not shown draws, in a thumbnail or an overview
([ADR-0015](adr/0015-agents-workspaces.md)). Nothing draws a peek before the desktop.

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
  changes nothing, that `Layout::size_on_open` foretells a new window's size, and that a peek
  at each workspace shows its own windows, the one shown as its output draws them;
- `crates/yuna-layout/examples/timing.rs`: an operation's time on workspaces of 100 and 1,000
  windows.

## The headless compositor

`crates/yuna/src`, one job per module:

- `compositor.rs`: the public face, `Compositor`. It holds calloop's event loop, the Wayland
  display and the state, and runs them: `dispatch` for one
  round, `run_until` for the program, `advance` to move the manual clock. Each round ends by
  freeing what went away, without drawing. First the clients the display ended: each records its
  end in a list the compositor holds, and `Backend::dispatch_single_client` frees it, its objects
  destroyed as when a client leaves. wayland-backend frees an ended client only after a dispatch
  that read some client, so one ended while Yuna drew its buffer (SIGBUS) or wrote it an event its
  full buffer could not take would otherwise keep its windows, shown and focused, until another
  client spoke. Then the outputs forget the surfaces destroyed (`Output::cleanup`), Smithay's
  popup trees forget the popups destroyed (`PopupManager::cleanup`), and the renderer frees the
  textures of closed windows, destroyed surfaces and destroyed dmabufs. Then it sends the events
  written. Its `Counters` count frames drawn, frame callbacks, configures, commits and wake-ups,
  for tests.
- `state.rs`: `State`, everything beside the loop and the display, and the protocol handlers that
  are not the shell's: `wl_compositor` (version 6) and `wl_subcompositor`, `wl_shm`,
  `wp_viewporter`, `wl_output` with `xdg-output`, presentation time, `cursor-shape`.
- `seats.rs`: the seats, the owner's and each agent's: their keyboards, pointers, focus, popup
  grabs and clients, their creation and their removal ([below](#seats-and-input)).
- `input.rs`: pointer and keyboard input on an agent's seat, what is under the pointer, text
  typed through a keymap, and drag and drop.
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
- `capture.rs`: captures, in Yuna's terms: which frame, which pixels, and how an agent's image maps
  to logical coordinates ([below](#captures)).
- `screencopy.rs`: `ext-image-copy-capture-v1` and its output sources, a thin layer over
  Smithay's modules and `capture.rs`.
- `text_input.rs`: `text-input-v3`, Yuna as the input method of agents' seats, a thin layer
  over Smithay's module ([below](#seats-and-input)).
- `scale.rs`: the scale each surface is told: `wp_fractional_scale_v1`, and `wl_surface`'s
  preferred buffer scale and transform.
- `dmabuf.rs`: `linux-dmabuf`: its global, and the import of each client's dmabuf.
- `output.rs`: virtual outputs: their mode, checked when they are created; their buffer and
  damage tracker; their refresh boundaries; their removal.
- `render.rs`: the one GLES renderer, on Mesa's llvmpipe or, with `--gpu`, the GPU's render node;
  the only module that knows OpenGL ([ADR-0012](adr/0012-opengl-first.md)); `read_pixels` is the
  only place pixels are read back. It keeps each EGL
  display it opens until the process ends, so that compositors started and dropped on several
  threads, as tests run them, never lose the display Smithay shares among them.
- `clock.rs`: the clock and its deadlines ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)).
- `mcp/`: the MCP server, [below](#the-mcp-server).
- `main.rs` and `stop_signal.rs`: the program alone: its command line, its logs, SIGTERM, and
  SIGCHLD for `yuna mcp`.

### Time and deadlines

The clock owns every deadline of the frame loop: two per output, its next refresh and the
boundary its held presentation feedback waits for, and one for the hidden windows. Each round of
the loop runs the deadlines due by its end, so that a refresh armed at once, on an idle output,
runs in the round that read the commit. In real mode, time is `CLOCK_MONOTONIC`, and the
compositor keeps one calloop timer armed at the earliest deadline, armed again before each wait;
with no deadline, no timer. In manual mode, for tests, time starts at the real monotonic time and
moves only through `Compositor::advance`, which runs each deadline it passes at its own time, in
order. A draw hook can pass manual time while a frame draws, to make drawing late.

An output's refresh boundary `k` comes `k / rate` seconds after the output was created, computed
from the rate each time, so that errors never add up.

A commit, or a change of the layout, arms its output's refresh
([ADR-0031](adr/0031-frames-drawn-ahead-of-their-boundary.md)). When the output is idle, the
boundary after its last refresh passed, the refresh is due at once and aims at the first boundary
at or after it. Otherwise it is due at a boundary minus the output's lead, the first after its
last refresh whose deadline is not past. The lead is the longest drawing of the last half second,
timed from its deadline to its end, at least 2 ms, plus 1 ms, at most one period
(`frames.rs`, `Drawings`). On the manual clock a drawing takes no time unless the draw hook
passes some, so the lead is 3 ms.

### A window's life

1. A client creates an `xdg_toplevel`; Yuna records it and waits. A second `xdg_toplevel` on a
   surface whose first lives ends its client with xdg-shell's `already_constructed`: Smithay
   gives a surface its role again ([debt](debt.md)), and Yuna's record of the first would be
   lost, its window left in the layout for ever. A toplevel on an `xdg_surface` whose surface
   its client destroyed has nothing to show, and is not recorded.
2. Its initial commit: Yuna reads its limits and parent, asks `Layout::size_on_open` and sends
   the first configure: that size, tiled on all four edges
   ([ADR-0024](adr/0024-no-title-bars.md)), decorations on the server's side.
3. Its first buffer: `Layout::open_window`, with its parent, its limits and its window geometry,
   clamped to the layout's `MAX_SIZE`; the window gets the next `WindowId`, never reused. The
   layout changed: every output's next refresh is armed. An activation asked before, if any, is
   applied now. A window is refused when its client has 1,000 windows already
   (`MAX_WINDOWS_PER_CLIENT`) or the layout holds 10,000 (`MAX_WINDOWS`): the layout and the
   frame loop walk every window at each commit and refresh, and each window holds memory
   ([ADR-0028](adr/0028-bounds-on-what-one-client-may-ask.md)).
   A refusal for a cap is logged once per
   client. Should the layout itself refuse it, which a client's values cannot cause, that is
   logged once per window. A refused window is not shown, and its client gets frame callbacks at
   the hidden windows' rate, never left waiting; its next buffer tries again.
4. Each later commit passes its limits (`set_size_limits`) and its geometry (`commit_size`) on
   when they changed, and arms its output's next refresh when it is shown.
5. Fullscreen and maximise requests become `set_fullscreen` and `set_full_width`, and each gets a
   configure, as xdg-shell asks: at the next arrangement when the layout takes it, even if
   nothing changes; at once, with the state the window has, when the layout refuses it. Before
   the window is mapped, it will open so, and a configure says it if the first one is past.
6. A commit without a buffer, the toplevel's destruction, or its surface's, closes it in the
   layout and dismisses its popups. A client that leaves, or that the display ends, has its
   toplevels destroyed by the display, the same way, by the end of the round.

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
   at once ([Scale](#scale)). A second `xdg_popup` on a surface whose first lives ends its client,
   as a second toplevel does.
2. Its initial commit gets its configure; its first buffer shows it. A `reposition` request
   places it again and is answered with `repositioned` and a configure, or dismisses it when it
   can no longer be placed. A commit without a buffer unmaps it: Smithay resets its role, its
   parent included, before Yuna sees the commit, so that it can never be shown again. Yuna then
   dismisses it with the popups over it, without a configure, and arms its window's output, which
   draws them away at its next refresh. Any later commit of that popup is its client's protocol
   error.
3. Its commits arm what its window's commits arm (`schedule_for_commit`), and its frame callbacks
   and presentation feedback go out with its window's.
4. An explicit grab (`xdg_popup.grab`) is granted on the popup's client's own seat, when the
   grab's serial is one that seat sent to the popup's window: a button press on it; or, while the
   seat's keyboard is on that window or one of its popups, the last `enter` or key press that
   keyboard sent, or the serial of the grab a parent popup holds. A grab asked otherwise, or
   naming another seat, is refused by dismissing the popup, as xdg-shell allows. A popup
   dismissed already, at its creation or with its parent, takes no grab. Each seat has its own
   grab. A granted grab sets Smithay's keyboard grab for popups, and its pointer grab when the
   seat has a pointer, and the keyboard goes to the topmost grabbing popup. A press outside the
   client's surfaces then dismisses the popups: Smithay ends the grab, and Yuna lets it go after
   that press, the keyboard back on the window. The grab ends when its popups are gone, or its topmost popup is
   dismissed, its other popups then dismissed too and the keyboard returning to the window; or
   when the focus moves to another window, at a refresh. A new grabbing popup on the
   window, not on the topmost grabbing one, ends the older grab first: Smithay would answer it
   with a protocol error, which xdg-shell does not ask. If the new popup hung from one of the
   older grab's popups, it is dismissed with them and takes no grab.
5. Between refreshes the keyboard moves only within the window it is on: to the popup
   that takes a grab there, or back to the window when the grab ends. A move to another window,
   such as an activation asks, waits for the next refresh ([A frame](#a-frame), step 1).
6. Its window unmapping or going dismisses it. When the window's surface is destroyed first,
   Smithay keeps that surface's popup tree while its popups live; Yuna counts none of them as
   tracked, so that none takes a grab. A client that destroys a parent popup before its child
   gets xdg-shell's `not_the_topmost_popup` error from Smithay, and leaves.

### Activation and dialogs

A client asks an `xdg-activation-v1` token, then asks with it that a window be activated, as a
launcher does for the program it starts (`activation.rs`). A token is valid when it was asked with
its client's own seat, for a surface of that client, and a serial that seat sent there: the
last `enter` or key press while its keyboard is on that surface, or a button press on the
surface's window. It lasts 10 s of Yuna's clock, is used once, and activates only a window of
the same seat. Tokens asked
otherwise are given, as the protocol requires, but are not valid. Yuna keeps the 64 newest
tokens of each client not used yet, without a timer, so that one client's tokens never push out
another's.

With a valid token, `Layout::request_activation` decides: the window takes the focus at the next
refresh, unless a fullscreen window holds the screen
([ADR-0026](adr/0026-real-clients-in-the-layout.md)). Refused, or with a token that is not valid
or expired, the window is marked urgent and nothing else changes. Urgent is the compositor's
state, kept on the toplevel: it ends when the window gets its seat's focus, at the arrangement
that gives it, or closes; a window that has the focus is not marked. A request for a toplevel not
mapped yet waits for its mapping, and is judged then, by when its token was asked: a token that
expired meanwhile marks the window urgent. A request for a surface that is not a toplevel is
ignored.

`xdg-dialog-v1` is offered. Dialogs already open on their parent through `xdg_toplevel.set_parent`;
the modal hint (`dialog.rs`) is read from Smithay's record of the toplevel, and nothing uses it
yet.

### A frame

At an output's refresh, due at once or a lead before the boundary it aims at:

1. If the layout changed, it is arranged once, for every output. Each window is told its asked
   size and its states: tiled, fullscreen, activated if it has its seat's focus, suspended if it
   is shown on no output. A configure goes out only when its size or states changed, or when a
   request of its client is owed one. Each seat's keyboard focus follows `Layout::focused` for
   that seat, or the topmost popup of the seat's grab on that window: only here does it change
   window. The seat's selections follow it; the focused window is no longer urgent. Each
   agent's pointer is put on what is under it now, if that changed.
2. The output is drawn if anything on it is damaged: each window it shows, bottom to top, its
   surface tree with its window geometry at its rectangle's corner, cut to the rectangle, then
   its popups just above it, over a plain background. Surfaces sit on whole pixels: where the
   geometry's offset in its surface is not a whole number of pixels, it is rounded up, so that
   the cut takes a sliver of content rather than show the window's margin. A surface drawn at
   its output's physical size is drawn exactly ([Scale](#scale)). A popup's corner is rounded
   to the nearest pixel, and a popup is cut to the output, not to its window: a menu may
   overflow it. Yuna waits until the drawing is done.
3. The surfaces shown there, windows' and their popups', get their frame callbacks. Their
   presentation feedback is taken and held by the output until the boundary the frame is
   presented at: the one it aimed at if its drawing ended by then, else the first after. It is
   sent at that boundary, through the output's presentation deadline, armed only when a surface
   asked feedback. `seq` is the boundary's number, the flags VSYNC, the refresh the output's
   period. A refresh that drew nothing still counts as the output's last refresh, so that
   callbacks asked without damage come once a period.

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
scale of its seat's active output, where it opens, else the first output's.
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
feedback naming the render node, which Smithay offers as version 6. On llvmpipe, Mesa's own
clients draw through `wl_shm` all the same: with no GPU named, their EGL and Vulkan fall back on
shared memory ([ADR-0029](adr/0029-the-rest-of-animated-applications.md)). Each dmabuf is imported when
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

Each seat has its own clipboard and primary selection, which follow its keyboard focus: a seat's
focus is only ever on its own clients, so a selection never crosses seats. Yuna holds no
selection of its own.

1. When the keyboard focus moves to another client's window, that client gets the selections
   just before its keyboard `enter`, as `wl_data_device` asks: each device it has is told what
   they hold, an offer with its MIME types, or nothing. When no window has the focus, no client
   has them. The frame loop does both where it moves the keyboard focus (`give_keyboard_to`).
2. Only the client with the focus may set them: Smithay ignores the request of any other. It does
   not check the serial the request names ([debt](debt.md)).
3. A new selection is offered to the client with the focus, and to every `ext-data-control-v1`
   client. A data-control client, such as a clipboard manager, gets the current selections as
   soon as it binds its device, and may set them without the focus. Every client of the owner's
   seat may bind it for now ([debt](debt.md)); no agent's client sees it, since it would reach
   any seat's selections.
4. To read a selection, a client sends the write end of a pipe with its `receive`. Yuna passes it
   to the source's client in a `send` event, then closes its own copy once that event is sent. It
   never reads, writes or waits on the pipe. For a MIME type the source does not offer, or a
   source gone, Yuna closes the pipe at once: the reader sees it end, empty.
5. A selection lasts as long as its source. When the source is destroyed, or its client leaves,
   the selection empties, and the client with the focus and the data-control clients are told.

A drag starts from a button an agent's pointer holds: Smithay accepts `start_drag` only with the
serial of the pointer's grab, which a press starts, and Yuna then gives the pointer Smithay's
drag grab. The drag offers its source to the surfaces the pointer crosses, all of its seat's,
and drops where the button is released, once the target accepted a type and an action. A drag
from a client of another seat than the pointer's, one that bound another agent's `wl_seat` and
guessed its serial, is refused, its source cancelled. The owner's seat has no pointer headless:
its clients start no drag. The drag's icon is not drawn ([debt](debt.md)).

### Seats and input

A seat (`seats.rs`) is a Smithay seat with a keyboard, and for an agent a pointer, its own
keyboard focus, popup grab, selections, cursor and clients, and its branch of the layout
([ADR-0035](adr/0035-agents-seats.md), [ADR-0037](adr/0037-seats-tools-input-and-closing.md)).
The owner's seat, 1, exists from the start; in `yuna mcp` it has no `wl_seat` global, no output
and no socket. An agent's seat gets the next id, never reused, a `wl_seat` named `agent-<id>`,
and a keyboard of the `us` layout whatever the machine's settings. A client belongs to the seat
whose socket it came through, recorded in its `ClientState`: its windows open on that seat's
workspaces, and each seat's focus goes only to its own clients' surfaces. A client is shown its
own seat's `wl_seat` and its seat's outputs' `wl_output`s alone, through the filter per client of
Yuna's fork of Smithay ([ADR-0042](adr/0042-main-on-smithays-fork.md)); a client that held
another seat's would still get no input, since that seat's focus is never on its surfaces.

Input (`input.rs`) reaches an agent's seat only through the compositor's calls, which the MCP
server makes for the seat an agent names; the owner's seat takes none. Its events carry Yuna's
clock's time.

1. A pointer moves to a place in logical units of one of its seat's outputs. What is under it
   is found among the windows that output shows, as drawn, top to bottom, each window's popups
   before it, a window cut to its rectangle: the surface whose input region holds the point. Smithay
   sends `enter`, `leave` and `motion`. After each arrangement, a pointer is moved onto what is
   under it then, when that changed.
2. A button press focuses the window under it: its keyboard follows at the next refresh. The
   press's serial, recorded with that window, may start a popup grab, a drag or an activation
   token there. A wheel step scrolls 15 units, and 120 in `axis_value120`.
3. A key is pressed by its XKB keycode, the name an agent gives looked up in the seat's keymap.
   Text is typed key by key through that keymap, Shift held where a character needs it. A run
   of characters the keymap lacks is committed to the text field enabled where the keyboard is,
   through `text-input-v3`; with none, it goes through Smithay's `inject_text_keysyms`, which
   gives the focused client a keymap of its own for them, then the seat's back.
4. Yuna is the input method of each agent's seat (`text_input.rs`,
   [ADR-0043](adr/0043-text-input-and-the-rest-of-slimeengines-examples.md)): a client's text
   field (`text-input-v3`) is entered with its surface's keyboard focus and left with it. Once
   its client enables it, Yuna sends it text committed, in pieces of 4,000 bytes at most, and
   text being composed, which shows until the next text typed or an empty one clears it. The
   owner's seat has no input method: its clients' fields are never entered, and it takes no
   text. Text or a key name holding a NUL, which a Wayland string or xkbcommon cannot take, is
   refused.
5. A client's cursor, asked through `wl_pointer.set_cursor` or `cursor-shape` with its last
   `enter` serial, is recorded on the seat; Yuna does not draw cursors yet.

A seat's socket that fails to `accept`, out of descriptors most often, rests 100 ms
(`SOCKET_PAUSE`) rather than stop Yuna ([ADR-0007](adr/0007-surviving-a-crash.md)).

Removing a seat (`Compositor::remove_seat`), in order: the keys and buttons it holds are
released, its popup grab ends, and its focus leaves every surface; those events are sent; its
socket is removed; its clients are ended and freed, their windows closed; then its outputs, its
place in the layout, and its `wl_seat` global, disabled now and destroyed later, as a removed
output's is. Smithay keeps a record of every seat it made: a removed one's stays, without its
keyboard and pointer.

### Outputs added and removed

An output belongs to one seat. An output added while Yuna runs goes to the right of every
output, and its first frame is armed; its `wl_output` global is announced at the round's end. A
removed output's workspaces go to its seat's active output, which keeps showing its own: their windows are hidden there until a
workspace switch shows them ([ADR-0018](adr/0018-workspaces.md)). Its windows' surfaces leave
it at once, and enter an output again once one shows them; the presentation feedback it
held for a frame not yet at its boundary is discarded; its two deadlines are disarmed. Its global
is disabled at once and destroyed 10 s later, at the end of a round, so that a client that asked
to bind it before it learnt of the removal is not ended for it. A seat's last output is removed
only with the seat, and a compositor keeps 16 at most (`MAX_OUTPUTS`), each about 33 MB at 4K.

### Captures

A capture shows an output's *newest* frame ([ADR-0034](adr/0034-captures.md),
[ADR-0038](adr/0038-captures-coordinates-windows-and-wayland.md)): when the output's refresh is
armed, a commit or a change of the layout on its way, the frame that refresh draws; otherwise
the frame its buffer holds. Nothing is drawn for a capture.

1. A capture is queued on its output (`capture.rs`). At that output's refresh, after drawing,
   the pixels are read from its buffer (`render::read_pixels`), on the main thread, and the
   capture records the boundary the frame is presented at. A failed drawing, or a removed
   output, fails the captures waiting on it. A Wayland client's capture of the newest frame
   arms the refresh when none is armed, due at once on an idle output and drawing nothing new,
   so that clients asking frames again and again cost one copy a refresh at most; an agent's,
   one at a time for each seat, is copied at once instead.
2. An agent's capture reads the physical pixels under its area: the whole output, a region in
   whole logical units, or the rectangle its window was drawn at, cut to the output. A window
   hidden when asked is refused; while an arrangement is pending, its capture waits for it, and
   takes the output it shows the window on, or fails if it hides it. An area that holds no whole
   pixel, an odd scale's last unit, is refused. Its *mapping* says where the image's top-left corner is in the
   output's logical units, and how many units a pixel covers: 1 at the logical size, 1 / scale at
   the physical size.
3. The MCP server encodes it on a thread of its own (`mcp/image.rs`): at the logical size each
   pixel is the average of the physical pixels its unit covers, weighted by how much of each;
   then a PNG with fdeflate's fast compression, then base64. Over 8,000 pixels a side or 5 MB of
   base64, it is refused. Four encoders run at most, those of captures cancelled included; a
   panic in one fails its capture. The reply goes once the image is encoded and the clock has
   reached the frame's boundary; on the real clock a timer wakes the loop for it. It gives the image, the
   mapping and an id; `pointer` maps a point of one of the seat's last 16 images to its output.
4. A Wayland client's capture (`screencopy.rs`) sees the globals whatever its seat, and captures
   only its own seat's outputs: Yuna stops a session of another's. A session's frame is the
   newest when it is its first, or when the output drew since its last; otherwise it waits for a
   refresh that draws something, so that a recorder costs nothing at rest. The whole output is
   copied once per refresh for all the frames waiting on it, into each client's `wl_shm` buffer,
   `Argb8888` or `Xrgb8888`, the renderer's R, G, B, A swapped to B, G, R, A; the frame is ready
   as soon as it is copied, as the protocol asks, with full damage and the time of the boundary
   it is presented at. A frame that could not be drawn or read fails as one the client may ask
   again; a removed output stops its sessions. A client keeps 4 sessions at most, all clients
   16.

### The MCP server

`yuna mcp` is a headless Yuna of one MCP client's own: it reads JSON-RPC requests from its
standard input, one per line, and writes the replies to its standard output, nothing else
([ADR-0033](adr/0033-the-mcp-server.md)). `mcp/`, one job per file:

- `protocol.rs`: the messages and the revisions. A request that names 2026-07-28 in its `_meta`
  is served on its own, with `server/discover`, `tools/list` and `tools/call`; one that names
  another revision gets -32022. A client that opens with `initialize` is answered 2025-11-25,
  and its requests without `_meta`'s revision are served under it, `ping` included: the one
  state kept across requests.
- `stdio.rs`: the lines, at most 1 MiB, and the queue of replies.
- `tools.rs`: the tools' schemas, their arguments, read with serde, and their results, given
  both as `structuredContent` and as text. A tool that does several things takes an `action`.
- `programs.rs`: the programs `spawn` starts, and which processes belong to a seat.
- `image.rs`: a capture's PNG and its base64, made on a thread of its own.
- `mod.rs`: `Server`, which holds the compositor and runs the tools.

Everything runs in the compositor's event loop. The sources of the client's input and output,
of SIGCHLD, of the waiting calls' timers and of the encoders' channel only note what came; after each round,
`Server::dispatch` reaps the programs that ended, reads one chunk of input, answers each whole
line, answers the calls whose window came or whose time is up, starts encoding the captures copied
and answers those encoded whose frame's boundary came, sends the compositor's events and
writes the replies. While 16 MiB of replies wait, no further line is answered and the client's
input is not read. A line over 1 MiB, or an input or output that fails, ends the connection.

`yuna mcp` starts with no output and no socket of the owner's seat, which announces no
`wl_seat`: everything is on agents' seats ([ADR-0037](adr/0037-seats-tools-input-and-closing.md)).
The tools, each but `clock` naming its seat by its handle, an id of another seat's output,
window or program unknown there:

- `seat` creates a seat, with an output of the mode it gives, else `--output`'s, and a Wayland
  socket, `seat-<id>` in Yuna's folder; lists the seats; closes one. A manual-clock Yuna holds one
  seat at a time.
- `outputs` lists, creates and removes a seat's outputs; `workspaces` lists them with their
  windows, creates, removes and shows a named one, and moves a window to one.
- `spawn` starts, lists and stops a seat's programs.
- `windows` lists a seat's windows, as `Compositor::windows` describes them, and with `wait_ms`
  waits up to 50 s, on real time whatever the clock's, for one that matches. A waiting call is
  answered once the layout has placed the window, which on the manual clock takes a `clock`
  advance; `notifications/cancelled` drops it unanswered; 64 wait at most.
- `capture` gives a PNG of a seat's output, a region of it or a window, at the logical or the
  physical size, with its mapping and an id ([Captures](#captures)); one capture of a seat at a
  time, waiting up to 50 s, on real time, for its frame; `notifications/cancelled` drops it.
- `pointer` moves, presses, releases, clicks and scrolls, moving first when given a place, on an
  output or in a capture's image, and tells where the pointer is and what is under it; `keyboard` presses, releases and taps keys,
  types text and shows text being composed, and tells the window the keyboard is on and whether
  a text field is enabled there ([Seats and input](#seats-and-input)).
- `clock` reads the time, and advances a manual clock, never past real time.

`seat close` removes the seat from the compositor at once ([Seats and input](#seats-and-input)),
fails the `windows` calls waiting on it, then ends its processes: every process under Yuna that
is, or descends from, one of the seat's programs still running, is in a process group one of
them led, is one of its clients, or has `YUNA_SEAT=<id>` in `/proc/<pid>/environ`; unless
another seat's `YUNA_SEAT`, or another seat's running program or its group, says it is that
seat's, as a reused pid would be. Each gets SIGTERM; 2 s later, SIGKILL goes to those still under
Yuna and to any found again; the call is answered once all are gone, or 1 s after SIGKILL, with
what was ended and what is left, or when Yuna stops. The seat's programs are then forgotten, their logs deleted. A
process that empties its environment and leaves its group and its parents is not found
([debt](debt.md)).

A program gets an environment of its own: from Yuna's, only the variables that find its
programs, libraries, locale and drivers (`programs::KEPT_VARIABLES`); `WAYLAND_DISPLAY`, its
seat's socket's full path; an empty `XDG_RUNTIME_DIR` of its own, where neither the user's
session bus nor their sound server is found; then the agent's variables; then `YUNA_SEAT`, its
seat's id, which the agent's cannot replace. It inherits no descriptor of
Yuna's but its standard input, `/dev/null`, and its output and errors: a pipe Yuna reads into a
log file. A log keeps 1 MiB; past it, the pipe is read 64 KiB every 100 ms and counted, so that a
program writing without end waits on its pipe and Yuna does not spin. 64 programs run at most,
128 are remembered. Each leads its own process group, which `stop` signals, its first process
gone or not. Yuna is its children's subreaper: a process whose parent ends becomes Yuna's, so
that the whole tree is reaped and ended by Yuna. When Yuna stops, every process under it, found
through `/proc`, gets SIGTERM, then SIGKILL 2 s later, while the compositor still serves its
clients; then the folder of sockets, logs and runtime folders, `yuna-mcp-<pid>` in
`XDG_RUNTIME_DIR`, is removed.

### The program

`yuna --headless` starts the compositor with real time, its outputs and a socket in
`XDG_RUNTIME_DIR` for the owner's seat, prints `WAYLAND_DISPLAY=<socket>`, and runs until SIGTERM
or SIGINT. `yuna mcp` starts the same compositor without an output or a socket, on the manual
clock with `--manual-clock`, and serves MCP until its input closes or SIGTERM or SIGINT comes. The signal handler writes to a pipe
the event loop watches, so that the loop returns and the socket is removed. At the `debug` level, it logs
`window opened` when a window maps, `frame drawn` for each frame, and, after each round of its
loop that woke, its counters in a line whose message is `counters`: what a test outside the
process reads. `frame drawn` gives the frame's time, `took_us`: from when its elements begin to be
built, which imports its clients' new buffers (a `wl_shm` buffer is copied then), to when its
drawing is done. Beside it: `lead_us`, how long before its boundary the refresh was due;
`at_once`, whether the output was idle; `boundary`, the boundary it aimed at; `presented`, the
one it was presented at, later when the frame was late. `capture copied` gives the time a
capture's copy of pixels held the main thread, `capture encoded` the time its PNG took on its
own thread, both `took_us`.

### Tests

- `crates/yuna/tests/headless/`: the compositor in the test's process, manual clock, clients
  written with `wayland-client` (`client.rs` and `client/`; `selection_client.rs` for the
  selections), each test a short story with exact figures: the windows' in `main.rs`, then one
  file per subject (`popups.rs`, `activation.rs`, `dialogs.rs`, `window_cap.rs`, `scale.rs`,
  `dmabuf.rs`, `selection.rs`, `outputs.rs`). `seats.rs` in it tells agents' seats: windows on
  their own outputs, the test that no input of one seat reaches another, the owner's or a client
  that bound another seat's `wl_seat`, a click's focus, text read back through the keymaps the
  client got, wheel and cursor, a popup grab and a drag from a press, activation and the
  clipboard kept to their seat, and a seat's closing; its
  clients are `input_client.rs`'s, which bind a seat by name and hold a text field.
  `text_input.rs` in it tells text fields: entered and left with the keyboard, text outside
  `us` committed, text composed, long text in pieces, and no seat's text in another's field.
  `captures.rs` in it captures through MCP: an output's logical image, its edges on whole units, a frame on its way answered
  at its boundary, a window, a region and the physical size with `pointer` aimed through them,
  the refusals, a closed seat, the last 16 ids, images too large. `screencopy.rs` captures
  through `ext-image-copy-capture-v1`, with `capture_client.rs`: both formats, a later frame
  waiting for new content at rest, another seat's output stopped, buffers that do not fit, a
  removed output, the bound on sessions, a client that leaves with a frame waiting. `mcp.rs` in it drives the MCP
  server through two socket pairs, its input and its output: both revisions, every tool but
  `spawn`, two seats that keep to their own, waits, cancellation and timeouts, a client that does
  not read, a line too long, an input that closes.
- `misbehaving.rs` in it: clients that ignore their configures, commit 1 × 1 or 8,192 × 8,192
  pixels, send absurd limits and geometry, break the protocol, shrink a buffer's pool under Yuna
  (SIGBUS, which Smithay turns into a protocol error; the client is freed in that round, with no
  other client speaking), leave mid-frame or holding a grab, a selection and tokens, flood with
  10,000 commits, or never read their socket. Each story ends the same way: before any client
  speaks, and again after, the layout keeps its invariants and agrees with the shell
  (`Compositor::check_windows`), and the popups and the keyboard are sound
  (`Compositor::check_popups_and_keyboard`), both hidden from the documentation, for tests; a
  well-behaved *sentinel* client, asking a frame callback once the output is idle, gets it in
  the round that reads its commit, then, asking again, at the next frame's deadline exactly; and
  Yuna rests.
- `random_requests.rs` in it: proptest's random sequences of up to 99 steps. Up to three clients
  send any request with any values: toplevels, acknowledgements, buffers of 1 to 2,048 pixels a
  side, limits, geometry, fullscreen, maximise, parents, popups with any positioner, grabs,
  activation tokens, activations, viewports, fractional scale, selections, destruction in any
  order, departure, second roles; the test moves the clock by up to 2 s, presses key bindings,
  and focuses a window and moves past the next refresh, which gives that window the keyboard. A
  request that breaks the protocol ends its client, which is allowed. A step in six is a
  request of the client that has the keyboard, sent as a well-behaved client would: a token for
  the surface its keyboard is on with its last `enter` serial, an activation of another of its
  windows with its newest token, a grabbing menu on that surface, a selection. After each step
  Yuna must dispatch, the layout keep its invariants and agree with the shell, and the popups and
  the keyboard be sound; after the sequence, the sentinel, focused by a key binding, gets its
  frame callbacks as in `misbehaving.rs`, and Yuna rests once every client has gone. Two tests share
  the cases, so that they run at once on several cores: 80 cases start from a busy state, three
  clients with a window, a dialog and a menu each; 80 from the sentinel alone.
  `sequences_found_before` replays the sequences found failing, written out, since a seed in
  `random_requests.proptest-regressions` replays its case only while the generator stays the
  same. Each test prints how often the requests that need the keyboard took effect, as the
  compositor's state and the clients' own events show it; those rates and the tests' times are
  in [`measurements.md`](measurements.md). `PROPTEST_CASES=<n>` runs `n` cases in each test
  ([`CONTRIBUTING.md`](../CONTRIBUTING.md)).
- `crates/yuna/tests/command_line.rs`: the program as a user runs it, its socket and its globals
  included.
- `crates/yuna/tests/mcp.rs`: `yuna mcp` as an MCP client runs it: its programs' environment and
  logs, an orphan adopted, `stop`, the whole tree ended when its input closes, a seat's closing
  that ends its processes and no other seat's, a seat whose programs exhaust Yuna's descriptors,
  and Alacritty started through `spawn`, found through `windows`, clicked and typed into: its
  shell runs the command typed. Applications get the environment of
  `tests/common/application_environment.rs`, which `applications.rs` shares.
- `crates/yuna/tests/applications.rs`: real applications in the program, `yuna --headless` or
  `yuna mcp`, as agents run it ([ADR-0025](adr/0025-applications-in-ci.md),
  [ADR-0029](adr/0029-the-rest-of-animated-applications.md)). One `yuna` per application, all at
  once: Alacritty, `gtk4-demo` and Qt's `qml` on a still file show their window, go quiet (no
  round for 2 s), and Yuna must then rest 10 s: no context switch in any of its threads, read in
  `/proc`, and no frame, frame callback, configure, commit or wake-up, read in its log. `vkcube`
  draws 120 frames and leaves; Yuna must get its 120 commits, draw at least 60 frames, a margin
  for a slow runner where every run measured drew them all, and rest after it; left to draw, it
  must fail the same check. Yuna and each application get an environment of their own, nothing of the caller's:
  no `DISPLAY` and no D-Bus, so that they reach neither the user's desktop nor an accessibility
  bus; a `PATH` of the store alone; throwaway home and XDG folders, the system's included;
  DejaVu alone (`YUNA_FONTS_CONF`); Mesa on the CPU, lavapipe alone for Vulkan, without the
  machine's layers; cursors that never blink; GTK's animations off; GTK's renderer and Qt's
  pinned. What still reads the machine is listed in the file's documentation. Every process it
  starts dies with the test's thread. The test takes about 15 s. Through `yuna mcp`, as an agent
  drives it, `vkcube` paused by its space key rests with its window open. SlimeEngine's examples,
  built by the weekly job, run in ignored tests of the same file
  ([ADR-0043](adr/0043-text-input-and-the-rest-of-slimeengines-examples.md)): `window` draws
  120 frames and leaves; `editor` rests with its window open; and M1.5's done-when, an agent
  that opens `interface` through MCP alone, clicks its text field, types `Grüße`, shows text
  being composed and clicks a button; before each step, the part it changes is captured once
  it stands still, and must differ after it.

## The tools around the code

- `flake.nix`: the development shell; `rust-toolchain.toml`: the toolchain
  ([ADR-0016](adr/0016-the-rust-toolchain.md)).
- `scripts/check.sh`: every automated check, which CI runs (`.github/workflows/ci.yml`).
- `.github/workflows/slimeengine.yml`: SlimeEngine's `window`, `editor` and `interface`
  examples in Yuna, weekly, on demand and when the drawing path or what the agent's story uses
  changes, built at a pinned revision with their data cooked, and cached.
- `crates/yuna/examples/frame_time.rs`: a frame's cost while `vkcube` draws, read from outside
  the program: its frames and `took_us` from its log, each thread's CPU time from `/proc`, the
  GPU's from the kernel's DRM usage statistics
  ([ADR-0030](adr/0030-a-frames-gpu-time-from-the-kernel.md)).
- `crates/yuna/examples/agent_time.rs`: what an agent's calls cost in `yuna mcp`: an MCP
  call's time at rest and under load, and a capture's, its copy on the main thread and its
  encoding read from the log, with the frames missed while captures run.
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
