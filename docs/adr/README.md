# Architecture decision records

An ADR records one decision: its context, the decision, the alternatives rejected and the
consequences. Once accepted, only its status changes.

- A decision changed, even in part, is a new ADR that supersedes the old one and states the whole
  rule then in force on its topic: its status is `Accepted; supersedes NNNN`, the old one's
  `Superseded by NNNN`. One ADR is read for a topic, never a chain of amendments.
- An ADR that holds several topics and changes on one of them alone is superseded in part: the new
  one states that topic's rule whole, `Accepted; supersedes in part NNNN (the topic)`, the old one
  `…; superseded in part by NNNN (the topic)`, and stays in force for the rest.
- An ADR that adds a decision without changing one names the older in its context alone.
- A precision decided while a milestone runs goes in the milestone's plan or in a new ADR, never
  into an older ADR's status.
- ADR-0001 to ADR-0070 were written under an earlier rule, which amended ADRs in part
  (`Accepted; amends NNNN`, `amended by NNNN`); Stage 1's audit consolidated their chains from
  ADR-0106 on, each new ADR stating one topic whole; the bounds on what a client may ask are ADR-0131's.

New ADRs copy [`0000-template.md`](0000-template.md) and take the next free number. The index
below is written by `scripts/adr-index.py --write`; `scripts/check.sh` checks it.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-what-yuna-is.md) | What Yuna is: one compositor, two modes | Accepted |
| [0002](0002-the-headless-mode-first.md) | The headless mode first, then the desktop | Accepted |
| [0003](0003-a-parallel-project-uwui-first.md) | A parallel project, uwui first on any conflict | Superseded by 0115; amended by 0060 |
| [0004](0004-the-compositor-alone-under-a-shell.md) | The compositor alone, a shell on top | Superseded by 0107; amended by 0057 and 0063 |
| [0005](0005-the-layouts.md) | The layouts: scrolling columns, dwindle, floating, special workspaces | Superseded by 0110, and 0111; amended by 0062 |
| [0006](0006-machines-and-gpus.md) | Machines and GPUs | Accepted |
| [0007](0007-surviving-a-crash.md) | Surviving a crash | Superseded by 0080; amended by 0058 and 0061 |
| [0008](0008-configuration-in-kdl.md) | Configuration in KDL | Superseded by 0129 |
| [0009](0009-gpl-private-until-ready.md) | GPL-3.0-or-later, private until ready; the logo apart | Superseded by 0114; amended by 0036 |
| [0010](0010-animations-and-effects.md) | Animations and effects | Superseded by 0109; amended by 0058, 0061 and 0062 |
| [0011](0011-built-on-smithay.md) | Built on Smithay, its patches kept in Yuna | Superseded by 0112; amended by 0022 and 0039 |
| [0012](0012-opengl-first.md) | OpenGL first, the pipeline independent of the renderer | Superseded by 0106; amended by 0041 and 0063 |
| [0013](0013-our-own-code-niri-and-jay-as-references.md) | Our own code; niri and jay as references | Superseded by 0113; amended by 0039 |
| [0014](0014-vrr-without-flicker.md) | VRR without flicker | Accepted |
| [0015](0015-agents-workspaces.md) | Agents' workspaces | Superseded by 0116; amended by 0060 |
| [0016](0016-the-rust-toolchain.md) | The Rust toolchain: an exact stable version, in one file | Superseded by 0125; amended by 0058 |
| [0017](0017-smithays-revision.md) | Smithay's revision: upstream, pinned, first at master of 2026-11-11 | Superseded by 0112; amended by 0021 |
| [0018](0018-workspaces.md) | Workspaces: named, with dynamic ones as an option | Accepted |
| [0019](0019-the-layouts-behaviours.md) | The layouts' behaviours: niri and Hyprland as references | Superseded by 0110; amended by 0064 |
| [0020](0020-the-layout-models-shape.md) | The layout model's shape | Superseded by 0111; amended by 0052 |
| [0021](0021-smithays-fork.md) | Smithay's fork: a private repository, generic patches | Superseded by 0112; amends 0017; amended by 0036, 0042 and 0045 |
| [0022](0022-the-headless-backend-in-yuna.md) | The headless backend: Yuna's own code, drawn on llvmpipe by default | Superseded by 0123; amends 0011; amended by 0029 |
| [0023](0023-frames-presentation-and-the-clock.md) | Frames, presentation times and the clock | Superseded by 0118; amended by 0031, 0046, 0054 and 0058 |
| [0024](0024-no-title-bars.md) | No title bars: decorations on the server's side | Accepted |
| [0025](0025-applications-in-ci.md) | Real applications in CI, and rest checked on the program | Superseded by 0122; amended by 0029, 0050 and 0104 |
| [0026](0026-real-clients-in-the-layout.md) | Real clients in the layout model | Superseded by 0110, and 0111 |
| [0027](0027-smithay-without-overflow-checks.md) | Smithay built without overflow checks in debug builds | Superseded by 0042 |
| [0028](0028-bounds-on-what-one-client-may-ask.md) | Bounds on what one client may ask | Superseded by 0131 |
| [0029](0029-the-rest-of-animated-applications.md) | The rest of animated applications, and the buffers CI's applications use | Superseded by 0122, and 0123; amends 0022 and 0025; amended by 0043 |
| [0030](0030-a-frames-gpu-time-from-the-kernel.md) | A frame's GPU time read from the kernel, the renderer's timers for effects | Accepted |
| [0031](0031-frames-drawn-ahead-of-their-boundary.md) | Frames drawn ahead of their boundary | Superseded by 0118; amends 0023; amended by 0046 |
| [0032](0032-input-to-present-read-by-mode.md) | Input to present, read by mode | Superseded by 0076 |
| [0033](0033-the-mcp-server.md) | The MCP server: a private Yuna on stdio, its own MCP layer | Superseded by 0120; amended by 0049, 0054 and 0055 |
| [0034](0034-captures.md) | Captures: PNG at the logical size | Superseded by 0121 |
| [0035](0035-agents-seats.md) | Agents' seats in one Yuna | Superseded by 0078; amended by 0042 |
| [0036](0036-yuna-is-public.md) | Yuna is public | Superseded by 0112, and 0114; amends 0009 and 0021 |
| [0037](0037-seats-tools-input-and-closing.md) | An agent's seat through MCP: its tools, its input, its keyboard, its closing | Superseded by 0095; amended by 0043 |
| [0038](0038-captures-coordinates-windows-and-wayland.md) | Captures: from a pixel to a click, a window's capture, and who captures through Wayland | Superseded by 0121; amended by 0054 |
| [0039](0039-leaving-smithay-step-by-step.md) | Leaving Smithay step by step | Superseded by 0112, and 0113; superseded in part by 0081; amends 0011 and 0013 |
| [0040](0040-the-wire-protocol-stays-wayland-rs.md) | The wire protocol stays wayland-rs's | Accepted |
| [0041](0041-vulkan-decided-on-a-measure.md) | Vulkan decided on a measure | Superseded by 0063; amends 0012 |
| [0042](0042-main-on-smithays-fork.md) | Main on Smithay's fork: seats and outputs filtered per client | Superseded by 0112; supersedes 0027; amends 0021 and 0035; amended by 0045 |
| [0043](0043-text-input-and-the-rest-of-slimeengines-examples.md) | Text input on an agent's seat, and the rest of SlimeEngine's examples | Superseded by 0117, and 0122; superseded in part by 0095; amends 0029 and 0037 |
| [0044](0044-xwayland-across-outputs-of-different-scales.md) | XWayland across outputs of different scales | Superseded by 0124 |
| [0045](0045-the-fork-over-https-a-branch-per-base.md) | The fork over HTTPS, a branch per base | Superseded by 0112; amends 0021 and 0042 |
| [0046](0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md) | Emulated VRR: amdgpu's rule, checked against the owner's monitor | Superseded by 0119; amends 0023 and 0031; amended by 0051 and 0058 |
| [0047](0047-color-management-v1-kwins-list.md) | `color-management-v1`: Yuna's own handler, KWin's list | Accepted |
| [0048](0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md) | HDR virtual outputs: PQ announced, drawn at Stage 5 | Superseded by 0128 |
| [0049](0049-vrr-and-hdr-outputs-through-mcp.md) | VRR and HDR outputs through MCP, and surfaces' colours read | Superseded by 0120; amends 0033 |
| [0050](0050-mpv-in-the-applications-tests.md) | mpv in the applications' tests | Superseded by 0122; amends 0025 |
| [0051](0051-emulated-vrr-as-the-monitors-traces-showed.md) | Emulated VRR, as the monitor's traces showed | Superseded by 0119; amends 0046; amended by 0054 |
| [0052](0052-motion-in-a-pure-crate.md) | Motion in a pure crate, the layout's jumps turned into offsets | Accepted; superseded in part by 0111; amends 0020 |
| [0053](0053-springs-in-closed-form-keeping-their-velocity.md) | Springs in closed form, keeping their velocity; curves | Accepted |
| [0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md) | Motion sampled at its presentation; what is drawn apart from what a client is told | Superseded by 0118, and 0119, 0120 and 0121; amends 0023, 0033, 0038 and 0051 |
| [0055](0055-what-moves-first-and-animations-off-headless.md) | What moves first, and animations off headless | Superseded by 0109; amends 0033; amended by 0059 |
| [0056](0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md) | Motion's budget judged on the GPU, against a still baseline | Superseded by 0127 |
| [0057](0057-what-yuna-does-and-what-the-shell-does.md) | What Yuna does itself, and what it leaves to the shell | Superseded by 0107, and 0108 and 0129; amends 0004; amended by 0063 and 0064 |
| [0058](0058-what-yuna-takes-from-the-owners-other-projects.md) | What Yuna takes from the owner's other projects | Superseded by 0109, and 0116, 0118, 0125 and 0126; superseded in part by 0080; amends 0007, 0010, 0016, 0023 and 0046; amended by 0061 |
| [0059](0059-reduced-motion-nothing-slides-fades-stay.md) | Reduced motion: nothing slides, fades stay | Superseded by 0109; amends 0055 |
| [0060](0060-development-first-agents-in-their-own-yuna.md) | Development first: agents never hold back for a game, each in its own Yuna | Superseded by 0115, and 0116; amends 0003 and 0015 |
| [0061](0061-yunas-uses.md) | Yuna's uses: creation, capture, games, HDR per display, development | Accepted; superseded in part by 0109 and 0110; amends 0007, 0010 and 0058 |
| [0062](0062-the-compositors-comfort-and-look.md) | The compositor's comfort and look | Accepted; superseded in part by 0109, 0110, 0121 and 0130; amends 0005 and 0010 |
| [0063](0063-vulkan-alone-and-uwui-for-what-yuna-draws.md) | Vulkan alone, and uwui for what Yuna draws and for Hollow | Superseded by 0073; supersedes 0041; amends 0004, 0012 and 0057 |
| [0064](0064-what-the-best-desktops-taught-yuna.md) | What the best desktops taught Yuna | Accepted; superseded in part by 0107, 0108, 0110 and 0129; amends 0019 and 0057; amended by 0065 |
| [0065](0065-what-hollow-asks-of-yuna.md) | What Hollow asks of Yuna: a sharing event, a greeter mode, a locker's relief | Superseded by 0107; superseded in part by 0086; amends 0064 |
| [0066](0066-yunas-own-x11-window-manager-on-x11rb.md) | Yuna's own X11 window manager, on x11rb | Accepted |
| [0067](0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md) | One XWayland per seat, on demand, behind a cookie | Accepted |
| [0068](0068-x11-windows-drawn-by-yuna-through-an-area-filter.md) | X11 windows drawn by Yuna through an area filter, at a scale fixed for an XWayland's life | Superseded by 0124 |
| [0069](0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md) | Fullscreen X11 games: gamescope, and Xwayland's emulated modes kept | Superseded by 0124 |
| [0070](0070-bounds-on-what-one-x11-client-may-ask.md) | Bounds on what one X11 client may ask | Superseded by 0131 |
| [0071](0071-the-rivals-measured-headless-by-agents-and-on-a-real-display-by-the-owner.md) | The rivals measured: costs headless by agents, frames, latency and watts on a real display by the owner | Superseded by 0075 |
| [0072](0072-budgets-set-against-the-peers-the-same-day.md) | Budgets set against the peers, the same day | Accepted |
| [0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md) | Vulkan alone, uwui for what Yuna draws, and a prototype of what no rival measures | Superseded by 0106; supersedes 0063; superseded in part by 0080 and 0084 |
| [0074](0074-the-requirements-of-stage-2s-core.md) | The requirements of Stage 2's core | Superseded by 0082 |
| [0075](0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md) | The rivals measured: costs headless by agents now, frames, latency and watts on a real display at Stage 2's exit | Accepted; supersedes 0071 |
| [0076](0076-input-to-present-read-by-mode.md) | Input to present, read by mode | Accepted; supersedes 0032 |
| [0077](0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md) | The budgets of a frame, memory and rest, set from the rivals | Accepted; superseded in part by 0083 and 0127 |
| [0078](0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md) | Agents' seats in one Yuna, frozen until Stage 2's core | Superseded by 0116; supersedes 0035 |
| [0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md) | Yuna's contracts in 0.x: changes announced, never silent | Accepted |
| [0080](0080-what-survives-a-failure-and-yunas-own-panics.md) | What survives a failure, and Yuna's own panics | Accepted; supersedes 0007; supersedes in part 0058 and 0073 |
| [0081](0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md) | Stage 2 in three gated parts, and the path off Smithay | Superseded by 0139; supersedes in part 0039 |
| [0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) | The requirements of Stage 2's core, its bets made rules or deferred | Accepted; supersedes 0074; superseded in part by 0135 and 0137 |
| [0083](0083-a-windows-last-wl-shm-buffer-kept-until-its-next.md) | A window's last `wl_shm` buffer kept until its next | Accepted; supersedes in part 0077 |
| [0084](0084-uwui-hosted-on-its-own-task-commands-and-images.md) | uwui hosted on its own task, commands and images | Superseded by 0106; supersedes in part 0073 |
| [0085](0085-outside-contributions-issues-now-pull-requests-from-0-1.md) | Outside contributions: issues now, pull requests from 0.1 | Accepted |
| [0086](0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md) | The lock begun by Yuna, relieved by the locker, and logind's `Unlock` | Superseded by 0108; supersedes in part 0065 |
| [0087](0087-a-locker-that-stops-answering-counts-as-dead.md) | A locker that stops answering counts as dead | Superseded by 0088 |
| [0088](0088-a-locker-that-stops-answering-counts-as-dead-its-answer-from-the-thread-that-draws-it.md) | A locker that stops answering counts as dead, its answer from the thread that draws it | Superseded by 0108; supersedes 0087 |
| [0089](0089-yuna-for-the-owners-other-projects.md) | Yuna for the owner's other projects: an agent's seat, wlr-screencopy, its own Mesa | Superseded by 0116, and 0121 and 0126 |
| [0090](0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md) | An output's mode and scale changed whole, through MCP | Accepted |
| [0091](0091-the-clipboard-between-x11-and-wayland-who-reads-and-its-bounds.md) | The clipboard between X11 and Wayland: who reads it, and its bounds | Accepted |
| [0092](0092-a-programs-protocol-trace-its-errors-and-its-log-through-mcp.md) | A program's protocol trace, its errors and its log through MCP | Accepted |
| [0093](0093-a-call-that-waits-for-the-seats-events.md) | A call that waits for the seat's events, and programs made stuck or slow | Accepted |
| [0094](0094-the-accessibility-tree-yuna-as-the-registry-of-each-seats-bus.md) | The accessibility tree: Yuna as the registry of each seat's bus | Accepted |
| [0095](0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md) | An agent's seat through MCP: its tools, its input, its keyboard and layouts, its closing | Superseded by 0117; supersedes 0037; supersedes in part 0043 |
| [0096](0096-a-rest-auditor-naming-what-wakes-yuna-or-makes-it-draw.md) | A rest auditor naming what wakes Yuna or makes it draw | Accepted |
| [0097](0097-the-budgets-of-yuna-mcp.md) | The budgets of `yuna mcp` | Superseded by 0100 |
| [0098](0098-on-llvmpipe-dmabufs-offered-for-no-device-and-checked-as-llvmpipe-maps-them.md) | On llvmpipe, dmabufs offered for no device and checked as llvmpipe maps them | Superseded by 0123 |
| [0099](0099-a-workspace-switch-keeps-each-workspace-on-its-row.md) | A workspace switch keeps each workspace on its row | Superseded by 0109 |
| [0100](0100-the-budgets-of-yuna-mcp-at-their-conditions.md) | The budgets of `yuna mcp`, at their conditions | Accepted; supersedes 0097 |
| [0101](0101-an-agents-clipboard-through-mcp.md) | An agent's clipboard through MCP | Accepted; superseded in part by 0132 |
| [0102](0102-drag-and-drop-between-x11-and-wayland-a-drag-session-beside-smithays-grabs.md) | Drag and drop between X11 and Wayland: a drag session beside Smithay's grabs | Accepted |
| [0103](0103-crates-updates-wait-three-days.md) | Crates' updates wait three days | Accepted |
| [0104](0104-stage-1s-exit-opium-in-aeriths-place-and-its-exceptions-written.md) | Stage 1's exit: Opium in Aerith's place, and its exceptions written | Accepted; superseded in part by 0122; amends 0025 |
| [0105](0105-an-agents-programs-on-a-throwaway-home.md) | An agent's programs on a throwaway home | Accepted |
| [0106](0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md) | Vulkan alone, and uwui hosted for what Yuna draws | Accepted; supersedes 0012, 0073 and 0084 |
| [0107](0107-what-yuna-does-itself-and-what-it-leaves-to-the-shell.md) | What Yuna does itself, and what it leaves to the shell | Accepted; supersedes 0004, 0057 and 0065; supersedes in part 0064 |
| [0108](0108-the-lock-begun-by-yuna-a-dead-or-frozen-locker-relieved.md) | The lock: begun by Yuna, a dead or frozen locker relieved, logind's `Unlock` | Accepted; supersedes 0057, 0086 and 0088; supersedes in part 0064 |
| [0109](0109-animations-and-effects-what-moves-reduced-motion-and-games.md) | Animations and effects: what moves, reduced motion, games and headless | Accepted; supersedes 0010, 0055, 0058, 0059 and 0099; supersedes in part 0061 and 0062 |
| [0110](0110-the-layouts-and-their-behaviours.md) | The layouts and their behaviours: niri and Hyprland as references, games first | Accepted; supersedes 0005, 0019 and 0026; supersedes in part 0061, 0062 and 0064 |
| [0111](0111-the-layout-models-shape-plain-ids-a-branch-per-seat-asked-and-committed-sizes.md) | The layout model's shape: plain ids, a branch per seat, sizes asked and committed | Accepted; supersedes 0005, 0020 and 0026; supersedes in part 0052 |
| [0112](0112-smithay-pinned-patched-in-a-public-fork-left-at-stage-2.md) | Smithay: pinned, patched in Yuna's public fork, left at Stage 2 | Accepted; supersedes 0011, 0017, 0021, 0036, 0039, 0042 and 0045; superseded in part by 0139 |
| [0113](0113-our-own-code-niri-jay-and-smithay-as-references.md) | Our own code; niri, jay and Smithay as references | Accepted; supersedes 0013 and 0039 |
| [0114](0114-gpl-3-0-or-later-public-the-logo-apart.md) | GPL-3.0-or-later, public; the logo apart | Accepted; supersedes 0009 and 0036 |
| [0115](0115-priorities-uwui-first-the-owners-activity-measures-that-check-their-load.md) | Priorities: uwui first, the owner's activity, measures that check their load | Accepted; supersedes 0003 and 0060 |
| [0116](0116-agents-seats-each-agent-in-its-own-yuna.md) | Agents' seats: each agent in its own Yuna, apart from the owner's seat | Superseded by 0138; supersedes 0015, 0058, 0060, 0078 and 0089 |
| [0117](0117-an-agents-seat-through-mcp-tools-input-keyboard-text-and-closing.md) | An agent's seat through MCP: its tools, input, keyboard, text and closing | Accepted; supersedes 0043 and 0095 |
| [0118](0118-frames-presentation-times-and-the-clock.md) | Frames, presentation times and the clock | Accepted; supersedes 0023, 0031, 0054 and 0058 |
| [0119](0119-emulated-vrr-as-amdgpu-and-the-owners-monitor-do.md) | Emulated VRR, as amdgpu and the owner's monitor do | Accepted; supersedes 0046, 0051 and 0054 |
| [0120](0120-the-mcp-server-a-private-yuna-on-stdio.md) | The MCP server: a private Yuna on stdio, its own MCP layer | Accepted; supersedes 0033, 0049 and 0054 |
| [0121](0121-captures-what-is-seen-at-the-logical-size.md) | Captures: what is seen, at the logical size, for agents and clients | Accepted; supersedes 0034, 0038, 0054 and 0089; supersedes in part 0062 |
| [0122](0122-real-applications-in-ci-and-rest-checked-on-the-program.md) | Real applications in CI, and rest checked on the program | Accepted; supersedes 0025, 0029, 0043 and 0050; supersedes in part 0104 |
| [0123](0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md) | The headless backend: virtual outputs, llvmpipe by default, the dmabufs it can read | Accepted; supersedes 0022, 0029 and 0098 |
| [0124](0124-x11-windows-across-outputs-of-different-scales.md) | X11 windows across outputs of different scales | Accepted; supersedes 0044, 0068 and 0069 |
| [0125](0125-the-rust-toolchain-an-exact-stable-version-a-nightly-for-fuzzing.md) | The Rust toolchain: an exact stable version, a pinned nightly for fuzzing alone | Accepted; supersedes 0016 and 0058 |
| [0126](0126-yuna-and-the-owners-other-projects.md) | Yuna and the owner's other projects: their test compositor, its package, shared practices | Accepted; supersedes 0058 and 0089 |
| [0127](0127-motions-budget-no-frame-lost-to-motion.md) | Motion's budget: no frame lost to motion, judged against a still baseline | Accepted; supersedes 0056; supersedes in part 0077 |
| [0128](0128-hdr-virtual-outputs-and-when-colour-is-chosen.md) | HDR virtual outputs, and when Yuna's colour pipeline is chosen | Accepted; supersedes 0048; superseded in part by 0134 |
| [0129](0129-configuration-in-kdl-its-includes-and-variants.md) | Configuration in KDL, its includes and variants | Accepted; supersedes 0008 and 0057; supersedes in part 0064 |
| [0130](0130-oled-care-chosen-at-stage-5s-design.md) | OLED care chosen at Stage 5's design, from a survey | Accepted; supersedes in part 0062 |
| [0131](0131-bounds-on-what-a-client-or-an-agent-may-ask.md) | Bounds on what a client or an agent may ask | Accepted; supersedes 0028 and 0070 |
| [0132](0132-an-agents-clipboard-data-with-control-characters-in-base64.md) | An agent's clipboard: data with control characters in base64 | Accepted; supersedes in part 0101 |
| [0133](0133-yunas-scene-retained-its-damage-found-at-each-latch.md) | Yuna's scene: retained, changed by batches, its damage found at each latch | Accepted |
| [0134](0134-blending-gamma-2-2-everywhere-its-precision-set-by-the-content.md) | Blending: gamma 2.2 everywhere, its precision set by the content | Accepted; supersedes in part 0128 |
| [0135](0135-priority-against-a-game-amds-master-override-turned-on-at-2b-on-a-measure.md) | Priority against a game: the mechanism now, turned on at Stage 2b on a measure | Accepted; supersedes in part 0082 |
| [0136](0136-a-client-that-stops-reading-1-mib-and-its-motion-merged.md) | A client that stops reading: 1 MiB of buffer, its pointer motion merged, never ended by motion | Accepted |
| [0137](0137-planes-yunas-own-allocator-scanout-and-the-cursor-plane-at-2b.md) | Planes: Yuna's own allocator; direct scanout and the cursor plane at Stage 2b | Accepted; supersedes in part 0082 |
| [0138](0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md) | Agents' seats: each agent in its own Yuna, one seat per Yuna from Stage 2's core | Accepted; supersedes 0116 |
| [0139](0139-stage-2-in-three-gated-parts-2as-path-revised.md) | Stage 2 in three gated parts; 2a's path off Smithay revised | Accepted; supersedes 0081; supersedes in part 0112 |
| [0140](0140-rust-1-100s-features-what-yuna-takes.md) | Rust 1.100's features: what Yuna takes | Accepted; superseded in part by 0143 |
| [0141](0141-a-dwindle-window-that-does-not-fit-floats.md) | A dwindle window that does not fit floats | Accepted |
| [0142](0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md) | Parity: a pinned baseline, figures in classes, three verdicts | Accepted |
| [0143](0143-allocations-a-frame-counted-by-dhat.md) | Allocations a frame, counted by valgrind's DHAT | Accepted; supersedes in part 0140 |
