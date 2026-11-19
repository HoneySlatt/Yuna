# 0119. Emulated VRR, as amdgpu and the owner's monitor do

- Status: Accepted; supersedes [0046](0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md),
  [0051](0051-emulated-vrr-as-the-monitors-traces-showed.md) and
  [0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md) (a VRR refresh during
  motion)
- Date: 2026-11-18

## Context

M1.6 made a virtual output behave as the owner's monitor does under VRR: a PG32UCDM on an RX 6900
XT, announcing 48 to 240 Hz. [ADR-0046](0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)
emulated amdgpu's rule from a reading of `freesync.c`, to be checked against the monitor;
[ADR-0051](0051-emulated-vrr-as-the-monitors-traces-showed.md) corrected the panel's rules from the
monitor's traces; [ADR-0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md) made a
refresh during motion show its frame again; [ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)
moved `fifo-v1` and `commit-timing-v1` to Stage 2. A reader needed four of them.

This ADR gathers the rule as it stands on 2026-11-18. It decides nothing new. The frame loop it
fits into is [ADR-0118](0118-frames-presentation-times-and-the-clock.md)'s; the policy of when
Yuna uses VRR is [ADR-0014](0014-vrr-without-flicker.md)'s, at Stage 4.

ADR-0046 showed a frame late within any inserted refresh at its end, and left `fifo-v1` and
`commit-timing-v1` to the games' stage; ADR-0051 and ADR-0058 replaced both. The later rule is the
one below.

## Decision

- **A virtual output may run under VRR** over a range [min, its rate], its rate at least twice the
  minimum (ADR-0046, 2026-11-14). It emulates amdgpu's DisplayPort Adaptive-Sync: a refresh lasts
  at least 1/max and at most 1/min; below the range, amdgpu inserts refreshes (BTR) as
  `freesync.c` computes them (their count n, its drift margin, its hysteresis). Other paths of the
  driver are not emulated.
- **The panel's rules** (ADR-0051, 2026-11-14). Under BTR, the first n − 1 inserted refreshes have
  a fixed length, and a frame ready during one shows at its end; the last is variable again, from
  the shortest refresh to the longest. A frame shows a vertical blank after it is ready at the
  soonest: the owner's mode's 269 of 2,429 lines, 461 µs at 240 Hz.
- **A frame is presented at the first refresh it can reach** (ADR-0046; ADR-0051). Refreshes the
  panel makes by itself are computed when needed, never armed as timers, and no deadline is armed
  at the range's floor: an output at rest wakes nothing. Its feedback counts every refresh in
  `seq`, gives 1/max as `refresh`, and the VSYNC flag, as `wp_presentation` version 2 allows.
- **The frame loop aims at the earliest refresh a frame can reach**, minus the output's lead, and
  draws at once when that refresh could begin now (ADR-0046). A frame is missed when it is
  presented after the refresh it aimed at.
- **A refresh that draws nothing is paced**, a shortest refresh after the one before, so that
  callbacks asked without a buffer never loop (ADR-0051). Something new to draw, a buffer attached
  or removed, a change of the layout or a popup gone, is never paced; two refreshes in a row armed
  for something new that drew nothing, its buffers all outside the output, pace again.
- **During a motion, a refresh that finds nothing changed shows its frame again** (ADR-0054,
  2026-11-14), so that the panel refreshes where the frame loop counts one and the motion keeps
  the shortest refresh to its end. At rest, such a refresh still flips nothing.
- **The emulation is checked against the monitor** (ADR-0046; ADR-0051). Two traces the owner
  recorded from a text console on Linux 7.3.1, steady rates and probes, are fixtures of `vrr.rs`;
  the replay checks every frame of both, the panel set back on the monitor's history after each,
  against 500 µs, the spread of the kernel's commit path.
- **What is not modelled is debt, for Stage 4** (ADR-0051): the hold after the panel's own
  refresh, BTR's count after a perturbation, and the workaround that holds the top rate after
  flips close to a vertical update. ADR-0014's band keeps games above the panel's floor, where
  these do not arise.
- **No policy turns VRR on or off yet**: an output is created with it or without it (ADR-0046).
- **`wp_tearing_control_v1` and presentation-time version 3** wait for the games' stage
  (ADR-0046); `fifo-v1` and `commit-timing-v1` come at Stage 2 (ADR-0058, 2026-11-14), as
  ADR-0118 says.

## Alternatives considered

- **The emulation without the trace**
  ([ADR-0046](0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)): "as on the owner's
  monitor" would rest on a reading of driver code, and on rules no source writes.
- **A simple panel that repeats at its minimum**
  ([ADR-0046](0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)): a 24 fps film's
  intervals would jitter where the monitor's do not.
- **ADR-0014's policy now** ([ADR-0046](0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)):
  Stage 4's and Stage 5's work, which needs games and films recognised.
- **Simulating amdgpu's commit path event by event**
  ([ADR-0051](0051-emulated-vrr-as-the-monitors-traces-showed.md)): more traces and reading, for
  regimes Yuna's own policy avoids.

## Consequences

- Each output keeps the times of its recent refreshes; the frame loop, captures and presentation
  read fixed and variable outputs through one interface.
- 91 % of the steady rates' frames and 82 % of the probes' start within 500 µs of the monitor's;
  96 % and 94 % at the monitor's refresh number.
- A VRR output's frames are presented a vertical blank after their drawing ends.
- At Stage 2, the DRM backend commits the unchanged buffer at a motion's refreshes: amdgpu counts a
  commit of the primary plane with a buffer as a flip (ADR-0054).
- A film on a VRR output loses its exact multiples while a motion runs there; Stage 5 runs motion
  at a multiple of the film's rate (ADR-0054).
- A kernel that changes `freesync.c`, or another monitor, needs new traces.

## Validation and reconsideration

Evidence: the replay of both traces ([measurements](../measurements.md)). Proposed again at
Stage 4, with games' VRR, when the debt above is taken up.
