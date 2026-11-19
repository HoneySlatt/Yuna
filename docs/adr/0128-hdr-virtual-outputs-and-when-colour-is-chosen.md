# 0128. HDR virtual outputs, and when Yuna's colour pipeline is chosen

- Status: Accepted; supersedes
  [0048](0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md)
- Date: 2026-11-18

## Context

[ADR-0048](0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md) decided what an HDR virtual
output announces, and left Yuna's pipeline to Stage 5, "on the renderer M1.9 chooses".
[ADR-0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)
then chose Vulkan without waiting for M1.9, and
[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 5 moved
the choice of the blending space and its precision to Stage 2's opening. No status said so: a
reader of ADR-0048 alone got the schedule wrong.

This ADR gathers the rule as it stands on 2026-11-18. It decides nothing new. The owner's
monitor's EDID, read on 2026-11-14: PQ and BT.2020 accepted; desired content luminance 1015.2
cd/m² at most, 265.0 on a frame's average, 0.000156 at least; panel primaries red 0.6835, 0.3046,
green 0.2441, 0.7089, blue 0.1435, 0.0556, white 0.3134, 0.3291. What an output announces is the
description clients aim at.

ADR-0048 left the whole pipeline to Stage 5 on the renderer M1.9 would choose; ADR-0073 chose
Vulkan and ADR-0082 moved the blending space to Stage 2's opening. The later rule is the one below.

## Decision

- **An HDR output announces the monitor's signal** (ADR-0048, 2026-11-14): PQ, BT.2020 primaries,
  luminances 0.005, 10,000 and a reference white of 203 cd/m² (BT.2408, the protocol's PQ default).
  Its target volume is the monitor's EDID: its panel primaries, 0.0002 to 1015 cd/m², max_cll 1015,
  max_fall 265, rounded to the protocol's units. The transfer function is provisional: Stage 5 may
  announce Yuna's blending space instead. Tests check the target volume, not the transfer
  function.
- **An SDR output announces gamma 2.2 and sRGB primaries**, the protocol's default luminances
  (ADR-0048).
- **A surface's preferred description** is its window's workspace's output's, whether the window is
  scrolled into view or not; before its window maps, its seat's focused output's. A subsurface or a
  popup takes its window's (ADR-0048).
- **Each surface's description is recorded** at its commit, for tests and agents to read
  (ADR-0048).
- **Yuna draws as before**: 8-bit, no conversion (ADR-0048).
- **When the pipeline is chosen.** The blending space and its precision are chosen at Stage 2's
  opening, per tier, on a measure (ADR-0082 item 5, 2026-11-17), for Yuna's Vulkan renderer
  ([ADR-0106](0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md)). The output's format,
  conversion and tone mapping are Stage 5's, with a profile per display
  ([ADR-0061](0061-yunas-uses.md) point 4) (ADR-0048).

## Alternatives considered

- **KWin's blending space announced now** (gamma 2.2 over BT.2020)
  ([ADR-0048](0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md)): it would presume the
  choice before it is made.
- **A minimal pipeline now** (a 10-bit buffer, sRGB converted to PQ)
  ([ADR-0048](0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md)): honest captures, but
  work on a renderer Stage 2 replaces.

## Consequences

- A capture of an HDR output shows the clients' values, unconverted: debt until Stage 5.
- The monitor's values are one constant, named after it, with its EDID as source.

## Validation and reconsideration

Evidence: the tests that read the target volume of an HDR output and each surface's recorded
description, mpv's among them
([ADR-0122](0122-real-applications-in-ci-and-rest-checked-on-the-program.md)).
