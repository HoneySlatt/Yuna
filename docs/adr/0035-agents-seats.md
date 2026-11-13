# 0035. Agents' seats in one Yuna

- Status: Accepted
- Date: 2026-11-13

## Context

[ADR-0033](0033-the-mcp-server.md) gives each MCP client a Yuna of its own; a client may still run
several agents on it, Claude Code's subagents sharing their parent's servers, and MCP forbids
keeping state on the connection. [ADR-0015](0015-agents-workspaces.md) gives each agent its own
seat. Today every window opens on the owner's seat. Upstream Smithay announces each `wl_seat`
global to every client and offers no filter per client; GLFW, SDL2 and vkcube take the first seat
they see ([survey](../survey.md#agents-and-mcp)).

## Decision

The owner chose, on 2026-11-13:

- An agent creates a seat through MCP and gets its **handle**, which every tool that acts on a
  seat takes. A stale handle fails plainly.
- Each seat has **its own Wayland socket**. A program the agent starts gets it as
  `WAYLAND_DISPLAY`; every client connected through it belongs to that seat, its windows open on
  that seat's workspaces, and only that seat's input reaches them. A program that forks keeps its
  seat.
- A seat starts with an output of its own, drawn at full rate.
- In `yuna mcp` the owner's seat announces no `wl_seat`: an agent's programs see only agents'
  seats.
- `wl_seat` is not filtered per client until Stage 3, which needs the same filter for outputs on
  the owner's desktop: main stays on upstream Smithay ([ADR-0022](0022-the-headless-backend-in-yuna.md)).
- When its seat is closed, or its Yuna stops, an agent's programs are ended, their whole tree
  under Yuna as subreaper, and its outputs and workspaces removed.

## Alternatives considered

- **Main on Yuna's fork of Smithay now**, with a generic filter of globals per client: several
  seats right from M1.5, but a fork to rebase at each raise of Smithay and a read key to the
  private fork in CI ([ADR-0021](0021-smithays-fork.md)), for a case ADR-0033 makes rare.
- **One connection, one seat**: MCP forbids it, and subagents would share a seat.

## Consequences

- With two seats in one Yuna, a program of the second may bind the first's `wl_seat`: it then
  gets no input, and never another seat's. A test pins that no input crosses seats; the gap is
  debt until Stage 3.
- Each seat has its own keyboard focus, and the selections and activation follow it. The caps of
  [ADR-0028](0028-bounds-on-what-one-client-may-ask.md) are unchanged: per client, and shared by
  all seats for the whole layout.
