# Plan

What is being worked on now: the milestones of the current stage, their status and how each is
checked. Direction beyond this stage is in [`ROADMAP.md`](ROADMAP.md); decisions are in
[`adr/`](adr/README.md). A *stage* is a step of the roadmap; a *milestone* is a small, verifiable
step of a stage.

Every milestone ends with the definition of done in [`AGENTS.md`](../AGENTS.md#definition-of-done).

## Current

- The design session ran on 2026-11-11: [ADR-0001](adr/0001-what-yuna-is.md) to
  [ADR-0015](adr/0015-agents-workspaces.md), the [survey](survey.md), the roadmap and its bar.
- Next: [M1.1](#m11--the-workspace-and-its-checks), on the owner's go.

## Status

| Milestone | Goal | Status |
|---|---|---|
| [M1.1](#m11--the-workspace-and-its-checks) | One command checks everything, and CI runs it | Planned |
| [M1.2](#m12--the-layout-model) | Every layout behaviour is a test, without a compositor | Planned |
| [M1.3](#m13--hdr-through-smithay-a-prototype) | HDR drawn through Smithay on the owner's monitor, or the reason it cannot be | Planned |
| [M1.4](#m14--the-headless-compositor) | An application runs in Yuna without a screen | Planned |
| [M1.5](#m15--agents-control) | An agent drives an application in Yuna through MCP, on its own seat | Planned |
| [M1.6](#m16--emulated-vrr-and-hdr) | Virtual outputs behave as a VRR and HDR monitor would | Planned |
| [M1.7](#m17--animations) | Motion on springs and curves, checked frame by frame | Planned |
| [M1.8](#m18--xwayland) | X11 applications run, crisp at scale 1.5 | Planned |
| [M1.9](#m19--the-rivals-measured) | The rivals' figures, and the bar's budgets set from them | Planned |
| [M1.10](#m110--the-stages-audit) | Stage 1 examined whole, 0.1 tagged | Planned |

## Stage 1 — The headless core

Agents and tests drive Yuna without a screen, and every layout behaviour is a test
([ADR-0002](adr/0002-the-headless-mode-first.md)). Each new dependency is put to the owner when its
milestone first needs it.

### M1.1 — The workspace and its checks

- **Goal.** Any agent builds, checks and tests Yuna with one command, and CI runs it.
- **Delivers.** A Cargo workspace; the toolchain pinned; `flake.nix` with every tool the checks
  use; `scripts/check.sh` (format, lints as errors, tests, documentation, `cargo deny` allowing only
  licences compatible with GPL-3.0-or-later, links, the size of `AGENTS.md` and of this file);
  CI on GitHub; `CONTRIBUTING.md`; `docs/ARCHITECTURE.md`; `docs/glossary.md`. Smithay's revision
  chosen and pinned.
- **Done when.** `nix develop -c scripts/check.sh` passes locally and CI is green.
- **Measured.** The check's time, warm and cold; CI's job time.

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

### M1.3 — HDR through Smithay, a prototype

- **Goal.** Know whether Smithay's DRM backend, with Yuna's patches, can drive the owner's monitor
  in HDR ([ADR-0011](adr/0011-built-on-smithay.md)).
- **Delivers.** A throwaway program on a branch: it takes the monitor from a text console, sets
  `HDR_OUTPUT_METADATA`, `Colorspace` and 10-bit output, and shows test patterns of known PQ values,
  through OpenGL ([ADR-0012](adr/0012-opengl-first.md)).
- **Done when.** The patterns show as their values say, by the owner's eye and the monitor's own HDR
  indicator, or the reason they cannot is written; the patches it needed are listed.
- **By hand.** Agents have no access to the owner's display: the owner runs it from a text console.
- **Measured.** Nothing beyond the verdict; the program is then deleted, its findings kept here.

### M1.4 — The headless compositor

- **Goal.** A Wayland application runs in Yuna without a screen.
- **Delivers.** Yuna's headless output backend, in Yuna or its Smithay fork: virtual outputs of any
  size, rate and scale, 3840 × 2160 at 240 Hz and 1.5 by default; a controlled clock; drawing
  through GLES on the GPU or Mesa's CPU renderer. The protocols an application needs: `xdg-shell`,
  `linux-dmabuf`, viewporter, fractional scale, presentation time, `xdg-decoration`, cursor shape,
  data device, text input; the layout model of M1.2 behind them.
- **Done when.** Alacritty, a GTK 4 and a Qt 6 application, and uwui's and SlimeEngine's windowed
  examples run in it in CI, and each draws nothing at rest.
- **Measured.** Rest; a frame's GPU and CPU time on a 4K output.

### M1.5 — Agents' control

- **Goal.** An agent drives an application through Yuna's MCP server, on a seat of its own
  ([ADR-0015](adr/0015-agents-workspaces.md)).
- **Delivers.** The MCP server: outputs and workspaces created and removed, programs started, windows
  captured (`ext-image-copy-capture-v1` inside), pointer and keyboard input on the agent's seat,
  presentation times read, the clock stepped. One seat per agent; a test that an agent's input never
  reaches another seat.
- **Done when.** An agent, through MCP alone, opens one of uwui's examples, types, clicks and checks
  the result by capture.
- **Measured.** An MCP call's time; a capture's time on a 4K output.

### M1.6 — Emulated VRR and HDR

- **Goal.** Virtual outputs behave as the owner's monitor does for VRR and HDR.
- **Delivers.** A virtual output's VRR range and its presentation at variable intervals; its HDR
  capabilities announced through `color-management-v1`, and each surface's colour description
  recorded for tests to read.
- **Done when.** A test application sees VRR and HDR as on the owner's monitor, and a test reads the
  colour description Aerith's or mpv's surface sent.
- **Measured.** Presentation times against the emulated range.

### M1.7 — Animations

- **Goal.** Motion on springs and curves, interrupted without a jump
  ([ADR-0010](adr/0010-animations-and-effects.md)).
- **Delivers.** Springs and curves, configurable; a running animation retargeted keeping its
  velocity; the layout's moves animated; every animation stepped by the test clock.
- **Done when.** Snapshot tests check each animation frame by frame; scrolling and workspace
  switches hold 240 Hz headless without a missed frame.
- **Measured.** A frame's time during the heaviest animation.

### M1.8 — XWayland

- **Goal.** X11 applications run, crisp at scale 1.5.
- **Delivers.** XWayland started on demand, through Smithay's window manager; X11 windows rendered
  at the physical size where they can scale themselves.
- **Done when.** An X11 test program and a Wine program run headless at 1.5, their captures sharp
  against reference images.
- **Measured.** XWayland's start time and memory.

### M1.9 — The rivals measured

- **Goal.** The bar's budgets set from facts.
- **Delivers.** niri, Hyprland, KWin, dwl, mango and Umbriel measured at rest and in motion: wake-ups,
  CPU and GPU time, memory, power, frames during scrolling and animations; headless where they allow
  it, on the owner's desktop by hand otherwise, after the owner's consent.
- **Done when.** Each has its figures in `docs/measurements.md`, with versions and dates, or a
  written reason; the ROADMAP's budgets are set from them, in an ADR.

### M1.10 — The stage's audit

- **Goal.** Stage 1 examined whole before 0.1.
- **Delivers.** The audit in `docs/audits/`, its fixes, the stage moved to `docs/history.md`, the tag
  `v0.1.0` on the owner's go.

## Ideas

- Contributing Yuna's headless series to the owner's other projects' CI, beside SlimeEngine's
  compositors.

## Known debt

None yet.
