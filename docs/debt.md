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
- "Float, then tile" on a workspace of 100 windows misses
  [ADR-0020](adr/0020-the-layout-models-shape.md)'s 25 µs at the 99th percentile in 3 of 5
  runs measured in M1.4's B4, up to 32 µs; its median is 15.7 to 24.2 µs. Every other row met
  its budget on a quiet machine ([measurements](measurements.md)). The cause is not found; the
  operation is to be profiled before M1.7 animates the layout's moves.

## The headless compositor

- At a fractional scale, a client that draws at an integer buffer scale, rather than at its
  output's physical size through fractional scale and a viewport, is scaled when drawn: where its
  window geometry starts inside its surface, its right and bottom edges may blend up to one pixel
  of its margin. It stays while such a client's pixels are scaled anyway
  ([Scale](ARCHITECTURE.md#scale)).
- The lead's floor and margin, 2 ms and 1 ms, are KWin's. On the GPU at 4K and 240 Hz, with
  two `vkcube` on lavapipe, 0 or 1 frame in 2,400 was missed in C5's runs; the lead itself was
  not read there ([measurements](measurements.md)). On llvmpipe it sits at its cap, one period.
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
- Smithay is built without overflow checks, so that a popup positioner's absurd values cannot
  panic a debug build ([ADR-0027](adr/0027-smithay-without-overflow-checks.md)): any overflow
  inside Smithay wraps silently. A saturating patch in the fork replaces the setting when main
  moves to the fork.
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
- With two agents' seats in one Yuna, a program of the second may bind the first's `wl_seat`,
  which upstream Smithay announces to every client: it then gets no input, never another seat's.
  The filter per client comes at Stage 3 ([ADR-0035](adr/0035-agents-seats.md)).
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
- Every seat's `wl_output` and `xdg-output` globals, like its `wl_seat`, are announced to every
  client: a program of one agent sees, and may bind, another agent's outputs, their names,
  modes and places, and a toolkit that lists monitors lists them. No input or focus crosses
  seats by them. The filter per client comes at Stage 3 with `wl_seat`'s
  ([ADR-0035](adr/0035-agents-seats.md)).
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

## Text input

- Smithay `8fd9fe0` sends `enter` to a text field made after its surface took the keyboard only
  when an input method program runs, not when the compositor is the input method
  (`zwp_text_input_manager_v3.get_text_input`): a client that makes its text field late is
  entered at its surface's next keyboard focus. winit makes it at start.
- Smithay `8fd9fe0` drops what an enabled text field tells of itself, its surrounding text, its
  content type and its cursor's rectangle, when no input method program runs: an agent cannot
  read them.
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
  by Stage 3, when agents capture on the desktop. A client's frame copies the whole output once a refresh for all the sessions that
  wait on it, 16 at most.
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
- A `yuna mcp` killed by SIGKILL leaves its programs running, their Wayland connection closed,
  and its folder of logs, up to 128 MiB, in `XDG_RUNTIME_DIR`: nothing ends or removes them.
  Stopping can take 4 s, a second for the last replies and three for the programs, and an MCP
  client may kill Yuna sooner.
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
