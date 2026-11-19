# 0098. On llvmpipe, dmabufs offered for no device and checked as llvmpipe maps them

- Status: Superseded by [0123](0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md)
- Date: 2026-11-18

## Context

M1.11's run of Claude Code through `yuna mcp` alone found that Yuna on llvmpipe, without
`--gpu`, died of SIGSEGV as soon as GTK 4 drew with the machine's GPU
([measurements](../measurements.md)). Read and reproduced on 2026-11-18:

- Yuna offered `linux-dmabuf` version 3, which names no device. Mesa's Vulkan then takes the
  compositor to be on its own GPU and shares its swapchain images, held in GPU memory the CPU may
  not map (`AMDGPU_GEM_CREATE_NO_CPU_ACCESS`).
- Mesa 26.2's llvmpipe maps a dmabuf's file to read it. When that fails, its import still
  succeeds, and the drawing reads through a null pointer.
- The tests never saw it: they force software rendering on their programs.

## Decision

Chosen while building M1.11, as implementation:

- **On llvmpipe, Yuna offers `linux-dmabuf` with its default feedback, naming no device**
  (`dev_t` 0), version 6. Mesa's Vulkan, seeing a device other than its own, copies each frame
  into a linear buffer llvmpipe can map. `--gpu` is unchanged: it names its render node.
- **On llvmpipe, every plane of a dmabuf is mapped as llvmpipe maps it**, then unmapped at once,
  before the import and again after it, and its rows must lie in its file, a stride holding a row
  of its pixels; a buffer that fails is refused: `failed`, or `invalid_wl_buffer` for
  `create_immed`, which ends that client alone. So llvmpipe offers only the formats of one plane
  whose bytes per pixel Yuna knows: 23 of its 57, its YUV formats left out.

## Alternatives considered

- **The check alone**: Yuna lives, but Mesa's Vulkan programs are ended and never show.
- **No `linux-dmabuf` on llvmpipe**, as Weston's software renderer: Mesa's Vulkan on a GPU then
  fails to make its surface, and the dmabuf path CI tests goes.
- **A real device named**: false for llvmpipe, and a GPU named brings the unmappable buffers back.

## Consequences

- `dev_t` 0 is Yuna's reading of the protocol, which says a device is "in general" a DRM node and
  tells clients not to compare device numbers; it steers only clients that do, as Mesa does. A
  GPU client that is not Mesa's, Chromium's by its source, may still send a buffer the processor
  cannot map, and is then refused, ended alone with `create_immed`; unverified for Chromium,
  Firefox, VA-API and NVIDIA.
- Each dmabuf created on llvmpipe costs two maps and unmaps per plane, not measured
  ([debt](../debt.md)).
- Video in YUV formats is not imported as dmabufs on llvmpipe; Smithay's panic in a debug build on
  a buffer of height 0 is not fixed here ([debt](../debt.md)).

## Validation and reconsideration

Evidence: a headless test with a sealed memfd llvmpipe cannot map, which killed Yuna before and is
refused after, without a GPU; the run that crashed, with the package built after the fix, showing
GTK's window. Proposed again when Stage 2's Vulkan renderer replaces llvmpipe's GL path.
