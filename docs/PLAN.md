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
- [M1.4](#m14--the-headless-compositor) designed on 2026-11-12
  ([ADR-0022](adr/0022-the-headless-backend-in-yuna.md) to
  [ADR-0025](adr/0025-applications-in-ci.md)); B2's bounds in
  [ADR-0027](adr/0027-smithay-without-overflow-checks.md) and
  [ADR-0028](adr/0028-bounds-on-what-one-client-may-ask.md); B3's in
  [ADR-0029](adr/0029-the-rest-of-animated-applications.md). Current: M1.4, Phase B4.

## Status

| Milestone | Goal | Status |
|---|---|---|
| [M1.1](#m11--the-workspace-and-its-checks) | One command checks everything, and CI runs it | Done |
| [M1.2](#m12--the-layout-model) | Every layout behaviour is a test, without a compositor | Done |
| [M1.3](#m13--hdr-through-smithay-a-prototype) | HDR drawn through Smithay on the owner's monitor, or the reason it cannot be | Done |
| [M1.4](#m14--the-headless-compositor) | An application runs in Yuna without a screen | In progress |
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
- **Result.** Done on 2026-11-11. 97 scenarios, the two Umbriel cases among them; 70 name the
  behaviour of niri or Hyprland they rewrite or depart from, the others test Yuna's own rules. Property
  tests over 1,536 random sequences per run check the invariants, that one seat never changes
  another's state, and that a refused operation changes nothing. Two rounds of closing review:
  their defects are fixed, each with its scenario; their other findings are answered in
  [ADR-0019](adr/0019-the-layouts-behaviours.md) and [ADR-0020](adr/0020-the-layout-models-shape.md)
  or kept as debt below. An operation and `arrange` on 100 windows: under 25 µs at the 99th
  percentile; linear up to 1,000 ([measurements](measurements.md)). What is not built yet is listed
  in ADR-0019; terminal swallowing and per-application rules need clients: Stage 2.

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
  HDR; [ADR-0011](adr/0011-built-on-smithay.md) holds. The owner ran it from a text console under
  KDE, once KWin's colour settings were off: KWin had left a `GAMMA_LUT`, which the prototype
  refuses. It set `max bpc` 10, `Colorspace` `BT2020_RGB` and `HDR_OUTPUT_METADATA` (each tested
  with a modeset), scanned out `Xbgr2101010` at 3840 × 2160 and 240 Hz, and gave all three back
  at exit. The monitor switched to HDR. Pattern 1's steps rose in order from a true black; above
  the panel's peak, pattern 2's squares kept rising up to 4,000 cd/m², so the monitor compresses
  highlights rather than clips them (Stage 5's tone mapping must know); pattern 3 showed from
  0.005 cd/m²; BT.2020's primaries were more saturated than BT.709's. A change of `Colorspace` is
  a modeset: the screen goes black about a second while amdgpu trains the DisplayPort link again,
  its first DPCD writes failing. Not observed: whether the monitor reads `Colorspace`, pattern
  5's bands, the depth the monitor shows. The program and its branch are deleted, its run steps
  with them; the patch stays in the fork for Stage 5 ([patches](smithay-patches.md)).

### M1.4 — The headless compositor

- **Goal.** A Wayland application runs in Yuna without a screen.
- **Design.** Decided with the owner on 2026-11-12: [ADR-0022](adr/0022-the-headless-backend-in-yuna.md)
  to [ADR-0025](adr/0025-applications-in-ci.md). Phase B starts once Phase A is done, with or
  without M1.3's verdict.
- **Delivers, Phase A: the layout model's client side** (done): asked and committed sizes,
  limits, dialogs, focus, activation ([ADR-0026](adr/0026-real-clients-in-the-layout.md)).
- **Delivers, Phase B: the compositor.** B1 (done): `yuna --headless`, virtual outputs (ADR-0022),
  frames and clock (ADR-0023), `xdg-shell`, `xdg-decoration` (ADR-0024), `wl_shm`, presentation
  time, a seat with a keyboard. B2 (done): popups, `xdg-activation`, `xdg-dialog`, `linux-dmabuf`,
  viewporter, fractional scale, data device, primary selection, `ext-data-control`; a cap on a
  client's windows; misbehaving clients and random requests in tests. B3 (done): the
  applications in CI and the rest check on the program, the weekly SlimeEngine job (ADR-0025).
  B4: the measures.
- **Done when.** Alacritty, `gtk4-demo`, Qt's `qml` and `vkcube` run in Yuna in CI at each push, and
  SlimeEngine's `window` example in the weekly job; each draws nothing at rest by ADR-0025's
  check, `vkcube` and `window` once they leave (ADR-0029); misbehaving clients never take Yuna
  down.
- **Measured.** Rest; a frame's GPU and CPU time on a 4K output, on llvmpipe and, with the owner's
  consent (given 2026-11-12), on the GPU; CI's time; the layout's timings again, on a quiet
  machine ([measurements](measurements.md)).
- **Where it stands** (2026-11-12). Phases A, B1, B2 and B3 are done. B2's tests found and
  fixed four defects: SIGBUS from a shrunk dmabuf memfd, corrupted records from second roles, a
  client ended outside a dispatch left shown. In B3, each application runs in its own
  `yuna --headless` in CI's job, all in about 15 s; Yuna then has 0 context switches in 10 s, and
  `vkcube` left to draw fails the check. All draw through `wl_shm`. SlimeEngine's example runs in
  a weekly workflow. A race that let Smithay terminate a shared EGL display under a starting
  compositor is fixed. What falls short is in [debt](debt.md). Next: B4, in a new session.
  Delegated briefs and their proofs are in `.delegation/` (local).

### M1.5 — Agents' control

- **Goal.** An agent drives an application through Yuna's MCP server, on a seat of its own
  ([ADR-0015](adr/0015-agents-workspaces.md)).
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
