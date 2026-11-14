# Plan

What is being worked on now: the milestones of the current stage, their status and how each is
checked. Direction beyond this stage is in [`ROADMAP.md`](ROADMAP.md); decisions are in
[`adr/`](adr/README.md). A *stage* is a step of the roadmap; a *milestone* is a small, verifiable
step of a stage.

Every milestone ends with the definition of done in [`AGENTS.md`](../AGENTS.md#definition-of-done).

## Current

- Design session 2026-11-11: [ADR-0001](adr/0001-what-yuna-is.md) to
  [ADR-0015](adr/0015-agents-workspaces.md), the [survey](survey.md), the roadmap and its bar.
- M1.1 to M1.6 are done (2026-11-11 to 14); a design study on 2026-11-13 decided to leave Smithay
  step by step ([ADR-0039](adr/0039-leaving-smithay-step-by-step.md) to
  [ADR-0042](adr/0042-main-on-smithays-fork.md)).
- [M1.7](#m17--animations) is under way: designed with the owner on 2026-11-14 (ADR-0052 to
  ADR-0056); its steps below.

## Status

| Milestone | Goal | Status |
|---|---|---|
| [M1.1](#m11--the-workspace-and-its-checks) | One command checks everything, and CI runs it | Done |
| [M1.2](#m12--the-layout-model) | Every layout behaviour is a test, without a compositor | Done |
| [M1.3](#m13--hdr-through-smithay-a-prototype) | HDR drawn through Smithay on the owner's monitor, or the reason it cannot be | Done |
| [M1.4](#m14--the-headless-compositor) | An application runs in Yuna without a screen | Done |
| [M1.5](#m15--agents-control) | An agent drives an application in Yuna through MCP, on its own seat | Done |
| [M1.6](#m16--emulated-vrr-and-hdr) | Virtual outputs behave as a VRR and HDR monitor would | Done |
| [M1.7](#m17--animations) | Motion on springs and curves, checked frame by frame | In progress |
| [M1.8](#m18--xwayland) | X11 applications run, crisp at scale 1.5 and the right size on every output | Planned |
| [M1.9](#m19--the-rivals-measured) | The rivals' figures, the bar's budgets set from them, Vulkan against GLES | Planned |
| [M1.10](#m110--the-stages-audit) | Stage 1 examined whole, 0.1 tagged | Planned |

## Stage 1 — The headless core

Agents and tests drive Yuna without a screen, and every layout behaviour is a test
([ADR-0002](adr/0002-the-headless-mode-first.md)). Each new dependency is put to the owner when its
milestone first needs it.

### M1.1 — The workspace and its checks

- **Goal.** Any agent builds, checks and tests Yuna with one command, and CI runs it.
- **Delivers.** A Cargo workspace, the toolchain pinned, `flake.nix`, `scripts/check.sh` and CI
  ([CONTRIBUTING](../CONTRIBUTING.md)); `docs/ARCHITECTURE.md`; `docs/glossary.md`. Smithay's
  revision chosen and pinned.
- **Done when.** `nix develop -c scripts/check.sh` passes locally and CI is green.
- **Measured.** The check's time, warm and cold; CI's job time.
- **Result.** Done on 2026-11-11: 1.2 s warm, about 13 s cold, CI's job 82 s uncached
  ([measurements](measurements.md)).

### M1.2 — The layout model

- **Goal.** Every behaviour of [ADR-0005](adr/0005-the-layouts.md) is tested without a compositor.
- **Delivers.** A crate with no Wayland object: workspaces, outputs and seats; scrolling columns,
  dwindle, floating windows, special workspaces; fullscreen, full-width, fake fullscreen, preset
  widths, a pinned window. Property tests over random sequences of operations; niri's behaviours,
  read in its documentation and tests, rewritten as scenarios
  ([ADR-0013](adr/0013-our-own-code-niri-and-jay-as-references.md)); the one-column fullscreen and
  dwindle cases the owner found broken in Umbriel among them.
- **Done when.** Each behaviour has its scenario; the property tests find no broken invariant over
  their runs in CI.
- **Measured.** An operation's time on a workspace of 100 windows.
- **Result.** Done on 2026-11-11: 97 scenarios, the two Umbriel cases among them; property tests
  over 1,536 random sequences per run; an operation on 100 windows under 25 µs at the 99th
  percentile ([measurements](measurements.md)).

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
- **Design.** Decided with the owner on 2026-11-14: a pure crate, `yuna-motion`, turns the
  layout's jumps into offsets, one camera per scrolling workspace, workspace ids
  ([ADR-0052](adr/0052-motion-in-a-pure-crate.md)); springs in closed form keeping their
  velocity, ended within half a pixel; Bézier curves
  ([ADR-0053](adr/0053-springs-in-closed-form-keeping-their-velocity.md)); sampled at each
  frame's presentation, drawn apart from what clients are told
  ([ADR-0054](adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)); motion, the
  special workspace's fade, a vertical switch over two workspaces, fullscreen instant, off
  headless unless `--animations` ([ADR-0055](adr/0055-what-moves-first-and-animations-off-headless.md));
  the budget on the GPU against a still baseline
  ([ADR-0056](adr/0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md)).
- **Steps.** E1: workspace ids, each placed window's workspace and layer, each shown workspace's
  camera. E2: `yuna-motion`'s springs and curves, checked against a numerical integration. E1
  and E2 run in parallel. E3: the motion of an arrangement, under random sequences. E4: the
  compositor: `--animations`, sampling and frames, the drawn list, what clients are told, the
  pointer, captures, popups, MCP's `windows`. E5: frame-by-frame tests and pacing under draw
  hooks. E6: the driver and the measures, the GPU's with the owner's consent.
- **Done when.** Each animation is checked frame by frame on the test clock against
  `yuna-motion`'s reference; scrolls and switches on the GPU at 4K and 240 Hz miss no more
  frames than a still baseline over 10,000 frames.
- **Measured.** A frame's time during the heaviest frame of the scripted sequence.

### M1.8 — XWayland

- **Goal.** X11 applications run, crisp at scale 1.5 and the right size on every output.
- **Delivers.** XWayland started on demand, through Smithay's window manager or Yuna's own, chosen
  in its design ([ADR-0039](adr/0039-leaving-smithay-step-by-step.md)); X11 windows drawn at the
  largest output scale where they can scale themselves, and scaled down on outputs of a smaller
  scale ([ADR-0044](adr/0044-xwayland-across-outputs-of-different-scales.md)).
- **Done when.** An X11 test program and a Wine program run headless at 1.5, their captures sharp
  against reference images; an X11 window moved between outputs at 1.5 and 1 keeps its size on
  both. The design states how a fullscreen X11 game on the output at 1 draws at that output's mode.
- **Measured.** XWayland's start time and memory.

### M1.9 — The rivals measured

- **Goal.** The bar's budgets set from facts, and Vulkan decided on them.
- **Delivers.** niri, Hyprland, KWin, dwl, mango and Umbriel measured at rest and in motion: wake-ups,
  CPU and GPU time, memory, power, frames during scrolling and animations; headless where they allow
  it, on the owner's desktop by hand otherwise, after the owner's consent. A throwaway Vulkan
  prototype measured against GLES on every tier, the laptop included
  ([ADR-0041](adr/0041-vulkan-decided-on-a-measure.md)).
- **Done when.** Each has its figures in `docs/measurements.md`, with versions and dates, or a
  written reason; the ROADMAP's budgets are set from them, in an ADR; the owner has chosen the
  renderer on the prototype's figures, in another.

### M1.10 — The stage's audit

- **Goal.** Stage 1 examined whole before 0.1.
- **Delivers.** The audit in `docs/audits/`, its fixes, the stage moved to `docs/history.md`, the tag
  `v0.1.0` on the owner's go.

## Ideas

- Contributing Yuna's headless series to the owner's other projects' CI, beside SlimeEngine's
  compositors.
- Layout behaviours the owner uses in Umbriel and ADR-0005 does not list: maximize-to-edges
  (Mod+Alt+M) and per-window heights in a column (Mod+Ctrl+Shift+R, Mod+Shift+Minus/Equal).
- Idle built into Yuna rather than swayidle (the owner's idea, 2026-11-12): its own timer, outputs
  dimmed and turned off, sleep, games, films and `idle-inhibit` keeping it awake, the shell's lock
  client started ([ADR-0004](adr/0004-the-compositor-alone-under-a-shell.md)); Stage 2's design.
- Directional focus that goes on to the next output at a strip's end (niri's `-or-monitor`
  actions).
- From Claude Code's run of `yuna mcp` (C5): a `wait_ms` on `keyboard` for a text field to be
  enabled, and a tool that reads a program's log, which an agent without a shell cannot.

## Known debt

In [`debt.md`](debt.md).
