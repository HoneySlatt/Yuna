# 0140. Rust 1.100's features: what Yuna takes

- Status: Accepted
- Date: 2026-11-19

## Context

Rust 1.100.0, Yuna's toolchain since Stage 1's audit
([ADR-0125](0125-the-rust-toolchain-an-exact-stable-version-a-nightly-for-fuzzing.md)), makes
stable, among others: custom allocators (`allocator_api`: `Allocator`, `Vec<T, A>`, `Box<T, A>`),
`core::mem::DropGuard`, `bool::toggle`, `trim_prefix` and `trim_suffix` on strings and slices
(`trim_prefix` on `Path` too), `Vec::from_fn`, and the never type `!` in every position (`Infallible` becomes its
alias). Each was put to the owner on a measure, on 2026-11-19
([measurements](../measurements.md#stage-2s-opening-design-2026-11-19)):

- Allocations a frame, by heaptrack on Yuna itself, on llvmpipe at 1080p: 83 a still frame and
  404 KiB, 91 and 390 KiB in motion, none at rest; half are vectors rebuilt in the elements' path
  that Stage 2a's scene replaces.
- Sites in Yuna's code at `a551df7`: 3 exact `x = !x`; of 24 `Drop` implementations and one
  clean-up by hand, 1 better as a `DropGuard`, 4 neutral, 20 worse; 0 `strip_*` call that
  `trim_*` improves (all need to know whether the pattern was there); `Vec::from_fn` 8 neutral,
  7 worse; 0 `Infallible`.

## Decision

The owner chose, on 2026-11-19:

- **No custom allocator.** Stage 2a's scene and renderer reuse their lists and buffers; the target
  is no heap allocation in Yuna's own code on a still or a moving frame, measured by heaptrack in
  the parity script. An allocator comes back only on a measure after that.
- **`bool::toggle`** replaces the three `x = !x`, and is the form for new code.
- **`DropGuard`** where a guard cleans up one value; a named type otherwise. The X11 display's lock
  file is the one site changed; no sweep.
- **`trim_prefix`, `trim_suffix` and `Vec::from_fn`** are allowed where clearer; existing code is
  not changed.
- **`!`** in new code where an operation cannot fail (`Result<T, !>`), rather than `Infallible`.

## Alternatives considered

- **An arena reset each frame for the drawing path's vectors**: gain unmeasured, an allocator
  parameter spread through the lists' types, `unsafe` or a new dependency.
- **Sweeps of every site**: changes with no gain where the measure found them neutral or worse.

## Consequences

- The parity script of Stage 2a counts allocations a frame.

## Validation and reconsideration

Evidence: the measures above. Proposed again if allocations a frame stay above zero in Yuna's own
code once Stage 2a's renderer is the default, and a profile shows them costing.
