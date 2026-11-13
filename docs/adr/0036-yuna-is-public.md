# 0036. Yuna is public

- Status: Accepted; amends [0009](0009-gpl-private-until-ready.md) (the repository's visibility),
  [0021](0021-smithays-fork.md) (the fork's visibility)
- Date: 2026-11-13

## Context

[ADR-0009](0009-gpl-private-until-ready.md) kept the repository private until the owner decided
Yuna was ready, and CI lean, its minutes counted. [ADR-0021](0021-smithays-fork.md) kept Yuna's
fork of Smithay private too, and foresaw a read key for CI once main used it.

## Decision

The owner made the repository public on 2026-11-13, with Yuna's fork of Smithay,
`HoneySlatt/smithay`, and SlimeEngine. Yuna stays under GPL-3.0-or-later; the logo stays apart.
The fork is a repository of its own, not a GitHub fork in Smithay's network.

## Alternatives considered

- **Private until ready**, as ADR-0009 had it: the owner's choice changed.

## Consequences

- Everything committed is public at once: the global rules on personal data, attribution and
  the terms kept out of public repositories apply to every commit, issue and pull request.
- GitHub Actions' standard runners cost nothing for a public repository: CI's minutes are no
  longer counted, though CI stays one job while one is enough.
- CI can fetch the fork without a key; main still stays on upstream Smithay while no code needs a
  patch ([ADR-0022](0022-the-headless-backend-in-yuna.md)).
- A pull request from a first-time contributor runs CI only once the owner approves it, GitHub's
  setting at the time of writing.
