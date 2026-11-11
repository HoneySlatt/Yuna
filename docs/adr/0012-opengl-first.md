# 0012. OpenGL first, the pipeline independent of the renderer

- Status: Accepted
- Date: 2026-11-11

## Context

KWin and Hyprland do HDR in OpenGL; jay in Vulkan, with about 9,900 lines and 248 `unsafe` blocks
in its renderer. Smithay's GLES renderer is the one niri and cosmic-comp use; it takes custom
shaders, on which niri built its blur. Smithay's main author is writing a Vulkan renderer. Power at
rest depends on drawing nothing and on direct scanout, not on the graphics API. wgpu cannot import
clients' dmabufs, their formats and their fences without bypassing its own safety, and no
compositor uses it.

## Decision

Yuna draws through **Smithay's GLES renderer** first. The layout, the animations and the colour and
HDR pipeline are written independently of the renderer; only the final drawing is OpenGL. Each
milestone that draws measures the GPU and CPU time a frame and the power. Yuna moves to Vulkan when
a measure shows OpenGL misses a budget, or adopts Smithay's Vulkan renderer once it exists.

## Alternatives considered

- **Yuna's own Vulkan renderer first**: full control of memory, queues and synchronisation, but the
  riskiest code first, months before a first window, and a duplicate of Smithay's work.
- **wgpu**, as the owner's other projects: see the context.

## Consequences

- Shaders are kept in one place, small, so that a move to Vulkan replaces a part, not the design.
