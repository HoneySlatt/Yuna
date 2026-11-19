# 0136. A client that stops reading: 1 MiB of buffer, its pointer motion merged, never ended by motion

- Status: Accepted
- Date: 2026-11-19

## Context

wayland-rs's Rust backend, which Yuna uses, keeps 4,096 bytes of outgoing data per client and
ends a client whose next event does not fit once its socket is full
([ADR-0040](0040-the-wire-protocol-stays-wayland-rs.md)); KWin, Mutter, sway, Hyprland and niri
raise libwayland's to 1 MiB. [ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
left the answer to Stage 2's opening, on a measure. The ROADMAP's bar asks that a game frozen
50 ms while an 8,000 Hz pointer moves is not disconnected.

Measured headless on 2026-11-19 ([measurements](../measurements.md#stage-2s-opening-design-2026-11-19)):
the kernel holds a fixed number of sends in a Unix socket (278 sends of up to about 112 bytes, its
send buffer 212,992 bytes), so how long a client that stops reading lasts depends on how many
events each flush carries. At 8,000 Hz, with 28 bytes a motion: 53 ms with a flush a motion,
157 ms with one every 4, 402 ms with one every 33. A game that binds `relative-pointer` costs
60 bytes a motion: about 43 ms, estimated, under the bar. Under libinput at 8,000 Hz, each event
can wake the loop and be flushed alone.

wayland-server's cargo feature `libwayland_1_23` gives `Display::set_default_max_buffer_size` and
`Client::set_max_buffer_size`, done by the Rust backend itself; it links nothing. A flush of one
client, `Handle::flush(Some(id))`, returns `WouldBlock` when its socket is full, while its buffer
is still nearly empty; `flush_clients`, which Yuna calls, drops each client's error. The Rust
backend never waits for a socket to become writable. Its buffer doubles on demand, never shrinks,
and holds the duplicated file descriptors of the events it keeps.

[ADR-0092](0092-a-programs-protocol-trace-its-errors-and-its-log-through-mcp.md) notes that such a client leaves as `closed`;
the bar's scenario, as [ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
listed it, froze a game 50 ms. This ADR changes both.

Without merging, 1 MiB lasts 2.2 s of 8,000 Hz relative motion, 4.7 s of plain motion, then ends
the client as if it had left; KWin marks a window unresponsive after 0.5 s and Mutter offers to
end it after 5 s.

## Decision

The owner chose, on 2026-11-19, **1 MiB of buffer per client, and a client's pointer motion merged
while its socket is full, so that motion never ends a client.**

- **1 MiB.** Yuna turns on `libwayland_1_23` and sets every client's buffer to 1 MiB, in its own
  commit, with its story, at Stage 2a's interface milestone, where `wayland-server` becomes a
  direct dependency.
- **A congested client.** Yuna flushes each client by its id. A client whose flush returns
  `WouldBlock` is congested until a flush completes; a one-shot watch for writability on the
  client's second descriptor (the rest auditor's,
  [ADR-0096](0096-a-rest-auditor-naming-what-wakes-yuna-or-makes-it-draw.md)), armed only then, flushes it again when its socket drains. Nothing is
  armed at rest.
- **Its motion merged.** For a congested client Yuna keeps one pending pointer motion: the last
  position, relative and unaccelerated deltas summed (once `relative-pointer` comes, at 2c), axis
  values summed, the latest time. It is written before any other pointer event of that client, so
  that a click keeps its place, and when the client drains. Tablet events are never merged: a pen
  stroke keeps every point.
- **Ended past 1 MiB of other events, and named.** A client whose other events pass 1 MiB is
  ended, the one answer wayland-rs gives; Yuna logs it and MCP reports it with its own departure
  reason, not as a client that left.
- **Events bounded by their senders.** Each event of variable length is kept under 4,096 bytes by
  the code that sends it, as `text_input.rs` does, since the larger buffer no longer refuses one.
- **The bar's scenario** becomes: a game frozen 10 s while an 8,000 Hz pointer moves is not
  disconnected, and then gets the right last position; with `relative-pointer` at 2c, the summed
  motion too.

## Alternatives considered

- **1 MiB alone**: the rivals' figure, one line, the 50 ms scenario passed 44 times over; but a frozen
  game ends silently after 2 to 5 s of motion, and the memory of a burst is never given back.
- **wayland-rs on libwayland** (`server_system`): a wake on a writable socket and a ring buffer,
  but a C parser in front of untrusted input, `unsafe`, a system library, and no merging either.
  ADR-0040 allows it; it is not taken.
- **A patch in a fork** (reading how full the buffer is, a ring buffer): needless while the
  per-client flush tells congestion.
- **A buffer size per client role**: who needs room is whichever client is under the pointer; a
  smaller default cannot shrink once grown.
- **4 KiB kept**: the bar fails for a game with relative pointer.

## Consequences

- A lagging client can make Yuna hold up to 1 MiB and its descriptors; one program opening many
  connections multiplies it. The raise therefore comes with a bound on the clients a seat
  connects, in the same milestone; a bound per process follows Stage 2's client identity
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 15).
- A burst of other events, such as MCP's `type` of 4,096 keys (192 KiB), grows a healthy client's
  buffer, and wayland-rs never gives it back while the client lives.
- A frozen Wayland program under the pointer stays, unmarked, until a ping marks it unresponsive
  (frozen applications shown, Stage 2c, [ADR-0064](0064-what-the-best-desktops-taught-yuna.md)).
- The stories that rest on 4 KiB, in `tests/headless/misbehaving.rs` and `tests/headless/mcp.rs`,
  change with the raise: the one exception to Stage 2a's rule that tests keep what they assert.
  The debt's items on 4 KiB are paid by it.
- MCP's departure reason is a change of its contract, announced in the changelog.

## Validation and reconsideration

Evidence: the measurement of 2026-11-19 and its throwaway story; the challenge's reading of
wayland-backend 0.3.17 and of KWin, Mutter and niri; libwayland's wake on a writable socket is from
memory, not read. Checked by a headless story at Stage 2a: a client frozen through 10 s of
8,000 Hz motion, a flush per motion, is not ended and gets the right position; at 2c with relative
motion; at 2b on the laptop, Yuna's rounds, `sendmsg` calls and processor time at 8,000 Hz under
libinput. Proposed again if merging costs a frame or if another event floods a congested client.
