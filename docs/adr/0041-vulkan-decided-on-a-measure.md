# 0041. Vulkan decided on a measure

- Status: Accepted; amends [0012](0012-opengl-first.md) (how Vulkan is decided)
- Date: 2026-11-13

## Context

[ADR-0012](0012-opengl-first.md) draws through Smithay's GLES and moves to Vulkan when a measure
shows OpenGL misses a budget, or when Smithay's Vulkan renderer exists. The owner asked whether
Yuna should draw through Vulkan, for performance, latency, HDR and effects
([survey](../survey.md#an-own-base-and-vulkan)).

- HDR through Vulkan is in production in jay and gamescope only. KWin and Hyprland do HDR in
  OpenGL; KWin plans a Vulkan renderer, an issue open since 2023. wlroots does colour only on its
  Vulkan renderer, which it tries after GLES2, and has HDR banding there.
- Smithay's Vulkan renderer is a draft (pull request 2165, opened 2026-09-09); its author is "not
  sure" he will do HDR on GLES, and Yuna leaves Smithay anyway
  ([ADR-0039](0039-leaving-smithay-step-by-step.md)).
- No compositor has published a frame's cost, latency or power in GL against Vulkan. The one power
  report, on Intel with wlroots, has Vulkan using more.
- Explicit sync and fp16 blending are not reasons: GLES has both already (Mutter, KWin, Smithay).
- Mesa's source gives every extension a compositor needs on RADV, ANV and lavapipe, and Vulkan 1.4
  to the laptop's Vega 3; jay says modifiers fail on AMD before RX 5000. No one has read the
  laptop's driver.

## Decision

The owner chose, on 2026-11-13:

- **A throwaway prototype measures Vulkan against GLES** before anything is decided: the same
  scene (`frame_time`'s: two `vkcube` windows covering a 4K output, every frame damaged whole),
  each frame's GPU and CPU time and the power, on llvmpipe and lavapipe, on the RX 6900 XT, and
  on the laptop, which the owner runs. It comes with M1.9, beside the rivals' figures.
- On its figures, the owner chooses: Vulkan alone, Vulkan with GLES kept as a plain SDR fallback
  (as jay keeps OpenGL), or GLES. That choice is its own ADR.
- First, the owner reads the laptop's Vulkan driver (`vulkaninfo --summary`, the Mesa and kernel
  versions).
- Until then, ADR-0012 holds: the colour pipeline, the layout and the animations do not depend on
  the renderer; only the final drawing is OpenGL.

## Alternatives considered

- **Vulkan alone, decided now**: one path to keep, but no figure, and no answer if the laptop
  fails.
- **GLES until a budget is missed** (ADR-0012 as it was): prudent, but HDR on GLES would be Yuna's
  alone to write, and the comparison that decides it costs one prototype.

## Consequences

- The prototype needs the owner's consent for its GPU measures, one at a time
  ([AGENTS.md](../../AGENTS.md)), and a Vulkan binding for its own code, put to the owner as a
  dependency then (ash, jay-ash or vulkanalia).
- A Vulkan renderer, if chosen, brings checks of its own: the validation and synchronisation
  layers in CI, and a test that `/dev/udmabuf`, which lavapipe needs for modifiers, is there.
