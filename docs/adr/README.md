# Architecture decision records

An ADR records one decision: its context, the decision, the alternatives rejected and the
consequences. Once accepted, only its status changes. A later decision that replaces it whole makes
it `Superseded by NNNN`; one that changes part of it makes it `Accepted; amended by NNNN`, and the
newer one `Accepted; amends NNNN (what)`.

New ADRs copy [`0000-template.md`](0000-template.md) and take the next free number.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-what-yuna-is.md) | What Yuna is: one compositor, two modes | Accepted |
| [0002](0002-the-headless-mode-first.md) | The headless mode first, then the desktop | Accepted |
| [0003](0003-a-parallel-project-uwui-first.md) | A parallel project, uwui first on any conflict | Accepted |
| [0004](0004-the-compositor-alone-under-a-shell.md) | The compositor alone, a shell on top | Accepted |
| [0005](0005-the-layouts.md) | The layouts: scrolling columns, dwindle, floating, special workspaces | Accepted |
| [0006](0006-machines-and-gpus.md) | Machines and GPUs | Accepted |
| [0007](0007-surviving-a-crash.md) | Surviving a crash | Accepted |
| [0008](0008-configuration-in-kdl.md) | Configuration in KDL | Accepted |
| [0009](0009-gpl-private-until-ready.md) | GPL-3.0-or-later, private until ready; the logo apart | Accepted |
| [0010](0010-animations-and-effects.md) | Animations and effects | Accepted |
| [0011](0011-built-on-smithay.md) | Built on Smithay, its patches kept in Yuna | Accepted; amended by 0022 |
| [0012](0012-opengl-first.md) | OpenGL first, the pipeline independent of the renderer | Accepted |
| [0013](0013-our-own-code-niri-and-jay-as-references.md) | Our own code; niri and jay as references | Accepted |
| [0014](0014-vrr-without-flicker.md) | VRR without flicker | Accepted |
| [0015](0015-agents-workspaces.md) | Agents' workspaces | Accepted |
| [0016](0016-the-rust-toolchain.md) | The Rust toolchain: an exact stable version, in one file | Accepted |
| [0017](0017-smithays-revision.md) | Smithay's revision: upstream, pinned, first at master of 2026-11-11 | Accepted; amended by 0021 |
| [0018](0018-workspaces.md) | Workspaces: named, with dynamic ones as an option | Accepted |
| [0019](0019-the-layouts-behaviours.md) | The layouts' behaviours: niri and Hyprland as references | Accepted |
| [0020](0020-the-layout-models-shape.md) | The layout model's shape | Accepted |
| [0021](0021-smithays-fork.md) | Smithay's fork: a private repository, generic patches | Accepted; amends 0017 |
| [0022](0022-the-headless-backend-in-yuna.md) | The headless backend: Yuna's own code, drawn on llvmpipe by default | Accepted; amends 0011; amended by 0029 |
| [0023](0023-frames-presentation-and-the-clock.md) | Frames, presentation times and the clock | Accepted |
| [0024](0024-no-title-bars.md) | No title bars: decorations on the server's side | Accepted |
| [0025](0025-applications-in-ci.md) | Real applications in CI, and rest checked on the program | Accepted; amended by 0029 |
| [0026](0026-real-clients-in-the-layout.md) | Real clients in the layout model | Accepted |
| [0027](0027-smithay-without-overflow-checks.md) | Smithay built without overflow checks in debug builds | Accepted |
| [0028](0028-bounds-on-what-one-client-may-ask.md) | Bounds on what one client may ask | Accepted |
| [0029](0029-the-rest-of-animated-applications.md) | The rest of animated applications, and the buffers CI's applications use | Accepted; amends 0025, 0022 |
| [0030](0030-a-frames-gpu-time-from-the-kernel.md) | A frame's GPU time read from the kernel, the renderer's timers for effects | Accepted |
