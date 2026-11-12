# 0024. No title bars: decorations on the server's side

- Status: Accepted
- Date: 2026-11-12

## Context

With `xdg-decoration`, a compositor tells each client whether to draw its own title bar and shadows
or to leave decorations to the compositor. Yuna will draw borders, never title bars
([ADR-0010](0010-animations-and-effects.md)). niri lets clients draw theirs by default, and omits
them with its `prefer-no-csd` option. GTK 4 does not implement `xdg-decoration`: its windows keep
their title bar, and drop their shadows and rounded corners when told they are tiled.

## Decision

Yuna asks every client that supports `xdg-decoration` for server-side decorations: no title bar,
no shadow. Every window, GTK's included, is told it is tiled on all four edges. The owner chose
this on 2026-11-12.

## Alternatives considered

- **Clients' own decorations**, niri's default: a floating window can be dragged by its title bar,
  but tiled windows waste space on bars that differ from one toolkit to the next.

## Consequences

- A floating window is moved with a modifier and the pointer, not by a title bar.
- GTK 4's windows keep a title bar until GTK changes.
