# 0026. Real clients in the layout model

- Status: Accepted
- Date: 2026-11-12

## Context

Until M1.4, the layout model assumed that every window takes the size it is given
([ADR-0020](0020-the-layout-models-shape.md)). Real clients ask a minimum and a maximum size,
commit their own size, and open dialogs. [ADR-0019](0019-the-layouts-behaviours.md) makes niri the
reference for scrolling columns, focus and floating windows, and Hyprland for dwindle; it departs
from them only to fix a defect, to serve games first, or on the owner's decision.

Read on 2026-11-12, in niri at commit 7f81a7f and Hyprland at commit 70ec30e:

- In niri, a tile is its window's committed size: a column is as wide as its widest window, and
  windows stack by their committed heights. A column's width respects every window's minimum and
  maximum width. Heights are shared, each window getting at least its minimum, and a window whose
  minimum and maximum heights are equal keeps that height. Minimums that do not fit overflow the
  column. Heights asked are rounded to whole logical units and widths floored, so that
  proportional columns fit the screen exactly; both at least 1. When a column changes width, the
  focused column stays where it is on screen. A fullscreen window smaller than its output is
  centred.
- In niri, a window with a parent, or with a fixed height, opens floating, centred on its parent
  at its own size; its first configure is 0 on each axis whose size is not fixed. A new window takes the
  focus unless a fullscreen column holds it; a dialog takes the focus only from its parent. A
  valid activation request focuses its window.
- niri tells the active window of every workspace of the active output that it is activated;
  Hyprland only the focused window.
- Hyprland's dwindle floats a window whose maximum is smaller than the space it would split, and
  asks a tile its space whatever its minimum (`misc:size_limits_tiled` false).
- Traces of Alacritty 0.17, GTK 4.24 and Qt 6.12 under sway: each commits exactly the size it is
  asked; GTK's buffer is larger than its window geometry, because of its shadows; sway's first
  configure is 0 × 0, so each client drew one frame at its own default size before its tile's.

## Decision

- **Asked and committed.** The model keeps each window's minimum, maximum and committed size, the
  latter its xdg window geometry. `Layout::arrange` gives each window the size it is asked, in
  whole logical units, and the rectangle it is drawn in, clipped to it.
- **Scrolling columns and floating windows follow niri**, as read above. Every window is drawn
  at the size its client committed; a size set by a rule or by the compositor is the size it is
  asked, and stays asked.
- **The first configure carries the size the window will have**, from `Layout::size_on_open`.
- **Dwindle follows Hyprland**: no minimum enforced, a maximum too small floats the window. Yuna's
  own rule: a window larger than its space is clipped to it, so that it never covers its
  neighbours.
- **A tile is never asked 0**, which would tell its client to choose its own size.
- **A dialog opens on its parent's workspace.**
- **Departures, games first.** A fullscreen window's own dialog takes the focus and shows over it,
  the window staying fullscreen. While the window, its dialog or a floating window over it has
  the focus, it holds its screen: an activation request is refused unless it comes from that
  window or one of its dialogs, nested ones included; the compositor then marks the requesting
  window urgent.
- **Departure, the owner's choice on 2026-11-12.** Only the window that has its seat's focus is
  told it is activated, as in Hyprland: a hidden terminal does not blink its cursor and wake Yuna.

## Alternatives considered

- **Places from the sizes asked alone**: simpler, but a window that commits more covers its
  neighbour, and one that commits less leaves a hole; niri places from what was committed.
- **Minimums enforced in dwindle**: a deep tree would then overflow its output; Hyprland does not.
- **Activations obeyed always, as niri**: any program could take a game's screen.
- **niri's activated windows**: no redraw when switching workspaces, but hidden windows that
  believe they are active keep drawing.

## Consequences

- A slow client resizing moves its column's neighbours when it commits, not before.
- A dwindle window whose minimum exceeds its space shows only its top-left part.
- The compositor reads each window's limits and geometry on every commit and passes them on,
  and opens a window at its first buffer, its size known.
- While a fullscreen game's dialog has the focus, the other floating windows of its workspace
  show over the game too, as they do whenever the fullscreen window does not hold the focus.
