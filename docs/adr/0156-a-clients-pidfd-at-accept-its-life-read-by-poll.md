# 0156. A client's pidfd at accept, its life read by `poll`

- Status: Accepted; supersedes [0154](0154-a-clients-pidfd-at-accept-through-so-peerpidfd.md)
- Date: 2026-11-23

## Context

[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 15 asks
that a client's identity come from its connection, the peer's pidfd recorded when the client
connects. ADR-0154 decided how; this ADR changes how Yuna checks that the pidfd's process lives,
and states the rule whole.

Yuna at `d43c648`:

- **It reads a client's pid with `SO_PEERCRED`** when it accepts the connection.
- **For an agent's seat, it walks the processes above the client in `/proc`**: the lineage and
  the process group.
- **MCP finds the client's program from that walk**, at each answer that names it. If the client
  exited before the accept and its pid was taken, the walk reads another process's lineage
  ([debt](../debt.md)).

The platform, read on 2026-11-22 and 2026-11-23:

- **`SO_PEERPIDFD`**, since Linux 6.5, gives a pidfd of the peer: it names that process and no
  other, even after its pid is reused. On 6.5 a peer already reaped gives `EINVAL`; later kernels
  give a pidfd of the dead process for a Unix socket. Its value is 77 on x86_64 and aarch64
  (Linux 7.3's `asm-generic/socket.h`, which both use; `libc` 0.2.191 gives other values for
  mips, powerpc and sparc alone).
- **The owner's machine runs Linux 7.3, CI's `ubuntu-24.04` runner 7.0**: both answer.
- **No library gives it.** rustix 1.1.6, the latest, has `pidfd_open` and `pidfd_send_signal`
  but no `SO_PEERPIDFD`. Calling `getsockopt` is `unsafe` whatever declares it.
- **rustix's `Signal` cannot be 0.** It holds a `NonZeroI32`, so `pidfd_send_signal` with no
  signal, ADR-0154's check, needs a second hand declaration and a second `unsafe` block, which
  [ADR-0151](0151-the-crate-map-checked-by-rustc-and-a-script-yuna-transitional.md)'s map does
  not let `yuna` add.
- **A pidfd reads as ready once its process has exited** (`pidfd_open(2)`), through rustix's safe
  `poll`.
- **Yuna already declares `close_range`, `sigaction` and `_exit` by hand**, each in an `extern`
  block with its `// SAFETY:` comment.

## Decision

The owner chose, on 2026-11-22 (the call) and 2026-11-23 (the check):

- **`getsockopt` declared by hand** in `yuna`'s process module, in the `extern` block that
  declares `close_range`, with `SO_PEERPIDFD`'s value, 77, for the architectures Yuna builds,
  x86_64 and aarch64.
  - Its call is one `unsafe` block with its `// SAFETY:` comment.
  - It is the one entry ADR-0151 lets the map add to `yuna`.
- **At accept, the pidfd fixes the client's own process.**
  - Yuna reads the client's `/proc` entry and its lineage, then polls the pidfd, without
    waiting. If it is not ready, the process had not exited, so its pid was not reused while
    Yuna read: what Yuna read was the client's own, its process group included.
  - If it is ready, the client had exited: it has no lineage and no program.
  - Then the pidfd is closed, so that no descriptor is added per client.
- **The program is resolved from the record made at accept, and kept with the client.** The
  compositor knows no programs; MCP does. MCP resolves each client the compositor accepted in the
  round that accepted it, and before any step that may forget a program, so that the result is
  the one an answer at the accept would have given. Every answer that names the client's program
  reads it there.
- **What stays inexact is said:**
  - Each parent read from `/proc` may still have been replaced. The pidfd fixes only the
    client's own process.
  - When the kernel refuses `SO_PEERPIDFD` (before 6.5), Yuna falls back to the walk alone and
    marks the result inexact.
  - A peer that exited before the accept has no program, whatever the kernel answers, a peer not
    yet reaped included.
- **X11 clients** connect to Xwayland, not to Yuna. They keep the pid XRes gives, which may be
  stale, written in the debt.

## Alternatives considered

- **`pidfd_send_signal` with no signal**, ADR-0154's check: it passes a peer that exited and was
  not yet reaped, which is as safe, but needs a second hand declaration and a second `unsafe`
  block, outside the one entry ADR-0151 allows.
- **`libc` as a direct dependency**, already in `Cargo.lock`: its constant is right on every
  architecture, but it is a new direct dependency, and the call stays `unsafe`.
- **`pidfd_open` on the `SO_PEERCRED` pid**: no `unsafe`, but it has the very race the pidfd
  removes.
- **The program resolved inside the accept**, through a resolver the compositor would hold:
  MCP's list of programs would be shared between the two, or copied into the compositor, which
  starts no programs.
- **The pidfd left to M2.18**, with the spawner of `yuna-process`: the debt stays open until then.
- **The pidfd kept for the client's life**: it would answer "is it alive?" later, which nothing
  asks; one more descriptor per client.

## Consequences

- M2.4c builds the call, the check, the fallback, the program resolved from the record, and a
  test that accepts both kernels' answers for a peer that already exited.
- `rest`'s and MCP's programs come from the record made at accept, not from a walk at each answer.
- M2.18's `yuna-process` takes the declaration with the rest of the process code.

## Validation and reconsideration

Evidence: Linux 6.5's `net/core/sock.c` and later sources, Linux 7.3's headers, `libc` 0.2.191,
rustix 1.1.6's `Signal` and `pidfd_send_signal`, `pidfd_open(2)`, the runner image's list of
2026-11-16. Checked by a test of a peer that exited before the accept, a test of the fallback
given the kernel's refusal, and the map's count of `unsafe`. Proposed again if rustix gains
`SO_PEERPIDFD`, which removes the hand declaration, or if Yuna builds for an architecture whose
value differs.
