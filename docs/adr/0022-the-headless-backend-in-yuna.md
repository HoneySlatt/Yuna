# 0022. The headless backend: Yuna's own code, drawn on llvmpipe by default

- Status: Accepted; amends [0011](0011-built-on-smithay.md) (its list of first patches)
- Date: 2026-11-12

## Context

M1.4 needs virtual outputs: outputs of any size, rate and scale that no monitor shows, drawn for real
([ADR-0015](0015-agents-workspaces.md)). [ADR-0011](0011-built-on-smithay.md) counted "a headless
output backend" among the first patches of Yuna's fork of Smithay. Read in Smithay `8fd9fe0` on
2026-11-12, every piece one needs is already public, and none ties an output to a backend:

- `smithay::output::Output` (`src/output.rs`) holds an output's modes and scale, with no backend;
- `EGLDevice::enumerate` lists the GPUs' render nodes and Mesa's software device, llvmpipe
  (`EGLDevice::is_software`), and an `EGLDisplay` opens on either (`src/backend/egl/native.rs`);
- `GlesRenderer` draws into an offscreen renderbuffer (`impl Offscreen<GlesRenderbuffer>`), and
  `OutputDamageTracker` redraws only what changed;
- calloop's timers give the refresh.

niri's headless backend for its tests is built on Smithay the same way ([survey](../survey.md)).
Main depends on upstream Smithay; moving it to the private fork would need a read key for CI
([ADR-0021](0021-smithays-fork.md)).

The owner's GPU runs games, and tests should give the same result on every machine and in CI, whose
runners have no GPU.

## Decision

- **The headless backend is Yuna's code**, on upstream Smithay. Main stays on upstream `8fd9fe0`.
- **A virtual output** is a Smithay `Output`, an offscreen buffer and a damage tracker. It draws with
  the session's one renderer, so that Stage 3 can put virtual outputs beside the DRM ones. Outputs
  are created and removed while Yuna runs. Their size, rate and scale are checked when they are
  created, against what the renderer can allocate. The default is 3840 × 2160 at 240 Hz and scale
  1.5. The buffer's format belongs to each output, so that M1.6 can make one HDR.
- **One renderer: Smithay's GLES** ([ADR-0012](0012-opengl-first.md)), on an EGL device. By
  default, the device is Mesa's llvmpipe, from the development shell's Mesa, never the host's
  drivers. The GPU's render node is used only when asked (`--gpu`). Yuna never opens a display
  device in the headless mode.

## Alternatives considered

- **The backend in the fork**, as ADR-0011 first said: the same code, in a patch to rebase at each
  raise of Smithay's revision, and main moved to the fork, with a read key for CI. Nothing in it
  needs a change to Smithay.
- **The GPU by default where there is one**: faster frames, but agents' tests would take frames
  from the owner's games, and give results that differ from CI's.
- **Smithay's pixman renderer for the CPU**: faster than llvmpipe for plain copies, but a second
  path that the desktop never uses, and it cannot run the colour and HDR shaders that later stages
  test headless.

## Consequences

- No patch to Smithay for M1.4; the fork stays as small as ADR-0021 wants.
- llvmpipe draws a 4K frame on the CPU: its cost is measured, and the GPU path is measured apart.
- Clients' dmabufs are imported only where the renderer can import them. With no GPU, Mesa's
  clients fall back on shared memory, so CI does not test the dmabuf path: that is debt until a
  machine with a GPU runs the tests.
- On the lowest tier's two cores, agents' runs will likely need `--gpu`.
