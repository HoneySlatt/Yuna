# Roadmap

Yuna's direction: what it is, its quality bar, its decisions, and its stages in order. The current
work is in [`PLAN.md`](PLAN.md); the decisions and why, in [`adr/`](adr/README.md); the facts behind
them, in [`survey.md`](survey.md). Update this file when a stage is reshaped, a budget changes or an
open decision is taken.

## 1. Vision

Yuna is a Wayland compositor in Rust, for Linux, in two modes ([ADR-0001](adr/0001-what-yuna-is.md)):

- **headless**, where agents and tests drive it: virtual outputs, emulated VRR and HDR, a controlled
  clock, captures and input over MCP, one seat per agent;
- **the desktop**, its owner's daily compositor: scrolling columns as good as niri's, dwindle,
  floating windows and special workspaces; animations more beautiful than Hyprland's; reliable HDR
  and colour; VRR without flicker on an OLED; games and films served first; less power than any
  rival; agents working in headless Yunas on the owner's GPU, never touching the owner's input.

Its uses, and the owner's programs that stand for them ([ADR-0061](adr/0061-yunas-uses.md)):

| Use | The owner's programs | What Yuna gives |
|---|---|---|
| Development by hand | Zed, VS Code, Neovim, browsers, Podman, QEMU | XWayland, virtual machines' keyboard grab, a test output in a window |
| Development by agents | Pluto, Claude Code, the owner's projects | a headless Yuna each, MCP, traces, logs, accessibility tree, situations on demand |
| Office and the web | LibreOffice, Brave, Firefox, Element, Signal | portals, sharing, picture in picture, windows back in place after a reboot |
| Games | Steam, Heroic, emulators | VRR without flicker, tearing, scanout, pointer lock, gamescope nested |
| Films and series | mpv, Jellyfin, browsers | exact cadence, HDR per display |
| Capture and streaming | OBS, Discord | captures that cost a game no frame, HDR or SDR, windows never captured |
| Creation | Kdenlive, Resolve, GIMP, Krita, Inkscape, Blender, Wine plugins, Magnolia, Unreal, SlimeEngine | colour measured, Wine and plugin windows placed, pointer warp |

It is made for its owner first, a heavy gamer and a cinephile. It is public, under
GPL-3.0-or-later ([ADR-0009](adr/0009-gpl-private-until-ready.md), [ADR-0036](adr/0036-yuna-is-public.md)).

## 2. Principles

1. **Polish before breadth.** A behaviour lands when it is as good as the best rival's, or waits.
2. **Every behaviour is a test first**, run headless in CI, before it is code on the desktop.
3. **Nothing runs, polls or draws at rest.**
4. **Games and films first**: no effect or animation may cost a game a frame or a film its
   cadence. Agents' work never holds back for a game: the owner's activity sets the priority
   ([ADR-0060](adr/0060-development-first-agents-in-their-own-yuna.md)).
5. **The owner's session is sacred**: an agent never reaches the owner's seat, and a crash costs at
   most a second.
6. **Measured, not claimed**: every budget has its measure, kept in `docs/measurements.md`.

## 3. Quality bar

### References

On each criterion, the best rival measured is the bar, and Yuna beats it
([survey](survey.md)):

| Criterion | Rival to beat |
|---|---|
| Scrolling columns, their feel and frames | niri |
| Animations | Hyprland |
| Power at rest and in motion | dwl, mango |
| HDR and colour | KWin |
| XWayland at a fractional scale | none: sharp at the largest scale and the right size on every output ([ADR-0044](adr/0044-xwayland-across-outputs-of-different-scales.md)) |
| Games: latency, VRR | gamescope, KWin |
| Capture and streaming: a game's frames and latency during a capture | gamescope, KWin [ADR-0061](adr/0061-yunas-uses.md) |
| Creation: colour accuracy, a tablet's latency and pressure | KWin [ADR-0061](adr/0061-yunas-uses.md) |
| A test compositor for agents | the series of headless compositors of SlimeEngine's M6d.6 (Weston, sway, KWin, Mutter) |

### Machines

- **High tier**: RX 6900 XT, Ryzen 7 5800X, ASUS PG32UCDM at 3840 × 2160 and 240 Hz, scale 1.5:
  4.17 ms a frame.
- **Lowest tier**: the HP laptop, AMD Athlon Silver 3050U with Radeon Vega graphics and 4 GiB; its
  display, storage and Vulkan driver are still to be read.
- **Steam Deck**, its desktop mode.
- **CI**: the headless mode on a CPU renderer.

([ADR-0006](adr/0006-machines-and-gpus.md))

### Budgets

Starting figures, tightened once the rivals are measured (Stage 1). A budget missed is debt until
it is met.

| Budget | Figure | Measured by |
|---|---|---|
| At rest | 0 frames, 0 own wake-ups a second, 0 % of a core | every thread's context switches over 10 s, headless and on the desktop |
| Motion | no missed frame at 240 Hz while scrolling, animating or switching workspaces | presentation feedback over scripted gestures, headless and on the owner's monitor |
| Input to present | at a fixed rate, no commit presented after the first boundary it could reach, and end to end at the 99th percentile against gamescope and KWin; under VRR or tearing, one frame, 4.17 ms on the high tier ([ADR-0032](adr/0032-input-to-present-read-by-mode.md)) | input given through the compositor, presentation feedback and the frame log read |
| A frame's cost | GPU and CPU time per frame, set from the rivals' at Stage 1 | the kernel's DRM usage statistics and each thread's CPU time; the renderer's timers for each effect from Stage 6 ([ADR-0030](adr/0030-a-frames-gpu-time-from-the-kernel.md)) |
| Power | below dwl's and mango's, at rest and in motion, on the high tier and the laptop | the GPU's sensor on the desktop; the battery on the laptop |
| Memory | set from the rivals' at Stage 1 | resident memory at rest with 15 windows |
| Start | from launch to the first frame, set from the rivals' at Stage 2 | the log's times |
| A crash | back within 1 s; no window lost of an application that reconnects; the others reopened in place | a test that kills Yuna and counts the windows |
| VRR flicker | none on the desktop; in games, measured on the owner's monitor | a light sensor on the screen |
| HDR | the output's luminance and colours as the colour description says, against KWin on the same content | captures of the output buffer in CI; the owner's monitor by eye and by sensor |
| The laptop | every feature, effects lighter, no missed frame at its refresh rate | the same measures on the laptop |
| A capture | no frame of a game lost and its direct scanout kept while captured, the capture lowering its own rate rather than cost one; the capture's GPU time | presentation feedback with and without a capture running |
| Colour accuracy | set from KWin's at Stage 5 | captures of the output buffer against the colours asked; the owner's monitor by probe |
| A tablet | set from KWin's, once a tablet is at hand | input to present of the pen; pressure and tilt passed on |
| `yuna mcp` | start, memory per instance and instances together, set at the agents' toolbox | the log's times; resident memory |
| An agent's output | full rate while its programs draw, nothing at rest; no input of an agent reaching the owner's seat | presentation feedback; a test that tries |

### Without a figure

- Games recognised and served first: VRR in a high band, tearing on request, direct scanout,
  effects off.
- Films at a constant rate, an exact multiple of their own.
- A configuration error never stops Yuna.
- Reduced motion and reduced transparency honoured by each animation and each effect; the rest of
  accessibility later ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)).

## 4. Decisions

### Fixed

| Topic | Decision |
|---|---|
| What Yuna is | One compositor, two modes ([ADR-0001](adr/0001-what-yuna-is.md)) |
| Order | Headless first ([ADR-0002](adr/0002-the-headless-mode-first.md)); a parallel project, uwui first ([ADR-0003](adr/0003-a-parallel-project-uwui-first.md)) |
| Scope | The compositor alone, a shell on top ([ADR-0004](adr/0004-the-compositor-alone-under-a-shell.md)) |
| Layouts | Scrolling columns, dwindle, floating, special workspaces ([ADR-0005](adr/0005-the-layouts.md)) |
| Machines | Linux; AMD and Intel first ([ADR-0006](adr/0006-machines-and-gpus.md)) |
| Crashes | Socket kept, restart within 1 s, windows restored ([ADR-0007](adr/0007-surviving-a-crash.md)) |
| Configuration | KDL, reloaded live ([ADR-0008](adr/0008-configuration-in-kdl.md)) |
| Licence | GPL-3.0-or-later, public ([ADR-0009](adr/0009-gpl-private-until-ready.md), [ADR-0036](adr/0036-yuna-is-public.md)) |
| Look | Springs and curves; blur, glass, corners, shadows, borders; built-in effects ([ADR-0010](adr/0010-animations-and-effects.md)) |
| Base | Smithay, left step by step, gone by the end of Stage 2 ([ADR-0011](adr/0011-built-on-smithay.md), [ADR-0039](adr/0039-leaving-smithay-step-by-step.md)) |
| Wire protocol | wayland-rs, calloop and drm-rs kept; Yuna's own wire only on a measure ([ADR-0040](adr/0040-the-wire-protocol-stays-wayland-rs.md)) |
| Renderer | Vulkan alone, Yuna's own from Stage 2; uwui hosted for what Yuna draws ([ADR-0012](adr/0012-opengl-first.md), [ADR-0063](adr/0063-vulkan-alone-and-uwui-for-what-yuna-draws.md)) |
| Code | Our own; niri, jay and Smithay as references ([ADR-0013](adr/0013-our-own-code-niri-and-jay-as-references.md), [ADR-0039](adr/0039-leaving-smithay-step-by-step.md)) |
| VRR | Fixed-rate desktop; films at exact multiples; games in a high band ([ADR-0014](adr/0014-vrr-without-flicker.md)) |
| Agents | Their own outputs and seats, at full rate, each in its own headless Yuna ([ADR-0015](adr/0015-agents-workspaces.md), [ADR-0060](adr/0060-development-first-agents-in-their-own-yuna.md)) |
| Toolchain | An exact stable Rust, pinned in `rust-toolchain.toml` ([ADR-0016](adr/0016-the-rust-toolchain.md)) |
| Smithay's revision | Upstream git, one revision pinned, raised at each stage's end ([ADR-0017](adr/0017-smithays-revision.md)) |
| Smithay's fork | A repository of its own, generic patches, listed in `smithay-patches.md`; main on it from M1.6 ([ADR-0021](adr/0021-smithays-fork.md), [ADR-0042](adr/0042-main-on-smithays-fork.md)) |
| Headless backend | Yuna's own code on Smithay, drawn on llvmpipe unless the GPU is asked ([ADR-0022](adr/0022-the-headless-backend-in-yuna.md)) |
| Frames and clock | Drawn on damage, ahead of their boundary, presented honestly, deadlines owned by a clock tests step; hidden windows once a second ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md), [ADR-0031](adr/0031-frames-drawn-ahead-of-their-boundary.md)) |
| Decorations | Server side: no title bars ([ADR-0024](adr/0024-no-title-bars.md)) |
| The other projects | Every protocol listed and tested; hidden games paced; Yuna 0.1 the owner's projects' test compositor; GPU resets survived; Nix modules in Yuna; fuzzing and crash reports ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)) |
| Yuna and the shell | Wallpaper, idle, captures, the share picker and a night light in Yuna; portals in `yuna-portal`; the theme a file Yuna includes; the shell told what is fullscreen; the owner's shell is Hollow, on uwui, on Yuna alone ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md), [ADR-0063](adr/0063-vulkan-alone-and-uwui-for-what-yuna-draws.md)) |
| Applications in CI | Real applications at each push, rest checked on the program ([ADR-0025](adr/0025-applications-in-ci.md)) |
| XWayland's scale | The largest output's, scaled down on smaller ones ([ADR-0044](adr/0044-xwayland-across-outputs-of-different-scales.md)) |

### Open

| Topic | Decided at |
|---|---|
| The KDL reader | the milestone that reads the configuration |
| The games' band: its floor and smoothing | Stage 4, on the light sensor's figures |
| How a film's rate is known | Stage 5 |
| Yuna's scene and damage tracking | the opening of Stage 2 ([ADR-0039](adr/0039-leaving-smithay-step-by-step.md)) |
| Yuna's X11 window manager or Smithay's | M1.8's design ([ADR-0039](adr/0039-leaving-smithay-step-by-step.md)) |
| Public release | the owner's go |

## 5. Stages

### Stage 1 — The headless core → 0.1

**Goal.** Agents and tests drive Yuna without a screen, and every layout behaviour is a test.

- The layout model, pure: scrolling columns, dwindle, floating, special workspaces, fullscreen,
  full-width and fake fullscreen, preset widths; property tests and niri's behaviours rewritten as
  scenarios.
- The headless backend: virtual outputs (3840 × 2160 at 240 Hz and scale 1.5 among them), a
  controlled clock, rendering on the GPU or a CPU renderer.
- The core protocols an application needs; XWayland.
- Agents' control: an MCP server, a seat per agent, captures, input, presentation times.
- Emulated VRR timing and HDR colour descriptions on virtual outputs; `color-management-v1`, Yuna's
  first protocol handler of its own.
- Animations on springs and curves, checked frame by frame on the test clock.
- A throwaway prototype of HDR through Smithay's DRM backend on the owner's monitor.
- The rivals measured at rest and in motion; the bar's figures set.
- A throwaway Vulkan prototype, measured on every tier, the laptop's driver checked, to set the
  frame's budgets ([ADR-0063](adr/0063-vulkan-alone-and-uwui-for-what-yuna-draws.md)).
- Yuna for the owner's other projects
  ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)): a Nix package, the
  virtual keyboard and pointer on agents' seats, what each project needs in
  `docs/client-needs.md`; every protocol in `docs/protocols.md`, kept true by a test;
  `scripts/test-check.sh`.

**Exit.** SlimeEngine's and Aerith's windowed examples, and uwui's once it has some
([ADR-0025](adr/0025-applications-in-ci.md)), run in Yuna headless in CI, driven by an agent
through MCP; every layout behaviour of ADR-0005 has its tests, except terminal swallowing
and per-application rules, which need clients and come at Stage 2; the HDR prototype has its
verdict; the rivals' figures and the Vulkan prototype's are in `docs/measurements.md`; the owner's
other projects can add Yuna to their shells and drive programs in it as they drive them in sway.

### Stage 2 — The desktop → 0.2

**Goal.** Yuna replaces Umbriel as the owner's daily compositor, without HDR yet, on a base of its
own ([ADR-0039](adr/0039-leaving-smithay-step-by-step.md)).

- First, Yuna's own core: surfaces, xdg-shell, seats and popups, its scene and damage tracking,
  its Vulkan renderer hosting uwui, on wayland-rs; WLCS run against it
  ([ADR-0063](adr/0063-vulkan-alone-and-uwui-for-what-yuna-draws.md)).
- DRM and KMS on drm-rs, libinput and the session, written by Yuna; several outputs at different rates, hotplug with workspaces put back, mirroring.
- The desktop at a fixed rate: no VRR flicker.
- XWayland crisp at 1.5 and the right size on every output.
- Amane on top through standard protocols.
- What Yuna takes from the shell's tools ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)):
  idle with dimming and the lock started; the wallpaper; the theme file and variants; the
  fullscreen state for the shell; `yuna-portal` with screen sharing, screenshots, global shortcuts,
  inhibition and the wallpaper; the share picker; captures and recording; a night light in SDR.
- The touchpad's gestures; the configuration live; per-application rules, terminal swallowing;
  input methods (`input-method-v2`).
- The crash restart and session restore; a crash report kept on the machine; recovery from a GPU
  reset without a restart ([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)).
- The protocols games and the owner's applications need, as `docs/protocols.md` sets them:
  pointer constraints and relative pointer, `fifo-v1` and `commit-timing-v1`, a hidden game paced
  at its configured rate.
- The package's NixOS and home-manager modules; the clients' messages fuzzed weekly.
- The owner's uses ([ADR-0061](adr/0061-yunas-uses.md)): tablets, pointer warp, `xdg-foreign`, Wine's and plugins'
  windows placed; `keyboard-shortcuts-inhibit`; windows never captured; a test output in a
  window; picture in picture; windows back in place after a reboot, on by default; the GPU's loss
  on demand for agents; VKMS studied first.
- Hot corners; tabbed columns; focus following the mouse and floating windows' sizes remembered,
  as settings ([ADR-0062](adr/0062-the-compositors-comfort-and-look.md)).
- What the best desktops taught
  ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md)): focus marked, not stolen;
  Super-drag; several fullscreen windows on a workspace; frozen applications shown; a primary output;
  back-and-forth workspaces; the laptop's lid; the cursor hidden while typing; custom gestures,
  shortcuts per application, keys passed to X11 applications; brightness and DDC/CI; a cast's
  target changed live; the bell to the shell; custom output modes; no black flash at start;
  Hollow's interface of commands and events.

**Exit.** The owner uses Yuna for a week in place of Umbriel; the budgets of rest, motion, input and
power are met on the high tier and the laptop; Smithay is no longer in `Cargo.lock`.

### Stage 3 — Removed

Agents work in their own headless Yunas on the owner's GPU, from Stage 1 on; none enters the
owner's desktop ([ADR-0060](adr/0060-development-first-agents-in-their-own-yuna.md)). The later
stages keep their numbers.

### Stage 4 — Games → 0.3

**Goal.** Games are better on Yuna than on any other compositor.

- VRR in a high band with smoothed changes, its prototype measured with a light sensor.
- Tearing on request, direct scanout and overlay planes, game mode; games told apart in the
  fullscreen state; settings for what a game turns off ([ADR-0061](adr/0061-yunas-uses.md)).
- gamescope nested in Yuna, tested; a smaller game scaled by the display hardware where it can;
  virtual outputs for streaming; a capture that costs a game no frame; the Steam Deck's touch
  and on-screen keyboard; a living-room session of one fullscreen application ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md)).

**Exit.** The owner's games run with their VRR flicker measured below the bar's figure, and
latency and frames at least as good as gamescope's.

### Stage 5 — The image → 0.4

**Goal.** HDR and colour as reliable as KWin's, or better.

- HDR output, SDR brightness, tone and gamut mapping, ICC profiles; a profile per display, from
  its EDID corrected by a guided adjustment or a probe; colour accuracy measured; HDR captures
  kept HDR or converted to SDR ([ADR-0061](adr/0061-yunas-uses.md)); HDR always on or only for
  fullscreen HDR content, a setting ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md)).
- The night light, captures and screen sharing correct under HDR; films at an exact multiple of
  their rate, and told apart in the fullscreen state; no sleep while a video plays.
- OLED care: luminance read from the panel, static elements protected from burn-in. Its means
  are chosen at Stage 5's design, from a survey of what panels, other systems and studies of
  burn-in do; a wear map of each output is one candidate ([ADR-0062](adr/0062-the-compositors-comfort-and-look.md)).

**Exit.** The owner's films and HDR games show correctly, measured against KWin on the same content.

### Stage 6 — Effects and polish → 0.5

**Goal.** The most beautiful desktop, at the lowest cost.

- Blur and glass, over the wallpaper too, animated borders, the built-in effects, exotic open and
  close animations; the wallpaper's animated transitions.
- The overview, Umbriel's, itself niri's improved ([ADR-0019](adr/0019-the-layouts-behaviours.md));
  Alt-Tab with previews, the capture tool's polish, a colour picker; snap zones for floating
  windows; shapes that morph ([ADR-0062](adr/0062-the-compositors-comfort-and-look.md)).
- Sandboxed applications kept from sensitive protocols; capture and new keyboards asked, allowed
  or refused per program; reduced transparency for blur and glass; a cross-fade on a change of
  theme; corners and shadows for windows that draw their own frame; effects lighter on battery
  ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md)).

**Exit.** Each effect's GPU time and power measured and within budget; the owner approves the look.

### Later

NVIDIA; the user's own shaders; accessibility: Yuna's own interfaces read by a screen reader, a
magnifier, high contrast, sticky and slow keys; shortcut modes; then the audit
before 1.0.

## 6. Release map

| Version | Stage |
|---|---|
| 0.1 | 1, the headless core |
| 0.2 | 2, the desktop |
| 0.3 | 4, games |
| 0.4 | 5, the image |
| 0.5 | 6, effects and polish |
| 1.0 | after the later work and its audit |

Releases: a version, a CHANGELOG and a tag, after the owner's go.
