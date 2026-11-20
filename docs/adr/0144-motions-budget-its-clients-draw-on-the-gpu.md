# 0144. Motion's budget: its clients draw on the GPU

- Status: Accepted; supersedes in part [0127](0127-motions-budget-no-frame-lost-to-motion.md) (where the budget's clients draw)
- Date: 2026-11-20

## Context

[ADR-0127](0127-motions-budget-no-frame-lost-to-motion.md) judges motion's budget on the GPU, at
3840 × 2160, 240 Hz, scale 1.5, with `motion_time`'s six `vkcube` windows: the scripted sequence
misses no more frames than the same output redrawn whole without motion, over 10,000 frames or
more, the still baseline measured with each run. It does not say where the clients draw.
`motion_time` gives them lavapipe, Mesa's Vulkan on the processor, unless `--client-vulkan` names
another driver; M1.7's run, which met the budget, gave them the GPU's (radv).

Measured again on a quiet machine on 2026-11-20 (M2.1, [measurements](../measurements.md)):

- **Clients on the GPU (radv)**: 240.0 frames a second in both phases, 0 missed of 10,000 still and
  in motion; the heaviest frame took 1,482 µs of a 4,167 µs period.
- **Clients on lavapipe**: the still baseline at 99.6 % of the rate, 41 missed; in motion 3,674
  missed, at 173.7 frames a second. The six clients used about 8.6 cores in motion, and 3,572 of
  the misses fell outside the frames a window enters: the machine was saturated by the clients'
  drawing, which the owner's desktop does on the GPU.

## Decision

The owner chose, on 2026-11-20: **motion's budget is judged with its clients drawing on the GPU**,
as on the owner's desktop and as at M1.7: `motion_time --gpu --client-vulkan <the GPU's ICD>` at
3840x2160@240,1.5, the rest of ADR-0127's rule unchanged. A run with the clients on lavapipe is
recorded, without a pass mark.

## Alternatives considered

- **Clients on lavapipe, `motion_time`'s default**: the same clients on every machine, but on the
  GPU tier it measures the processor saturated by six software-rendered clients, not Yuna's
  motion; the budget would fail for a cause outside Yuna.

## Consequences

- CONTRIBUTING's command for the budget names the clients' driver.
- A machine with no GPU Vulkan driver cannot judge the budget; it records the lavapipe run.

## Validation and reconsideration

Evidence: the two runs above and M1.7's. Proposed again if a run with the clients on the GPU misses
frames that the still baseline does not, or if the owner's tour at Stage 2's exit shows motion
losing frames that this run does not.
