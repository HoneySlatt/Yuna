# 0130. OLED care chosen at Stage 5's design, from a survey

- Status: Accepted; supersedes in part [0062](0062-the-compositors-comfort-and-look.md) (point 8,
  OLED care)
- Date: 2026-11-18

## Context

[ADR-0062](0062-the-compositors-comfort-and-look.md) point 8 made an OLED wear map the method of
OLED care, at Stage 5. The owner reopened that point the same day, and the reopening was written
only into ADR-0062's status, which the ADRs' index cannot show: a reader of the index, or of
ADR-0062's decision, saw the wear map as chosen. The [ROADMAP](../ROADMAP.md) already reads as the
owner reopened it.

This ADR states that rule. It decides nothing new.

## Decision

- **OLED care comes at Stage 5**: the panel's luminance read, static elements protected from
  burn-in ([ROADMAP](../ROADMAP.md)).
- **Its means are chosen at Stage 5's design, from a survey** of what panels, other systems and
  studies of burn-in do (ADR-0062, point 8 reopened by the owner on 2026-11-14).
- **A wear map is one candidate** (ADR-0062): per output, how long each region has shown the same
  bright, still content, to choose what to dim slowly or shift by a pixel, read only while Yuna
  draws anyway, never at rest.

## Alternatives considered

- **The wear map chosen now** ([ADR-0062](0062-the-compositors-comfort-and-look.md) point 8, as
  first written): one method fixed before the survey that compares it with others.

## Consequences

- Stage 5's design includes the survey, and an ADR of its own for the means it chooses.

## Validation and reconsideration

Reviewed reasoning, not measured. Proposed again at Stage 5's design, with its survey.
