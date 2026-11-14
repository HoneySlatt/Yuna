# 0010. Animations and effects

- Status: Accepted; amended by 0058, 0061
- Date: 2026-11-11

## Context

The owner finds Hyprland's animations the most beautiful, and wants better. niri animates with
springs and curves; Hyprland with Bézier curves, and springs since 0.57; KWin retargets running
animations ([survey](../survey.md#animations)).

## Decision

- **Motion** (windows, columns, workspaces, the overview) moves on **springs**, interrupted without a
  jump, keeping its velocity. **Fades and appearances** follow **curves**. Each animation is set in
  the configuration.
- **Effects in Yuna**: blur and glass, rounded corners, shadows, animated borders.
- **Built-in effects**, HDR-correct and tested: vibrance, filters (greyscale, sepia, colour-blind),
  a CRT effect for emulators, sharpening for games, a few exotic open and close animations.
- **The user's own shaders** come later, as an advanced option with guards.
- In **game mode**, effects and animations are off unless the owner keeps one.
- Room is kept for **reduced motion**, not built now.

## Alternatives considered

- **Curves only**: full control, but an interrupted animation restarts less naturally.
- **Springs only**: natural motion, less control over fades.
- **The user's shaders first**: freedom at once, but each effect must handle HDR itself.

## Consequences

- An effect costs its GPU time and power, measured; it lightens on the laptop.
- Animations run on a clock that tests control, so that each one is checked frame by frame.
