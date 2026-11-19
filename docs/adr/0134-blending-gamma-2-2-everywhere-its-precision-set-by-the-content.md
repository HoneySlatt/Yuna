# 0134. Blending: gamma 2.2 everywhere, its precision set by the content

- Status: Accepted; supersedes in part [0128](0128-hdr-virtual-outputs-and-when-colour-is-chosen.md) (what Yuna draws, and when its pipeline is chosen)
- Date: 2026-11-19

## Context

[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 5 has
the blending space and its precision chosen at Stage 2's opening, not at Stage 5, and colour as
one pipeline of standard steps whose final steps run in a fixed order: night light, filters,
dimming, display profile, encoding. The ROADMAP's bar is to beat KWin on HDR and colour.

Read on 2026-11-19 ([survey](../survey.md#blending-and-a-compositors-priority)): KWin blends in
gamma 2.2 in [0, 1] since Plasma 6.2, having left linear light in 16-bit floats because
applications expect gamma blending and linear blending made a translucent window look different
once HDR was on (KDE bug 476868); Mutter (linear for ICC profiles), jay by default, Hyprland in
SDR and niri blend in an encoded space; wlroots' Vulkan renderer and gamescope blend in linear light. For HDR, Mutter,
Hyprland, wlroots' Vulkan renderer, Windows and KWin's accuracy mode keep 16-bit floats; KWin's
default keeps 10 bits.

Measured in M1.9's prototype: 16-bit floats cost 19 to 33 % more GPU time than 8 bits, before any
final pass (56 against 75 µs a 4K composition on the desktop; 1.1 against 1.4 ms at 1080p on the
laptop), and twice the memory, 63.3 MiB a 4K image. Computed in the challenge: gamma 2.2 as a pure
power law blends the same at any luminance scale, so SDR content blends alike in SDR and in HDR,
and a per-channel gain is exact on its encoded values; 10 bits of gamma 2.2 over 0 to 1,015 cd/m²
(the owner's monitor) keep 15 codes below 0.1 cd/m², where an HDR10 film has 55.

The laptop's display engine (DCN 1.0) has no colour step after blending; amdgpu's plane colour
pipelines are not in Linux 6.18; the headless mode has no display hardware at all.

## Decision

The owner chose, on 2026-11-19, **gamma 2.2 as the one blending space, on every tier and in every
mode, and a precision set by the content's range.**

- **Gamma 2.2 as a pure power law**, the space clients' sRGB content is taken in, as KWin does.
- **SDR**: Yuna blends straight into the output's buffer, in 10 bits where the plane offers them
  and no underlay needs alpha (the same memory as 8 bits), in 8 bits otherwise.
- **HDR** (Stage 5): Yuna blends gamma 2.2 in an intermediate image of 16-bit floats, values above
  1.0 for headroom and below 0 for wide gamut, then one final pass over the damage writes the
  output's encoding (PQ) in 10 bits. About 63 MiB more a 4K HDR output, and about 19 µs more a
  composition, on the desktop alone.
- **When the final steps need an intermediate**: Yuna blends straight into the output's buffer only
  when no final step is needed, or every needed one is a per-channel curve the display applies
  after blending; otherwise it blends into an intermediate image and one final pass writes the
  output. Gains (night light, dimming) act on encoded values; matrices, lookup tables and profiles
  decode first. The order of ADR-0082 item 5 stands.
- **The shader's final pass is the reference.** CI's captures check it headless. A step moved to
  the display hardware is an optimisation, turned on per display engine once a probe on the
  owner's hardware shows the same result.
- **Parameters of each scene target**: the blend primaries, the image's format and its alpha bits
  ([ADR-0133](0133-yunas-scene-retained-its-damage-found-at-each-latch.md)), so that planes at
  Stage 4 and colour at Stage 5 change data, not the core. An underlay needs 8 alpha bits. The
  primaries in SDR on a wide-gamut profile are chosen at Stage 5.
- **What Yuna draws, and when**: Smithay's GLES renderer draws as before, 8 bits and no
  conversion, until Stage 2a's Vulkan renderer, which draws SDR as above. The conversion of HDR
  outputs to PQ, tone mapping and a profile per display are Stage 5's
  ([ADR-0061](0061-yunas-uses.md) point 4); what an HDR or SDR output announces stays
  [ADR-0128](0128-hdr-virtual-outputs-and-when-colour-is-chosen.md)'s.
- **An effect that needs linear light** (blur at Stage 6) works in its own images, budgeted then;
  the blending space does not change for it.

## Alternatives considered

- **Gamma 2.2 in 10 bits everywhere** (KWin's default for HDR): no intermediate, the least memory; but the
  shadows of composited HDR lose three to six times their steps, and the bar is to beat KWin.
- **Linear light in 16-bit floats everywhere** (wlroots' Vulkan renderer, gamescope): twice the
  memory and bandwidth on every tier, the laptop's included; translucency unlike what applications
  expect, which KWin left.
- **Per tier, gamma on the laptop and linear on the desktop**: two looks of the same window, two
  paths to test.

## Consequences

- Stage 2 blends SDR only; the HDR intermediate and its final pass come at Stage 5, the pipeline's
  shape from Stage 2a.
- The memory budget of the GPU, set in SDR, gets an HDR figure at Stage 5, against KWin and Mutter
  in HDR the same day.
- An `A2B10G10R10` composition is timed on each tier when the renderer is written, as M1.9 timed 8
  bits and 16-bit floats.
- A test from Stage 2a counts the distinct output codes per luminance band of a ramp through the
  pipeline, against its source's.

## Validation and reconsideration

Evidence: the survey and challenge of 2026-11-19 (KWin, Mutter, wlroots, jay, gamescope, Hyprland
sources; the kernel's DCN 1.0 capabilities), M1.9's measured costs, the challenge's computed
quantisation. Not observed: whether 10-bit HDR shadows show worse by eye on the owner's monitor.
Proposed again if the owner's eye or a probe at Stage 5 shows another choice better, or if the HDR
intermediate's cost misses Stage 5's budgets.
