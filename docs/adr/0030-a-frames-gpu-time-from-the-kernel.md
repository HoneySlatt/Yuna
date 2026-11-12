# 0030. A frame's GPU time read from the kernel, the renderer's timers for effects

- Status: Accepted; amends the [ROADMAP](../ROADMAP.md#budgets)'s budget "A frame's cost"
- Date: 2026-11-12

## Context

The ROADMAP's budget "A frame's cost" is GPU and CPU time per frame, set from the rivals' at
Stage 1 (M1.9), measured by "the renderer's timers": OpenGL's timer queries inside Yuna's
renderer. M1.4 measures that cost on a 4K output for the first time. Found then, on the owner's
desktop (RX 6900 XT, amdgpu, Linux 7.3.1):

- The kernel counts, for each DRM client, the time each GPU engine spent on its work, in
  nanoseconds: the `drm-engine-<engine>` keys of the DRM usage statistics, in the `fdinfo` of the
  process's render node (`/proc/<pid>/fdinfo/<fd>`, the kernel's
  `Documentation/gpu/drm-usage-stats.rst`). amdgpu gives `drm-engine-gfx` and
  `drm-engine-compute`. Anyone may read them from outside the process, for any compositor.
- Smithay `8fd9fe0` binds `GL_EXT_disjoint_timer_query`; its own GPU spans need its
  `tracy_gpu_profiling` feature, which adds tracy-client and the Tracy profiler.
- On llvmpipe there is no GPU: Mesa's `llvmpipe-N` threads rasterize, and their CPU time is the
  GPU's work.

## Decision

- **A frame's GPU time is read from the kernel's DRM usage statistics**: the GPU engines' busy
  time of Yuna's DRM clients over a measured window, divided by the frames drawn in it. M1.4 and
  M1.9 measure Yuna and its rivals this way, from outside.
- **On llvmpipe, the CPU time of its rasterizer threads** stands for the GPU's, reported apart from
  the rest of Yuna's CPU time.
- **The renderer's timers come at Stage 6**, when each effect's GPU time is measured
  (the ROADMAP's Stage 6 exit), as Yuna's own code on the timer queries Smithay binds.

The owner chose this on 2026-11-12.

## Alternatives considered

- **The renderer's timers from M1.4 on**, as the ROADMAP said: a time per pass, but about 40 lines
  of OpenGL code in the renderer now, and they measure Yuna alone: M1.9's rivals would need
  another instrument.
- **Both at once**, to compare them: more code and work now, for a breakdown nothing uses before
  Stage 6.
- **Smithay's `tracy_gpu_profiling`**: spans per pass for free in Smithay's code, but a new
  dependency, the Tracy profiler as a tool, and a feature turned on in Smithay.

## Consequences

- Nothing runs inside Yuna to measure its GPU time; `crates/yuna/examples/frame_time.rs` reads
  it, with the CPU time of each thread from `schedstat`.
- The figure is a total per frame: what Yuna sent the GPU, its clients' buffers' imports and
  copies included, with no time per pass.
- It needs a driver that gives the statistics: amdgpu does, on the desktop and on the lowest
  tier's Radeon Vega; another driver is checked when a machine with it is measured.
- The ROADMAP's budget names this measure.
