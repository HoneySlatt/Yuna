# 0099. A workspace switch keeps each workspace on its row

- Status: Superseded by [0109](0109-animations-and-effects-what-moves-reduced-motion-and-games.md)
- Date: 2026-11-18

## Context

A switch between workspaces slides a stack of them through the view
([ADR-0055](0055-what-moves-first-and-animations-off-headless.md): the workspaces in between left
out). Until M1.11 the stack closed up when a workspace left it, and a workspace shown mid-switch
was inserted between two others: a workspace beside it moved by a whole height, and a window of
it hanging into the view jumped on screen, against
[ADR-0052](0052-motion-in-a-pure-crate.md)'s rule that motion never jumps. The motion's property
test found it at random, about once in 17,000 cases, during M1.11's checks; it existed since
M1.7.

## Decision

Chosen while building M1.11, as implementation within ADR-0055:

- **Each workspace of a switch keeps a fixed row.** A workspace that leaves the output leaves its
  row empty, and the view slides on over the gap. A workspace newly shown takes the row after the
  one before it in the output's order, or the row above the first.
- **A workspace shown between two on adjacent rows takes the place of the one farther from the
  view**, which leaves the switch; on a tie, the one after it. A window of the workspace that
  leaves stops being drawn at once, rather than jump.

## Alternatives considered

- **The stack closed up, as before**: fewer rows, but a window jumps.
- **Rows shifted to make room for the one shown between**: nothing vanishes, but the rows beside
  it move, which is the jump again.

## Consequences

- The property test no longer exempts a workspace leaving a switch; it passed 204,800 random cases.
- A window of a workspace that leaves a switch is told hidden as it stops being drawn, before the
  switch ends.

## Validation and reconsideration

Evidence: two scenarios of the motion crate, the minimal cases of both kinds, each failing before;
0 failures in 204,800 random cases after, against 6 in 102,400 before. Proposed again if the
layout ever reorders an output's workspaces, which the rows assume it never does.
