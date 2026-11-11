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
| [0011](0011-built-on-smithay.md) | Built on Smithay, its patches kept in Yuna | Accepted |
| [0012](0012-opengl-first.md) | OpenGL first, the pipeline independent of the renderer | Accepted |
| [0013](0013-our-own-code-niri-and-jay-as-references.md) | Our own code; niri and jay as references | Accepted |
| [0014](0014-vrr-without-flicker.md) | VRR without flicker | Accepted |
| [0015](0015-agents-workspaces.md) | Agents' workspaces | Accepted |
| [0016](0016-the-rust-toolchain.md) | The Rust toolchain: an exact stable version, in one file | Accepted |
| [0017](0017-smithays-revision.md) | Smithay's revision: upstream, pinned, first at master of 2026-11-11 | Accepted |
