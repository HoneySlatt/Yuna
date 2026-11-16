# 0072. Budgets set against the peers, the same day

- Status: Accepted
- Date: 2026-11-15

## Context

The ROADMAP's bar names a rival to beat for most criteria: niri for scrolling, Hyprland for
animations, dwl and mango for power, KWin for HDR, gamescope and KWin for latency. It names none
for memory and a frame's cost, which M1.9 sets from the rivals
([ROADMAP](../ROADMAP.md#budgets)).

The rivals do not do the same work. By default Hyprland blurs and shadows and draws its own
wallpaper; niri starts a bar; mango starts XWayland; sway and dwl draw almost nothing and animate
nothing. The cheapest compositor would always be one that does least.

An absolute figure, such as a GPU time a frame, holds for one machine, kernel and Mesa; five
years on, or on another GPU, it says nothing.

## Decision

The owner chose, on 2026-11-15:

- **Memory and a frame's cost are set against the peers in features**: the best of niri,
  Hyprland, Umbriel and KWin, each measured at equal work
  ([ADR-0071](0071-the-rivals-measured-headless-by-agents-and-on-a-real-display-by-the-owner.md)).
  sway's and dwl's figures are recorded as the floor, not as a budget. The other criteria keep
  the rival the bar names.
- **A budget is a comparison, and the figure it gives is checked.** Its rule reads "Yuna at most
  the reference, on the same scenario and machine tier, measured the same day". The figure the
  last such measure gave is the one Yuna's checks hold it to, until the rivals are measured again
  at the next stage's end.

## Alternatives considered

- **The best of all the rivals**: the hardest bar, most likely dwl's; Yuna, with springs, its
  X11 manager and uwui, would carry the debt from the start.
- **One named rival for both**: simple, but one point of comparison.
- **Absolute figures alone**: simple to check, out of date at each kernel, Mesa or machine.
- **Comparisons alone**: fair, but nothing to check between two measures.

## Consequences

- The bench of ADR-0071 records each run's machine, kernel, Mesa, firmware, GPU level and
  clocks, so that a comparison is made only between runs of the same day and state.
- The ROADMAP's budgets are written as their rule and their current figure; the figures change
  at each stage's end, the rules only by an ADR.
- Yuna may cost more than sway or dwl; it must not cost more than the peers.

## Validation and reconsideration

Evidence at the time: the rivals' defaults read in their sources (what each draws and animates)
and the bench's first equal-work runs. Proposed again if the peers' equal-work figures turn out
too far apart to name one best, or if a new rival joins the peers in features.
