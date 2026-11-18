# 0092. A program's protocol trace, its errors and its log through MCP

- Status: Accepted
- Date: 2026-11-18

## Context

[ADR-0061](0061-yunas-uses.md) gives M1.11 each program's Wayland protocol trace and its log
through MCP, and the milestone is done when an agent finds a protocol error through MCP alone
([PLAN](../PLAN.md#m111--the-agents-toolbox)). An agent without a shell could not read a
program's log: `spawn` gave only its path. Facts read on 2026-11-17
([survey](../survey.md#the-agents-toolbox)):

- wayland-backend's Rust server, which Yuna uses
  ([ADR-0040](0040-the-wire-protocol-stays-wayland-rs.md)), switches its trace on for every client
  at once, when the display is made, to standard error; nothing sees one client's messages.
  libwayland's server has a protocol logger, which Weston uses; wayland-rs has none.
- Every protocol error, Yuna's, Smithay's or the backend's, ends in
  `ClientData::disconnected`, with its code, object, interface and message.
- libwayland and wayland-rs clients print their own trace with `WAYLAND_DEBUG=client`, and
  libwayland prints a protocol error's text on the client's standard error.

## Decision

The owner chose on 2026-11-18 not to fork wayland-rs before Stage 2, and the rest followed within
the milestone:

- **The trace is the program's own.** An agent sets `WAYLAND_DEBUG=client` in `spawn`'s `env`;
  the trace goes to the program's log. `spawn` has no argument of its own for it, which would be a
  second way to set one variable.
- **Yuna records each protocol error itself**, exactly, when the client is disconnected for one:
  its interface, object, code and message, beside the client's pid, its lineage and process group
  read when it connected, which name its program. `spawn list` gives each program's last error;
  the `events` tool tells each client that leaves, with its reason
  ([ADR-0093](0093-a-call-that-waits-for-the-seats-events.md)).
- **A log is read through MCP**: `spawn log` gives its bytes by offsets since the program started,
  as text, 64 KiB by default and 1 MiB at most a call. A log keeps its first 256 KiB, where a
  start-up failure shows, and its newest 1 MiB, where the end shows; what lies between is counted
  as dropped. 128 logs kept at most, 160 MiB on disk at worst.

## Alternatives considered

- **A fork of wayland-rs with an observer of each client's messages**: an exact trace of every
  client, Xwayland's included, and each client's wake-ups named for M1.11's rest auditor, but a
  second fork to carry until Stage 2's core, which may take it then with the per-client buffer
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)).
- **wayland-backend's own trace switched on**: every client's messages mixed on Yuna's standard
  error, which `yuna mcp` keeps for its own log, and their cost paid always.
- **The first 1 MiB of a log kept**, as before: a program that runs long loses its end, where the
  error usually is.

## Consequences

- A trace exists only for clients built on libwayland or wayland-rs, and only when asked before
  the program starts. X11 programs have none: their Wayland client is Xwayland.
- A client cut off by wayland-backend because it stopped reading leaves as `closed`, not as a
  protocol error: an agent tells it by the program still running.
- Proposed again at Stage 2's opening, with the per-client buffer, when a fork of wayland-rs is
  weighed anyway.
