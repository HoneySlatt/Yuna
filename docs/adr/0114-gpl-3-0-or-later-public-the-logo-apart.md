# 0114. GPL-3.0-or-later, public; the logo apart

- Status: Accepted; supersedes [0009](0009-gpl-private-until-ready.md) and
  [0036](0036-yuna-is-public.md) (the repository's licence and visibility)
- Date: 2026-11-18

## Context

[ADR-0009](0009-gpl-private-until-ready.md) put Yuna under GPL-3.0-or-later and kept it private
until ready; [ADR-0036](0036-yuna-is-public.md) made it public. A reader needed both, and
ADR-0009 still read "private". This ADR gathers the rule as it stands on 2026-11-18; it decides
nothing new. The visibility of Yuna's fork of Smithay is
[ADR-0112](0112-smithay-pinned-patched-in-a-public-fork-left-at-stage-2.md)'s.

Opium, Slatty and Magnolia, the owner's other projects, are under GPL-3.0-or-later, and niri too;
Smithay is under MIT; jay and cosmic-comp under GPL-3.0-only.

ADR-0009 kept the repository private until the owner decided Yuna was ready; ADR-0036, later,
made it public, and its rule is the one below.

## Decision

- **Yuna's code is under GPL-3.0-or-later** (ADR-0009, 2026-11-11).
- **The repository is public**, since the owner made it so on 2026-11-13, with Yuna's fork of
  Smithay and SlimeEngine (ADR-0036, 2026-11-13).
- **The logo, `logo/yuna.svg`, is not under the code's licence**: its author keeps its rights
  ([`logo/README.md`](../../logo/README.md)) (ADR-0009; ADR-0036).
- **No code is copied from a GPL-3.0-only project**, which would make Yuna GPL-3.0-only (ADR-0009;
  [ADR-0113](0113-our-own-code-niri-jay-and-smithay-as-references.md)).
- Contributions come under the same licence, with no licence agreement
  ([ADR-0085](0085-outside-contributions-issues-now-pull-requests-from-0-1.md)).

## Alternatives considered

- **MIT OR Apache-2.0** ([ADR-0009](0009-gpl-private-until-ready.md)): anyone could take Yuna
  closed, and it would forbid reusing GPL code.
- **MPL-2.0** ([ADR-0009](0009-gpl-private-until-ready.md)): sharing required file by file only.
- **Private until ready** ([ADR-0036](0036-yuna-is-public.md)): the owner's first choice, changed.

## Consequences

- Everything committed is public at once: the global rules on personal data, attribution and the
  terms kept out of public repositories apply to every commit, issue and pull request.
- GitHub Actions' standard runners cost nothing for a public repository: CI's minutes are not
  counted, though CI stays one job while one is enough.
- A pull request from a first-time contributor runs CI only once the owner approves it, GitHub's
  setting at the time of writing.

## Validation and reconsideration

Reviewed reasoning, not measured. Contributors keep the rights to their code, so the licence can
change only with every contributor's consent (ADR-0085).
