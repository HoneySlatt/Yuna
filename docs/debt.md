# Known debt

What is known to fall short, why it stays for now, and when it is paid. Each item is short; the
[PLAN](PLAN.md) links here. Paid items are removed.

## Smithay and the HDR prototype

- The HDR metadata's primaries are sent red, green, blue; that order is not checked against
  CTA-861-G (M1.3's prototype sent them so and the monitor showed HDR; Stage 5 checks the order).
- Smithay `8fd9fe0` stops reading a CRTC's properties at the first of `ACTIVE` and
  `VRR_ENABLED` (`State::current_state`), so one of the two is not read back; it matters once
  Yuna uses VRR (Stage 4).
- Smithay's `DrmDevice`, dropped while active, commits back every property it read at opening,
  immutable ones and `DPMS` among them, which the kernel likely refuses: an exit may not give the
  previous state back. Check it at Stage 2, with a `tracing` subscriber.
- Smithay's connector property patch has debt to pay before main uses it
  ([patches](smithay-patches.md)).

## The layout model

- A dwindle tree deeper than its space gives its last windows empty slots: they are asked 1 × 1
  and not drawn ([ADR-0026](adr/0026-real-clients-in-the-layout.md)).
- "Float, then tile" on a workspace of 100 windows sits close to
  [ADR-0020](adr/0020-the-layout-models-shape.md)'s 25 µs at the 99th percentile: met on a
  quiet machine at M1.7, 19.1 and 23.9 µs, missed under load, up to 32 µs at M1.4 and 25.4 µs
  at M1.7, main's code before M1.7 too ([measurements](measurements.md)). The cause is not
  found: the agents' isolation refuses `perf` its events, so it could not be profiled. The
  stage's audit profiles it, the owner allowing a profiler.

## The headless compositor

- At a fractional scale, a client that draws at an integer buffer scale, rather than at its
  output's physical size through fractional scale and a viewport, is scaled when drawn: where its
  window geometry starts inside its surface, its right and bottom edges may blend up to one pixel
  of its margin. It stays while such a client's pixels are scaled anyway
  ([Scale](ARCHITECTURE.md#scale)).
- The lead's floor and margin, 2 ms and 1 ms, are KWin's. On the GPU at 4K and 240 Hz, with
  two `vkcube` on lavapipe, 0 or 1 frame in 2,400 was missed in C5's runs; the lead itself was
  not read there ([measurements](measurements.md)). On llvmpipe it sits at its cap, one period.
  [ADR-0032](adr/0032-input-to-present-read-by-mode.md)'s budget, no frame missed, is missed:
  0.4 to 0.9 % on llvmpipe at 1080p, 0 or 1 frame in 2,400 on the GPU at 4K.
- `Compositor::advance`, which the tests use, may run virtual time ahead of real time, while
  [ADR-0023](adr/0023-frames-presentation-and-the-clock.md) says never while real clients are
  connected. MCP's `clock` refuses it; in the tests, every client is the test's own.
- `--gpu`, its dmabuf feedback and a GPU's dmabufs were checked once, by hand, on the owner's
  RX 6900 XT ([measurements](measurements.md)); no test covers them: CI has no GPU, and the
  tests never use `--gpu` ([ADR-0022](adr/0022-the-headless-backend-in-yuna.md)). The stories
  draw a client's dmabuf on llvmpipe alone, from a sealed memfd. No real application in CI uses
  `linux-dmabuf`: on llvmpipe, Mesa's clients draw through `wl_shm`
  ([ADR-0029](adr/0029-the-rest-of-animated-applications.md)).
- On llvmpipe, Yuna's default output, 3840 × 2160 at 240 Hz, does not hold its rate when the
  whole output changes at each frame: with two `vkcube` covering it, a frame takes about 7 ms,
  and Yuna draws 116 a second, keeping 8.7 to 8.8 of the desktop's 16 threads busy
  ([measurements](measurements.md)). The bar's budget for an agent's output, full rate while
  its programs draw, is missed there; `--gpu` holds it, as ADR-0022 foresaw.
- On llvmpipe, Smithay logs `Erroneous EGL call didn't set EGLError` once at start (seen since
  M1.4's B1); its cause is not read.
- Every output draws one background frame when it is created, even with no client.
- The development shell's `LD_LIBRARY_PATH` (libglvnd, for EGL) applies to every program run in
  it.
- Without libEGL on its library path, `yuna --headless` panics inside Smithay `8fd9fe0`
  (`src/backend/egl/ffi.rs:154`) rather than reporting the error and exiting.
- A toplevel made again on a surface that kept its old buffer is mapped at its next commit, never
  configured; xdg-shell calls that its client's error, and wlroots refuses it. Yuna should end
  that client with a protocol error.
- A popup is placed when it is created and when it asks again; a reactive popup
  (`set_reactive`) is not placed again when its window moves or resizes. A popup of a window
  shown on no output is placed without constraint.
- Smithay resets a popup's role when it commits without a buffer, its parent included, before
  Yuna sees the commit. Yuna dismisses that popup, but cannot let its client show it again: any
  later commit of it is a protocol error for its client (Smithay `8fd9fe0`,
  `src/wayland/shell/xdg/mod.rs:1974`), which xdg-shell does not ask. Clients destroy their
  popups rather than unmap them.
- `MAX_POPUPS_PER_WINDOW` bounds the popups Yuna tracks on a window, not every `xdg_popup` a
  client keeps: Smithay lists every live one, dismissed or not, and walks that list, all clients
  together, when a popup is destroyed or asks a grab. A client that keeps thousands of dismissed
  popups slows those two requests for every client, until it destroys them.
- Smithay passes on only the activation tokens it keeps: a token used already, or pushed out by
  64 newer ones of its client, is ignored without marking its window urgent.
- Activation tokens of a client that left are kept, as a launcher's must be for the program it
  starts, and nothing bounds them: Yuna's bound of 64 is per client, and Smithay keeps a token
  until it is used. A client that connects again and again, asking 64 tokens each time, grows
  them without end, against [ADR-0028](adr/0028-bounds-on-what-one-client-may-ask.md)'s intent.
  A bound on the tokens of clients gone, the oldest pushed out, would close it; it changes
  ADR-0028, so it waits for the owner's decision.
- Smithay `8fd9fe0` gives a surface the role it has again: a second `xdg_toplevel` or
  `xdg_popup` while the first lives, which xdg-shell forbids. Yuna refuses it itself, with
  `xdg_surface.already_constructed`; for a second `xdg_surface` on the surface, xdg-shell names
  `xdg_wm_base.role`, which Yuna cannot send: Smithay keeps the `xdg_wm_base` to itself.
- The owner's seat has no pointer headless, and takes no input: its clients' tokens and popup
  grabs carry a keyboard `enter` serial alone, and they start no drag. Its input comes with the
  desktop (Stage 2).
- The modal hint of `xdg-dialog-v1` is read but not used: a modal dialog does not yet keep its
  parent from the focus.
- `ext-data-control-v1` is offered to every client of the owner's seat: any of them can read and
  set the clipboard and the primary selection without the focus. It is filtered at Stage 2,
  through security-context or a rule for privileged clients. No agent's client sees it: a
  clipboard manager does not work on an agent's seat.
- Smithay `8fd9fe0` checks that a selection request comes from the client with the keyboard
  focus, not the serial it names: that client may set a selection with any serial, not only in
  answer to its own input. Checking it needs a patch to Smithay; a test pins the gap down
  (`the_serial_of_the_client_with_the_focus_is_not_checked`).
- A drag's icon is dropped, and the cursors clients ask are recorded, not drawn: a capture
  shows neither, and a client asking cursors painted into its frames gets none. Touch has no drag: Yuna has no touch.
- An `accept` that fails on a seat's socket, out of descriptors most often, pauses that socket
  100 ms (`SOCKET_PAUSE`); nothing bounds the clients one seat connects. The owner's socket, in
  `yuna --headless`, is Smithay's, which returns the error and stops Yuna.
- Smithay keeps a record of every seat it made and offers no way to drop one: each seat closed
  leaves its record, without keyboard or pointer, until Yuna stops. An agent that makes and
  closes seats without end grows it.
- A pointer is moved onto what is under it after each arrangement, not when a surface changes
  its input region or a popup opens under it between arrangements.
- Smithay `8fd9fe0` departs from `wl_data_device` twice: an offer stays readable after its client
  loses the focus, and a device created after its client got the focus is told the selection
  only at the next change. Both are Smithay's; neither leaks across seats.

## Colours

- A client may make any number of `color-management-v1` feedback objects: each is a map entry,
  made and dropped in constant time, which Yuna checks after every arrangement. The protocol has no graceful refusal, and wayland-server
  bounds no count of objects; a cap per client would end the client
  ([ADR-0047](adr/0047-color-management-v1-kwins-list.md)).
- An HDR output's captures show its clients' values unconverted: nothing is drawn in its
  colours until Stage 5 ([ADR-0048](adr/0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md)).
- `wp_presentation` is version 2 in wayland-protocols 0.32.13: the `variable_rate` flag of
  version 3, which tells a client its output runs under VRR, cannot be sent yet.

## VRR

What the emulation does not reproduce of the owner's monitor, from its two traces
([ADR-0051](adr/0051-emulated-vrr-as-the-monitors-traces-showed.md)); to take up at Stage 4,
with games' VRR:

- After the panel's own refresh, when no frame came for 1/48 s, the monitor holds a new frame 7
  to 12 ms more; the emulation shows it a vertical blank later, or at the shortest refresh's end.
  About 6 % of the traces' frames.
- After a perturbation, BTR's count of refreshes may differ: at 25 fps, 41 % of the frames are
  counted wrong, 34 % with more refreshes than the monitor shows, the emulation keeping 4 where
  the monitor goes back to 3.
- A change that brings no buffer, a subsurface moved or restacked or a viewport changed alone,
  is paced as if nothing were new: it may show up to a shortest refresh later than the monitor
  would show it.
- `freesync.c`'s workaround that holds the top rate after flips close to a vertical update, and
  its writes of the refresh's range deferred to a worker, are not emulated.
- The kernel's commit path sometimes shows a frame 0.9 ms after its submission rather than 0.5;
  the emulation always takes the vertical blank, 461 µs.

## Text input

- Smithay `8fd9fe0` sends `enter` to a text field made after its surface took the keyboard only
  when an input method program runs, not when the compositor is the input method
  (`zwp_text_input_manager_v3.get_text_input`): a client that makes its text field late is
  entered at its surface's next keyboard focus. winit makes it at start.
- Smithay `8fd9fe0` drops what an enabled text field tells of itself, its surrounding text, its
  cursor's rectangle and the content type it changes later, when no input method program runs.
  The content type it is enabled with reaches `TextInputActivation::activated`, which Yuna
  ignores: an agent reads none of them.
- Smithay `8fd9fe0` keeps every text field a client makes, unbounded, in its seat's list, and
  walks the list at each `commit` and each object destroyed: a client that makes n fields and
  leaves costs about n² steps on the main thread (`text_input_handle.rs`). Not measured; it
  matters when agents' seats are in the owner's Yuna (Stage 3), and ADR-0028 bounds the like.
- The owner's seat has no input method: a real one, `input-method-v2`, comes with the desktop.

## Captures

- Smithay `8fd9fe0` asks a session's buffer constraints (`capture_constraints`) without naming
  its client: a client of another seat is told that output's size and formats before Yuna stops
  its session. It learns no more than that output's `wl_output` tells it already.
- Smithay `8fd9fe0` ignores what `ext-image-copy-capture-v1` calls protocol errors: a buffer
  attached or damaged after `capture`, a damage of no area, a second `capture`; a `capture`
  without a buffer fails the frame rather than end the client with `no_buffer`. Fixing it needs
  a patch to Smithay.
- Smithay `8fd9fe0`'s `Frame::success` sends no damage for `None`, which its documentation calls
  full damage: Yuna gives the whole buffer as damage on every frame. No frame tells less, so a
  recorder copies every pixel each time.
- A capture's copy holds the main thread while the renderer reads its pixels back, at the
  refresh: 4 ms at 4K on the GPU, up to 39 ms; 47 ms on llvmpipe. An agent that captures without
  pause costs its output 6 % of its frames on the GPU, 37 % on llvmpipe at 4K
  ([measurements](measurements.md)): the full rate of an agent's output is missed meanwhile, and
  on the desktop a capture would cost a game frames. A readback into a buffer the GPU fills
  while the loop goes on, read at a later refresh, would free it: to be planned with the owner,
  by Stage 3, when agents capture on the desktop. A client's frame copies the whole output once
  a refresh for all the sessions that wait on it, 16 at most.
- An agent's capture is refused past 8,000 pixels a side once its pixels are read, not when it
  is asked: one readback wasted, one capture of a seat at a time.
- On a manual clock, a capture copied has no time limit until the clock reaches its frame's
  boundary: the agent advances it.
- An agent's capture of a 4K output takes 68 to 79 ms to encode in a release build, on its own
  thread, and about 1.2 s in a debug build, as the tests run; a logical image, its pixels
  averaged first, takes longer than a physical one.
- Every client of `yuna --headless` may capture its seat's outputs without being asked; consent
  comes with the desktop ([ADR-0038](adr/0038-captures-coordinates-windows-and-wayland.md)).
- A capture waiting for its frame times out after 50 s; no test waits that long.
- `png` and `flate2` build two versions of `miniz_oxide`, 0.8 and 0.9, until `png` moves to 0.9.

## The MCP server

- Claude Code 2.1.318 ran the done-when's story through `yuna mcp` alone
  ([measurements](measurements.md)); Codex and OpenCode are untested, and which revision of MCP
  each client speaks, Claude Code's included, is not recorded.
- A `yuna mcp` killed by SIGKILL, or one that panics, a bug unwinding past its stopping, leaves
  its programs running, their Wayland connection closed, and its folder of logs, up to 128 MiB,
  in `XDG_RUNTIME_DIR`: nothing ends or removes them.
  Stopping can take 4 s, a second for the last replies and three for the programs, and an MCP
  client may kill Yuna sooner.
- An agent reads presentation times only through a capture: its frame's `boundary` and
  `presented_ns`. No tool gives a program's presentation feedback, which the program reads
  itself through `wp_presentation`.
- `windows` lists every window of its seat: past a few hundred, its reply passes Claude Code's
  25,000 tokens of tool output.
- A process that empties its environment and leaves its process group and its parents, a
  double fork with `setsid` and `env -i`, escapes its seat's closing
  ([ADR-0037](adr/0037-seats-tools-input-and-closing.md)); Yuna's stopping ends it. A test pins
  the gap. A cgroup per seat would close it, where systemd delegates one.
- A seat whose agent forgets it keeps its programs until Yuna stops: there is no lease.
- A closed seat's programs are forgotten with their logs: an agent reads a log before it
  closes the seat.
- 64 programs run at most, counted by their first process: one that exits leaving its children
  frees its place.
- When Yuna stops or a seat closes, the processes are found through `/proc`, then signalled by
  pid: a process that ends and is reaped by its own parent between the two frees a pid that
  another process may take. The window is short; pidfds would close it. A seat's closing ends
  the process groups its ended programs led, as an orphan stays in its group; a process of
  another seat in such a group, a pid reused, is spared by its `YUNA_SEAT` or its own program,
  not one that emptied its environment.
- The programs' logs and runtime folders are deleted when Yuna stops: an agent reads them
  before it closes Yuna's input.

## Motion

- While a special workspace fades out, `Motion::update` asks the layout where each of its
  windows is, a search of the seat's workspaces each: the cost grows with the square of the
  windows for that time. A lookup by window in `yuna-layout` would make it linear.
- A workspace that leaves its output while a switch shows it (an empty one the layout removes,
  one moved to another output) closes up the switch: the workspace beside it shows at once. So
  does leaving, with dynamic workspaces, a workspace whose last window just closed: it is gone
  before the switch starts, and the one shown appears without sliding. Keeping a gone workspace
  as an empty slot until the switch ends would let it slide.
- A second special workspace shown while the first fades out replaces it at once.
- On llvmpipe at 1080p, motion misses more than half its frames, against 1.9 % still: Yuna's
  main thread takes 2.7 ms a frame in motion against 1.0 ms, and clients in view draw on the
  CPU beside llvmpipe ([measurements](measurements.md)). The cause is not isolated; the GPU at
  4K misses none.
- A special workspace replaced by another one vanishes at once rather than fading out, and the
  new one fades in from 0: a fade holds one special workspace.
- A motion wholly outside an output (a column off the view widening) keeps that output
  refreshing until it ends, drawing nothing new, about 80 refreshes at 240 Hz.
- `SpringParams::time_to_rest` and `Motion::set_settings` have no caller but tests: the 1.5 s
  refusal of [ADR-0053](adr/0053-springs-in-closed-form-keeping-their-velocity.md) and settings
  changed live come with the configuration file (Stage 2).
- A failed drawing of a motion's last frame is drawn again at the next boundary; no test covers
  it, since a drawing cannot be made to fail headless.
- At each arrangement and each motion's end, every window not told visible is looked up in the
  layout, a search of its seat each: the cost grows with the square of the windows. Reading
  fullscreen states once per arrangement would make it linear.
- `ten_thousand_columns` runs 10,000 windows through an animated update but times nothing: a
  quadratic search coming back would fail no check; `examples/motion_timing.rs` shows it.
- On a VRR output, a motion that starts after a pause shows its second frame about 12 ms after
  its first, amdgpu's insertion of refreshes after a pause, as `vrr.rs` emulates it; then every
  shortest refresh. The desktop runs at a fixed rate ([ADR-0014](adr/0014-vrr-without-flicker.md)):
  it matters for motion on agents' VRR outputs only.

## The examples

- `agent_time.rs` keeps its own copies of what `examples/common/` now shares with `frame_time`
  and `motion_time` (its run folder, the vkcube environment, `percentile`); `motion_time` reads
  an output's mode with its own parser beside the program's private one.
- Reading an Xauthority's cookie is written four times, `cookie_of` in
  `tests/applications.rs`, `tests/headless/x11_client.rs`, `tests/headless/xwayland.rs` and
  `examples/xwayland_time.rs`, and decoding base64 twice, `decode_base64` in
  `tests/applications.rs` and `tests/headless/captures.rs`. One copy of each in `tests/common/`
  would serve the tests, and the example could include it with `#[path]`. Paid in M1.12's audit.

## XWayland

- An X11 application that does not scale itself (xterm, xeyes, Motif) is small on outputs at
  XWayland's scale: drawing it at one X11 pixel per logical unit needs a per-application rule
  ([ADR-0068](adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)). Wine's programs are
  small the same way unless their prefix's DPI is 96 × S: Wine reads its registry's `LogPixels`,
  not `Xft.dpi` (`wine reg add "HKCU\Control Panel\Desktop" /v LogPixels /t REG_DWORD /d 144`
  at 1.5). Paid at Stage 2, with the configuration's rules per application.
- No clipboard or drag and drop between X11 and Wayland programs until M1.11; between X11
  programs of one seat they work inside Xwayland
  ([ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md)).
- No X11 keyboard grab (`xwayland-keyboard-grab`): a virtual machine's viewer cannot take every
  key, until Stage 2.
- In `yuna mcp`, the programs' reaping (`wait` on any child) may take an Xwayland's status before
  its pidfd does: a crash after `WM_S0` then counts as a clean end, and starts no pause. A crash
  loop in `yuna mcp` restarts Xwayland at each new X11 connection. Paid at M1.11, which reworks
  what `yuna mcp` sees of its programs.
- A lead to follow, not known to be a leak: on llvmpipe, Yuna's resident set grows by 37 MiB
  while an Xwayland with one small window runs, and keeps 22 MiB of it once Xwayland has ended
  ([measurements](measurements.md)); on the GPU it grows by less than 1 MiB. Whether that memory
  is in use or freed and kept by the allocator is not known. Looked at in M1.12's audit.
- On llvmpipe, Xwayland runs without GLX: Mesa's GLX for Xwayland loads the machine's own drivers
  and opens a display node (`/dev/dri/card*`), which agents never take. X11 programs drawing
  through GLX (Wine's OpenGL among them) need `--gpu`, where Xwayland opened only the render
  node (checked on 2026-11-14 with the cards hidden, `glxinfo` on the GPU). Stays for headless
  agents on llvmpipe; the desktop at Stage 2 owns its display device and keeps GLX.
- An X11 menu drawn with a window keeps its offset from that window as it was when the menu was
  placed, so that it slides with the window. If the window moves while the menu stays open, X11
  still has the menu at its own place: a click on the window where X11 has the menu goes to the
  root, as Xwayland picks the window under the pointer through X11's stacking. Toolkits close
  their menus when their window moves. Paid at Stage 2, with interactive moves.
- A menu over a fullscreen game in an emulated mode is placed by its offset in X11 pixels, not
  scaled by the viewport Xwayland gives the game's surface. Paid at Stage 4, with games.
- A menu drawn with no window enters no output (`wl_surface.enter`): Xwayland's surfaces have one
  scale, which outputs do not change. Kept: nothing reads it for X11 yet; revisited at Stage 2.
- A menu drawn with its window is drawn only on the outputs its window overlaps: one overflowing
  onto the next output is cut there, as an xdg popup is. Paid at Stage 2, with xdg popups'
  (Yuna's own scene, ADR-0039).
- No reparenting (`xwayland/manager.rs`): Yuna draws no frames, so a window stays a child of the
  root. Java's AWT waits for a reparenting manager unless it runs with
  `_JAVA_AWT_WM_NONREPARENTING=1`; its windows otherwise stay blank. Decided with the desktop at
  Stage 2: Yuna setting it for its programs, or reparenting.
- An X11 window's size hints give its limits alone: its increments and base size (a terminal's
  cells) and its aspect are read and not followed, as tiling managers do; its window type gives
  only whether it is managed: a dialog, a splash or a utility window opens tiled unless it names
  a parent through `WM_TRANSIENT_FOR`. Paid at Stage 2, with window rules.
- `_NET_WM_SYNC_REQUEST` uses the basic counter alone: the extended one, which tells each frame
  drawn, is left. A window that never answers waits 100 ms at each resize, where KWin stops
  syncing it after a timeout. Paid at Stage 4, where games' frames are paced.
- A ping carries `CurrentTime`, not the time of the event that asked to close, and its answer is
  matched by its window alone, so that any X11 client of the seat can answer another's ping and
  clear its "unresponsive"; one ping waits at a time. Kept: X11 isolates nothing within a seat
  (ADR-0067); the timestamp is paid with `WM_TAKE_FOCUS`'s below.
- `_NET_STARTUP_ID` is read when a window asks to be mapped, not when it changes later; X11's
  startup notification messages (`_NET_STARTUP_INFO`) are not read. X11 keeps its clients apart
  in nothing: another X11 client of the seat can read a window's startup id and use its token
  first. Seats stay apart: a token activates its own seat's windows alone. Startup messages are
  read at Stage 2, with the launcher.
- `WM_TAKE_FOCUS` is sent with `CurrentTime`, not the time of the event that moved the focus, as
  ICCCM asks: a client that checks it may refuse the focus. Paid at Stage 2, with real input
  timestamps from libinput.
- `_NET_WM_MOVERESIZE` is taken and does nothing: Yuna has no interactive move. A request to
  iconify or hide (`WM_CHANGE_STATE`, `_NET_WM_STATE_HIDDEN`) does nothing: Yuna minimises
  nothing. Paid at Stage 2, with interactive moves and minimising, if the owner wants them.
- A managed window typed a menu, a notification or a tooltip, not override-redirect, is managed
  as any window: tiled, unless it names a parent. Paid at Stage 2, with window rules.
- An X11 window's minimum size is honoured up to X11's 32,767 pixels: a window asking that much
  is configured to it, Xwayland draws no buffer so large, and it never shows. A client can make
  such a window itself, without hints; Yuna does not bound X11 windows to its outputs. Looked at
  in M1.12's audit.
- Unmapped X11 windows are tracked without a bound per client: each holds up to about 192 KiB of
  text, three titles of 64 KiB, twice, in its manager and in Yuna; Xwayland holds them too. A
  client can so grow Yuna's memory: a bound per X11 client is to be added in M1.12's audit.
- The orders in which a window and its surface pair, the serial before the surface's commit or
  after, around a map or an unmap, are not each forced by a test: the stories take the order
  Xwayland gives. Tests to write in M1.12's audit.
- Telling the windows at each arrangement asks the layout for each window shown
  (`Layout::window`), whose lookup walks the workspaces, and an arrangement walks every window:
  it is part of a flood's longest round
  ([measured](measurements.md#m18--a-flood-of-x11-windows-and-menus-2026-11-15)). Paid at the opening
  of Stage 2, when Yuna's core moves and the layout's lookups with it.
- An X11 client's `_NET_ACTIVE_WINDOW` is taken as xdg-activation without a token: the window
  takes the focus only when its client has it already; a pager's request (source 2) is no better.
  Revisited at Stage 2, with Hollow's taskbar.
- An X11 window's place is cut to X11's 16-bit coordinates: with outputs past 32,767 X11 pixels
  from the root's origin, many outputs side by side at a large scale, a window there is told a
  place its pointer and Xwayland's emulated modes miss. An output whose logical position is not a
  whole unit is told to Xwayland rounded, so that its windows' X11 places may be a pixel off its
  own. Kept: no output layout so large is planned.
- Commands to a manager go at the end of the round that made them; the X11 focus and a window's
  place reach Xwayland on its manager's connection, the keyboard's `enter` on its Wayland one, in
  no set order between them. Kept: no client found that depends on the order.
- The area filter reads each texel 1/1024 past its centre, which holds for buffers up to about
  8,000 texels a side (`area_filter.frag`): past it, a sampler's rounding may take a texel's
  neighbour, a slight blur of one X11 window wider than 8,000 pixels below XWayland's scale.
  Revisited with the Vulkan renderer (ADR-0063), which draws the filter again.
- A `KeymapNotify` sent by an X11 client (`SendEvent`) stalls its seat's manager: x11rb 0.14
  knows that event has no sequence number only without the sent bit (`buffer[0] ==
  KEYMAP_NOTIFY_EVENT`, `x11rb-protocol` `connection/mod.rs`), so it reads two bytes the client
  chose as one, loses its place among the replies, and the manager waits for ever. Yuna's loop
  and the other seats go on; the seat's X11 windows no longer open. To pay before Stage 2: a
  report to x11rb on the owner's go, or a wrapper of the manager's stream that clears the bit.
- The X11 element declares no opaque region: the damage tracker draws what lies under an opaque
  X11 window too, the background and the windows below it, where a Wayland surface's opaque
  region would let it skip them; a whole frame costs up to that overdraw more on each output.
  Paid with Yuna's own scene at the opening of Stage 2.
