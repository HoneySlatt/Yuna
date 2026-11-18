# 0097. The budgets of `yuna mcp`

- Status: Accepted
- Date: 2026-11-18

## Context

The ROADMAP's bar left `yuna mcp`'s start, its memory and many instances together to M1.11, and
an agent's Yuna's first frame with them. M1.11 measured them with `examples/instances.rs`
([measurements](../measurements.md)), on the owner's desktop (Ryzen 7 5800X), cold, with one seat
of 1920 × 1080 at 60 Hz, quiet runs only:

- from spawn to the seat's first frame, the longest of 20 starts: 60.9 ms on the GPU, 113.5 ms on
  llvmpipe; `seat create` answered within 52.2 and 88.7 ms;
- at rest, the process's own memory, `Pss_Anon` + `Pss_File` as
  [ADR-0077](0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md) reads it:
  83.3 MiB on the GPU, 116.6 MiB on llvmpipe, of which anonymous 19.4 and 40.7 MiB; the GPU's
  26.5 MiB at 1080p, 50.1 MiB at 4K;
- ten instances held five minutes: their anonymous memory did not move.

`Pss_File` shares Mesa's pages among every process that maps the same files: another program on
the same Mesa lowers Yuna's figure, by half in one disturbed run. The anonymous part does not move.

## Decision

The owner chose on 2026-11-18:

- **Start**: the seat's first frame within 100 ms of `yuna mcp`'s start on the GPU, 150 ms on
  llvmpipe; `seat create` answered within 100 ms.
- **Memory at rest, one seat**: the process's own at most 102.5 MiB on the GPU, the desktop's
  budget, and 125 MiB on llvmpipe, read with no other process on its Mesa; its anonymous part
  at most 25 MiB on the GPU and 45 MiB on llvmpipe, read on any machine; the GPU's at most
  30 MiB at 1080p and 55 MiB at 4K.
- **Many together**: ten instances grow nothing in five minutes at rest.

## Alternatives considered

- **The anonymous part alone**: robust on a busy machine, but it changes ADR-0077's measure.
- **ADR-0077's measure alone**: one measure for every budget, but a busy machine or CI cannot
  check it reliably.

## Consequences

- The start at 4K on the GPU was measured only on a disturbed machine (126 ms at the longest): it
  is measured again on a quiet one before the budget is judged there.
- Only the desktop was measured; the laptop and the CPU tier's lowest machine are not.

## Validation and reconsideration

Checked by `examples/instances.rs`. Proposed again at Stage 2's opening, when the core loads
nothing before its first frame that the frame does not need
([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 25),
or when a lower machine is measured.
