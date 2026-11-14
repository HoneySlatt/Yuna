# 0038. Captures: from a pixel to a click, a window's capture, and who captures through Wayland

- Status: Accepted; amended by [0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md) (captures and clicks during motion)
- Date: 2026-11-13

## Context

[ADR-0034](0034-captures.md) makes a capture a PNG at its output's logical size by default, so
that a pixel of a whole output's image is a logical unit where `pointer` clicks
([ADR-0037](0037-seats-tools-input-and-closing.md)). It also lets an agent capture a region,
or the physical size; the milestone captures windows, with `ext-image-copy-capture-v1` inside
([PLAN](../PLAN.md#m15--agents-control)). The design's challenge left three questions:

- A region's or a window's image starts elsewhere than the output's corner, and a pixel of a
  physical image is 1 / scale of a unit: a pixel no longer names a click.
- What a window's capture shows: what the output shows where the window is, or the window
  drawn alone.
- Whether `ext-image-copy-capture-v1`, which grim, wf-recorder and the desktop portal speak, is
  offered to clients, and to which. Read in Smithay `8fd9fe0` on 2026-11-13: its
  `image_copy_capture` and `image_capture_source` modules take a filter per client for their
  globals, and Yuna decides each session's constraints. A window's source needs
  `ext-foreign-toplevel-list-v1`, which Yuna does not offer.

## Decision

The owner chose, on 2026-11-13:

- **A capture's mapping, and its id in `pointer`.** Each capture's result gives its mapping:
  its output, the logical point of its image's top-left corner on that output, and the logical
  units a pixel covers, 1 at the logical size, 1 / scale at the physical size. A point of the
  image at (px, py) pixels is the logical point (x + px × units, y + py × units). It also gives
  an id. `pointer` takes `capture` with x and y in that image's pixels, and Yuna maps them.
  Yuna keeps the last 16 captures of each seat.
- **A window's capture is what is seen.** It is the window's rectangle on its output, cut from
  that output's frame: what covers it, a menu or another window, is in the image, so that a
  click at a pixel reaches what the image shows there. A hidden window is refused, with what
  shows it.
- **`ext-image-copy-capture-v1` for every client, on its own seat.** Its globals are offered to
  every client; a client captures only its own seat's outputs: a session of another seat's
  output is stopped. No cursor is captured, since Yuna draws none, and no window is captured
  through Wayland until Yuna offers `ext-foreign-toplevel-list-v1`. Consent before a capture
  comes with the desktop ([ROADMAP](../ROADMAP.md)). A client's frame is ready as soon as it is
  copied, at its refresh, as the protocol asks, with the time of the boundary it is presented
  at; an agent's capture comes at that boundary ([ADR-0034](0034-captures.md)).

## Alternatives considered

- **The mapping alone**: nothing kept and `pointer` unchanged, but the agent adds and scales,
  and a model that adds wrong clicks elsewhere without a word.
- **A window drawn alone, offscreen**: a hidden or covered window shows, but a click on its
  covered part reaches the window above, and it needs a second drawing path, which another
  renderer would have to write again ([ADR-0012](0012-opengl-first.md)).
- **The protocol for agents' seats alone**: closer, but grim does not work in
  `yuna --headless`, where other projects' tests run, and HDR's checks will capture there.
- **No client**: nothing to guard, but no Wayland capture tool works.

## Consequences

- A window's capture never shows a hidden window, nor the part of a window another covers.
- Every client of `yuna --headless` may capture its outputs; in `yuna mcp`, an agent's
  programs capture their seat's. That is debt until the desktop asks consent.
- A capture id is valid while its output lives and it is among its seat's last 16.
