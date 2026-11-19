# 0104. Stage 1's exit: Opium in Aerith's place, and its exceptions written

- Status: Accepted; amends [0025](0025-applications-in-ci.md); superseded in part by
  [0122](0122-real-applications-in-ci-and-rest-checked-on-the-program.md) (Opium's weekly run)
- Date: 2026-11-18

## Context

Stage 1's exit asked that SlimeEngine's and Aerith's windowed examples, and uwui's once it has
some, run in Yuna headless in CI, driven by an agent through MCP
([ROADMAP](../ROADMAP.md#stage-1--the-headless-core--01)). Stage 1's audit found, on 2026-11-18:

- SlimeEngine's `window`, `editor` and `interface` run in Yuna in a weekly job, `editor` and
  `interface` driven through `yuna mcp` ([ADR-0025](0025-applications-in-ci.md),
  [ADR-0043](0043-text-input-and-the-rest-of-slimeengines-examples.md)).
- Aerith, the owner's media player library, has no window of its own: its `aerith-player`
  example plays without an interface, and nothing ran it in Yuna.
- Opium, the owner's music player, has a desktop window, drawn with egui, and a Nix package
  whose `bin/opium` starts it. Its repository is public.
- Two exceptions the owner had decided were missing from the exit's text: sway's IPC, which Yuna
  does not offer ([ADR-0089](0089-yuna-for-the-owners-other-projects.md)), and tabbed columns,
  added to the layouts for Stage 2 ([ADR-0062](0062-the-compositors-comfort-and-look.md)).

## Decision

The owner chose, on 2026-11-18, at the stage's audit:

- **Opium takes Aerith's place in Stage 1's exit**, Aerith having no window. Opium's desktop
  program, built from its Nix package at a revision Yuna pins, runs in Yuna in a weekly job, as
  SlimeEngine's examples do: an agent, through `yuna mcp` alone, opens it on a throwaway profile,
  finds its window, acts on it and checks it by capture.
- **The exit names its exceptions**: terminal swallowing and per-application rules, which need
  clients, tabbed columns ([ADR-0062](0062-the-compositors-comfort-and-look.md)), all at Stage 2;
  and sway's IPC, which has no equivalent in Yuna
  ([ADR-0089](0089-yuna-for-the-owners-other-projects.md)).

## Alternatives considered

- **Aerith's example run in the weekly job**: it would run, but test no window, which is what the
  clause is for.
- **The clause dropped until Stage 2**: quicker, but 0.1 would prove nothing for a second
  application of the owner's.

## Consequences

- Yuna's CI builds Opium at its pinned revision once a week and on demand; the revision is
  raised by hand, as SlimeEngine's is.
- Opium is frozen until it moves to uwui (Opium's ADR-0026): its window changes rarely, and the
  job follows its move when it comes.

## Validation and reconsideration

Checked by the weekly job's run on its first push. Proposed again if Opium's window moves to uwui
and uwui's own examples can take its place.
