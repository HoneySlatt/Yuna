# 0008. Configuration in KDL

- Status: Accepted
- Date: 2026-11-11

## Context

niri's configuration is KDL; Umbriel's is TOML; Hyprland moved to Lua. The owner chose KDL for every
document of uwui (SlimeEngine's ADR-0241), with a reader of its own.

## Decision

Yuna's configuration is KDL 2.0. It reloads live; an error is shown, with its line and column, and
never stops Yuna, which keeps the last valid configuration. A NixOS module can write it. uwui's KDL
reader is reused once it is published on its own, if its licence and API fit; until then the choice
of reader is made at the milestone that needs it.

## Alternatives considered

- **TOML**: heavier for nested rules.
- **A scripting language (Lua)**: more power, but an error can break the session, and it is slower
  to read.

## Consequences

- One format across the owner's projects.
