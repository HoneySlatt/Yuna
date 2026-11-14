# 0044. XWayland across outputs of different scales

- Status: Accepted
- Date: 2026-11-14

## Context

X11 has one scale for every output. The owner's desktop mixes scales: the main monitor at 1.5, a
second at 1. No compositor in the [survey](../survey.md#games) is known to keep an X11 window both
sharp and the right size on each output:

- Umbriel draws X11 at the largest scale and does not correct it elsewhere: seen by the owner on
  2026-11-14, a window sharp at 1.5 stays 1.5 times too large when moved to the output at 1.
- KWin, when applications scale themselves, has the same limit.
- COSMIC's `descale_xwayland` was not checked across outputs of different scales.

The [ROADMAP](../ROADMAP.md#references) named Umbriel as the rival to beat on XWayland at a
fractional scale; no rival reaches what the owner expects.

## Decision

The owner chose, on 2026-11-14: **X11 applications draw at the largest output scale, and Yuna
scales each X11 window down on outputs of a smaller scale.** The approach is the one proposed at
Linux Plumbers 2019 (Roman Gilg, "XWayland multi DPI"). An X11 window keeps its size on every
output, including when moved between them; it is sharp on the outputs of the largest scale and a
little softer on the others.

The bar is this behaviour, not a rival's.

## Alternatives considered

- **One XWayland per application**, named by KWin's developers as a possible fix: each application
  could be given the scale of its output. Most X11 applications read their scale once, at start, so
  it does not help a window moved later; it costs one X server per application. Nobody ships it.
- **Umbriel's behaviour**: one scale, no correction. The simplest, but X11 windows are too large on
  outputs of a smaller scale.

## Consequences

- M1.8's done-when adds an X11 window moved between virtual outputs at 1.5 and 1: the right size
  on both, sharp at 1.5.
- The downscale needs a filter that stays sharp at a ratio of 2/3; captures against reference
  images check it.
- An X11 application that does not scale itself is small on the outputs of the largest scale. It
  needs a per-application rule that draws it at scale 1, scaled up: the right size, but soft.
- A fullscreen X11 game on an output of a smaller scale sees that output larger than its mode and
  would draw more pixels than it shows. Games come first: M1.8's design finds how such a game draws
  at the output's own mode, or states the cost.
