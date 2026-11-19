# 0142. Parity: a pinned baseline, figures in classes, three verdicts

- Status: Accepted
- Date: 2026-11-19

## Context

[ADR-0139](0139-stage-2-in-three-gated-parts-2as-path-revised.md) makes parity the rule of every
step of Stage 2a: the tests unchanged in what they assert, the traces and captures compared, and
"the baselines' figures no worse by the stated rule"; a step that makes any of these worse is
reverted. M2.1 builds the parity script and states the rule. ADR-0139 also says the processor tier
is judged on its own budget, not by parity on a full frame.

A first design compared each push with the one before it, gated one figure for a frame's
instructions and judged timed runs with a sign test over ten pairs. Its challenge, on 2026-11-19,
with a trial of valgrind on `yuna mcp --manual-clock` (`.delegation/m2.1-vg-trial/`, local),
found:

- **A frame's instructions are llvmpipe's.** On two `vkcube` windows at 1080p, 79.5 % are
  llvmpipe's compiled shaders, 19.1 % Mesa's driver, 0.22 % the `yuna` binary. A threshold on the
  total cannot see Yuna's own code; the `yuna` binary's own figure repeats to 0.19 %.
- **A push compared with the one before** lets regressions through: a run cancelled by a later
  push is never compared, a flagged push becomes the next one's base, and steps just under the
  threshold add up.
- **One rule for every figure** contradicts ADR-0139 on the processor tier, and has no way to
  accept a change made on purpose (Vulkan in place of GLES, 1 MiB per client).
- **A sign test over ten pairs** detects a true 5 % slowdown one time in three at the noise of a
  4K llvmpipe frame (about 4 % a pair), and a fixed order of runs biases it.
- **Clients that draw at their own pace** (`vkcube`) make the frames of two builds differ, so that
  a figure divided by frames compares different work.

## Decision

The owner chose, on 2026-11-19:

- **A pinned baseline.** A tracked file names the baseline commit, M2.1's at first. Every push is
  compared with it, not with the push before; the push before is compared too, to say which push
  moved a figure, never to judge.
- **Each figure has a class.** *Parity*: no worse than the baseline. *Budget*: judged against its
  budget in the ROADMAP or an ADR, its figure written, as a full 4K frame on the processor tier
  (ADR-0139). *Recorded*: written, no verdict.
- **A change made on purpose moves the baseline** by a commit that names the figures it moves,
  says why, and cites the ADR or the owner's go. The baseline moves whole to that commit; the
  parity run of that commit against the old baseline flags only the figures named.
- **Three verdicts for timed figures**: *no worse*, *worse* or *inconclusive*, from an interval on
  the ratio of head to base over pairs run in random order; an inconclusive figure runs more
  pairs, up to a cap. *Inconclusive*, and a run that could not measure (*void*), never pass.
- **Equal work before costs.** A figure a frame is compared only when both builds did the same
  work: the same frames, the same commits from each client, the same causes of drawing. A run
  where they differ is void.
- **The tiers.** At each push, in CI: figures the machine's load does not move, counted by
  valgrind on the manual clock with scripted clients that commit in step with it: instructions of
  the `yuna` binary and in total, allocations a frame, memory, wakes, the layout's and motion's
  operations. In an agent's runs on the GPU: the kernel's DRM memory. Once per milestone, by an
  agent on the high tier's desktop under ADR-0115's rules: timed runs, start time, GPU memory, a
  partial-damage frame, a soak.
- **Controls.** The script proves itself: the baseline against itself passes, and an injected
  cost of each kind it watches, about twice its threshold, is flagged.

Each threshold, and the figures it rests on, is written in the script's documentation and in
[`measurements.md`](../measurements.md).

## Alternatives considered

- **Each push against the push before**: no baseline to keep, but regressions pass through
  cancelled runs and add up below the threshold.
- **One class, strict parity everywhere**: simpler; M2.9 (Vulkan on the processor) would need an
  exception decided at that time, against ADR-0139.
- **A sign test over a fixed number of pairs**: simple, but too weak at the noise measured, and it
  passes on thin evidence.

## Consequences

- Moving the baseline is a deliberate commit, read in the history; nothing moves it silently.
- CI builds two revisions at each push, the baseline and the head, and measures both on the same
  runner: figures from different machines are never compared.
- The scripted clients and the MCP tools the script uses are its contract with the builds it
  compares; a base that lacks one makes that figure void, not passing.
- A milestone's timed runs take hours on a busy machine; ADR-0115 lets the owner's activity come
  first, so a milestone may wait for a quiet machine.

## Validation and reconsideration

Evidence: the trial and the challenge of 2026-11-19 (figures above), the M1.4 and M1.13 spreads in
[`measurements.md`](../measurements.md). M2.1 is done when the script flags the injected costs and
passes the baseline against itself. Proposed again if a control fails by noise more than once in a
milestone, or if the runs a milestone needs grow beyond a day.
