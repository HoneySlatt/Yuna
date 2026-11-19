# 0127. Motion's budget: no frame lost to motion, judged against a still baseline

- Status: Accepted; supersedes
  [0056](0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md); supersedes in part
  [0077](0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md) (motion's budget)
- Date: 2026-11-19

## Context

The ROADMAP's bar set motion's budget as no missed frame at 240 Hz while scrolling, animating or
switching workspaces, headless and on the owner's monitor.
[ADR-0056](0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md) decided how it is
judged: on the GPU, against the same output redrawn without motion, and it rejected "zero misses"
alone, since a miss unrelated to motion would fail it.
[ADR-0077](0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md), setting the other
budgets two days later, wrote that motion "keeps its rule, no missed frame at 240 Hz", citing
ADR-0056, and [ADR-0075](0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)
kept ADR-0056's judgement until Stage 2's exit. The two wordings disagree on a run with one miss
in the still baseline and one during motion. A reader needed three ADRs and the bar.

This ADR gathers the rule as it stands on 2026-11-19. It decides nothing new: ADR-0077 kept
motion's rule rather than changing it, and named ADR-0056 as its rule; the owner confirmed
ADR-0056's judgement on 2026-11-19, and it stands here.

Without any motion, llvmpipe misses 0.4 to 0.9 % of frames at 1080p and every frame at 4K; the GPU
missed 0 or 1 in 2,400 at 4K. On the manual clock, drawing takes no time unless a test makes it.

## Decision

- **In CI, the logic** (ADR-0056, 2026-11-14): on the manual clock, each motion stepped boundary by
  boundary, every frame's drawn rectangles and alphas equal to `yuna-motion`'s reference rounded
  once; draw hooks that take 70 % of a period (nothing missed), one drawing of 1.5 periods (one
  missed, the next aimed right, the end unmoved), and every drawing 1.2 periods (every other
  boundary, the end unmoved); random sequences of operations with animations on.
- **The budget, on the GPU** (ADR-0056; confirmed by the owner on 2026-11-19): at 3840 × 2160, 240 Hz, scale 1.5, a scripted sequence of
  scrolls, column moves, switches and the special workspace's fade misses no more frames than the
  same output redrawn whole without motion, over 10,000 frames or more. Misses are counted by where
  they fall: a motion's first frames, a window entering the view. The still baseline is measured
  with each run, so that the comparison holds on the same load.
- **Headless until Stage 2's exit** (ADR-0075, 2026-11-16; ADR-0077, 2026-11-16). At Stage 2's
  exit, the owner's tour counts the frames missed while moving on a real display (ADR-0075).
  **Open:** whether the tour judges them against a still baseline, as here, or as no miss at all,
  as the bar's row reads; it is put to the owner with the tour's design.
- **llvmpipe at 1080p** is measured the same way and recorded, without a pass mark (ADR-0056).
- **The driver** is an example, `motion_time`, that runs Yuna in its own process on the real clock
  and calls the layout's operations; MCP gains no layout tool for it (ADR-0056).
- **The figure kept**: a frame's time during the heaviest frame of that sequence (ADR-0056).

## Alternatives considered

- **Zero misses on the GPU, alone**
  ([ADR-0056](0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md)): a miss unrelated
  to motion would fail it; the measure would not say what it measures.
- **MCP layout tools as the driver**
  ([ADR-0056](0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md)): useful to agents
  too, but a change of the MCP server that grew the milestone; an idea for later.
- **A pass mark on llvmpipe**
  ([ADR-0056](0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md)): it misses frames
  without motion.

## Consequences

- CI proves that motion paces and ends right; only the owner's GPU proves the rate.
- The ROADMAP's row on motion is read with this rule.

## Validation and reconsideration

Evidence: `motion_time`'s run of M1.7 on the owner's GPU, motion missing no frame of 10,000 at 4K,
which meets either reading ([measurements](../measurements.md)). Proposed again with the tour's
design at Stage 2's exit, for the open question above.
