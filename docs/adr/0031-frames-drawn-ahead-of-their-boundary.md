# 0031. Frames drawn ahead of their boundary

- Status: Accepted; amends [0023](0023-frames-presentation-and-the-clock.md) (when a frame is
  drawn, when its feedback is sent); amended by
  [0045](0045-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md) (no deadline at a VRR floor)
- Date: 2026-11-13

## Context

[ADR-0023](0023-frames-presentation-and-the-clock.md) has a commit arm its output's next refresh
boundary; drawing starts there, and the frame is presented at the first boundary at or after its
drawing ends. A drawing that takes any time is presented a boundary later: a commit waits more
than one period before it shows, up to two. At 240 Hz, with a period of 4.17 ms, an animating
client sees about 8.3 ms from its commit to its presentation. The manual clock, where drawing
takes no time, hid it; M1.4's measures found it ([debt](../debt.md)).

How the field times its frames ([survey](../survey.md#when-a-frame-is-drawn)): niri draws at once
on an idle output, and just after vblank otherwise; sway, by default, just after the refresh.
KWin, Mutter and gamescope draw at a deadline before the next vblank: their predicted drawing
time and a margin. Mutter also draws at once after an idle period, for "lowest average latency
for sporadic user input". KWin measures each drawing from its scheduled wake-up, with a floor of
2 ms and a margin of 1 ms; gamescope keeps a peak of its draw times that jumps up at once.
Mesa's Vulkan present-wait ends when `presented` arrives, whatever time it carries.

The recommendation was challenged before the owner chose; the challenge added the cap, the
timing from the deadline, the definition of idle and the held feedback below.

## Decision

The owner chose, on 2026-11-13, Mutter's rule, with these parts:

- **An output's next deadline.** A commit, or a change of the layout, arms it:
  - **at once** when the output is idle, at the end of the round that read the commit;
  - otherwise at **b(k) − lead**, for the first boundary k after the last one a refresh aimed at
    whose deadline is not yet past.
- **Idle** means no frame waits for its boundary, and the boundary after the output's last
  refresh has passed. A refresh that drew nothing counts, so that a client asking frame
  callbacks without damage is answered once a period, never in a loop. An output's creation
  counts as a refresh at boundary 0.
- **The lead** is the longest drawing of the last 0.5 s on the clock, plus a margin, at most one
  period. A drawing is timed from its deadline to its end, so that a refresh waiting behind other
  outputs or a flood of clients counts. Each drawing counts at least a floor. Floor and margin
  start at KWin's, 2 ms and 1 ms, and are set from the miss rate measured headless
  ([measurements](../measurements.md)). At most one period: one frame waits at a time, and a single
  slow drawing lengthens the lead for half a second at most.
- **A frame is presented** at the boundary it aimed at if its drawing ended by then, else at the
  first boundary after: missed, as ADR-0023 has it.
- **Frame callbacks** go out right after the refresh, stamped with its end, as before.
- **Presentation feedback** is taken when the frame is drawn, held by its output, and sent at its
  boundary, through a deadline of its own. A capture will be copied when its frame is drawn and
  delivered at that boundary the same way.

## Alternatives considered

- **Keeping ADR-0023's rule**: a period lost on every frame; the owner chose to end it.
- **Drawing at once, at every commit** (niri): the lowest latency for a single key, about 2.3 ms
  on average at 240 Hz (estimated). An animating client's commit, though, lands while a frame
  waits, and is shown two boundaries later, as today.
- **The deadline alone** (KWin, gamescope): one rule, and content taken as late as possible. An
  isolated key pays the whole lead, about 3 ms more on average at 240 Hz with KWin's values
  (estimated).
- **A lead without a cap**: on llvmpipe at 4K, drawings of 7 to 11 ms would put the deadline two
  or three boundaries ahead. Several frames would then wait at once in the output's one buffer,
  and one 50 ms drawing would add 50 ms to every frame for half a second. KWin caps at two periods
  with a third buffer, gamescope under one.
- **A lead set by hand** (sway's `max_render_time`): right for one machine and one load only.
- **Feedback sent right after drawing**, with its boundary's time: one wake-up fewer per frame.
  But a Vulkan client waiting for presentation would be released before the frame shows, and an
  agent stepping the clock would read a presentation still in its future.
- **Retrying a deadline just missed** (Mutter): a second rule for a commit that arrived a moment
  late. The commit waits one period instead, and its feedback says so.

## Consequences

- At 240 Hz, estimated: an isolated commit is presented about 2.3 ms after it on average, against
  6.3 ms before; an animating client about one period after its commit, against two.
- Each animated frame costs three wake-ups: the client's commit, the deadline and the boundary,
  against two before. Nothing changes at rest: no deadline is armed.
- Two latch points coexist: at once when idle, b − lead when not. A client committing at a steady
  slow rate, a 24 fps film, finds the output idle between its frames. A commit between b − lead
  and b − its drawing then makes b when the output is idle, and b + 1 when another client draws.
  Tests pin a steady client's cadence, alone and beside an animating one.
- At a low rate, a commit that just missed a deadline waits up to two periods, 66 ms at 30 Hz.
- On llvmpipe at 4K, where a drawing takes longer than a period, the lead is one period: the
  frame is drawn at the boundary before, and is presented late, as it was.
- The output's buffer holds the last frame drawn, which may not be presented yet.
- Tests that pin frame callback times move by the lead; ADR-0023's "arranged once per output
  frame" now means once, at the first output's refresh after the layout changed.
- **The DRM backend** (Stage 2) adds to the margin the mode's vblank time, during which the kernel
  refuses commits. It renders before committing, at the commit's own deadline, as KWin does. It
  times a drawing by its fence, since the main thread must not wait on the GPU.
- **VRR** (M1.6, Stage 4) keeps a deadline at the range's ceiling and its floor.
- `fifo-v1` and `commit-timing-v1`, which Mesa's FIFO uses, would add a rule "not before this
  time" to the same deadline. They are not offered yet.
