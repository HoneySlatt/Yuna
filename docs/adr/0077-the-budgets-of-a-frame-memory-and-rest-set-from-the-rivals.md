# 0077. The budgets of a frame, memory and rest, set from the rivals

- Status: Accepted
- Date: 2026-11-16

## Context

The ROADMAP's bar left a frame's cost and memory to be "set from the rivals' at Stage 1"
([ROADMAP](../ROADMAP.md#budgets)). [ADR-0072](0072-budgets-set-against-the-peers-the-same-day.md)
decided how: against the peers in features (niri, Hyprland, Umbriel, KWin) at equal work, as a
comparison measured the same day, with the figure that comparison gives checked until the next
stage's end. [ADR-0075](0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)
measures headless now what the clock does not decide, and the rest on real displays at Stage 2's
exit.

The headless series of 2026-11-16 ran every rival but niri and cosmic-comp, which cannot run
headless, at equal work, three runs of every case, none disturbed, on the high tier's desktop at
3840 × 2160, 240 Hz, scale 1.5 ([measurements](../measurements.md#m19--the-rivals-headless-at-equal-work-2026-11-16)).
Of the peers, Umbriel, Hyprland and KWin were measured; niri waits for the tour.

## Decision

The owner chose the method in ADR-0072; this ADR applies it to the figures of 2026-11-16, on the
high tier.

- **A frame's cost**: in the game scenario (mpv fullscreen at 240 frames a second, Vulkan on radv,
  its frames counted by mpv), Yuna's CPU and GPU time a frame are at most the best peer's at equal
  work, the same day. The best peer was Umbriel on both: **198 µs of CPU and 203 µs of GPU a
  frame**, the lowest of its three runs, the figures checked until Stage 2's end. Yuna measured
  160 to 166 µs and 191 to 196 µs: met. The floor, sway's and dwl's, was 150 µs and 215 µs.
- **Memory**: at rest with 15 still windows, the proportional set size of Yuna's process is at
  most the best peer's at equal work, the same day, and so is its growth a window from 1 to 150
  windows. The best peer was Umbriel: **102 MiB, and 0.03 MiB a window**. Yuna measured 114 MiB and
  0.32 MiB a window: missed, debt until met ([debt](../debt.md#architecture)); Stage 2's bounded GPU
  memory and damage-only uploads ([ADR-0074](0074-the-requirements-of-stage-2s-core.md)) are where
  it is paid.
- **At rest**: 0 frames, 0 own wake-ups a second, 0 % of a core, unchanged; the peers did not all
  meet it (Umbriel 10.1 wake-ups a second, Hyprland 3.5), Yuna does.
- **Motion** keeps its rule, no missed frame at 240 Hz, judged headless on the GPU until Stage 2's
  exit ([ADR-0056](0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md)); Yuna meets
  it.
- **Input to present, power and the laptop's figures** wait for the tour at Stage 2's exit
  ([ADR-0076](0076-input-to-present-read-by-mode.md), ADR-0075). The headless costs a second that
  drive power are recorded as context, not as a budget: in the game, dwl used 36.5 to 37.1 ms of
  CPU and 52.4 to 52.9 ms of GPU a second at 244 frames a second, Yuna 38.4 to 39.8 and 45.7 to
  47.0 at 240.
- **Start** stays set at Stage 2 ([ROADMAP](../ROADMAP.md#budgets)); the rivals' start to ready,
  headless, is recorded: dwl 76 ms, mango 131 ms, Mutter 126 to 192 ms, Umbriel 226 to 286 ms,
  Hyprland 337 to 377 ms with its parent.
- **The Vulkan prototype's floor**, 56 µs of GPU a 4K composition in 8 bits and 75 µs in 16-bit
  floats on the desktop ([ADR-0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)),
  is no budget: it shows Stage 2's renderer has room far below the peers' figure.

## Alternatives considered

- **The best of all the rivals** (dwl's 150 µs of CPU): rejected by ADR-0072.
- **The figures of the next best peer for memory** (Hyprland's 132 MiB), so that Yuna meets it
  now: the bar would be lowered to fit the code.
- **Budgets for the laptop and the CPU renderer now**: no rival was measured there; set at Stage
  2's exit tour and from lavapipe's runs then.

## Consequences

- The ROADMAP's budgets of a frame's cost and memory carry their rule and these figures; the
  figures change at each stage's end, when the rivals are measured again the same day.
- Memory is debt: Yuna's process is 12 % above Umbriel's at 15 windows, and grows ten times
  faster a window.
- niri, a peer, is in no figure until Stage 2's exit; the budgets may tighten then.

## Validation and reconsideration

Evidence at the time: the headless series at commit `627f160`, 189 cases, three runs each, none
disturbed, on the desktop with the GPU at the owner's level `high`. Not measured: niri,
cosmic-comp, any real display, the laptop and the Deck under the rivals. Proposed again if the
peers measured the same day at Stage 2's exit, niri among them, change which is best, or if the
GPU's level `auto` changes the order.
