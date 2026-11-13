# 0009. GPL-3.0-or-later, private until ready; the logo apart

- Status: Accepted; amended by [0036](0036-yuna-is-public.md) (the repository is public)
- Date: 2026-11-11

## Context

Opium, Slatty and Magnolia are under GPL-3.0-or-later; niri is too. Smithay is MIT. jay and
cosmic-comp are GPL-3.0-only.

## Decision

- Yuna's code is under **GPL-3.0-or-later**.
- The repository stays **private** until the owner decides Yuna is ready.
- The logo, `logo/yuna.svg`, is not under the code's licence: its author keeps its rights
  ([`logo/README.md`](../../logo/README.md)).

## Alternatives considered

- **MIT OR Apache-2.0**: anyone could take Yuna closed; it would also forbid reusing GPL code.
- **MPL-2.0**: sharing required file by file only.
- **Public from the start, unannounced**: free CI minutes, but the owner prefers private.

## Consequences

- No code is copied from a GPL-3.0-only project, which would make Yuna GPL-3.0-only
  ([ADR-0013](0013-our-own-code-niri-and-jay-as-references.md)).
- CI minutes are those of a private repository; CI stays lean.
