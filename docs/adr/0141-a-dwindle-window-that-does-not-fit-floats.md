# 0141. A dwindle window that does not fit floats

- Status: Accepted
- Date: 2026-11-19

## Context

Dwindle gives each new window half the space of the focused one
([ADR-0110](0110-the-layouts-and-their-behaviours.md)). A tree deeper than its space allows gives
its last windows slots under a pixel: today the layout asks them 1 × 1 and does not draw them
([debt](../debt.md#the-layout-model)).
[ADR-0020](0020-the-layout-models-shape.md) left the decision to M1.4, naming Hyprland, which
floats a window that does not fit; M1.4 did not take it, and no ADR does. Stage 2's opening design placed the item, on 2026-11-19.

## Decision

The owner chose, on 2026-11-19, **that a window dwindle cannot give a slot of at least one pixel
floats, as Hyprland does**, so that a window is never open and unseen. Built at Stage 2c, tested
in the layout model as every layout behaviour is.

## Alternatives considered

- **Keep the empty slot**: nothing to build, and a tree that deep is rare; but a window the user
  opened stays invisible, with no way to find it but the window list.

## Consequences

- The layout model's dwindle gains a rule: a window whose slot would be under a pixel leaves the
  tree for the floating layer; the debt's item is paid then.

## Validation and reconsideration

A property test: no window of a dwindle workspace is ever asked 1 × 1 and left undrawn. Proposed
again if a real use fills a tree that deep and floating windows get in the way.
