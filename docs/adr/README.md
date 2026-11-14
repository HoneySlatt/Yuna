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
| [0003](0003-a-parallel-project-uwui-first.md) | A parallel project, uwui first on any conflict | Accepted; amended by 0060 |
| [0004](0004-the-compositor-alone-under-a-shell.md) | The compositor alone, a shell on top | Accepted; amended by 0057, 0063 |
| [0005](0005-the-layouts.md) | The layouts: scrolling columns, dwindle, floating, special workspaces | Accepted; amended by 0062 |
| [0006](0006-machines-and-gpus.md) | Machines and GPUs | Accepted |
| [0007](0007-surviving-a-crash.md) | Surviving a crash | Accepted; amended by 0058, 0061 |
| [0008](0008-configuration-in-kdl.md) | Configuration in KDL | Accepted |
| [0009](0009-gpl-private-until-ready.md) | GPL-3.0-or-later, private until ready; the logo apart | Accepted; amended by 0036 |
| [0010](0010-animations-and-effects.md) | Animations and effects | Accepted; amended by 0058, 0061, 0062 |
| [0011](0011-built-on-smithay.md) | Built on Smithay, its patches kept in Yuna | Accepted; amended by 0022, 0039 |
| [0012](0012-opengl-first.md) | OpenGL first, the pipeline independent of the renderer | Accepted; amended by 0041, 0063 |
| [0013](0013-our-own-code-niri-and-jay-as-references.md) | Our own code; niri and jay as references | Accepted; amended by 0039 |
| [0014](0014-vrr-without-flicker.md) | VRR without flicker | Accepted |
| [0015](0015-agents-workspaces.md) | Agents' workspaces | Accepted; amended by 0060 |
| [0016](0016-the-rust-toolchain.md) | The Rust toolchain: an exact stable version, in one file | Accepted; amended by 0058 |
| [0017](0017-smithays-revision.md) | Smithay's revision: upstream, pinned, first at master of 2026-11-11 | Accepted; amended by 0021 |
| [0018](0018-workspaces.md) | Workspaces: named, with dynamic ones as an option | Accepted |
| [0019](0019-the-layouts-behaviours.md) | The layouts' behaviours: niri and Hyprland as references | Accepted |
| [0020](0020-the-layout-models-shape.md) | The layout model's shape | Accepted |
| [0021](0021-smithays-fork.md) | Smithay's fork: a private repository, generic patches | Accepted; amends 0017; amended by 0036, 0042, 0045 |
| [0022](0022-the-headless-backend-in-yuna.md) | The headless backend: Yuna's own code, drawn on llvmpipe by default | Accepted; amends 0011; amended by 0029 |
| [0023](0023-frames-presentation-and-the-clock.md) | Frames, presentation times and the clock | Accepted; amended by 0031, 0046, 0058 |
| [0024](0024-no-title-bars.md) | No title bars: decorations on the server's side | Accepted |
| [0025](0025-applications-in-ci.md) | Real applications in CI, and rest checked on the program | Accepted; amended by 0029, 0050 |
| [0026](0026-real-clients-in-the-layout.md) | Real clients in the layout model | Accepted |
| [0027](0027-smithay-without-overflow-checks.md) | Smithay built without overflow checks in debug builds | Superseded by 0042 |
| [0028](0028-bounds-on-what-one-client-may-ask.md) | Bounds on what one client may ask | Accepted; context corrected |
| [0029](0029-the-rest-of-animated-applications.md) | The rest of animated applications, and the buffers CI's applications use | Accepted; amends 0025, 0022; amended by 0043 |
| [0030](0030-a-frames-gpu-time-from-the-kernel.md) | A frame's GPU time read from the kernel, the renderer's timers for effects | Accepted; amends the ROADMAP |
| [0031](0031-frames-drawn-ahead-of-their-boundary.md) | Frames drawn ahead of their boundary | Accepted; amends 0023; amended by 0046 |
| [0032](0032-input-to-present-read-by-mode.md) | Input to present, read by mode | Accepted; amends the ROADMAP |
| [0033](0033-the-mcp-server.md) | The MCP server: a private Yuna on stdio, its own MCP layer | Accepted; amended by 0049 |
| [0034](0034-captures.md) | Captures: PNG at the logical size | Accepted |
| [0035](0035-agents-seats.md) | Agents' seats in one Yuna | Accepted; amended by 0042 |
| [0036](0036-yuna-is-public.md) | Yuna is public | Accepted; amends 0009, 0021 |
| [0037](0037-seats-tools-input-and-closing.md) | An agent's seat through MCP: its tools, its input, its keyboard, its closing | Accepted; amended by 0043 |
| [0038](0038-captures-coordinates-windows-and-wayland.md) | Captures: from a pixel to a click, a window's capture, and who captures through Wayland | Accepted |
| [0039](0039-leaving-smithay-step-by-step.md) | Leaving Smithay step by step | Accepted; amends 0011, 0013 |
| [0040](0040-the-wire-protocol-stays-wayland-rs.md) | The wire protocol stays wayland-rs's | Accepted |
| [0041](0041-vulkan-decided-on-a-measure.md) | Vulkan decided on a measure | Superseded by 0063 |
| [0042](0042-main-on-smithays-fork.md) | Main on Smithay's fork: seats and outputs filtered per client | Accepted; amends 0021, 0035; supersedes 0027; amended by 0045 |
| [0043](0043-text-input-and-the-rest-of-slimeengines-examples.md) | Text input on an agent's seat, and the rest of SlimeEngine's examples | Accepted; amends 0037, 0029 |
| [0044](0044-xwayland-across-outputs-of-different-scales.md) | XWayland across outputs of different scales | Accepted |
| [0045](0045-the-fork-over-https-a-branch-per-base.md) | The fork over HTTPS, a branch per base | Accepted; amends 0021 |
| [0046](0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md) | Emulated VRR: amdgpu's rule, checked against the owner's monitor | Accepted; amends 0023, 0031; amended by 0051, 0058 |
| [0047](0047-color-management-v1-kwins-list.md) | `color-management-v1`: Yuna's own handler, KWin's list | Accepted |
| [0048](0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md) | HDR virtual outputs: PQ announced, drawn at Stage 5 | Accepted |
| [0049](0049-vrr-and-hdr-outputs-through-mcp.md) | VRR and HDR outputs through MCP, and surfaces' colours read | Accepted; amends 0033 |
| [0050](0050-mpv-in-the-applications-tests.md) | mpv in the applications' tests | Accepted; amends 0025 |
| [0051](0051-emulated-vrr-as-the-monitors-traces-showed.md) | Emulated VRR, as the monitor's traces showed | Accepted; amends 0046 |
| [0057](0057-what-yuna-does-and-what-the-shell-does.md) | What Yuna does itself, and what it leaves to the shell | Accepted; amends 0004; amended by 0063 |
| [0058](0058-what-yuna-takes-from-the-owners-other-projects.md) | What Yuna takes from the owner's other projects | Accepted; amends 0007, 0010, 0016, 0023, 0046; amended by 0061 |
| [0060](0060-development-first-agents-in-their-own-yuna.md) | Development first: agents never hold back for a game, each in its own Yuna | Accepted; amends 0003, 0015 |
| [0061](0061-yunas-uses.md) | Yuna's uses: creation, capture, games, HDR per display, development | Accepted; amends 0007, 0010, 0058 |
| [0062](0062-the-compositors-comfort-and-look.md) | The compositor's comfort and look | Accepted; amends 0005, 0010; point 8 reopened |
| [0063](0063-vulkan-alone-and-uwui-for-what-yuna-draws.md) | Vulkan alone, and uwui for what Yuna draws and for Hollow | Accepted; amends 0004, 0012, 0057; supersedes 0041 |
