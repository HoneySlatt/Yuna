# 0143. Allocations a frame, counted by valgrind's DHAT

- Status: Accepted; supersedes in part [0140](0140-rust-1-100s-features-what-yuna-takes.md) (how allocations a frame are counted)
- Date: 2026-11-19

## Context

[ADR-0140](0140-rust-1-100s-features-what-yuna-takes.md) set the target of no heap allocation in
Yuna's own code on a still or a moving frame, "measured by heaptrack in the parity script". The
parity script runs at each push in CI ([ADR-0142](0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)),
in a development shell of its own, `.#parity`, which holds valgrind for the instructions (the owner's
go, 2026-11-19).

A trial on 2026-11-19 (`.delegation/m2.1-vg-trial/`, local) ran both tools on the `yuna` process
itself, `yuna mcp --manual-clock` with two `vkcube` windows at 1080p on llvmpipe, three pairs of
runs of 20 and 120 advances:

- **valgrind's DHAT**: 167 blocks a frame, spread 2.95 %; 95 % of them pass through the `yuna`
  binary's own code. Under valgrind every advance draws one frame, the clients keeping up.
- **heaptrack**: 176 a frame, spread 8.2 %: at its speed the clients do not keep up with the clock,
  and the frames an advance vary. Its package in nixpkgs, with its Qt and KDE interface, is a
  1.2 GB closure; without the interface it must be built from source at each CI run.
- DHAT runs 30 to 100 times slower than Yuna alone; heaptrack about 2 times.

## Decision

The owner chose, on 2026-11-19: **allocations a frame are counted by valgrind's DHAT** on the
`yuna` process, in the parity script, with the instructions. The target stays ADR-0140's: no heap
allocation in Yuna's own code on a still or a moving frame. heaptrack stays a tool for an analysis
by hand; it is not in the project's shells.

## Alternatives considered

- **heaptrack, its whole package**: ADR-0140's tool, faster; 1.2 GB more for every parity run, and
  a figure three times noisier until the clients commit in step with the clock.
- **heaptrack built without its interface**: light, but built from source at each CI run, and a
  package override to keep.
- **A counting allocator in Yuna**: no tool at all, but code in the program for a measure, and it
  sees only Rust's allocations.

## Consequences

- The parity shell holds valgrind alone for both figures.
- A frame's figure is taken only where every advance draws, and is compared between builds counted
  by the same tool, never against a heaptrack figure: the two disagree by 15 to 30 % on bytes.

## Validation and reconsideration

Evidence: the trial's figures above. Proposed again if DHAT's time makes the parity job miss its
time in CI, or if its figure's spread stays above the threshold the script needs once the clients
commit in step.
