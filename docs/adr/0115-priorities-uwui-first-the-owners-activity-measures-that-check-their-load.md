# 0115. Priorities: uwui first, the owner's activity, measures that check their load

- Status: Accepted; supersedes [0003](0003-a-parallel-project-uwui-first.md) and
  [0060](0060-development-first-agents-in-their-own-yuna.md) (the priorities and the measures)
- Date: 2026-11-18

## Context

[ADR-0003](0003-a-parallel-project-uwui-first.md) started Yuna beside uwui, uwui first on any
conflict, and ran the timed measures of all the owner's projects one at a time.
[ADR-0060](0060-development-first-agents-in-their-own-yuna.md) put development first: agents
never hold back for a game, and a measure checks its own conditions instead of waiting. A reader
needed both. This ADR gathers the rule as it stands on 2026-11-18; it decides nothing new. That
each agent runs in its own Yuna, ADR-0060's other part, is
[ADR-0116](0116-agents-seats-each-agent-in-its-own-yuna.md)'s.

ADR-0003 also put uwui first for the machine, its timed measures running one at a time, each
other project's waiting. ADR-0060, later, rejected measures run one at a time: no measure waits
for another, and the owner, told of a disturbed run, decides what runs again. Its rule is
the one below.

## Decision

- **Yuna is a parallel project, with sessions of its own; uwui comes first** on any conflict of
  the owner's attention or of the subscription's quota (ADR-0003, 2026-11-11).
- **The owner's activity sets the priority** (ADR-0060, 2026-11-14). When the owner plays, the
  desktop is tuned for the game; when the owner develops, the work comes first. Agents never hold
  back for a game: a game the owner plays may lose frames to an agent's work, which the owner
  accepts while developing.
- **A timed measure checks its own conditions** (ADR-0060). Before and while it runs, it reads the
  load other programs put on the GPU and the processor: a game, a build, another session. A run
  they disturb is marked so and reported to the owner, who stops the other work or has it
  measured again later. A measure the load does not affect, and every functional test, runs
  anyway. Each timed measure's script reads the load; until a script does, the agent reads it
  itself and says so in the figures.
- **An agent runs `--gpu` freely**, without asking; never in tests or CI (ADR-0060). An agent's
  Yuna runs at normal priority, never lowered for a game
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
  item 24).
- **In Yuna itself, games and films come first**: no effect or animation may cost a game a frame or
  a film its cadence (ADR-0060).

## Alternatives considered

- **Yuna after SlimeEngine's Stage 6d, or after uwui's 1.0**
  ([ADR-0003](0003-a-parallel-project-uwui-first.md)): agents gain nothing meanwhile, or Yuna
  comes very late.
- **Agents pause or lower their priority while a game runs**
  ([ADR-0060](0060-development-first-agents-in-their-own-yuna.md)): the game untouched, but the
  work waits on the owner's play.
- **Measures run one at a time, by rule**
  ([ADR-0060](0060-development-first-agents-in-their-own-yuna.md)): no measure disturbed, but
  agents wait on each other and on the owner.

## Consequences

- Yuna's `AGENTS.md` and the owner's global rules say the same: no "never beside a game", no
  consent for `--gpu`, the priority and the measure's check.
- Several agents run several headless Yunas, on the GPU when they measure.

## Validation and reconsideration

Reviewed reasoning, not measured: the owner's choice of 2026-11-14, for all of the owner's
projects.
