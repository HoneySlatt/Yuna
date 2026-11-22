# 0154. A client's pidfd at accept, through `SO_PEERPIDFD`

- Status: Accepted
- Date: 2026-11-22

## Context

[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 15 asks
that a client's identity come from its connection, the peer's pidfd recorded when the client
connects.

Yuna today, at `5b9fa13`:

- **It reads a client's pid with `SO_PEERCRED`** when it accepts the connection.
- **For an agent's seat, it walks the processes above the client in `/proc`**: the lineage and
  the process group.
- **MCP finds the client's program from that walk.** If the client exited before the accept and
  its pid was taken, the walk reads another process's lineage ([debt](../debt.md)).

The platform:

- **`SO_PEERPIDFD`**, since Linux 6.5, gives a pidfd of the peer: it names that process and no
  other, even after its pid is reused. On 6.5 a peer that already exited gives `EINVAL`; later
  kernels give a pidfd of the dead process for a Unix socket.
- **No library gives it.** No rival compositor takes it, and libwayland has no interface for it.
  rustix 1.1.6, the latest, has `pidfd_open` but no `SO_PEERPIDFD`. Calling `getsockopt` is
  `unsafe` whatever declares it.
- **Yuna already declares `close_range`, `sigaction` and `_exit` by hand**, each in an `extern`
  block with its `// SAFETY:` comment.

## Decision

The owner chose, on 2026-11-22:

- **`getsockopt` declared by hand** in `yuna`'s process module, in the `extern` block that
  declares `close_range`, with `SO_PEERPIDFD`'s value, 77, for the architectures Yuna builds,
  x86_64 and aarch64.
  - Its call is one `unsafe` block with its `// SAFETY:` comment.
  - It is the one entry
    [ADR-0151](0151-the-crate-map-checked-by-rustc-and-a-script-yuna-transitional.md) lets the
    map add to `yuna`.
- **At accept, the pidfd fixes the client's own process.**
  - Yuna reads the client's `/proc` entry and its lineage, then checks that the pidfd's process
    is still alive (`pidfd_send_signal` with no signal). If it is, the pid was not reused while
    Yuna read, so what Yuna read was the client's own, its process group included.
  - The program is resolved at accept and kept with the client.
  - Then the pidfd is closed, so that no descriptor is added per client.
- **What stays inexact is said:**
  - Each parent read from `/proc` may still have been replaced. The pidfd fixes only the
    client's own process.
  - When the kernel refuses `SO_PEERPIDFD` (before 6.5), Yuna falls back to today's walk and marks
    the result inexact.
  - A peer that exited before the accept has no program, whatever the kernel answers.
- **X11 clients** connect to Xwayland, not to Yuna. They keep the pid XRes gives, which may be
  stale, written in the debt.

## Alternatives considered

- **`libc` as a direct dependency**, already in `Cargo.lock`: its constant is right on every
  architecture, but it is a new direct dependency, and the call stays `unsafe`.
- **`pidfd_open` on the `SO_PEERCRED` pid**: no `unsafe`, but it has the very race the pidfd
  removes.
- **The pidfd left to M2.18**, with the spawner of `yuna-process`: the debt stays open until then.
- **The pidfd kept for the client's life**: it would answer "is it alive?" later, which nothing
  asks; one more descriptor per client.

## Consequences

- M2.4c builds the call, the check, the fallback, the program resolved at accept, and a test that
  accepts both kernels' answers for a peer that already exited.
- `rest`'s and MCP's programs come from the record made at accept, not from a walk at each answer.
- M2.18's `yuna-process` takes the declaration with the rest of the process code.

## Validation and reconsideration

Evidence: Linux 6.5's `net/core/sock.c` and later sources, rustix 1.1.6, libwayland 1.25, and the
rivals' sources, read on 2026-11-22. Proposed again if rustix gains `SO_PEERPIDFD`, which removes
the hand declaration, or if Yuna builds for an architecture whose value differs.
