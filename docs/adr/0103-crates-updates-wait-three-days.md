# 0103. Crates' updates wait three days

- Status: Accepted
- Date: 2026-11-18

## Context

Yuna's crates come from crates.io, pinned in `Cargo.lock` and raised at each stage's end
([CONTRIBUTING](../../CONTRIBUTING.md#dependencies)). A release that is broken, or published by
someone who took over a crate, is usually yanked within days of its publication. Until Rust 1.100,
`cargo update` took the newest version whatever its age.

Cargo 1.100 reads a minimum age for new versions, `registry.global-min-publish-age`: with
`resolver.incompatible-publish-age = "deny"`, its default, `cargo update` leaves a version younger
than that age, unless `Cargo.lock` holds it already. Builds are unchanged: CI and the package build
with `--locked`.

On 2026-11-18, at Stage 1's end, five patch releases were available, published 0 to 6 days before:
`regex-syntax` and `smallvec` that morning, `rand` 3 days before, `cc` 5, `errno` 6.

## Decision

The owner chose, on 2026-11-18, among no delay, 7 days and 14 days, a delay of its own:

- **A crate's new version waits three days** before `cargo update` takes it:
  `global-min-publish-age = "3 days"` in the repository's `.cargo/config.toml`, which every
  contributor's Cargo reads.
- **A fix for an advisory is taken at once**, the delay lifted for that one update:
  `CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE=allow cargo update -p <crate>`.

## Alternatives considered

- **No delay**: updates as fresh as possible, but a release yanked a day later may already be in
  `Cargo.lock`.
- **Seven or fourteen days**: a longer guard, but every update a week or two behind, at a stage's
  end that comes every few weeks.

## Consequences

- At a stage's end, the newest few days of releases wait for the next one.
- Git dependencies (Smithay's fork) are not concerned: they are pinned to a commit and raised by
  hand ([ADR-0017](0017-smithays-revision.md)).
- A Cargo older than 1.100 ignores the setting; `rust-toolchain.toml` names 1.100.0
  ([ADR-0016](0016-the-rust-toolchain.md)).

## Validation and reconsideration

Measured on the day: three of the five updates passed the delay, two waited. Proposed again if an
advisory's fix is held back by the delay without anyone noticing, or if the delay keeps a needed
fix out for a whole stage.
