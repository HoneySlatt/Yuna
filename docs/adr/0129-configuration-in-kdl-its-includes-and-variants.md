# 0129. Configuration in KDL, its includes and variants

- Status: Accepted; supersedes [0008](0008-configuration-in-kdl.md) and
  [0057](0057-what-yuna-does-and-what-the-shell-does.md) (the theme file and the variants);
  supersedes in part [0064](0064-what-the-best-desktops-taught-yuna.md) (configuration includes)
- Date: 2026-11-18

## Context

[ADR-0008](0008-configuration-in-kdl.md) chose KDL and kept the last valid configuration on an
error. [ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md) point 5 added the theme file the
configuration includes and the variants switched by command, and
[ADR-0064](0064-what-the-best-desktops-taught-yuna.md) optional includes. No status linked them,
and [ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 7
kept, for outputs, the last configuration the hardware accepted. A reader needed four ADRs.

This ADR gathers the rule as it stands on 2026-11-18. It decides nothing new. The configuration
file is read from Stage 2; until then, the animations' settings are one `Settings` value given to
the compositor ([ADR-0109](0109-animations-and-effects-what-moves-reduced-motion-and-games.md)).

## Decision

- **Yuna's configuration is KDL 2.0** (ADR-0008, 2026-11-11). It reloads live. An error is shown,
  with its line and column, and never stops Yuna, which keeps the last valid configuration. For
  outputs, the configuration kept is the last the hardware accepted: an output change is tested
  against the kernel as one configuration and applied whole or not at all (ADR-0082 item 7,
  2026-11-17).
- **Includes are optional, `~` expanded** (ADR-0064, 2026-11-14).
- **The theme is a file the configuration includes** (ADR-0057, 2026-11-14): colours only, written
  by a shell or a theme tool, reloaded live. Yuna's own interfaces take their colours from it. Its
  format is a public interface, documented and versioned.
- **A variant is a named set of settings in the configuration**, applied over it and switched by a
  Yuna command; the shell's compact mode is one (ADR-0057). **Nothing outside Yuna rewrites its
  configuration.**
- **A NixOS module can write it**; the home-manager module writes it from Nix at Stage 2 (ADR-0008;
  [ADR-0126](0126-yuna-and-the-owners-other-projects.md)).
- **The reader**: uwui's KDL reader is reused once it is published on its own, if its licence and
  API fit; until then the reader is chosen at the milestone that needs it (ADR-0008).

## Alternatives considered

- **TOML** ([ADR-0008](0008-configuration-in-kdl.md)): heavier for nested rules.
- **A scripting language, Lua** ([ADR-0008](0008-configuration-in-kdl.md)): more power, but an error
  can break the session, and it is slower to read.
- **The theme and compact mode sent by command only**
  ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md)): lost when Yuna restarts until the
  shell sends them again, with two sources of truth.
- **Themes managed by Yuna** ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md)): a theme
  spans dozens of applications and has no place in a compositor. A command that sets any key until
  the next reload, as `hyprctl keyword` does, stays an idea.

## Consequences

- One format across the owner's projects.
- The configuration and the theme file are public contracts, read across versions keeping what
  they do not know (ADR-0082 item 13).

## Validation and reconsideration

Reviewed reasoning, not measured: no configuration file is read before Stage 2. There, ADR-0082's
scenarios check the outputs' part: a refused mode changes nothing, and an output unplugged while
another changes mode leaves a valid state.
