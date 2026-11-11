# 0018. Workspaces: named, with dynamic ones as an option

- Status: Accepted
- Date: 2026-11-11

## Context

The owner's Umbriel has named workspaces 1 to 10, each on its own monitor (odd ones on the 4K,
even ones on the 1080p), switched with Mod+1 to Mod+0 and with next and previous. niri has dynamic
workspaces instead: unnamed, created and removed on their own, one empty workspace always last on
each monitor; named workspaces are an option there ([survey](../survey.md)). Agents create and
remove workspaces of their own through MCP ([ADR-0015](0015-agents-workspaces.md)).

## Decision

Both, chosen by configuration (`Options::dynamic_workspaces` in `yuna-layout`):

- **Named workspaces always exist.** The configuration declares them, each with its home output,
  even one not plugged in yet; switching to an unknown name creates it on the seat's active
  output. They stay, empty or not, until removed.
- **Unnamed workspaces** hold what has no name: an output with no workspace gets one, and they go
  as soon as they are empty and not shown.
- **Dynamic workspaces**, when the option is on, keep one empty unnamed workspace last on every
  output, as niri does.
- Workspaces of an output are stacked vertically: up is the previous one, down the next.
- **Homes** follow niri: a removed output's workspaces move to another output of its seat and
  come back when it returns, the one it showed shown again. A window opening on an unnamed
  workspace makes its current output its home; a named workspace keeps the home it was declared
  with (niri since 25.02). An empty unnamed workspace does not come back.
- Names belong to a seat: an agent's workspace "1" is not the owner's.

## Alternatives considered

- **Named only, as Umbriel**: the owner's habits, but no endless stack for those who like niri's.
- **Dynamic only, as niri**: fluid, but the owner's Mod+1 to Mod+0 change meaning, and agents need
  names to address their workspaces.

## Consequences

- Both models are tested, with the option on and off, in the property tests and the scenarios.
- More behaviours to polish than one model alone: this was the owner's choice, against the
  recommendation of named workspaces alone.
- Moving a workspace to another output, reordering workspaces and renaming them are not built yet;
  they come when a milestone needs them.
