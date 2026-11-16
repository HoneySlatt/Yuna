# Architecture decision records

An ADR records one decision: its context, the decision, the alternatives rejected and the
consequences. Once accepted, only its status changes.

- A decision changed, even in part, is a new ADR that supersedes the old one and states the whole
  rule then in force on its topic: its status is `Accepted; supersedes NNNN`, the old one's
  `Superseded by NNNN`. One ADR is read for a topic, never a chain of amendments.
- An ADR that adds a decision without changing one names the older in its context alone.
- A precision decided while a milestone runs goes in the milestone's plan or in a new ADR, never
  into an older ADR's status.
- ADR-0001 to ADR-0070 were written under an earlier rule, which amended ADRs in part
  (`Accepted; amends NNNN`, `amended by NNNN`); their chains are consolidated at Stage 1's audit.

New ADRs copy [`0000-template.md`](0000-template.md) and take the next free number. The index
below is written by `scripts/adr-index.py --write`; `scripts/check.sh` checks it.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-what-yuna-is.md) | What Yuna is: one compositor, two modes | Accepted |
| [0002](0002-the-headless-mode-first.md) | The headless mode first, then the desktop | Accepted |
| [0003](0003-a-parallel-project-uwui-first.md) | A parallel project, uwui first on any conflict | Accepted; amended by 0060 |
| [0004](0004-the-compositor-alone-under-a-shell.md) | The compositor alone, a shell on top | Accepted; amended by 0057 and 0063 |
| [0005](0005-the-layouts.md) | The layouts: scrolling columns, dwindle, floating, special workspaces | Accepted; amended by 0062 |
| [0006](0006-machines-and-gpus.md) | Machines and GPUs | Accepted |
| [0007](0007-surviving-a-crash.md) | Surviving a crash | Accepted; amended by 0058 and 0061 |
| [0008](0008-configuration-in-kdl.md) | Configuration in KDL | Accepted |
| [0009](0009-gpl-private-until-ready.md) | GPL-3.0-or-later, private until ready; the logo apart | Accepted; amended by 0036 |
| [0010](0010-animations-and-effects.md) | Animations and effects | Accepted; amended by 0058, 0061 and 0062 |
| [0011](0011-built-on-smithay.md) | Built on Smithay, its patches kept in Yuna | Accepted; amended by 0022 and 0039 |
| [0012](0012-opengl-first.md) | OpenGL first, the pipeline independent of the renderer | Accepted; amended by 0041 and 0063 |
| [0013](0013-our-own-code-niri-and-jay-as-references.md) | Our own code; niri and jay as references | Accepted; amended by 0039 |
| [0014](0014-vrr-without-flicker.md) | VRR without flicker | Accepted |
| [0015](0015-agents-workspaces.md) | Agents' workspaces | Accepted; amended by 0060 |
| [0016](0016-the-rust-toolchain.md) | The Rust toolchain: an exact stable version, in one file | Accepted; amended by 0058 |
| [0017](0017-smithays-revision.md) | Smithay's revision: upstream, pinned, first at master of 2026-11-11 | Accepted; amended by 0021 |
| [0018](0018-workspaces.md) | Workspaces: named, with dynamic ones as an option | Accepted |
| [0019](0019-the-layouts-behaviours.md) | The layouts' behaviours: niri and Hyprland as references | Accepted; amended by 0064 |
| [0020](0020-the-layout-models-shape.md) | The layout model's shape | Accepted; amended by 0052 |
| [0021](0021-smithays-fork.md) | Smithay's fork: a private repository, generic patches | Accepted; amends 0017; amended by 0036, 0042 and 0045 |
| [0022](0022-the-headless-backend-in-yuna.md) | The headless backend: Yuna's own code, drawn on llvmpipe by default | Accepted; amends 0011; amended by 0029 |
| [0023](0023-frames-presentation-and-the-clock.md) | Frames, presentation times and the clock | Accepted; amended by 0031, 0046, 0054 and 0058 |
| [0024](0024-no-title-bars.md) | No title bars: decorations on the server's side | Accepted |
| [0025](0025-applications-in-ci.md) | Real applications in CI, and rest checked on the program | Accepted; amended by 0029 and 0050 |
| [0026](0026-real-clients-in-the-layout.md) | Real clients in the layout model | Accepted |
| [0027](0027-smithay-without-overflow-checks.md) | Smithay built without overflow checks in debug builds | Superseded by 0042 |
| [0028](0028-bounds-on-what-one-client-may-ask.md) | Bounds on what one client may ask | Accepted |
| [0029](0029-the-rest-of-animated-applications.md) | The rest of animated applications, and the buffers CI's applications use | Accepted; amends 0022 and 0025; amended by 0043 |
| [0030](0030-a-frames-gpu-time-from-the-kernel.md) | A frame's GPU time read from the kernel, the renderer's timers for effects | Accepted |
| [0031](0031-frames-drawn-ahead-of-their-boundary.md) | Frames drawn ahead of their boundary | Accepted; amends 0023; amended by 0046 |
| [0032](0032-input-to-present-read-by-mode.md) | Input to present, read by mode | Accepted |
| [0033](0033-the-mcp-server.md) | The MCP server: a private Yuna on stdio, its own MCP layer | Accepted; amended by 0049, 0054 and 0055 |
| [0034](0034-captures.md) | Captures: PNG at the logical size | Accepted |
| [0035](0035-agents-seats.md) | Agents' seats in one Yuna | Accepted; amended by 0042 |
| [0036](0036-yuna-is-public.md) | Yuna is public | Accepted; amends 0009 and 0021 |
| [0037](0037-seats-tools-input-and-closing.md) | An agent's seat through MCP: its tools, its input, its keyboard, its closing | Accepted; amended by 0043 |
| [0038](0038-captures-coordinates-windows-and-wayland.md) | Captures: from a pixel to a click, a window's capture, and who captures through Wayland | Accepted; amended by 0054 |
| [0039](0039-leaving-smithay-step-by-step.md) | Leaving Smithay step by step | Accepted; amends 0011 and 0013 |
| [0040](0040-the-wire-protocol-stays-wayland-rs.md) | The wire protocol stays wayland-rs's | Accepted |
| [0041](0041-vulkan-decided-on-a-measure.md) | Vulkan decided on a measure | Superseded by 0063; amends 0012 |
| [0042](0042-main-on-smithays-fork.md) | Main on Smithay's fork: seats and outputs filtered per client | Accepted; supersedes 0027; amends 0021 and 0035; amended by 0045 |
| [0043](0043-text-input-and-the-rest-of-slimeengines-examples.md) | Text input on an agent's seat, and the rest of SlimeEngine's examples | Accepted; amends 0029 and 0037 |
| [0044](0044-xwayland-across-outputs-of-different-scales.md) | XWayland across outputs of different scales | Accepted |
| [0045](0045-the-fork-over-https-a-branch-per-base.md) | The fork over HTTPS, a branch per base | Accepted; amends 0021 and 0042 |
| [0046](0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md) | Emulated VRR: amdgpu's rule, checked against the owner's monitor | Accepted; amends 0023 and 0031; amended by 0051 and 0058 |
| [0047](0047-color-management-v1-kwins-list.md) | `color-management-v1`: Yuna's own handler, KWin's list | Accepted |
| [0048](0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md) | HDR virtual outputs: PQ announced, drawn at Stage 5 | Accepted |
| [0049](0049-vrr-and-hdr-outputs-through-mcp.md) | VRR and HDR outputs through MCP, and surfaces' colours read | Accepted; amends 0033 |
| [0050](0050-mpv-in-the-applications-tests.md) | mpv in the applications' tests | Accepted; amends 0025 |
| [0051](0051-emulated-vrr-as-the-monitors-traces-showed.md) | Emulated VRR, as the monitor's traces showed | Accepted; amends 0046; amended by 0054 |
| [0052](0052-motion-in-a-pure-crate.md) | Motion in a pure crate, the layout's jumps turned into offsets | Accepted; amends 0020 |
| [0053](0053-springs-in-closed-form-keeping-their-velocity.md) | Springs in closed form, keeping their velocity; curves | Accepted |
| [0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md) | Motion sampled at its presentation; what is drawn apart from what a client is told | Accepted; amends 0023, 0033, 0038 and 0051 |
| [0055](0055-what-moves-first-and-animations-off-headless.md) | What moves first, and animations off headless | Accepted; amends 0033; amended by 0059 |
| [0056](0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md) | Motion's budget judged on the GPU, against a still baseline | Accepted |
| [0057](0057-what-yuna-does-and-what-the-shell-does.md) | What Yuna does itself, and what it leaves to the shell | Accepted; amends 0004; amended by 0063 and 0064 |
| [0058](0058-what-yuna-takes-from-the-owners-other-projects.md) | What Yuna takes from the owner's other projects | Accepted; amends 0007, 0010, 0016, 0023 and 0046; amended by 0061 |
| [0059](0059-reduced-motion-nothing-slides-fades-stay.md) | Reduced motion: nothing slides, fades stay | Accepted; amends 0055 |
| [0060](0060-development-first-agents-in-their-own-yuna.md) | Development first: agents never hold back for a game, each in its own Yuna | Accepted; amends 0003 and 0015 |
| [0061](0061-yunas-uses.md) | Yuna's uses: creation, capture, games, HDR per display, development | Accepted; amends 0007, 0010 and 0058 |
| [0062](0062-the-compositors-comfort-and-look.md) | The compositor's comfort and look | Accepted; amends 0005 and 0010 |
| [0063](0063-vulkan-alone-and-uwui-for-what-yuna-draws.md) | Vulkan alone, and uwui for what Yuna draws and for Hollow | Superseded by 0073; supersedes 0041; amends 0004, 0012 and 0057 |
| [0064](0064-what-the-best-desktops-taught-yuna.md) | What the best desktops taught Yuna | Accepted; amends 0019 and 0057; amended by 0065 |
| [0065](0065-what-hollow-asks-of-yuna.md) | What Hollow asks of Yuna: a sharing event, a greeter mode, a locker's relief | Accepted; amends 0064 |
| [0066](0066-yunas-own-x11-window-manager-on-x11rb.md) | Yuna's own X11 window manager, on x11rb | Accepted |
| [0067](0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md) | One XWayland per seat, on demand, behind a cookie | Accepted |
| [0068](0068-x11-windows-drawn-by-yuna-through-an-area-filter.md) | X11 windows drawn by Yuna through an area filter, at a scale fixed for an XWayland's life | Accepted |
| [0069](0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md) | Fullscreen X11 games: gamescope, and Xwayland's emulated modes kept | Accepted |
| [0070](0070-bounds-on-what-one-x11-client-may-ask.md) | Bounds on what one X11 client may ask | Accepted |
| [0071](0071-the-rivals-measured-headless-by-agents-and-on-a-real-display-by-the-owner.md) | The rivals measured: costs headless by agents, frames, latency and watts on a real display by the owner | Accepted |
| [0072](0072-budgets-set-against-the-peers-the-same-day.md) | Budgets set against the peers, the same day | Accepted |
| [0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md) | Vulkan alone, uwui for what Yuna draws, and a prototype of what no rival measures | Accepted; supersedes 0063 |
| [0074](0074-the-requirements-of-stage-2s-core.md) | The requirements of Stage 2's core | Accepted |
