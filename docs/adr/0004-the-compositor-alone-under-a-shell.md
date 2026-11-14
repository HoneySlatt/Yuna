# 0004. The compositor alone, a shell on top

- Status: Accepted; amended by 0057, 0063
- Date: 2026-11-11

## Context

The owner's bar, launcher, notifications, lock and login screens come from Amane, an external shell
patched in the owner's NixOS configuration, which today reads Umbriel's workspaces through a patch.
uwui's targets exclude the desktop shell (SlimeEngine's ADR-0199).

## Decision

Yuna is a compositor alone. It speaks the standard protocols a shell needs (`wlr-layer-shell`,
`ext-session-lock-v1`, `ext-workspace-v1`, `ext-foreign-toplevel-list-v1`, foreign toplevel
management, image capture), so Amane or any other shell runs on top without a patch. Yuna draws
only what must be inside a compositor: the overview, transitions, borders, effects, the screenshot
tool, Alt-Tab.

A shell of the owner's own, on uwui, may come later; it is decided after uwui's first stage.

## Alternatives considered

- **A compositor and its own shell now**: a whole desktop of the owner's, but a second large project,
  a lock screen whose security becomes Yuna's, and a reversal of ADR-0199.

## Consequences

- Amane's patch for Umbriel's workspaces is replaced by `ext-workspace-v1`.
- The parts Yuna draws itself are written without uwui.
