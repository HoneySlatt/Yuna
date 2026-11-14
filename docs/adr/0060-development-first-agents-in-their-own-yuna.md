# 0060. Development first: agents never hold back for a game, each in its own Yuna

- Status: Accepted; amends 0003, 0015
- Date: 2026-11-14

## Context

Two rules kept agents away from the owner's games. Yuna's `AGENTS.md` let an agent run `--gpu`
only for a measure the owner consented to, one at a time, never beside a game; the owner's global
rules asked an agent to check that no game runs before reaching the desktop.
[ADR-0003](0003-a-parallel-project-uwui-first.md) ran the timed measures of all the owner's
projects one at a time.

The owner plays a lot, and agents work in the background. Under those rules, an agent either
waits or measures on llvmpipe, which draws 116 frames a second where the output asks 240.

Stage 3 was to put agents' outputs and seats inside the owner's desktop Yuna, peeked at from the
overview ([ADR-0015](0015-agents-workspaces.md)). Yuna's headless mode already runs on the GPU's
render node with `--gpu`, at full rate, with emulated VRR and HDR, driven through MCP, and in a
process of its own: a fault there ends the agent's Yuna, never the owner's session.

## Decision

The owner chose, on 2026-11-14, for all of the owner's projects:

- **The owner's activity sets the priority.** When the owner plays, the desktop is tuned for the
  game; when the owner develops, the work comes first. Agents never hold back for a game.
- **A timed measure checks its own conditions.** Before and while it runs, it reads the load other
  programs put on the GPU and the processor: a game, a build, another session. A run they disturb
  is marked so and reported to the owner, who stops the other work or has it measured again later.
  A measure the load does not affect, and every functional test, runs anyway.
- **An agent runs `--gpu` freely**, without asking. It still never runs it in tests or CI.
- **Stage 3 is removed.** Each agent runs its own headless Yuna, on the GPU when it measures; no
  agent's output or seat enters the owner's desktop Yuna. Watching an agent work goes to the PLAN's
  ideas. The later stages keep their numbers, which the ADRs cite; their versions follow their
  order: games 0.3, the image 0.4, effects 0.5.
- In Yuna itself, games and films still come first: no effect or animation may cost a game a frame
  or a film its cadence.

## Alternatives considered

- **Agents pause or lower their priority while a game runs**, told by the desktop Yuna's fullscreen
  state ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md)): the game untouched, but the
  work waits on the owner's play; the owner put development first.
- **Stage 3 as planned**: agents in the overview and one process, but a fault in an agent's path
  could end the owner's session, and an agent's frames would share the owner's compositor while
  the owner plays.
- **Measures run one at a time, by rule**: no measure disturbed, but agents wait on each other and
  on the owner; a measure that checks its own conditions keeps the figures honest without the wait.

## Consequences

- Yuna's `AGENTS.md` loses "never beside a game" and the consent for `--gpu`; the owner's global
  rules lose "check first that no game runs" and gain the priority and the measure's check.
- Each timed measure of Yuna, and of the owner's other projects, needs a check of other programs'
  load, written into its script; until a script has one, the agent reads the load itself and says
  so in the figures.
- Several agents run several headless Yunas; their memory is not measured yet.
- What ADR-0015 kept for the desktop goes: its peek from the overview and the change of the global
  rules at Stage 3. ADRs that wait for Stage 3 ([ADR-0022](0022-the-headless-backend-in-yuna.md)'s
  virtual outputs beside DRM ones, [ADR-0033](0033-the-mcp-server.md)'s agents' entry,
  [ADR-0035](0035-agents-seats.md)'s debt until Stage 3) and the debt items naming it are read
  again at Stage 1's audit.
- A game the owner plays may lose frames to an agent's work; the owner accepts it while developing.
