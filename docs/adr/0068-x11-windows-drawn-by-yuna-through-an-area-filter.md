# 0068. X11 windows drawn by Yuna through an area filter, at a scale fixed for an XWayland's life

- Status: Superseded by [0124](0124-x11-windows-across-outputs-of-different-scales.md); carries out
  [0044](0044-xwayland-across-outputs-of-different-scales.md)
- Date: 2026-11-14

## Context

[ADR-0044](0044-xwayland-across-outputs-of-different-scales.md) chose that X11 applications draw
at the largest output scale, S, and that Yuna scales each X11 window down on outputs of a smaller
scale. Rootless Xwayland always draws at buffer scale 1; it learns outputs' sizes only from
`xdg_output`'s logical size, so a compositor sets X11's scale by what it reports there. Smithay's
`client_scale` multiplies what one client is told of outputs and converts its surfaces' and its
pointer's coordinates; KWin and cosmic-comp do the same.

M1.8's design found two flaws in drawing through Smithay as it stands
([survey](../survey.md#xwayland)):

- Smithay rounds a client-scaled surface's size to whole logical units before drawing it: at
  S = 1.5, one X11 width in three is drawn a pixel off and resampled, even on the output at 1.5.
- The GPU's bilinear filter at a ratio of 2/3 gives every third source pixel half the weight of
  its neighbours: a one-pixel stroke shows as one pixel at 75 % or two at 25 %, and flips between
  them as a window slides.

Most X11 toolkits read their scale once, at start: Qt and Wine do; GTK 3 rereads its font DPI.

## Decision

The owner chose, on 2026-11-14:

- **S is the largest scale among the seat's outputs when its Xwayland starts, fixed for that
  Xwayland's life.** Xwayland's client gets S as its `client_scale`, so that it sees each output's
  logical rectangle times S and its pointer in X11 pixels. `Xft.dpi` is 96 × S, rounded, in
  `RESOURCE_MANAGER`, and XSETTINGS gives `Xft/DPI` with it. When an output is added, removed or
  rescaled, X11 windows keep their size, through the same scaling; the new S applies at the next
  start.
- **Yuna draws X11 surfaces with its own element**: the buffer, in X11 pixels, covers its
  window's rectangle at s / S physical pixels per X11 pixel on an output of scale s, its geometry
  kept in X11 pixels; exact, pixel for pixel, where s = S.
- **Below S, an area filter**: each physical pixel is the average of the X11 pixels it covers,
  weighted by how much of each, in a texture shader of Yuna's. It is the computation the captures'
  encoder already makes (`capture.rs`), which is the tests' reference.

## Alternatives considered

- **Following S at once when it changes**: GTK's fonts follow, but each window of a program that
  read its scale at start shrinks on every output, by S_old / S_new, until it is restarted.
- **Smithay's drawing of client-scaled surfaces**: no code, but a window in three is resampled at
  S.
- **The GPU's bilinear filter**: free, but thin text is uneven on outputs at 1 and shimmers while
  it slides, short of ADR-0044's bar.

## Consequences

- The X11 window manager configures windows in X11 pixels, and converts with S alone.
- The area filter's cost is measured at 4K on the GPU and on llvmpipe, against the plain copy.
- On an output added with a scale above S, X11 windows are scaled up, the right size, softer,
  until Xwayland restarts.
- An X11 application that does not scale itself stays small on outputs at S: a per-application
  rule is [debt](../debt.md).
