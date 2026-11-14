# 0015. Agents' workspaces

- Status: Accepted; amended by 0060
- Date: 2026-11-11

## Context

The owner's agents must test graphical programs without touching the owner's screen, input or focus.
Compositors throttle what is not shown: niri sends frame callbacks to hidden windows about once a
second; Hyprland only to the shown workspaces; KWin suspends hidden windows unless held for
offscreen rendering. No compositor renders a hidden workspace at full rate by default
([survey](../survey.md#hidden-workspaces)). Today's global rules forbid agents to inject input into
the owner's session.

## Decision

- **In the headless mode** and **on the desktop**, each agent gets its own workspaces on a virtual
  output, never shown unless the owner asks, and **its own seat**: its pointer, keyboard and focus,
  apart from the owner's.
- These outputs are **drawn for real on the GPU, at full rate** (3840 × 2160 at 240 Hz by default),
  with frame callbacks and presentation feedback as on a shown output, so that measures hold.
- Agents drive them through **Yuna's MCP server**: start programs, capture, type, click, read the
  presentation times.
- **The owner can peek** at an agent's workspace, in a thumbnail or in the overview.

## Alternatives considered

- **Hidden workspaces throttled, as elsewhere**: measures taken there would be wrong.
- **Agents on the owner's seat**: their input would reach the owner's windows.

## Consequences

- The global rules for agents in the owner's NixOS configuration change at Stage 3, on the owner's
  go, to allow this path and only this one.
- An agent's output costs GPU time and power while it draws; it draws only while its programs do.
