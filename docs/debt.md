# Known debt

What is known to fall short, why it stays for now, and when it is paid. Each item is short; the
[PLAN](PLAN.md) links here. Paid items are removed. An item kept on purpose says "Kept:" and why.

An item about Smithay's own behaviour goes with Smithay at Stage 2a's gate, where Yuna's own core
replaces it ([ADR-0139](adr/0139-stage-2-in-three-gated-parts-2as-path-revised.md)). Each
of Smithay's own behaviours becomes a test in the milestone that replaces its module, so that the
replacement does not repeat it.
The owner placed every other item on 2026-11-19: in a milestone of Stage 2a
([ADR-0139](adr/0139-stage-2-in-three-gated-parts-2as-path-revised.md)), in Stage 2b, 2c or a
later stage, or as "Kept" ([audit](audits/2026-11-18-stage-1.md)). Each item, or its section's
head, names its place.

Smithay's facts here were read at upstream `8fd9fe0`, but where an item names `7ff0ede4`; the pin
moved to upstream `7ff0ede4` at M1.13 ([patches](smithay-patches.md)), and the others were not
read again.

## Smithay and the HDR prototype

- The HDR metadata's primaries are sent red, green, blue; that order is not checked against
  CTA-861-G (M1.3's prototype sent them so and the monitor showed HDR; Stage 5 checks the order).
- Smithay `8fd9fe0` stops reading a CRTC's properties at the first of `ACTIVE` and
  `VRR_ENABLED` (`State::current_state`), so one of the two is not read back. Yuna builds no
  Smithay DRM backend: its own, on drm-rs, comes at Stage 2b.
- Smithay's `DrmDevice`, dropped while active, commits back every property it read at opening,
  immutable ones and `DPMS` among them, which the kernel likely refuses: an exit may not give the
  previous state back.
- Smithay's connector property patch has debt to pay before main uses it
  ([patches](smithay-patches.md)).
- Smithay's `create_dmabuf` posts `invalid_dimensions` for a width or a height under 1, then goes
  on: with a height of 0, a plane's offset and stride added past `u32::MAX` panic in a debug
  build (`src/wayland/dmabuf/mod.rs`, `plane.offset + plane.stride`), which stops Yuna as any
  panic of its state, and wrap in a release build, where EGL then refuses the buffer. Seen with
  a test client, in a test not kept since it panics; its fix is a patch to the fork, not made
  yet.

## The layout model

- A dwindle tree deeper than its space gives its last windows empty slots: they are asked 1 × 1
  and not drawn ([ADR-0020](adr/0020-the-layout-models-shape.md) left it to M1.4).
  Paid at Stage 2c: such a window floats, as Hyprland's does
  ([ADR-0141](adr/0141-a-dwindle-window-that-does-not-fit-floats.md)).
- "Float, then tile" on a workspace of 100 windows sits close to
  [ADR-0111](adr/0111-the-layout-models-shape-plain-ids-a-branch-per-seat-asked-and-committed-sizes.md)'s
  25 µs at the 99th percentile: met on a quiet machine at M1.7, 19.1 and 23.9 µs, missed under
  load, up to 32 µs at M1.4 and 25.4 µs at M1.7, main's code before M1.7 too
  ([measurements](measurements.md)). Met again at M1.13, 24.13 µs; callgrind gives 22.8 % of its
  instructions to hashing `WindowId`s with SipHash. Paid at Stage 2a, M2.5 (the boundary), with
  the layout's index below and a faster hasher, measured.
- `Layout::window` finds a window by walking its seat's workspaces, so that a caller that asks it
  for each window costs the square of the windows: `Motion::update` while a special workspace
  fades out; each arrangement, which tells the windows shown and, at a motion's end too, looks up
  those not told visible, part of a flood's longest round
  ([measured](measurements.md#m18--a-flood-of-x11-windows-and-menus-2026-11-15));
  `Compositor::check_windows`; MCP's `windows` and `events` (below). An index of each window's
  place would make them linear. Paid at Stage 2a, M2.5 (the boundary), when Yuna's core moves and
  the layout's lookups with it.

## The headless compositor

- At a fractional scale, a client that draws at an integer buffer scale, rather than at its
  output's physical size through fractional scale and a viewport, is scaled when drawn: where its
  window geometry starts inside its surface, its right and bottom edges may blend up to one pixel
  of its margin ([Scale](ARCHITECTURE.md#scale)). Kept: such a client's pixels are scaled anyway.
- The lead's floor and margin, 2 ms and 1 ms, are KWin's. On the GPU at 4K and 240 Hz, with
  two `vkcube` on lavapipe, 0 or 1 frame in 2,400 was missed in C5's runs; the lead itself was
  not read there ([measurements](measurements.md)). On llvmpipe it sits at its cap, one period.
  [ADR-0076](adr/0076-input-to-present-read-by-mode.md)'s budget, no frame missed, is missed:
  0.4 to 0.9 % on llvmpipe at 1080p, 0 or 1 frame in 2,400 on the GPU at 4K. Paid at Stage 2a, M2.4
  (the map, tasks, frames and events), with the frame scheduler; its budget is checked on KMS at
  Stage 2b.
- `Compositor::advance`, which the tests use, may run virtual time ahead of real time, while
  [ADR-0118](adr/0118-frames-presentation-times-and-the-clock.md) says never while real clients
  are connected. Kept: MCP's `clock` refuses it, and in the tests every client is the test's own.
- `--gpu`, its dmabuf feedback and a GPU's dmabufs were checked once, by hand, on the owner's
  RX 6900 XT ([measurements](measurements.md)); no test covers them: CI has no GPU, and the
  tests never use `--gpu`
  ([ADR-0123](adr/0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md)). On
  llvmpipe, a GPU client's dmabufs, Mesa's Vulkan's linear copies outside its video memory
  (`dmabuf.rs`), were checked once too, by hand: `gtk4-demo` on that GPU, through `yuna mcp`. How
  fast llvmpipe reads them is not measured: radv puts them in system memory the GPU writes
  combined, which the processor reads uncached. The stories check the feedback that leads such a
  client there, draw a client's dmabuf on llvmpipe from a sealed memfd, and refuse files llvmpipe
  cannot map and strides shorter than a row. No real application in CI uses `linux-dmabuf`: on
  llvmpipe, Mesa's clients on the processor draw through `wl_shm`. Paid at Stage 2a, M2.8 (the
  Vulkan renderer, behind a switch): an agent's script compares RADV and lavapipe at each renderer
  milestone.
- On llvmpipe, a GPU client that is not Mesa's may share a buffer in video memory the processor
  cannot map: the `dev_t` 0 of the feedback steers only clients that compare it with their GPU,
  as Mesa does, which the protocol tells them not to rely on
  ([ADR-0123](adr/0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md)).
  Yuna refuses such a buffer, which ends that client, all its windows with it, if it used
  `create_immed`. Chromium, by its source, binds version 4, logs an error for device 0, and uses
  `create_immed`; what Chromium, Firefox, VA-API's video decoders and NVIDIA's driver send is not
  verified. Paid at Stage 2a, M2.15 (the other protocols), with Yuna's own `linux-dmabuf`, under
  [ADR-0131](adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)'s rule.
- On llvmpipe, `linux-dmabuf` offers no YUV format: a program that shares decoded video as YUV
  dmabufs must draw another way there. Offering them needs each plane's layout checked, as the
  one plane of an RGB format's is. Goes with Smithay's GLES renderer at Stage 2a, M2.9 (Vulkan the
  default), whose Vulkan renderer draws on lavapipe headless
  ([ADR-0106](adr/0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md)).
- On llvmpipe, Yuna's checks of each dmabuf cost, per plane, two maps and unmaps of its file and
  a few seeks, at its creation; not measured. Goes with Smithay's GLES renderer, at Stage 2a, M2.9
  (Vulkan the default).
- Mesa 26.2's llvmpipe never frees the buffer of a plane's size it allocates for a file it
  cannot map (`dri_sw_displaytarget_from_handle`). Yuna refuses such a file before the import;
  only a client that seals its file against writing while Mesa imports it reaches that path,
  and Yuna then refuses the buffer, but the size leaks each time. Goes with Smithay's GLES
  renderer, at Stage 2a, M2.9 (Vulkan the default).
- On llvmpipe, Yuna's default output, 3840 × 2160 at 240 Hz, does not hold its rate when the
  whole output changes at each frame: with two `vkcube` covering it, a frame takes about 7 ms,
  and Yuna draws 116 a second, keeping 8.7 to 8.8 of the desktop's 16 threads busy
  ([measurements](measurements.md)). The bar's budget for an agent's output, full rate while
  its programs draw, is missed there; `--gpu` holds it, as ADR-0123 foresaw. Stage 2a's renderer
  on the processor, lavapipe, holds a budget of its own
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
  item 26). Paid at Stage 2a, M2.9 (Vulkan the default), the processor tier's own budget
  ([ADR-0139](adr/0139-stage-2-in-three-gated-parts-2as-path-revised.md)).
- On llvmpipe, Smithay logs `Erroneous EGL call didn't set EGLError` once at start (seen since
  M1.4's B1); its cause is not read.
- Every output draws one background frame when it is created, even with no client. Taken up
  with Yuna's own scene and damage tracking at Stage 2a, M2.7 (the scene, alone).
- A panic of Yuna's state stops it through `_exit`, no destructor run
  ([ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)): `yuna --headless`
  leaves its socket and its lock in `XDG_RUNTIME_DIR`, and its Xauthority folder beside them;
  the hook removes the X displays' locks and file sockets in `/tmp`, which name its pid. The next
  server takes the Wayland socket as stale by its lock, but the Xauthority folder of a socket
  name no later Yuna takes stays. The fatal path of `yuna --headless` has no test, its trigger
  being an MCP tool's: it was run by hand on Smithay's own panic when libEGL is missing. Taken up
  with the keeper that restarts Yuna, at Stage 2b.
- The development shell's `LD_LIBRARY_PATH` (libglvnd, for EGL) applies to every program run in
  it. Paid at Stage 2a, M2.9 (Vulkan the default), when EGL leaves and the Vulkan loader is
  linked.
- Without libEGL on its library path, `yuna --headless` panics inside Smithay `8fd9fe0`
  (`src/backend/egl/ffi.rs:154`) rather than reporting the error and exiting.
- A toplevel made again on a surface that kept its old buffer is mapped at its next commit, never
  configured; xdg-shell calls that its client's error, and wlroots refuses it. Yuna should end
  that client with a protocol error. Taken up with Yuna's own xdg-shell at Stage 2a, M2.13
  (xdg-shell and popups).
- A popup is placed when it is created and when it asks again; a reactive popup
  (`set_reactive`) is not placed again when its window moves or resizes. A popup of a window
  shown on no output is placed without constraint. Taken up with Yuna's own popups at Stage 2a,
  M2.13 (xdg-shell and popups).
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
- Smithay `8fd9fe0` gives a surface the role it has again: a second `xdg_toplevel` or
  `xdg_popup` while the first lives, which xdg-shell forbids. Yuna refuses it itself, with
  `xdg_surface.already_constructed`; for a second `xdg_surface` on the surface, xdg-shell names
  `xdg_wm_base.role`, which Yuna cannot send: Smithay keeps the `xdg_wm_base` to itself.
- The owner's seat has no pointer headless, and takes no input: its clients' tokens and popup
  grabs carry a keyboard `enter` serial alone, and they start no drag. Its input comes with the
  desktop's libinput, at Stage 2b.
- The modal hint of `xdg-dialog-v1` is read but not used: a modal dialog does not yet keep its
  parent from the focus. Paid at Stage 2c.
- `ext-data-control-v1` is offered to every client of the owner's seat: any of them can read and
  set the clipboard and the primary selection without the focus. It is filtered per client at Stage
  2a, M2.14 (selections and text), through security-context or a rule for privileged clients, with
  one seat per Yuna ([ADR-0138](adr/0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md)). No
  agent's client sees it: a clipboard manager does not work on an agent's seat.
- Smithay `8fd9fe0` checks that a selection request comes from the client with the keyboard
  focus, not the serial it names: that client may set a selection with any serial, not only in
  answer to its own input. Checking it needs a patch to Smithay; a test pins the gap down
  (`the_serial_of_the_client_with_the_focus_is_not_checked`).
- A Wayland drag's icon (`wl_drag_icon`) is dropped, and the cursors clients ask are recorded,
  not drawn: a capture shows neither, and a client asking cursors painted into its frames gets
  none. An X11 drag's icon is a menu, drawn. Touch has no drag: Yuna has no touch. Taken up with
  Yuna's own scene at Stage 2a, M2.7 (the scene, alone).
- A client's shared-memory pools and the descriptors they hold, its buffers' memory, a window's
  subsurfaces and a client's windows not yet mapped are unbounded: their protocols refuse only by
  ending the client. The owner kept them so on 2026-11-18, until Stage 2's core and its queue of
  pending commits bound them
  ([ADR-0131](adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)). Nothing bounds the
  clients one seat connects either. Yuna raises its limit of open files at start, and an
  `accept` that fails on a seat's socket, the owner's in `yuna --headless` too, pauses that socket
  100 ms (`SOCKET_PAUSE`) rather than stop Yuna. Paid at Stage 2a: the pools, buffers, subsurfaces
  and unmapped windows are modelled at M2.6 (the queue and the store, alone) and bounded when wired,
  at M2.16 (the store, wired behind a switch); the clients one seat connects, at M2.2 (the interface), with the
  1 MiB of [ADR-0136](adr/0136-a-client-that-stops-reading-1-mib-and-its-motion-merged.md).
- One commit of a window whose subsurfaces are synchronised in a deep chain costs Smithay a linear
  search of its transaction at each level (`transaction.rs:119`, `tree.rs:267` and `:277`, at
  `7ff0ede4`), about
  the cube of the chain's depth: such a commit took 0.14 to 0.30 s for a chain of 1,000 in
  release, 1.0 to 1.7 s for 2,000, on a disturbed machine, most of it Smithay's
  ([measurements](measurements.md)). Yuna's own share grows with the depth alone.
- Yuna reads a window's tree once per commit, at the call for the surface committed
  (`state::Committing`), which holds while each transaction applies within the commit that made
  it. Revisited with the queue of pending commits, whose blockers end that, at Stage 2a, M2.6 (the
  queue and the store, alone)
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item
  1).
- Each time the hidden windows' deadline fires, `send_hidden_frames` walks every toplevel, not
  only the hidden ones. Paid at Stage 2a, M2.4 (the map, tasks, frames and events), with the frame
  scheduler.
- Smithay keeps a record of every seat it made and offers no way to drop one: each seat closed
  leaves its record, without keyboard or pointer, until Yuna stops. An agent that makes and
  closes seats without end grows it.
- A pointer is moved onto what is under it after each arrangement, not when a surface changes
  its input region or a popup opens under it between arrangements. Taken up with Yuna's own seats
  and input at Stage 2a, M2.12 (seats and input).
- Smithay `8fd9fe0` departs from `wl_data_device` twice: an offer stays readable after its client
  loses the focus, and a device created after its client got the focus is told the selection
  only at the next change. Both are Smithay's; neither leaks across seats.
- The bound on a failing renderer's log is tested on `FailedDrawings` alone: no test makes a
  drawing fail (`frames.rs`, `State::draw`), which needs a renderer a test can break. Stage 2a,
  M2.10 (captures and the GPU's loss), loses the GPU on demand
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
  item 22).
- On the manual clock, removed outputs' and closed seats' globals wait until the clock is
  stepped 10 s. Kept: only a test that makes and removes many and never steps the clock keeps
  them all, disabled.
- A client's capture whose read fails warns at the first refresh of its output's run of
  failures, then logs at the debug level until a refresh reads every client's capture of that
  output (`FailedCopies`, `capture.rs`): runs are kept by output, not by capture session, so a
  failing region read beside the whole output read alone at other refreshes begins a run, and a
  warning, each time. No test makes a read fail. Stage 2a, M2.10 (captures and the GPU's loss),
  loses the GPU on demand (ADR-0082 item 22).
- After `outputs configure` shrinks an output, the pointer keeps its logical place, as after an
  output's removal: it may sit on no output until it moves
  ([ADR-0090](adr/0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md)). Taken up with
  Yuna's own seats and input at Stage 2a, M2.12 (seats and input).
- No test drives an X11 window that keeps its size across an output's change: its answer is
  taken from its first commit after the size is told, read but not run. Paid at Stage 2a, M2.1
  (baselines): a test before the rewrite.

## Colours

- A client may make any number of `color-management-v1` feedback objects: each is a map entry,
  made and dropped in constant time, which Yuna checks after every arrangement. Kept: the
  protocol has no graceful refusal, wayland-server bounds no count of objects, and a cap per
  client would end the client ([ADR-0047](adr/0047-color-management-v1-kwins-list.md)).
- An HDR output's captures show its clients' values unconverted: nothing is drawn in its
  colours until Stage 5 ([ADR-0128](adr/0128-hdr-virtual-outputs-and-when-colour-is-chosen.md)).
- `wp_presentation` is version 2 in wayland-protocols 0.32.13: the `variable_rate` flag of
  version 3, which tells a client its output runs under VRR, cannot be sent yet. Paid at Stage 4,
  with games' VRR, once wayland-protocols has version 3.

## VRR

What the emulation does not reproduce of the owner's monitor, from its two traces
([ADR-0119](adr/0119-emulated-vrr-as-amdgpu-and-the-owners-monitor-do.md)); to take up at Stage
4, with games' VRR:

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
  leaves costs about n² steps on the main thread (`text_input_handle.rs`). Not measured; Yuna's
  own text-input at Stage 2a, M2.14 (selections and text), is bounded as ADR-0131 bounds the like.
- The owner's seat has no input method: a real one, `input-method-v2`, comes at Stage 2c.

## Keyboard layouts

- A change of an agent's layout compiles XKB's files and reads the keymap's keys on the state
  thread, which answers no client meanwhile: 2.8 ms in the middle in release, 5.1 ms at most
  for four layouts with 14 group options, and 11.1 ms in a debug build
  ([measurements](measurements.md#m111--a-keyboard-layouts-change-2026-11-18)). A thread of its
  own would take the compile off, the change then answered once it is done. Paid at Stage 2a, M2.12
  (seats and input), by a worker task.
- `type` reads the keys again at each call from a state other than the plain one, such as Num
  Lock or Caps Lock on: 0.2 to 0.5 ms more a call in release (measurements). A cache by state
  would keep them. Paid at Stage 2a, M2.12 (seats and input), by a cache by state.
- `type` never holds a key that reaches a group only while held, such as `grp:switch`'s Right
  Alt: it locks groups alone. Every keymap of the evdev rules has the layout key `<I592>`,
  which locks the next group (`inet(evdev)` in xkeyboard-config 2.49), but X11 never gets that
  key: an X11 program then reads the group from the modifiers alone, which no test checks. Paid at
  Stage 2a, M2.12 (seats and input), with a test.

## Captures

- `wlr-screencopy` frames are `wl_shm` alone, their damage the whole frame: a tool that asks a
  dmabuf, or reads the damage to copy less, gets none
  ([ADR-0121](adr/0121-captures-what-is-seen-at-the-logical-size.md)). Paid at Stage 2a, M2.10
  (captures and the GPU's loss), which rewrites the protocol.
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
  while the loop goes on, read at a later refresh, would free it. Paid at Stage 2a, M2.10 (captures
  and the GPU's loss), by Yuna's Vulkan renderer, whose acceptance includes it (ROADMAP), the owner chose on
  2026-11-18; the same measure compares the two. A client's frame copies the whole output once a
  refresh for all the sessions that wait on it, 16 at most.
- An agent's capture is refused past 8,000 pixels a side once its pixels are read, not when it
  is asked: one readback wasted, one capture of a seat at a time. Paid at Stage 2a, M2.10, with the
  readback off the loop.
- On a manual clock, a capture copied has no time limit until the clock reaches its frame's
  boundary. Kept: the agent advances it.
- An agent's capture of a 4K output takes 68 to 79 ms to encode in a release build, on its own
  thread, and about 1.2 s in a debug build, as the tests run; a logical image, its pixels
  averaged first, takes longer than a physical one. Kept: it runs on its own thread and costs no
  frame.
- Every client of `yuna --headless` may capture its seat's outputs without being asked; consent
  comes with the share picker at Stage 2c
  ([ADR-0121](adr/0121-captures-what-is-seen-at-the-logical-size.md)).
- A capture waiting for its frame times out after 50 s; no test waits that long. Paid at Stage 2a,
  M2.1 (baselines): a test before the rewrite.
- A capture waiting on an output whose size or scale changes fails, an agent's with a reason, an
  `ext-image-copy-capture-v1` frame as `unknown` rather than `buffer_constraints`, after its
  session is told its new size. Kept: the client asks again either way. A frame whose buffer is
  larger than the new size is not refused.
- `png` and `flate2` build two versions of `miniz_oxide`, 0.8 and 0.9, until `png` moves to 0.9:
  looked at again when the crates are raised, at each stage's end.

## The rest auditor

- The rest auditor's command is the start option `--rest-audit` until Hollow's interface,
  `yuna-ipc`, at Stage 2c, as the owner chose
  ([ADR-0096](adr/0096-a-rest-auditor-naming-what-wakes-yuna-or-makes-it-draw.md)).
- X11 programs are named as their seat's Xwayland, the one Wayland client of them all, and by
  the X11 manager's wakes, by seat: which X11 client woke Yuna is not told. Paid at Stage 2a, M2.4
  (the map, tasks, frames and events), with the frame's identity
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item
  11).
- A window's first buffer, which maps it, draws as a layout change, not as its client's commit:
  a client that opens windows again and again is named by its wakes and commits, its frames by
  the layout. Paid at Stage 2a, M2.4, with the frame's identity.
- A commit is among a frame's causes when it armed its output's refresh or joined it: one that
  brought nothing new is named too when another cause drew the frame. Damage is not followed by
  client. Paid at Stage 2a, M2.4, with the frame's identity.
- Each client costs Yuna a second descriptor, so a seat's socket runs out of descriptors at about
  half the clients it did. A client whose second descriptor cannot be made is served unnamed,
  counted among the unnamed wakes. Kept: the limit is raised at start; the pidfd that Stage 2's core
  records for each client
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item
  15) adds one descriptor and removes none.
- A client's program is found as a departure's is ([below](#the-mcp-server)), at each answer:
  one that left its program's tree and group has none. `rest`'s programs are those remembered:
  one forgotten in a window, past 128 or with its seat, is not listed, its wakes counted in the
  sources. Paid at Stage 2a, M2.4 (the map, tasks, frames and events), by a pidfd taken at accept.
- The log of each second runs on real time, a manual clock's too. A Yuna that stops within a
  second does not log it. Kept: it is a log for people; `rest` reads the counters.

## The MCP server

- Claude Code 2.1.318 ran the done-when's story through `yuna mcp` alone
  ([measurements](measurements.md)); Codex and OpenCode are untested, and which revision of MCP
  each client speaks, Claude Code's included, is not recorded. Paid at Stage 2a, M2.18 (the agents'
  runtime).
- A `yuna mcp` killed by SIGKILL leaves its programs running, their Wayland connection closed:
  nothing ends them. Its folder of sockets and logs, up to 160 MiB, stays in `XDG_RUNTIME_DIR`
  until the next `yuna mcp` starts there and removes it, its lock free; its seats' homes, bounded
  by the disk alone, stay in `TMPDIR` until a `yuna mcp` starts with the same `TMPDIR`. So does
  one that aborts, overflows its stack or panics inside its panic hook: no hook runs then, or it
  cannot finish. A panic of its state ends them
  ([ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)). Stopping can take 4 s,
  a second for the last replies and three for the programs, and an MCP client may kill Yuna
  sooner. Paid at Stage 2a, M2.18 (the agents' runtime), by a PID namespace or pidfds.
- A test of `tests/mcp.rs` that fails drops its `yuna mcp` with SIGKILL, so its programs run
  on: the tests that start a busy program end it before they assert. Paid at Stage 2a, M2.18 (the
  agents' runtime).
- A folder with no lock file, a `yuna mcp`'s older than its lock, is never swept: nothing tells
  whether its Yuna lives. It is removed by hand. So is a folder whose making a crash interrupted,
  between its `mkdir` and its rename: `.yuna-mcp-<pid>-<n>-starting` or
  `.yuna-mcp-homes-<pid>-<n>-starting`, `n` random, is never swept. Paid at Stage 2a, M2.18 (the
  agents' runtime).
- A homes' folder a program made again, its `mkdir -p "$HOME/.cache"` after a cleaner of the
  temporary folder removed Yuna's, is left behind: Yuna makes its own under another name, and
  neither its stop nor the next start removes the other. Paid at Stage 2a, M2.18 (the agents'
  runtime).
- The programs' and homes' folders take a random number from `/dev/urandom`, read in
  `mcp/programs/folder.rs` as `xwayland/display.rs` reads it for a cookie: a sandbox without
  `/dev/urandom` stops `yuna mcp` at its start. Paid at Stage 2a, M2.18 (the agents' runtime), with
  `getrandom`.
- An agent reads presentation times only through a capture: its frame's `boundary` and
  `presented_ns`. No tool gives a program's presentation feedback, which the program reads
  itself through `wp_presentation`. Paid at Stage 2a, M2.4 (the map, tasks, frames and events), when
  the frame's identity is exposed.
- `windows` lists every window of its seat: past a few hundred, its reply passes Claude Code's
  25,000 tokens of tool output. Paid at Stage 2a, M2.4, by the event stream, paged.
- A process that empties its environment and leaves its process group and its parents, a
  double fork with `setsid` and `env -i`, escapes its seat's closing
  ([ADR-0117](adr/0117-an-agents-seat-through-mcp-tools-input-keyboard-text-and-closing.md));
  Yuna's stopping ends it. A test pins the gap. A cgroup per seat would close it, where systemd
  delegates one. Paid at Stage 2a, M2.18 (the agents' runtime).
- A seat whose agent forgets it keeps its programs until Yuna stops: there is no lease. Kept: each
  agent's `yuna mcp` ends with its MCP client.
- A closed seat's programs are forgotten with their logs: an agent reads a log, through
  `spawn log`, before it closes the seat. Kept: the contract is written.
- Once its program has written more than 1.25 MiB, a log's file holds its newest bytes in a
  ring: `spawn log` reads them in order, the file's path does not. `spawn log`'s text is lossy
  UTF-8 cut at byte offsets: a character split at a read's edge shows as replacement
  characters. 1 MiB of control characters, a terminal's colour codes for one, makes a reply of
  about 13 MiB, each escaped as six bytes and given twice. Paid at Stage 2a, M2.18 (the agents'
  runtime), when replies are bounded in bytes.
- `events` tells windows by comparing what the compositor describes with its last look: a window
  that opens and closes between two looks is not seen, and what changed several times shows its
  last state. A look runs only once the compositor's description generation has moved, so a
  round in which clients only drew costs 0.2 µs; but a look after a change describes every
  window, through `Layout::window` ([above](#the-layout-model)): 0.13 ms at 100 windows, 4.4 ms
  at 1,000 and 11 ms at 2,000 in a release build ([measurements](measurements.md)). A title
  changed each round, among 1,000 windows, takes a frame's time of 240 Hz. `windows` pays the
  same. Paid at Stage 2a, M2.4 (the map, tasks, frames and events): the one event stream
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md),
  item 14) replaces the comparison.
- An event's title, application id or workspace's name is cut to 512 bytes, at a character's
  boundary: a change past the cut is not told. Kept: an event holds 1 KiB of text at most, a
  seat's log about 1 MiB (`MAX_EVENT_TEXT`).
- `missed` is exact while a seat's dropped events form at most 1,024 runs of consecutive
  numbers; past that, other seats' events interleaving, the oldest runs merge, and a cursor
  inside a merged run is told at most that run's count. Paid at Stage 2a, M2.4, with the event
  stream's resync event.
- A client's program is found from the processes above it when Yuna accepted it, or its process
  group: one that left both, a double fork with `setsid`, is told with no program, its protocol
  error kept on none. A client whose process exited before the accept has no lineage, and no
  program; were its pid taken meanwhile, the lineage read would be the other process's. Paid at
  Stage 2a, M2.4 (the map, tasks, frames and events): the core records a pidfd of the peer when a
  client connects, which makes it exact
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item
  15).
- Yuna keeps no protocol trace of a client: wayland-rs has none, and a fork for it was not
  taken. `WAYLAND_DEBUG=client` is the client's own, in its log, for clients on libwayland or
  wayland-rs, set before the program starts. Kept: the client's own trace is enough; Stage 2a's parity
  checks compare clients' traces.
- `slow` paces a program on real time: a manual clock does not, so a slowed program makes a
  manual-clock run depend on the machine's timing. `freeze`, `thaw` and `slow` act on the
  program's process group: a process it moved to a group of its own runs on. Paid at Stage 2a, M2.18
  (the agents' runtime).
- A frozen program that is sent events is cut off once wayland-rs's 4 KiB buffer for its
  connection and its socket's buffer are full: within one keyboard `type` of 4,096 characters,
  8,192 key events of 24 bytes, in the test that types to such a client on the development
  machine, whose socket buffers set the figure. Its windows close.
  [ADR-0136](adr/0136-a-client-that-stops-reading-1-mib-and-its-motion-merged.md) decides it: 1 MiB,
  at Stage 2a, M2.2 (the interface), and the pointer's motion merged, at M2.12 (seats and input).
- 64 programs run at most, counted by their first process: one that exits leaving its children
  frees its place. Paid at Stage 2a, M2.18 (the agents' runtime).
- When Yuna stops or a seat closes, the processes are found through `/proc`, then signalled by
  pid: a process that ends and is reaped by its own parent between the two frees a pid that
  another process may take. The window is short; pidfds would close it. A seat's closing ends
  the process groups its ended programs led, as an orphan stays in its group; a process of
  another seat in such a group, a pid reused, is spared by its `YUNA_SEAT` or its own program,
  not one that emptied its environment. Paid at Stage 2a, M2.18 (the agents' runtime).
- The programs' logs and runtime folders are deleted when Yuna stops: an agent reads them
  before it closes Yuna's input. Kept: they are throwaway by design.
- The answers of calls that waited, held while 16 MiB of replies wait, are bounded in number, not
  in bytes: 64 clipboard reads and 16 captures hold about 280 MB at worst (`Server::held`). Paid at
  Stage 2a, M2.18 (the agents' runtime), when replies are bounded in bytes.
- `clipboard set` takes 256 KiB of text, a quarter of a request's line (`MAX_LINE`, 1 MiB), so
  that any text but control characters fits it once escaped as JSON; a text of control
  characters, 6 bytes each once escaped, may still pass the line, which ends the connection
  rather than refusing the call. Kept: the 256 KiB is
  [ADR-0101](adr/0101-an-agents-clipboard-through-mcp.md)'s decision.
- `clipboard get` lists a client's first 64 MIME types of 255 bytes at most, those X11 is
  offered: a longer type is not listed, and cannot be named. Kept: these are
  [ADR-0091](adr/0091-the-clipboard-between-x11-and-wayland-who-reads-and-its-bounds.md)'s bounds.
- `clipboard get`'s data is held whole until it is answered: up to `max_bytes`, 1 MiB, in a
  buffer that may grow to twice that, for each of the 64 calls that may wait. Paid at Stage 2a,
  M2.18 (the agents' runtime), when replies are bounded in bytes.
- A `clipboard get` of the agent's own text, answered at once, is refused all the same while 64
  calls wait: the bound is checked before Yuna knows who holds the selection, so that no program
  is asked for data in vain. Kept: the bound is checked before any program is asked.
- A `clipboard get` that the X11 bridge refuses, past its 8 transfers, or whose program closes
  its pipe at once, answers empty data, as a paste would: nothing tells a refusal from an empty
  copy. Paid at Stage 2a, M2.14 (selections and text): the X11 bridge moves with the selections.
- Yuna's writes of an agent's text, and the transfers between X11 and Wayland, are bounded per
  seat, not per program: 8 at once, each ended after 5 s without progress. A Wayland client that
  starts 8 reads and never drains them holds every other program's paste of the agent's text, or
  of X11 data, up to that limit, at a time; an X11 program that starts 8 requests, the same for
  X11. It is one seat and one user's programs. An offer a client keeps keeps the text it offered,
  256 KiB at most, until the client destroys it. Paid at Stage 2a, M2.15 (the other protocols), with
  per-client budgets
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item
  15).

## Accessibility

- AccessKit's programs, egui's among them, are not read: `accesskit_unix` 0.24 turns on only when
  a session bus's `org.a11y.Status` says `IsEnabled`, and a program of Yuna's has no session bus
  ([survey](survey.md#the-agents-toolbox)). Paid at Stage 2a, M2.18 (the agents' runtime), when a
  seat's bus tells that accessibility is on.
- Tested: GTK 4.24 on Wayland, Qt 6.12 on Wayland and on X11 at scale 1.5. Untested: GTK 3,
  Firefox, Chromium and Electron (which need `--force-renderer-accessibility`), Java. Their
  coordinates are given as their toolkit counts them, uncorrected: Chromium counts from its
  surface, its shadow included, and an X11 toolkit that does not scale by `Xft.dpi`, GTK 3 on
  X11, counts in X11 pixels, 1.5 times Yuna's units at scale 1.5. Paid at Stage 2a, M2.18: until
  then, an agent clicks the wrong place in the owner's Brave and VS Code.
- A node on a surface of its own, a GTK 4 popover or a menu in a window of its own, is placed as
  its toolkit counts it from its window: untested. Kept: tested when an owner's program needs it.
- Yuna registers no event listener, so that programs send nothing: a waiting call walks the trees
  again when a client commits, an application embeds, and every second, rather than when a tree
  changes. A walk makes three calls a node; the ATK bridge's `Collection.GetMatches`, which
  would search Firefox's and Chromium's large trees on their side, is not used. Kept: programs send
  nothing while none listens.
- Yuna's registry sets no application's `Id` and sends no `ChildrenChanged`: an assistive program
  other than Yuna on a seat's bus would not see applications come and go. Kept: no other assistive
  program runs on agents' buses.
- The daemon wakes once, up to 5 s after a walk's last call, its timer of calls waiting for a
  reply firing to find none ([measurements](measurements.md#m111-accessibility-at-rest-2026-11-18)).
  Paid at Stage 2a, M2.18: at rest nothing should wake.
- A Flatpak program's connection is its D-Bus proxy's, whose process is not the program's: its
  nodes would get no window. Kept: no agent runs a Flatpak program.
- The D-Bus code has property tests on random and broken messages, no fuzz target yet
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md),
  item 22). Paid at Stage 2a, M2.14 (selections and text).

## Motion

- Leaving an unnamed workspace whose last window closed shows the workspace focused without
  sliding: the layout removes the empty one left in the same operation, so no switch starts from
  it. Paid at Stage 2c, with motion's polish.
- A fade holds one special workspace: a second shown while the first fades out replaces it at
  once, the first vanishing rather than fading out, and the new one fades in from 0. Paid at Stage
  2c, with motion's polish.
- On llvmpipe at 1080p, motion misses more than half its frames, against 1.9 % still: Yuna's
  main thread takes 2.7 ms a frame in motion against 1.0 ms, and clients in view draw on the
  CPU beside llvmpipe ([measurements](measurements.md)). The cause is not isolated; the GPU at
  4K misses none. Stage 2a's renderer on the processor, lavapipe, holds a budget of its own
  (ADR-0082 item 26). Paid at Stage 2a, M2.9 (Vulkan the default), the processor tier.
- A motion wholly outside an output (a column off the view widening) keeps that output
  refreshing until it ends, drawing nothing new, about 80 refreshes at 240 Hz. Paid at Stage 2a,
  M2.9 (Vulkan the default): the scene draws nothing, so nothing refreshes.
- `SpringParams::time_to_rest` and `Motion::set_settings` have no caller but tests: the 1.5 s
  refusal of [ADR-0053](adr/0053-springs-in-closed-form-keeping-their-velocity.md) comes with the
  configuration file at Stage 2b, settings changed live at Stage 2c.
- A failed drawing of a motion's last frame is drawn again at the next boundary; no test covers
  it, since a drawing cannot be made to fail headless. Stage 2a, M2.10 (captures and the GPU's
  loss), loses the GPU on demand (ADR-0082 item 22).
- `ten_thousand_columns` runs 10,000 windows through an animated update but times nothing: a
  quadratic search coming back would fail no check; `examples/motion_timing.rs` shows it. Paid at
  Stage 2a, M2.1 (baselines): a guard before the rewrite.
- On a VRR output, a motion that starts after a pause shows its second frame about 12 ms after
  its first, amdgpu's insertion of refreshes after a pause, as `vrr.rs` emulates it; then every
  shortest refresh. Kept: it is what amdgpu does, and the desktop runs at a fixed rate
  ([ADR-0014](adr/0014-vrr-without-flicker.md)), so it matters for motion on agents' VRR outputs
  only.

## The owner's other projects

- `crates/yuna/tests/consumers.rs` loads uwui's `measure.py` with its check that no game runs
  answering none, and puts on the `PATH` a `sway` and a `swaymsg` that fail, which `--display`
  checks for and never runs. Kept: a later `measure.py` that runs either in `--display` fails the
  test, as it should, and needs the test changed.
- `ext-data-control-v1` is offered to the owner's seat alone, so `yuna --headless --agent-seat`
  has none: SlimeEngine's check of it runs in `yuna --headless` instead, where no virtual input
  is offered. A test of both together waits for Stage 2a, M2.14 (selections and text):
  [ADR-0138](adr/0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md) decided one seat per
  Yuna.
- A virtual keyboard with its own keymap switches its seat's keyboard to it at each of its keys,
  and Yuna's own keys switch it back: an agent's `keyboard` tool and a virtual keyboard used in
  turn send a keymap to the focused client at each change. A seat's keyboard has one state for
  all its devices: a virtual keyboard's Caps Lock makes Yuna's own keys capitals while it lives,
  where wlroots keeps a state for each keyboard. Its modifiers are released when it goes. Paid at
  Stage 2a, M2.12 (seats and input): the input chain decides the state per device
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item
  10).
- `scripts/test-check.sh` runs by hand, after a change of `check.sh` or before a release, not in
  CI. Kept: it takes about as long as the check twice.
- Opium's weekly job builds Opium from its Nix package uncached: 29 minutes on GitHub's runner
  at its first run, 2026-11-19, and the test's step 12 minutes, its build included. It turns off the AppArmor rule of the runner's
  Ubuntu that keeps an unprivileged user from a network namespace, on that throwaway machine
  alone. Opium, built from its own nixpkgs, loads the development shell's Mesa, which holds while
  that Mesa needs no newer glibc than Opium's nixpkgs gives (`tests/opium.rs`). Kept: it is weekly,
  off the push's path.

## Tests and examples

- `yuna mcp`'s start missed two budgets on 2026-11-19, the machine quiet but the session's page
  cache unknown: its first frame on the GPU at 1080p, 110.3 ms against 100 (58.7 at M1.11), and
  its `seat create` on llvmpipe at 4K, 128.8 ms against 100 ([measurements](measurements.md)).
  Its start at 4K on the GPU was measured only on a disturbed machine, 126 ms at the longest
  against a budget of 100. The laptop and the lowest machine are not measured; no budget holds with programs open
  ([ADR-0100](adr/0100-the-budgets-of-yuna-mcp-at-their-conditions.md)). Paid at Stage 2a, M2.1
  (baselines): M1.11's commit and main measured side by side the same day, quiet, then fixed or
  put to the owner.
- Motion's budget was not judged at M1.13's release: the machine never gave the still baseline its
  rate. Judged again on a quiet machine with the measures above. Paid at Stage 2a, M2.1 (baselines).
- Yuna's own code allocates on every frame: 83 allocations and 404 KiB on a still frame, 91 and 390
  KiB in motion, none at rest, measured by heaptrack on llvmpipe at 1080p; half are vectors rebuilt
  in the elements' path ([ADR-0140](adr/0140-rust-1-100s-features-what-yuna-takes.md)). The target
  is none on a still or a moving frame, checked by heaptrack in the parity script. Paid at Stage 2a,
  M2.9 (Vulkan the default).
- `tests/consumers.rs` builds Pluto's `pluto-input` once into `target/consumers/`, and Cargo does
  not build it again when the development shell's libraries change: after a raise of `nixpkgs`, a
  build kept from before drove no key into Yuna until removed. CI builds afresh. To remove at the
  next change of the consumers' tests. Paid at Stage 2a, M2.2 (the interface).
- Mutation testing (Stage 1's audit) leaves motion's mutants that a comparison over random
  sequences of changes tells apart, but that no scenario test pins with an independent oracle:
  `Camera::velocity`, `Camera::follow`'s `focused_column`, `remember_focus`, `follow_cameras`,
  two `unmoved` operators and `draws` (`yuna-motion/src/arrangement/output.rs`). The others that
  survive are equivalent or unreachable, each argued in the run's report. Paid at Stage 2a, M2.1
  (baselines): tests before the rewrite.
- `motion_time` reads an output's mode with its own parser beside the program's private one. Paid at
  Stage 2a, M2.2 (the interface), the helpers made one.
- `short_temp_dir` is written twice, in `tests/mcp.rs` and `tests/opium.rs`, and `tests/mcp.rs`
  keeps a context-switch reader of its own beside `tests/common/rest.rs`'s; a test folder a
  failed run left is used again in `tests/mcp.rs`. Paid at Stage 2a, M2.2, the helpers made one.

## XWayland

- An X11 application that does not scale itself (xterm, xeyes, Motif) is small on outputs at
  XWayland's scale: drawing it at one X11 pixel per logical unit needs a per-application rule
  ([ADR-0124](adr/0124-x11-windows-across-outputs-of-different-scales.md)). Wine's programs are
  small the same way unless their prefix's DPI is 96 × S: Wine reads its registry's `LogPixels`,
  not `Xft.dpi` (`wine reg add "HKCU\Control Panel\Desktop" /v LogPixels /t REG_DWORD /d 144`
  at 1.5). Paid at Stage 2c, with the rules per application.
- Drags between X11 and Wayland programs
  ([ADR-0102](adr/0102-drag-and-drop-between-x11-and-wayland-a-drag-session-beside-smithays-grabs.md))
  were run with one real toolkit, Qt, on X11 and on Wayland. GTK 3 and 4, Chromium and Electron,
  Java and Wine were not: their sources are read, not run. Chromium's X11 source takes as its
  target only a window with a `WM_NAME` and `XdndAware` 5 or more (its `window_cache.cc` and
  `x11_drag_drop_client.cc`, read on 2026-11-18): the proxy window has both. Kept: checked when one
  of them is first needed.
- Some of the drags' rules are tested on the drag session and the XDND messages alone, not by a
  story with Xwayland: a source naming a version outside 3 to 5, a point outside X11's 16 bits, a
  moving or centred fullscreen window's point, Ask, Link, a third drag, and the one position kept
  waiting. Paid at Stage 2a, M2.1 (baselines): stories before the rewrite.
- Three races of an X11 drag are left, Yuna's loop never waiting for the manager's thread; the
  owner kept them, the second on 2026-11-18 and all three on 2026-11-19, against holding the
  release until the manager confirms
  ([drag_session.rs](../crates/yuna/src/xwayland/drag_session.rs)):
  - released onto a Wayland window in the round of its last motion, the source may look for its
    target before the manager has mapped the proxy, find the X11 window under the pointer,
    hidden or covered, and drop there: about 1 in 12 at rest with the tests' source, likely more
    with real toolkits. Such a drop is refused, never given to the Wayland window;
  - released onto an X11 window in the round it came over it, the source may still find the proxy,
    and the drop is lost: Yuna repeats the motion once the proxy is hidden, but the release
    reached the source first (1 drop in 20 lost before that repeat, none in 80 after, at M1.12's
    closing review);
  - a press that reaches Xwayland before the manager has unmapped the proxy, in the round of the
    motion over an X11 window that hid it, goes to the proxy: MCP's `click` given a place does
    that.

  A hand gives the manager the time; MCP's `pointer` given a place moves and presses or releases
  in one round. A passive grab on the proxy that replays the press was tried and not finished.
  Kept: the owner's choice; looked at again at Stage 2a, M2.12 (seats and input), with Yuna's own
  grabs.
- After an X11 drag dropped on a Wayland window, as after any press, motion and release, the
  pointer's focus stays on the pressed window until the next motion: Smithay's grab of a held
  button does not look again at its end. A click without a motion then reaches the pressed
  window, not the one under the pointer. Yuna looks again after a Wayland drag's grab only. MCP's
  answer to the release that ends a Wayland drop on an X11 window names no window, being made
  before the round's end. Paid at Stage 2a, M2.12 (seats and input), with Yuna's own grabs.
- An X11 source that changes its proposed action mid-drag, a modifier key pressed, has the new
  actions offered to its Wayland target, but Smithay chooses the drag's action again only when
  the target sets its own actions again. An X11 source's `XdndDrop` that reaches the manager
  after Yuna gave the drag up, 5 s after the release, gets no `XdndFinished`: the source waits
  for its own time limit. The missing `XdndFinished` is paid at Stage 2a, M2.12 (seats and input),
  with Yuna's own drags.
- A Wayland drag released over an X11 window relies on the order in which Smithay's drag grab
  ends its source and then calls its handler; a story holds it.
- An `ext-data-control-v1` source destroyed while it is the owner's seat's selection still
  clears it silently: Yuna's patch of Smithay (patch 6) tells of `wl_data_source`s and primary
  sources alone. The X11 selection Yuna holds for it stays until an X11 program asks for it, which
  finds it gone, is refused, and releases it. Paid at Stage 2a, M2.14 (selections and text), with
  Yuna's own data control.
- Between X11 and Wayland, the targets `MULTIPLE`, `SAVE_TARGETS` and `DELETE` are refused: an
  X11 clipboard manager cannot keep a Wayland selection after its source goes, and nothing asks
  several targets at once of Yuna yet. Kept: nothing asks them.
- Text crosses as its bytes, never encoded again: `STRING`, Latin-1 by ICCCM, is served from
  UTF-8 text as it is, an agent's text included, and an X11 reply is passed in whatever type and
  format its owner gave. Paid at Stage 2a, M2.14 (selections and text): `STRING` served as UTF-8
  garbles accents for a program that asks `STRING` alone.
- MCP's `clipboard clear` of a selection an X11 program owns empties it on Wayland's side alone:
  the X11 program keeps X11's selection, and the seat's other X11 programs still paste it. The
  manager would have to take X11's selection and release it. A test pins it
  (`an_x11_copy_is_read_through_the_bridge_and_cleared_on_waylands_side_alone`). Paid at Stage 2a,
  M2.14 (selections and text).
- An X11 owner that lost its selection, then exits mid-transfer, ends its transfers only at the
  time limit, 5 s: XFixes tells of a client gone only while it owns the selection. Kept: XFixes
  tells no more.
- The MIME types of Wayland selections are interned as atoms in Xwayland, which never frees one:
  a client of the seat setting selections of ever new types grows Xwayland's table of atoms, 64 a
  selection, until Xwayland ends. Paid at Stage 2a, M2.14 (selections and text): on the desktop
  Xwayland lives a session, so the table needs a bound.
- An X11 requestor window the manager does not follow, given a Wayland selection through INCR,
  keeps the manager's `PropertyChange` mask for its life: its property changes wake the manager,
  which leaves them. Kept: the wakes are harmless.
- The transfers' time limit runs on the manager's thread, on real time, not on Yuna's clock: a
  test on the manual clock cannot step it. Paid at Stage 2a, M2.4 (the map, tasks, frames and
  events): tasks run inline on the manual clock.
- No X11 keyboard grab (`xwayland-keyboard-grab`): a virtual machine's viewer cannot take every
  key, until Stage 2c, with the keyboard shortcuts' inhibitor
  ([ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md), [ROADMAP](ROADMAP.md)).
- On llvmpipe, Yuna's resident set grows by 37 to 50 MiB while an Xwayland with one small window
  runs, and keeps about 20 MiB once it has ended; on the GPU, less than 1 MiB. Not live data:
  heaptrack finds 30 KB more live heap. It is likely, by its size, the freed llvmpipe texture of
  the X11 window's buffer, whose pages glibc keeps; why glibc keeps them when told to trim at every
  free is open ([measurements](measurements.md#stage-2s-opening-design-2026-11-19)). Paid at Stage
  2a, M2.9 (Vulkan the default), when that path leaves with GLES.
- On llvmpipe, Xwayland runs without GLX: Mesa's GLX for Xwayland loads the machine's own drivers
  and opens a display node (`/dev/dri/card*`), which agents never take. X11 programs drawing
  through GLX (Wine's OpenGL among them) need `--gpu`, where Xwayland opened only the render
  node (checked on 2026-11-14 with the cards hidden, `glxinfo` on the GPU). Kept for headless
  agents on llvmpipe; the desktop at Stage 2b owns its display device and keeps GLX.
- An X11 menu drawn with a window keeps its offset from that window as it was when the menu was
  placed, so that it slides with the window. If the window moves while the menu stays open, X11
  still has the menu at its own place: a click on the window where X11 has the menu goes to the
  root, as Xwayland picks the window under the pointer through X11's stacking. Toolkits close
  their menus when their window moves. Paid at Stage 2c, with interactive moves.
- A menu over a fullscreen game in an emulated mode is placed by its offset in X11 pixels, not
  scaled by the viewport Xwayland gives the game's surface. Paid at Stage 4, with games.
- A menu drawn with no window enters no output (`wl_surface.enter`): Xwayland's surfaces have one
  scale, which outputs do not change. Kept: nothing reads it for X11 yet; revisited at Stage 2a,
  M2.7 (the scene, alone).
- A menu drawn with its window is drawn only on the outputs its window overlaps: one overflowing
  onto the next output is cut there, as an xdg popup is. Paid at Stage 2a, M2.13 (xdg-shell and
  popups), with xdg popups', in Yuna's own scene.
- A menu past its own client's 64 waits for its next map or commit; one past its Xwayland's 256
  shows once another of that Xwayland goes, lowest in X11's stacking first: one program can delay
  another's menus, never for good
  ([ADR-0131](adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)). Kept: it is ADR-0131's
  rule.
- No reparenting (`xwayland/manager/mod.rs`): Yuna draws no frames, so a window stays a child of
  the root. Java's AWT waits for a reparenting manager unless it runs with
  `_JAVA_AWT_WM_NONREPARENTING=1`; its windows otherwise stay blank. Decided with the desktop at
  Stage 2b: Yuna setting it for its programs, or reparenting.
- An X11 window's size hints give its limits alone: its increments and base size (a terminal's
  cells) and its aspect are read and not followed, as tiling managers do; its window type gives
  only whether it is managed: a dialog, a splash or a utility window opens tiled unless it names
  a parent through `WM_TRANSIENT_FOR`. Paid at Stage 2c, with the rules per application.
- `_NET_WM_SYNC_REQUEST` uses the basic counter alone: the extended one, which tells each frame
  drawn, is left. A window that never answers waits 100 ms at each resize, where KWin stops
  syncing it after a timeout. Paid at Stage 4, where games' frames are paced.
- A ping carries `CurrentTime`, not the time of the event that asked to close, and its answer is
  matched by its window alone, so that any X11 client of the seat can answer another's ping and
  clear its "unresponsive"; one ping waits at a time. Kept: X11 isolates nothing within a seat
  ([ADR-0067](adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)); the timestamp is
  paid with `WM_TAKE_FOCUS`'s below.
- `_NET_STARTUP_ID` is read when a window asks to be mapped, not when it changes later; X11's
  startup notification messages (`_NET_STARTUP_INFO`) are not read. X11 keeps its clients apart
  in nothing: another X11 client of the seat can read a window's startup id and use its token
  first. Seats stay apart: a token activates its own seat's windows alone. Startup messages are
  read at Stage 2c, with the launcher.
- `WM_TAKE_FOCUS` is sent with `CurrentTime`, not the time of the event that moved the focus, as
  ICCCM asks: a client that checks it may refuse the focus. Paid at Stage 2b, with real input
  timestamps from libinput.
- `_NET_WM_MOVERESIZE` is taken and does nothing: Yuna has no interactive move. A request to
  iconify or hide (`WM_CHANGE_STATE`, `_NET_WM_STATE_HIDDEN`) does nothing: Yuna minimises
  nothing. Moves are paid at Stage 2c, with Super-drag; minimising, if the owner wants it.
- A managed window typed a menu, a notification or a tooltip, not override-redirect, is managed
  as any window: tiled, unless it names a parent. Paid at Stage 2c, with the rules per
  application.
- An X11 window's minimum size is bounded by its seat's largest output, and follows the outputs
  as they change; its own size, kept on an axis the layout leaves to it, is not bounded by them.
  The limits follow the outputs after the windows were sized: one more configure per output
  change, bounded, which moving the follow-up before the arrangement would remove. No story makes
  that follow-up matter: removing an output re-sizes X11 windows through another path in every
  scenario tried. Paid at Stage 2a, M2.17 (the X11 manager, a task).
- An X11 client's windows are bounded per connection: the manager reads the properties of 1,000
  of one X11 client, each title and class kept to 4 KiB, about 35 KiB a window measured on Yuna's
  side, and Xwayland's own 194 KiB ([measurements](measurements.md)). A program that opens many
  connections multiplies it. A window past the 1,000 is never shown, even after its client's
  other windows go, and its surface gets no frame callbacks: a program that paces it with
  Present might wait on it, not checked. Unmapped windows of a Wayland client are left unbounded
  until Stage 2a, M2.16, the owner chose on 2026-11-18 ([above](#the-headless-compositor)). The
  missing frame callbacks of a window past the 1,000 are paid at Stage 2a, M2.17 (the X11 manager, a
  task).
- Override-redirect windows past their client's 1,000, or past the menus' caps, are not lowered:
  they may lie above windows Yuna shows and take their clicks. A managed window past the 1,000
  made override-redirect and raised stays on top until the next full restack, which sends one
  `ConfigureWindow` for each mapped window past the limit, their number unbounded (4,000 in the
  flood story). Stacking the new bottom just below the old one, when it stays mapped, would need
  one. Paid at Stage 2a, M2.17 (the X11 manager, a task).
- The orders in which a window and its surface pair: commit first is forced by a story and
  counted by Yuna; serial first, by the unit tests alone, since the managers' slice may end
  between a window's map and its serial. The stories of `x11_refused.rs` guard their fix only
  while the manager reads the serial after the commit, which nothing asserts. Paid at Stage 2a, M2.1
  (baselines): tests before the rewrite.
- An X11 client's `_NET_ACTIVE_WINDOW` is taken as xdg-activation without a token: the window
  takes the focus only when its client has it already; a pager's request (source 2) is no better.
  Revisited at Stage 2c, with Hollow's taskbar.
- An X11 window's place is cut to X11's 16-bit coordinates: with outputs past 32,767 X11 pixels
  from the root's origin, many outputs side by side at a large scale, a window there is told a
  place its pointer and Xwayland's emulated modes miss. An output whose logical position is not a
  whole unit is told to Xwayland rounded, so that its windows' X11 places may be a pixel off its
  own. Kept: no output layout so large is planned.
- Commands to a manager go at the end of the round that made them; the X11 focus and a window's
  place reach Xwayland on its manager's connection, the keyboard's `enter` on its Wayland one, in
  no set order between them. Kept: no client found that depends on the order.
- The managers' slice is checked by unit tests of `handle_within_slice`; no story sees the whole
  flood handled in one round, nor tests `manager_slice_end`'s wiring. `x11_bounds.rs`'s story of
  the properties' bound uses `MAX_WINDOWS_PER_CLIENT`, the layout's cap, which has the same
  figure, 1,000. Paid at Stage 2a, M2.1, tests before the rewrite.
- A backlog of the managers' events that comes back at least every 11 s keeps moving a stalled
  manager's watchdog, so that it is not ended; its memory stays within what Yuna may owe it. Paid at
  Stage 2a, M2.17 (the X11 manager, a task): the recovery is wrong, not the memory.
- Xwayland's list of the variables it keeps (`server.rs`) repeats MCP's programs' list
  (`mcp/programs/environment.rs`): XWayland, part of the core, may not depend on the MCP server.
  Paid at Stage 2a, M2.18 (the agents' runtime): one home, `yuna-process`.
- The area filter reads each texel 1/1024 past its centre, which holds for buffers up to about
  8,000 texels a side (`area_filter.frag`): past it, a sampler's rounding may take a texel's
  neighbour, a slight blur of one X11 window wider than 8,000 pixels below XWayland's scale.
  Revisited with the Vulkan renderer at Stage 2a, M2.8
  ([ADR-0106](adr/0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md)), which draws the
  filter again.
- The X11 element declares no opaque region: the damage tracker draws what lies under an opaque
  X11 window too, the background and the windows below it, where a Wayland surface's opaque
  region would let it skip them; a whole frame costs up to that overdraw more on each output.
  Paid at Stage 2a, M2.7 (the scene, alone), with Yuna's own scene.

## Architecture

Today's code against [ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md); Stage 2's core
replaces it, and nothing new is built on it that Stage 2 would undo.

- One `State` of 59 fields, 49 of them public, implements 24 of Smithay's handler traits on
  2026-11-19 (`grep -E '^impl .*Handler for State' -r crates/yuna/src | wc -l` counts them): any
  handler can change any state. Smithay's shape; Stage 2's core gives each protocol module its
  own objects. Paid at Stage 2a, M2.19's done-when (Smithay removed).
- A global, `OPENED_DISPLAYS` (`crates/yuna/src/render.rs`), keeps EGL displays alive because
  tests run several compositors in one process; Yuna's Vulkan core needs none. Paid at Stage 2a,
  M2.9 (Vulkan the default): it leaves with EGL.
- wayland-rs bounds each client's outgoing buffer at 4,096 bytes and ends a client whose buffer
  is full, where the rivals raise libwayland's to 1 MiB;
  [ADR-0136](adr/0136-a-client-that-stops-reading-1-mib-and-its-motion-merged.md) sets it at 1 MiB,
  at Stage 2a, M2.2 (the interface), and merges the pointer's motion, at M2.12 (seats and input).
- No crate forbids `unsafe` code; each `unsafe` block has its `// SAFETY:` comment, and
  `grep -rn 'unsafe {' crates/*/src` lists them: 15 in `yuna`, one in `yuna-bench`, none in
  `yuna-layout` and `yuna-motion`, on 2026-11-19. Paid at Stage 2a, M2.4 (the map, tasks, frames and
  events), with the crate map's checks.
- The process's memory grows by 0.0087 MiB a window, above Hyprland's 0.0063, the budget's figure
  ([ADR-0077](adr/0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md)); its cause
  is not isolated. Paid at Stage 2a, M2.16 (the store, wired behind a switch): the new store is
  measured against the budget.
- GLES types reach three modules beside `render.rs`: `state.rs` holds the renderer, `output.rs`
  each output's buffer and damage tracker, `dmabuf.rs` the import. Stage 2's Vulkan renderer
  replaces all four. Paid at Stage 2a, M2.9 (Vulkan the default).
- Yuna's writes of agents' selections live in the shared `State`, beside Smithay's selection
  state (`Selections` in `crates/yuna/src/selection.rs`: the writes, their next key, those to
  start, and the keys their sources note), since Smithay hands a client's read of a selection
  Yuna holds to its `SelectionHandler`. Stage 2's core gives them to the clipboard's own module.
  Paid at Stage 2a, M2.19's done-when (Smithay removed).
- Two places say which output shows a window: `scale.rs` reads the last arrangement, `colour/`
  the layout as it is; between a change and the next arrangement they may differ. Stage 2's core
  gives each fact one owner, at Stage 2a, M2.19's done-when (Smithay removed). Found by the [audit
  of 2026-11-17](audits/2026-11-17-architecture.md).
