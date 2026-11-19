# 0131. Bounds on what a client or an agent may ask

- Status: Accepted; supersedes [0028](0028-bounds-on-what-one-client-may-ask.md) and [0070](0070-bounds-on-what-one-x11-client-may-ask.md)
- Date: 2026-11-18

## Context

Some requests are valid one at a time and harmful in number or in size. Wayland and X11 have no
error for "too many": a protocol error ends the client, and its other windows with it.
[ADR-0028](0028-bounds-on-what-one-client-may-ask.md) bounded what one Wayland client may ask
(2026-11-12), and [ADR-0070](0070-bounds-on-what-one-x11-client-may-ask.md) extended it to X11
(2026-11-15). Stage 1's audit, on 2026-11-18, found requests both left unbounded: held keys that
end a seat's clients, X11 windows that each cost Yuna 649 KiB, commands owed to a stalled X11
manager, outputs of any size, names of any length, accessibility replies kept whole
(Stage 1's audit). This ADR gathers the rule whole, with the bounds the
audit added.

## Decision

**The rule.** Over a bound, Yuna refuses the one request, or leaves the one thing undrawn, and
never ends the client: losing a window or a menu is better than losing the application and its
user's data (ADR-0028). Each bound is a named constant whose documentation says why its figure.

**Wayland clients** (ADR-0028, 2026-11-12):

- A client may have 1,000 windows in the layout, and all clients 10,000. A window over either cap
  is not shown, as a window the layout refuses: its client gets frame callbacks at the hidden
  windows' rate and keeps its other windows; the refusal is logged once per client.
- A window may have 64 popups, nested at most 32 deep; a positioner's values are bounded by 2^20.
  A popup over a bound, or whose parents loop, is dismissed (`popup_done`), which xdg-shell allows
  at any time. Yuna walks a popup's parents itself, bounded, before Smithay does.
- A client may keep 64 activation tokens not yet used; a new one pushes out its oldest.

**A seat's input** (the audit, 2026-11-18): a seat holds 64 keys and 16 buttons at once; a press
past either, or of a keycode the keyboard's keymap names no key for, is dropped, whoever sends it: a
virtual keyboard or pointer, or an agent through MCP, which is told why. A `wl_keyboard.enter`
lists every held key in one message of at most 4,096 bytes; past about 1,018 keys the message
cannot be sent and the display ends the client.

**X11 clients** (ADR-0070, 2026-11-15, and the audit):

- Windows are counted per X11 client, the client bits of the window's id: 1,000 in the layout for
  one X11 client, the 10,000 of all clients together holding.
- The manager reads the properties of 1,000 windows per X11 client; a window past it is followed
  without them, mapped when it asks below every window Yuna stacks, out of the client lists, and
  never shown. An override-redirect one maps itself and is not lowered ([debt](../debt.md)). Of each title, class and instance, at most
  4 KiB is kept once decoded; every property read is bounded: 64 KiB of text, 64 atoms, 1 KiB of
  startup id.
- An X11 client may show 64 menus (override-redirect windows) at once, and one Xwayland 256, all
  its clients together. A menu past its client's 64 is not shown until its next map or commit; one
  past its Xwayland's 256 shows once another of that Xwayland goes, lowest in X11's stacking
  first: one program can delay another's menus, never take their place for good.
- The manager keeps no atom name longer than the longest MIME type it passes on, 255 bytes.
- Each round of Yuna's loop gives the managers' events a slice: until 1 ms before the earliest
  deadline armed when the round began, 8 ms at most, one event at least. Events past the slice
  wait, in order, and the managers' channels pause meanwhile, so that X11's own buffers hold the
  burst rather than Yuna's memory.
- What Yuna owes a manager is bounded: the latest focus, stacking and selection of each kind
  replace the one owed, and the rest is held to 8 MiB. A manager that takes none of Yuna's
  commands for 10 s while some wait, and still none a second later, or is owed past the bound,
  has its Xwayland ended, which starts again at the seat's next X11 connection. Nothing of this
  is armed while the manager keeps up.
- A sync request waits 100 ms at most; a window that does not answer is drawn again as it is. A
  ping waits 5 s, EWMH's suggestion; a window that does not answer is marked unresponsive, never
  killed on a timer.

**An agent through MCP** (the audit, 2026-11-18): an output's side is 8,192 pixels at most, and
all of Yuna's outputs together hold at most the pixels of 16 outputs at 3840 × 2160; a
workspace's name is 256 bytes at most; an accessibility walk keeps at most 1,000 characters of
each text, an application's failure among them, 4 KiB of each object path and two words of
states as each reply arrives. The bounds of
the transfers between X11 and Wayland are [ADR-0091](0091-the-clipboard-between-x11-and-wayland-who-reads-and-its-bounds.md)'s,
the drags' [ADR-0102](0102-drag-and-drop-between-x11-and-wayland-a-drag-session-beside-smithays-grabs.md)'s,
the MCP server's own (lines, replies, waiting calls) [ADR-0120](0120-the-mcp-server-a-private-yuna-on-stdio.md)'s.

**Left unbounded, by the owner's decision of 2026-11-18**: a window's subsurfaces, a client's
shared-memory pools and the descriptors they hold, its buffers' memory and its windows not yet
mapped. Their protocols have no refusal but an error that ends the client; they are bounded by
Stage 2's core and its queue of pending commits ([debt](../debt.md)).

## Alternatives considered

- **No caps, the costs made linear**: removes the cost of each walk, not memory (ADR-0028).
- **Ending a client over a bound**: the only answer subsurfaces and pools allow; the owner kept
  the rule that never ends a client, and left those unbounded until Stage 2's core.
- **Bounds per Xwayland only** for X11: one X11 program could take every window or menu of its
  seat (ADR-0070); kept per client, with a per-Xwayland bound on menus besides.

## Consequences

- A malicious or broken client loses what it asked past a bound and keeps the rest; other
  clients and Yuna go on.
- Until Stage 2's core, one client can still stall or exhaust Yuna through subsurfaces, pools,
  buffers or unmapped windows ([debt](../debt.md)).

## Validation and reconsideration

Each bound is tested at its edge (the audit's stories, in `crates/yuna/tests/headless/`).
Proposed again when Stage 2's core gives each client a budget per round, or if a real application
meets a bound.
