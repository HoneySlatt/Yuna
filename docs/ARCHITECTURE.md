# Architecture

How the code is organised now, for a developer who has never seen it. Decisions and their reasons
are in [`adr/`](adr/README.md); the contracts of each item are in the code's documentation; what
comes next is in [`PLAN.md`](PLAN.md).

## In one page

The parts, bottom up:

- **The pure models**: [`yuna-layout`](#the-layout-model) decides where every window goes;
  [`yuna-motion`](#motion) turns the layout's jumps into what each frame draws. Neither knows
  Wayland, Smithay or a renderer.
- **The headless compositor**, the `yuna` crate's library on Smithay: the
  [protocols](#a-windows-life) and [XWayland](#xwayland) in front, the layout and the motion
  behind, virtual [outputs](#outputs-added-and-removed) with their [clock](#time-and-deadlines),
  drawn through Smithay's GLES renderer, on llvmpipe or the GPU's render node.
- **The [MCP server](#the-mcp-server)** around it, for agents; the [program](#the-program),
  `yuna --headless` and `yuna mcp`.
- **[The rivals' bench](#the-rivals-bench)**, `yuna-bench`, apart: it measures every compositor,
  Yuna included, from outside.

The main flows:

- **A client's commit** reaches the compositor, changes the layout, and the next
  [frame](#a-frame) is drawn on damage ahead of its boundary, presented honestly, its feedback
  sent.
- **An agent's call** over MCP creates a [seat](#seats-and-input), starts a program, types,
  clicks and [captures](#captures), on the agent's own seat only.
- **A change of the layout** becomes [motion](#motion) sampled at each frame's presentation.
- **An X11 program** connects to its seat's Xwayland, started on demand, managed by Yuna's own
  [X11 window manager](#an-x11-windows-life).

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
- **`yuna-motion`** (`crates/yuna-motion`): motion. Its springs and curves are functions of
  time alone, exact at their end ([ADR-0053](adr/0053-springs-in-closed-form-keeping-their-velocity.md)).
  Its `Motion` reads the layout after each change and turns its jumps into what each frame
  draws, and which windows are told they are visible
  ([ADR-0052](adr/0052-motion-in-a-pure-crate.md), [ADR-0054](adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md));
  its crate documentation explains how. It holds no Wayland or Smithay object.
- **`yuna-bench`** (`crates/yuna-bench`): the rivals' bench. It runs a compositor headless in a
  sandbox of its own, the same scenarios with the same clients for each, and reads its costs
  from outside ([ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md));
  [The rivals' bench](#the-rivals-bench) below. It depends on nothing of `yuna`: it measures
  Yuna as it measures the others.

## Dependencies

- `yuna` → Smithay, at a pinned git revision, with only its `wayland_frontend`, `desktop` and
  `renderer_gl` features ([ADR-0011](adr/0011-built-on-smithay.md),
  [ADR-0022](adr/0022-the-headless-backend-in-yuna.md)); `tracing` and `tracing-subscriber` for
  its logs; `serde`, with its derive, and `serde_json` for MCP's messages
  ([ADR-0033](adr/0033-the-mcp-server.md)); `png` for a capture's image
  ([ADR-0034](adr/0034-captures.md)), which builds `miniz_oxide` twice, 0.8 for itself and 0.9
  for `flate2`; x11rb's `RustConnection`, the X11 protocol in Rust, for its X11 window manager
  ([ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md)); `yuna-layout` and
  `yuna-motion`. rustix, which Smithay re-exports, makes its system calls: pipes, sockets,
  signals, `waitpid`, pidfds, `prctl`. Its tests use `wayland-client` and `wayland-protocols`'
  client side, staging protocols included, proptest, without its default features, `png` to
  decode captures, and x11rb for their X11 clients; its measuring examples read `/proc`, the
  DRM statistics and the machine's load through `yuna-bench`'s library.
- `yuna-layout` → nothing. Its tests use proptest, without its default features.
- `yuna-motion` → `yuna-layout`. Its tests use proptest, without its default features.
- `yuna-bench` → `wayland-client` and `wayland-protocols`' client side, for its own clients;
  `wayland-protocols-misc`, for its virtual keyboard's `zwp_virtual_keyboard_v1`; `serde` and
  `serde_json`, for its run file and mpv's IPC; rustix, already Smithay's, for CLOCK_MONOTONIC,
  signals, the user's id, memfds and the kernel's socket diagnostics. At run time, from the bench
  shell: the compositors of its nine recipes, sway (for sway and sway-vulkan), KWin, dwl and
  mango (each also built without XWayland), Umbriel, Hyprland (through `start-hyprland`, inside a
  dwl) and Mutter, and Yuna, which it builds from the workspace with `cargo`; their tools,
  `swaymsg`, `dbus-send`, `wlr-randr`, `mmsg`, `umbriel`, `hyprctl` and `gdctl`; mpv, from the
  applications' `PATH`, and weston's `weston-presentation-shm`, under coreutils' `stdbuf`, as
  clients; Xwayland, which the rivals as shipped start; ffmpeg, for its clips; D-Bus's
  `dbus-run-session` and `dbus-daemon`; coreutils' `env`, and bash; `git`, for the commit it
  records; and the host's `systemd-run`, which makes each sandbox.
- At run time, from the development shell: libxkbcommon, linked; libglvnd's EGL and the shell's
  own Mesa, loaded, never the host's drivers; Xwayland, run, found on `PATH`.

## The layout model

`yuna-layout` is a tree that each seat owns a branch of:

```text
Layout
├── options: gaps, widths, default tiling, dynamic workspaces
├── windows: what is known of each window beyond its place (its sizes, its parent, fake
│   fullscreen, last floating place)
├── ids: the counters that name the workspaces and columns the model creates
└── seats
    └── Seat (the owner's, or an agent's)
        ├── outputs
        │   └── Output: rectangle, scale
        │       ├── workspaces, one shown
        │       │   └── Workspace: id, name, home output
        │       │       ├── tiling: scrolling columns, each with an id, or a dwindle tree
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
Nothing in the model runs between operations. The compositor hands the same layout to
`yuna-motion`'s `Motion`, which turns the jump into motion: each frame draws the windows where the
motion has them at the time the frame is shown, until they rest at their places
([A frame](#a-frame), [ADR-0052](adr/0052-motion-in-a-pure-crate.md)). `Layout::arrange_workspace(output, workspace)`
arranges any of an output's workspaces, named by its id, as that output would show it alone,
changing nothing: what the owner's peek at a workspace not shown draws, in a thumbnail or an
overview ([ADR-0015](adr/0015-agents-workspaces.md)). Nothing draws a peek before the desktop.

The model jumps from one state to the next. So that animations can follow windows, columns and
workspaces across a jump ([ADR-0052](adr/0052-motion-in-a-pure-crate.md)), the model names the
workspaces and scrolling columns it creates, from counters that never give an id twice, and each
window `arrange` places also says:

- its **workspace**, by id: the one its output shows, or the special one over it;
- its **layer**: on the strip of scrolling columns, in the tiles of dwindle, floating or pinned;
- its **column**, by id, on a strip.

A workspace keeps its id when it moves to another output or loses its name; a column keeps its
id while it exists, wherever it moves along its strip and whatever its width.
`Layout::shown_workspaces(output)` gives each workspace an output shows with its working area
and its **camera**, `view_x`: in scrolling columns, the position on the strip shown at the
working area's left edge. A window's strip position, `rect.x - working.x + view_x`, changes only
when its column moves along the strip or it changes column; focusing another column moves the
camera alone. `WorkspaceView`'s documentation holds the whole rule and its one limit.

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
  changes nothing, that `Layout::size_on_open` foretells a new window's size, that a peek
  at each workspace shows its own windows, the one shown as its output draws them, that each
  window's workspace and layer are right, that no workspace or column id comes back once gone,
  and that moving the focus moves no window along its strip;
- `crates/yuna-layout/examples/timing.rs`: an operation's time on workspaces of 100 and 1,000
  windows.

## Motion

`yuna-motion` turns the layout's jumps into what each frame draws
([ADR-0052](adr/0052-motion-in-a-pure-crate.md)); its crate documentation gives the detail.

- `spring.rs`, `oscillator.rs`, `curve.rs`: a spring in closed form, its end within half a
  pixel, retargeted keeping its velocity; Bézier curves and named easings
  ([ADR-0053](adr/0053-springs-in-closed-form-keeping-their-velocity.md)).
- `arrangement.rs`: `Motion`, its `Settings` and `Change`. `update` reads every output of
  every seat after a change; `frame` gives what an output draws at a time; `end` when its
  motions end; `told_visible` which windows are told they are shown.
- `arrangement/output.rs`: one output's motion: each moved window's offset, each scrolling
  workspace's camera and its anchor column, rounding once to the physical grid.
- `arrangement/switch.rs`, `special.rs`: the workspace switch over the workspaces involved, and
  the special workspace's fade.

Its tests: `tests/spring.rs` and `tests/curve.rs` against a numerical integration and known
values; `tests/arrangement.rs` and `tests/arrangement_still.rs`, scenarios of each behaviour,
camera and windows on different springs among them; `tests/arrangement_properties.rs`, random
sequences of operations, clock steps and settings, checking that nothing jumps and that each
output rests on its arrangement. `examples/motion_timing.rs` times `update` and `frame`.

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
- `shell.rs`: `xdg-shell` and `xdg-decoration` in front of the layout, and the record of windows
  both shells share, `Toplevel`: an `xdg_toplevel`, or an X11 window paired with its Wayland
  surface. It maps each to a `WindowId`, passes on what its client asks, tells it what each
  arrangement says, and caps the windows of each client.
- `popups.rs`: popups (`xdg_popup`), through Smithay's `PopupManager`: their placement, their
  configures, their keyboard grabs, their dismissal.
- `activation.rs`: `xdg-activation-v1`: tokens, activation through the layout, urgent windows.
- `dialog.rs`: `xdg-dialog-v1`: offered; the modal hint is read, and nothing uses it yet.
- `selection.rs`: the clipboard (`wl_data_device`), the primary selection and
  `ext-data-control-v1`, on Smithay's handlers.
- `frames.rs`: the frame loop: when an output refreshes, what is arranged, the motion each frame
  samples, what is drawn, called back and reported presented, what clients are told, and the
  hidden windows.
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
  the module that draws ([ADR-0012](adr/0012-opengl-first.md)), though GLES types reach
  `state.rs` (the renderer), `output.rs` (each output's buffer and damage tracker) and
  `dmabuf.rs` (the import); `read_pixels` is the
  only place pixels are read back. It keeps each EGL
  display it opens until the process ends, so that compositors started and dropped on several
  threads, as tests run them, never lose the display Smithay shares among them.
- `clock.rs`: the clock and its deadlines ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)).
- `xwayland/`: each seat's X display, its Xwayland and its X11 window manager,
  [below](#xwayland): `display.rs`, the display's number, sockets, lock and cookie; `server.rs`,
  Xwayland's process; `manager.rs`, the manager's thread; `properties.rs`, the X11 properties it
  reads, decoded and bounded; `shell.rs`, `xwayland-shell-v1`; `windows.rs`, X11 windows on
  Yuna's side ([An X11 window's life](#an-x11-windows-life)); `menus.rs`, override-redirect
  windows, menus and tooltips; `answers.rs`, the sync requests and pings Yuna waits on;
  `mod.rs`, each display's phases on Yuna's loop.
- `process.rs`: what every program Yuna starts shares, MCP's and Xwayland: the descriptors it
  does not inherit.
- `mcp/`: the MCP server, [below](#the-mcp-server).
- `main.rs` and `stop_signal.rs`: the program alone: its command line, its logs, SIGTERM, and
  SIGCHLD for `yuna mcp`.

### Time and deadlines

The clock owns every deadline of the frame loop: two per output, its next refresh and the
boundary its held presentation feedback waits for, one for the hidden windows, and one for the
X11 windows' sync requests and pings ([An X11 window's life](#an-x11-windows-life)). Each round of
the loop runs the deadlines due by its end, so that a refresh armed at once, on an idle output,
runs in the round that read the commit. In real mode, time is `CLOCK_MONOTONIC`, and the
compositor keeps one calloop timer armed at the earliest deadline, armed again before each wait;
with no deadline, no timer. In manual mode, for tests, time starts at the real monotonic time and
moves only through `Compositor::advance`, which runs each deadline it passes at its own time, in
order. A draw hook can pass manual time while a frame draws, to make drawing late.

An output's refresh boundary `k` comes `k / rate` seconds after the output was created, computed
from the rate each time, so that errors never add up. An output under VRR has no fixed
boundaries: its panel (`vrr.rs`) says when a frame ready at a given time starts showing, and
counts its refreshes ([VRR outputs](#vrr-outputs)). The frame loop asks both kinds the same
questions, through `Timing` (`output.rs`).

A commit, or a change of the layout, arms its output's refresh
([ADR-0031](adr/0031-frames-drawn-ahead-of-their-boundary.md)); while the output's motion runs,
each refresh arms the next. When the output is idle, the boundary after its last refresh passed,
the refresh is due at once and aims at the first boundary at or after it; its motion is sampled at
the boundary its drawing can reach, the first at or after now plus its expected drawing: the
longest drawing of the last half second, or after rest the last one, at most one period, without
the lead's floor and margin. Otherwise it is due at a boundary minus the output's lead, the first
after its last refresh whose deadline is not past, and samples its motion at that boundary. While
the output's motion runs, that boundary also comes after the one its last frame sampled
([A frame](#a-frame)). The lead is the longest drawing of the last half second, timed from its
deadline to its end, at least 2 ms, plus 1 ms, at most one period (`frames.rs`, `Drawings`). On
the manual clock a drawing takes no time unless the draw hook passes some: the lead is 3 ms, and
a drawing at once is expected to take none.

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

`Compositor::close_window` asks a window's client to close it: `xdg_toplevel.close`, or for an
X11 window `WM_DELETE_WINDOW`, else the end of its X11 client.

### An X11 window's life

An X11 window joins the same record and the same layout (`xwayland/windows.rs`,
[ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md)). Its manager's thread
(`xwayland/manager.rs`) follows each child of the root, reads the properties Yuna needs when it
appears and when one changes, and posts what it learns as events to Yuna's loop; Yuna's decisions
go back as commands, passed to the manager at the round's end (`Compositor::flush`):

| The manager posts | Yuna sends back |
|---|---|
| `Created`: the window, its X11 client, override-redirect or not, its rectangle, its process from XRes, its properties | |
| `Properties`: all of them again, when one changed | |
| `MapRequest` | `Configure` to the size it opens at, then `Map` |
| `Mapped`, override-redirect or not; `Unmapped`; `Destroyed` | |
| `ConfigureRequest`, with what it asks | `Configure`: granted until it asks to be mapped, the place Yuna gives it after |
| `Configured`, `Restacked`: an override-redirect window moved, or restacked among the others | |
| `SurfaceSerial`, from Xwayland's `WL_SURFACE_SERIAL` | |
| `StateRequest`: fullscreen or maximised, added, removed or toggled | `State`: `_NET_WM_STATE` |
| `ActivateRequest`, from `_NET_ACTIVE_WINDOW` | |
| | `Focus`, as the seat's keyboard moves |
| `SyncCounter`: a window's sync counter changed | `Configure` with a sync request; `AllowCommits` |
| `PingAnswered` | `Close`, then `Ping` |
| | `Restack` |

What ICCCM and EWMH settle alone, the manager does itself: `WM_STATE`, `_NET_FRAME_EXTENTS` of
0, the root's `_NET_CLIENT_LIST`, `_NET_CLIENT_LIST_STACKING` and `_NET_ACTIVE_WINDOW`, the
synthetic `ConfigureNotify` after each configure (ICCCM 4.1.5), the focus model (ICCCM 4.1.7:
`SetInputFocus` for a window that takes input, `WM_TAKE_FOCUS` for one that lists it),
`WM_DELETE_WINDOW` or `KillClient`, and, at a withdrawal, the deletion of `_NET_WM_STATE`
(EWMH), so that a window mapped again starts from what its client sets then. It follows X11's
stacking of the root's children from their events, to tell Yuna where each override-redirect
window lies among the others. Each property read is bounded, 64 KiB of text, 64 atoms of a
list; a malformed value is taken as unset, whatever the property held before, logged once per
window. Titles are read in UTF-8, in Latin-1 for `STRING`, and `COMPOUND_TEXT` as Latin-1. Any
X11 client may send any event to the root (`SendEvent`), which the server marks as sent: the
manager takes no sent event but a client message, so that a made-up `DestroyNotify` or
`MapRequest` of another program's window changes nothing, and among client messages no
`WL_SURFACE_SERIAL` sent so: only Xwayland's own pairs a window with a surface. An event x11rb
cannot read is logged and left. The manager never reparents: Yuna draws no frames
([ADR-0024](adr/0024-no-title-bars.md); Java's AWT then needs `_JAVA_AWT_WM_NONREPARENTING=1`,
[debt](debt.md#xwayland)).

1. The window is created: Yuna records it, its X11 client, the bits of its id that name the
   client, its rectangle and its properties.
2. It asks to be mapped. A managed window, neither override-redirect nor a dock or a desktop, is
   configured to the size `Layout::size_on_open` foretells, from its size hints, its
   `WM_TRANSIENT_FOR` and its `_NET_WM_STATE`, at its seat's active output, keeping its own size
   on an axis the layout leaves to it, as a floating dialog's; then mapped. Its
   `_NET_STARTUP_ID` is read then: when it names an `xdg-activation-v1` token Yuna gave and that
   was not used, the token is used, and the window's opening is judged as an activation with it
   ([Activation and dialogs](#activation-and-dialogs)). A dock or a desktop is mapped as it asks
   and never shown.
3. Xwayland makes its Wayland surface, gives it the role (`xwayland-shell-v1`, a global Xwayland's
   client alone sees, `xwayland/shell.rs`) and a serial, applied at the surface's commit, and
   sends the same serial to the manager. The window and its surface are paired in either order:
   a managed window gets its `Toplevel`. A serial naming a window Yuna does not know pairs
   nothing. Xwayland itself posting a second role is the only error Yuna posts it: what one X11
   client does never ends the connection every X11 program of the seat goes through
   ([ADR-0067](adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)).
4. Mapped, paired and with a buffer, it opens in the layout as an `xdg_toplevel` does at its
   first buffer: its limits are its size hints less its frame extents (`_GTK_FRAME_EXTENTS`),
   divided by XWayland's scale S and rounded outward; its parent, the window its
   `WM_TRANSIENT_FOR` names if that one is open; its size, its surface's less its frame extents,
   over S. The caps count the X11 client behind it, never Xwayland, one Wayland client for every
   X11 program of its seat
   ([ADR-0028](adr/0028-bounds-on-what-one-client-may-ask.md)). Later commits pass its size and
   limits on as an xdg window's do.
5. At each arrangement it is told its place (`tell_x11`): an X11 rectangle, the place's corner
   times S and its size round(S × L) for the logical size L asked, widened by its frame extents,
   relative to the root's origin, Yuna's logical origin; on an axis asked 0, which a floating
   window chooses itself, its own size. With it, its `_NET_WM_STATE`: fullscreen, maximised when
   full width, hidden when it is. A fullscreen window is at its output's corner;
   on an output whose mode its client emulates, it gets the rectangle Xwayland gives that mode in
   `_XWAYLAND_RANDR_EMU_MONITOR_RECTS`, so that Xwayland scales its surface to the output
   through a viewport
   ([ADR-0069](adr/0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md)). A
   hidden window keeps its X11 place, and the maximised state it was told. Only a change is sent. A
   window's own `ConfigureRequest` is granted until it asks to be mapped; then it is answered with
   the place Yuna gives it, the one foretold until it opens, but on an axis the layout leaves to it,
   where it is granted, and for a fullscreen window shown on an output asking its emulated
   mode's rectangle there, which is granted. The windows are then stacked: those no output shows
   at the bottom, the tiled ones shown by id, those over them last, for Xwayland finds the
   window under the pointer through X11's stacking.
6. A configure that changes the size of a mapped window that lists `_NET_WM_SYNC_REQUEST` and
   names a counter in `_NET_WM_SYNC_REQUEST_COUNTER` goes with a sync request of the next value,
   from 1 for each counter, the manager setting the counter to 0 and watching it through an XSync
   alarm (`xwayland/answers.rs`). Xwayland holds the window's commits meanwhile
   (`_XWAYLAND_ALLOW_COMMITS` 0), so that Yuna keeps drawing its last complete frame, until the
   counter reaches the value or 100 ms pass (`X11_SYNC_TIMEOUT`), then commits again. A request
   reaching a manager that saw the window unmapped meanwhile is answered at once, and a window
   mapped is never left with its commits held. One request waits at a time: a configure meanwhile
   goes without one, and the wait keeps its deadline, so that a window resized again and again is
   never held past it. A window that misses it is logged once.
7. The seat's keyboard on its surface gives it the X input focus, and `_NET_ACTIVE_WINDOW`;
   leaving the seat's X11 windows gives it to none. A client's `_NET_ACTIVE_WINDOW` is an
   activation without a token: the window takes the focus when its client has it, else is
   marked urgent.
8. Asked to close, it is sent `WM_DELETE_WINDOW`, or its client is ended. One that lists
   `_NET_WM_PING` is pinged too: an answer within 5 s (`X11_PING_TIMEOUT`) does nothing more;
   without one it is marked unresponsive, which MCP's `windows` gives, until it answers a later
   ping. It is never ended on a timer.
9. Unmapped, it leaves the layout, what it was waited for is let go, and its manager deletes its
   `_NET_WM_STATE`, and its `_XWAYLAND_ALLOW_COMMITS` if a sync request held it; destroyed, Yuna
   forgets it. Xwayland destroys its surface when it is unmapped, which closes it too.

The sync request's and the ping's deadline is one of the clock's, armed at the earliest time one
waits until, and only while one waits ([Time and deadlines](#time-and-deadlines)): a wait added
arms it at its own time, the clock keeping the earlier; a wait that goes, its window or its
Xwayland with it, has it armed again from those left.

An override-redirect window, a menu, a tooltip or a drag's icon, is placed and mapped by its
client alone (`xwayland/menus.rs`). It is shown once mapped and paired with a surface that has a
buffer; never in the layout, never focused by Yuna: X11 menus grab the input within Xwayland, so
the keyboard stays on the focused window, and a click on a menu moves no focus. When it is shown,
and again when its client moves it or names another window in `WM_TRANSIENT_FOR`, it is given a
window, by the first of these rules: the managed window its `WM_TRANSIENT_FOR` names, if that one
is open; else, among the managed windows of its own X11 client shown on an output, topmost first,
the one shown on the output under the menu's X11 corner whose told X11 rectangle holds that
corner; else the topmost of them. With one, it keeps its offset from that window's X11 corner as
Yuna told it then, and is drawn just above the window, below the windows over it, as a popup is:
at the window's surface's corner, where the frame draws it, plus that offset × s / S, exactly the
offset at s = S, cut to the output, not to the window, at the window's alpha, on each output the
window is drawn on. It so slides with the window's drawn rectangle
([ADR-0054](adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)), and its surface
is one of the window's surface trees: called back, told presented and entering outputs with it.
The offset is kept from the placement on, a decision: computed again from the window's X11 corner
at each frame, the menu would jump when an arrangement tells the window its new place while its
drawn rectangle has yet to slide there; kept, it follows the window across a column's move, and X11
still has it at its own place ([debt](debt.md#xwayland)). A window that leaves the layout, unmapped,
destroyed, its surface gone or committed without a buffer, gives its menus to the rules again.
Without one, it is drawn at its X11 position less the output's X11 corner × s / S, over every window
of each output of the seat it lies on, called back by each output that draws it, else at the hidden
windows' rate. Menus are stacked among themselves as X11 stacks them, and are drawn through the X11
element as any X11 surface is ([Scale](#scale)). An X11 client shows 64 at most
(`MAX_MENUS_PER_CLIENT`), as a window has 64 popups: one past it is not shown until another goes,
logged once per client. The frame loop, the pointer and each window's surface trees walk the menus
shown alone, through a list of them in stacking order, their count per client and an index by
window, never the mapped override-redirect windows, which a client may have by the thousand.

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

An X11 window's `_NET_STARTUP_ID`, read when it asks to be mapped, may name a token: one Yuna
gave and not used yet is used, and the window's opening is judged as an activation with it, as
one asked before its mapping is ([An X11 window's life](#an-x11-windows-life)).

`xdg-dialog-v1` is offered. Dialogs already open on their parent through `xdg_toplevel.set_parent`;
the modal hint (`dialog.rs`) is read from Smithay's record of the toplevel, and nothing uses it
yet.

### VRR outputs

An output created with a VRR range ([ADR-0046](adr/0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md),
[ADR-0051](adr/0051-emulated-vrr-as-the-monitors-traces-showed.md)) emulates amdgpu driving a
DisplayPort Adaptive-Sync monitor, as the owner's monitor's traces showed it:

- A frame shows a vertical blank after it is ready at the soonest: 269 of 2,429 lines of the
  shortest refresh, the owner's mode's, 461 µs at 240 Hz.
- A refresh lasts at least 1/max and at most 1/min. A frame ready sooner than 1/max after the
  refresh in progress began waits; a later one starts a refresh a vertical blank later. With no
  new frame, the
  panel refreshes by itself every 1/min.
- Below the range, BTR, freesync.c's rule, shows each frame `n` times; the first `n − 1`
  refreshes have a fixed length, chosen from the time between the last two frames, and a frame
  ready during one of them shows at its end; the last is variable again.
- None of these refreshes is a timer: the panel computes them when a frame asks when it would
  show. An output at rest wakes nothing.
- The frame loop is the same as at a fixed rate: at once when a refresh could begin now, else
  at the refresh the frame can reach minus the lead. A refresh with nothing new to draw, which
  answers callbacks asked without a buffer, comes at most once a shortest refresh. Something new
  is never held back by it (`content_pending`): a buffer attached or removed anywhere in the
  committed tree, read before Smithay takes it, a change of the layout, a popup gone. After two
  refreshes in a row armed for something new that drew nothing, all is paced again
  (`content_misses`), so that buffers outside the output never loop.
- While a motion runs, a refresh that finds nothing changed shows the same frame again: the
  panel then refreshes where the frame loop counts a refresh, and the motion keeps the shortest
  refresh to its end. Shown nothing, the panel would make no refresh there: the next frame would
  show sooner than a shortest refresh after the one counted, its feedback numbered after it, and
  after about 20 ms without a frame the panel would insert refreshes below its range.
- `seq` counts every refresh, the panel's own included; `wp_presentation` version 2 is told
  1/max as the refresh, version 1 zero.
- The range is at least twice as wide as its least rate; a narrower one takes a path of the
  driver Yuna does not emulate.
- A test replays both traces, every frame (`tests/fixtures/`): what the emulation does not
  reproduce is in the [debt](debt.md#vrr).

### A frame

At an output's refresh, due at once or a lead before the boundary it aims at:

1. If the layout changed, every output is arranged twice: by `Motion::update`, which turns what
   moved into motion, at the refresh's time
   ([ADR-0054](adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)), then by the
   compositor, which keeps the places for what follows them: configures, popups and MCP's
   `windows`. A change is animated, unless an output or a seat was removed since the last
   arrangement: then it is instant for that seat alone, its outputs' motions ending at once while
   the other seats' go on, so that an agent's outputs coming and going never cut the owner's
   motion short ([ADR-0015](adr/0015-agents-workspaces.md)). An output added starts at rest.
   Changes made before one arrangement combine, instant wherever one of them is
   (`Change::and`). With animations off, the default of both headless modes, every change is
   instant ([ADR-0055](adr/0055-what-moves-first-and-animations-off-headless.md)).
   Each window is told its asked size and its states: tiled, fullscreen, activated if it has its
   seat's focus, suspended if it is told it is hidden, as below. A configure goes out only when
   its size or states changed, or when a request of its client is owed one. An X11 window is
   told its X11 rectangle and its `_NET_WM_STATE` the same way
   ([An X11 window's life](#an-x11-windows-life)). Each seat's keyboard focus follows
   `Layout::focused` for that seat, or the topmost popup of the seat's grab on that window: only
   here does it change window. The seat's selections follow it; the focused window
   is no longer urgent. Each agent's pointer is put on what is under it now, if that changed,
   unless windows move on its output.
2. The motion gives the output's frame at the time it is shown if on time: the boundary the
   refresh aims at, or, for a refresh due at once, the one its drawing reaches if it takes as
   long as the output's longest drawing of the last half second, or after rest its last one, at
   most one period, without the lead's floor and margin; on the manual clock with no draw hook,
   the first boundary at or after now. While the output's motion runs, each refresh samples a
   later time than its last frame did, at a fixed rate the boundary after at least, so that the
   motion never steps back. At rest this rule does not apply, so that a client's frames keep the
   soonest boundary they can reach: the frame is the arrangement whatever the time. Each window
   has a rectangle and an alpha there; at rest, and with animations off, they are its place and
   1.
   The output is drawn if anything on it is damaged: each window the frame shows on it, bottom
   to top, its surface tree with its window geometry at its rectangle's corner, cut to the
   rectangle, at its alpha, then its popups just above it, at the same alpha, over a plain
   background; an X11 window, its surface through Yuna's own element ([Scale](#scale)), and its
   menus just above it. X11 menus drawn with no window are above every window
   ([An X11 window's life](#an-x11-windows-life)).
   Surfaces sit on whole pixels: where the geometry's offset in its surface is not a whole
   number of pixels, it is rounded up, so that the cut takes a sliver of content rather
   than show the window's margin. A surface drawn at its output's physical size is drawn exactly
   ([Scale](#scale)). A popup's corner is rounded to the nearest pixel, and a popup is cut to the
   output, not to its window: a menu may overflow it. Yuna waits until the drawing is done. The
   output keeps the frame's list, which captures cut ([Captures](#captures)).
3. The surfaces it shows, windows' and their popups', and those of the windows it tells they are
   visible, get their frame callbacks. The presentation feedback of those it shows is taken and
   held by the output until the boundary the frame is presented at: the one it aimed at if its
   drawing ended by then, else the first after. It is sent at that boundary, through the output's
   presentation deadline, armed only when a surface asked feedback. `seq` is the boundary's
   number, the flags VSYNC, the refresh the output's period. A refresh that drew nothing still
   counts as the output's last refresh, so that callbacks asked without damage come once a
   period; under VRR, while a motion runs, it shows its frame again ([VRR outputs](#vrr-outputs)).
4. While the output's motion is not over at the time sampled, the refresh arms the next one, as
   for something new, whether it drew, found nothing changed or failed. The last frame shows the
   arrangement exactly; then nothing is armed. At that refresh the windows are told once what the
   motion's end says, and the pointer on that output is put on what is under it.

What is drawn follows the motion; what a client is told follows its place. A window is told it
is visible, `enter` and full-rate callbacks, when its place comes onto an output, at an
arrangement; while the output's motion runs it stays told so as long as it is drawn, and is told
hidden only at the refresh where the motion ends, if its place is not on the output. A window
that only passes through the view is drawn from its last buffer and called back, never told. So
a window gets at most one such change per operation, never one a frame. A frame drawn at once
is shown at the boundary it sampled when its drawing takes about as long as predicted; when the
drawing ends after that boundary, the frame is shown a boundary later, and when it ends before
the boundary before, a boundary sooner. Shown sooner, it stays on screen at the boundary it
sampled, and the motion's next frame samples the boundary after: the motion holds one frame
rather than step back. Under VRR, the next frame is shown a shortest refresh after the first
and samples a later time than the first did; from the frame after it at the latest, each frame
shows where it samples.

What follows what is drawn, hit tests, the pointer's focus and MCP's `windows`, reads an
output's motion at the clock's time, or at the time its last frame sampled if that is later:
never a motion that frame drew past. Once the refresh that sampled a motion's end has run, they
read the output at rest, as that frame shows it, though the clock has not reached the end.

A window hidden, on a workspace not shown or scrolled out of view, is told `suspended`: its
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

X11 windows are drawn by an element of Yuna's own (`render/x11_surface.rs`,
[ADR-0068](adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)). Rootless Xwayland
draws each window at XWayland's scale S, the largest of its seat's outputs when it started, into
a buffer of scale 1: a buffer pixel is an X11 pixel. On an output of scale s, the buffer takes
s / S physical pixels per X11 pixel, its window geometry's corner, past its frame extents, at the
window's rectangle's, its far edges the geometry's size × s / S from that corner, rounded once,
cut to the rectangle. At s = S it is copied texel for texel. Below S each pixel is the average of
the X11 pixels it covers, weighted by how much of each, in a texture shader of Yuna's, the *area
filter*: the average the captures' encoder takes ([Captures](#captures)), of the stored
sRGB-encoded values, not of linear light, as every other drawing of Yuna's blends today. Above S, on
an output added since Xwayland started, the renderer's bilinear filter scales it up. A surface with
a viewport, which Xwayland sets while a fullscreen client emulates a mode, is as large as its
viewport's destination and shows its source: a game at the output's own mode is copied pixel for
pixel. Smithay's surface element is not used for them: it rounds a client-scaled surface to whole
logical units, so that one X11 width in three would be drawn a pixel off at S = 1.5. The pointer
on an X11 window is measured from where this element draws its surface, so that Smithay, which
multiplies it by S for Xwayland, lands it on the X11 pixel drawn under it, at S and below.

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
([ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)).

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
([ADR-0078](adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md), [ADR-0037](adr/0037-seats-tools-input-and-closing.md)).
Seats isolate focus and input, not processes: a program may reach another seat's socket of the
same Yuna. Nothing new depends on several seats in one Yuna until Stage 2's core decides.
The owner's seat, 1, exists from the start; in `yuna mcp` it has no `wl_seat` global, no output
and no socket. An agent's seat gets the next id, never reused, a `wl_seat` named `agent-<id>`,
and a keyboard of the `us` layout whatever the machine's settings. A client belongs to the seat
whose socket it came through, recorded in its `ClientState`: its windows open on that seat's
workspaces, and each seat's focus goes only to its own clients' surfaces. A client is shown its
own seat's `wl_seat` and its seat's outputs' `wl_output`s alone, through the filter per client of
Yuna's fork of Smithay ([ADR-0042](adr/0042-main-on-smithays-fork.md)); a client that held
another seat's would still get no input, since that seat's focus is never on its surfaces.

Input (`input.rs`) reaches an agent's seat through the compositor's calls, which the MCP
server makes for the seat an agent names, and through its own clients' virtual keyboards and
pointers (point 7); the owner's seat takes none. Its events carry Yuna's clock's time.

1. A pointer moves to a place in logical units of one of its seat's outputs. What is under it
   is found among the windows that output draws at the event's time, or at the time its last
   frame sampled if that is later ([A frame](#a-frame)), where the motion has them,
   top to bottom, each window's popups and X11 menus before it, X11 menus with no window before
   every window, a window cut to its rectangle: the surface whose input region holds the point.
   An X11 menu is measured from where its frame draws it, as an X11 window is
   ([Scale](#scale)). Smithay sends `enter`, `leave` and `motion`. After each
   arrangement, a pointer is moved onto what is under it then, when that changed; while windows
   move on its output, its focus waits instead, and a button or a scroll first moves it onto what
   is drawn under it, so that a click reaches what the screen shows; the refresh where the motion
   ends moves it once ([ADR-0054](adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
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
6. X11 windows take the same input through their seat's Xwayland: the keyboard on an X11
   window's surface also gives it the X input focus, which its manager sets
   ([An X11 window's life](#an-x11-windows-life)).
7. A client of an agent's seat may drive it as a test driver drives a headless sway, through
   `zwp_virtual_keyboard_v1` and `zwlr_virtual_pointer_v1` (`virtual_input.rs`,
   [ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)); the owner's seat's clients are
   not shown their globals. A device acts on its client's own seat, read when it is made and
   kept while it lives, so that the keys and buttons Smithay releases when its client dies
   reach that seat; one made for another seat's `wl_seat` does nothing, and a client keeps 8
   that act (`MAX_VIRTUAL_DEVICES_PER_CLIENT`). Its events take the paths of points 1 to 3. A
   virtual keyboard's keys are read with its own keymap: the seat's keyboard switches to it,
   its clients told, and back to the seat's own before Yuna's keys. An absolute motion is a
   fraction of the seat's outputs together, or of the output the device names; a relative one
   moves from where the pointer is; a motion off every output of the seat is dropped.

A seat's socket that fails to `accept`, out of descriptors most often, rests 100 ms
(`SOCKET_PAUSE`) rather than stop Yuna ([ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)).

Removing a seat (`Compositor::remove_seat`), in order: the keys and buttons it holds are
released, its popup grab ends, and its focus leaves every surface; those events are sent; its
socket is removed, and its X display, its Xwayland ended ([XWayland](#xwayland)); its clients are
ended and freed, their windows closed; then its outputs, its
place in the layout, and its `wl_seat` global, disabled now and destroyed later, as a removed
output's is. A seat's outputs added or removed, or the seat removed, never end another seat's
motion ([A frame](#a-frame)). Smithay keeps a record of every seat it made: a removed one's
stays, without its keyboard and pointer.

### Outputs added and removed

An output belongs to one seat. An output added while Yuna runs goes to the right of every
output, and its first frame is armed; its `wl_output` global is announced at the round's end. A
removed output's workspaces go to its seat's active output, which keeps showing its own: their windows are hidden there until a
workspace switch shows them ([ADR-0018](adr/0018-workspaces.md)). Its windows' surfaces leave
it at once, and enter an output again once one shows them; the presentation feedback it
held for a frame not yet at its boundary is discarded; its two deadlines are disarmed. Its global
is disabled at once and destroyed 10 s later on Yuna's clock, at the end of a round, so that a
client that asked to bind it before it learnt of the removal is not ended for it; a closed seat's
`wl_seat` goes the same way. A seat's last output is removed
only with the seat, and a compositor keeps 16 at most (`MAX_OUTPUTS`), each about 33 MB at 4K.

### XWayland

X11 programs run on Xwayland, an X server that is a Wayland client of Yuna's: one per seat,
started by the seat's first X11 connection, ended when idle, each behind a cookie of its own
([ADR-0067](adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)). It is on in
`yuna --headless` and `yuna mcp`, off in tests unless one asks (`Config::xwayland`). Xwayland is
looked for on `PATH` once, when the compositor starts; without it, no seat gets a display, which
is logged once. Its windows are windows of Yuna's layout
([An X11 window's life](#an-x11-windows-life)).

A seat gets its display with its Wayland socket: the owner's when the compositor listens, an
agent's in `Compositor::listen_for_seat` (`xwayland/display.rs`).

1. Its number is the first free one from 0, below 1,000 (`MAX_DISPLAYS`). The abstract socket
   `@/tmp/.X11-unix/X<n>` is bound first: a name taken sends Yuna to the next number, and the
   kernel frees it when Yuna dies. Then the lock `/tmp/.X<n>-lock`, Yuna's process id written
   whole then linked to its name: a lock of a process that no longer exists is replaced, a live
   one or one that cannot be read sends Yuna to the next number. Then, under that lock, the
   file socket `/tmp/.X11-unix/X<n>`, the folder made sticky and open to every user if missing.
   What `/tmp` cannot hold, the display goes without, once said: its abstract socket alone
   serves it, which libxcb tries first. Both sockets listen with a backlog of 128.
2. Its cookie, 16 bytes of `/dev/urandom`, is in an Xauthority file, one `FamilyWild` entry for
   the display's number, mode 0600, written to a temporary file then renamed, in a folder of the
   seat's own, mode 0700, beside its Wayland socket: `<socket>.x11/Xauthority`, in
   `XDG_RUNTIME_DIR` for the owner's seat, in `yuna mcp`'s folder for an agent's.

Each display then goes through three phases, on Yuna's loop (`xwayland/mod.rs`); none of them
waits:

1. **Watching.** calloop watches both sockets. The first connection, left in the backlog,
   starts Xwayland (`xwayland/server.rs`): `Xwayland :<n> -rootless -terminate 10 -wm <fd>
   -displayfd <fd> -listenfd <fd>… -auth <Xauthority> -nolisten tcp`, its Wayland connection a
   socket pair whose end Yuna inserts as a client of the seat, marked as its Xwayland in its
   `ClientState`, with the client scale S, the largest scale among the seat's outputs now, fixed
   for this Xwayland's life
   ([ADR-0068](adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)). On llvmpipe, Xwayland
   also gets `-shm` and `-extension GLX`: Mesa's GLX for Xwayland opens the machine's display device
   even to draw on the CPU, and the software device touches no GPU. It inherits no descriptor of
   Yuna's but those it is given, and Yuna's environment without `DISPLAY`, `WAYLAND_DISPLAY` and
   `XAUTHORITY`; its errors go to Yuna's. Yuna stops watching the sockets, which Xwayland accepts
   on.
2. **Serving.** Once Xwayland writes its number on `-displayfd`, the manager's thread connects
   through `-wm` and takes charge, in this order (`xwayland/manager.rs`): XFixes'
   disconnection mode, without which `-terminate` never fires while the manager is connected;
   `RESOURCE_MANAGER`, `Xft.dpi` at 96 × S; XSETTINGS, `Xft/DPI` with it; Composite's manual
   redirection and SubstructureRedirect on the root; `_NET_SUPPORTED`,
   `_NET_SUPPORTING_WM_CHECK` and the root's empty lists of windows; `WM_S0` last. Xwayland accepts
   its clients only once a client owns `WM_S0`, so the first one finds every setting in place. The
   thread then reads X11's events; what Yuna decides on comes to its loop through a calloop channel
   of 256 events, and Yuna's commands go back, at the end of the round that made them, through a
   channel the thread reads when its wake-up socket says so. Yuna handles the managers' events in a
   slice of each round, one at least: until 1 ms before the earliest deadline armed when the round
   began, a frame most often, and 8 ms at most (`MANAGER_SLICE`). Those past it wait, in order, for
   the next rounds, which come at once, the deadlines due between them run first; meanwhile the
   channels are not read, so that once one is full its manager waits, and X11's own buffers hold
   what comes. A burst of thousands of windows so never holds another window's frames long
   ([measured](measurements.md#m18--a-flood-of-x11-windows-and-menus-2026-11-15)). A start
   that has not reached `WM_S0` within 30 s (`START_TIMEOUT`) is ended.
3. **Paused.** Only after starts that fail in a row: below.

Xwayland ends itself `-terminate`'s delay after its last X11 client leaves, its manager still
connected, with status 0. It may also crash or be killed; Yuna ends it, with SIGKILL through its
pidfd, when its start fails, when its manager fails or its thread panics, when it outlives its
manager's connection by 5 s (`MANAGER_GRACE`), and with its seat. Its pidfd, which the loop
watches, reaps it in both modes and in tests: Yuna handles SIGCHLD only in `yuna mcp`, whose
reaping of every child may take it first, its status then unknown. Its Wayland client goes, its
manager's thread ends, and the display is watched again. A failure is an Xwayland that ends
before its manager owns `WM_S0`, or with another status than 0; in `yuna mcp`, an Xwayland whose
status was taken by the programs' reaping ends unknown, and counts as a failure only before
`WM_S0` ([debt](debt.md#xwayland)). After a clean end or a first failure, the display is watched
again at once. Each further failure within 60 s of the one before (`FAILURE_MEMORY`) pauses it:
1 s, then twice as long each time, up to 30 s (`FIRST_PAUSE`, `LONGEST_PAUSE`). A program that
connects meanwhile waits in the backlog; at the longest pause, the connections waiting are
closed, since no Xwayland could serve them and each would start one again. When the compositor
drops, it ends its Xwaylands and waits until each is reaped, with no time limit, then removes the
displays' files.

At rest, before the first connection and after Xwayland ends, a seat's display holds two
sockets and nothing else: no process, no timer, no wake-up.

### Colours

`colour/` holds `color-management-v1`, Yuna's first handler written on wayland-server rather
than through Smithay ([ADR-0047](adr/0047-color-management-v1-kwins-list.md)). Its global lists
KWin's set at the version each client bound. `description.rs` holds the image descriptions and
their checks, with no Wayland object; `protocol.rs` the dispatch of every object, each keeping
its state in its own user data. A record of a description gets an identity never used again.

- Each output announces one description for its life, made when it is created: the owner's
  monitor's PQ volume for an HDR output, gamma 2.2 and sRGB otherwise
  ([ADR-0048](adr/0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md)). A removed
  output's colour objects fail at once, with `no_output`.
- A surface's description is double-buffered in Smithay's cached state, applied at its commit
  as subsurfaces' state is; `Compositor::window_colours` reads a window's tree.
- A surface's preferred description is its window's workspace's output's, else its seat's
  focused output's. After each arrangement, each feedback object whose answer changed is told
  `preferred_changed`, once.
- The information a client asks of a description is sent after the dispatch, not inside it:
  its last event destroys its object, and wayland-backend 0.3.17 panics when a request's new
  object is destroyed before the request returns.
- Nothing is drawn differently: the colour pipeline is Stage 5's.

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
   whole logical units, or the rectangle its window was drawn at in that frame, moving or not,
   cut to the output. A window
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
   again; a removed output stops its sessions. A client keeps 4 sessions and `wlr-screencopy`
   frames waiting together at most, all clients 16; a session or a frame past them is stopped.
5. `wlr-screencopy-unstable-v1` (`wlr_screencopy.rs`, [ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)),
   which uwui's measures speak, takes the same path under the same rules: every client sees it,
   a frame of another seat's output fails at once. A frame is one copy of the whole output or
   of a region in its logical units, cut to it and read at its physical size, into a `wl_shm`
   buffer; a buffer of another size or format, or a second copy of one frame, is the client's
   protocol error. `copy` takes the newest frame; `copy_with_damage` takes it too when the
   output drew since that manager object's last copy of it, or for its first, and otherwise
   waits for a refresh that draws something; its damage is the whole frame. `ready` gives the
   time of the boundary the frame is presented at. Its frames waiting count among the 4 a client
   keeps and the 16 of all clients, with the sessions of point 4; an area read for one client's
   frame of a refresh, the whole output or a region, is read once for all.

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

- `seat` creates a seat, with an output of the mode it gives, else `--output`'s, a Wayland
  socket, `seat-<id>` in Yuna's folder, and an X display ([XWayland](#xwayland)); lists the
  seats; closes one. A manual-clock Yuna holds one
  seat at a time.
- `outputs` lists, creates and removes a seat's outputs; `workspaces` lists them with their
  windows, creates, removes and shows a named one, and moves a window to one. `seat` and
  `outputs` create an output under VRR with `vrr_min`, over `vrr_min` to its rate, refused
  unless the rate is at least twice it ([VRR outputs](#vrr-outputs)), and an HDR one, which
  announces the colours of the owner's monitor, a PG32UCDM, with `hdr` ([Colours](#colours),
  [ADR-0049](adr/0049-vrr-and-hdr-outputs-through-mcp.md)). Each value a call leaves out is
  `--output`'s, `hdr` included, which `hdr: false` turns off; `--output`'s VRR range only when
  the call leaves out `rate` too, since a range belongs with its rate. An output is told with its VRR range in Hz and its monitor's name,
  each null when it has none.
- `spawn` starts, lists and stops a seat's programs.
- `windows` lists a seat's windows, as `Compositor::windows` describes them, each with its
  `shell`, `xdg` or `x11`: its `rect`, where
  it is drawn at the call's time, read as hit tests read it ([A frame](#a-frame)); its `place`,
  where it rests once its motion ends; and `motion_ends_ns`, when its output's motion ends, null
  at rest, from the refresh that drew its end on. With `colour`,
  each comes with the descriptions its surfaces declared, as `Compositor::window_colours` gives
  them, luminances in cd/m² and chromaticities as numbers; without it the reply leaves them out,
  so that it does not grow for agents that do not ask. With `wait_ms`, it waits up to 50 s, on
  real time whatever the clock's, for one that matches. A waiting call is answered once the
  layout has placed the window, which on the manual clock takes a `clock` advance;
  `notifications/cancelled` drops it unanswered; 64 wait at most.
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
seat's socket's full path; `DISPLAY` and `XAUTHORITY`, its seat's X display and Xauthority
file, when the seat has one, with `DBUS_SESSION_BUS_ADDRESS=disabled:`, since D-Bus's library
would otherwise launch a bus of its own through X11 that holds the seat's Xwayland for ever; an
empty `XDG_RUNTIME_DIR` of its own, where neither the user's session bus nor their sound server
is found; then the agent's variables; then `YUNA_SEAT`, its
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
`XDG_RUNTIME_DIR` for the owner's seat, prints `WAYLAND_DISPLAY=<socket>`, then, when Xwayland
is on `PATH`, `DISPLAY=:<n>` and `XAUTHORITY=<file>`, the owner's seat's X display
([XWayland](#xwayland)), and runs until SIGTERM or SIGINT. With `--agent-seat`, the owner's seat
has no output, socket or `wl_seat`, as in `yuna mcp`, and one agent's seat takes the outputs and
a socket of its own in `XDG_RUNTIME_DIR`, named `wayland-<n>` as the owner's would be, whose
name and X display are printed: what the owner's other projects drive with a virtual keyboard
and pointer ([ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)). `yuna mcp` starts the same compositor
without an output or a socket, on the manual clock with `--manual-clock`, and serves MCP until its
input closes or SIGTERM or SIGINT comes. Both turn XWayland on.
Both run with animations off, each change shown at once; `--animations` turns them on, with
`yuna-motion`'s default springs and curve, and `--reduced-motion` turns them on under reduced
motion, one or the other
([ADR-0055](adr/0055-what-moves-first-and-animations-off-headless.md),
[ADR-0059](adr/0059-reduced-motion-nothing-slides-fades-stay.md)). A flag given twice is a
usage error.
`--output WIDTHxHEIGHT@RATE,SCALE[,vrr=MIN][,hdr]` gives an output's mode, under VRR from MIN to
RATE Hz with `vrr=MIN`, announcing the HDR colours of the owner's monitor, a PG32UCDM, with
`hdr` ([ADR-0049](adr/0049-vrr-and-hdr-outputs-through-mcp.md)); the two items come in any
order, each once. With `mcp`, it is each new seat's output. A mode refused, an item unknown,
repeated or malformed, is a usage error: the program prints why and its help, and exits with 2.
The signal handler writes to a pipe the event loop watches, so that the loop returns and the
socket is removed. At the `debug` level, it logs
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
  removed output, the bound on sessions, a client that leaves with a frame waiting.
  `xwayland.rs` in it runs the shell's Xwayland, an x11rb client on a thread of its own while
  the test runs the compositor's loop: no Xwayland before the first connection, which it serves;
  `Xft.dpi` and XSETTINGS of the largest scale for the first client; a client without the cookie,
  or with another seat's, refused; Xwayland ended when idle, the manager connected, and again
  on the next connection; a crash, and a second one in a row paused; a number whose abstract
  name is taken skipped; no display when XWayland is off or Xwayland is missing; no GPU driver
  on llvmpipe; and Yuna at rest before and after. `x11_windows.rs` in it opens X11 windows,
  with `x11_client.rs`'s x11rb client, which reads `Xft.dpi`, draws with `PutImage` and sets a
  mode through RandR: a window tiled and focused at the tile × S, its own configure answered with
  its place; sharp at 1.5, area-filtered at 1 within a level of `axis_taps`' reference, and moved
  between them at one logical size; typing and clicks on the X11 pixel drawn, at 1.5 and at 1;
  each frame of a slide against `yuna-motion`'s reference, each window configured once, and Yuna
  at rest; `WM_DELETE_WINDOW`, or the client ended; fullscreen asked; hidden and told so where it
  is; a game's emulated mode drawn pixel for pixel; the cap counting X11 clients, not Xwayland;
  two seats' X11 windows apart; a transient dialog floating at its own size over its parent; a
  forged serial pairing nothing; a window withdrawn mapped again without its states; a configure
  asked before the window opens answered with its foretold place; a resize held for its sync
  counter, and never past the timeout; a sync request crossing an unmap never holding the
  window mapped again; an unanswered ping marking a window unresponsive; a pinged window closed,
  or its Xwayland killed, leaving no deadline; a startup id activating with its token.
  `x11_menus.rs` in it shows menus: at their place over their window, pixel for pixel at 1.5 and
  area-filtered at 1; at a constant offset from their window's drawn corner in each frame of a
  slide; clicked on the X11 pixel drawn; a tooltip alone at its position; a menu whose window goes
  drawn alone. `x11_misbehaving.rs` in it runs hostile X11 clients on one seat beside a Wayland
  sentinel on another: a title of 16 MiB, invalid UTF-8 and COMPOUND_TEXT, messages and states
  naming atoms never interned, nonsense size hints, cycles of `WM_TRANSIENT_FOR`, 5,000 windows then
  5,000 override-redirect windows mapped at once, the cap of menus, windows destroyed before
  they pair, a client dying mid-request, menus far off every output, and 24 random sequences of
  up to 32 X11 requests, checked after each; each ends with Yuna sound, the sentinel untouched
  and called back, and Yuna at rest once the hostile clients have gone. The flood checks that
  the sentinel is never starved, and in a release build that no round of Yuna's loop works a
  period. `mcp.rs` in it drives the MCP
  server through two socket pairs, its input and its output: both revisions, every tool but
  `spawn`, two seats that keep to their own, waits, cancellation and timeouts, a client that does
  not read, a line too long, an input that closes. `motion.rs` in it turns animations on: each
  boundary of two retargeted scrolls checked against a `Motion` of the test's own, fed the same
  layout at the same times, until the last frame is the arrangement and Yuna rests; a frame
  drawn at once sampled and shown at the next boundary, or, its drawing across that one, at the
  one after, and no boundary showing one frame twice; half a second or 100 ms after a drawing
  of 20 ms, a frame drawn at once sampling at most one period ahead, and the next frame the
  boundary after; a workspace switch that tells the window left hidden once, after its last
  frame; five moves during a motion, at most one configure for
  each window from each; a pointer and a click mid-scroll reaching the window drawn under them;
  the pointer and `windows` at rest once the last refresh drew the motion's end; a window's
  capture where its frame drew it; the special workspace's fade read in its pixels; animations
  off drawing one frame; `windows` through MCP mid-motion and at rest; and an agent's output
  added, its output removed and its seat removed mid-scroll, which leave the owner's motion
  running.
  Its frame-by-frame stories (`step_frames`) check every refresh's time and frame against the
  reference: a switch turned back mid-way, the special workspace's fade toggled mid-way, reduced
  motion's scroll in one frame and fade over half its time, a window closed and one opened
  mid-scroll; beside them, a popup drawn at its window's drawn rectangle mid-scroll.
  `motion_pacing.rs` in it times drawings with the draw hook, on a scroll under a floating
  window whose frame callbacks and feedback tell each refresh and where its frame was shown:
  drawings of 70 % of a period missing nothing, one of 1.5 periods missing one boundary, every
  drawing 1.2 periods shown every other boundary, each ending on time; drawings of 20, 0, 3, 0
  and 9 ms in turn, each frame sampling a later boundary than the one before; and on a VRR
  output, a scroll refreshing at the shortest refresh, and, after a drawing of 5 ms, the frame
  after one drawn at once sampling a later time, the frames after it on time.
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
  frame callbacks as in `misbehaving.rs`, and Yuna rests once every client has gone. The cases
  run with animations off, as both headless modes default to, and on, at their defaults
  ([ADR-0056](adr/0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md)): then
  every deadline runs until Yuna rests once the sentinel is focused, and again once every client
  has gone. Four tests share the cases, so that they run at once on several cores: 80 cases start
  from a busy state, three clients with a window, a dialog and a menu each, and 80 from the
  sentinel alone, each with animations off and on. `sequences_found_before` replays the sequences
  found failing, written out, both ways, since a seed in `random_requests.proptest-regressions`
  replays its case only while the generator stays the same. Each test prints how often the
  requests that need the keyboard took effect, as the compositor's state and the clients' own
  events show it; M1.4's rates and times are in [`measurements.md`](measurements.md), not
  measured again since the two tests with animations on came.
  `PROPTEST_CASES=<n>` runs `n` cases in each test ([`CONTRIBUTING.md`](../CONTRIBUTING.md)).
- `crates/yuna/tests/command_line.rs`: the program as a user runs it, its socket and its globals
  included, with `--agent-seat` too.
- `crates/yuna/tests/headless/protocols.rs`: the rows `Offered` of [`protocols.md`](protocols.md)
  are the globals a client of the owner's seat and one of an agent's are shown.
- `crates/yuna/tests/contracts.rs`: `yuna --help` and `tools/list` compared with the files of
  `tests/contracts/` ([ADR-0079](adr/0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)).
- `crates/yuna/tests/consumers.rs`: the owner's other projects' own drivers against `yuna
  --headless --agent-seat`, at the revisions `flake.nix` pins
  ([ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)): Pluto's `pluto-input`, built from
  Pluto's lock, clicks and types into a window of the test's, which gets each event; uwui's
  `measure.py --display` moves and clicks with its virtual pointer and counts `vkcube`'s frames
  through `wlr-screencopy`. Its check that no game runs answers none, and `sway` and
  `swaymsg`, which `--display` never runs, are stubs that fail.
- `crates/yuna/tests/mcp.rs`: `yuna mcp` as an MCP client runs it: its programs' environment and
  logs, an orphan adopted, `stop`, the whole tree ended when its input closes, a seat's closing
  that ends its processes and no other seat's, a seat whose programs exhaust Yuna's descriptors,
  and Alacritty started through `spawn`, found through `windows`, clicked and typed into: its
  shell runs the command typed. Applications get the environment of
  `tests/common/application_environment.rs`, which `applications.rs` and `consumers.rs` share,
  with `tests/common/dies_with_test.rs`.
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
  starts dies with the test's thread. The file takes about 45 s, its X11 tests the longest.
  Through `yuna mcp`, as an agent
  drives it, `vkcube` paused by its space key rests with its window open. Two tests run mpv
  there, in Vulkan on lavapipe, on a picture it makes itself: an SDR one on an HDR output and a
  PQ one on an SDR output ([ADR-0050](adr/0050-mpv-in-the-applications-tests.md)). Once mpv
  and Yuna have gone quiet, each reads through `windows` the colours its surface declared, and
  requires values that only the description Yuna prefers for it gives: the HDR output's
  monitor volume as mpv's target, or the SDR output's gamma 2.2 and minimum luminance for a PQ
  picture. mpv then quits by its `q` key, and Yuna must rest. Two tests run X11 programs there,
  which their agent's seat gives `DISPLAY`: Yuna finds Xwayland on the `PATH` the test gives it.
  Qt's `qml` on X11, on a seat at 1.5, draws a scene of one-pixel lines and text at a device
  pixel ratio of 1.5, read from `Xft.dpi` 144: its capture at the physical size must be exactly
  what X11 holds of its window (`GetImage`), and the same scene drawn by Qt on Wayland at 1.5 on
  another seat; moved to an output at 1, it must be within a level of its logical capture at
  1.5, the average of the same pixels
  ([ADR-0068](adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)).
  Wine's notepad, from fresh prefixes at 96 and 144 dots per inch on seats at 1 and 1.5, both
  tiled, must draw its menu bar's height and its scroll bar's width 1.5 times larger at 144,
  within half a pixel, and its capture must be exactly what X11 holds of it; Wine opens no
  network connection, and the folders its wineservers keep in `/tmp/.wine-<uid>` are removed.
  Each rests with its window open, leaves by its keys, and Yuna rests again once Xwayland has
  ended. SlimeEngine's examples, built by the weekly job, run in ignored tests of the same file
  ([ADR-0043](adr/0043-text-input-and-the-rest-of-slimeengines-examples.md)): `window` draws
  120 frames and leaves; `editor` rests with its window open; and M1.5's done-when, an agent
  that opens `interface` through MCP alone, clicks its text field, types `Grüße`, shows text
  being composed and clicks a button; before each step, the part it changes is captured once
  it stands still, and must differ after it.

## The tools around the code

- `flake.nix`: the development shell, and Yuna's package, `bin/yuna` wrapped to draw with the
  package's own Mesa and find Xwayland, which CI builds and starts from an empty environment
  ([ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)); `rust-toolchain.toml`: the
  toolchain ([ADR-0016](adr/0016-the-rust-toolchain.md)).
- `scripts/check.sh`: every automated check, which CI runs (`.github/workflows/ci.yml`).
- `.github/workflows/slimeengine.yml`: SlimeEngine's `window`, `editor` and `interface`
  examples in Yuna, weekly, on demand and on a push that changes anything its test builds or
  reads (`scripts/test_workflows.py`), built at a pinned revision with their data cooked, and
  cached.
- `crates/yuna/examples/frame_time.rs`: a frame's cost while `vkcube` draws, read from outside
  the program: its frames and `took_us` from its log, each thread's CPU time from `/proc`, the
  GPU's from the kernel's DRM usage statistics
  ([ADR-0030](adr/0030-a-frames-gpu-time-from-the-kernel.md)).
- `crates/yuna/examples/agent_time.rs`: what an agent's calls cost in `yuna mcp`: an MCP
  call's time at rest and under load, and a capture's, its copy on the main thread and its
  encoding read from the log, with the frames missed while captures run.
- `crates/yuna/examples/motion_time.rs`: motion's budget
  ([ADR-0056](adr/0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md)): Yuna
  in the example's own process on the real clock, `vkcube` covering the output, a still
  baseline then a scripted round of scrolls, moves, switches and the special workspace's fade;
  frames missed in each, where they fall, the heaviest frame, and the verdict.
  `examples/common/` holds what it shares with `frame_time`.
- `crates/yuna-motion/examples/motion_timing.rs`: what motion costs at an operation and at a frame:
  `Motion::update` after a focus move and after a column opened left of the focus, and
  `Motion::frame` while a motion runs, on strips of 100 and 1,000 windows.
- `crates/yuna-bench`: the rivals' bench, [below](#the-rivals-bench); `flake.nix`'s `bench`
  shell holds what it runs.
- `deny.toml`: the dependency policy.

## The rivals' bench

`crates/yuna-bench` measures compositors from outside, the same way for each
([ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)).
Its crate documentation gives each part's contract; how to run it is in
[`CONTRIBUTING.md`](../CONTRIBUTING.md). A case is one compositor, one configuration, one
scenario; each run of a case goes:

1. **The sandbox.** `systemd-run --user --scope` makes a transient scope, a control group of its
   own; `env -i` clears the environment and sets store paths and the run's own folders alone;
   `dbus-run-session` gives a private session bus; the bench's `enter-core` moves the compositor
   into a child group, `core`, then becomes it. The scope is the *stack*, `core` the *core*.
   Each client runs in a scope of its own. No descriptor of the bench but its standard input,
   output and error reaches them: it marks every other it inherited close-on-exec at its start.
   A compositor that runs inside another, its *parent* (Hyprland), is started by the bench's
   `parent` instead: it starts the parent in the stack, waits for its Wayland socket, starts the
   compositor into `core`, connected to it, and stops the parent once the compositor has ended.
2. **The recipe** (`recipes.rs`): the compositor's program, variables and configuration file,
   written into the case's folder, as shipped or at equal work. Once the compositor is ready, the
   recipe's setup runs (dwl's output mode, Hyprland's output), then the bench reads back the
   output it made, which the run's file records. KWin's 240 Hz comes from its own output
   configuration, which a first KWin writes and the bench edits.
3. **Ready, then checked.** The Wayland socket and the recipe's own sign. Then, and again in
   every scenario once its clients are up, the compositor's scope and each client's are checked:
   the case is *refused*, and stopped, if a process holds `/dev/dri/card*` or an input device
   (`/dev/input/*`, `/dev/uinput`, `/dev/hidraw*`), reaches a socket in the owner's runtime
   folder, is connected to an X11 display served outside the sandbox, or names the owner's
   runtime folder in its environment. Each window's readers check the same nodes again every
   250 ms, and one found then refuses the case too. These checks detect; they do not prevent
   (the known limits below).
4. **The windows** (`scenarios.rs`). In each measured window, the control groups' `cpu.stat` at
   its ends give exact CPU times; a reader every 250 ms keeps each thread's time and context
   switches and each DRM client's engine times, so that those that end inside count up to their
   last reading (`sampler.rs`). Wake-ups are a lower bound when the threads read account for less
   than 90 % of the group's CPU time, or when that share is unknown: processes then lived and
   ended between readings. A group whose `cpu.stat` counted no CPU time ran no thread, so woke
   exactly 0 times. Another reader takes the machine's state each second (`machine.rs`). Memory
   is read once the compositor is ready and after the cost window: each process's
   `smaps_rollup`, and the DRM `fdinfo` of its GPU files. The run notes the core's threads under
   a realtime policy.
5. **The frames.** A cost a frame is given only from an exact count of the window's own frames:
   KWin's frame log, which it writes in the cost window; Umbriel's count of its output's
   commits, read at the cost window's ends; or, in `game`, mpv's frames shown, read over its
   IPC, mpv being paced one frame for one of the compositor's. The others but Mutter are counted
   by `ext-image-copy-capture` in a *frames window* apart, the scenario run again with the frame
   counter, whose copies would weigh on the costs. That count is a lower bound: the counter asks
   each frame after the last copy, so at a game's rate it misses frames. Mutter offers no capture
   protocol: its frames are counted only by mpv in `game`. Yuna's cost window runs at its default
   log level: its `frame drawn` lines would weigh on its costs. Each cost window records its
   *achieved rate*, the frames a second the compositor really drew (`achieved_rate`), which the
   run's file keeps; no ranking depends on it.
6. **The stop**, as the recipe says, then `cgroup.kill` for what is left; KWin's frame log is
   read once it has flushed it.

`game` is the same scene everywhere: mpv, which asks fullscreen itself, plays a clip at 240
frames a second as fast as the compositor takes them, drawing it with Vulkan on radv over the
whole output. The bench refuses the case unless mpv reports itself fullscreen at the output's
logical size, and fails it unless mpv's clip played during its warm-up. Its clips, `film`'s too,
are long enough never to loop inside a window: mpv is not sought back to its start, which is one
more chance for it to stall on Mutter (below).

Each recipe, all on the render node at 3840 × 2160, 240 Hz, scale 1.5:

| Recipe | Backend, its pace | Its output's mode | `motion` | Frames | Equal work turns off |
|---|---|---|---|---|---|
| sway, sway-vulkan | wlroots headless, GLES2 or Vulkan; wlroots' timer | its configuration's `output` | `swaymsg` | capture | its bar, swaybg, XWayland |
| KWin | its virtual backend; the mode's rate | its output configuration | its D-Bus | its frame log | its default effects, XWayland |
| Yuna | its headless backend, `--gpu`; the mode's rate | `--output` | none | capture | nothing: both are the same |
| dwl | wlroots headless; wlroots' timer | wlr-randr, once ready | the bench's virtual keyboard | capture | XWayland: a build without it |
| mango | wlroots headless, SceneFX; wlroots' timer | a `monitor_rule` | `mmsg` | capture | animations, rounded corners; XWayland: a build without it |
| Umbriel | wlroots headless, its SceneFX fork; wlroots' timer | its configuration's output | `umbriel msg` | its commits | animations, blur, shadows, corners, its cheatsheet, XWayland |
| Hyprland | Aquamarine's headless, inside a dwl; the mode's rate | `hyprctl output create headless`, a monitor rule | `hyprctl` | capture | animations, blur, shadows, corners, its wallpaper, XWayland, realtime scheduling |
| Mutter | its headless backend; the mode's rate | `--virtual-monitor`; scale from its `monitors.xml` | none | mpv in `game` alone | animations, XWayland |

niri and cosmic-comp have no recipe that runs: niri's headless backend serves its own tests
only, and cosmic-comp has none. They are measured on the owner's tour alone, and the summary
says so. The recipe of each gives its reasons, with its sources' lines.

The run file holds every figure with its unit, the raw `fdinfo`, the machine's state each second,
how each window's frames were counted, which figures are lower bounds, the lock the run held,
and each still window's configured size and buffer. The bench writes it before its first case
and again after each, through a temporary file flushed to the disk and a rename, with the time
of the writing: a run cut short keeps every case it ran, whole.

`yuna-bench report` turns run files into tables. It ranks only the runs of one day and one
machine state ([ADR-0072](adr/0072-budgets-set-against-the-peers-the-same-day.md)): the UTC day
their run file started, one machine whole (processor, memory, GPU, VBIOS, firmware, kernel and
Mesa) and one GPU performance level, those of the first file named. Other runs are shown on rows
of their own, marked ‡, unranked, with why. It ranks a figure only where the runs' spreads do
not overlap, among *equal work* runs that other programs did not disturb, and names a compositor
all of whose runs were disturbed; it never ranks a lower bound. In `game` and `pacing`, whose
client draws as fast as the compositor takes its frames, it ranks a cost a second only between
compositors on the same pace, the recipe's: wlroots' headless timer, or the mode's rate. In
`rest`, `motion` and `film`, where equal work draws only when something changes, it ranks them
across every compositor. A cost a frame does not scale with the pace: it ranks across backends,
between compositors whose frames were counted the same exact way, by mpv in `game` or by one
compositor's own count; frame counts never rank. A figure of the stack ranks apart for a
compositor whose stack holds its parent. Its memory at rest gives each compositor's core after
`rest`'s cost window, at each count of still windows the files ran (1, 15 and 150 by default),
and its growth a window from the fewest to the most: its own memory (`Pss_Anon` + `Pss_File`) and its GPU memory (`resident-vram` + `resident-gtt`) rank, and
the clients' buffers it holds (`Pss_Shmem`) are shown, not ranked, since each layout gives the
still windows sizes of its own. A run file written before the bench gave a group that ran
nothing exactly 0 wake-ups is read under that rule. It ends with every rival of
[ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md),
measured or not, and those on the tour only.

Known limits:

- The sandbox detects; it does not prevent. No namespace keeps a rival from the owner's display,
  devices, runtime folder or session bus: the rivals run as the owner's user, kept off them by
  their configuration alone, a cleared environment and a runtime folder and bus of their own. A
  rival that reached one would be seen at the next check, once ready, once its scenario's clients
  are up, or by a window's readers every 250 ms for the device nodes, and its case refused; what
  it did between two checks is not undone. To see which socket a socket is connected to, the
  checks read the kernel's socket diagnostics (`sock_diag`). Without them, they see the sockets
  bound in the owner's runtime folder, not those connected to one, and each case's run says so.
- Frames counted by capture are lower bounds, and each copy of a 4K shm buffer costs the
  compositor milliseconds.
- wlroots' headless backend sends a frame event 4 ms after each commit: it counts whole
  milliseconds, `1000000 / 240000` = 4 (`backend/headless/output.c:31` and `:76`). sway, dwl,
  mango and Umbriel draw up to 250 frames a second for a 240 Hz mode, fewer as each frame takes
  time to draw; the summary gives the rates their runs reached. KWin, Hyprland, Mutter and Yuna
  draw at the mode's rate, 240 at most. Where the client paces the compositor, their costs a
  second rank apart.
- Yuna and Mutter have no actions for `motion`. Mutter takes no command for its windows from
  outside: GNOME Shell drives them from inside Mutter's process, and is not measured.
- Hyprland's parent is dwl, not sway: Aquamarine binds `xdg_wm_base` at version 6, which sway
  1.12 offers at 5. Hyprland is started through its watchdog, `start-hyprland`, as its session
  file does; started alone it shows a warning for 15 s. A Hyprland that `start-hyprland`
  restarts, in its safe mode, fails the case. Hyprland 0.57.1 puts its main thread on `SCHED_RR`,
  from rtkit: kept as shipped; at equal work its system bus is a socket that does not exist, so
  it runs without, as 0.57.2 ships. The run notes the core's realtime threads. Its stack holds its
  parent, which the summary says, and its figures of the stack rank apart.
- dwl and mango have no switch for XWayland at run time: equal work runs nixpkgs' build of each
  without it, the same code but XWayland's.
- mpv, with `--untimed` and a FIFO swap chain, can stay at its first frames on Mutter, with or
  without a seek. The bench then fails the case ("mpv's clip did not play in its warm-up")
  rather than measure an empty scene; a run again may play.
- Mutter's default plugin draws a plain colour with a vignette behind the windows, which no switch
  removes; Umbriel as shipped shows its keybind cheatsheet throughout.
- Umbriel's commits are read inside the cost window, at its ends: two answers of its IPC count in
  its costs.

## Where to start reading

1. [`../crates/yuna-layout/src/lib.rs`](../crates/yuna-layout/src/lib.rs), then
   [`layout.rs`](../crates/yuna-layout/src/layout.rs) for the operations.
2. A few scenarios, for example
   [`scrolling.rs`](../crates/yuna-layout/tests/scenarios/scrolling.rs).
3. [`../crates/yuna-motion/src/lib.rs`](../crates/yuna-motion/src/lib.rs), then
   [`arrangement.rs`](../crates/yuna-motion/src/arrangement.rs).
4. [`../crates/yuna/src/lib.rs`](../crates/yuna/src/lib.rs), then
   [`frames.rs`](../crates/yuna/src/frames.rs) and
   [`shell.rs`](../crates/yuna/src/shell.rs).
5. A story of [`../crates/yuna/tests/headless/main.rs`](../crates/yuna/tests/headless/main.rs).
6. [`../scripts/check.sh`](../scripts/check.sh).
