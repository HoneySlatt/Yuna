# 0121. Captures: what is seen, at the logical size, for agents and clients

- Status: Accepted; supersedes [0034](0034-captures.md),
  [0038](0038-captures-coordinates-windows-and-wayland.md),
  [0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md) (captures during motion)
  and [0089](0089-yuna-for-the-owners-other-projects.md) (`wlr-screencopy-unstable-v1`);
  supersedes in part [0062](0062-the-compositors-comfort-and-look.md) (point 9, a capture yields
  to a game)
- Date: 2026-11-18

## Context

How Yuna captures was decided in five ADRs. [ADR-0034](0034-captures.md) made an agent's capture a
PNG at the output's logical size. [ADR-0038](0038-captures-coordinates-windows-and-wayland.md)
mapped its pixels to clicks, captured windows as seen, and offered `ext-image-copy-capture-v1` to
clients. [ADR-0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md) cut captures
from what a frame drew during motion. [ADR-0062](0062-the-compositors-comfort-and-look.md)
point 9 had a capture yield to a game, and
[ADR-0089](0089-yuna-for-the-owners-other-projects.md) added `wlr-screencopy-unstable-v1` for the
owner's other projects. A reader needed all of them.

This ADR gathers the rule as it stands on 2026-11-18. It decides nothing new. What the desktop
does with captures, the selection, recording and the share picker, is
[ADR-0107](0107-what-yuna-does-itself-and-what-it-leaves-to-the-shell.md)'s; windows never
captured are [ADR-0061](0061-yunas-uses.md)'s point 2.

An agent checks what it did by capture. MCP returns an image as base64; Claude's API refuses an
image over 5 MB and scales one whose long edge passes 2,576 pixels, and a model answers in the
coordinates of the image it sees.

## Decision

- **An agent's capture is a compressed PNG**, written with the `png` crate, by default at the
  output's **logical size**: 2,560 × 1,440 for the default 4K output at scale 1.5, so that a pixel
  is a unit where the agent clicks. A region, or the physical size, is asked for (ADR-0034,
  2026-11-13). Its size is checked against the 5 MB limit before it is sent.
- **The pixels are copied on the main thread when the frame they show is drawn**, cut from what
  that frame drew, motion included, and delivered at its boundary (ADR-0034; ADR-0054,
  2026-11-14). The encoding runs on a thread of its own, so that no frame waits for it. One capture
  of a seat is in flight at a time.
- **A capture's mapping, and its id in `pointer`** (ADR-0038, 2026-11-13). Each result gives the
  capture's output, the logical point of its image's top-left corner, and the logical units a pixel
  covers: 1 at the logical size, 1 / scale at the physical size. It also gives an id: `pointer`
  takes `capture` with x and y in that image's pixels, and Yuna maps them. Yuna keeps the last 16
  captures of each seat; an id is valid while its output lives and it is among them.
- **A window's capture is what is seen** (ADR-0038): the window's rectangle on its output, cut from
  that output's frame, with whatever covers it, so that a click at a pixel reaches what the image
  shows there. A hidden window is refused, with what shows it.
- **`ext-image-copy-capture-v1` for every client, on its own seat** (ADR-0038). Its globals are
  offered to every client; a client captures only its own seat's outputs, and a session of another
  seat's output is stopped. No cursor is captured, since Yuna draws none, and no window is captured
  through Wayland until Yuna offers `ext-foreign-toplevel-list-v1`. A client's frame is ready as
  soon as it is copied, at its refresh, with the time of the boundary it is presented at.
- **`wlr-screencopy-unstable-v1` version 3**, Yuna's own handler on the same captures, under the
  same rules (ADR-0089, 2026-11-17): every client sees it, a client copies only its own seat's
  outputs, one copy a refresh at most, a copy with damage waits at rest without a wake-up, and its
  frames count among the capture sessions' bounds. `wl_shm` buffers alone; no cursor. Its server
  side is `wayland-protocols-wlr`, already in `Cargo.lock` through Smithay, under MIT; its client
  side joins the tests.
- **A capture yields to a game** (ADR-0062, 2026-11-14): rather than cost a game a frame, a capture
  lowers its own rate, and says so.
- **Consent before a capture comes with the desktop**, given through the share picker at Stage 2
  (ADR-0038; ADR-0107).

## Alternatives considered

- **An uncompressed PNG, or raw pixels** ([ADR-0034](0034-captures.md)): 14.7 MB at 2,560 × 1,440,
  over the limit, or nothing the model sees.
- **The mapping alone, without an id**
  ([ADR-0038](0038-captures-coordinates-windows-and-wayland.md)): the agent adds and scales, and a
  model that adds wrong clicks elsewhere without a word.
- **A window drawn alone, offscreen** ([ADR-0038](0038-captures-coordinates-windows-and-wayland.md)):
  a click on its covered part would reach the window above, and it needs a second drawing path.
- **The protocol for agents' seats alone, or for no client**
  ([ADR-0038](0038-captures-coordinates-windows-and-wayland.md)): grim would not work in
  `yuna --headless`, where other projects' tests run.
- **No `wlr-screencopy`** ([ADR-0089](0089-yuna-for-the-owners-other-projects.md)): uwui's measure
  would count no frame in Yuna until uwui changes.

## Consequences

- Dependency: `png` (image-rs, MIT OR Apache-2.0) and its six crates; base64 is Yuna's own.
- A window's capture never shows a hidden window, nor the part of a window another covers.
- Every client of `yuna --headless` may capture its outputs; in `yuna mcp`, an agent's programs
  capture their seat's. That is debt until the desktop asks consent.
- Yuna keeps a capture protocol deprecated upstream in favour of `ext-image-copy-capture-v1`, until
  no tool the owner uses speaks it.

## Validation and reconsideration

Evidence: the stories of captures, of a click by a capture's pixels, of a window's capture and of
both capture protocols (`tests/headless/captures.rs`, `tests/headless/wlr_screencopy.rs`). Proposed
again when uwui leaves `wlr-screencopy`.
