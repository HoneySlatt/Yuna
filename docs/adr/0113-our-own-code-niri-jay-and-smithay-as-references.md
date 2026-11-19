# 0113. Our own code; niri, jay and Smithay as references

- Status: Accepted; supersedes [0013](0013-our-own-code-niri-and-jay-as-references.md) and
  [0039](0039-leaving-smithay-step-by-step.md) (Smithay as a reading reference)
- Date: 2026-11-18

## Context

[ADR-0013](0013-our-own-code-niri-and-jay-as-references.md) had Yuna write its own code, with niri
and jay as references. [ADR-0039](0039-leaving-smithay-step-by-step.md) added Smithay as a reading
reference when Yuna chose to leave it. A reader needed both. This ADR gathers the rule as it
stands on 2026-11-18; it decides nothing new.

niri is under GPL-3.0-or-later, as Yuna; jay under GPL-3.0-only, so that code copied from it would
make Yuna GPL-3.0-only ([ADR-0114](0114-gpl-3-0-or-later-public-the-logo-apart.md)); Smithay under
MIT.

## Decision

- **Yuna writes its own code**: the layout model, the animations, the colour pipeline and the rest
  (ADR-0013, 2026-11-11).
- **niri is the close reference** for scrolling, animations, the test clock and its testing method
  (ADR-0013). Its behaviours and test scenarios are read and rewritten as Yuna's tests; Yuna
  matches them, then goes past them by measure.
- **jay is a reading reference only**, for HDR, VRR, tearing and XWayland scaling (ADR-0013). No
  code is copied from it.
- **Smithay is a reading reference too** (ADR-0039, 2026-11-13). Under MIT, a passage close to its
  source is allowed, credited in a comment.
- **An agent that reads niri's or jay's code does not paste it**; a passage close to niri's source
  is rewritten or credited, and jay's is never copied (ADR-0013).

## Alternatives considered

- **niri's layout code taken whole** ([ADR-0013](0013-our-own-code-niri-and-jay-as-references.md)):
  the fastest, but a structure for one layout and one seat.
- **niri's proven parts taken, Yuna's own structure**
  ([ADR-0013](0013-our-own-code-niri-and-jay-as-references.md)): faster, at the cost of tracking
  what came from where.
- **jay's colour module taken** ([ADR-0013](0013-our-own-code-niri-and-jay-as-references.md)): a
  working base, but GPL-3.0-only, and tone mapping grafted onto another author's code.

## Consequences

- More time than reusing niri's code, for a structure designed for every layout and for agents'
  seats.

## Validation and reconsideration

Reviewed reasoning, not measured: the three projects' licences and code, read on 2026-11-11 and
2026-11-13.
