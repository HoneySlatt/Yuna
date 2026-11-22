# 0093. A call that waits for the seat's events, and programs made stuck or slow

- Status: Accepted; superseded in part by [0153](0153-one-event-stream-marks-at-the-change-mcps-log-per-seat-the-bound.md) (the log of events, and how its events are found)
- Date: 2026-11-18

## Context

[ADR-0061](0061-yunas-uses.md) gives M1.11 a call that waits for an event, instead of a loop that
asks, and situations made on demand, a slow or stuck client among them. Until then only `windows`
waited, for a window that matches. MCP's revision 2026-07-28 keeps no state between requests
([ADR-0033](0033-the-mcp-server.md)), and
[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) asks events
bounded per subscriber, with a resync at the bound, and nothing new built on today's shared
`State`.

## Decision

Chosen while building M1.11, as implementation within ADR-0061:

- **A log of events per seat**, 1,024 at most, each with a sequence number unique in the Yuna:
  windows opened, closed, changed, placed, unresponsive; the focus; programs that exit; clients
  that leave, with the protocol error they were ended for
  ([ADR-0092](0092-a-programs-protocol-trace-its-errors-and-its-log-through-mcp.md)); outputs
  added, removed, changed. Texts are cut at 512 bytes.
- **`events` reads it after a cursor the agent keeps**, 256 a reply, with the next cursor and how
  many events the bound dropped since the cursor; without a cursor, from now. With `wait_ms`, up
  to 50 s on real time, it waits for the first that matches; a cursor that already missed events
  is answered at once. Its waits count among the 64 waiting calls, fail when the seat closes and
  are dropped by a cancellation. The server keeps nothing of the agent's between calls.
- **Events are told by comparing what the compositor describes with the last look**, not by hooks
  in the state: a change counter says when anything they report may have changed, so that a round
  where clients only draw describes nothing, and a look tells each change once. A window opened
  and closed between two looks is not told.
- **`spawn freeze`, `thaw` and `slow`** stop and continue a program's process group; `slow` lets
  it run a given share of each 100 ms, on real time. SIGCHLD ignores stops and continues, so that
  pacing does not wake Yuna more than its own timer, and every ending sends SIGCONT after
  SIGTERM, so that a frozen program ends at once.
- **`yuna mcp` locks a file in its folder**, and at start removes the folders of others whose lock
  it can take, the folders a `yuna mcp` killed by SIGKILL left.

## Alternatives considered

- **A `wait_ms` on each tool**: the PLAN's idea for `keyboard`, but one waiting rule per tool, and
  nothing to wait on for a program's exit or a protocol error.
- **Hooks in the shell, the layout and the outputs that record each event**: nothing missed, but
  many places of today's shared `State` that Stage 2's core replaces.
- **A subscription kept by the server**: MCP's revision forbids state across requests.
- **A cgroup's CPU limit for `slow`**: exact and without wake-ups, but it needs systemd's
  delegation, which CI and a session without systemd lack
  ([ADR-0037](0037-seats-tools-input-and-closing.md)).
- **Old folders removed by their pid alone**: a live Yuna in another pid namespace looks dead.

## Consequences

- `events` and `spawn`'s new actions are part of the MCP contract
  ([ADR-0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)).
- After a change, a look costs about the square of the windows, the layout model searching each
  window: 4.4 ms at 1,000 windows ([measurements](../measurements.md), [debt](../debt.md)).
- A `yuna mcp` killed by SIGKILL still leaves its programs running; its folder goes at the next
  start.

## Validation and reconsideration

Evidence: tests of each kind of event, of a wait answered and timed out, of a cursor too old, of
1,500 titles giving one event, of a cancellation shown failing when broken, of a frozen and a
slowed program and of the folders' sweep, each mutation that breaks them caught. Proposed again
when Stage 2's core gives one event stream for every caller
([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 14):
`events` then reads it.
