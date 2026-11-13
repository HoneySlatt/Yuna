# Plan

What is being worked on now: the milestones of the current stage, their status and how each is
checked. Direction beyond this stage is in [`ROADMAP.md`](ROADMAP.md); decisions are in
[`adr/`](adr/README.md). A *stage* is a step of the roadmap; a *milestone* is a small, verifiable
step of a stage.

Every milestone ends with the definition of done in [`AGENTS.md`](../AGENTS.md#definition-of-done).

## Current

- The design session ran on 2026-11-11: [ADR-0001](adr/0001-what-yuna-is.md) to
  [ADR-0015](adr/0015-agents-workspaces.md), the [survey](survey.md), the roadmap and its bar.
- [M1.1](#m11--the-workspace-and-its-checks) is done (2026-11-11): the workspace, its checks and CI;
  Rust 1.99.1 ([ADR-0016](adr/0016-the-rust-toolchain.md)) and Smithay `8fd9fe0`
  ([ADR-0017](adr/0017-smithays-revision.md)) pinned.
- [M1.2](#m12--the-layout-model) is done (2026-11-11): the layout model, `crates/yuna-layout`;
  workspaces ([ADR-0018](adr/0018-workspaces.md)), behaviours
  ([ADR-0019](adr/0019-the-layouts-behaviours.md)) and the model's shape
  ([ADR-0020](adr/0020-the-layout-models-shape.md)) decided with the owner.
- [M1.3](#m13--hdr-through-smithay-a-prototype) is done (2026-11-12): HDR through Smithay works on
  the owner's monitor; [ADR-0011](adr/0011-built-on-smithay.md) holds.
- [M1.4](#m14--the-headless-compositor) is done (2026-11-12):
  [ADR-0022](adr/0022-the-headless-backend-in-yuna.md) to
  [ADR-0030](adr/0030-a-frames-gpu-time-from-the-kernel.md).
- [M1.5](#m15--agents-control) designed on 2026-11-13 (ADR-0031 to ADR-0035); C1 done. Next:
  C2, `yuna mcp`.

## Status

| Milestone | Goal | Status |
|---|---|---|
| [M1.1](#m11--the-workspace-and-its-checks) | One command checks everything, and CI runs it | Done |
| [M1.2](#m12--the-layout-model) | Every layout behaviour is a test, without a compositor | Done |
| [M1.3](#m13--hdr-through-smithay-a-prototype) | HDR drawn through Smithay on the owner's monitor, or the reason it cannot be | Done |
| [M1.4](#m14--the-headless-compositor) | An application runs in Yuna without a screen | Done |
| [M1.5](#m15--agents-control) | An agent drives an application in Yuna through MCP, on its own seat | In progress |
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
- **Delivers.** A throwaway program on a branch: it takes the monitor from a text console, sets
  `HDR_OUTPUT_METADATA`, `Colorspace` and 10-bit output, and shows test patterns of known PQ values,
  through OpenGL ([ADR-0012](adr/0012-opengl-first.md)).
- **Done when.** The patterns show as their values say, by the owner's eye and the monitor's own HDR
  indicator, or the reason they cannot is written; the patches it needed are listed.
- **By hand.** Agents have no access to the owner's display: the owner runs it from a text console.
- **Measured.** Nothing beyond the verdict; the program is then deleted, its findings kept here.
- **Result.** Done on 2026-11-12: Smithay, with Yuna's one patch, drives the owner's monitor in
  HDR at 3840 × 2160 and 240 Hz; [ADR-0011](adr/0011-built-on-smithay.md) holds. The monitor
  compresses highlights above its peak rather than clip them, which Stage 5's tone mapping must
  know; a change of `Colorspace` is a modeset. What was set, seen and not observed is in
  [measurements](measurements.md) and [patches](smithay-patches.md); the program is deleted.

### M1.4 — The headless compositor

- **Goal.** A Wayland application runs in Yuna without a screen.
- **Design.** Decided with the owner on 2026-11-12: [ADR-0022](adr/0022-the-headless-backend-in-yuna.md)
  to [ADR-0025](adr/0025-applications-in-ci.md). Phase B starts once Phase A is done, with or
  without M1.3's verdict.
- **Delivers, Phase A: the layout model's client side**: asked and committed sizes, limits,
  dialogs, focus, activation ([ADR-0026](adr/0026-real-clients-in-the-layout.md)).
- **Delivers, Phase B: the compositor.** B1: `yuna --headless`, virtual outputs (ADR-0022),
  frames and clock (ADR-0023), `xdg-shell`, `xdg-decoration` (ADR-0024), `wl_shm`, presentation
  time, a seat with a keyboard. B2: popups, `xdg-activation`, `xdg-dialog`, `linux-dmabuf`,
  viewporter, fractional scale, data device, primary selection, `ext-data-control`; a cap on a
  client's windows; misbehaving clients and random requests in tests. B3: the applications in
  CI and the rest check on the program, the weekly SlimeEngine job (ADR-0025). B4: the measures.
- **Done when.** Alacritty, `gtk4-demo`, Qt's `qml` and `vkcube` run in Yuna in CI at each push, and
  SlimeEngine's `window` example in the weekly job; each draws nothing at rest by ADR-0025's
  check, `vkcube` and `window` once they leave (ADR-0029); misbehaving clients never take Yuna
  down.
- **Measured.** Rest; a frame's GPU and CPU time on a 4K output, on llvmpipe and, with the owner's
  consent (given 2026-11-12), on the GPU; CI's time; the layout's timings again, on a quiet
  machine ([measurements](measurements.md)).
- **Result.** Done on 2026-11-12, its closing review's findings fixed or decided. B2's and B3's
  tests found and fixed five defects. A 4K frame with two `vkcube`: 0.2 ms on the GPU with
  dmabufs, 7 ms on llvmpipe, which draws half the rate ([ADR-0030](adr/0030-a-frames-gpu-time-from-the-kernel.md),
  [measurements](measurements.md)); Yuna rests on the 4K output. What fell short is in
  [debt](debt.md).

### M1.5 — Agents' control

- **Goal.** An agent drives an application through Yuna's MCP server, on a seat of its own
  ([ADR-0015](adr/0015-agents-workspaces.md)).
- **Design.** Decided with the owner on 2026-11-13: frames ahead of their boundary
  ([ADR-0031](adr/0031-frames-drawn-ahead-of-their-boundary.md),
  [ADR-0032](adr/0032-input-to-present-read-by-mode.md)); `yuna mcp`, a private Yuna on stdio
  with its own MCP layer ([ADR-0033](adr/0033-the-mcp-server.md)); captures
  ([ADR-0034](adr/0034-captures.md)); seats ([ADR-0035](adr/0035-agents-seats.md)).
- **Steps.** C1: frames ahead (done). C2: `yuna mcp`, the MCP layer, `outputs`, `spawn`,
  `windows`, `clock`. C3: seats, their sockets, pointer and keyboard input, `cursor-shape`, drag
  and drop. C4: captures. C5: `text-input-v3`, the owner's peek, the done-when story, the rest of
  `vkcube` and `window`, the measures.
- **Delivers.** The MCP server: outputs and workspaces created and removed, programs started, windows
  captured (`ext-image-copy-capture-v1` inside), pointer and keyboard input on the agent's seat,
  presentation times read, the clock stepped; `cursor-shape` and `text-input-v3`, moved from M1.4
  so that input tests them. One seat per agent; a test that an agent's input never
  reaches another seat. The layout model arranges a workspace not shown, for the owner's peek
  ([ADR-0020](adr/0020-the-layout-models-shape.md)).
- **Done when.** An agent, through MCP alone, opens one of SlimeEngine's examples (uwui's once it has
  some, [ADR-0025](adr/0025-applications-in-ci.md)), types, clicks and checks
  the result by capture. `vkcube`, paused by its space key, and SlimeEngine's `window` rest with
  their window open, or the reason one cannot is written
  ([ADR-0029](adr/0029-the-rest-of-animated-applications.md)).
- **Measured.** An MCP call's time; a capture's time on a 4K output; frames missed against their
  lead, on the real clock ([measurements](measurements.md)).
- **For C2 to C5**, from the design's challenge: a bounded reply queue per client, its input
  no longer read while full, a line over 1 MiB ending it; MCP's timeouts on real time, `wait`
  under 60 s; programs with a clean environment (no `DISPLAY`, no D-Bus) and logs the agent
  reads; one seat per manual-clock Yuna, an `advance` past real time refused; ids checked
  against the seat that owns them; button serials for popup grabs; a capture's mapping to
  logical coordinates.
- **C1, frames ahead.** Done on 2026-11-13: Mutter's rule, at once when the output is idle, else at
  the boundary minus a lead measured per output, at most a period; feedback sent at its
  boundary. Eight stories pin it, the cadence of a steady client among them. On llvmpipe at
  1080p and 240 Hz, 0.4 to 0.9 % of frames missed, the lead at its cap; at 4K every frame late,
  as before ([measurements](measurements.md)).

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
  velocity; the layout's moves animated, the scrolling view as one camera the model gives out
  ([ADR-0020](adr/0020-the-layout-models-shape.md)); every animation stepped by the test clock.
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
- Layout behaviours the owner uses in Umbriel and ADR-0005 does not list: maximize-to-edges
  (Mod+Alt+M) and per-window heights in a column (Mod+Ctrl+Shift+R, Mod+Shift+Minus/Equal).
- Idle built into Yuna rather than swayidle (the owner's idea, 2026-11-12): its own timer, outputs
  dimmed and turned off, sleep, games, films and `idle-inhibit` keeping it awake, the shell's lock
  client started ([ADR-0004](adr/0004-the-compositor-alone-under-a-shell.md)); Stage 2's design.
- Directional focus that goes on to the next output at a strip's end (niri's `-or-monitor`
  actions).

## Known debt

In [`debt.md`](debt.md).
