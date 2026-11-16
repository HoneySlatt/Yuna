# 0071. The rivals measured: costs headless by agents, frames, latency and watts on a real display by the owner

- Status: Accepted
- Date: 2026-11-15

## Context

M1.9 measures dwl, sway, Hyprland, niri, mango, Umbriel, KWin, Mutter and cosmic-comp, so that
the bar's budgets rest on facts ([ROADMAP](../ROADMAP.md#3-quality-bar)). No controlled
comparison of them exists ([survey](../survey.md#the-rivals-measured)).

Their sources, read on 2026-11-15, show that they cannot all be measured the same way without a
display:

- sway, dwl, mango and Umbriel run on wlroots' headless backend, which draws a fixed number of
  milliseconds after each commit, with no grid of refreshes: a frame there is never late. Hyprland
  and Mutter present at "now". Only KWin's virtual backend and Yuna keep a grid of refreshes
  ([ADR-0023](0023-frames-presentation-and-the-clock.md)).
- Hyprland cannot start without a DRM session or a parent compositor; niri's headless backend
  serves only its own tests; cosmic-comp has none.
- A client that asks frame callbacks makes each of them draw: a client cannot count a
  compositor's own frames without changing them.

The owner's monitor and laptop panel give every compositor the same clock: the kernel's.

## Decision

The owner chose, on 2026-11-15:

- **One bench, outside the compositor, measures every compositor the same way.** It reads the
  compositor's process tree from `/proc` and the kernel's DRM usage statistics, as
  [ADR-0030](0030-a-frames-gpu-time-from-the-kernel.md) does for Yuna. Each rival runs in a
  sandbox of its own: a cleared environment, its own runtime folder and session bus, no socket
  or bus of the owner's. Each is measured twice: as shipped, and at equal work (effects,
  wallpaper and XWayland off). The bench's own clients are counted apart.
- **Agents measure, headless, what does not depend on the clock**: CPU and GPU time per frame
  drawn, wake-ups and frames at rest, memory, start time. The figures of rivals that cannot run
  headless come from the tour.
- **The owner measures, by hand, what does**: a *tour* on each machine, the desktop, the laptop
  and the Steam Deck. From a spare virtual terminal, one command starts each rival on the real
  display in turn, runs the same rounds, and stops it. It gives frames missed while moving,
  input to present end to end against gamescope and KWin
  ([ADR-0032](0032-input-to-present-read-by-mode.md)), and the GPU's or the battery's watts. The
  keys it sends reach only the compositors it started, never the owner's own session.
- **The GPU's performance level is `auto` during a tour.** The owner sets it before and puts
  back their own after; the tour reads it and refuses to start on another level. Agents never
  change it; their measures record it, with the GPU's clocks.
- **A tour now, for the rivals; another at Stage 2's exit, with Yuna in it**, when Yuna drives a
  display, so that watts are compared on the same day, kernel and drivers. Until then, Yuna's
  motion is judged headless by [ADR-0056](0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md).

## Alternatives considered

- **Everything headless, the limit written**: no time of the owner's, but missed frames, latency
  and watts compared on clocks that differ, or not at all until Stage 2.
- **VKMS**, a virtual display the kernel paces, for agents to measure everything: a kernel
  module and a seat of its own to set up, and its pacing at 240 Hz not checked; the ROADMAP
  studies it at Stage 2.
- **Each compositor's own tools** (KWin's frame log, Mutter's frame rate, Hyprland's overlay):
  precise, but not comparable, and missing for five of them.
- **wcbench** (2026-10-29), a bench of the logged-in session: its method's rules are taken, not
  its code, which measures one session only and cannot run headless.
- **The tour in the owner's daily session**: no logging out, but it measures the shell and the
  owner's windows with the compositor.

## Consequences

- The bench lives in the repository and is kept working, since the rivals are measured again at
  each stage's end ([ADR-0072](0072-budgets-set-against-the-peers-the-same-day.md)).
- The owner runs three tours at M1.9 and one at Stage 2's exit; the steps are in the PLAN.
- A tour takes the display: only the owner runs it, from a virtual terminal of their own.
- Yuna enters the tour at Stage 2; its headless figures are compared to the rivals' headless
  ones only on what the clock does not decide.

## Validation and reconsideration

Evidence at the time: the rivals' sources and the web survey of 2026-11-15, and the bench's first
runs on the desktop. Reviewed reasoning: only a real display gives every compositor the same
clock. Proposed again if a headless backend that paces every rival on one clock becomes usable
(VKMS, studied at Stage 2), or if the tours cannot be run.
