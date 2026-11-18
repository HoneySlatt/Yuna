# 0102. Drag and drop between X11 and Wayland: a drag session beside Smithay's grabs

- Status: Accepted
- Date: 2026-11-18

## Context

M1.12 carries drags between a seat's X11 and Wayland programs, both ways, through Yuna's own X11
manager ([ADR-0066](0066-yunas-own-x11-window-manager-on-x11rb.md)), on the clipboard's bridge
([ADR-0091](0091-the-clipboard-between-x11-and-wayland-who-reads-and-its-bounds.md)). X11 programs
drag with XDND: the source finds the X11 window under the pointer, and talks to it in client
messages; the data passes through the `XdndSelection` selection. KWin, Mutter and Smithay's own X11
manager bridge it the same way: a hidden window of the manager's stands for Wayland's windows to an
X11 source, and the manager speaks XDND to an X11 target for a Wayland source
([survey](../survey.md#the-agents-toolbox)).

Yuna's Wayland drags run on Smithay's drag grab, whose targets are Wayland surfaces only. Xwayland
binds no `wl_data_device`, so a drag over an X11 window offers it nothing. An X11 program that
drags needs the pointer's motion and its release to keep reaching its window, through Xwayland,
for the whole drag. Smithay's `xwayland` feature keeps them there only for an Xwayland client that
carries Smithay's own client data, which Yuna's cannot ([ADR-0066](0066-yunas-own-x11-window-manager-on-x11rb.md)).
[ADR-0039](0039-leaving-smithay-step-by-step.md) moves the seats and the pointer off Smithay only
at Stage 2's opening.

## Decision

The owner chose, on 2026-11-18:

- **A drag session of Yuna's, per seat, beside Smithay's grabs; no new pointer grab.**
  - An X11 program's drag keeps Smithay's ordinary grab of a held button, which sends the
    pointer's motion and release to the pressed X11 window. The session follows the pointer,
    picks the target, and offers the drag to a Wayland window through Smithay's offers.
  - A Wayland program's drag keeps Smithay's drag grab. Over an X11 window, the grab is given no
    target, and the session speaks XDND to the window through the manager. Yuna wraps the
    program's source, so that the grab's ending of it at a release over an X11 window is decided
    by the session: dropped if the X11 window accepted, cancelled otherwise.
  - The session's decisions hold no Smithay, Wayland or X11 type, and are tested alone; Stage 2
    keeps them when the seats move.

Chosen while building M1.12, as implementation within ADR-0066 and ADR-0091:

- **The target is a shown window of the layout under the pointer**, its popups and subsurfaces
  included: never an X11 menu, tooltip or drag icon, never a hidden window.
- **The X11 source decides the drop**: Yuna drops on the Wayland window when the source sends
  `XdndDrop`, never at the release; the Wayland window is offered the drag only once the source
  has entered Yuna's hidden window.
- **Who reads and who speaks.** A Wayland program's dragged data is served only to the X11
  client Yuna's messages go to: the target window's, or its `XdndProxy`'s. An X11 drag is offered
  to Wayland only when the X11 client that owns `XdndSelection` is the one whose window the button
  was pressed on, from any of its windows, as Qt and GTK own it from a hidden one. A message that
  does not name the drag's current peer is left.
- **A drag outlives what it was over.** While the button is held, an X11 target window gone,
  `XdndSelection` taken by another X11 client, or Xwayland gone, ends only the drag's X11 side:
  the drag goes on among Wayland windows, and its source is cancelled at the release.
- **XDND versions 3 to 5**, as the specification asks.
- **Times.** Every message to an X11 target names the server time at which the manager took
  `XdndSelection`; reads from an X11 source name its latest position's time, or its drop's, and
  are asked again, four asks in all at most, when the source refuses them for a newer position, as Qt
  serves only its latest.
- **Bounds**, the clipboard's: a wait without progress ends after 5 s; the data goes through the
  bridge's transfers, their 8 at once each way and their 256 MiB; a type list is read up to 64
  atoms ([ADR-0070](0070-bounds-on-what-one-x11-client-may-ask.md)); a drag outlives its grab
  while its data or its last answer runs, two at most per seat.

## Alternatives considered

- **A drag grab of Yuna's own, for every drag**: everything in one place, and no reliance on the
  order in which Smithay's grab ends a source; but larger, it rewrites the Wayland-to-Wayland drag
  that works today, and it moves a part of the seats' core before Stage 2, against ADR-0039.
- **Smithay's `xwayland` feature, the pointer's target made a type of Yuna's**: does not work, its
  grab keeps the pointer on Xwayland only for Smithay's own Xwayland client, and it would compile
  Smithay's X11 manager beside Yuna's.
- **Three patches to Smithay's drag grab in Yuna's fork**: the smallest change to Yuna, but the
  fork's largest, carried across every raise of Smithay.
- **KWin's rule, Yuna's hidden window over Wayland's windows alone**: an X11 program's drag over
  empty space would find a hidden X11 window there.

## Consequences

- X11 and Wayland programs of one seat drag text, files and any type both ways; an agent drags
  through MCP's pointer as a user does.
- The way a Wayland drag over an X11 window ends depends on the order Smithay's grab calls its
  source and its handler; a test holds it, and it goes with the seats at Stage 2.
- A Wayland drag's icon is still not drawn ([debt](../debt.md)); an X11 drag's icon window is
  drawn as an X11 menu.

## Validation and reconsideration

Evidence: 40 headless stories, 38 failing before the code and two guards of X11's own drags
and of a drag with no data, with X11 test programs that act as Qt's and GTK 4's XDND do: text, a file's URI and a mebibyte through INCR, both ways; a drag
cancelled, refused, never finished, its source or its target gone mid-drag, its selection taken,
a drag icon under the pointer, a still pointer, a read before the drop, a move, a proxy, XDND 2
and 3 targets, made-up messages, an old target's late read, the root point against Xwayland's
own, two drags at once, and an X11 drag that crosses a Wayland window to another X11 window.
The closing review added 11: a jump of the pointer out of a Wayland window, the pointer after a
drop on X11, a flood of `XdndEnter`, a source told only an action it offered, the proxy found as
Chromium looks, and Xwayland killed or the seat removed mid-drag, 51 in all.
Qt's `qml` on X11 and on Wayland, driven through MCP, drops text and a URI both ways, and Yuna
rests after. Not run: GTK, Chromium, Java or Wine ([debt](../debt.md#xwayland)).

Proposed again at Stage 2's opening, when the seats leave Smithay and Yuna writes its own drag
grab, or if a program the owner uses fails to drag.
