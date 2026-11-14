# 0047. HDR virtual outputs: PQ announced, drawn at Stage 5

- Status: Accepted
- Date: 2026-11-14

## Context

M1.6 makes a virtual output look to clients like the owner's monitor in HDR. The monitor's EDID,
read on 2026-11-14: PQ and BT.2020 accepted; desired content luminance 1015.2 cd/m² at most,
265.0 on a frame's average, 0.000156 at least; panel primaries red 0.6835, 0.3046, green 0.2441,
0.7089, blue 0.1435, 0.0556, white 0.3134, 0.3291.

What an output announces is the description clients aim at. KWin, the bar for HDR
([ROADMAP](../ROADMAP.md#3-quality-bar)), announces its blending space, gamma 2.2 over BT.2020
with the headroom in its luminances; jay announces PQ with the display's volume. Stage 5 decides
Yuna's pipeline, on the renderer M1.9 chooses ([ADR-0041](0041-vulkan-decided-on-a-measure.md)).

## Decision

The owner chose, on 2026-11-14:

- **An HDR output announces the monitor's signal**: PQ, BT.2020 primaries, luminances 0.005,
  10,000 and a reference white of 203 cd/m² (BT.2408, the protocol's PQ default); its target
  volume is the monitor's EDID: its panel primaries, 0.0002 to 1015 cd/m², max_cll 1015,
  max_fall 265, rounded to the protocol's units. The transfer function is provisional: Stage 5
  may announce its blending space instead. Tests check the target volume, not the transfer
  function.
- **An SDR output announces gamma 2.2 and sRGB primaries**, the protocol's default luminances.
- **A surface's preferred description** is its window's workspace's output's, whether the window
  is scrolled into view or not; before its window maps, its seat's focused output's. A
  subsurface or a popup takes its window's.
- **Each surface's description is recorded** at its commit, for tests and agents to read.
- **Yuna draws as before**: 8-bit, no conversion. The format, conversion and tone mapping are
  Stage 5's.

## Alternatives considered

- **KWin's blending space** (gamma 2.2 over BT.2020): presumes Stage 5's choice to blend as KWin
  does before Stage 5 makes it.
- **A minimal pipeline now** (a 10-bit buffer, sRGB converted to PQ): honest captures, but Stage
  5's work on GLES, which M1.9 may replace.

## Consequences

- A capture of an HDR output shows the clients' values, unconverted: debt until Stage 5.
- The monitor's values are one constant, named after it, with its EDID as source.
