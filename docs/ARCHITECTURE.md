# Architecture

How Yuna's code fits together, for a developer who has never seen it. This page gives the parts,
the flows across them, who owns what, and what fails how. Each part's contracts, figures and
limits are in its code's documentation, linked from here; why each decision was taken is in the
[ADRs](adr/README.md); what comes next is in the [PLAN](PLAN.md); what falls short is in the
[debt](debt.md). Terms in italics are defined in the [glossary](glossary.md).

## In one page

**The parts, bottom up:**

- **The pure models.** [`yuna-layout`](#the-layout-model) decides where every window goes.
  [`yuna-motion`](#motion) turns the layout's jumps into what each frame draws. Neither knows
  Wayland, Smithay, a renderer or a clock.
- **The headless compositor**, the `yuna` crate's library, on Smithay. In front: the Wayland
  protocols, and for each seat [XWayland](#xwayland) with Yuna's own X11 window manager. Behind:
  the layout and the motion. Underneath: virtual [outputs](#outputs), refreshed by one
  [clock](#time-and-deadlines) and drawn by Smithay's GLES renderer, on Mesa's llvmpipe or, with
  `--gpu`, on the GPU's render node.
- **[The MCP server](#the-mcp-server)** around the compositor, for agents: seats, programs,
  input, captures, events, the clipboard and each seat's [accessibility bus](#accessibility).
  **[The program](#the-program)**, `yuna`, runs `yuna --headless` or `yuna mcp`.
- **[The rivals' bench](#the-rivals-bench)**, `yuna-bench`, apart: it measures every compositor,
  Yuna included, from outside.

**The main flows:**

- **[A round of the event loop](#a-round-of-the-event-loop)** wakes on a client, a timer or a
  pipe, handles what came, runs the deadlines due, frees what went away, and sends the events
  written.
- **A client's commit** arms its output's next refresh. The [frame](#a-frame) is drawn ahead of
  the boundary it is shown at, only if something is damaged; frame callbacks and presentation
  feedback follow.
- **A change of the layout** becomes motion, sampled at the time each frame is shown. What is
  drawn follows the motion; what a client is told follows its place.
- **An agent's call** over MCP creates a seat, starts programs, types, clicks, captures and reads
  events, on the agent's own seat only.
- **An X11 program** connects to its seat's display, which starts Xwayland on demand. Yuna's X11
  window manager, on a thread of its own, makes windows of the layout of its windows
  ([An X11 window's life](#an-x11-windows-life)).

**Who owns which state:**

- [`Compositor`](../crates/yuna/src/compositor.rs) owns the event loop, the Wayland display and
  `State`. `State` holds the rest: the layout, the motion, the clock, the outputs, the seats, the
  record of windows, the renderer, the selections and the rest auditor.
- The layout alone decides where each window goes and which window has each seat's focus. The
  shell's record, `Toplevel`, ties each `WindowId` to its surface.
- The clock owns every deadline of the frame loop.
- Each seat owns its branch of the layout, its focus, its selections, its outputs, its socket and
  its X display. Nothing one seat does reaches another's focus or input.
- The MCP `Server` owns the `Compositor`, the agents' programs, their logs and folder, each
  seat's log of events and each seat's accessibility bus.
- Each X11 window manager owns its X11 connection and its view of X11's windows. It shares
  nothing with `State`: it talks to Yuna's loop through two channels.

**What runs on which thread:**

- The *state thread* runs the event loop and everything `State` holds: the protocols, the layout,
  the motion, the frames, the drawing, and in `yuna mcp` the server. While Yuna runs, it never
  waits on a client, a program's pipe or an X server: their sources note what came, and the loop
  reads it when it is ready. In tests, the state thread is the test's own.
- One thread for each running Xwayland, its X11 window manager (`yuna-x11-<seat>`): it waits on
  X11, so that the loop never does.
- In `yuna mcp`, up to four encoder threads (`yuna-capture`) make the captures' PNGs.

**What survives which failure** ([ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)):

- A client's protocol error, or a request past a bound, ends or refuses that client alone.
- A panic of the state thread is fatal: Yuna stops at once ([The program](#the-program)).
- A panic of an encoder fails its capture alone. A panic or failure of an X11 window manager ends
  its Xwayland. An Xwayland that ends takes its seat's X11 programs with it; the next X11
  connection starts another.
- A seat's socket that cannot accept a connection pauses, and a seat whose X display cannot be
  made goes without X11: neither stops Yuna.

## Crates and dependencies

One Cargo workspace; its crates live in `crates/`.

| Crate | What it is | Depends on |
|---|---|---|
| `yuna-layout` | the layout model | nothing |
| `yuna-motion` | the motion of the layout's arrangements | `yuna-layout` |
| `yuna` | the headless compositor as a library, its MCP server, and the `yuna` program (`src/main.rs`) | Smithay, `yuna-layout`, `yuna-motion`, and the crates below |
| `yuna-bench` | the rivals' bench | nothing of Yuna's |

- Each dependency's reason is a comment beside it, in the root [`Cargo.toml`](../Cargo.toml)'s
  `[workspace.dependencies]` and in each crate's manifest. `deny.toml` holds the policy on
  licences and advisories; [CONTRIBUTING](../CONTRIBUTING.md#dependencies) says how a new
  dependency is accepted.
- `yuna` uses Yuna's fork of Smithay at a pinned revision
  ([ADR-0011](adr/0011-built-on-smithay.md), [ADR-0042](adr/0042-main-on-smithays-fork.md), its
  [patches](smithay-patches.md)), with only its `wayland_frontend`, `desktop` and `renderer_gl`
  features, and rustix through Smithay's re-export. Beside them: x11rb for the X11 window
  manager, zbus's wire format for D-Bus, `serde` and `serde_json` for MCP, `png` for captures,
  `tracing` for the logs. Its tests and measuring examples add proptest, the Wayland client
  crates, and `yuna-bench`'s readers of `/proc` and the DRM statistics.
- At run time Yuna needs libxkbcommon, linked; libglvnd's EGL and Mesa, loaded; Xwayland, found
  on `PATH`; and `dbus-daemon` for the accessibility buses, named by `YUNA_DBUS_DAEMON`, else
  found on `PATH`. The development shell and Yuna's package give all four (`flake.nix`).
- The rules: `yuna-layout` and `yuna-motion` hold no Wayland or Smithay object; `yuna-bench`
  depends on nothing of `yuna`, so that it measures Yuna as it measures the others; `yuna` uses
  `yuna-bench` only in its tests and examples.

## The layout model

`yuna-layout` decides where every window is: on which seat, output and workspace, at which
rectangle, and which window each seat focuses ([ADR-0005](adr/0005-the-layouts.md),
[ADR-0020](adr/0020-the-layout-models-shape.md)). Its crate documentation,
[`lib.rs`](../crates/yuna-layout/src/lib.rs), explains the tree and the operations; `sizing.rs`,
`scrolling.rs`, `dwindle.rs`, `floating.rs` and `geometry.rs` hold their own rules.

- **Its boundary.** Plain ids in, rectangles out. The caller names seats, outputs and windows;
  the model names the workspaces and columns it creates, never giving an id twice. It holds no
  Wayland or Smithay object and reads no clock, so that every behaviour is a test without a
  compositor.
- **Who owns what.** A `Layout` holds seats; a seat holds its outputs, its workspaces left
  without an output, and its hidden special workspaces; an output holds its workspaces, shows one,
  and may show a special workspace over it. An operation reaches only its seat's branch, so an
  agent's seat never touches the owner's ([ADR-0015](adr/0015-agents-workspaces.md)).
- **An operation** names a seat, for a key binding, which acts on what that seat focuses; or a
  window, for its client's request. The model finds them, applies the change to the workspace's
  tiling, scrolling columns or dwindle, or to its floating windows, then tidies the tree.
- **Reading it.** The compositor reads `Layout::arrange`: each window an output shows, bottom to
  top, with the size it is asked, its rectangle on the output's pixel grid, its states, and its
  workspace, layer and column, which motion follows. Nothing in the model runs between
  operations.
- **Clients' sizes.** A window has the size it is *asked*, for its configure, and the size it is
  *drawn* at, what its client committed. The compositor passes on each client's limits and
  committed geometry ([ADR-0026](adr/0026-real-clients-in-the-layout.md)). A dialog opens
  floating, centred on its parent.
- **Its invariants.** `Layout::check_invariants` checks every rule the model keeps; the tests
  call it after every operation.
- **How it fails.** Every operation takes ids that may be stale and answers with an error, never
  a panic; a refused operation changes nothing.

## Motion

`yuna-motion` turns the layout's jumps into what each frame draws
([ADR-0052](adr/0052-motion-in-a-pure-crate.md),
[ADR-0054](adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)). Its crate
documentation, [`lib.rs`](../crates/yuna-motion/src/lib.rs), explains its springs, its curves
and the motion of an arrangement.

- **Its boundary.** It reads the layout and a time it is given. It holds no Wayland or Smithay
  object and reads no clock.
- **The flow.** After a change of the layout, the compositor calls `Motion::update` at the
  refresh's time. At each refresh, `Motion::frame` gives each window's rectangle and alpha at the
  time the frame is shown; `Motion::end` says when the output's motion ends; `told_visible`,
  which windows are told they are shown.
- **Its invariants.** Springs and curves are computed in closed form from their start alone,
  exact at their end ([ADR-0053](adr/0053-springs-in-closed-form-keeping-their-velocity.md)): a
  motion rests exactly at its arrangement, and then nothing runs. A spring retargeted keeps its
  velocity.
- **Its settings.** Both headless modes run with animations off, each change instant;
  `--animations` and `--reduced-motion` turn them on
  ([ADR-0055](adr/0055-what-moves-first-and-animations-off-headless.md),
  [ADR-0059](adr/0059-reduced-motion-nothing-slides-fades-stay.md)).

## The headless compositor

### Modules

`crates/yuna/src`, one job per module. Each module's documentation gives its contracts.

| Module | Its job |
|---|---|
| [`compositor.rs`](../crates/yuna/src/compositor.rs) | `Compositor`, the public face: the event loop, the display and `State`; a round; seats and outputs added and removed |
| [`state.rs`](../crates/yuna/src/state.rs) | `State`, and the handlers that are not the shell's: surfaces, shared memory, outputs, presentation time, cursors |
| [`clock.rs`](../crates/yuna/src/clock.rs) | the clock and its deadlines |
| [`output.rs`](../crates/yuna/src/output.rs) | virtual outputs: their mode, buffer, damage, refresh boundaries and removal |
| [`vrr.rs`](../crates/yuna/src/vrr.rs) | the VRR panel an output may emulate |
| [`frames.rs`](../crates/yuna/src/frames.rs) | the frame loop |
| [`render.rs`](../crates/yuna/src/render.rs), `render/` | drawing: the one GLES renderer, and Yuna's element for X11 surfaces (`render/x11_surface.rs`) |
| [`scale.rs`](../crates/yuna/src/scale.rs) | the scale each surface is told |
| [`dmabuf.rs`](../crates/yuna/src/dmabuf.rs) | `linux-dmabuf`: clients' dmabufs, checked and imported |
| [`shell.rs`](../crates/yuna/src/shell.rs) | `xdg-shell` and `xdg-decoration` in front of the layout; `Toplevel`, the record both shells' windows share |
| [`popups.rs`](../crates/yuna/src/popups.rs) | `xdg_popup`: placement, grabs, dismissal |
| [`activation.rs`](../crates/yuna/src/activation.rs) | `xdg-activation-v1`: tokens, activation, urgent windows |
| [`dialog.rs`](../crates/yuna/src/dialog.rs) | `xdg-dialog-v1`: the modal hint, read and not used yet |
| [`seats.rs`](../crates/yuna/src/seats.rs) | the seats, the owner's and each agent's |
| [`input.rs`](../crates/yuna/src/input.rs) | pointer and keyboard input on an agent's seat, and drag and drop |
| [`keymap.rs`](../crates/yuna/src/keymap.rs) | an agent's seat's keymap, and which keys type each keysym |
| [`text_input.rs`](../crates/yuna/src/text_input.rs) | `text-input-v3`, Yuna as the input method of agents' seats |
| [`virtual_input.rs`](../crates/yuna/src/virtual_input.rs) | the virtual keyboard and pointer of agents' seats' clients |
| [`selection.rs`](../crates/yuna/src/selection.rs) | the clipboard, the primary selection and data control; the selections Yuna holds itself |
| [`capture.rs`](../crates/yuna/src/capture.rs) | captures: which frame, which pixels, the image's mapping |
| [`screencopy.rs`](../crates/yuna/src/screencopy.rs) | `ext-image-copy-capture-v1`, on captures |
| [`wlr_screencopy.rs`](../crates/yuna/src/wlr_screencopy.rs) | `wlr-screencopy-unstable-v1`, on captures |
| [`colour/`](../crates/yuna/src/colour/mod.rs) | `color-management-v1`: what outputs announce and surfaces declare |
| [`rest.rs`](../crates/yuna/src/rest.rs) | the rest auditor, through which every event source is inserted |
| [`process.rs`](../crates/yuna/src/process.rs) | what every program Yuna starts shares, and what Yuna reads of a process in `/proc` |
| [`xwayland/`](../crates/yuna/src/xwayland/mod.rs) | each seat's X display, its Xwayland and its X11 window manager |
| [`dbus/`](../crates/yuna/src/dbus/mod.rs) | Yuna's own D-Bus client, for the accessibility buses |
| [`mcp/`](../crates/yuna/src/mcp/mod.rs) | the MCP server |
| [`main.rs`](../crates/yuna/src/main.rs), `fatal.rs`, `stop_signal.rs` | the program alone: its command line, its logs, its signals, its fatal path |

### A round of the event loop

`Compositor::dispatch` runs one round. `Compositor::run_until` repeats rounds for the program;
tests call `dispatch` themselves, and `Compositor::advance` on the manual clock.

1. On the real clock, one calloop timer is armed at the clock's earliest deadline. The loop then
   waits for a source: a client, a new connection, that timer, an X11 window manager's events, or
   in `yuna mcp` the server's pipes and timers. Every source is inserted through the
   [rest auditor](#the-rest-auditor), which counts what woke Yuna.
2. The X11 window managers' events are handled, in a slice of the round (`MANAGER_SLICE`).
3. The clients' requests are read: Smithay dispatches them to Yuna's handlers.
4. The clock's deadlines due by now run.
5. The round ends (`Compositor::flush`): the clients the display ended are freed, Yuna's writes
   of agents' clipboards go on, the X11 window managers get their commands, what closed windows
   and destroyed surfaces left is freed, and the events written are sent.

In `yuna mcp`, the server then serves what came (`Server::dispatch`).

### Time and deadlines

The clock ([`clock.rs`](../crates/yuna/src/clock.rs),
[ADR-0023](adr/0023-frames-presentation-and-the-clock.md)) owns every deadline of the frame loop.
Its `Deadline` names each kind, each armed once at most:

- two per output: its next refresh, and the boundary its held presentation feedback waits for;
- the hidden windows' frame callbacks;
- the X11 windows' sync requests and pings (`xwayland/answers.rs`);
- the drags between X11 and Wayland (`xwayland/drags.rs`).

In real mode, time is `CLOCK_MONOTONIC`; with no deadline armed, no timer is. In manual mode, for
tests, time starts at the real monotonic time and moves only through `Compositor::advance`, which
runs each deadline it passes at its own time, in order. A *draw hook*
(`Compositor::set_draw_hook`) may move manual time while a frame draws, to make a drawing late.

Every other timer is an event source of its own, inserted through the rest auditor: MCP's waiting
calls and captures, the programs' pace, a seat's closing, Xwayland's start and pauses, a seat
socket's pause, an agent's clipboard reads and writes (`rest::Source`), and the rest log's own
second.

An output's *refresh boundaries* come at its rate from its creation, each computed from the rate,
so that errors never add up (`Boundaries`, in `output.rs`). An output under VRR asks its panel
instead (`vrr.rs`). Both answer the frame loop through `Timing`.

### A frame

The frame loop ([`frames.rs`](../crates/yuna/src/frames.rs),
[ADR-0031](adr/0031-frames-drawn-ahead-of-their-boundary.md),
[ADR-0054](adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)) draws an output
only at a refresh something armed, and only if something on it is damaged. Its module
documentation gives each step; in short:

- **What arms a refresh**: a commit of a surface shown on the output; a change of the layout; a
  capture; the output's creation or change of mode; a popup or an X11 menu that came, went or
  moved; and, while the output's motion runs, each refresh. At rest, nothing is armed.
- **When it is due**: at once when the output is idle; otherwise the output's *lead* before the
  boundary it can reach. The lead is the longest drawing of the last half second, with a floor
  and a margin, at most one period (`DRAWING_WINDOW`, `DRAWING_FLOOR`, `LEAD_MARGIN`).
- **At the refresh**:
  1. If the layout changed, the motion takes it and every output is arranged again. Windows are
     told their sizes and states. Each seat's keyboard focus changes window only here.
  2. The motion gives the frame at the time it is shown. The output is drawn if anything on it is
     damaged, and keeps what it drew, which captures cut.
  3. Frame callbacks go out. Presentation feedback is held until the boundary the frame is shown
     at, through the output's presentation deadline.
  4. While the motion runs, the next refresh is armed.
- **Drawn, and told.** What is drawn follows the motion; what a client is told, its configure,
  its visibility and its outputs, follows its place. So a window gets at most one such change per
  operation, never one a frame. Hit tests, the pointer's focus and MCP's `windows` read the
  motion now, or at the time the output's last frame sampled if that is later: never past what
  was drawn.
- **Hidden windows**, on a workspace not shown or scrolled out of view, are told `suspended` and
  called back once a second (`HIDDEN_FRAME_INTERVAL`). A window the layout refused is treated the
  same way, so that no client waits for ever.
- **How it fails.** A drawing the renderer fails is logged, once every
  `FAILED_DRAWINGS_LOG_PERIOD` at most; its presentation feedback is discarded and the captures
  waiting on it fail. A client's buffer that fails to draw is left out of the frame by Smithay's
  renderer, which logs it.

### A window's life

An `xdg_toplevel` ([`shell.rs`](../crates/yuna/src/shell.rs),
[ADR-0026](adr/0026-real-clients-in-the-layout.md)):

1. Its initial commit gets a first configure: the size `Layout::size_on_open` foretells, tiled on
   all four edges, decorations on the server's side ([ADR-0024](adr/0024-no-title-bars.md)).
2. Its first buffer opens it in the layout, with its parent, its limits and its window geometry.
   A window past a cap (`MAX_WINDOWS_PER_CLIENT`, `MAX_WINDOWS`), or that the layout refuses, is
   not shown, and gets frame callbacks at the hidden windows' rate
   ([ADR-0028](adr/0028-bounds-on-what-one-client-may-ask.md)).
3. Each later commit passes on its limits and geometry when they change, and arms its output's
   refresh when the window is shown.
4. Its configures follow the layout's arrangements, at refreshes. Fullscreen and maximise
   requests always get one, as xdg-shell asks.
5. A commit without a buffer, or the destruction of the toplevel or of its surface, closes it. A
   client that leaves, or that the display ends, has its toplevels destroyed by the round's end.

`Compositor::close_window` asks a window's client to close it: `xdg_toplevel.close`, or for an
X11 window `WM_DELETE_WINDOW`, else its X11 client is ended.

### Popups

Popups ([`popups.rs`](../crates/yuna/src/popups.rs)) are not windows of the layout: Smithay's
`PopupManager` keeps each window's popups in a tree on its toplevel's surface. A popup is placed
by its positioner within the output its window is shown on, drawn just above its window, called
back with it, and dismissed with it. An explicit grab is granted only on the popup's client's own
seat, with a serial that seat sent to the window; each seat has its own grab. Between refreshes,
the keyboard moves only within the window it is on. A popup past a bound (`MAX_POPUPS_PER_WINDOW`,
`MAX_POPUP_DEPTH`), or with a nonsense positioner, is dismissed at once.

### Activation and dialogs

[`activation.rs`](../crates/yuna/src/activation.rs) serves `xdg-activation-v1`. A token is valid
only when asked with its client's own seat and a serial that seat sent there; it lasts
`TOKEN_LIFETIME` and is used once. With a valid token, the layout decides
(`Layout::request_activation`): the window takes the focus at the next refresh, unless a
fullscreen window holds the screen. Otherwise the window is only marked urgent, so that no
program takes a game's screen. An X11 window's `_NET_STARTUP_ID` may name a token
([An X11 window's life](#an-x11-windows-life)).

[`dialog.rs`](../crates/yuna/src/dialog.rs) offers `xdg-dialog-v1`. Dialogs open on their
parent through `xdg_toplevel.set_parent`; the modal hint is read, and nothing uses it yet.

### Outputs

Virtual outputs ([`output.rs`](../crates/yuna/src/output.rs),
[ADR-0022](adr/0022-the-headless-backend-in-yuna.md)) have any size, rate and scale, and no
monitor. Each belongs to one seat, which keeps one at least while it lives. A compositor keeps
`MAX_OUTPUTS` at most.

- **Added** (`Compositor::add_output`): to the right of every output. Its first frame is armed,
  and its global announced at the round's end.
- **Changed** (`Compositor::configure_output`, MCP's `outputs configure`): applied whole, or
  refused with nothing changed, as a monitor set to another mode
  ([ADR-0090](adr/0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md),
  [ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
  item 7). `State::configure_output` lists what follows: the layout's places, the buffer, the
  refresh timing, the colours, what clients and surfaces are told, the captures.
- **Removed** (`Compositor::remove_output`): its workspaces go to another output of its seat. Its
  surfaces leave it, its held feedback is discarded, its deadlines are disarmed, and its captures
  fail (`State::drop_output`). Its global is disabled at once and destroyed
  `RETIRED_GLOBAL_LIFETIME` later, so that a client that bound it late is not ended for it. A
  closed seat's `wl_seat` goes the same way.
- **Under VRR**, an output emulates amdgpu driving the owner's DisplayPort Adaptive-Sync monitor
  ([ADR-0046](adr/0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md),
  [ADR-0051](adr/0051-emulated-vrr-as-the-monitors-traces-showed.md)). Its panel
  ([`vrr.rs`](../crates/yuna/src/vrr.rs)) computes when a frame ready at a given time would show,
  when asked, never on a timer. A test in `vrr.rs` replays the monitor's traces
  (`crates/yuna/tests/fixtures/`); what the emulation does not reproduce is in the
  [debt](debt.md#vrr).
- **HDR**: an HDR output announces the owner's monitor's colours ([Colours](#colours)).

### Scale

Each surface is told the scale of the output its window is shown on: `wp_fractional_scale_v1`'s
preferred scale, and `wl_surface.preferred_buffer_scale`
([`scale.rs`](../crates/yuna/src/scale.rs)). A client that follows the fractional scale draws
its buffer at its window's physical size and sets its viewport to its logical size. Drawn at a
rectangle the layout put on the physical grid, its window is then exact: one buffer pixel to one
physical pixel. A window that moves to another output is told the new scale before the configure
that gives its size there, so that the buffer that answers it has that scale. A client that draws
at an integer buffer scale is scaled when drawn ([debt](debt.md)).

X11 windows are drawn by an element of Yuna's own
([`render/x11_surface.rs`](../crates/yuna/src/render/x11_surface.rs),
[ADR-0068](adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)). Xwayland draws every
window of its seat at one scale, *XWayland's scale* S: the largest of the seat's outputs when it
started, fixed for its life. On an output of scale s, an X11 pixel covers s / S physical pixels:
copied texel for texel at s = S, averaged by the *area filter* below S, scaled up bilinearly
above S. The pointer on an X11 window is measured from where this element draws it.

### Buffers

Clients' buffers come through `wl_shm` or `linux-dmabuf`
([`dmabuf.rs`](../crates/yuna/src/dmabuf.rs), [ADR-0022](adr/0022-the-headless-backend-in-yuna.md),
[ADR-0098](adr/0098-on-llvmpipe-dmabufs-offered-for-no-device-and-checked-as-llvmpipe-maps-them.md)).
`linux-dmabuf` is offered only when the renderer imports a format, with feedback that names one
main device: the render node with `--gpu`; on llvmpipe, no device (`dev_t` 0), since llvmpipe
reads a buffer with the processor, through a mapping of its file. On llvmpipe, then:

- Mesa's EGL and Vulkan on the processor, llvmpipe and lavapipe, draw through `wl_shm`
  ([ADR-0029](adr/0029-the-rest-of-animated-applications.md)).
- Mesa's Vulkan on a GPU sees another device than its own. It draws in its video memory, then
  copies each frame into a linear buffer outside it, shared as a dmabuf, which llvmpipe maps.
- A GPU client that does not compare device numbers, as Mesa does, may still share a buffer the
  processor cannot map: Yuna refuses it.

Each dmabuf is checked and imported when its client creates its buffer, so that the client
learns at once whether it can be used. Yuna takes only buffers that cannot make it fault when it
draws: each plane in a file that can never shrink, and on llvmpipe, laid out row by row within
its file and mapping as llvmpipe maps it. Anything else is refused: `failed`, or for
`create_immed` the protocol's error, which ends that client alone. The module says why each check
exists.

### Seats and input

A seat ([`seats.rs`](../crates/yuna/src/seats.rs),
[ADR-0078](adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md),
[ADR-0095](adr/0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md))
is a Smithay seat with a keyboard and, for an agent's, a pointer. Each has its own focus, popup
grab, selections, cursor, clients, outputs, socket, X display and branch of the layout. Seats
isolate focus and input, not processes: any program of the user may reach any seat's socket.

- **The owner's seat**, `OWNER_SEAT`, exists from the start and takes no input headless; in
  `yuna mcp` it has no `wl_seat` global, output or socket. **An agent's seat** gets the next id,
  never reused, a `wl_seat` named `agent-<id>`, and a `us` keyboard whatever the machine's
  settings.
- **A client belongs** to the seat whose socket it came through, recorded in its `ClientState`
  with its process (`ClientProcess`), read once when Yuna accepts it. Its windows open on that
  seat's workspaces, and every focus of a seat is on its own clients' surfaces. Yuna's fork of
  Smithay shows each client only its own seat's `wl_seat` and outputs
  ([ADR-0042](adr/0042-main-on-smithays-fork.md)).
- **Input** reaches an agent's seat through the compositor's calls, which MCP makes
  ([`input.rs`](../crates/yuna/src/input.rs)), and through its clients' virtual keyboards and
  pointers ([`virtual_input.rs`](../crates/yuna/src/virtual_input.rs)). A pointer finds what is
  under it where the frame drew it. A press focuses the window under it, whose keyboard follows
  at the next refresh. Keys are named by keysym and typed through the seat's keymap
  ([`keymap.rs`](../crates/yuna/src/keymap.rs)), which its agent may change. Text that no key
  types goes to the focused text field through `text-input-v3`, Yuna being the input method
  ([`text_input.rs`](../crates/yuna/src/text_input.rs)), or else through a keymap Smithay injects
  for it.
- **Removing a seat** (`Compositor::remove_seat`): its keys and buttons are released and its
  focus leaves first; then its socket and X display go, its clients are ended and freed, and last
  its outputs, its place in the layout and its `wl_seat` global. A seat's outputs coming and going,
  or its removal, never end another seat's motion.
- **How it fails.** A seat's socket that fails to `accept`, most often out of descriptors, pauses
  `SOCKET_PAUSE` rather than stop Yuna.

### Selections

Each seat has its own clipboard and primary selection
([`selection.rs`](../crates/yuna/src/selection.rs)), which follow its keyboard focus. A seat's
focus is only on its own clients, so a selection never crosses seats.

- **Between Wayland clients**, on Smithay's handlers. When the keyboard moves to another
  client's window, that client is offered the selections just before its `enter`. Only the
  client with the focus may set them; Smithay does not check the serial ([debt](debt.md)). A
  reader's pipe goes to the source's client: Yuna never reads, writes or waits on it. A selection
  lasts as long as its source. `ext-data-control-v1` reaches every seat's selections, so only the
  owner's seat's clients see it.
- **Yuna holds a selection itself** for two kinds of owner: an X11 program of the seat, and an
  agent through MCP.
- **Between X11 and Wayland**, both ways
  ([ADR-0091](adr/0091-the-clipboard-between-x11-and-wayland-who-reads-and-its-bounds.md)). The
  seat's X11 window manager speaks X11's side on its thread
  ([`xwayland/bridge.rs`](../crates/yuna/src/xwayland/bridge.rs), which tells the flow step by
  step). Yuna's loop decides what the seat's Wayland selections are
  ([`xwayland/selections.rs`](../crates/yuna/src/xwayland/selections.rs)) and never touches X11's
  data.
  - An X11 client's selection becomes the seat's Wayland selection, held by Yuna. A Wayland
    reader's pipe goes to the manager, which converts the selection and writes into the pipe.
  - A Wayland client's or an agent's selection is told to the manager at the round's end; the
    manager takes the X11 selection and serves each request from a pipe the source writes into.
  - X11 reads the seat's Wayland selections only while one of the seat's X11 windows has the
    keyboard, as a Wayland client reads them only with the focus.
  - Transfers are bounded in time, size and number (`X11_TRANSFER_TIMEOUT`,
    `MAX_X11_TRANSFER_BYTES`, `MAX_X11_TRANSFERS`). Each failure ends its own transfer alone.
- **An agent's clipboard**, MCP's `clipboard` tool
  ([ADR-0101](adr/0101-an-agents-clipboard-through-mcp.md)). `get` reads the selection as a
  client's paste would; the server watches the pipe on the loop and never waits. `set` makes text
  the seat's selection, held by Yuna, which writes it into each reader's pipe at the reader's pace
  (`MAX_CLIPBOARD_WRITES`, `CLIPBOARD_TIMEOUT`). `clear` empties the selection; an X11 owner's
  stays its own in X11 ([debt](debt.md#xwayland)).

#### Drag and drop

A drag starts from a button an agent's pointer holds: Smithay accepts it only with the serial of
the pointer's grab. It offers its source to the surfaces of its seat that the pointer crosses,
and drops where the button is released. A drag from another seat's client is refused. The
owner's seat has no pointer headless, so its clients start no drag. The drag's icon is not drawn
([debt](debt.md)). `input.rs` gives the rules.

Between X11 and Wayland, drags go both ways in XDND
([ADR-0102](adr/0102-drag-and-drop-between-x11-and-wayland-a-drag-session-beside-smithays-grabs.md)).
Three parts share the work, so that each rule is tested alone:

- [`xwayland/drag_session.rs`](../crates/yuna/src/xwayland/drag_session.rs), each seat's *drag
  session*, decides. It is plain Rust, with no Smithay, Wayland or X11 type.
- [`xwayland/drags.rs`](../crates/yuna/src/xwayland/drags.rs) carries out its decisions, with
  Smithay's data device and the manager's commands.
- [`xwayland/xdnd.rs`](../crates/yuna/src/xwayland/xdnd.rs), on the manager's thread, speaks
  XDND. It answers each X11 peer at once from what Yuna last told it. An X11 source finds a
  Wayland target through a proxy window over the root, shown while the drag is over a Wayland
  window or over nothing.

A drag's data moves through the clipboard's transfers, in their count and bounds. A seat holds
two drags at most, the one in the hand and one ending. The clock's drags deadline is armed only
while an X11 program's drag, released, waits for its source's drop or leave.

### XWayland

X11 programs run on Xwayland, an X server that is a Wayland client of Yuna's: one for each seat,
started by the seat's first X11 connection, ended when idle, behind a cookie of its own
([ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md),
[ADR-0067](adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)). It is on in
`yuna --headless` and `yuna mcp`, and off in tests unless one asks (`Config::xwayland`).
[`xwayland/mod.rs`](../crates/yuna/src/xwayland/mod.rs) gives the whole; `display.rs` the
display's files; `server.rs` the process; `manager/` the X11 window manager.

- **A seat's display** comes with its Wayland socket: a number, an abstract and a file socket, a
  lock in `/tmp`, and a cookie in an Xauthority file in a folder of the seat's own. At rest it
  holds the two sockets and nothing else: no process, no timer.
- **Watching, serving, paused**, all on Yuna's loop, none waiting. The first connection starts
  Xwayland, rootless; its Wayland connection is a client of the seat, at XWayland's scale. Once
  Xwayland is ready, the manager's thread connects and takes charge, and Xwayland accepts the
  waiting program. Xwayland ends itself `IDLE_SECONDS` after its last X11 client leaves. Starts
  that fail in a row pause the display, longer each time.
- **The manager's thread** waits on X11, so that Yuna's loop never does. It posts what X11 windows
  do through a bounded channel; Yuna handles those events in a slice of each round, so that a
  burst of thousands of windows never holds another window's frames long
  ([measured](measurements.md#m18--a-flood-of-x11-windows-and-menus-2026-11-15)). Yuna's commands
  go back at the round's end.
- **On llvmpipe**, Xwayland draws in shared memory and has no GLX, so that it touches no GPU.
- **How it fails.** Xwayland may crash or be killed. Yuna ends it, with SIGKILL through its
  pidfd, when its start fails, when its manager fails or panics, and when it outlives its
  manager's connection by `MANAGER_GRACE`. Its seat's X11 programs end with it, and the display
  is watched again. Its pidfd reaps it; in `yuna mcp`, whose reaping of every child may take it first,
  `Compositor::child_reaped` hands its status over. When the compositor drops, it ends its
  Xwaylands, waits until each is reaped, and removes the displays' files.

### An X11 window's life

An X11 window joins the same record, `Toplevel`, and the same layout as an `xdg_toplevel`
([`xwayland/windows.rs`](../crates/yuna/src/xwayland/windows.rs),
[ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md)). The manager posts what it learns
(`ManagerEvent`, in `manager/messages.rs`), and does itself what ICCCM and EWMH settle alone;
Yuna decides, and its `Command`s go back at the round's end.

- The manager never reparents: Yuna draws no title bars ([ADR-0024](adr/0024-no-title-bars.md)).
  Java's AWT then needs `_JAVA_AWT_WM_NONREPARENTING=1` ([debt](debt.md#xwayland)).
- Every property it reads is bounded, and a malformed value is taken as unset
  (`properties.rs`, [ADR-0070](adr/0070-bounds-on-what-one-x11-client-may-ask.md)).
- It takes no event another X11 client sent with `SendEvent` but client messages and selection
  answers, so that a made-up event of another program's window changes nothing. Only Xwayland's
  own `WL_SURFACE_SERIAL` pairs a window with its surface.

The life of a window:

1. **Created**: Yuna records it, its X11 client, its rectangle and its properties.
2. **Asks to be mapped**: a managed window, neither override-redirect nor a dock or a desktop, is
   configured to the size `Layout::size_on_open` foretells from its size hints, its
   `WM_TRANSIENT_FOR` and its `_NET_WM_STATE`, then mapped. A `_NET_STARTUP_ID` that names an
   unused activation token Yuna gave makes its opening an activation. A dock or a desktop is
   mapped as it asks, and is never a window of the layout.
3. **Paired**: Xwayland gives its surface the role through `xwayland-shell-v1`
   ([`xwayland/shell.rs`](../crates/yuna/src/xwayland/shell.rs)) and a serial, which the manager
   receives too; the two meet in either order. Xwayland's own mistake there is the only protocol
   error Yuna posts it: nothing one X11 client does ends the connection every X11 program of the
   seat goes through ([ADR-0067](adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)).
4. **Opens** in the layout once mapped, paired and with a buffer, as an `xdg_toplevel` does at its
   first buffer: its limits from its size hints, its size from its surface, both less its frame
   extents and divided by XWayland's scale. The caps on windows count the X11 client behind it,
   never Xwayland.
5. **Told its place** at each arrangement (`tell_x11`): an X11 rectangle, its logical place times
   XWayland's scale, and its `_NET_WM_STATE`. A fullscreen window on an output whose mode its
   client emulates gets that mode's rectangle, so that Xwayland scales its surface
   ([ADR-0069](adr/0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md)).
   Only a change is sent. The windows are then restacked, since Xwayland finds the window under
   the pointer through X11's stacking.
6. **Resized** with a sync request, when it lists `_NET_WM_SYNC_REQUEST`: Xwayland holds its
   commits until its counter answers or `X11_SYNC_TIMEOUT` passes, so that Yuna keeps drawing
   its last whole frame ([`xwayland/answers.rs`](../crates/yuna/src/xwayland/answers.rs)).
7. **Focused**: the seat's keyboard on its surface gives it the X input focus and
   `_NET_ACTIVE_WINDOW`. A client's own `_NET_ACTIVE_WINDOW` is an activation without a token.
8. **Asked to close**: `WM_DELETE_WINDOW`, else its client is ended. One that lists
   `_NET_WM_PING` is pinged too; unanswered within `X11_PING_TIMEOUT`, it is marked
   unresponsive, and never ended on a timer.
9. **Unmapped**, it leaves the layout and what Yuna waited for from it is let go; **destroyed**,
   Yuna forgets it.

The sync requests and pings of every X11 window share one clock deadline, armed only while one
waits.

**Menus.** An override-redirect window, a menu, a tooltip or a drag's icon, is placed and mapped
by its client alone ([`xwayland/menus.rs`](../crates/yuna/src/xwayland/menus.rs)). It is shown
once mapped and paired with a buffer; it is never in the layout and never focused by Yuna, since
X11 menus grab the input within Xwayland. When it is shown, moved, or names another window in
`WM_TRANSIENT_FOR`, it is given a window, by the first of these rules that finds one:

1. the managed window its `WM_TRANSIENT_FOR` names, if that window is open;
2. among its own client's managed windows shown on an output, topmost first, the one shown on the
   output under the menu's corner whose told X11 rectangle holds that corner;
3. the topmost of them.

With a window, a menu keeps its offset from that window's X11 corner as it was placed. It is
drawn just above the window, at the window's drawn corner plus that offset × s / S, cut to the
output, so that it slides with the window as a popup does. A window that leaves the layout gives
its menus to the rules again. A menu with no window is drawn at its X11 position, over every
window of the outputs of its seat it lies on. A client shows `MAX_MENUS_PER_CLIENT` menus at
most. The frame loop and the pointer walk only the menus shown, never every override-redirect
window, which a client may have by the thousand.

### Colours

[`colour/`](../crates/yuna/src/colour/mod.rs) serves `color-management-v1` on wayland-server,
Yuna's own handler rather than Smithay's
([ADR-0039](adr/0039-leaving-smithay-step-by-step.md),
[ADR-0047](adr/0047-color-management-v1-kwins-list.md),
[ADR-0048](adr/0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md)). Each output announces
one description: the owner's monitor's PQ volume when it is HDR, gamma 2.2 and sRGB otherwise. A
surface's preferred description is its window's workspace's output's. Each surface's own
description is recorded at its commit, for tests and agents to read
(`Compositor::window_colours`, MCP's `windows` with `colour`). Nothing is drawn differently: the
colour pipeline comes at Stage 5. The answers about a description are sent after the clients'
dispatch, never inside it, since wayland-backend panics when a request's new object is destroyed
before the request returns.

### Captures

A capture copies an output's *newest frame* ([ADR-0034](adr/0034-captures.md),
[ADR-0038](adr/0038-captures-coordinates-windows-and-wayland.md)): the frame its armed refresh
draws, if one is armed, else the frame its buffer holds. Nothing is drawn for a capture.
[`capture.rs`](../crates/yuna/src/capture.rs) holds the rules; `render::read_pixels` is the only
place pixels are read back.

- **An agent's capture**, MCP's `capture`: the pixels of the whole output, a region in logical
  units, or the rectangle a window was drawn at, are read on the state thread when their frame is
  drawn. Its *capture mapping* says where the image lies in the output's logical units. The server
  encodes the PNG on its encoder threads, `MAX_ENCODERS` at most
  ([`mcp/captures.rs`](../crates/yuna/src/mcp/captures.rs)), and answers once the clock has
  reached the frame's boundary.
- **A client's capture**, through `ext-image-copy-capture-v1`
  ([`screencopy.rs`](../crates/yuna/src/screencopy.rs)) or `wlr-screencopy-unstable-v1`
  ([`wlr_screencopy.rs`](../crates/yuna/src/wlr_screencopy.rs),
  [ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)). Every client sees the globals, and
  captures only its own seat's outputs. A frame is copied at a refresh, once for all the frames
  waiting, so that a client asking again and again costs one copy a refresh at most. A later
  frame waits for a refresh that draws something, so that a recorder costs nothing at rest. Each
  client, and all of them together, keep a bounded number of captures waiting
  (`MAX_CAPTURE_SESSIONS_PER_CLIENT`, `MAX_CAPTURE_SESSIONS`).
- **How it fails.** A frame that could not be drawn or read fails its captures, which the client
  may ask again; a removed output stops them.

### The rest auditor

At rest, nothing runs, polls or draws. The rest auditor ([`rest.rs`](../crates/yuna/src/rest.rs),
[ADR-0096](adr/0096-a-rest-auditor-naming-what-wakes-yuna-or-makes-it-draw.md)) names what broke
that rest: each wake of the loop, by source, seat and client, and each frame drawn, by cause.

- Every event source is inserted through `rest::insert_source`, with what it is. Clippy refuses
  calloop's own insertion anywhere else (`clippy.toml`).
- It counts, and makes no text unless asked: by MCP's `rest` (`Compositor::rest_counts`), or by
  `--rest-audit`'s line for each second that woke Yuna, whose own timer it never counts.
- Its memory is bounded by the clients connected and the seats that exist.

## The MCP server

`yuna mcp` is a headless Yuna of one MCP client's own: JSON-RPC on its standard input and output,
one message per line, nothing else there ([ADR-0033](adr/0033-the-mcp-server.md)).
[`mcp/mod.rs`](../crates/yuna/src/mcp/mod.rs) gives the server's parts, one module for each group
of tools; `protocol.rs` the two revisions it serves; `tools/` the tools' schemas, which
`tests/contracts/tools-list.json` holds as a contract
([ADR-0079](adr/0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)).

- **On the loop.** Everything runs on the state thread. The client's pipes, the programs' output
  and exits, SIGCHLD, the waiting calls' timers, the clipboard reads, the accessibility buses and
  the encoders' results are sources that only note what came. After each round,
  `Server::dispatch` reaps the children that ended, reads and answers the requests, records the
  seats' events and writes the replies.
- **Seats.** Everything an agent does is on a seat of its own, which `seat create` makes with its
  outputs, its Wayland socket and its X display
  ([ADR-0095](adr/0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)).
  Every tool but `clock` names it, `rest` when it is to name only that seat's; another seat's
  output, window or program is unknown there. The owner's seat has no output, socket or
  `wl_seat`. A Yuna on the manual clock holds one seat at a time.
- **The tools**: `seat`, `outputs`, `workspaces`, `spawn`, `windows`, `events`, `accessibility`,
  `capture`, `pointer`, `keyboard`, `clipboard`, `clock` and `rest`. Each one's arguments and
  results are in its schema (`mcp/tools/schemas.rs`).
- **Calls that wait**, `windows`, `events`, `rest`, `clipboard get`, `accessibility` and a
  `spawn` waiting for its seat's bus, are answered from a later round or when their time is up,
  on real time, `MAX_WAIT` at most. `MAX_WAITING_CALLS` wait at once. `notifications/cancelled`
  drops one, and a seat's closing fails those on it
  ([`mcp/waits.rs`](../crates/yuna/src/mcp/waits.rs)). A capture waits for its frame, its encoder
  and its frame's boundary.
- **Programs** ([`mcp/programs/`](../crates/yuna/src/mcp/programs/mod.rs)). Each program `spawn`
  starts gets an environment of its own (`environment.rs`), a log of its output (`log.rs`), and a
  process group of its own, which `stop`, `freeze`, `thaw` and `slow` signal (`pace.rs`). Yuna is
  its programs' subreaper, so that their whole tree stays Yuna's to reap and to end
  (`processes.rs`). Their folder, `yuna-mcp-<pid>` in `XDG_RUNTIME_DIR`, holds the seats'
  sockets, the logs and the programs' runtime folders; it is locked while Yuna lives
  (`folder.rs`). `MAX_RUNNING` programs run at once, and `MAX_KEPT` are remembered.
- **Events** ([`mcp/events.rs`](../crates/yuna/src/mcp/events.rs),
  [ADR-0093](adr/0093-a-call-that-waits-for-the-seats-events.md)). Each agent seat keeps a log of
  its last `MAX_EVENTS` events. Once a round, when the compositor's description generation has
  moved, the server looks at its windows and outputs and records what changed; it also records
  the clients the display ended, with their protocol error, and the programs reaped. The cursor
  is the agent's: the server keeps no state of a call.
- **Closing a seat** (`seat close`, [`mcp/seats.rs`](../crates/yuna/src/mcp/seats.rs)). The
  compositor removes the seat at once ([Seats and input](#seats-and-input)) and the calls waiting
  on it fail. Then every process that belongs to the seat gets SIGTERM and SIGCONT, and SIGKILL
  `END_GRACE` later; the call is answered once they are gone. A process that empties its
  environment and leaves its group and its parents is not found
  ([debt](debt.md#the-mcp-server)).
- **Bounds.** A line over `MAX_LINE` ends the connection. While `MAX_QUEUED_OUTPUT` of replies
  wait, no further request is read.
- **Stopping.** When its input closes, or at SIGTERM or SIGINT, Yuna ends every process under it
  while it still serves their clients, then removes its folder. The folder a Yuna killed by
  SIGKILL left is removed by the next `yuna mcp`, once its lock is free.
- **How it fails.** A panic of the server's thread, the state thread, is fatal
  ([The program](#the-program)).

### Accessibility

Each agent's seat may have an accessibility bus of its own, on which Yuna is the AT-SPI registry
and the one reader ([ADR-0094](adr/0094-the-accessibility-tree-yuna-as-the-registry-of-each-seats-bus.md)).
An agent reads the trees of the programs it started with `accessibility: true` through the
`accessibility` tool. [`mcp/accessibility/mod.rs`](../crates/yuna/src/mcp/accessibility/mod.rs)
gives the call and its bounds; `bus.rs` the bus's start; `registry.rs` the registry's answers;
[`dbus/`](../crates/yuna/src/dbus/mod.rs) Yuna's own D-Bus client.

- The seat's first `spawn` with `accessibility: true` starts `dbus-daemon`, never before. Yuna
  connects to it and owns `org.a11y.atspi.Registry`; then the program starts with the bus's
  address.
- A call walks the trees of the applications on its seat's bus, its calls pipelined. Each
  application has a deadline (`APP_DEADLINE`), so that one that does not answer never holds the
  others. A node's rectangle is in the units `pointer` takes.
- Yuna's D-Bus client refuses bad input from the bus with an error, never a panic, and refuses to
  send a message the bus would disconnect it for, such as a name a program gave that breaks the
  specification's rules.
- A bus whose daemon ends is forgotten and its socket removed; the next such `spawn` starts
  another. The daemon is one of its seat's processes, ended with the seat.
- At rest, nothing runs: no timer and no call
  ([measured](measurements.md#m111-accessibility-at-rest-2026-11-18)).
- Seats are no security boundary: any process of the user reaches any seat's bus, as it reaches
  any seat's socket ([ADR-0078](adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)).

## The program

[`main.rs`](../crates/yuna/src/main.rs) reads the command line, whose contract is `yuna --help`
(`tests/contracts/help.txt`), and runs one of two modes:

- `yuna --headless` starts the compositor on real time, with its outputs and a socket in
  `XDG_RUNTIME_DIR`. It prints `WAYLAND_DISPLAY`, then `DISPLAY` and `XAUTHORITY` when Xwayland
  is found, and runs until SIGTERM or SIGINT. With `--agent-seat`, an agent's seat takes the
  outputs and the socket: what the owner's other projects drive
  ([ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)).
- `yuna mcp` starts the same compositor without an output or a socket, and serves MCP until its
  input closes, or SIGTERM or SIGINT comes.

Both turn XWayland on and animations off. A usage error prints why and the help, and exits with
2. The signal handlers write to a pipe the loop watches
([`stop_signal.rs`](../crates/yuna/src/stop_signal.rs)), so that the loop returns and the socket
is removed. In `yuna mcp`, SIGCHLD wakes the loop only when a child ends, never when one is
stopped or continued.

**The fatal path** ([`fatal.rs`](../crates/yuna/src/fatal.rs),
[ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)). The panic hook, installed
first, keeps the standard library's message. On the state thread, it then removes the X
displays' files in `/tmp`; `yuna mcp` ends every process under it and removes its folder; and
Yuna exits with status 101 through `_exit`, so that no destructor runs on a state left half
changed. `yuna --headless` leaves its Wayland socket and its lock, which the next server takes as
stale, and its Xauthority folder beside them ([debt](debt.md#the-headless-compositor)). A panic on
another thread unwinds that thread alone: an encoder's fails its capture, and an X11 window
manager's ends its Xwayland. Nothing starts Yuna again yet: ADR-0080's keeper comes with the
desktop.

**The log** goes to standard error, at `YUNA_LOG`'s level. At `debug`, tests and measures read
these lines: `window opened` when a window maps; `frame drawn` for each frame, with its drawing
time `took_us`, the boundary it aimed at and the one it was presented at; `counters` after each
round that woke; `capture copied` and `capture encoded`, each with `took_us`.

## Tests

`cargo test --workspace` runs every test but the ignored ones, as `scripts/check.sh` does;
[CONTRIBUTING](../CONTRIBUTING.md) says how to run a part. Each test's name says what it checks,
and each test file's documentation says what the file covers.

- **Unit tests** sit beside their code, in a `tests` module or a `tests.rs` beside the module.
- **The layout model**: `crates/yuna-layout/tests/scenarios/`, one test per behaviour, through a
  harness that checks every invariant after each operation (`harness.rs`); a test that rewrites
  a behaviour of niri or Hyprland names its source
  ([ADR-0019](adr/0019-the-layouts-behaviours.md)). `tests/properties.rs` runs random sequences
  of operations.
- **Motion**: `crates/yuna-motion/tests/`, against numerical integration and known values,
  scenarios, and random sequences.
- **The headless compositor**: `crates/yuna/tests/headless/`, one test binary. The compositor
  runs in the test's own thread on the manual clock, beside test clients written with
  `wayland-client`: `client.rs` and `client/`, and the `*_client.rs` files for selections, input,
  captures, colours, X11 and XDND. `main.rs` holds the windows' stories and the helpers
  (`manual_compositor`, `advance_to`, `run_armed_deadlines`, `assert_rests`); one file per subject
  holds the others. Each test is a short story with exact figures, most on the default output,
  3840 × 2160 at 240 Hz and scale 1.5.
- **Robustness**: `misbehaving.rs` and `x11_misbehaving.rs` run hostile clients beside a
  well-behaved *sentinel*. Each story checks that Yuna's state stays sound
  (`Compositor::check_windows`, `Compositor::check_popups_and_keyboard`), that the sentinel is
  still served, and that Yuna rests. `random_requests.rs` runs random sequences of requests from
  up to three clients, in four tests of 80 cases each, with animations off and on: a count that
  fitted about 10 s of the check in a debug build at M1.4
  ([measurements](measurements.md#m14-phase-b2--the-protocols-and-the-robustness-tests-2026-11-12)).
  `PROPTEST_CASES=<n>` runs more. A sequence found failing goes into `sequences_found_before`,
  since a seed replays its case only while the generator stays the same.
- **The program**, run as a process, in `crates/yuna/tests/`: `command_line.rs`; `contracts.rs`,
  which compares `--help` and `tools/list` with `tests/contracts/`; `mcp.rs`, `yuna mcp` with its
  programs; `applications.rs`, real applications, with Yuna's rest read from its log and `/proc`
  ([ADR-0025](adr/0025-applications-in-ci.md)); `consumers.rs`, the owner's other projects'
  drivers ([ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)). They share
  `tests/common/`. SlimeEngine's examples run in ignored tests, which a weekly job runs
  (`.github/workflows/slimeengine.yml`).
- **Documents as tests**: `headless/protocols.rs` checks that the rows `Offered` of
  [`protocols.md`](protocols.md) are the globals each seat's clients are shown.

To add a test:

- a layout behaviour: a scenario in `crates/yuna-layout/tests/scenarios/`, always;
- a compositor behaviour: a story in its subject's file in `crates/yuna/tests/headless/`, driving
  the manual clock, its figures exact; a new request goes into its test client;
- a bug: the story that fails first, then the fix;
- what needs Yuna as a process, a real application or another project's driver: a test in the
  program-level files.

## The tools around the code

- `flake.nix`: the development shell, and Yuna's package. The package wraps `bin/yuna` so that
  Yuna draws with the package's own Mesa and finds Xwayland and `dbus-daemon`, for Yuna alone:
  the caller's own `PATH`, `LD_LIBRARY_PATH` and `__EGL_VENDOR_LIBRARY_FILENAMES` are kept in
  `YUNA_CALLER_*`, which `yuna mcp`'s programs get back (`mcp/programs/environment.rs`). Its
  `bench` shell holds what the rivals' bench runs. `rust-toolchain.toml` pins the toolchain
  ([ADR-0016](adr/0016-the-rust-toolchain.md)).
- `scripts/check.sh`: every automated check, which CI runs (`.github/workflows/ci.yml`);
  `docs.yml` runs its document checks alone, and `slimeengine.yml` SlimeEngine's examples.
- `deny.toml`: the dependency policy. `clippy.toml`: the rule that every event source goes
  through the rest auditor.
- The measuring examples, each documented in its file, their figures in
  [`measurements.md`](measurements.md):
  - in `crates/yuna/examples/`: `frame_time.rs`, a frame's cost while `vkcube` draws;
    `agent_time.rs`, an agent's calls and captures; `motion_time.rs`, motion's budget
    ([ADR-0056](adr/0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md));
    `xwayland_time.rs`, XWayland's start and cost; `instances.rs`, N `yuna mcp` against one with N
    seats; `layout_time.rs`, a keyboard layout's change; `rest_cost.rs`, the rest auditor's cost;
    `events_look.rs`, the events' look. What they share is in `examples/common/`, whose
    [`mod.rs`](../crates/yuna/examples/common/mod.rs) says which file each one includes;
  - `crates/yuna-layout/examples/timing.rs` and `crates/yuna-motion/examples/motion_timing.rs`:
    the models' costs at an operation and at a frame.
- `crates/yuna-bench`: [the rivals' bench](#the-rivals-bench).

## The rivals' bench

`crates/yuna-bench` measures compositors from outside, the same way for each, Yuna among them
([ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)).
Its crate documentation, [`lib.rs`](../crates/yuna-bench/src/lib.rs), names each part; how to
run it is in [CONTRIBUTING](../CONTRIBUTING.md). It depends on nothing of `yuna`.

A *case* is one compositor, one configuration and one scenario. A run of a case:

1. **The sandbox** (`sandbox.rs`): a transient systemd scope, an environment built from nothing,
   a private session bus. The compositor moves itself into a child group, `core`. The scope is
   the *stack*, `core` the *core*.
2. **The recipe** (`recipes.rs`): the compositor's program, environment and configuration, *as
   shipped* or at *equal work* ([ADR-0072](adr/0072-budgets-set-against-the-peers-the-same-day.md)).
3. **Ready, then checked**: once the compositor is ready, and again once a scenario's clients are
   up, the case is *refused* if a process holds a display device or an input device, reaches the
   owner's runtime folder or names it in its environment, or is connected to an X11 display
   served outside the sandbox.
4. **The windows** (`scenarios.rs`): exact CPU times from the control groups' `cpu.stat`; each
   thread's time and wake-ups and each DRM client's engine times from readers every 250 ms
   (`sampler.rs`); the machine's state each second (`machine.rs`); memory once ready and after
   the cost window.
5. **The frames**: a cost a frame only from an exact count of the window's own frames, by KWin's
   frame log, Umbriel's commits, or mpv in `game`. A count through `ext-image-copy-capture`, in a
   window apart, is a lower bound.
6. **The stop**, as the recipe says, then `cgroup.kill` for what is left.

The run file (`record.rs`) holds every figure with its unit, versioned. The bench writes it again
after each case, through a temporary file and a rename (`cli.rs`): a run cut short keeps every
case it ran.

`yuna-bench report` ([`report.rs`](../crates/yuna-bench/src/report.rs)) ranks only the runs of one
day and one machine state, only where the runs' spreads do not overlap, and never a lower bound;
its documentation gives the rules for each scenario.

Each recipe runs on the render node at 3840 × 2160, 240 Hz and scale 1.5 (`recipes::RECIPES`):

| Recipe | Backend | Its pace | Frames counted by | `motion` driven by |
|---|---|---|---|---|
| sway, sway-vulkan | wlroots headless, GLES2 or Vulkan | wlroots' timer | capture | `swaymsg` |
| KWin | its virtual backend | the mode's rate | its frame log | its D-Bus |
| Yuna | its headless backend, `--gpu` | the mode's rate | capture | none |
| dwl | wlroots headless | wlroots' timer | capture | the bench's virtual keyboard |
| mango | wlroots headless, SceneFX | wlroots' timer | capture | `mmsg` |
| Umbriel | wlroots headless, its SceneFX fork | wlroots' timer | its commits | `umbriel msg` |
| Hyprland | Aquamarine's headless, inside a dwl | the mode's rate | capture | `hyprctl` |
| Mutter | its headless backend | the mode's rate | mpv, in `game` alone | none |

niri and cosmic-comp have no recipe that runs, and are measured on the owner's tour alone
(`recipes::TOUR_ONLY`).

Known limits:

- The sandbox detects; it does not prevent. The rivals run as the owner's user, kept off the
  owner's display, devices, runtime folder and bus by their configuration and environment alone;
  what one did between two checks is not undone. Without the kernel's socket diagnostics, the
  checks see sockets bound in the owner's runtime folder, not those connected to one
  (`sockets.rs`).
- wlroots' headless backend sends a frame event 4 ms after each commit, so sway, dwl, mango and
  Umbriel draw up to 250 frames a second for a 240 Hz mode; their costs a second rank apart from
  the others' (`recipes::Pace`).
- Yuna and Mutter take no command for `motion` from outside.
- Hyprland runs inside dwl, since Aquamarine binds `xdg_wm_base` at version 6, which sway 1.12
  offers at 5. Its stack holds its parent, and its figures of the stack rank apart.
- mpv can stay at its first frames on Mutter: the bench then fails the case rather than measure
  an empty scene.

## Where to start reading

1. [`crates/yuna-layout/src/lib.rs`](../crates/yuna-layout/src/lib.rs), then
   [`layout.rs`](../crates/yuna-layout/src/layout.rs) for the operations, and a few scenarios,
   such as [`scrolling.rs`](../crates/yuna-layout/tests/scenarios/scrolling.rs).
2. [`crates/yuna-motion/src/lib.rs`](../crates/yuna-motion/src/lib.rs), then
   [`arrangement.rs`](../crates/yuna-motion/src/arrangement.rs).
3. [`crates/yuna/src/lib.rs`](../crates/yuna/src/lib.rs), then
   [`compositor.rs`](../crates/yuna/src/compositor.rs) for a round,
   [`frames.rs`](../crates/yuna/src/frames.rs) and [`shell.rs`](../crates/yuna/src/shell.rs),
   and a story of [`tests/headless/main.rs`](../crates/yuna/tests/headless/main.rs).
4. For X11: [`xwayland/mod.rs`](../crates/yuna/src/xwayland/mod.rs), then
   [`xwayland/windows.rs`](../crates/yuna/src/xwayland/windows.rs) and the manager's
   [`manager/mod.rs`](../crates/yuna/src/xwayland/manager/mod.rs).
5. For agents: [`mcp/mod.rs`](../crates/yuna/src/mcp/mod.rs), then the module of the tool you
   change, and [`tests/headless/mcp.rs`](../crates/yuna/tests/headless/mcp.rs).
6. For the rivals' bench: [`crates/yuna-bench/src/lib.rs`](../crates/yuna-bench/src/lib.rs), then
   [`scenarios.rs`](../crates/yuna-bench/src/scenarios.rs).
