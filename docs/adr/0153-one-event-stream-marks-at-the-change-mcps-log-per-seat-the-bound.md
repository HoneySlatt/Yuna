# 0153. One event stream: marks where state changes, MCP's log per seat its bound

- Status: Accepted; supersedes in part
  [0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) (item 14, the
  event stream) and [0093](0093-a-call-that-waits-for-the-seats-events.md) (the log of events)
- Date: 2026-11-22

## Context

[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 14 asks
for one command type and one event stream for every caller, "events bounded per subscriber, a
resync event at the bound". [ADR-0146](0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md)
put the commands in `yuna-ipc` and left the events to M2.4. ADR-0093 decided the log that MCP's
`events` tool reads.

How events work today, at `5b9fa13`:

- **One log per agent's seat**, 1,024 events at most, numbered by one counter across the whole
  Yuna. An agent keeps its cursor, and `missed` counts what the bound dropped after it.
- **Numbering across seats makes `missed` approximate.** Other seats' events take numbers between
  a seat's own, so the log keeps runs of dropped numbers. Past 1,024 runs, the oldest merge, and a
  cursor inside a merged run is told at most that run's count.
- **Window and output events are told by comparison.** When a change counter moves, MCP describes
  every window and output and compares the result with its last look:
  - 4.4 ms at 1,000 windows;
  - a title changed each round, among 1,000 windows, costs a 240 Hz frame;
  - a window opened and closed between two looks is never told.
- **`windows` is not paged.** Past a few hundred windows, its reply passes an agent's limit of tool
  output.
- **The same fact has four or five shapes**: the compositor's description, its JSON, the event's
  data, and the comparison's private copies.
- **13 calls mark that something described changed** (`note_description_changed`), at the points
  where windows map, change, are arranged, and where outputs and X11 windows change.

What the field does:

- niri drops a subscriber past 64 queued events, and sends a new one the state as events first.
- Hyprland drops a subscriber past 64 events; sway past 4 MB.
- Mutter's display configuration signals "changed, read again", and checks a serial when a change
  is applied.
- No rival numbers its events.

[ADR-0138](0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md) gives each Yuna one seat from
M2.12: a filter by seat in the core would serve a shape that is leaving.

## Decision

The owner chose, on 2026-11-22:

- **Marks where state changes**, in `yuna` until `yuna-core` exists. The 13 change points, a window's map and unmap, and their X11
  equivalents mark what changed:
  - a window, with the fields that changed;
  - an output;
  - the focus;
  - a client's departure or refusal.

  A window's place comes from the layout's arrangement, which knows what moved.
- **Events built once a round.** At the round's end the core turns its marks into
  `yuna_ipc::Event`s, one per window and kind, repeats merged. So a title changed a thousand
  times in a round is one event. The cost follows what changed, not the number of windows. With
  no log to write to, no event is built.
- **`yuna_ipc::Event`**, `#[non_exhaustive]`, holds the kinds the compositor tells: window
  opened, changed, placed, unresponsive and closed; the focus; output added, changed and removed;
  a client left or refused.
  - Each event carries its seat.
  - No wire format until Hollow's interface (Stage 2c), so no serde.
  - A program's exit stays MCP's own event: the core starts no programs.
- **No queue per subscriber in the core.** MCP's log per seat is the bound: 1,024 events, read
  with a cursor the agent keeps. The resync ADR-0082 asked for is `missed`: a reply whose cursor
  fell off the log says how many events it missed. The reader then reads `windows` again. A
  title, an app id or a workspace is cut at 512 bytes in an event, as today. A
  reader that falls behind keeps the newest 1,024 events and their count, rather than a lone
  resync. Hollow will read a log of its own the same way.
- **Events numbered per seat.** Each seat's log numbers its own events, so `missed` is exact.
  This changes MCP's `events` contract, which said `seq` grows across the whole Yuna, and is
  announced by a `changed-` fragment ([ADR-0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)).
- **`windows` is paged** by a cursor, a window id, and a limit: a page lists the windows after
  the cursor, by id. A window opened meanwhile, with a larger id, comes on a later page; one closed
  meanwhile is not listed. With neither it lists every window, as today.
- **The comparison becomes a test oracle.** A property test applies random operations, folds the
  stream into a state, and compares it with what the compositor describes.

## Alternatives considered

- **A queue per subscriber in the core, emptied at its bound and followed by a `Resync` event**
  (ADR-0082's letter): a reader behind gets nothing but `Resync`, and the core needs a filter by
  seat, against ADR-0138.
- **Dropping a reader that falls behind**, as niri and Hyprland do: simple, but an agent's
  `events` call has no connection to drop.
- **An incremental comparison** of the windows marked dirty: less change, but a window opened and
  closed between looks stays unseen, and two shapes remain.
- **`seq` kept across the whole Yuna**, and `missed`'s inexactness left to M2.12's front for
  sibling Yunas: no contract change now, the debt left open.

## Consequences

- M2.4c builds the marks, `yuna_ipc::Event`, the round's construction, MCP's log on it, the
  numbering per seat, `windows`' paging, the oracle test, and the contract's files and fragments.
- MCP's `events` tells a window opened and closed within one round, and no longer describes every
  window to find what changed.
- The marks sit at change points of today's shared `State`, which M2.13 to M2.17 replace. Each
  replacement keeps marking at its own change points, and the oracle test says when one is missed.
- Hollow's events, their wire format and its own log come with Hollow's interface.

## Validation and reconsideration

Evidence: MCP's code and ADR-0093 at `5b9fa13`; niri 26.04's, Hyprland 0.57.1's and Mutter 51's
sources; a challenge of the first draft on 2026-11-22. Checked by the oracle's property test, a
story where a window opens and closes in one round, one where a title changes many times in a
round, a cursor older than the log told its exact count, and the parity script's figures. Proposed
again if a reader needs events the core does not mark, or if Hollow's interface needs a bound the
log cannot give.
