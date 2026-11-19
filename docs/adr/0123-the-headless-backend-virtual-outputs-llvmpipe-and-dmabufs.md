# 0123. The headless backend: virtual outputs, llvmpipe by default, the dmabufs it can read

- Status: Accepted; supersedes [0022](0022-the-headless-backend-in-yuna.md),
  [0029](0029-the-rest-of-animated-applications.md) (what CI tests of dmabufs) and
  [0098](0098-on-llvmpipe-dmabufs-offered-for-no-device-and-checked-as-llvmpipe-maps-them.md)
- Date: 2026-11-18

## Context

[ADR-0022](0022-the-headless-backend-in-yuna.md) made the headless backend Yuna's own code,
drawn on llvmpipe by default, and left dmabufs untested in CI.
[ADR-0029](0029-the-rest-of-animated-applications.md) found what CI does test of dmabufs, and
[ADR-0098](0098-on-llvmpipe-dmabufs-offered-for-no-device-and-checked-as-llvmpipe-maps-them.md),
after Yuna died under a GTK 4 program drawing on the machine's GPU, changed what llvmpipe offers
and checks. ADR-0022 also justified one renderer by a Stage 3 that
[ADR-0060](0060-development-first-agents-in-their-own-yuna.md) removed, and kept main on upstream
Smithay, which [ADR-0042](0042-main-on-smithays-fork.md) changed. A reader needed three ADRs and
two corrections.

This ADR gathers the rule as it stands on 2026-11-18. It decides nothing new. The owner's GPU runs
games, and tests should give the same result on every machine and in CI, whose runners have no
GPU.

ADR-0022 drew virtual outputs with the session's one renderer "so that Stage 3 can put virtual
outputs beside the DRM ones", and kept main on upstream Smithay; Stage 3 is removed (ADR-0060)
and main is on Yuna's fork (ADR-0042). The rule of one renderer stands, on the parity of both modes.

## Decision

- **The headless backend is Yuna's own code**, built on Smithay's public pieces (its `Output`, EGL,
  the GLES renderer's offscreen buffers, its damage tracker) and calloop's timers, not a patch to
  Smithay (ADR-0022, 2026-11-12).
- **A virtual output** is an output, an offscreen buffer and a damage tracker (ADR-0022). Outputs
  are created and removed while Yuna runs; their size, rate and scale are checked against what the
  renderer can allocate. The default is 3840 × 2160 at 240 Hz and scale 1.5. The buffer's format
  belongs to each output, so that one can be HDR. An output's mode and scale change whole through
  MCP ([ADR-0090](0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md)).
- **One renderer for every output** (ADR-0022): both modes run the same code, only the backend
  differs ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
  item 29). It is Smithay's GLES renderer on an EGL device until Stage 2's Vulkan renderer
  ([ADR-0106](0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md)).
- **llvmpipe by default** (ADR-0022): Mesa's software device, from the development shell's Mesa or
  the package's own ([ADR-0126](0126-yuna-and-the-owners-other-projects.md)), never the host's
  drivers. The GPU's render node is used only when asked, `--gpu`. Yuna never opens a display
  device in the headless mode.
- **On llvmpipe, `linux-dmabuf` names no device** (ADR-0098, 2026-11-18): version 6, its default
  feedback's device `dev_t` 0. Mesa's Vulkan, seeing a device other than its own, then copies each
  frame into a linear buffer llvmpipe can map. `--gpu` names its render node.
- **On llvmpipe, every plane of a dmabuf is mapped as llvmpipe maps it**, then unmapped at once,
  before the import and again after it, and its rows must lie in its file, a stride holding a row
  of its pixels (ADR-0098). A buffer that fails is refused: `failed`, or `invalid_wl_buffer` for
  `create_immed`, which ends that client alone. llvmpipe offers only the formats of one plane whose
  bytes per pixel Yuna knows: 23 of its 57, its YUV formats left out.
- **What CI tests of dmabufs** (ADR-0029, 2026-11-12): the compositor's in-process tests import a
  client's dmabuf on llvmpipe, from a sealed memfd, and draw it. No real application in CI uses
  `linux-dmabuf`, since Mesa on the CPU falls back on shared memory; a GPU's dmabufs are tested
  nowhere ([debt](../debt.md)).

## Alternatives considered

- **The backend in Yuna's fork of Smithay** ([ADR-0022](0022-the-headless-backend-in-yuna.md)): the
  same code, in a patch to rebase at each raise, though nothing in it needs a change to Smithay.
- **The GPU by default where there is one** ([ADR-0022](0022-the-headless-backend-in-yuna.md)):
  agents' tests would take frames from the owner's games, and differ from CI's.
- **Smithay's pixman renderer for the CPU** ([ADR-0022](0022-the-headless-backend-in-yuna.md)): a
  second path the desktop never uses, which cannot run the colour shaders.
- **The check of dmabufs alone**
  ([ADR-0098](0098-on-llvmpipe-dmabufs-offered-for-no-device-and-checked-as-llvmpipe-maps-them.md)):
  Yuna lives, but Mesa's Vulkan programs are ended and never show.
- **No `linux-dmabuf` on llvmpipe**, as Weston's software renderer
  ([ADR-0098](0098-on-llvmpipe-dmabufs-offered-for-no-device-and-checked-as-llvmpipe-maps-them.md)):
  Mesa's Vulkan on a GPU fails to make its surface, and the dmabuf path CI tests goes.

## Consequences

- llvmpipe draws a 4K frame on the processor: its cost is measured, and the GPU's path apart. On
  the lowest tier's two cores, agents' runs will likely need `--gpu`.
- `dev_t` 0 is Yuna's reading of the protocol, which tells clients not to compare device numbers;
  it steers only clients that do, as Mesa does. A GPU client that is not Mesa's may still send a
  buffer the processor cannot map, and is then refused; unverified for Chromium, Firefox, VA-API
  and NVIDIA.
- Each dmabuf created on llvmpipe costs two maps and unmaps per plane, not measured; video in YUV
  formats is not imported as dmabufs on llvmpipe ([debt](../debt.md)).

## Validation and reconsideration

Evidence: a headless test with a sealed memfd llvmpipe cannot map, which killed Yuna before and is
refused after, without a GPU; the run that crashed, with the package built after the fix, showing
GTK's window. Proposed again when Stage 2's Vulkan renderer replaces llvmpipe's GL path.
