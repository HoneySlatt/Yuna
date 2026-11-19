# 0109. Animations and effects: what moves, reduced motion, games and headless

- Status: Accepted; supersedes [0010](0010-animations-and-effects.md),
  [0055](0055-what-moves-first-and-animations-off-headless.md),
  [0058](0058-what-yuna-takes-from-the-owners-other-projects.md) (reduced motion and reduced
  transparency), [0059](0059-reduced-motion-nothing-slides-fades-stay.md) and
  [0099](0099-a-workspace-switch-keeps-each-workspace-on-its-row.md); supersedes in part
  [0061](0061-yunas-uses.md) (point 9, effects in games) and
  [0062](0062-the-compositors-comfort-and-look.md) (point 5, shapes that morph)
- Date: 2026-11-18

## Context

Yuna's animations and effects were decided in seven ADRs.
[ADR-0010](0010-animations-and-effects.md) set springs, curves and the effects, turned them off in
a game mode, and kept "room" for reduced motion.
[ADR-0055](0055-what-moves-first-and-animations-off-headless.md) chose what moves first and turned
animations off headless; [ADR-0099](0099-a-workspace-switch-keeps-each-workspace-on-its-row.md)
fixed the workspace switch's rows within it.
[ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md) brought reduced motion and
reduced transparency with each animation and effect, and
[ADR-0059](0059-reduced-motion-nothing-slides-fades-stay.md) said what reduced motion does.
[ADR-0061](0061-yunas-uses.md) point 9 reversed ADR-0010's game mode, and
[ADR-0062](0062-the-compositors-comfort-and-look.md) point 5 turned its exotic animations into
shapes that morph. The quality bar and the glossary still carried ADR-0010's game mode.

ADR-0010 turned effects and animations off in a game mode, and built nothing for reduced motion.
ADR-0061 and ADR-0058, later, reversed both; their rule is the one below.

This ADR gathers the rule as it stands on 2026-11-18. It decides nothing new. How springs and
curves are computed is [ADR-0053](0053-springs-in-closed-form-keeping-their-velocity.md)'s; the
motion's own crate is [ADR-0052](0052-motion-in-a-pure-crate.md)'s; motion's budget is
[ADR-0127](0127-motions-budget-no-frame-lost-to-motion.md)'s.

## Decision

- **Motion moves on springs, fades on curves** (ADR-0010, 2026-11-11). Windows, columns,
  workspaces and the overview move on springs, interrupted without a jump, keeping their velocity.
  Fades and appearances follow curves. Each animation is set in the configuration.
- **What moves today** (ADR-0055, 2026-11-14): on springs, the scrolling camera, every window a
  layout operation moves (scrolling, dwindle, floating) and the workspace switch; on a curve, the
  special workspace fading in and out. **Fullscreen is instant**, in and out: a game wants its
  output at once.
- **A workspace switch slides** along the output's vertical stack
  ([ADR-0018](0018-workspaces.md)), moving only the workspace left and the one shown, however far
  apart; retargeted mid-way, it slides on from where it is (ADR-0055). Each workspace of a switch
  keeps a fixed row (ADR-0099, 2026-11-18). One that leaves the output leaves its row empty, and
  the view slides on over the gap. One newly shown takes the row after the one before it in the
  output's order, or the row above the first. One shown between two on adjacent rows takes the
  place of the one farther from the view, which leaves the switch and stops being drawn at once;
  on a tie, the one after it.
- **Opening, closing and resizing come with the effects, at Stage 6, as shapes that morph**
  (ADR-0055; ADR-0062, 2026-11-14): a window that changes shape into its place, glass that merges,
  measured as every effect. They take the place of ADR-0010's few exotic open and close
  animations.
- **Effects in Yuna** (ADR-0010): blur and glass, rounded corners, shadows, animated borders; and
  built-in effects, HDR-correct and tested: vibrance, filters (greyscale, sepia, colour-blind), a
  CRT effect for emulators, sharpening for games. The user's own shaders come later, as an
  advanced option with guards. Each effect costs its GPU time and power, measured, and lightens on
  the laptop.
- **Effects stay on in games by default** (ADR-0061, 2026-11-14): settings choose what a game turns
  off, blur, glass and animations, each on its own. Games still come first: no effect or animation
  may cost a game a frame or a film its cadence
  ([ADR-0115](0115-priorities-uwui-first-the-owners-activity-measures-that-check-their-load.md)).
- **Reduced motion and reduced transparency come with each animation and each effect** (ADR-0058,
  2026-11-14): with M1.7 for motion, at Stage 6 for blur and glass. Under reduced motion nothing
  crosses the screen: the camera, the windows a layout operation moves and the workspace switch
  show their result at once; fades stay, over half their duration (ADR-0059, 2026-11-14); shapes
  that morph are off (ADR-0062). Stage 6's effects follow the same rule: what moves stops, what
  fades stays.
- **Reduced motion is a setting of its own**, beside `off` (ADR-0059): `Settings::reduced_motion`
  in `yuna-motion`, and `--reduced-motion` for `yuna --headless` and `yuna mcp`, in place of
  `--animations`. The desktop reads the shell's or the session's setting at Stage 2. The rest of
  accessibility stays later, named in the ROADMAP: Yuna's interfaces read by a screen reader, a
  magnifier, high contrast, sticky and slow keys (ADR-0058).
- **Animations are off headless** (ADR-0055): in `yuna --headless` and `yuna mcp` an operation
  shows its result at once; `--animations` turns them on, in both. The desktop has them on. `off`
  stays a setting of its own, for agents and for what a game's settings turn off.
- **Settings** are one `Settings` value in the compositor's configuration, each animation a
  spring or a curve with its defaults, until the configuration file is read at Stage 2
  (ADR-0055).
- **Animations run on a clock that tests control**, so that each is checked frame by frame
  (ADR-0010).

## Alternatives considered

- **Curves only, or springs only** ([ADR-0010](0010-animations-and-effects.md)): an interrupted
  curve restarts less naturally; springs give less control over fades.
- **The user's shaders first** ([ADR-0010](0010-animations-and-effects.md)): each effect would
  handle HDR itself.
- **Every effect off in a game mode** ([ADR-0061](0061-yunas-uses.md)): a fullscreen game is
  scanned out and Yuna draws nothing then, so it changes nothing; the owner wants to choose.
- **Fullscreen animated**, as niri
  ([ADR-0055](0055-what-moves-first-and-animations-off-headless.md)): about 0.4 s before a game
  has its output.
- **The whole stack crossing the view on a far switch**, as niri, or **a cross-fade switch**
  ([ADR-0055](0055-what-moves-first-and-animations-off-headless.md)): workspaces nobody asked for
  shown and woken, or twice a slide's fill.
- **Animations on by default headless**
  ([ADR-0055](0055-what-moves-first-and-animations-off-headless.md)): other projects' tests and
  agents would see motion half-way.
- **Cross-fades in place of slides under reduced motion**, as Apple
  ([ADR-0059](0059-reduced-motion-nothing-slides-fades-stay.md)): more GPU time where the rule asks
  for less.
- **Everything at once under reduced motion**, as GNOME
  ([ADR-0059](0059-reduced-motion-nothing-slides-fades-stay.md)): the special workspace would
  appear abruptly.
- **Rows shifted to make room for a workspace shown between two**
  ([ADR-0099](0099-a-workspace-switch-keeps-each-workspace-on-its-row.md)): the rows beside it
  move, and a window jumps.

## Consequences

- Reduced motion costs less than motion: it draws fewer frames.
- A window of a workspace that leaves a switch is told hidden as it stops being drawn, before the
  switch ends.
- Agents run in a headless Yuna of their own, whose animations are off unless the agent asks.

## Validation and reconsideration

Evidence: M1.7's tests on the manual clock, frame by frame, and the motion's property test, 0
failures in 204,800 random cases with the switch's rows, against 6 in 102,400 before. Proposed
again if the layout ever reorders an output's workspaces, which the switch's rows assume it never
does.
