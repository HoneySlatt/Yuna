# 0147. A seat connects 256 clients at most

- Status: Accepted
- Date: 2026-11-20

## Context

[ADR-0136](0136-a-client-that-stops-reading-1-mib-and-its-motion-merged.md) raises each client's
outgoing buffer from 4,096 bytes to 1 MiB, and pairs the raise with a bound on the clients a seat
connects, in the same milestone, since one program opening many connections multiplies what a
lagging client can make Yuna hold. Nothing bounds that number today: a seat's clients are a list
that grows with each connection, and Yuna raises its limit of open files to the hard limit at
start.

What a lagging client costs, read in wayland-backend 0.3.17 on 2026-11-20:

- Its buffer grows by doubling before it flushes, zero-filled, so resident at once, and never
  shrinks: a healthy client keeps its largest burst of one round, a frozen one up to 1 MiB.
- Each descriptor an event carries is duplicated and kept with it: at 1 MiB, a client that reads
  nothing can make Yuna hold up to some 65,000 descriptors (a keymap event takes 16 bytes), where
  4 KiB held about 256.

None of KWin, Mutter, sway, Hyprland or niri bounds its number of clients (their sources read on
2026-11-20); X.Org's default is 256 clients, from memory. Yuna's tests open 100 clients on a seat
at most, and MCP runs 64 programs at most.

## Decision

The owner chose, on 2026-11-20:

- **256 clients at most on each seat**, Xwayland apart: a seat's own Xwayland is Yuna's, and
  refusing it would break the seat's X11.
- **At the bound**, a connection on a seat's socket is accepted and closed at once, without
  stopping the socket; Yuna logs it, at most once a second a seat, and MCP tells the seat's agent
  as an event. An in-process client (`Compositor::connect_client_to`) gets an error instead.
- **The worst case is accepted until Stage 2a bounds it**: with every client of a seat frozen,
  256 MiB of buffers and their descriptors. M2.12 merges a congested client's motion and knows
  which clients are congested; M2.15 brings per-client budgets; a bound per process follows
  Stage 2's client identity (ADR-0082 item 15).
- **Bursts of Yuna's own** do not grow a healthy client's buffer: MCP's typing flushes its
  seat's clients as it goes, and every event Yuna sends stays under 4,096 bytes, which wayland-rs
  clients cannot read past whatever the server's buffer (ADR-0136).

## Alternatives considered

- **128 a seat**: half the worst case, nearer the 30 to 60 clients of a busy desktop.
- **Congested clients bounded now**, M2.12's per-client flush brought forward: the worst case
  bounded at once, but half of M2.12's mechanism built without its merging, in a milestone that
  was not to hold it.
- **The raise postponed to M2.12**: no worst case without its guard, but a frozen game cut off
  after about 43 ms until then, against ADR-0136.

## Consequences

- MCP's events gain a kind, announced in the changelog
  ([ADR-0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)).
- The worst case is debt, in [`debt.md`](../debt.md), until M2.12 and M2.15.
- A test opens a seat's 257th client, in a process of its own, so that the other tests' clients
  and descriptors do not mix with it.

## Validation and reconsideration

Evidence: wayland-backend 0.3.17's `rs/socket.rs` and `rs/wire.rs`, the rivals' sources and the
challenge of M2.2's design, 2026-11-20. Checked by the story of the 257th client, and by MCP's
event. Proposed again if a real session nears the bound, or when M2.12 can bound the clients that
are congested.
