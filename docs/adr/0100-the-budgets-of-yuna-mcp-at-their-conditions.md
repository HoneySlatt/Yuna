# 0100. The budgets of `yuna mcp`, at their conditions

- Status: Accepted; supersedes [0097](0097-the-budgets-of-yuna-mcp.md)
- Date: 2026-11-18

## Context

[ADR-0097](0097-the-budgets-of-yuna-mcp.md) set `yuna mcp`'s budgets from M1.11's quiet
measures, all of one seat at 1920 × 1080 with no program open, but did not say so. M1.11's
closing review found that its own figures miss them elsewhere ([measurements](../measurements.md)):

- a seat's default output is 3840 × 2160; there, on llvmpipe, `yuna mcp`'s anonymous memory is
  64.8 MiB, against 45;
- with one program open, 45.3 MiB anonymous on llvmpipe, and 36.9 MiB of the GPU's on the GPU,
  against 30: the program's buffers, which depend on the program.

`Pss_File` shares Mesa's pages among every process that maps them, so another program on the same
Mesa lowers Yuna's figure; the anonymous part does not move.

## Decision

The owner chose on 2026-11-18 that the budgets name their conditions, and that the default output
has its own:

- **Start**, from `yuna mcp`'s start to its seat's first frame: within 100 ms on the GPU, 150 ms on
  llvmpipe; `seat create` answered within 100 ms. One seat, cold, the longest of 20 starts.
- **Memory at rest, one seat at 1920 × 1080, no program**: the process's own, `Pss_Anon` +
  `Pss_File` with no other process on its Mesa, at most 102.5 MiB on the GPU and 125 MiB on
  llvmpipe; its anonymous part at most 25 MiB on the GPU and 45 MiB on llvmpipe; the GPU's at most
  30 MiB.
- **At the default output, 3840 × 2160, no program**: the anonymous part at most 25 MiB on the GPU
  and 70 MiB on llvmpipe; the GPU's at most 55 MiB.
- **With programs open**: no budget, their buffers being theirs.
- **Many together**: ten instances grow nothing in five minutes at rest.

## Alternatives considered

- **Every budget at the default output**: one condition, but the tighter figure at 1080p, which
  catches a regression sooner, lost.
- **The conditions written alone, no budget at 4K**: the default case left without a figure.

## Consequences

- The start at 4K on the GPU was measured only on a disturbed machine, 126 ms at the longest: it is
  measured again on a quiet one; the laptop and the lowest machine are not measured
  ([debt](../debt.md)).
- ADR-0097's figures stand; only their conditions and the 4K row are added.

## Validation and reconsideration

Checked by `examples/instances.rs`, at each condition. Proposed again at Stage 2's opening, when
the core loads nothing before its first frame that the frame does not need
([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 25),
or when a lower machine is measured.
