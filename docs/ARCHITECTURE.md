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
  Wayland, Smithay, a renderer or a clock. Both name their ids from `yuna-types`, the shared
  vocabulary that depends on nothing.
- **The headless compositor**, the `yuna` crate's library, on Smithay. In front: the Wayland
  protocols, and for each seat [XWayland](#xwayland) with Yuna's own X11 window manager. Behind:
  the layout and the motion. Underneath: virtual [outputs](#outputs), refreshed by one
  [clock](#time-and-deadlines) and drawn by Smithay's GLES renderer, on Mesa's llvmpipe or, with
  `--gpu`, on the GPU's render node.
- **[The MCP server](#the-mcp-server)** around the compositor, for agents: seats, programs,
  input, captures, events, the clipboard and each seat's [accessibility bus](#accessibility).
  **[The program](#the-program)**, `yuna`, runs `yuna --headless` or `yuna mcp`.
- **[The rivals' bench](#the-rivals-bench)**, `yuna-bench`, apart: it measures every compositor,
  Yuna included, from outside; beside it, **[the parity script](#the-parity-script)** compares
  two builds of Yuna on figures the machine's load does not move.

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

- [`Compositor`](../crates/yuna/src/compositor.rs) owns the event loop, the Wayland display,
  `State`, the seats' sockets and the Xwaylands, which it ends first when it drops. `State` holds
  the rest: the layout, the motion, the clock, the outputs, the seats, the record of windows, the
  renderer, the selections and the rest auditor.
- The layout alone decides where each window goes and which window has each seat's focus. The
  shell's record, `Toplevel`, ties each `WindowId` to its surface.
- The clock owns every deadline of the frame loop.
- Each seat owns its branch of the layout, its focus, its selections, its outputs, its socket and
  its X display. Nothing one seat does reaches another's focus or input.
- The MCP `Server` owns the `Compositor`, the agents' programs, their logs, folder and homes, each
  seat's log of events and each seat's accessibility bus.
- Each X11 window manager owns its X11 connection and its view of X11's windows. It shares
  nothing with `State`: it talks to Yuna's loop through its channel of events and what Yuna owes
  it.

**What runs on which thread**
([`lib.rs`](../crates/yuna/src/lib.rs) gives each): the *state thread* runs the event loop and
everything `State` holds, and in `yuna mcp` the server, and never waits on a client, a pipe or an
X server; each running Xwayland's X11 window manager waits on X11 on a thread of its own; in
`yuna mcp`, encoder threads make the captures' PNGs.

**What survives which failure** ([ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)):

- A client's protocol error, or a request past a bound, ends or refuses that client alone
  ([ADR-0131](adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)).
- A panic of the state thread is fatal: Yuna stops at once ([The program](#the-program)).
- A panic of an encoder fails its capture alone. A panic, failure or stall of an X11 window
  manager ends its Xwayland. An Xwayland that ends takes its seat's X11 programs with it; the
  next X11 connection starts another.
- A seat's socket that cannot accept a connection pauses, and a seat whose X display cannot be
  made goes without X11: neither stops Yuna.

## Crates and dependencies

One Cargo workspace; its crates live in `crates/`.

| Crate | What it is | Depends on |
|---|---|---|
| `yuna-types` | the shared vocabulary: the ids, `Direction`, `Tiling` and `PixelRect`; level 0 of the map in [ADR-0145](adr/0145-the-crate-map-motion-on-the-layout-the-configuration-on-the-commands.md) | nothing |
| `yuna-ipc` | the commands a caller asks of the compositor, and why one is refused ([ADR-0146](adr/0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md)) | `yuna-types` |
| `yuna-layout` | the layout model | `yuna-types` |
| `yuna-motion` | the motion of the layout's arrangements | `yuna-layout`, `yuna-types` |
| `yuna` | the headless compositor as a library, its MCP server, and the `yuna` program (`src/main.rs`) | Smithay, `yuna-ipc`, `yuna-layout`, `yuna-motion`, `yuna-types`, and the crates below |
| `yuna-bench` | the rivals' bench and the parity script | nothing of Yuna's |

- Each dependency's reason is a comment beside it, in the root [`Cargo.toml`](../Cargo.toml)'s
  `[workspace.dependencies]` and in each crate's manifest. `deny.toml` holds the policy on
  licences and advisories; [CONTRIBUTING](../CONTRIBUTING.md#dependencies) says how a new
  dependency is accepted.
- `yuna` uses Yuna's fork of Smithay at a pinned revision
  ([ADR-0112](adr/0112-smithay-pinned-patched-in-a-public-fork-left-at-stage-2.md), its
  [patches](smithay-patches.md)), with only its `wayland_frontend`, `desktop` and `renderer_gl`
  features. Beside it, named directly though Smithay depends on them too: wayland-server, calloop,
  the server side of wayland-protocols and wayland-protocols-wlr, xkbcommon, drm-fourcc, and rustix
  for the system calls; x11rb for the X11 window manager; zbus's wire format for D-Bus; `serde` and `serde_json` for
  MCP; `png` for captures; `tracing` for the logs. Its tests and measuring examples add proptest,
  the Wayland client crates, and `yuna-bench`'s readers of `/proc` and the DRM statistics.
- At run time Yuna needs libxkbcommon, linked; libglvnd's EGL and Mesa, loaded; Xwayland, found
  on `PATH`; and `dbus-daemon` for the accessibility buses, named by `YUNA_DBUS_DAEMON`, else
  found on `PATH`. The development shell and Yuna's package give all four (`flake.nix`).
- The rules: `yuna-types`, `yuna-ipc`, `yuna-layout` and `yuna-motion` hold no Wayland or
  Smithay object;
  `yuna-types` depends on nothing; no crate re-exports another's types: each names the crate
  that defines them (ADR-0146); `yuna-bench` depends on nothing of `yuna`, so that it measures Yuna as it measures the
  others; `yuna` uses `yuna-bench` only in its tests and examples.

## The layout model

`yuna-layout` decides where every window is: on which seat, output and workspace, at which
rectangle, and which window each seat focuses
([ADR-0110](adr/0110-the-layouts-and-their-behaviours.md),
[ADR-0111](adr/0111-the-layout-models-shape-plain-ids-a-branch-per-seat-asked-and-committed-sizes.md)).
Its crate documentation, [`lib.rs`](../crates/yuna-layout/src/lib.rs), explains the tree and the
operations; `sizing.rs`, `scrolling.rs`, `dwindle.rs`, `floating.rs` and `geometry.rs` hold their
own rules.

- **Its boundary.** Plain ids in, rectangles out. The caller names seats, outputs and windows;
  the model names the workspaces and columns it creates, never giving an id twice. It holds no
  Wayland or Smithay object and reads no clock, so that every behaviour is a test without a
  compositor.
- **Who owns what.** A `Layout` holds seats; a seat holds its outputs, its workspaces left
  without an output, and its hidden special workspaces; an output holds its workspaces, shows one,
  and may show a special workspace over it. An operation reaches only its seat's branch, so an
  agent's seat never touches the owner's
  ([ADR-0138](adr/0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md)).
- **An operation** names a seat, for a key binding, which acts on what that seat focuses; or a
  window, for its client's request. The model finds them, applies the change to the workspace's
  tiling, scrolling columns or dwindle, or to its floating windows, then tidies the tree.
- **Reading it.** The compositor reads `Layout::arrange`: each window an output shows, bottom to
  top, with the size it is asked, its rectangle on the output's pixel grid, its states, and its
  workspace, layer and column, which motion follows. Nothing in the model runs between
  operations.
- **Clients' sizes.** A window has the size it is *asked*, for its configure, and the size its
  client *committed*, which it is drawn at. The compositor passes on each client's limits and
  committed geometry. A dialog opens floating, centred on its parent.
- **Its invariants.** `Layout::check_invariants` checks every rule the model keeps; the tests
  call it after every operation.
- **How it fails.** Every operation takes ids that may be stale and answers with an error, never
  a panic; a refused operation changes nothing.

## Motion

`yuna-motion` turns the layout's jumps into what each frame draws
([ADR-0052](adr/0052-motion-in-a-pure-crate.md),
[ADR-0118](adr/0118-frames-presentation-times-and-the-clock.md)). Its crate documentation,
[`lib.rs`](../crates/yuna-motion/src/lib.rs), explains its springs, its curves and the motion of
an arrangement.

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
  ([ADR-0109](adr/0109-animations-and-effects-what-moves-reduced-motion-and-games.md)).

## The headless compositor

### Modules

`crates/yuna/src`, one job per module. Each module's documentation gives its contracts.

| Module | Its job |
|---|---|
| [`compositor.rs`](../crates/yuna/src/compositor.rs) | `Compositor`, the public face: the event loop, the display and `State`; a round; seats and outputs added and removed |
| [`command.rs`](../crates/yuna/src/command.rs) | `yuna-ipc`'s commands turned into the layout's operations for their caller's seat, or, to close a window, a request to its client |
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
| [`canary.rs`](../crates/yuna/src/canary.rs) | the parity script's canaries, costs injected only with the `parity-canary` feature, which no shipped build has |
| [`process.rs`](../crates/yuna/src/process.rs) | what every program Yuna starts shares, what Yuna reads of a process in `/proc`, and its limit of open files |
| [`xwayland/`](../crates/yuna/src/xwayland/mod.rs) | each seat's X display, its Xwayland and its X11 window manager |
| [`dbus/`](../crates/yuna/src/dbus/mod.rs) | Yuna's own D-Bus client, for the accessibility buses |
| [`mcp/`](../crates/yuna/src/mcp/mod.rs) | the MCP server |
| [`main.rs`](../crates/yuna/src/main.rs), `fatal.rs`, `stop_signal.rs` | the program alone: its command line, its logs, its signals, its fatal path |

### A round of the event loop

`Compositor::dispatch` runs one round. `yuna --headless` repeats rounds through
`Compositor::run_until`; in `yuna mcp`, `Server::dispatch` runs each round, then serves what came;
tests call `dispatch` themselves, and `Compositor::advance` on the manual clock.

1. On the real clock, one calloop timer is armed at the clock's earliest deadline. The loop then
   waits for a source: a client, a new connection, that timer, an X11 window manager's events, or
   in `yuna mcp` the server's pipes and timers. Every source is inserted through the
   [rest auditor](#the-rest-auditor), which counts what woke Yuna.
2. The seats' sockets that paused on a failed `accept` listen again once their pause ends. The
   X11 window managers' events are handled, in a slice of the round (`MANAGER_SLICE`).
3. The clients' requests are read: Smithay dispatches them to Yuna's handlers.
4. The clock's deadlines due by now run.
5. The round ends (`Compositor::flush`): the clients the display ended are freed, Yuna's writes
   of agents' clipboards go on, the X11 window managers get their commands, what closed windows
   and destroyed surfaces left is freed, and the events written are sent.

### Time and deadlines

The clock ([`clock.rs`](../crates/yuna/src/clock.rs),
[ADR-0118](adr/0118-frames-presentation-times-and-the-clock.md)) owns every deadline of the frame
loop. Its `Deadline` names each kind, each armed once at most:

- two per output: its next refresh, and the boundary its held presentation feedback waits for;
- the hidden windows' frame callbacks;
- the X11 windows' sync requests and pings (`xwayland/answers.rs`);
- the drags between X11 and Wayland (`xwayland/drags.rs`).

In real mode, time is `CLOCK_MONOTONIC`; with no deadline armed, no timer is. In manual mode,
for tests and for `yuna mcp --manual-clock`, time starts at the real monotonic time and moves only
through `Compositor::advance`, a test's call or MCP's `clock` tool, which never moves it past real
time; each deadline it passes runs at its own time, in order. A *draw hook*
(`Compositor::set_draw_hook`) may move manual time while a frame draws, to make a drawing late.

Every other timer is an event source of its own, inserted through the rest auditor, which names
each kind in `rest::Source`: MCP's waiting calls and captures, the programs' pace and throttle, a
seat's closing, Xwayland's start, end and pauses and its manager's watchdog, a seat socket's
pause, the accessibility buses' start and walks, an agent's clipboard reads and writes, and the
rest log's own second, among others.

An output's *refresh boundaries* come at its rate from its creation, each computed from the rate,
so that errors never add up (`Boundaries`, in `output.rs`). An output under VRR asks its panel
instead (`vrr.rs`). Both answer the frame loop through `Timing`.

### A frame

The frame loop ([`frames.rs`](../crates/yuna/src/frames.rs),
[ADR-0118](adr/0118-frames-presentation-times-and-the-clock.md)) draws an output only at a refresh
something armed, and only if something on it is damaged. Its module documentation gives each
step; in short:

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
[ADR-0111](adr/0111-the-layout-models-shape-plain-ids-a-branch-per-seat-asked-and-committed-sizes.md)):

1. Its initial commit gets a first configure: the size `Layout::size_on_open` foretells, tiled on
   all four edges, decorations on the server's side ([ADR-0024](adr/0024-no-title-bars.md)).
2. Its first buffer opens it in the layout, with its parent, its limits and its window geometry.
   A window past a cap (`MAX_WINDOWS_PER_CLIENT`, `MAX_WINDOWS`), or that the layout refuses, is
   not shown, and gets frame callbacks at the hidden windows' rate
   ([ADR-0131](adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)).
3. Each later commit passes on its limits and geometry when they change, and arms its output's
   refresh when the window is shown.
4. Its configures follow the layout's arrangements, at refreshes. Fullscreen and maximise
   requests always get one, as xdg-shell asks.
5. A commit without a buffer, or the destruction of the toplevel or of its surface, closes it. A
   client that leaves, or that the display ends, has its toplevels destroyed by the round's end.

The command `CloseWindow` asks a window's client to close it: `xdg_toplevel.close`, or for an
X11 window `WM_DELETE_WINDOW`, else its X11 client is ended.

### A command

A caller (a test, MCP's `workspaces` tool, later key bindings and Hollow) asks
`Compositor::command(Caller, Command)` with `yuna-ipc`'s types
([ADR-0146](adr/0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md));
[`command.rs`](../crates/yuna/src/command.rs) answers it:

1. A window or an output the command names (`Command::window`, `Command::output`) must be the
   caller's seat's: any other, another seat's or none, is refused with `WindowNotOnSeat` or
   `OutputNotOnSeat`, alike, so that an agent neither reaches nor learns of another seat's.
2. A window given as none is the one the caller's seat focuses: `NoFocusedWindow` when none is.
3. `CloseWindow` asks the window's client to close it
   ([A window's life](#a-windows-life)); every other command becomes the layout's operations for
   the caller's seat, and the layout's refusal becomes a `Refusal` in its own words.
4. A command that changed the layout arms every output's next refresh, animated; a refused one
   arms nothing.

A command added is a variant in `yuna-ipc`, its window and output in `Command::window` and
`Command::output`, whose matches list every variant so that the compiler asks for them, and its
dispatch here, with its story in `tests/headless/commands.rs`.

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
`TOKEN_LIFETIME` and is used once, by a window of that seat: another seat's window that names it
leaves it unused. With a valid token, the layout decides (`Layout::request_activation`): the
window takes the focus at the next refresh, unless a fullscreen window holds the screen. Otherwise
the window is only marked urgent, so that no program takes a game's screen. An X11 window's
`_NET_STARTUP_ID` may name a token, and its client's `_NET_ACTIVE_WINDOW` is an activation without
one ([An X11 window's life](#an-x11-windows-life)).

[`dialog.rs`](../crates/yuna/src/dialog.rs) offers `xdg-dialog-v1`. Dialogs open on their
parent through `xdg_toplevel.set_parent`; the modal hint is read, and nothing uses it yet.

### Outputs

Virtual outputs ([`output.rs`](../crates/yuna/src/output.rs),
[ADR-0123](adr/0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md)) have any size,
rate and scale, and no monitor. Each belongs to one seat. A compositor keeps `MAX_OUTPUTS` at
most; through MCP, an output's side and all outputs' pixels
are bounded too (`mcp/outputs.rs`).

- **Added** (`Compositor::add_output`): to the right of every output. Its first frame is armed,
  and its global announced at the round's end.
- **Changed** (`Compositor::configure_output`, MCP's `outputs configure`): applied whole, or
  refused with nothing changed, as a monitor set to another mode
  ([ADR-0090](adr/0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md),
  [ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
  item 7). `State::configure_output` lists what follows: the layout's places, the buffer, the
  refresh timing, the colours, what clients and surfaces are told, the captures.
- **Removed** (`Compositor::remove_output`), unless it is its seat's last: its workspaces go to
  another output of its seat. Its surfaces leave it, its held feedback is discarded, its deadlines
  are disarmed, and its captures fail (`State::drop_output`). Its global is disabled at once and
  destroyed `RETIRED_GLOBAL_LIFETIME` later, so that a client that bound it late is not ended for
  it. A closed seat's `wl_seat` goes the same way.
- **Under VRR**, an output emulates amdgpu driving the owner's DisplayPort Adaptive-Sync monitor
  ([ADR-0119](adr/0119-emulated-vrr-as-amdgpu-and-the-owners-monitor-do.md)). Its panel
  ([`vrr.rs`](../crates/yuna/src/vrr.rs)) computes when a frame ready at a given time would show,
  when asked, never on a timer. A test in `vrr.rs` replays the monitor's traces
  (`crates/yuna/tests/fixtures/`); what the emulation does not reproduce is in the
  [debt](debt.md#vrr).
- **HDR**: an HDR output announces the owner's monitor's colours ([Colours](#colours)).

### Scale

Each surface is told the scale of the output its window is shown on: `wp_fractional_scale_v1`'s
preferred scale, and `wl_surface.preferred_buffer_scale`
([`scale.rs`](../crates/yuna/src/scale.rs), which says when). A client that follows the
fractional scale draws its buffer at its window's physical size and sets its viewport to its
logical size. Drawn at a rectangle the layout put on the physical grid, its window is then exact:
one buffer pixel to one physical pixel. A client that draws at an integer buffer scale is scaled
when drawn ([debt](debt.md)).

X11 windows are drawn by an element of Yuna's own
([`render/x11_surface.rs`](../crates/yuna/src/render/x11_surface.rs),
[ADR-0124](adr/0124-x11-windows-across-outputs-of-different-scales.md)). Xwayland draws every
window of its seat at one scale, *XWayland's scale* S: the largest of the seat's outputs when it
started, fixed for its life. On an output of scale s, an X11 pixel covers s / S physical pixels:
copied texel for texel at s = S, averaged by the *area filter* below S, scaled up bilinearly
above S. The pointer on an X11 window is measured from where this element draws it.

### Buffers

Clients' buffers come through `wl_shm` or `linux-dmabuf`
([`dmabuf.rs`](../crates/yuna/src/dmabuf.rs),
[ADR-0123](adr/0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md)).
`linux-dmabuf` is offered only when the renderer imports a format, with feedback that names one
main device: the render node with `--gpu`; on llvmpipe, no device (`dev_t` 0), since llvmpipe
reads a buffer with the processor, through a mapping of its file. Mesa's clients on the processor
then draw through `wl_shm`, and a GPU client shares buffers llvmpipe can map, or has its buffer
refused.

Each dmabuf is checked and imported when its client creates its buffer, so that the client
learns at once whether it can be used. Yuna takes only buffers that cannot make it fault when it
draws: each plane in a file that can never shrink, and on llvmpipe, laid out row by row within
its file and mapping as llvmpipe maps it. Anything else is refused: `failed`, or for
`create_immed` the protocol's error, which ends that client alone. The module says why each check
exists.

### Seats and input

A seat ([`seats.rs`](../crates/yuna/src/seats.rs),
[ADR-0138](adr/0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md),
[ADR-0117](adr/0117-an-agents-seat-through-mcp-tools-input-keyboard-text-and-closing.md))
is a Smithay seat with a keyboard and, for an agent's, a pointer. Each has its own focus, popup
grab, selections, cursor, clients, outputs, socket, X display and branch of the layout. Seats
isolate focus and input, not processes: any program of the user may reach any seat's socket or
bus.

- **The owner's seat**, `OWNER_SEAT`, exists from the start and takes no input headless; in
  `yuna mcp` it has no `wl_seat` global, output or socket. **An agent's seat** gets the next id,
  never reused, a `wl_seat` named `agent-<id>`, and a `us` keyboard whatever the machine's
  settings.
- **A client belongs** to the seat whose socket it came through, recorded in its `ClientState`
  with its process (`ClientProcess`), read once when Yuna accepts it. Its windows open on that
  seat's workspaces, and every focus of a seat is on its own clients' surfaces. Yuna's fork of
  Smithay shows each client only its own seat's `wl_seat` and outputs.
- **Input** reaches an agent's seat through the compositor's calls, which MCP makes
  ([`input.rs`](../crates/yuna/src/input.rs)), and through its clients' virtual keyboards and
  pointers ([`virtual_input.rs`](../crates/yuna/src/virtual_input.rs)). A pointer finds what is
  under it where the frame drew it. A press focuses the window under it, whose keyboard follows
  at the next refresh. Keys are named by keysym and typed through the seat's keymap
  ([`keymap.rs`](../crates/yuna/src/keymap.rs)), which its agent may change. Text that no key
  types goes to the focused text field through `text-input-v3`, Yuna being the input method
  ([`text_input.rs`](../crates/yuna/src/text_input.rs)), or else through a keymap Smithay injects
  for it. A seat holds `MAX_HELD_KEYS` keys and `MAX_HELD_BUTTONS` buttons at once, whoever
  presses them.
- **Removing a seat** (`Compositor::remove_seat`): its keys and buttons are released and its
  focus leaves first; then its socket and X display go, its clients are ended and freed, and last
  its outputs, its place in the layout and its `wl_seat` global. A seat's outputs coming and going,
  or its removal, never end another seat's motion.
- **Bounds.** A seat serves `MAX_CLIENTS_PER_SEAT` clients at once, its Xwayland neither counted
  nor refused ([ADR-0147](adr/0147-a-seat-connects-256-clients-at-most.md)). Past it, a
  connection on the seat's socket is accepted and closed at once while the socket listens on; Yuna
  logs a warning and tells MCP's agents `client_refused`, both at most once a second of real time
  per seat, with the refusals since the last told. An in-process connection
  (`Compositor::connect_client_to`) gets an error. A place is free once the display has ended
  the client that left. Yuna keeps up to `CLIENT_BUFFER_BYTES`, 1 MiB, of events for a client that
  does not read them; the display ends one whose next event passes it
  ([ADR-0136](adr/0136-a-client-that-stops-reading-1-mib-and-its-motion-merged.md)).
- **How it fails.** A seat's socket that fails to `accept`, most often out of descriptors, pauses
  `SOCKET_PAUSE` rather than stop Yuna, the owner's in `yuna --headless` too. The program raises
  its limit of open files at start ([The program](#the-program)).

### Selections

Each seat has its own clipboard and primary selection, which follow its keyboard focus, so a
selection never crosses seats. Between Wayland clients, Smithay's handlers carry them and Yuna
never touches the data ([`selection.rs`](../crates/yuna/src/selection.rs)). Yuna holds a seat's
selection itself for two kinds of owner: an X11 program of the seat, and an agent through MCP.

- **Between X11 and Wayland**, both ways
  ([ADR-0091](adr/0091-the-clipboard-between-x11-and-wayland-who-reads-and-its-bounds.md)). The
  seat's X11 window manager speaks X11's side on its thread
  ([`xwayland/bridge.rs`](../crates/yuna/src/xwayland/bridge.rs), which tells the flow step by
  step). Yuna's loop decides what the seat's Wayland selections are
  ([`xwayland/selections.rs`](../crates/yuna/src/xwayland/selections.rs)) and never touches X11's
  data. X11 reads the seat's Wayland selections only while one of the seat's X11 windows has the
  keyboard. Transfers are bounded in time, size and number (`X11_TRANSFER_TIMEOUT`,
  `MAX_X11_TRANSFER_BYTES`, `MAX_X11_TRANSFERS`); each failure ends its own transfer alone.
- **An agent's clipboard**, MCP's `clipboard` tool
  ([ADR-0101](adr/0101-an-agents-clipboard-through-mcp.md),
  [ADR-0132](adr/0132-an-agents-clipboard-data-with-control-characters-in-base64.md)). `get`
  reads the selection as a client's paste would; the server watches the pipe on the loop and never
  waits. `set` makes text the seat's selection, held by Yuna, which writes it into each reader's
  pipe at the reader's pace (`MAX_CLIPBOARD_WRITES`, `CLIPBOARD_TIMEOUT`). `clear` empties the
  selection; an X11 owner's stays its own in X11 ([debt](debt.md#xwayland)).

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
  session*, decides. It is plain Rust, with no Smithay, Wayland or X11 type, and tells each
  drag's flow.
- [`xwayland/drags.rs`](../crates/yuna/src/xwayland/drags.rs) carries out its decisions, with
  Smithay's data device and the manager's commands.
- [`xwayland/xdnd.rs`](../crates/yuna/src/xwayland/xdnd.rs), on the manager's thread, speaks
  XDND. It answers each X11 peer at once from what Yuna last told it. An X11 source finds a
  Wayland target through a proxy window over the root, shown while the drag is over a Wayland
  window or over nothing. After the release, the proxy stays until the source drops or leaves,
  the drag ends, or the pointer comes over an X11 window. Yuna never waits for the manager, so
  two races are left ([debt](debt.md#xwayland)).

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
display's files; `server.rs` the process, whose environment holds only the variables Yuna keeps
for it; `manager/` the X11 window manager.

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
  go back at the round's end, through what Yuna owes the manager, bounded
  (`manager/owed.rs`). A watchdog is armed only while a manager owes commands from an earlier
  round.
- **On llvmpipe**, Xwayland draws in shared memory and has no GLX, so that it touches no GPU.
- **How it fails.** Xwayland may crash or be killed. Yuna ends it, with SIGKILL through its
  pidfd, when its start fails, when its manager fails, panics or stalls, when what Yuna owes the
  manager passes its bound, and when it outlives its manager's connection by `MANAGER_GRACE`. Its
  seat's X11 programs end with it, and the display is watched again. Its pidfd reaps it; in
  `yuna mcp`, whose reaping of every child may take it first, `Compositor::child_reaped` hands its
  status over. When the compositor drops, it ends its Xwaylands, waits until each is reaped, and
  removes the displays' files.

### An X11 window's life

An X11 window joins the same record, `Toplevel`, and the same layout as an `xdg_toplevel`
([`xwayland/windows.rs`](../crates/yuna/src/xwayland/windows.rs), which gives its life step by
step). The manager posts what it learns (`ManagerEvent`, in `manager/messages.rs`), and does
itself what ICCCM and EWMH settle alone; Yuna decides, and its `Command`s go back at the round's
end.

- The manager never reparents: Yuna draws no title bars ([ADR-0024](adr/0024-no-title-bars.md)).
  Java's AWT then needs `_JAVA_AWT_WM_NONREPARENTING=1` ([debt](debt.md#xwayland)).
- What one X11 client may ask is bounded per client, never per Xwayland alone: the windows it
  opens, the properties it is read for, the menus it shows
  ([ADR-0131](adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)). A malformed property is
  taken as unset (`properties.rs`).
- It takes no event another X11 client sent with `SendEvent` but client messages and selection
  answers, so that a made-up event of another program's window changes nothing; its connection's
  stream keeps a made-up `KeymapNotify` from making x11rb lose its place (`manager/stream.rs`).
  Only Xwayland's own `WL_SURFACE_SERIAL` pairs a window with its surface.
- Its sync requests and pings share one clock deadline, armed only while one waits.

**Menus.** An override-redirect window, a menu, a tooltip or a drag's icon, is placed and mapped
by its client alone. It is shown once mapped and paired with a buffer, drawn just above the window
of the layout its rules give it, sliding with it as a popup does, or alone at its X11 position;
it is never in the layout and never focused by Yuna, since X11 menus grab the input within
Xwayland ([`xwayland/menus.rs`](../crates/yuna/src/xwayland/menus.rs) gives the rules and the
bounds).

### Colours

[`colour/`](../crates/yuna/src/colour/mod.rs) serves `color-management-v1` on wayland-server,
Yuna's own handler rather than Smithay's
([ADR-0112](adr/0112-smithay-pinned-patched-in-a-public-fork-left-at-stage-2.md),
[ADR-0047](adr/0047-color-management-v1-kwins-list.md),
[ADR-0128](adr/0128-hdr-virtual-outputs-and-when-colour-is-chosen.md)). Each output announces
one description: the owner's monitor's PQ volume when it is HDR, gamma 2.2 and sRGB otherwise. A
surface's preferred description is its window's workspace's output's. Each surface's own
description is recorded at its commit, for tests and agents to read
(`Compositor::window_colours`, MCP's `windows` with `colour`). Nothing is drawn differently: the
colour pipeline's shape comes with Stage 2a's renderer, HDR's conversion at Stage 5. The answers about a description are sent after the clients'
dispatch, never inside it, since wayland-backend panics when a request's new object is destroyed
before the request returns.

### Captures

A capture copies an output's *newest frame*
([ADR-0121](adr/0121-captures-what-is-seen-at-the-logical-size.md)): the frame its armed refresh
draws, if one is armed, else the frame its buffer holds. Nothing is drawn for a capture.
[`capture.rs`](../crates/yuna/src/capture.rs) holds the rules; `render::read_pixels` is the only
place pixels are read back, on the state thread ([debt](debt.md#captures)).

- **An agent's capture**, MCP's `capture`: the pixels of the whole output, a region in logical
  units, or the rectangle a window was drawn at, are read on the state thread when their frame is
  drawn. Its *capture mapping* says where the image lies in the output's logical units. The server
  encodes the PNG on its encoder threads, `MAX_ENCODERS` at most
  ([`mcp/captures.rs`](../crates/yuna/src/mcp/captures.rs)), and answers once the clock has
  reached the frame's boundary.
- **A client's capture**, through `ext-image-copy-capture-v1`
  ([`screencopy.rs`](../crates/yuna/src/screencopy.rs)) or `wlr-screencopy-unstable-v1`
  ([`wlr_screencopy.rs`](../crates/yuna/src/wlr_screencopy.rs)). Every client sees the globals,
  and captures only its own seat's outputs. A frame is copied at a refresh, once for all the
  frames waiting, so that a client asking again and again costs one copy a refresh at most. A
  later frame waits for a refresh that draws something, so that a recorder costs nothing at rest.
  Each client, and all of them together, keep a bounded number of captures waiting
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
one message per line, nothing else there
([ADR-0120](adr/0120-the-mcp-server-a-private-yuna-on-stdio.md)).
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
  ([ADR-0117](adr/0117-an-agents-seat-through-mcp-tools-input-keyboard-text-and-closing.md)).
  Every tool but `clock` names it, `rest` when it is to name only that seat's; another seat's
  output, window or program is unknown there. The owner's seat has no output, socket or
  `wl_seat`. A Yuna on the manual clock holds one seat at a time.
- **The tools**: `seat`, `outputs`, `workspaces`, `spawn`, `windows`, `events`, `accessibility`,
  `capture`, `pointer`, `keyboard`, `clipboard`, `clock` and `rest`. Each one's arguments are its
  schema and its results are told in its description (`mcp/tools/schemas.rs`); the arguments are
  read in `mcp/tools/arguments.rs`, and the results made in `mcp/tools/results.rs` and the tool's
  own module.
- **Calls that wait**, `windows`, `events`, `rest`, `clipboard get`, `accessibility` and a
  `spawn` waiting for its seat's bus, are answered from a later round or when their time is up,
  on real time, `MAX_WAIT` at most. `MAX_WAITING_CALLS` wait at once. `notifications/cancelled`
  drops one, and a seat's closing fails those on it
  ([`mcp/waits.rs`](../crates/yuna/src/mcp/waits.rs)). A capture waits for its frame, its encoder
  and its frame's boundary.
- **Programs** ([`mcp/programs/`](../crates/yuna/src/mcp/programs/mod.rs)). Each program `spawn`
  starts gets an environment of its own (`environment.rs`), its seat's *throwaway home*
  (`homes.rs`, [ADR-0105](adr/0105-an-agents-programs-on-a-throwaway-home.md)), a log of its
  output (`log.rs`), and a process group of its own, which `stop`, `freeze`, `thaw` and `slow`
  signal (`pace.rs`). Yuna is its programs' subreaper, so that their whole tree stays Yuna's to
  reap and to end (`processes.rs`). Their folder, `yuna-mcp-<pid>` in `XDG_RUNTIME_DIR`, holds the
  seats' sockets, the logs and the programs' runtime folders; the homes' folder is in `TMPDIR`.
  Each is locked while Yuna lives (`folder.rs`). `MAX_RUNNING` programs run at once, and
  `MAX_KEPT` are remembered.
- **Events** ([`mcp/events.rs`](../crates/yuna/src/mcp/events.rs),
  [ADR-0093](adr/0093-a-call-that-waits-for-the-seats-events.md)). Each agent seat keeps a log of
  its last `MAX_EVENTS` events. Once a round, when the compositor's description generation has
  moved, the server looks at its windows and outputs and records what changed; it also records
  the clients the display ended, with their protocol error, the connections the seats' sockets
  refused past their bound, and the programs reaped. The cursor is the agent's: the server keeps
  no state of a call.
- **Closing a seat** (`seat close`, [`mcp/seats.rs`](../crates/yuna/src/mcp/seats.rs)). The
  compositor removes the seat at once ([Seats and input](#seats-and-input)) and the calls waiting
  on it fail. Then every process that belongs to the seat gets SIGTERM and SIGCONT, and SIGKILL
  `END_GRACE` later: one under its running programs whatever its environment names, and others
  as `Programs::seat_processes` says. The call is answered once they are gone, and the seat's
  home goes. A process that empties its environment and leaves its group and its parents is not
  found ([debt](debt.md#the-mcp-server)).
- **Bounds.** A line over `MAX_LINE` ends the connection. While `MAX_QUEUED_OUTPUT` of replies
  wait, no further request is read, and a waiting call's answer is held until the client reads.
  What an agent may ask is bounded ([ADR-0131](adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)).
- **Stopping.** When its input closes, or at SIGTERM or SIGINT, Yuna ends every process under it
  while it still serves their clients, then removes its folder and the seats' homes. The folders
  a Yuna killed by SIGKILL left are removed by the next `yuna mcp` that finds them, once their
  lock is free.
- **How it fails.** A panic of the server's thread, the state thread, is fatal
  ([The program](#the-program)).

### Accessibility

Each agent's seat may have an accessibility bus of its own, on which Yuna is the AT-SPI registry
and the one reader ([ADR-0094](adr/0094-the-accessibility-tree-yuna-as-the-registry-of-each-seats-bus.md)).
An agent reads the trees of the programs it started with `accessibility: true` through the
`accessibility` tool. [`mcp/accessibility/mod.rs`](../crates/yuna/src/mcp/accessibility/mod.rs)
gives the call and its bounds; `bus.rs` the bus's start; `registry.rs` the registry's answers;
`reader.rs` a walk, bounded in what it keeps of each reply;
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

## The program

[`main.rs`](../crates/yuna/src/main.rs) reads the command line, whose contract is `yuna --help`
(`tests/contracts/help.txt`), and runs one of two modes:

- `yuna --headless` starts the compositor on real time, with its outputs and a socket in
  `XDG_RUNTIME_DIR`. It prints `WAYLAND_DISPLAY`, then `DISPLAY` and `XAUTHORITY` when Xwayland
  is found, and runs until SIGTERM or SIGINT. With `--agent-seat`, an agent's seat takes the
  outputs and the socket: what the owner's other projects drive
  ([ADR-0138](adr/0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md)).
- `yuna mcp` starts the same compositor without an output or a socket, and serves MCP until its
  input closes, or SIGTERM or SIGINT comes.

Both turn XWayland on, and animations off unless asked. Both raise their limit of open files at
start, and give the programs they start the limit they started with. A usage error prints why
and the help, and exits with 2. The signal handlers write to a pipe the loop watches
([`stop_signal.rs`](../crates/yuna/src/stop_signal.rs)), so that the loop returns and the socket
is removed. In `yuna mcp`, SIGCHLD wakes the loop only when a child ends, never when one is
stopped or continued.

**The fatal path** ([`fatal.rs`](../crates/yuna/src/fatal.rs),
[ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)). The panic hook, installed
first, keeps the standard library's message. On the state thread, it then removes the X
displays' files in `/tmp`; `yuna mcp` ends every process under it and removes its seats' homes and
its folder; and Yuna exits with status 101 through `_exit`, so that no destructor runs on a state
left half changed. `yuna --headless` leaves its Wayland socket and its lock, which the next server
takes as stale, and its Xauthority folder beside them ([debt](debt.md#the-headless-compositor)). A
panic on another thread unwinds that thread alone. Nothing starts Yuna again yet.

**The log** goes to standard error, at `YUNA_LOG`'s level; `main.rs` lists the `debug` lines
tests and measures read.

## Tests

`cargo test --workspace` runs every test but the ignored ones, as `scripts/check.sh` does;
[CONTRIBUTING](../CONTRIBUTING.md) says how to run a part. Each test's name says what it checks,
and each test file's documentation says what the file covers.

- **Unit tests** sit beside their code, in a `tests` module or a `tests.rs` beside the module.
- **The layout model**: `crates/yuna-layout/tests/scenarios/`, one test per behaviour, through a
  harness that checks every invariant after each operation (`harness.rs`); a test that rewrites
  a behaviour of niri or Hyprland names its source
  ([ADR-0110](adr/0110-the-layouts-and-their-behaviours.md)). `tests/properties.rs` runs random
  sequences of operations.
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
  still served, and that Yuna rests. `input_bounds.rs` and `x11_bounds.rs` push clients past
  their bounds; `core_costs.rs` counts the work of paths whose cost must follow the work asked.
  `random_requests.rs` runs random
  sequences of requests from up to three clients, in four tests of 80 cases each, with
  animations off and on: a count that fitted about 10 s of the check in a debug build at M1.4
  ([measurements](measurements.md#m14-phase-b2--the-protocols-and-the-robustness-tests-2026-11-12)).
  `PROPTEST_CASES=<n>` runs more. A sequence found failing goes into `sequences_found_before`,
  since a seed replays its case only while the generator stays the same.
- **The program**, run as a process, in `crates/yuna/tests/`: `command_line.rs`; `contracts.rs`,
  which compares `--help` and `tools/list` with `tests/contracts/`; `mcp.rs`, `yuna mcp` with its
  programs; `applications.rs`, real applications, with Yuna's rest read from its log and `/proc`
  ([ADR-0122](adr/0122-real-applications-in-ci-and-rest-checked-on-the-program.md));
  `consumers.rs`, the owner's other projects' drivers
  ([ADR-0126](adr/0126-yuna-and-the-owners-other-projects.md)); `opium.rs`, Opium's window, which
  an agent opens, clicks and checks by capture through `yuna mcp` alone. They share
  `tests/common/`. SlimeEngine's examples and Opium run in ignored tests, which weekly jobs run.
- **Documents as tests**: `headless/protocols.rs` checks that the rows `Offered` of
  [`protocols.md`](protocols.md) are the globals each seat's clients are shown.
- **Protocol traces held for parity**
  ([ADR-0139](adr/0139-stage-2-in-three-gated-parts-2as-path-revised.md)):
  `headless/traces.rs` runs eight scripted scenarios, each in a process of its own (the headless
  binary run again on `traces::traced_process`, with `WAYLAND_DEBUG=client`), and records what the
  test clients send and receive. `traces/normalise.rs` reads `wayland-backend`'s printout into
  typed messages by the generated descriptions, then writes, for each client, its objects by
  interface and rank, each with its own messages in order, and only the orders between objects
  the protocol requires. What the server chooses where the protocol gives no order is not held:
  the interleaving of objects, the order of globals, its numbers for serials and globals. Serials
  are named by the event that carried them, registry names by the object bound from them;
  descriptors, keymap sizes and error texts are masked, the renderer's dmabuf formats collapsed,
  sets sorted, the harness's own round trips left out. Times are kept from the first frame
  callback on the manual clock and masked on the real one. Each result is compared with
  `crates/yuna/tests/traces/<scenario>.<manual|real>.trace`; all eight run on the manual clock,
  the window, input and outputs scenarios also against `yuna --headless`, and
  `every_trace_is_the_same_five_times_in_a_row` keeps them deterministic.
- **Captures held for parity**
  ([ADR-0139](adr/0139-stage-2-in-three-gated-parts-2as-path-revised.md)):
  `headless/captures_held.rs` builds eight scenes (`captures_held/scenes.rs`) and captures each
  output through `ext-image-copy-capture-v1` into the test client's own `wl_shm` buffer, which Yuna
  fills from its readback; each capture is compared with a PNG in `crates/yuna/tests/captures/`.
  Each pixel has a class from its scene's geometry (`captures_held/compare.rs`): *opaque*, one
  buffer pixel to one output pixel, compared exactly; *blended* and *filtered*, within 2 on each
  channel. A failure says how far each class moved, so that a change made on purpose, such as a
  new blending, can be told from a regression. Yuna draws no cursor and no border; X11 windows
  are not captured.

To add a test:

- a layout behaviour: a scenario in `crates/yuna-layout/tests/scenarios/`, always;
- a compositor behaviour: a story in its subject's file in `crates/yuna/tests/headless/`, driving
  the manual clock, its figures exact; a new request goes into its test client;
- a protocol behaviour Stage 2a must keep: a step in a scenario of `headless/traces/manual.rs`
  (and `traces/real.rs`), its trace rewritten and its diff read;
- a change to what Yuna draws: a scene in `headless/captures_held/scenes.rs` and its test in
  `captures_held.rs`;
- a bug: the story that fails first, then the fix;
- what needs Yuna as a process, a real application or another project's driver: a test in the
  program-level files.

## The tools around the code

- `flake.nix`: the development shell, and Yuna's package. The package wraps `bin/yuna` so that
  Yuna draws with the package's own Mesa and finds Xwayland and `dbus-daemon`, for Yuna alone:
  the caller's own `PATH`, `LD_LIBRARY_PATH` and `__EGL_VENDOR_LIBRARY_FILENAMES` are kept in
  `YUNA_CALLER_*`, which `yuna mcp`'s programs get back (`mcp/programs/environment.rs`). Its
  `bench` shell holds what the rivals' bench runs. `rust-toolchain.toml` pins the toolchain
  ([ADR-0125](adr/0125-the-rust-toolchain-an-exact-stable-version-a-nightly-for-fuzzing.md)).
- `scripts/check.sh`: every automated check, which CI runs (`.github/workflows/ci.yml`);
  `docs.yml` runs its document checks alone. `slimeengine.yml` runs SlimeEngine's examples, and
  `opium.yml` Opium's window, each weekly and on a push that changes what its test builds or
  reads (`scripts/test_workflows.py` checks their filters), Opium built from its Nix package at a
  pinned revision, uncached ([ADR-0122](adr/0122-real-applications-in-ci-and-rest-checked-on-the-program.md)).
- `deny.toml`: the dependency policy. `clippy.toml`: the rule that every event source goes
  through the rest auditor.
- The measuring examples, each documented in its file, their figures in
  [`measurements.md`](measurements.md):
  - in `crates/yuna/examples/`: `frame_time.rs`, a frame's cost while `vkcube` draws;
    `agent_time.rs`, an agent's calls and captures; `motion_time.rs`, motion's budget
    ([ADR-0127](adr/0127-motions-budget-no-frame-lost-to-motion.md));
    `xwayland_time.rs`, XWayland's start and cost; `instances.rs`, N `yuna mcp` against one with N
    seats; `layout_time.rs`, a keyboard layout's change; `rest_cost.rs`, the rest auditor's cost;
    `events_look.rs`, the events' look. What they share is in `examples/common/`, whose
    [`mod.rs`](../crates/yuna/examples/common/mod.rs) says which file each one includes;
  - `crates/yuna-layout/examples/timing.rs` and `crates/yuna-motion/examples/motion_timing.rs`:
    the models' costs at an operation and at a frame; `operations.rs` and
    `motion_operations.rs` beside them, the same operations repeated and timed by nothing, for
    [the parity script](#the-parity-script).
- `crates/yuna-bench`: [the rivals' bench](#the-rivals-bench) and
  [the parity script](#the-parity-script).

## The rivals' bench

`crates/yuna-bench` measures compositors from outside, the same way for each, Yuna among them
([ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)).
Its crate documentation, [`lib.rs`](../crates/yuna-bench/src/lib.rs), names each part; how to
run it is in [CONTRIBUTING](../CONTRIBUTING.md). It depends on nothing of `yuna`.

A *case* is one compositor, one configuration and one scenario. A run of a case:

1. **The sandbox** ([`sandbox.rs`](../crates/yuna-bench/src/sandbox.rs)): a transient systemd
   scope, an environment built from nothing, a private session bus. The compositor moves itself
   into a child group, `core`. The scope is the *stack*, `core` the *core*.
2. **The recipe** ([`recipes.rs`](../crates/yuna-bench/src/recipes.rs), whose table gives each
   compositor's backend, pace, frame count and driver): the compositor's program, environment
   and configuration, *as shipped* or at *equal work*
   ([ADR-0072](adr/0072-budgets-set-against-the-peers-the-same-day.md)).
3. **Ready, then checked**: once the compositor is ready, and again once a scenario's clients are
   up, the case is *refused* if a process breaks the sandbox's rules. The sandbox detects; it
   does not prevent.
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
its documentation gives the rules for each scenario. The rivals on wlroots' headless backend draw
up to 250 frames a second for a 240 Hz mode, so their costs a second rank apart from the others'
(`recipes::Pace`). niri and cosmic-comp have no recipe that runs, and are measured on the owner's
tour alone (`recipes::TOUR_ONLY`). mpv can stay at its first frames on Mutter: the bench then
fails the case rather than measure an empty scene.

## The parity script

`yuna-bench parity run` compares two builds of Yuna, a *base* and a *head*, by the rule of
[ADR-0142](adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md). Every step of
Stage 2a must keep their figures
([ADR-0139](adr/0139-stage-2-in-three-gated-parts-2as-path-revised.md)). CI runs it at each push,
the head against the pinned baseline. Its modules are in
[`crates/yuna-bench/src/parity/`](../crates/yuna-bench/src/parity/mod.rs); how to run it is in
[CONTRIBUTING](../CONTRIBUTING.md#the-parity-script). It depends on nothing of `yuna`: it drives
both builds through `yuna mcp`, whose tools are its contract with them.

**The tiers** (`--tier`), ADR-0142's three:

- **CI** (`ci`, the default), at each push: figures the machine's load does not move, callgrind's
  instructions, DHAT's allocations ([ADR-0143](adr/0143-allocations-a-frame-counted-by-dhat.md)),
  memory, wakes and the layout's operations. The numbered steps below are its flow.
- **GPU** (`gpu`), an agent's run on the desktop, never CI: each CI scenario natively on `yuna
  mcp --gpu`, which opens the render node alone, and at rest after its measured periods the
  kernel's DRM memory of Yuna's DRM clients, each client once: *GPU memory*, `resident` less
  `shared`, every region summed (*parity*), and `shared` (*recorded*). Judged as the CI tier's
  figures are.
- **Timed** (`timed`), once per milestone, by an agent on the high tier's desktop under
  [ADR-0115](adr/0115-priorities-uwui-first-the-owners-activity-measures-that-check-their-load.md)'s
  rules: timed runs judged over pairs, cold starts, and a soak ([below](#the-timed-tier)).
  `timed,gpu` runs both tiers, and the timed runs on the GPU too.

A run of the CI tier:

1. **The builds** (`build.rs`). Each side is a revision, checked out as a git worktree in the
   work folder, or a folder built as it is. Each builds in its own development shell, `nix
   develop <tree>`, with its own toolchain and Mesa, both recorded, for release with line tables
   and in one codegen unit, so that a change elsewhere in a crate does not move what is inlined
   ([ADR-0148](adr/0148-parity-builds-each-side-in-one-codegen-unit.md)), in a target folder of
   its own, its paths remapped: one tree built twice gives the same binary,
   and two trees of one revision the same code (`.text`), their files differing only by the run
   path the shell's linker wrapper writes. `yuna` is built with rustix's `use-libc-auxv`, so that
   valgrind can run it. Each tree's thresholds file is read with it. The baseline files of the
   head and of the push before are read before any build, from git or the folder, so that a run
   that cannot tell which moves are new fails at once.
2. **The plan** (`mod.rs`): each scenario five ways on each side (natively, and under callgrind
   and DHAT, each `n` and `2n` periods), each case of the operations programs twice (`n` and
   `2n` repeats), all in a random order from a recorded seed, several at once.
3. **Each run** (`scenario.rs`, `launch.rs`) is a user namespace of its own, with its own PID,
   mount and network namespaces and its own `/tmp`. In it, a launcher starts `yuna mcp
   --manual-clock`, alone or under valgrind, and beside it the *lockstep client*
   (`lockstep.rs`). The script speaks MCP to Yuna on its standard input and output, and to the
   client through a socket. Each *period* gives Yuna one refresh of work, always in this order:
   the scenario's MCP calls (pointer motions, a workspace's switch), the client's step (its
   commits, then a round trip), and one advance of the clock by the output's period, which draws
   the frame. Nothing else runs. In its namespaces, Yuna's start and stop see the same processes
   and X displays in every run.
4. **The figures** (`figures.rs`). A valgrind figure a frame is the run of `2n` periods less the
   run of `n`, over `n`: Yuna's start, warm-up and stop cancel. Allocations a frame leave out
   what an advance costs alone, `idle`'s. A DHAT run ends Yuna by a signal it does not handle,
   SIGUSR1, so that it frees nothing first: the heap it still holds then, over the frames, is
   its heap's growth, a leak among it. Native runs send each message only once Yuna's main
   thread sleeps, and read `rest`'s wakes, the context switches of Yuna's own threads (its main
   thread and those it names `yuna-…`, never Mesa's), and, once Yuna is at rest (`clock now`
   arms no deadline), its wakes over 2 s of real time and its `Pss`. llvmpipe draws on Yuna's
   thread (`LP_NUM_THREADS=0`): under valgrind, threads that wait by spinning count what the
   scheduling gives them. LLVM, compiling llvmpipe's shaders at Yuna's start, still counts
   differently from run to run, its hash sets keyed by address, the heap laid out after Yuna's
   hash maps, which take random keys: tens of thousands of instructions, which the total's floors
   absorb (a filled shader cache does not spare that work). A timer on real time would distort
   every valgrind figure in proportion to the machine's slowness; none runs in a parity run
   today.
5. **Equal work, then the rule** (`rule.rs`, `thresholds.rs`). A scenario's figures are compared
   only if every run of it, on both sides, drew the same frames for the same causes, took the
   same commits, and gave its client the same frame callbacks, pointer events and configures,
   and made it the same buffers; otherwise they are *void*. Each figure has a class: *parity*,
   worse beyond its threshold; *budget*, against a figure of the ROADMAP or an ADR; *recorded*,
   never judged. The classes and thresholds are data,
   [`crates/yuna-bench/parity-thresholds.toml`](../crates/yuna-bench/parity-thresholds.toml),
   each threshold about three times its spread with a floor, by scenario or case where they
   differ. The head's file judges; a figure whose class the head changes, or whose threshold or
   budget it loosens, against the base's file is void unless a new move of the baseline names
   it. A figure that cannot be measured, its program missing from a tree included, is void. The
   exit status is 1 if a figure is worse, else 2 if one is void, else 0.
6. **The move, and the push before** (`mod.rs`). `--before` names the push before: CI passes a
   push's `github.event.before`, a pull request's base (`HEAD^1`, the first parent of the merge
   commit it checks out) and, for the schedule, the head itself. The moves the head's baseline
   file holds and the push before's lacks are the head's *new* moves; never judged against the
   base's file, which cannot hold the move to itself. By hand, a run against the pinned baseline
   needs `--before`. The head's moves must be the push before's with new ones at the end, and a
   new move may not pin the commit the move before it pinned; otherwise the run fails. At a push
   with a new move, the head is also run against the old baseline, the one the push before
   pinned: every figure worse there, or judged more loosely than the old baseline's file judges
   it, must be named by a new move, else the run is void. That check runs at that push alone: a
   move check that fails or is cancelled is never redone by CI, since the next push's push
   before holds the move. It is run again by hand, with `--before` the commit before the move,
   and no push goes on top of a red or cancelled move check. When a figure is worse, the push
   before is run on the scenarios that flagged and compared with the head, to say whether this
   push moved them, unless it is the head's own commit; it never changes the verdict.
7. **The run file and its report** (`record.rs`, `report.rs`): versioned JSON, written again
   after each run through a temporary file and a rename, and its Markdown, appended to the CI
   job's summary.

**The scenarios**, each with its windows and what changes each period: `idle` (nothing: an
advance alone), `partial` (a 64 × 64 square at 1080p), `full` (two whole windows; its runs,
shorter than the half second of drawings Yuna keeps to predict its lead, start after 121 periods
of a square, which fill the drawings Yuna keeps), `motion` (workspaces switched and a window
moved on springs), `scaled` (a square at 4K and scale 1.5), `many` (a square among 30 windows)
and `input` (8 pointer motions). Popups, X11 and captures
are not covered yet. The operations programs, `crates/yuna-layout/examples/operations.rs` and
`crates/yuna-motion/examples/motion_operations.rs`, print a digest of the work they did; two
builds whose digests differ are not compared.

**The baseline.** [`crates/yuna-bench/parity-baseline.toml`](../crates/yuna-bench/parity-baseline.toml),
the head's, names the commit every push is compared with, and each of its moves: the commit, an
earlier one that holds the change, the figures it moves, why, and the ADR or the owner's go. A
base that lacks a program the head has, an operations program for instance, makes its figures
void.

**The controls.** The base against itself, the *self control*, must pass, in every tier. Each
*canary*, built into the head with the `parity-canary` feature of `yuna`, `yuna-layout` or
`yuna-motion` and turned on by `YUNA_PARITY_CANARY`, injects one cost about twice its figure's
threshold, which must be flagged: for the CI tier `spin` (Yuna's own instructions), `spin-total`
(the total's), `allocate`, `leak`, `memory`, `wake` (at rest), `frame-wake` (a wake and a context
switch a frame, more than twice: one a frame is the least), `round`, `layout`, `motion`; for the
timed tier `spin-draw` (the processor kept busy inside each frame's drawing, which its time
counts) and `start` (a sleep as Yuna starts) ([`canary.rs`](../crates/yuna/src/canary.rs)). No
canary covers the GPU tier's memory yet. No shipped build has the feature; `scripts/check.sh`
lints it.

### The timed tier

[`timed.rs`](../crates/yuna-bench/src/parity/timed.rs) runs its cases one run at a time, never
beside another of the script's, after reading the machine's load for 3 s:

- **Frames**: `partial` (1080p, a 64 × 64 square a frame), `scaled` (4K at scale 1.5, the same
  square) and `full-4k` (4K at scale 1.5, two windows whole each frame), on the manual clock with
  the lockstep client as in the CI tier, but with llvmpipe on its own threads, the agents' case,
  and Yuna logging each frame at debug. Two figures: *a frame's drawing time*, the median of the
  measured frames' `took_us`, which the log's `frame drawn` lines give; and *CPU time a frame*,
  every thread of Yuna's process, llvmpipe's included, from each thread's `schedstat`. Every
  run reads the shaders of a shader cache the whole parity run shares: LLVM compiles the same
  shaders differently from start to start, and about one run in ten then drew 20 % faster with
  40 % less of the processor. On llvmpipe, `full-4k`'s figures are *recorded*: ADR-0139 judges
  the processor tier on its own budget, which no ROADMAP row or ADR sets yet.
- **Starts** ([`start.rs`](../crates/yuna-bench/src/parity/start.rs)): `start-1080p` (1920 ×
  1080 at 60 Hz) and `start-4k` (the seats' default, 3840 × 2160 at 240 Hz and scale 1.5). `yuna
  mcp` starts alone in the run's namespaces, on the real clock, *cold*: a new home, so an empty
  Mesa shader cache. The launcher writes the wall-clock time it starts Yuna to the log; the
  script writes `seat create` at once, and times its reply as it reads it, and the seat's first
  frame by Yuna's first `frame drawn` line on its output. As the owner chose on 2026-11-21, its
  `TMPDIR` depends on what is judged:
  - the *parity* figures, judged over pairs, take a `TMPDIR` of 40,000 files of no Yuna's, as a
    busy `/tmp` holds, so that a start that reads each entry it sweeps shows (`58f2e75` fixed
    one);
  - the *budget* figures of
    [ADR-0100](adr/0100-the-budgets-of-yuna-mcp-at-their-conditions.md) are judged at that ADR's
    conditions, an empty `TMPDIR`: after the pairs, the head alone starts until it has 20 quiet
    starts, and the longest of exactly those 20 is judged against 100 ms to the reply, and 150 ms
    to the first frame on llvmpipe, 100 on the GPU. A disturbed one is replaced until 20 were
    dropped; fewer than 20 quiet ones make the budgets void.
- **The soak** ([`soak.rs`](../crates/yuna-bench/src/parity/soak.rs)), after the pairs: ten `yuna
  mcp` of each side, a seat and a lockstep client each, held at rest on the real clock for 10
  minutes, both sides at once (an instance's anonymous memory is its own); each side's
  `Pss_Anon`, summed, read every 30 s. Its figure is the head's least-squares slope, with
  Student's one-sided bounds on it, against no growth
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item
  25): no worse when the upper bound is below the threshold's floor, in KiB a minute; worse when
  the lower bound is above 0 and the slope above the floor; otherwise inconclusive.

**Pairs.** A pair is a run of the base and a run of the head of one case. One warm-up run of each
build comes first, discarded. Then each case's least pairs, every case's mixed in an order drawn
from the run's seed, and each pair's two runs in an order drawn from it too. Each figure is then
judged by the three verdicts of [`rule.rs`](../crates/yuna-bench/src/parity/rule.rs): over the
quiet pairs, the mean of `ln(head / base)` and Student's one-sided 95 % bounds on it; *no worse*
when the upper bound is below `ln(1 + T)`, *worse* when the lower bound is above 0 and the mean
above `ln(1 + T)`, otherwise more pairs, a batch at a time, up to a cap, then *inconclusive*,
which never passes. Frames take 10 pairs at least and 30 at most; starts 20 and 60. `T` is each
figure's `relative` in the thresholds file, twice the widest standard deviation of `ln(head /
base)` the tree against itself gave, so that the self control passes on its least pairs; the
classes are the file's too, `full-4k`'s recorded and the longest starts' budgets.

**Disturbed runs.** The machine is read each second of each run (`machine.rs`). A run is
*disturbed* when the other programs used more than ADR-0115's limits, 2 cores or 10 % of the
GPU's drawing engines, on average or in any second; or when Yuna's own threads (its main thread
and its `yuna-…` ones) waited on the run queue for more than 5 % of the time they ran
(`schedstat`'s second field): a quiet machine keeps it at 2 to 3 % in a frame's run, a fraction
of a percent in a start. A pair with a disturbed run is dropped and replaced, until as many
were dropped as the cap; a figure left with fewer quiet pairs than its least is *void*. The
other programs' share of the GPU is read only where their DRM files can be: in an agent's
sandbox, which hides them, it is unknown. On llvmpipe the run file (`gpu_load_unread`) and the
report's headline then say so for each case it touched; a case on the GPU, which a game there
would slow unseen, is void, unless `--gpu-load-unknown-ok` says by hand that the GPU was free. A
run that fails voids its case's figures. The frames' runs must do equal work, as in the CI tier.

A head from before the GPU and timed tiers, whose thresholds file has no entry for their kinds,
is judged on them by the script's own file's entries, which the report says. A base from before
them may not let the head loosen them unseen either: for each such kind, the head's entry is
checked against the push before's, else the script's own.

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
