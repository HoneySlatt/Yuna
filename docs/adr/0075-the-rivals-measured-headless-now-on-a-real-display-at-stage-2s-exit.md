# 0075. The rivals measured: costs headless by agents now, frames, latency and watts on a real display at Stage 2's exit

- Status: Accepted; supersedes [0071](0071-the-rivals-measured-headless-by-agents-and-on-a-real-display-by-the-owner.md)
- Date: 2026-11-16

## Context

M1.9 measures dwl, sway, Hyprland, niri, mango, Umbriel, KWin, Mutter and cosmic-comp, so that
the bar's budgets rest on facts ([ROADMAP](../ROADMAP.md#3-quality-bar)). No controlled
comparison of them exists ([survey](../survey.md#the-rivals-measured)).

Their sources show that they cannot all be measured the same way without a display. sway, dwl,
mango and Umbriel run on wlroots' headless backend, which draws 4 ms after each commit, with no
grid of refreshes; Hyprland and Mutter present at "now"; only KWin's virtual backend and Yuna keep
a grid ([ADR-0023](0023-frames-presentation-and-the-clock.md)). Hyprland needs a parent
compositor without a DRM session; niri's headless backend serves its own tests only; cosmic-comp
has none. A client that asks frame callbacks makes each compositor draw. The owner's monitor and
laptop panel give every compositor the same clock: the kernel's.

[ADR-0071](0071-the-rivals-measured-headless-by-agents-and-on-a-real-display-by-the-owner.md)
planned a tour on the owner's machines at M1.9, for the rivals, and another at Stage 2's exit with
Yuna. But a budget is a comparison measured the same day, on the same machine, kernel and drivers
([ADR-0072](0072-budgets-set-against-the-peers-the-same-day.md)), and Yuna cannot drive a display
before Stage 2: a tour at M1.9 could set no budget, and Stage 2's exit measures the rivals again.

## Decision

The owner chose, on 2026-11-15 and 2026-11-16:

- **One bench, outside the compositor, measures every compositor the same way.** It reads the
  compositor's process tree from `/proc` and the kernel's DRM usage statistics, as
  [ADR-0030](0030-a-frames-gpu-time-from-the-kernel.md) does for Yuna. Each rival runs in a
  sandbox of its own: a cleared environment, its own runtime folder and session bus, no socket or
  bus of the owner's. Each is measured as shipped and at equal work (effects, wallpaper and
  XWayland off). The bench's own clients are counted apart.
- **Agents measure, headless, what does not depend on the clock**: CPU and GPU time a frame
  drawn, wake-ups and frames at rest, memory, start time; at M1.9, and again at each stage's end.
- **The owner measures what does, on a real display, at Stage 2's exit**, with Yuna among the
  rivals: a *tour* on each machine, the desktop, the laptop and the Steam Deck. From a spare
  virtual terminal, one command starts each rival on the display in turn, runs the same rounds,
  and stops it. It gives frames missed while moving, input to present end to end against
  gamescope and KWin ([ADR-0076](0076-input-to-present-read-by-mode.md)), and the GPU's or the
  battery's watts. The keys it sends reach only the compositors it started. niri and cosmic-comp,
  which cannot run headless, are measured there.
- **The GPU's performance level is `auto` during a tour.** The owner sets it before and puts back
  their own after; the tour reads it and refuses to start on another level. Agents never change
  it; their measures record it, with the GPU's clocks.
- **Until Stage 2's exit**, Yuna's motion is judged headless by
  [ADR-0056](0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md), and the bar's
  budgets of input to present and power keep their rule without a rival's figure.

## Alternatives considered

- **A tour at M1.9 for the rivals, as ADR-0071 planned**: reference figures on real displays at
  once, but hours of the owner's on three machines for figures no budget can use, measured again
  at Stage 2's exit.
- **Everything headless, the limit written**: no time of the owner's, but missed frames, latency
  and watts compared on clocks that differ, or not at all.
- **VKMS**, a virtual display the kernel paces, for agents to measure everything: a kernel module
  and a seat of its own to set up, its pacing at 240 Hz not checked; the ROADMAP studies it at
  Stage 2.
- **Each compositor's own tools** (KWin's frame log, Mutter's frame rate, Hyprland's overlay):
  precise, but not comparable, and missing for five of them.
- **wcbench** (2026-10-29), a bench of the logged-in session: its method's rules are taken, not its
  code, which measures one session only and cannot run headless.
- **The tour in the owner's daily session**: no logging out, but it measures the shell and the
  owner's windows with the compositor.

## Consequences

- The bench lives in the repository and is kept working, since the rivals are measured again at
  each stage's end.
- The tour's command, begun at M1.9 on branch `m1.9-tour` and not merged, is finished at Stage 2,
  with its dry run headless; the owner runs it at Stage 2's exit, its steps then written in
  `docs/owner-runs.md`.
- A tour takes the display: only the owner runs it, from a virtual terminal of their own.
- niri and cosmic-comp have no figures until Stage 2's exit.

## Validation and reconsideration

Evidence at the time: the rivals' sources and the web survey of 2026-11-15; the bench's headless
series of 2026-11-16, 189 cases at equal work, none disturbed ([measurements](../measurements.md)).
Reviewed reasoning: only a real display gives every compositor the same clock, and only a tour
with Yuna in it gives a comparison a budget can use. Proposed again if a headless backend that
paces every rival on one clock becomes usable (VKMS, studied at Stage 2), or if Stage 2's exit
tour cannot be run.
