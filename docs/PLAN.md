# Plan

What is being worked on now: the milestones of the current stage, their status and how each is
checked. Direction beyond this stage is in [`ROADMAP.md`](ROADMAP.md); decisions are in
[`adr/`](adr/README.md). A *stage* is a step of the roadmap; a *milestone* is a small, verifiable
step of a stage.

Every milestone ends with the definition of done in [`AGENTS.md`](../AGENTS.md#definition-of-done).

## Current

- The design session ran on 2026-11-11: [ADR-0001](adr/0001-what-yuna-is.md) to
  [ADR-0015](adr/0015-agents-workspaces.md), the [survey](survey.md), the roadmap and its bar.
- M1.1 to M1.5 are done (2026-11-11 to 13), with ADR-0016 to ADR-0038 and ADR-0043.
- A design study on 2026-11-13: Yuna leaves Smithay step by step, keeps wayland-rs, and decides
  Vulkan on a prototype's figures ([ADR-0039](adr/0039-leaving-smithay-step-by-step.md) to
  [ADR-0042](adr/0042-main-on-smithays-fork.md)).
- [M1.6](#m16--emulated-vrr-and-hdr) is done (2026-11-14), with ADR-0045 to ADR-0051. Next:
  M1.7, on the owner's go.
- A design session on 2026-11-14 ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md),
  [ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md),
  [ADR-0060](adr/0060-development-first-agents-in-their-own-yuna.md),
  [ADR-0061](adr/0061-yunas-uses.md)): the line between Yuna and the shell; what the owner's
  other projects taught; development first and Stage 3 removed; Yuna's uses. M1.10 and M1.11
  added; reduced motion joins M1.7.

## Status

| Milestone | Goal | Status |
|---|---|---|
| [M1.1](#m11--the-workspace-and-its-checks) | One command checks everything, and CI runs it | Done |
| [M1.2](#m12--the-layout-model) | Every layout behaviour is a test, without a compositor | Done |
| [M1.3](#m13--hdr-through-smithay-a-prototype) | HDR drawn through Smithay on the owner's monitor, or the reason it cannot be | Done |
| [M1.4](#m14--the-headless-compositor) | An application runs in Yuna without a screen | Done |
| [M1.5](#m15--agents-control) | An agent drives an application in Yuna through MCP, on its own seat | Done |
| [M1.6](#m16--emulated-vrr-and-hdr) | Virtual outputs behave as a VRR and HDR monitor would | Done |
| [M1.7](#m17--animations) | Motion on springs and curves, checked frame by frame | Planned |
| [M1.8](#m18--xwayland) | X11 applications run, crisp at scale 1.5 and the right size on every output | Planned |
| [M1.9](#m19--the-rivals-measured) | The rivals' figures, the bar's budgets set from them, a Vulkan prototype | Planned |
| [M1.10](#m110--yuna-for-the-owners-other-projects) | The owner's other projects can measure and drive their programs in Yuna | Planned |
| [M1.11](#m111--the-agents-toolbox) | An agent sees inside the programs it drives and makes the situations it tests | Planned |
| [M1.12](#m112--the-stages-audit) | Stage 1 examined whole, 0.1 tagged | Planned |

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
- **Result.** Done on 2026-11-11. The check takes 1.2 s warm and about 13 s cold locally, CI's job
  82 s without a cache ([measurements](measurements.md)). The `yuna` crate depends on Smithay
  without using it until M1.4 (ADR-0017).

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
  over 1,536 random sequences per run; two rounds of closing review; an operation and `arrange` on
  100 windows under 25 µs at the 99th percentile ([measurements](measurements.md)).

### M1.3 — HDR through Smithay, a prototype

- **Goal.** Know whether Smithay's DRM backend, with Yuna's patches, can drive the owner's monitor
  in HDR ([ADR-0011](adr/0011-built-on-smithay.md)).
- **Result.** Done on 2026-11-12, by the owner from a text console: Smithay, with Yuna's one
  patch, drives the monitor in HDR at 3840 × 2160 and 240 Hz; ADR-0011 holds. The monitor
  compresses highlights above its peak; a change of `Colorspace` is a modeset. Details in
  [measurements](measurements.md) and [patches](smithay-patches.md); the program is deleted.

### M1.4 — The headless compositor

- **Goal.** A Wayland application runs in Yuna without a screen.
- **Design.** Decided with the owner on 2026-11-12:
  [ADR-0022](adr/0022-the-headless-backend-in-yuna.md) to
  [ADR-0026](adr/0026-real-clients-in-the-layout.md).
- **Result.** Done on 2026-11-12, its closing review's findings fixed or decided: `yuna
  --headless`, virtual outputs, frames and clock, the shell and the protocols of B1 and B2;
  Alacritty, `gtk4-demo`, `qml` and `vkcube` in CI and SlimeEngine's `window` weekly, Yuna at
  rest after each (ADR-0025, ADR-0029); misbehaving clients and random requests in tests. A 4K
  frame with two `vkcube`: 0.2 ms on the GPU, 7 ms on llvmpipe
  ([ADR-0030](adr/0030-a-frames-gpu-time-from-the-kernel.md), [measurements](measurements.md)).
  What fell short is in [debt](debt.md).

### M1.5 — Agents' control

- **Goal.** An agent drives an application through Yuna's MCP server, on a seat of its own
  ([ADR-0015](adr/0015-agents-workspaces.md)).
- **Result.** Done on 2026-11-13, decided with the owner in ADR-0031 to ADR-0035, ADR-0037,
  ADR-0038 and ADR-0043. Frames drawn ahead of their boundary; `yuna mcp` with its tools:
  outputs, workspaces, programs, windows, captures, pointer and keyboard on the agent's own
  seat and socket, the clock, text input; the owner's peek. An agent, through MCP alone, opens
  SlimeEngine's `editor`, types, clicks and checks by capture; `vkcube` paused and `editor` rest
  with their windows open, `window` cannot by its design. A call takes about 60 µs; a 4K capture
  holds the main thread 4 ms on the GPU, 47 ms on llvmpipe ([measurements](measurements.md),
  [debt](debt.md#captures)). The closing review's findings are fixed or written as debt.

### M1.6 — Emulated VRR and HDR

- **Goal.** Virtual outputs behave as the owner's monitor does for VRR and HDR.
- **Design.** Decided with the owner on 2026-11-14: the fork over HTTPS, a branch per base
  ([ADR-0045](adr/0045-the-fork-over-https-a-branch-per-base.md)); amdgpu's VRR emulated, checked
  against a trace of the monitor ([ADR-0046](adr/0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md));
  `color-management-v1` with KWin's list ([ADR-0047](adr/0047-color-management-v1-kwins-list.md));
  HDR outputs announce PQ, drawn at Stage 5 ([ADR-0048](adr/0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md));
  `vrr_min`, `hdr` and `colour` through MCP ([ADR-0049](adr/0049-vrr-and-hdr-outputs-through-mcp.md));
  mpv in CI ([ADR-0050](adr/0050-mpv-in-the-applications-tests.md)).
- **Steps.** D1: main on the fork, seats and outputs filtered per client, saturating
  positioners ([ADR-0042](adr/0042-main-on-smithays-fork.md)). D2: `color-management-v1`,
  versions 1 to 3, through Yuna's own test client. D3: VRR outputs on the manual clock. D4:
  `vrr_min`, `hdr`, `colour`. D5: mpv. D6: the owner's trace, replayed; the measures.
- **Result.** Done on 2026-11-14. Main is on the fork; `color-management-v1` is Yuna's own
  handler; VRR outputs emulate amdgpu as two traces of the owner's monitor showed
  ([ADR-0051](adr/0051-emulated-vrr-as-the-monitors-traces-showed.md)); mpv in CI reads
  Yuna's colours and declares its own. A frame's start is the monitor's within 500 µs for 91 %
  of steady frames, 82 % of probes; what is left is [debt](debt.md#vrr). The closing review
  found two blocking defects and the last check one; all are fixed with their tests. CI's job
  takes 3 min 39 s ([measurements](measurements.md)).

### M1.7 — Animations

- **Goal.** Motion on springs and curves, interrupted without a jump
  ([ADR-0010](adr/0010-animations-and-effects.md)).
- **Delivers.** Springs and curves, configurable; a running animation retargeted keeping its
  velocity; the layout's moves animated, the scrolling view as one camera the model gives out
  ([ADR-0020](adr/0020-the-layout-models-shape.md)); every animation stepped by the test clock;
  reduced motion honoured by each animation ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)).
- **Done when.** Snapshot tests check each animation frame by frame; scrolling and workspace
  switches hold 240 Hz headless without a missed frame.
- **Measured.** A frame's time during the heaviest animation.

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

- **Goal.** The bar's budgets set from facts.
- **Delivers.** niri, Hyprland, KWin, dwl, mango and Umbriel measured at rest and in motion: wake-ups,
  CPU and GPU time, memory, power, frames during scrolling and animations; headless where they allow
  it, on the owner's desktop by hand otherwise, after the owner's consent. A throwaway Vulkan
  prototype measured on every tier, the laptop's driver checked
  ([ADR-0063](adr/0063-vulkan-alone-and-uwui-for-what-yuna-draws.md)).
- **Done when.** Each has its figures in `docs/measurements.md`, with versions and dates, or a
  written reason; the ROADMAP's budgets are set from them, in an ADR; the prototype's figures set the
  frame's budgets.

### M1.10 — Yuna for the owner's other projects

- **Goal.** The owner's other projects measure and drive their programs in Yuna as in a headless
  sway ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)).
- **Delivers.** A Nix package, built in CI; the virtual keyboard and pointer on agents' seats
  only, tested never to reach the owner's; `docs/client-needs.md`; `docs/protocols.md` and its
  test against the globals announced; `scripts/test-check.sh`.
- **Done when.** uwui's `measure.py` and Pluto's bench drive a program in Yuna as in sway, without
  changing their repositories; both tests run in `check.sh`.
- **Measured.** A program's presentation times in Yuna against headless sway's.

### M1.11 — The agents' toolbox

- **Goal.** An agent sees inside the programs it drives and makes the situations it tests
  ([ADR-0061](adr/0061-yunas-uses.md), [ADR-0062](adr/0062-the-compositors-comfort-and-look.md)).
- **Delivers.** Through MCP: each program's protocol trace and log; the accessibility tree, its
  dependency put to the owner; outputs plugged and unplugged, scale and mode changed, a slow or
  stuck client, keyboard layouts, clipboard and drag and drop between programs; a call that waits
  for an event; a rest auditor naming what wakes Yuna or makes it draw.
- **Done when.** Through MCP alone, an agent finds a protocol error and a button by its name, and
  checks a window across an output unplugged.
- **Measured.** `yuna mcp`'s start, memory and instances together, which set their budgets.

### M1.12 — The stage's audit

- **Goal.** Stage 1 examined whole before 0.1.
- **Delivers.** The audit in `docs/audits/`, its fixes, the stage moved to `docs/history.md`, the tag
  `v0.1.0` on the owner's go.

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

## Known debt

In [`debt.md`](debt.md).
