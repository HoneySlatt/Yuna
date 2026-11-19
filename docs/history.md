# History

The finished stages of the roadmap, newest first: what each delivered, its release and its
decisions. [`PLAN.md`](PLAN.md) holds the stage in progress.

- What each release changed for users: [`CHANGELOG.md`](../CHANGELOG.md).
- A stage's milestones in full, how each was checked and what it measured: the plan as it stood
  at the release, `git show v<version>:docs/PLAN.md`, and the commits that made them.
- The measurements behind a decision: its ADR, and [`measurements.md`](measurements.md).

## Stage 1 — The headless core → 0.1

- Released as 0.1 on 2026-11-19, tag `v0.1.0`; the budgets measured again and the closing review
  of M1.13 followed the tag, at the owner's choice.
- Decisions: [ADR-0001](adr/0001-what-yuna-is.md) to
  [ADR-0132](adr/0132-an-agents-clipboard-data-with-control-characters-in-base64.md), the
  chains among them consolidated by ADR-0106 to ADR-0131 at the stage's audit.
- Delivered: a pure layout model, every behaviour of the layouts a test; motion on springs and
  curves, checked frame by frame; a headless compositor on Smithay with virtual outputs, emulated
  VRR and HDR descriptions, `color-management-v1`, captures and the protocols applications need;
  XWayland with Yuna's own X11 window manager, sharp at 1.5 and the right size on every output,
  the clipboard and drag and drop between X11 and Wayland; an MCP server that gives each agent a
  seat of its own, its programs on a throwaway home, their traces, logs and accessibility trees,
  the situations it tests, a call that waits for events and a rest auditor; `yuna --headless
  --agent-seat` and a Nix package for the owner's other projects; the rivals measured headless and
  the frame's, memory's and rest's budgets set from them; a Vulkan prototype measured on every
  tier; HDR through Smithay's DRM on the owner's monitor. SlimeEngine's examples and Opium's window
  run in Yuna in CI, driven by an agent through MCP.
- Milestones: M1.1 the workspace and its checks; M1.2 the layout model; M1.3 HDR through Smithay,
  a prototype; M1.4 the headless compositor; M1.5 agents' control; M1.6 emulated VRR and HDR;
  M1.7 animations; M1.8 XWayland; M1.9 the rivals measured; M1.10 Yuna for the owner's other
  projects; M1.11 the agents' toolbox; M1.12 drag and drop between X11 and Wayland; M1.13 the
  stage's audit ([audit](audits/2026-11-18-stage-1.md)): Rust 1.100, the audit's fixes, the X11
  manager and the MCP server split, `ARCHITECTURE.md` brought down, the ADRs consolidated,
  `SECURITY.md`, the issue and pull request templates.
- An audit of the architecture on 2026-11-17 ([audit](audits/2026-11-17-architecture.md)) set
  Stage 2's path: three gated parts, the way off Smithay, outside pull requests from 0.1.
