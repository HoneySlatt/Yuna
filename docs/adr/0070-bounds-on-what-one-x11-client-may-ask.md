# 0070. Bounds on what one X11 client may ask

- Status: Superseded by [0131](0131-bounds-on-what-a-client-or-an-agent-may-ask.md); extends
  [0028](0028-bounds-on-what-one-client-may-ask.md) to X11
- Date: 2026-11-15

## Context

[ADR-0028](0028-bounds-on-what-one-client-may-ask.md) bounds what one Wayland client may ask, so
that no client's numbers take Yuna or the other clients down. Under XWayland, every X11 program
of a seat reaches Yuna through one Wayland client, Xwayland's
([ADR-0067](0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)): a bound on that client
would let one X11 program fill it for all. X11 also lets a program create, map and change windows
far faster than Yuna's frame loop needs: M1.8's stories mapped 5,000 windows at once, and before
these bounds one round of Yuna's loop took up to 455 ms, holding every output's frames.

X11 has no error for "too many" either, and a window manager cannot refuse a map without
leaving its program waiting.

## Decision

Chosen while building M1.8, within ADR-0028's rule:

- **Windows are counted per X11 client**, the client bits of the window's id, not per Xwayland:
  1,000 in the layout for one X11 client, as for a Wayland client; the 10,000 of all clients
  together still hold. A window over a cap is treated as ADR-0028 says.
- **An X11 client may show 64 menus** (override-redirect windows) at once, as a Wayland window may
  have 64 popups; one over the bound is not shown.
- **Each round of Yuna's loop gives the managers' events a slice**: until 1 ms before the earliest
  deadline armed when the round began, 8 ms at most, one event at least. Events past the slice
  wait, in order, and the managers' channels pause meanwhile, so that X11's own buffers hold the
  burst rather than Yuna's memory, and a frame is never late for X11.
- **A sync request waits 100 ms at most**: a client draws a new size in one or two frames, and
  past six frames at 60 Hz a still window reads as stuck. A window that does not answer is drawn
  again as it is.
- **A ping waits 5 s**, EWMH's suggestion; a window that does not answer is marked unresponsive,
  never killed on a timer.
- Every property read is bounded: 64 KiB of text, 64 atoms, 1 KiB of startup id.

## Alternatives considered

- **Bounds per Xwayland, as on any Wayland client**: simplest, but one X11 program could take
  every X11 window or menu of its seat.
- **No slice, every event handled when read**: the fewest lines, but a flood of X11 requests holds
  the owner's frames for hundreds of milliseconds, against ADR-0007 and the frame budget.
- **KWin's 10 s and Smithay's 1 s for a sync request**: fewer windows drawn before their content
  is ready, but a stuck program freezes its window for seconds.

## Consequences

- A flood of X11 windows opens more slowly, while the other windows keep their frames
  ([measurements](../measurements.md#m18--a-flood-of-x11-windows-and-menus-2026-11-15)).
- `MAX_MENUS_PER_CLIENT`, `MANAGER_SLICE`, `X11_SYNC_TIMEOUT` and `X11_PING_TIMEOUT` hold these
  figures in the code; a change of one of them changes this ADR.
- The layout's own lookups stay linear in its windows ([debt](../debt.md)).
