# 0017. Smithay's revision: upstream, pinned, first at master of 2026-11-11

- Status: Accepted
- Date: 2026-11-11

## Context

[ADR-0011](0011-built-on-smithay.md) builds Yuna on Smithay at a pinned revision, raised at each
stage's end. Smithay's last release, 0.7.0, dates from 2025-06-24; compositors build on git
revisions. On 2026-11-11, from Smithay's, niri's and cosmic-comp's repositories on GitHub (their
commits, `Cargo.toml`, `Cargo.lock` and CI runs):

- master was `8fd9fe0c74e5c13a1fc9b6797d3960c8381bda55` (2026-11-11). Its tests passed in Smithay's
  CI; only the publication of its documentation failed.
- niri pinned `65aa86f` (2026-11-04), 11 commits behind master; cosmic-comp pinned `8a27023`
  (2026-10-14), 39 behind.
- The 11 commits after niri's revision guard against negative or zero sizes sent by a client, fix
  two XWayland bugs, change how input grabs are locked, and raise Smithay's minimum Rust to 1.88.

## Decision

- Yuna depends on Smithay's upstream repository at **`8fd9fe0c74e5c13a1fc9b6797d3960c8381bda55`**,
  in `[workspace.dependencies]`, without its default features; each crate turns on what it uses.
- The `yuna` crate depends on it from M1.1, before it uses it: `Cargo.lock` holds the exact commit,
  and `cargo deny` checks its licences, advisories and sources from the start.
- The dependency moves to Yuna's fork of Smithay with the first patch (ADR-0011), listed in
  `docs/smithay-patches.md`.
- Raising the revision at a stage's end is routine: the commit that raises it says why, and this
  ADR stays as it is. Only a change of policy needs a new ADR.

## Alternatives considered

- **niri's revision, `65aa86f`**: tried daily by niri's users, but without the guard against bad
  sizes, which [ADR-0007](0007-surviving-a-crash.md) wants, nor the XWayland fixes.
- **The 0.7.0 release**: a version number, but 17 months behind, used by no compositor; M1.4 would
  begin by catching up on a year and a half of API changes.

## Consequences

- The newest base for the fork, but its input grab change has no daily use behind it yet.
- An unused dependency until M1.4; it builds in seconds without features.
