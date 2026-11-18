# Plan

What is being worked on now: the milestones of the current stage, their status and how each is
checked. Direction beyond this stage is in [`ROADMAP.md`](ROADMAP.md); decisions are in
[`adr/`](adr/README.md). A *stage* is a step of the roadmap; a *milestone* is a small, verifiable
step of a stage.

Every milestone ends with the definition of done in [`AGENTS.md`](../AGENTS.md#definition-of-done).

## Current

- Stage 1's design sessions: 2026-11-11 ([ADR-0001](adr/0001-what-yuna-is.md) to
  [ADR-0015](adr/0015-agents-workspaces.md), the [survey](survey.md), the roadmap and its bar);
  2026-11-13, leaving Smithay step by step ([ADR-0039](adr/0039-leaving-smithay-step-by-step.md)
  to [ADR-0042](adr/0042-main-on-smithays-fork.md)); 2026-11-14, the line between Yuna and the
  shell, development first, Yuna's uses ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)
  to [ADR-0061](adr/0061-yunas-uses.md)).
- M1.1 to M1.7 are done (2026-11-11 to 14). Hollow's needs are in
  [`client-needs.md`](client-needs.md), three decided in [ADR-0065](adr/0065-what-hollow-asks-of-yuna.md).
- [M1.8](#m18--xwayland) is done (2026-11-15), with ADR-0066 to ADR-0070.
- [M1.9](#m19--the-rivals-measured) is done (2026-11-16), with ADR-0072 to ADR-0077.
- [M1.10](#m110--yuna-for-the-owners-other-projects) is done (2026-11-17), with ADR-0089.
- [M1.11](#m111--the-agents-toolbox) is in progress since 2026-11-17; the owner split X11's drag
  and drop off into [M1.12](#m112--drag-and-drop-between-x11-and-wayland) on 2026-11-18.
- From M1.11, the owner chose on 2026-11-17 to work on `main` directly, without a milestone's
  branch: push only a state where `scripts/check.sh` passes, and follow CI to green after each
  push; the closing review reads the milestone's commits on `main`, and its fixes follow there.
- The owner's desktop runs niri since 2026-11-16: Stage 2's goal is to replace it (ROADMAP).
- An [audit of the architecture](audits/2026-11-17-architecture.md) on 2026-11-17: ADR-0078 to
  ADR-0085 decided: Stage 2 in three gated parts, outside pull requests from 0.1.

## Status

| Milestone | Goal | Status |
|---|---|---|
| [M1.1](#m11--the-workspace-and-its-checks) | One command checks everything, and CI runs it | Done |
| [M1.2](#m12--the-layout-model) | Every layout behaviour is a test, without a compositor | Done |
| [M1.3](#m13--hdr-through-smithay-a-prototype) | HDR drawn through Smithay on the owner's monitor, or the reason it cannot be | Done |
| [M1.4](#m14--the-headless-compositor) | An application runs in Yuna without a screen | Done |
| [M1.5](#m15--agents-control) | An agent drives an application in Yuna through MCP, on its own seat | Done |
| [M1.6](#m16--emulated-vrr-and-hdr) | Virtual outputs behave as a VRR and HDR monitor would | Done |
| [M1.7](#m17--animations) | Motion on springs and curves, checked frame by frame | Done |
| [M1.8](#m18--xwayland) | X11 applications run, crisp at scale 1.5 and the right size on every output | Done |
| [M1.9](#m19--the-rivals-measured) | The rivals' figures, the bar's budgets set from them, a Vulkan prototype | Done |
| [M1.10](#m110--yuna-for-the-owners-other-projects) | The owner's other projects can measure and drive their programs in Yuna | Done |
| [M1.11](#m111--the-agents-toolbox) | An agent sees inside the programs it drives and makes the situations it tests | In progress |
| [M1.12](#m112--drag-and-drop-between-x11-and-wayland) | Drag and drop between X11 and Wayland programs | Planned |
| [M1.13](#m113--the-stages-audit) | Stage 1 examined whole, 0.1 tagged | Planned |

## Stage 1 — The headless core

Agents and tests drive Yuna without a screen, and every layout behaviour is a test
([ADR-0002](adr/0002-the-headless-mode-first.md)). Each new dependency is put to the owner when its
milestone first needs it.

### M1.1 — The workspace and its checks

- **Goal.** Any agent builds, checks and tests Yuna with one command, and CI runs it.
- **Result.** Done on 2026-11-11: the workspace, `flake.nix`, `scripts/check.sh` and CI
  ([CONTRIBUTING](../CONTRIBUTING.md)); 1.2 s warm, about 13 s cold ([measurements](measurements.md)).

### M1.2 — The layout model

- **Goal.** Every behaviour of [ADR-0005](adr/0005-the-layouts.md) is tested without a compositor.
- **Result.** Done on 2026-11-11: 97 scenarios, niri's behaviours rewritten and the two Umbriel
  cases among them; property tests over 1,536 random sequences per run; an operation on 100
  windows under 25 µs at the 99th percentile ([measurements](measurements.md)).

### M1.3 — HDR through Smithay, a prototype

- **Goal.** Know whether Smithay's DRM backend, with Yuna's patches, can drive the owner's monitor
  in HDR ([ADR-0011](adr/0011-built-on-smithay.md)).
- **Result.** Done on 2026-11-12 by the owner: Smithay, with one patch, drives the monitor in HDR
  at 3840 × 2160 and 240 Hz; ADR-0011 holds ([measurements](measurements.md),
  [patches](smithay-patches.md)).

### M1.4 — The headless compositor

- **Goal.** A Wayland application runs in Yuna without a screen.
- **Result.** Done on 2026-11-12 (ADR-0022 to ADR-0030): `yuna --headless`, virtual outputs,
  frames and clock, the shell and the protocols of B1 and B2; Alacritty, `gtk4-demo`, `qml`,
  `vkcube` in CI, SlimeEngine's `window` weekly, Yuna at rest after each; misbehaving clients and
  random requests in tests ([measurements](measurements.md), [debt](debt.md)).

### M1.5 — Agents' control

- **Goal.** An agent drives an application through Yuna's MCP server, on a seat of its own
  ([ADR-0015](adr/0015-agents-workspaces.md)).
- **Result.** Done on 2026-11-13 (ADR-0031 to ADR-0035, ADR-0037, ADR-0038, ADR-0043): frames
  drawn ahead of their boundary; `yuna mcp` and its tools; an agent, through MCP alone, opens
  SlimeEngine's `editor`, types, clicks and checks by capture. A call takes about 60 µs
  ([measurements](measurements.md), [debt](debt.md#captures)).

### M1.6 — Emulated VRR and HDR

- **Goal.** Virtual outputs behave as the owner's monitor does for VRR and HDR.
- **Result.** Done on 2026-11-14 (ADR-0045 to ADR-0051): main on the fork;
  `color-management-v1`, Yuna's own handler; VRR outputs emulate amdgpu as two traces of the
  owner's monitor showed; mpv in CI. A frame's start is the monitor's within 500 µs for 91 % of
  steady frames ([measurements](measurements.md), [debt](debt.md#vrr)).

### M1.7 — Animations

- **Goal.** Motion on springs and curves, interrupted without a jump
  ([ADR-0010](adr/0010-animations-and-effects.md)).
- **Result.** Done on 2026-11-14 (ADR-0052 to ADR-0056, ADR-0059): `yuna-motion`, springs in
  closed form and curves, tested alone; Yuna draws it with `--animations` or
  `--reduced-motion`, each frame checked against the reference. On the GPU at 4K and 240 Hz,
  motion missed no frame of 10,000, the heaviest 1.3 ms; on llvmpipe at 1080p it misses more than
  half, recorded ([measurements](measurements.md), [debt](debt.md#motion)).

### M1.8 — XWayland

- **Goal.** X11 applications run, crisp at scale 1.5 and the right size on every output.
- **Design.** Decided with the owner on 2026-11-14, ADR-0066 to ADR-0069.
- **Result.** Done on 2026-11-15, its closing review's findings fixed or written as debt: Yuna's
  own X11 manager, one Xwayland per seat on demand, X11 windows and menus in the layout and the
  motion, drawn pixel for pixel at 1.5 and area-filtered at 1, the bounds of ADR-0070
  ([ADR-0070](adr/0070-bounds-on-what-one-x11-client-may-ask.md)); Qt on X11 equal to Qt on
  Wayland at 1.5, Wine scaled by its `LogPixels`. Xwayland starts in 29 ms on llvmpipe, 76 ms on
  the GPU, and takes 36 to 61 MiB; the area filter costs 0.18 ms a 4K frame on the GPU
  ([measurements](measurements.md)). What is left is [debt](debt.md#xwayland).

### M1.9 — The rivals measured

- **Goal.** The bar's budgets set from facts.
- **Result.** Done on 2026-11-16 (ADR-0072 to ADR-0077), its closing review's findings fixed or
  written as debt: `crates/yuna-bench` runs nine rivals headless, each in a sandbox, and reads their
  costs from outside; the series at equal work, 189 cases, none disturbed; the Vulkan prototype on
  the desktop, lavapipe, the laptop and the Deck, the laptop's display checked
  ([measurements](measurements.md)). In a fullscreen game Yuna costs 160 to 166 µs of CPU and 191
  to 196 µs of GPU a frame, below every peer; its own and its GPU's memory are the peers' least,
  the process's growth a window above Hyprland's ([ADR-0077](adr/0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md),
  [debt](debt.md#architecture)). Frames, latency and watts on real displays, with Yuna, come at
  Stage 2's exit ([ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md));
  the tour's command waits on branch `m1.9-tour`. Stage 2's core is decided
  ([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)).

### M1.10 — Yuna for the owner's other projects

- **Goal.** The owner's other projects measure and drive their programs in Yuna as in a headless
  sway ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)).
- **Result.** Done on 2026-11-17 ([ADR-0089](adr/0089-yuna-for-the-owners-other-projects.md)):
  `yuna --headless --agent-seat`; the virtual keyboard and pointer on agents' seats, never the
  owner's; `wlr-screencopy` v3; a Nix package with its own Mesa, built and started in CI. uwui's
  `measure.py` and Pluto's bench's driver, `pluto-input`, pinned in `flake.nix`, drive a program
  in it in `check.sh`; sway's IPC, which the rest of Pluto's bench reads, has no equivalent ([consumers](../crates/yuna/tests/consumers.rs)). [`protocols.md`](protocols.md)
  and its test, [`client-needs.md`](client-needs.md) complete, `--help` and `tools/list` held by
  tests, `scripts/test-check.sh`. The audit's three debts paid.
  Measured by uwui's `measure.py`, `vkcube` on the GPU at 4K and 240 Hz: Yuna presents 240.0
  frames a second, every interval 4.17 ms; a headless sway 244, not on a grid
  ([measurements](measurements.md)).

### M1.11 — The agents' toolbox

- **Goal.** An agent sees inside the programs it drives and makes the situations it tests
  ([ADR-0061](adr/0061-yunas-uses.md), [ADR-0062](adr/0062-the-compositors-comfort-and-look.md)).
- **Delivers.** Through MCP: each program's protocol trace and log; the accessibility tree, its
  dependency put to the owner; outputs plugged and unplugged, scale and mode changed, a slow or
  stuck client, keyboard layouts, the clipboard between programs, X11's and Wayland's
  ([ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md)); a call that waits
  for an event; a rest auditor naming what wakes Yuna or makes it draw; on a panic of its state,
  its programs ended and its folder removed ([ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)).
- **Done when.** Through MCP alone, an agent finds a protocol error and a button by its name, and
  checks a window across an output unplugged.
- **Measured.** `yuna mcp`'s start, memory and instances together, which set their budgets; N
  `yuna mcp` against one with N seats, up to 10, for Stage 2's choice of
  [ADR-0078](adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md).
- **Status.** The owner chose on 2026-11-18: Yuna as the accessibility registry on a bus of each
  seat's, off by default, D-Bus on `zvariant`; the trace from the client, protocol errors from
  Yuna, no wayland-rs fork before Stage 2; layouts changed at run time; the auditor's command a
  start option until `yuna-ipc`. Done on `main`: the fatal path, the debts of `keyboard tap`,
  encoders and reaping.

### M1.12 — Drag and drop between X11 and Wayland

- **Goal.** A drag from an X11 program drops into a Wayland one, and the reverse, through Yuna's
  X11 manager ([ADR-0066](adr/0066-yunas-own-x11-window-manager-on-x11rb.md)).
- **Done when.** Headless tests drag text and a file's URI both ways, a drag cancelled and a
  source gone mid-drag included.

### M1.13 — The stage's audit

- **Goal.** Stage 1 examined whole before 0.1.
- **Toolchain.** Rust 1.100 replaces 1.99.1 there, in a commit of its own
  ([ADR-0016](adr/0016-the-rust-toolchain.md)), chosen by the owner on 2026-11-18; its new
  features are then put to the owner one at a time, each on a measure.
- **Delivers.** The audit in `docs/audits/`, its fixes, the stage moved to `docs/history.md`, the tag
  `v0.1.0` on the owner's go. For contributors: the X11 manager and the MCP server split into
  smaller modules; a README that says what to read for each part; `SECURITY.md`; issue and pull
  request templates holding the definition of done and
  [ADR-0085](adr/0085-outside-contributions-issues-now-pull-requests-from-0-1.md)'s process; a newcomer test, an agent new to Yuna making a
  small change, its stumbles fixed; the chains of ADRs that amend one another (ADR-0001 to
  ADR-0070) consolidated, one ADR read for each topic.

## Ideas

- Scripts inside Yuna, on events only, bounded in time; an effect governor that lightens the
  costliest effect near a frame's deadline; a replay capsule for agents, after M1.11
  ([ADR-0062](adr/0062-the-compositors-comfort-and-look.md)).
- Layout behaviours the owner uses in Umbriel and ADR-0005 does not list: maximize-to-edges
  (Mod+Alt+M) and per-window heights in a column (Mod+Ctrl+Shift+R, Mod+Shift+Minus/Equal).
- The RemoteDesktop and InputCapture portals, for remote control or a mouse shared between
  machines, once a concrete use asks for them, with consent each session, an indicator and a
  shortcut that stops them ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)).
- A settings application, apart from Hollow, on uwui, opened from its control centre: it sets
  Yuna through Yuna's commands or an include of its own, and is the first client of an
  output-management protocol ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md)).
- A command that sets one configuration key until the next reload, as `hyprctl keyword` does,
  beside variants ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)).
- Directional focus that goes on to the next output at a strip's end (niri's `-or-monitor`
  actions).
- From Claude Code's run of `yuna mcp` (C5): a `wait_ms` on `keyboard` for a text field to be
  enabled, and a tool that reads a program's log, which an agent without a shell cannot.

- A second Xwayland at scale 1 for games, on its own display, so that a game on an output of a
  smaller scale always draws at its mode ([ADR-0069](adr/0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md)).

## Known debt

In [`debt.md`](debt.md).
