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
- A frame is presented at least one period after the commit it shows: a commit arms the first
  boundary after it, drawing starts there, and the frame is presented at the first boundary
  after its drawing ends (`frames.rs`). At 240 Hz, a client waits over 4.17 ms however fast the
  frame, so the bar's input-to-present budget, one frame, read headless, is missed by
  construction; the manual clock, where drawing takes no time, hides it. M1.5 opens with the
  design of frames drawn ahead of their boundary, in an ADR that amends
  [ADR-0023](adr/0023-frames-presentation-and-the-clock.md) (the owner's choice, 2026-11-12).
- Virtual time is not kept from running ahead of real time while real clients are connected
  ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)); it matters when M1.5 steps the
  clock.
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
- Until M1.5 brings input, only a keyboard `enter` serial makes an activation token or a popup
  grab valid; key, button and touch serials come with input.
- The modal hint of `xdg-dialog-v1` is read but not used: a modal dialog does not yet keep its
  parent from the focus.
- `ext-data-control-v1` is offered to every client: any client can read and set the clipboard and
  the primary selection without the focus. It is filtered at Stage 2, through security-context or
  a rule for privileged clients.
- Smithay `8fd9fe0` checks that a selection request comes from the client with the keyboard
  focus, not the serial it names: that client may set a selection with any serial, not only in
  answer to its own input. Checking it needs a patch to Smithay; a test pins the gap down
  (`the_serial_of_the_client_with_the_focus_is_not_checked`).
- Drag and drop is refused until the pointer comes (M1.5), with no answer to the client: its
  source is not cancelled. With a pointer, Smithay's default `dnd_requested` would cancel every
  drag; M1.5 implements it.
- Smithay `8fd9fe0` departs from `wl_data_device` twice: an offer stays readable after its client
  loses the focus, and a device created after its client got the focus is told the selection
  only at the next change. Both are Smithay's; neither leaks across seats.
