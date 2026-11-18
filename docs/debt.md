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
  [ADR-0076](adr/0076-input-to-present-read-by-mode.md)'s budget, no frame missed, is missed:
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
- A panic of Yuna's state stops it through `_exit`, no destructor run
  ([ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)): `yuna --headless`
  leaves its socket and its lock in `XDG_RUNTIME_DIR`, and its Xauthority folder beside them;
  the hook removes the X displays' locks and file sockets in `/tmp`, which name its pid. The next
  server takes the Wayland socket as stale by its lock, but the Xauthority folder of a socket
  name no later Yuna takes stays. The fatal path of `yuna --headless` has no test, its trigger being an MCP tool's: it was run by hand on Smithay's own
  panic when libEGL is missing.
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
- The bound on a failing renderer's log is tested on `FailedDrawings` alone: no test makes a
  drawing fail (`frames.rs`, `State::draw`), which needs a renderer a test can break.
- On the manual clock, removed outputs' and closed seats' globals wait until the clock is
  stepped 10 s; a test that makes and removes many and never steps keeps them all, disabled.
- A client's capture whose copy fails logs a warning each time (`capture.rs`), at most one a
  refresh for each of the 16 capture sessions: a copy that keeps failing while drawing works
  writes one a frame. A failed drawing fails its captures without a copy, so a lost renderer
  does not reach it. Found at M1.10, beside the failed drawings' bound (`FailedDrawings`).
- After `outputs configure` shrinks an output, the pointer keeps its logical place, as after an
  output's removal: it may sit on no output until it moves
  ([ADR-0090](adr/0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md)).
- No test drives an X11 window that keeps its size across an output's change: its answer is
  taken from its first commit after the size is told, read but not run.

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

## Keyboard layouts

- A change of an agent's layout compiles XKB's files and reads the keymap's keys on the state
  thread, which answers no client meanwhile: 2.8 ms in the middle in release, 5.1 ms at most
  for four layouts with 14 group options, and 11.1 ms in a debug build
  ([measurements](measurements.md#m111--a-keyboard-layouts-change-2026-11-18)). A thread of its
  own would take the compile off, the change then answered once it is done.
- `type` reads the keys again at each call from a state other than the plain one, such as Num
  Lock or Caps Lock on: 0.2 to 0.5 ms more a call in release (measurements). A cache by state
  would keep them.
- `type` never holds a key that reaches a group only while held, such as `grp:switch`'s Right
  Alt: it locks groups alone. Every keymap of the evdev rules has the layout key `<I592>`,
  which locks the next group (`inet(evdev)` in xkeyboard-config 2.49), but X11 never gets that
  key: an X11 program then reads the group from the modifiers alone, which no test checks.

## Captures

- `wlr-screencopy` frames are `wl_shm` alone, their damage the whole frame: a tool that asks a
  dmabuf, or reads the damage to copy less, gets none (ADR-0089).

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
- A capture waiting on an output whose size or scale changes fails, an agent's with a reason, an
  `ext-image-copy-capture-v1` frame as `unknown` rather than `buffer_constraints`, after its
  session is told its new size: the client asks again either way. A frame whose buffer is
  larger than the new size is not refused.
- `png` and `flate2` build two versions of `miniz_oxide`, 0.8 and 0.9, until `png` moves to 0.9.

## The rest auditor

- ADR-0062 asks a command, an MCP tool and a log line: the command is the start option
  `--rest-audit` until Stage 2's `yuna-ipc`, the owner's choice.
- X11 programs are named as their seat's Xwayland, the one Wayland client of them all, and by
  the X11 manager's wakes, by seat: which X11 client woke Yuna is not told.
- A window's first buffer, which maps it, draws as a layout change, not as its client's commit:
  a client that opens windows again and again is named by its wakes and commits, its frames by
  the layout.
- A commit is among a frame's causes when it armed its output's refresh or joined it: one that
  brought nothing new is named too when another cause drew the frame. Damage is not followed by
  client.
- Each client costs Yuna a second descriptor, so a seat's socket runs out of descriptors at about
  half the clients it did. A client whose second descriptor cannot be made is served unnamed,
  counted among the unnamed wakes.
- A client's program is found as a departure's is ([below](#the-mcp-server)), at each answer:
  one that left its program's tree and group has none. `rest`'s programs are those remembered:
  one forgotten in a window, past 128 or with its seat, is not listed, its wakes counted in the
  sources.
- The log of each second runs on real time, a manual clock's too. A Yuna that stops within a
  second does not log it.

## The MCP server

- Claude Code 2.1.318 ran the done-when's story through `yuna mcp` alone
  ([measurements](measurements.md)); Codex and OpenCode are untested, and which revision of MCP
  each client speaks, Claude Code's included, is not recorded.
- A `yuna mcp` killed by SIGKILL leaves its programs running, their Wayland connection closed:
  nothing ends them. Its folder of sockets and logs, up to 160 MiB, stays in `XDG_RUNTIME_DIR`
  until the next `yuna mcp` starts there and removes it, its lock free. So does one that aborts,
  overflows its stack or panics inside its panic hook: no hook runs then, or it cannot finish. A
  panic of its state ends them
  ([ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)). Stopping can take 4 s,
  a second for the last replies and three for the programs, and an MCP client may kill Yuna
  sooner.
- A test of `tests/mcp.rs` that fails drops its `yuna mcp` with SIGKILL, so its programs run
  on: the tests that start a busy program end it before they assert.
- A folder with no lock file, a `yuna mcp`'s older than its lock, is never swept: nothing tells
  whether its Yuna lives. It is removed by hand. A folder whose making a crash interrupted,
  `.yuna-mcp-<pid>-starting`, is removed only by the next Yuna that has that pid.
- An agent reads presentation times only through a capture: its frame's `boundary` and
  `presented_ns`. No tool gives a program's presentation feedback, which the program reads
  itself through `wp_presentation`.
- `windows` lists every window of its seat: past a few hundred, its reply passes Claude Code's
  25,000 tokens of tool output.
- A process that empties its environment and leaves its process group and its parents, a
  double fork with `setsid` and `env -i`, escapes its seat's closing
  ([ADR-0095](adr/0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)); Yuna's stopping ends it. A test pins
  the gap. A cgroup per seat would close it, where systemd delegates one.
- A seat whose agent forgets it keeps its programs until Yuna stops: there is no lease.
- A closed seat's programs are forgotten with their logs: an agent reads a log, through
  `spawn log`, before it closes the seat.
- Once its program has written more than 1.25 MiB, a log's file holds its newest bytes in a
  ring: `spawn log` reads them in order, the file's path does not. `spawn log`'s text is lossy
  UTF-8 cut at byte offsets: a character split at a read's edge shows as replacement
  characters.
- `events` tells windows by comparing what the compositor describes with its last look: a window
  that opens and closes between two looks is not seen, and what changed several times shows its
  last state. A look runs only once the compositor's description generation has moved, so a
  round in which clients only drew costs 0.2 µs; but a look after a change describes every
  window, and `Layout::window` walks the windows of its workspace, so a look costs 0.13 ms at
  100 windows, 4.4 ms at 1,000 and 11 ms at 2,000 in a release build
  ([measurements](measurements.md)): a title changed each round, among 1,000 windows, takes a
  frame's time of 240 Hz. `windows` pays the same. An index of each window's place in the layout
  model would make it linear. Stage 2's one event stream
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md),
  item 14) replaces the comparison.
- An event's title, application id or workspace's name is cut to 512 bytes, at a character's
  boundary: a change past the cut is not told.
- `missed` is exact while a seat's dropped events form at most 1,024 runs of consecutive
  numbers; past that, other seats' events interleaving, the oldest runs merge, and a cursor
  inside a merged run is told at most that run's count.
- A client's program is found from the processes above it when Yuna accepted it, or its process
  group: one that left both, a double fork with `setsid`, is told with no program, its protocol
  error kept on none. A client whose process exited before the accept has no lineage, and no
  program; were its pid taken meanwhile, the lineage read would be the other process's. A pidfd
  of the peer (`SO_PEERPIDFD`) would make it exact.
- Yuna keeps no protocol trace of a client: wayland-rs has none, and a fork for it was not
  taken. `WAYLAND_DEBUG=client` is the client's own, in its log, for clients on libwayland or
  wayland-rs, set before the program starts.
- `slow` paces a program on real time: a manual clock does not, so a slowed program makes a
  manual-clock run depend on the machine's timing. `freeze`, `thaw` and `slow` act on the
  program's process group: a process it moved to a group of its own runs on.
- A frozen program that is sent events is cut off once wayland-rs's 4 KiB buffer for its
  connection and its socket's buffer are full: within one keyboard `type` of 4,096 characters,
  8,192 key events of 24 bytes, in the test that types to such a client on the development
  machine, whose socket buffers set the figure. Its windows close. Stage 2 measures how long a
  client that stops reading should be kept
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)).
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
- `clipboard set` takes 256 KiB of text, a quarter of a request's line (`MAX_LINE`, 1 MiB), so
  that any text but control characters fits it once escaped as JSON; a text of control
  characters, 6 bytes each once escaped, may still pass the line, which ends the connection
  rather than refusing the call.
- `clipboard get` lists a client's first 64 MIME types of 255 bytes at most, those X11 is
  offered: a longer type is not listed, and cannot be named.
- `clipboard get`'s data is held whole until it is answered: up to `max_bytes`, 1 MiB, in a
  buffer that may grow to twice that, for each of the 64 calls that may wait.
- A `clipboard get` of the agent's own text, answered at once, is refused all the same while 64
  calls wait: the bound is checked before Yuna knows who holds the selection, so that no program
  is asked for data in vain.
- A `clipboard get` that the X11 bridge refuses, past its 8 transfers, or whose program closes
  its pipe at once, answers empty data, as a paste would: nothing tells a refusal from an empty
  copy.
- Yuna's writes of an agent's text are bounded per seat, not per client: a client that starts 8
  reads and never drains them holds every other program's paste of it up to the time limit, 5 s.
  An offer a client keeps keeps the text it offered, 256 KiB at most, until the client destroys
  it.

## Accessibility

- AccessKit's programs, egui's among them, are not read: `accesskit_unix` 0.24 turns on only when
  a session bus's `org.a11y.Status` says `IsEnabled`, and a program of Yuna's has no session bus.
  The facts behind M1.11's accessibility, read in the toolkits' sources on 2026-11-17, are not in
  the [survey](survey.md) yet.
- Tested: GTK 4.24 on Wayland, Qt 6.12 on Wayland and on X11 at scale 1.5. Untested: GTK 3,
  Firefox, Chromium and Electron (which need `--force-renderer-accessibility`), Java. Their
  coordinates are given as their toolkit counts them, uncorrected: Chromium counts from its
  surface, its shadow included, and an X11 toolkit that does not scale by `Xft.dpi`, GTK 3 on
  X11, counts in X11 pixels, 1.5 times Yuna's units at scale 1.5.
- A node on a surface of its own, a GTK 4 popover or a menu in a window of its own, is placed as
  its toolkit counts it from its window: untested.
- Yuna registers no event listener, so that programs send nothing: a waiting call walks the trees
  again when a client commits, an application embeds, and every second, rather than when a tree
  changes. A walk makes three calls a node; the ATK bridge's `Collection.GetMatches`, which
  would search Firefox's and Chromium's large trees on their side, is not used.
- Yuna's registry sets no application's `Id` and sends no `ChildrenChanged`: an assistive program
  other than Yuna on a seat's bus would not see applications come and go.
- The daemon wakes once, up to 5 s after a walk's last call, its timer of calls waiting for a
  reply firing to find none ([measurements](measurements.md#m111-accessibility-at-rest-2026-11-18)).
- A Flatpak program's connection is its D-Bus proxy's, whose process is not the program's: its
  nodes would get no window.
- The D-Bus code has property tests on random and broken messages, no fuzz target yet
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md),
  item 22).

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

## The owner's other projects

- `crates/yuna/tests/consumers.rs` loads uwui's `measure.py` with its check that no game runs
  answering none, and puts on the `PATH` a `sway` and a `swaymsg` that fail, which `--display`
  checks for and never runs: a later `measure.py` that runs either in `--display` would fail
  the test, as it should, and need the test changed.
- `ext-data-control-v1` is offered to the owner's seat alone, so `yuna --headless --agent-seat`
  has none: SlimeEngine's check of it runs in `yuna --headless` instead, where no virtual input
  is offered. A test of both together waits for Stage 2's decision on seats
  ([ADR-0078](adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)).
- A virtual keyboard with its own keymap switches its seat's keyboard to it at each of its keys,
  and Yuna's own keys switch it back: an agent's `keyboard` tool and a virtual keyboard used in
  turn send a keymap to the focused client at each change.
- A seat's keyboard has one state for all its devices: a virtual keyboard's Caps Lock makes
  Yuna's own keys capitals while it lives, where wlroots keeps a state for each keyboard. Its
  modifiers are released when it goes. Stage 2's input chain decides the state per device.
- `scripts/test-check.sh` runs by hand, after a change of `check.sh` or before a release, not in
  CI: it takes about as long as the check twice.
- Most rows of [`protocols.md`](protocols.md) not yet offered say `Not decided`; Stage 2's
  opening settles each that an application needs.

## The examples

- `agent_time.rs` keeps its own copies of what `examples/common/` now shares with `frame_time`
  and `motion_time` (its run folder, the vkcube environment, `percentile`); `motion_time` reads
  an output's mode with its own parser beside the program's private one.
- Reading an Xauthority's cookie is written four times, `cookie_of` in
  `tests/applications.rs`, `tests/headless/x11_client.rs`, `tests/headless/xwayland.rs` and
  `examples/xwayland_time.rs`, and decoding base64 twice, `decode_base64` in
  `tests/applications.rs` and `tests/headless/captures.rs`. One copy of each in `tests/common/`
  would serve the tests, and the example could include it with `#[path]`. Paid in the stage's audit.

## XWayland

- An X11 application that does not scale itself (xterm, xeyes, Motif) is small on outputs at
  XWayland's scale: drawing it at one X11 pixel per logical unit needs a per-application rule
  ([ADR-0068](adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)). Wine's programs are
  small the same way unless their prefix's DPI is 96 × S: Wine reads its registry's `LogPixels`,
  not `Xft.dpi` (`wine reg add "HKCU\Control Panel\Desktop" /v LogPixels /t REG_DWORD /d 144`
  at 1.5). Paid at Stage 2, with the configuration's rules per application.
- No drag and drop between X11 and Wayland programs yet: a later task of M1.11; between X11
  programs of one seat it works inside Xwayland
  ([ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md)). The clipboard and the primary
  selection pass both ways ([ARCHITECTURE](ARCHITECTURE.md#between-x11-and-wayland)).
- An `ext-data-control-v1` source destroyed while it is the owner's seat's selection still
  clears it silently: Yuna's patch of Smithay (patch 6) tells of `wl_data_source`s and primary
  sources alone. The X11 selection Yuna holds for it stays until an X11 program asks for it, which
  finds it gone, is refused, and releases it.
- The transfers between X11 and Wayland are bounded per seat, not per program: a Wayland client
  that starts 8 reads of an X11 selection and never drains them holds every other program's
  paste of X11 data up to the time limit, 5 s, at a time; an X11 program that starts 8 requests,
  the same for X11. It is one seat and one user's programs.
- Between X11 and Wayland, the targets `MULTIPLE`, `SAVE_TARGETS` and `DELETE` are refused: an
  X11 clipboard manager cannot keep a Wayland selection after its source goes, and nothing asks
  several targets at once of Yuna yet.
- Text crosses as its bytes, never encoded again: `STRING`, Latin-1 by ICCCM, is served from
  UTF-8 text as it is, an agent's text included, and an X11 reply is passed in whatever type and
  format its owner gave.
- MCP's `clipboard clear` of a selection an X11 program owns empties it on Wayland's side alone:
  the X11 program keeps X11's selection, and the seat's other X11 programs still paste it. The
  manager would have to take X11's selection and release it. A test pins it
  (`an_x11_copy_is_read_through_the_bridge_and_cleared_on_waylands_side_alone`).
- An X11 owner that lost its selection, then exits mid-transfer, ends its transfers only at the
  time limit, 5 s: XFixes tells of a client gone only while it owns the selection.
- The MIME types of Wayland selections are interned as atoms in Xwayland, which never frees one:
  a client of the seat setting selections of ever new types grows Xwayland's table of atoms, 64 a
  selection, until Xwayland ends.
- An X11 requestor window the manager does not follow, given a Wayland selection through INCR,
  keeps the manager's `PropertyChange` mask for its life: its property changes wake the manager,
  which leaves them.
- The transfers' time limit runs on the manager's thread, on real time, not on Yuna's clock: a
  test on the manual clock cannot step it.
- No X11 keyboard grab (`xwayland-keyboard-grab`): a virtual machine's viewer cannot take every
  key, until Stage 2.
- A lead to follow, not known to be a leak: on llvmpipe, Yuna's resident set grows by 37 MiB
  while an Xwayland with one small window runs, and keeps 22 MiB of it once Xwayland has ended
  ([measurements](measurements.md)); on the GPU it grows by less than 1 MiB. Whether that memory
  is in use or freed and kept by the allocator is not known. Looked at in the stage's audit.
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
  in the stage's audit.
- Unmapped X11 windows are tracked without a bound per client: each holds up to about 192 KiB of
  text, three titles of 64 KiB, twice, in its manager and in Yuna; Xwayland holds them too. A
  client can so grow Yuna's memory: a bound per X11 client is to be added in the stage's audit.
- The orders in which a window and its surface pair, the serial before the surface's commit or
  after, around a map or an unmap, are not each forced by a test: the stories take the order
  Xwayland gives. Tests to write in the stage's audit.
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

## Architecture

Today's code against [ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md); Stage 2's core
replaces it, and nothing new is built on it that Stage 2 would undo.

- One `State` of 52 fields, 43 public, implements 22 of Smithay's handler traits: any handler can
  change any state. Smithay's shape; Stage 2's core gives each protocol module its own objects.
- The X11 manager's commands go through an unbounded channel
  (`crates/yuna/src/xwayland/manager.rs`, `mpsc::channel()`); its events are bounded. Bounded at
  Stage 2, or earlier if a client can fill it. The selections' commands are bounded before it:
  a Wayland client's reads of an X11 selection at 8 running (`MAX_X11_TRANSFERS`), each with its
  pipe, and the seat's Wayland selections once a round each.
- A global, `OPENED_DISPLAYS` (`crates/yuna/src/render.rs`), keeps EGL displays alive because
  tests run several compositors in one process; Yuna's Vulkan core needs none.
- wayland-rs bounds each client's outgoing buffer at 4,096 bytes and ends a client whose buffer
  is full, where the rivals raise libwayland's to 1 MiB; how long a client that stops reading
  survives a fast pointer is not measured yet (ADR-0074, chosen at Stage 2's opening).
- No crate forbids `unsafe` code; `yuna` has 8 `unsafe` sites, `yuna-bench` one, `yuna-layout`
  and `yuna-motion` none.
- The process's memory grows by 0.0087 MiB a window, above Hyprland's 0.0063, the budget's figure
  ([ADR-0077](adr/0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md)); its cause
  is not isolated.
- GLES types reach three modules beside `render.rs`: `state.rs` holds the renderer, `output.rs`
  each output's buffer and damage tracker, `dmabuf.rs` the import. Stage 2's Vulkan renderer
  replaces all four.
- Yuna's writes of agents' selections live in the shared `State`, beside Smithay's selection
  state (`Selections` in `crates/yuna/src/selection.rs`: the writes, their next key, those to
  start, and the keys their sources note), since Smithay hands a client's read of a selection
  Yuna holds to its `SelectionHandler`. Stage 2's core gives them to the clipboard's own module.
- Two places say which output shows a window: `scale.rs` reads the last arrangement, `colour/`
  the layout as it is; between a change and the next arrangement they may differ. Stage 2's core
  gives each fact one owner. Found by the [audit of 2026-11-17](audits/2026-11-17-architecture.md).
