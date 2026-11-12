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
- Its timings were measured beside another build, and some 99th percentiles miss
  [ADR-0020](adr/0020-the-layout-models-shape.md)'s budgets ([measurements](measurements.md)):
  measured again on a quiet machine in M1.4.

## The headless compositor

- At a fractional scale, a client that draws at an integer buffer scale, rather than at its
  output's physical size through fractional scale and a viewport, is scaled when drawn: where its
  window geometry starts inside its surface, its right and bottom edges may blend up to one pixel
  of its margin. It stays while such a client's pixels are scaled anyway
  ([Scale](ARCHITECTURE.md#scale)).
- Virtual time is not kept from running ahead of real time while real clients are connected
  ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)); it matters when M1.5 steps the
  clock.
- `--gpu`, and with it the dmabuf feedback, has not been run yet
  ([ADR-0022](adr/0022-the-headless-backend-in-yuna.md)). The stories draw a client's dmabuf on
  llvmpipe alone, from a sealed memfd; a real GPU's dmabufs are tested nowhere, in CI or out.
- Every output draws one background frame when it is created, even with no client.
- The development shell's `LD_LIBRARY_PATH` (libglvnd, for EGL) applies to every program run in
  it.
- Smithay `8fd9fe0` places a new popup by its client's positioner with plain `i32` sums
  (`PositionerState::get_geometry`, in its `get_popup` handler), before Yuna sees the popup. An
  anchor rectangle or an offset near `i32::MAX` panics a debug build, whose overflow checks are on
  (seen: "attempt to add with overflow", `src/wayland/shell/xdg/mod.rs:591`), and wraps in a
  release build. Yuna checks the values before its own placement, but cannot reach that first
  sum: a patch in the fork, or overflow checks off, before debug builds run untrusted clients.
- A popup is placed when it is created and when it asks again; a reactive popup
  (`set_reactive`) is not placed again when its window moves or resizes. A popup of a window
  shown on no output is placed without constraint.
- Smithay resets a popup's role when it commits without a buffer, its parent included: mapping
  it again is then a protocol error for its client. Clients destroy their popups rather than
  unmap them.
- Smithay passes on only the activation tokens it keeps: a token used already, or pushed out by
  64 newer ones, is ignored without marking its window urgent.
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
