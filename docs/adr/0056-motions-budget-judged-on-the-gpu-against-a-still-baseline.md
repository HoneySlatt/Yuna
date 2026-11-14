# 0056. Motion's budget judged on the GPU, against a still baseline

- Status: Accepted
- Date: 2026-11-14

## Context

The [ROADMAP](../ROADMAP.md#3-quality-bar)'s motion budget is no missed frame at 240 Hz while
scrolling, animating or switching workspaces, headless and on the owner's monitor. Without any
motion, llvmpipe misses 0.4 to 0.9 % of frames at 1080p and every frame at 4K; the GPU missed 0
or 1 in 2,400 at 4K ([measurements](../measurements.md), [debt](../debt.md)). On the manual
clock, drawing takes no time unless a test makes it, so no frame can be missed there
([ADR-0032](0032-input-to-present-read-by-mode.md)). Nothing outside Yuna can script a scroll
today.

## Decision

The owner chose, on 2026-11-14:

- **In CI, the logic**: on the manual clock, each motion stepped boundary by boundary, every
  frame's drawn rectangles and alphas equal to `yuna-motion`'s reference rounded once; draw hooks
  that take 70 % of a period (nothing missed), one drawing of 1.5 periods (one missed, the next
  aimed right, the end unmoved), and every drawing 1.2 periods (every other boundary, the end
  unmoved); random sequences of operations with animations on.
- **The budget, on the GPU**: at 3840 × 2160, 240 Hz, scale 1.5, a scripted sequence of scrolls,
  column moves, switches and the special workspace's fade misses no more frames than the same output redrawn whole without motion, over 10,000 frames or more.
  Misses are counted by where they fall: a motion's first frames, a window entering the view.
- **llvmpipe at 1080p** is measured the same way and recorded, without a pass mark.
- **The driver** is an example that runs Yuna in its own process on the real clock and calls the
  layout's operations; MCP gains no layout tool for it.
- **The figure kept**: a frame's time during the heaviest frame of that sequence.

## Alternatives considered

- **Zero misses on the GPU, alone**: a miss unrelated to motion would fail it; the measure would
  not say what it measures.
- **MCP layout tools as the driver**: useful to agents too, but an ADR-0033 change that grows the
  milestone; an idea for later.
- **A pass mark on llvmpipe**: it misses frames without motion.

## Consequences

- CI proves that motion paces and ends right; only the owner's GPU proves the rate.
- The still baseline is measured with each run, so that the comparison holds on the same load.
