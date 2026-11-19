# 0124. X11 windows across outputs of different scales

- Status: Accepted; supersedes [0044](0044-xwayland-across-outputs-of-different-scales.md),
  [0068](0068-x11-windows-drawn-by-yuna-through-an-area-filter.md) and
  [0069](0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md)
- Date: 2026-11-18

## Context

X11 has one scale for every output, and the owner's desktop mixes scales: the main monitor at 1.5,
a second at 1. [ADR-0044](0044-xwayland-across-outputs-of-different-scales.md) chose the behaviour;
[ADR-0068](0068-x11-windows-drawn-by-yuna-through-an-area-filter.md) carried it out and
[ADR-0069](0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md) answered its
question on games, relations that the ADRs' index could not show. A reader needed all three. This
ADR gathers the rule as it stands on 2026-11-18; it decides nothing new.

Yuna's own X11 window manager is [ADR-0066](0066-yunas-own-x11-window-manager-on-x11rb.md)'s, one
Xwayland per seat [ADR-0067](0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)'s, and the
bounds on X11 clients [ADR-0070](0070-bounds-on-what-one-x11-client-may-ask.md)'s. Rootless
Xwayland draws at buffer scale 1 and learns outputs' sizes from `xdg_output`'s logical size alone,
so that a compositor sets X11's scale by what it reports there. Most X11 toolkits read their scale
once, at start.

## Decision

- **X11 applications draw at the largest output scale, and Yuna scales each X11 window down on
  outputs of a smaller scale** (ADR-0044, 2026-11-14), the approach proposed at Linux Plumbers 2019
  ("XWayland multi DPI"). An X11 window keeps its size on every output, moved between them
  included; it is sharp on the outputs of the largest scale and a little softer on the others. The
  bar is this behaviour, not a rival's.
- **S is the largest scale among the seat's outputs when its Xwayland starts, fixed for that
  Xwayland's life** (ADR-0068, 2026-11-14). Xwayland's client gets S as its `client_scale`, so that
  it sees each output's logical rectangle times S and its pointer in X11 pixels. `Xft.dpi` is
  96 × S, rounded, in `RESOURCE_MANAGER`, and XSETTINGS gives `Xft/DPI` with it. When an output is
  added, removed, rescaled or changed through MCP, X11 windows keep their size through the same
  scaling and are placed again in X11 pixels
  ([ADR-0090](0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md)); the new S applies at
  the next start.
- **Yuna draws X11 surfaces with its own element** (ADR-0068): the buffer, in X11 pixels, covers
  its window's rectangle at s / S physical pixels per X11 pixel on an output of scale s, its
  geometry kept in X11 pixels; exact, pixel for pixel, where s = S.
- **Below S, an area filter** (ADR-0068): each physical pixel is the average of the X11 pixels it
  covers, weighted by how much of each, in a texture shader of Yuna's. It is the computation the
  captures' encoder already makes (`capture.rs`), which is the tests' reference.
- **gamescope, run inside Yuna, is the documented answer for a fullscreen X11 game on an output of
  a smaller scale** (ADR-0069, 2026-11-14): the game draws at the resolution given to gamescope.
- **Yuna does not break Xwayland's emulated modes** (ADR-0069). Its window manager configures a
  fullscreen window to the rectangle `_XWAYLAND_RANDR_EMU_MONITOR_RECTS` gives for its output,
  rather than to the output, and never moves a hidden fullscreen window. A game run without
  gamescope that sets the output's mode is then drawn pixel for pixel.

## Alternatives considered

- **One Xwayland per application** ([ADR-0044](0044-xwayland-across-outputs-of-different-scales.md)):
  most applications read their scale once, so it does not help a window moved later, and costs one
  X server each.
- **One scale, no correction**, as Umbriel ([ADR-0044](0044-xwayland-across-outputs-of-different-scales.md)):
  X11 windows too large on outputs of a smaller scale.
- **Following S at once when it changes**
  ([ADR-0068](0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)): each window of a program
  that read its scale at start would shrink until it restarts.
- **Smithay's drawing of client-scaled surfaces, or the GPU's bilinear filter**
  ([ADR-0068](0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)): one width in three drawn
  a pixel off, or thin text uneven at a ratio of 2/3 and shimmering as it slides.
- **Xwayland's emulation as the answer for games**
  ([ADR-0069](0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md)): a game left
  at the desktop's size pays 2.25 times the output's pixels.
- **A second Xwayland at scale 1 for games**
  ([ADR-0069](0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md)): small on the
  output at 1.5, two servers and a rule per application; in the PLAN's ideas.

## Consequences

- The X11 window manager configures windows in X11 pixels, and converts with S alone.
- On an output added with a scale above S, X11 windows are scaled up, the right size, softer, until
  Xwayland restarts.
- An X11 application that does not scale itself is small on the outputs at S; a rule per
  application, drawing it at scale 1, scaled up, is [debt](../debt.md).
- A game run without gamescope at the desktop's size costs 2.25 times the output's pixels at 1.5
  beside 1: stated in the documentation.
- Proton's own scaling with Yuna is on the owner's checklist: it cannot be run headless.

## Validation and reconsideration

Evidence: an X11 window moved between virtual outputs at 1.5 and 1, the right size on both, sharp
at 1.5, checked against reference images; a test that sets the output's mode from an X11 client
and checks the window drawn pixel for pixel; the area filter's cost measured at 4K on the GPU and
on llvmpipe ([measurements](../measurements.md)).
