# 0077. The budgets of a frame, memory and rest, set from the rivals

- Status: Accepted; superseded in part by [0083](0083-a-windows-last-wl-shm-buffer-kept-until-its-next.md) (the clients' buffers
  Yuna holds)
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

Two facts of that series shape the budgets:

- **A frame's cost compares across backends** when the frames are counted the same exact way: in
  the game, mpv counts the frames it shows, in the measured window itself, whatever the
  compositor's pace. What does not compare across backends is the count of frames itself, and a
  cost a second, which follows the pace (wlroots' headless backend draws 4 ms after each commit,
  the others at the mode's rate).
- **Memory is two things.** A compositor's process holds its own memory, and may hold its
  clients' `wl_shm` buffers mapped: wlroots' rivals release a buffer once they have copied it,
  Yuna keeps it until the client replaces it, so its proportional set size grows with the clients'
  buffers (`Pss_Shmem`), and only slightly with its own (`Pss_Anon` and `Pss_File`). Its GPU memory, which
  the process's figures leave out, is system memory on the laptop and the Deck; at 15 windows it
  ranged among the peers from 115 MiB (KWin) to 1,159 MiB (Hyprland).

## Decision

The owner chose the method in ADR-0072, and on 2026-11-16 that memory counts the process and its
GPU apart; this ADR applies them to the figures of 2026-11-16, on the high tier. Each figure is
the best peer's lowest run, and each growth a window the difference of the means of its
three runs at 1 and at 150 windows.

- **A frame's cost**: in the game scenario (mpv fullscreen at 240 frames a second, Vulkan on radv,
  its frames counted by mpv), Yuna's CPU and GPU time a frame are at most the best peer's at equal
  work, the same day: **198 µs of CPU and 203 µs of GPU**, Umbriel's on both. Yuna: 160 to 166 µs
  and 191 to 196 µs: met. The floor, sway's and dwl's: 150 µs and 215 µs.
- **The process's memory** (`Pss_Anon` + `Pss_File`) at rest with 15 still windows, and its growth
  a window from 1 to 150, at most the best peer's: **102.5 MiB**, Umbriel's, and **0.0063 MiB a
  window**, Hyprland's. Yuna: 83.6 to 84.0 MiB, met; 0.0087 MiB a window, missed, debt: each of its
  nine pairs of runs is above the figure, though the bench cannot tell its range (0.0075 to
  0.0103) from Hyprland's (0.0054 to 0.0079).
- **The GPU's memory** (the kernel's DRM `resident-vram` + `resident-gtt`) at rest with 15
  windows, and its growth a window, at most the best peer's: **115.4 MiB**, KWin's, and **2.31 MiB
  a window**, Hyprland's. Yuna: 115.1 MiB, met, 0.3 MiB below and the same in every run; 0.43 MiB a
  window, met.
- **The clients' buffers a compositor holds** (`Pss_Shmem`) are recorded beside, not a budget:
  Yuna 30.4 MiB at 15 windows and 0.31 MiB a window; the wlroots rivals, Umbriel and Hyprland
  none; KWin and Mutter 39.6 MiB and 2.6 MiB a window. Yuna's are debt: a buffer is released once
  it is uploaded ([ADR-0074](0074-the-requirements-of-stage-2s-core.md)'s bounded memory and
  release from the last user).
- **At rest**: 0 frames, 0 own wake-ups a second, 0 % of a core, unchanged; Yuna used no CPU time
  in any rest window, so no thread of it woke. Umbriel woke 10.1 times a second, Hyprland 3.5.
- **Motion** keeps its rule, no missed frame at 240 Hz, judged headless on the GPU until Stage 2's
  exit ([ADR-0056](0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md)); Yuna meets
  it.
- **Input to present, power and the laptop's figures** wait for the tour at Stage 2's exit
  ([ADR-0076](0076-input-to-present-read-by-mode.md), ADR-0075).
- **Start** stays set at Stage 2 ([ROADMAP](../ROADMAP.md#budgets)); the series' start to ready is
  recorded in the measurements, accurate to the bench's 50 ms polling.
- **The figures are checked** by the bench's equal-work series, run again at each stage's end; no
  test in `scripts/check.sh` holds them, since they need the GPU and the rivals.

## Alternatives considered

- **The best of all the rivals** (dwl's 150 µs of CPU, sway on Vulkan's 84 MiB): rejected by
  ADR-0072.
- **One total of the process and the GPU**: one figure, Yuna's below the peers' (199 MiB against
  KWin's 323, the lowest), but a difference would not say where it comes from.
- **The process's proportional set size whole**, as first written: Yuna would miss it for the
  clients' buffers it keeps, and the GPU's memory, larger than the process's for most rivals,
  would be left out.
- **A frame's cost compared only within one backend**: Yuna, alone on its backend, would have no
  comparison at all; a cost a frame does not depend on the pace once frames are counted exactly.

## Consequences

- The ROADMAP's budgets of a frame's cost and memory carry their rule and these figures; the
  figures change at each stage's end, when the rivals are measured again the same day.
- Debt: the process's growth a window, and the clients' buffers Yuna keeps
  ([debt](../debt.md#architecture)).
- A window's size depends on each compositor's layout, which the series did not record: growth a
  window compares windows of different sizes, within each rival's own layout.
- Umbriel as shipped, its blur and shadows on, drew a game frame in 169 µs of GPU, below its
  equal-work 203 to 207 µs: unexplained; one run, from a series other programs disturbed.
- niri, a peer, is in no figure until Stage 2's exit; the budgets may tighten then.

## Validation and reconsideration

Evidence at the time: the headless series at commit `627f160`, 189 cases, three runs each, none
disturbed, the GPU at the owner's level `high`. Not measured: niri, cosmic-comp, any real display,
the laptop and the Deck under the rivals, window sizes. Proposed again if the peers measured the
same day at Stage 2's exit, niri among them, change which is best, or if the GPU's level `auto`
changes the order.
