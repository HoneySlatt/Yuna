# 0006. Machines and GPUs

- Status: Accepted
- Date: 2026-11-11

## Context

The owner's machines: a desktop with an RX 6900 XT and a Ryzen 7 5800X, an ASUS PG32UCDM (QD-OLED,
3840 × 2160, 240 Hz, scale 1.5), a second monitor and a television; a very modest HP laptop (AMD
Athlon Silver 3050U, two cores, Radeon Vega graphics, 4 GiB of single-channel memory) on niri; a
Steam Deck, whose game mode runs gamescope.

## Decision

- **Linux only.**
- **The high tier** is the desktop: 3840 × 2160 at 240 Hz, 4.17 ms a frame.
- **The lowest tier** is the HP laptop: every feature works there, effects lighter, never less fluid
  at its own refresh rate.
- **The Steam Deck's desktop mode** is supported; its game mode keeps gamescope.
- **AMD and Intel GPUs first**, on Mesa. **NVIDIA** comes at a later stage of its own.

## Alternatives considered

- **AMD only**: simplest, but Intel shares Mesa's drivers and costs little more.
- **NVIDIA from the start**: its driver's differences would slow every stage.
- **Replacing gamescope on the Deck**: Steam depends on it closely; not worth it now.

## Consequences

- The laptop's display, storage and Vulkan driver are still to be read; its measures wait for them.
- CI tests the headless mode on a CPU renderer (Mesa's llvmpipe or lavapipe).
