# 0091. The clipboard between X11 and Wayland: who reads it, and its bounds

- Status: Accepted
- Date: 2026-11-18

## Context

Yuna's own X11 manager ([ADR-0066](0066-yunas-own-x11-window-manager-on-x11rb.md)) left the
clipboard and the primary selection between a seat's X11 and Wayland programs to M1.11. KWin,
Mutter and xwayland-satellite bridge them the same way: a window of the manager's owns the X11
selection for Wayland, an X11 owner becomes a Wayland selection whose types come from its
`TARGETS`, and the data passes only when a program pastes, in pieces (INCR) when it is large
([survey](../survey.md#the-agents-toolbox)). Their differences are the open choices: KWin gives X11 the Wayland
selection only while an X11 window is active, Mutter always; neither Mutter nor satellite puts a
time limit on a transfer.

Wayland gives the selection only to the client with the keyboard focus. X11 has no such rule:
any X11 client may ask the owner for the selection at any time.
[ADR-0070](0070-bounds-on-what-one-x11-client-may-ask.md) bounds what one X11 client may ask of
windows and menus.

Smithay at Yuna's pin told the compositor nothing when a selection ended with its source, a
client destroying the source or leaving.

## Decision

Chosen while building M1.11, as implementation within ADR-0066:

- **The seat's Wayland selection is readable from X11 while one of the seat's X11 windows has the
  keyboard focus**, as Wayland's own rule has it. The manager owns the X11 selection as soon as a
  Wayland client sets one, and refuses every request, `TARGETS` included, while a Wayland window
  has the focus. An X11 selection always passes to Wayland, which then gives it to the focused
  client as any selection. A request older than Yuna's taking of the selection is refused, as
  ICCCM asks; `CurrentTime` is answered.
- **Bounds.** A transfer that makes no progress for 5 s ends. A transfer ends at 256 MiB. At most
  8 transfers run each way on a seat, the 9th refused. An owner's `TARGETS` is read up to 256
  atoms; at most 64 types of 255 bytes go to X11. Each transfer holds one piece in memory, never
  the whole data. A malformed answer, a pipe closed or a client gone ends its transfer alone.
- **All X11 work stays on the manager's thread**, pipes included; Yuna's loop only makes pipes
  and passes descriptors in messages, so that nothing waits on the state thread.
- **Yuna's fork of Smithay calls `new_selection(None)`** when a selection's source is destroyed,
  as when its client clears it ([patch 6](../smithay-patches.md)), so that the X11 selection is
  released at once.

## Alternatives considered

- **KWin's rule, the X11 selection taken and given up at each focus change**: the same protection,
  but every X11 listener is told of a new owner at each click, and X11 would see an older X11
  owner's content while a Wayland window has the focus.
- **Mutter's rule, no focus check**: any X11 program, in the background too, reads the Wayland
  clipboard at any time.
- **No time limit**, as Mutter and satellite: a program that never answers holds a transfer, and
  with it a place among the 8, for good.
- **Asking Smithay at each round whether the selection lives**, with a type no client can offer:
  no patch, but about a thousand system calls a second while a client animates, and a panic on
  the state thread if Smithay ever forwards that type.

## Consequences

- X11 and Wayland programs of one seat copy and paste text, images and any type both ways;
  drag and drop between them comes at [M1.12](../PLAN.md#m112--drag-and-drop-between-x11-and-wayland).
- The cap of transfers is per seat, not per program; a destroyed `ext-data-control` source is not
  reported, the patch covering the data device and the primary selection; `STRING` is served
  from UTF-8 without conversion; `MULTIPLE` is refused. All are in [debt](../debt.md).
- The fork carries one more patch to carry across each raise of Smithay.

## Validation and reconsideration

Evidence: 16 headless stories, each failing before its code: text both ways, 1 MiB through INCR
both ways, the primary selection, an owner gone mid-transfer, a source that never writes, a
selection set before Xwayland starts, the focus rule and its edges, malformed and made-up
answers, a request older than the taking, nine reads at once, a source destroyed or its client
gone. No real application was run. Proposed again if a program the owner uses needs X11 to read
the clipboard without the focus, a clipboard manager under X11 among them.
