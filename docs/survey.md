# Survey of the field

The facts behind Yuna's first decisions, read in their sources on 2026-11-11. Each section names its
sources; versions and dates are those of that day. Update a fact when it is found wrong; a decision
that rests on it is reopened in an ADR.

## Foundations

- **Smithay** ([github.com/Smithay/smithay](https://github.com/Smithay/smithay)): MIT. Latest release
  0.7.0 (2025-06-24); compositors build on git revisions. 234 commits by 60 authors in the six months
  before the survey. GLES, glow and pixman renderers, a multi-GPU wrapper, no Vulkan renderer (its
  main author is writing one, per a comment of 2026-05-12 on pull request 982). Atomic and legacy
  DRM, overlay and cursor planes, direct scanout, `linux-drm-syncobj-v1`, libei, an X11 window
  manager for XWayland. No colour management or HDR: pull requests 982 and 1143 are drafts. No
  tearing (pull requests 1325, 1948, 2276 open). No headless output backend. CI runs part of WLCS
  against its sample compositor, anvil. `AI.md` asks that AI-generated code be flagged in commit
  messages and advises against language models.
- **niri** ([github.com/niri-wm/niri](https://github.com/niri-wm/niri)): GPL-3.0-or-later, v26.04
  (2026-04-25), effectively one maintainer. Scrolling columns, floating windows, tabbed columns; no
  dwindle. Animations on springs or curves. No HDR, tearing or explicit sync. XWayland through
  xwayland-satellite, whose X11 windows are slightly blurry at fractional scales (its issue 479).
  One seat. Property tests of the layout, snapshot tests, a test clock, a headless backend for
  tests. Its contribution policy refuses code written by language models.
- **cosmic-comp** ([github.com/pop-os/cosmic-comp](https://github.com/pop-os/cosmic-comp)):
  GPL-3.0-only, a tiling tree with floating and stacked windows, VRR on fullscreen, no HDR (tracking
  issue 1384), tied to COSMIC's protocols and libcosmic.
- **jay** ([github.com/mahkoh/jay](https://github.com/mahkoh/jay)): GPL-3.0-only, v1.15.0
  (2026-09-28), one author, about 301,000 lines of Rust with no Smithay. Its own Vulkan renderer
  (about 9,900 lines), `wp_color_manager_v1` v3 and HDR10 output (no tone or gamut mapping, per its
  1.10 notes), VRR and tearing modes, explicit sync, headless and virtual outputs, overlay
  ("special") workspaces, XWayland rendered at the highest integer scale then downscaled. An
  i3-like layout, no animation system.
- **Umbriel** ([github.com/noctalia-dev/umbriel](https://github.com/noctalia-dev/umbriel)): MIT,
  C++23 on wlroots 0.20 and a fork of SceneFX, no tagged release. `xwayland_native_resolution` shows
  X11 applications the physical size, so they are sharp.

## HDR and colour

- `color-management-v1` is in wayland-protocols' staging, at version 3 (wayland-protocols 1.50,
  2026-10-29). KWin, Mutter, Hyprland, jay, Weston and wlroots 0.20 implement it; gamescope uses a
  protocol of its own. Clients: mpv, Kodi, Wine's Wayland driver, Mesa's Vulkan WSI, Firefox,
  Chromium (on by default), GTK, Qt.
- The kernel's per-plane colour pipeline (`COLOR_PIPELINE`) merged in Linux 6.19, with amdgpu and
  i915. KWin, Weston and jay (off by default) use it. The older connector properties
  `HDR_OUTPUT_METADATA` (from 5.3) and `Colorspace` remain.
- **KWin**, the reference most cite
  ([invent.kde.org/plasma/kwin](https://invent.kde.org/plasma/kwin)): colour descriptions and
  pipelines; blending in a gamma 2.2 space; SDR brightness as a reference luminance; tone mapping in
  ICtCp when content is brighter than the output; SDR and HDR ICC profiles; luminance from the
  EDID's HDR static metadata, overridable, 800 cd/m² when missing; night light as a white-point
  change with a final colorimetric step, correct under HDR.
- Hyprland's wiki still marks HDR experimental, and its releases 0.55.1 to 0.57.2 fix HDR bugs.
- What compositors list in `color-management-v1`, read on 2026-11-14 (KWin `01a35d1f`, Mutter
  and Weston `main`, wlroots 0.20.2, jay `ede5bd77`): KWin version 3, parametric, no ICC, no
  `srgb` transfer function ("intentionally not supported"); Mutter version 2, ICC, `srgb` to
  version-1 clients; Weston version 2, ICC; wlroots version 2, gamma 2.2 and PQ; jay version 3,
  no ICC. KWin announces its blending space (gamma 2.2 over BT.2020) as outputs' and surfaces'
  preferred description; jay announces PQ with the display's EDID volume. None caps a client's
  descriptions.
- What clients need from it, read the same day: mpv 0.42 (`video/out/wayland_common.c`) uses it
  only with `parametric`, sets PQ over BT.2020 with 0 / 10,000 / 203 cd/m² and HDR metadata, and
  by default copies its surface's preferred description as its target. Mesa's Vulkan WSI binds
  version 1, exposes a Vulkan colour space when its primaries and transfer function are listed,
  takes a colour surface for sRGB swapchains only when `srgb` is listed, and turns a refused
  description into a lost surface when `extended_target_volume` is listed. GTK 4 binds version 1
  and drops colour management without `srgb`; Chromium needs `parametric`.
- In `color-management-v1` version 1, `max_cll` and `max_fall` outside the target range are a
  protocol error; version 2 drops the rule. Mesa binds version 1.
- `wp_presentation` version 3 (wayland-protocols 1.50) adds the flags `fixed_rate` and
  `variable_rate`; KWin, Mutter and wlroots send version 2, where a VRR output's refresh is its
  fastest rate or 0. No other protocol tells a client that VRR is on.
- No compositor reads panel luminance from DisplayID (libdisplay-info does not parse it yet), models
  an OLED's automatic brightness limiting, or protects OLEDs from burn-in.
- No Wayland protocol lets a client ask for a refresh rate; no compositor switches the rate itself
  for 23.976, 24 or 25 fps video. KWin's open merge request 9882 proposes `content-frame-rate-v1`.

## VRR flicker

- OLED gamma is tuned for one refresh rate; a change of rate shifts dark greys
  ([TFTCentral, 2025-06-05](https://tftcentral.co.uk/articles/exploring-and-testing-oled-vrr-flicker);
  [RTINGS](https://www.rtings.com/monitor/learn/research/vrr-flicker)). On QD-OLED, TFTCentral finds
  short spikes at random rates, more frequent at low rates. RTINGS notes visible VRR flicker on the
  PG32UCDM.
- The PG32UCDM's firmware (MCM112, 2026-09-22) has no anti-flicker setting; ASUS's "OLED
  Anti-Flicker", on other models, narrows the VRR range (for example 160 to 240 Hz).
- On Linux desktops, the rate falls to the panel's minimum at rest and jumps on each cursor move
  (KDE bug 477016). Mitigations shipped: VRR on fullscreen only (KWin's default "Automatic", Hyprland
  2 and 3, niri `on-demand`), cursor updates that do not raise the rate (Hyprland
  `no_break_fs_vrr`, niri `skip-cursor-only-updates-during-vrr`, KWin merge request 7700). KWin
  turned adaptive sync off by default (merge request 7684), citing flicker "we can't properly limit
  without new KMS APIs". No compositor limits the rate's change per frame or holds a floor.
- amdgpu repeats frames below the panel's minimum (BTR, `freesync.c`, Linux master `97d73c82`,
  2026-11-13, the same as 6.17): from the time between the last two flips it picks `n`
  refreshes of equal, fixed length near the range's middle, with hysteresis; at 48–240 Hz, a
  24 fps film gets 3 refreshes of 13.9 ms. Ranges under 2× take `apply_fixed_refresh`, which
  holds the top rate after five slow frames. KMS exposes only a `VRR_ENABLED` boolean.
- In amdgpu's display code (Linux `e0d8e8de`, 2026-11-08, the same in 7.3): BTR's flip
  timestamp is taken in the commit's plane loop; its writes of the refresh's range are deferred
  from the vertical-update interrupt to a worker that takes the lock the flip holds; after five
  flips close to a vertical update, a workaround holds the top rate and skips BTR; the commit
  waits in 1 ms steps while the CRTC is in its blank. A flip event's time is the next active
  area's start.
- Under VRR, KWin, Mutter, niri and gamescope aim a frame at the last presentation plus 1/max and
  report the page flip's time; KWin and gamescope hold cursor-only updates, Mutter coalesces at
  30 Hz, gamescope can hold the Steam interface at its rate; none limits how fast the rate
  changes (KWin `01a35d1f`, Mutter `35f64c3d`, gamescope `56603967`).
- The owner's monitor's EDID (read 2026-11-14): 48–240 Hz, PQ and BT.2020, desired content
  luminance 1015.2 cd/m², 265.0 on a frame's average.

## Games

- No desktop compositor upscales games; gamescope offers FSR, NIS and others, a frame limiter and
  HDR.
- Tearing control: KWin, Hyprland, sway, jay; not niri or COSMIC. Games are recognised by the
  `content-type-v1` hint (Hyprland, KWin, Umbriel).
- XWayland at fractional scale: KWin lets applications scale themselves; Hyprland's
  `force_zero_scaling`; Umbriel's native resolution; jay's downscaled mode; COSMIC's
  `descale_xwayland`.
- XWayland across outputs of different scales (read on 2026-11-14): X11 has one scale for every
  output, and no compositor found keeps an X11 window both sharp and the right size on each.
  - Umbriel's native resolution draws X11 at the largest output's scale and does not correct it
    elsewhere ([its configuration
    reference](https://github.com/noctalia-dev/umbriel/blob/main/docs/user/configuration.md)). Seen
    by the owner on 2026-11-14: an X11 window sharp on an output at 1.5 stays 1.5 times too large
    when moved to an output at 1.
  - KWin's "apply scaling themselves" has the same limit (KDE bugs
    [459183](https://bugs.kde.org/show_bug.cgi?id=459183) and
    [474738](https://bugs.kde.org/show_bug.cgi?id=474738)). Its developers call downscaling on one
    of the screens unavoidable with one XWayland, and name one XWayland per application as a
    possible future fix ([502390](https://bugs.kde.org/show_bug.cgi?id=502390)).
  - The approach proposed at Linux Plumbers 2019 ([Roman Gilg, "XWayland multi
    DPI"](https://lpc.events/event/5/contributions/358/attachments/457/712/xwayland_multi_dpi.pdf)):
    X11 draws at the largest scale, and the compositor scales each window down on outputs of a
    smaller scale. It is sharp on the largest scale, the right size everywhere and a little softer
    on smaller scales.
- No measured comparison of compositors' power was found; only users' reports.

## Surviving a crash

- KWin's wrapper keeps the Wayland socket and restarts KWin; Qt 6 reconnects with
  `QT_WAYLAND_RECONNECT`; GTK's reconnection is a draft; SDL's is disabled
  ([KDE wiki, Restarting](https://invent.kde.org/plasma/kwin/-/wikis/Restarting)).
  [wl-restart](https://github.com/Ferdi265/wl-restart) does the same for other compositors.
- `xdg-session-management-v1` is in staging since 2026-03; KWin and Mutter implement it.

## Testing and automation

- WLCS, Mir's conformance suite, runs in Smithay's CI on anvil. niri tests its layout with property
  tests and snapshots on a test clock. KWin's integration tests run on a virtual backend with
  llvmpipe. Weston alone compares renders with reference images. No compositor is fuzzed, as far as
  the sources show.
- libei/EIS in KWin, Mutter, Hyprland and Smithay; `ext-image-copy-capture-v1` in wlroots, Hyprland,
  Smithay, niri and COSMIC; `security-context-v1` in wlroots, Hyprland, Smithay, niri, COSMIC and
  KWin.
- Shells: Quickshell needs `wlr-layer-shell`, `ext-session-lock-v1`, foreign toplevel management,
  `ext-workspace-v1` and image capture.

## Hidden workspaces

- Hyprland sends frame callbacks only to shown workspaces; niri sends hidden windows about one a
  second (`FRAME_CALLBACK_THROTTLE`, 995 ms); KWin suspends hidden windows unless held for
  offscreen rendering; Mutter suspends them after 3 s. None renders a hidden workspace at full rate
  by default.

## Animations

Read in their sources on 2026-11-14: niri `73a04441` (`src/animation/`, `src/layout/scrolling.rs`,
`monitor.rs`, `src/niri.rs`), hyprutils `ab978a33` and Hyprland `0767f634`, KWin `c90230b7`
(`src/effect/springmotion.cpp`, `animationclock`), libadwaita `main` of 2026-11-07, Apple's
`SwiftUI.Spring` documentation and WWDC23 session 10158.

- **The spring.** All solve `x'' + c x' + k x = 0`, mass 1, around the target. niri and
  libadwaita evaluate it in closed form from (from, to, velocity, start); Hyprland steps a
  normalised (value, velocity) by the elapsed time, exactly since hyprutils PR #110
  (2026-05-22), by Euler with a clamped step before; KWin integrates it by RK4 at 10 ms.
- **Its parameters.** niri, KWin, libadwaita and Android: damping ratio and stiffness. Hyprland:
  mass, stiffness and damping. SwiftUI: duration and bounce, where stiffness = (2π/duration)² and
  bounce = 1 − damping ratio for bounce ≥ 0; its "duration" is perceptual, not the time to rest.
- **When it ends.** niri and libadwaita compute a duration once, from the envelope `e^(−βt)`
  falling under epsilon; for a critically damped spring this ignores the factor `1 + ωt`, and
  niri ends a 1,280-unit move about 2 physical pixels short at scale 1.5, then jumps.
  Hyprland ends on epsilons of position and velocity; KWin on one epsilon of 1 pixel for both.
- **Interrupted.** Hyprland's springs and SwiftUI keep the velocity; niri restarts the camera
  and the workspace switch at velocity 0 (its own `FIXME`s); Hyprland's Bézier curves restart
  from the current value over their full duration. Springs since Hyprland 0.55 (2026-05-09).
- **The scrolling view.** niri keeps one camera per workspace, relative to the active column,
  and gives each moved column an offset that decays to 0, so that logical positions jump and
  nothing on screen does; its workspace switch is one fractional index over the whole stack.
  Hyprland animates the leaving and the entering workspace each by its own offset or alpha.
- **Time.** niri samples every animation at the frame's predicted presentation, KWin at the
  view's next presentation, with a step capped at one 60 Hz period; libadwaita at GDK's frame
  time. Hyprland ticks every millisecond on the wall clock.
- **Tests.** niri's layout runs on a frozen clock that tests advance by milliseconds, with linear
  1,000 ms animations so that positions are exact, and snapshots them; random sequences of
  operations check its invariants.
- **Pointer.** niri does not refresh the pointer's focus while a transition runs under it, and
  hit-tests the positions it draws.
- **Defaults.** niri: springs of ratio 1 and stiffness 800 for the view and windows, 1,000 for
  workspaces; a 150 ms fade to open and to close. Hyprland's example: a spring of ratio 0.785 for
  windows; a fade for workspaces.

## When a frame is drawn

Read in their sources on 2026-11-12, at the commits named; latencies are derived from the code,
not published. T is the refresh period.

- **niri** (`589bc406`, `src/niri.rs`, `src/backend/tty.rs`): an idle output draws a commit at
  once; a commit while a frame waits for vblank is drawn just after it. No prediction, no margin.
  Frame callbacks after drawing, at most one per surface per refresh cycle, "to prevent
  empty-damage commit busy loops". Its headless backend presents at once, refresh
  unknown.
- **sway** (`1652c54b`, `sway-output(5)`): `max_render_time off`, the default, composites right
  after the refresh, about 2T for an animating client; `max_render_time <ms>` composites that
  long before the next refresh, set by hand. wlroots' headless backend (`72956e71`) sends
  `frame` from a timer, 60 Hz by default.
- **KWin** (`ad9ba773`, `src/core/renderloop.cpp`, `renderjournal.cpp`,
  `src/backends/drm/drm_commit_thread.cpp`): composites at the next vblank minus the expected
  compositing time, capped at 2T: an exponential average of render times, each measured from
  the scheduled wake-up and at least 2 ms, plus twice a decaying peak of overshoot, plus a safety
  margin of the mode's vblank time and 1 ms, raised at once on a miss. After 100 idle refreshes it
  starts at once. Its latency setting was removed in 2023 for "one hardcoded algorithm". Frame
  callbacks after compositing; its virtual output sends `presented` from a timer at the frame's
  target.
- **Mutter** (`5bdc78fd`, `clutter/clutter/clutter-frame-clock.c`): dispatches at the next
  presentation minus its longest update time and margins (1 ms and up to T/4, raised on a miss),
  capped at 3T; after an idle period it dispatches at once, for "lowest average latency for
  sporadic user input".
- **gamescope** (`865a71c4`, `src/vblankmanager.cpp`): wakes at vblank minus a decaying peak of its
  draw time, which jumps up at once, and a red zone of 1.65 ms; under T.
- **Smithay** (`9ff2c5a4`) ships no scheduler; anvil repaints a fixed 0.6 T after each vblank.
- Mesa's Vulkan WSI ends `vkWaitForPresentKHR` when `wp_presentation_feedback.presented` arrives,
  whatever time it carries; its FIFO mode uses the `fifo-v1` and `commit-timing-v1` protocols
  where offered.

## Agents and MCP

Read on 2026-11-13 (modelcontextprotocol.io, the clients' documentation, crates.io, GitHub).

- **MCP 2026-07-28** (released 2026-07-28): stateless. No `initialize`; each request carries
  `io.modelcontextprotocol/protocolVersion` and the client's capabilities in `_meta`; servers
  answer `server/discover`; a server must not rely on earlier requests of the same connection;
  -32022 answers an unsupported revision. Revision 2025-11-25 and earlier open with `initialize`
  and `notifications/initialized`.
- **stdio**: the client starts the server; one JSON-RPC message per line; nothing but MCP on
  standard output; logs on standard error; the server exits when its input closes.
- **Clients**: Claude Code accepts stdio and HTTP, warns past 10,000 tokens of tool output and
  caps it at 25,000; Codex accepts stdio and Streamable HTTP, 60 s per call by default; OpenCode
  accepts a command or a URL and speaks the old revision by default. None documents Unix sockets.
- **Images**: `{"type": "image", "data": <base64>, "mimeType": "image/png"}`. Claude's API takes
  at most 5 MB of base64 and 8,000 pixels a side, and scales past 2,576 pixels on the long edge;
  the model answers in the coordinates of the image it sees.
- **rmcp** 3.10.0, the official Rust SDK: Apache-2.0, older contributions MIT; both revisions;
  tokio required. Other Rust crates (rust-mcp-sdk, pmcp, turbomcp) are smaller and use tokio too.
- **Prior art**: kwin-mcp (MIT) starts a private `kwin_wayland --virtual`, gives input through
  KWin's EIS interface and captures through ScreenShot2, over stdio, screenshots as files by
  default; computer-use-linux (MIT, Rust) speaks stdio or loopback HTTP with a token; swampcy
  drives a headless sway; computer-use-mcp (Rust, rmcp) a nested sway. niri's IPC is JSON lines
  inside calloop. gamescope makes itself subreaper to end a program's whole tree.
- **Seats**: upstream Smithay announces each `wl_seat` global to every client, with no filter per
  client; GLFW keeps the first seat it sees, SDL2 one input for all seats, vkcube the first
  pointer.

## An own base and Vulkan

Read on 2026-11-13 for the design study of [ADR-0039](adr/0039-leaving-smithay-step-by-step.md)
to [ADR-0042](adr/0042-main-on-smithays-fork.md).

- **jay** ([github.com/mahkoh/jay](https://github.com/mahkoh/jay)): first commit 2022-01-02;
  about 60,000 hand-written lines and its author its only user by 2022-05-31 (issue 37); 1.0 on
  2024-04-07; 3,218 commits, 97 % by one author. Its own wire protocol, generated from text
  descriptions (`wire/`, `build/wire.rs`); its own io_uring loop; its own DRM ioctls; FFI to
  libgbm and libinput; its own XKB (`kbvm`); logind over its own D-Bus client; its own X11 window
  manager. Its Vulkan renderer (about 9,800 lines, Vulkan 1.3) is the default, OpenGL kept for
  older hardware; colour works only on Vulkan. Its open defects are mostly drivers': NVIDIA,
  colour pipelines, atomic commits, `VK_ERROR_DEVICE_LOST`. In 2022 its author refused wayland-rs
  because no compositor in daily use was built on it.
- **Bases**: Mutter, KWin, wlroots, Weston, Mir, Louvre and Hyprland write their own handlers and
  link libwayland-server. Hyprland replaced wlroots in 2024 with its own backend, aquamarine, and
  kept libwayland ([blog.vaxry.net](https://blog.vaxry.net/articles/2024-wlrootsRewrite)). No
  other Rust compositor without Smithay was found in daily use.
- **wayland-rs** ([github.com/Smithay/wayland-rs](https://github.com/Smithay/wayland-rs)):
  wayland-server filters globals per client (`GlobalDispatch::can_view`); wayland-backend bounds a
  client's buffer at 4,096 bytes and its file descriptors at 28 a message, and tests each case
  against libwayland. A message of more than four arguments allocates when dispatched (pull
  request 985). No measured benchmark of its dispatch was found. No `AI.md` in wayland-rs,
  calloop, drm-rs or gbm.rs; Smithay's covers contributions to Smithay.
- **Smithay**: its Vulkan renderer is draft pull request 2165 (opened 2026-09-09, "very early");
  its author on colour (pull request 982, 2026-05-12): "I am not sure I'll bother implementing
  HDR support for the GL renderer". CI runs four groups of WLCS.
- **Vulkan in compositors**: HDR through Vulkan in production in jay and gamescope only. wlroots
  tries GLES2 before Vulkan, does colour only on Vulkan, and has HDR banding there (issue 4131).
  KWin does HDR in OpenGL; its Vulkan renderer is planned (issue 169, open since 2023). Mutter and
  Hyprland have none. No published figure compares a compositor's frame cost, latency or power in
  GL and Vulkan; one user report (wlroots issue 4035, Intel) has Vulkan using more power.
- **Drivers** (Mesa 26.3.0 source): RADV, ANV and lavapipe offer dmabuf import, DRM format
  modifiers, foreign queues, external semaphores and fences, and timeline semaphores; lavapipe
  offers modifiers only with `/dev/udmabuf`. RADV reports Vulkan 1.4 on GFX9, the laptop's Vega 3,
  without `host_image_copy`. jay says modifiers fail on AMD before RX 5000; the kernel lists
  modifiers for Raven. The laptop itself is unread.
- **Bindings**: `ash` 0.38 has had no release since 2024-04-01; `jay-ash` regenerates its
  bindings every six hours; `vulkanalia` is maintained; `shaderc-rs` is archived; `naga` is pure
  Rust.

## Wayland protocols: the rivals and the clients

Read on 2026-11-14 from wayland.app's compositor tables and the compositors' own sources: niri
26.04, KWin 6.8.3, Hyprland 0.57.2, Sway 1.12, Mutter 51 (Mutter from wayland.app and its `main`
file list only). `Y` offered, `Yn` at version n, `N` not found, `?` unknown, `c` behind an option,
`*` from wayland.app alone. Private protocols of KDE, Hyprland and GTK, and experimental `xx-`
drafts, are left out. [ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)
makes `docs/protocols.md` the list Yuna keeps from this.

| Protocol | Status | What it does | niri | KWin | Hyprland | Sway | Mutter |
|---|---|---|---|---|---|---|---|
| wayland | core | Core: compositor, shm, seat, output, data device, subcompositor, fixes | Y | Y | Y | Y | Y* |
| linux-dmabuf-v1 | stable | Share GPU buffers with clients, with format feedback | Y | Y5 | Y5 | Y | Y5* |
| presentation-time | stable | Tell clients when a frame was shown | Y | Y2 | Y2 | Y | Y2* |
| tablet-v2 | stable | Drawing tablets | Y | Y2 | Y1 | Y | Y2* |
| viewporter | stable | Crop and scale a surface | Y | Y1 | Y1 | Y | Y1* |
| xdg-shell | stable | Application windows: toplevels and popups | Y | Y6 | Y7 | Y | Y7* |
| alpha-modifier-v1 | staging | Constant opacity for a surface | N | Y1 | Y1 | Y | N* |
| color-management-v1 | staging | Colour spaces, HDR, image descriptions | N | Y2 | Y3c | Y2 | Y2* |
| color-representation-v1 | staging | Pixel format hints: matrix, range, alpha mode | N | Y1 | N | Y | Y1* |
| commit-timing-v1 | staging | Present a commit at a chosen time | N | Yc | Yc | N | Y1* |
| content-type-v1 | staging | Hint: video, game, photo | N | Y1 | Y1 | Y1 | N* |
| cursor-shape-v1 | staging | Client names a cursor shape; compositor draws it | Y | Y2 | Y2 | Y2 | Y2* |
| drm-lease-v1 | staging | Lend a display to a VR client | Y | Y1 | Y1 | Y | Y1* |
| ext-background-effect-v1 | staging | Ask for blur behind a surface | Y | Y1 | Y1 | N | Y1* |
| ext-data-control-v1 | staging | Clipboard managers (read and set selections) | Y | Y1 | Y1 | Y1 | N* |
| ext-foreign-toplevel-list-v1 | staging | List windows for taskbars (read only) | Y1* | N | Y1 | Y | N* |
| ext-idle-notify-v1 | staging | Tell a client when the user is idle | Y | Y2 | Y2 | Y | N* |
| ext-image-capture-source-v1 | staging | Pick what to capture: output or window | N (main: Y) | N | Y | Y1 | N* |
| ext-image-copy-capture-v1 | staging | Copy a captured source into client buffers | N (main: Y) | N | Y1 | Y1 | N* |
| ext-session-lock-v1 | staging | Screen lockers | Y | N | Y1 | Y | N* |
| ext-transient-seat-v1 | staging | Short-lived seats for remote input | N | N | N | Y | N* |
| ext-workspace-v1 | staging | List and switch workspaces | Y | N | Y1 | N | N* |
| fifo-v1 | staging | Frame queue: a commit waits for the next refresh (vsync) | N | Y1 | Y1 | N | Y1* |
| fractional-scale-v1 | staging | Tell a surface its fractional scale | Y | Y1 | Y1 | Y1 | Y1* |
| linux-drm-syncobj-v1 | staging | Explicit GPU sync with timelines | N | Y1 | Y1 | Y1 | Y1* |
| pointer-warp-v1 | staging | Client moves the pointer | N | Y1 | Y1 | N | Y1* |
| security-context-v1 | staging | Sandboxed clients get a restricted socket | Y | Y1 | Y1 | Y | N* |
| single-pixel-buffer-v1 | staging | One-colour buffers without memory | Y | Y1 | Y1 | Y | Y1* |
| tearing-control-v1 | staging | Allow tearing (async present) for a surface | N | Y1 | Y1 | Y1 | N* |
| xdg-activation-v1 | staging | Ask to focus a window with a token | Y | Y1 | Y1 | Y | Y1* |
| xdg-dialog-v1 | staging | Mark a toplevel as a modal dialog | N | Y1 | Y1 | N | Y1* |
| xdg-system-bell-v1 | staging | System bell | N | Y1 | Y1 | N | Y1* |
| xdg-toplevel-drag-v1 | staging | Drag a whole toplevel (tab tear-off) | N | Y1 | N | N | Y1* |
| xdg-toplevel-icon-v1 | staging | Window icons | N | Y1 | N | N | N* |
| xdg-toplevel-tag-v1 | staging | Tag a window so placement is remembered | N | Y1 | Y1 | Y1 | Y1* |
| xwayland-shell-v1 | staging | Pair X11 windows with surfaces (Xwayland) | N | Y | Y1 | ? | N* |
| fullscreen-shell-unstable-v1 | unstable | Fullscreen shell for kiosks | N | N | N | N | ? |
| idle-inhibit-unstable-v1 | unstable | Stop idle and screen blanking | Y | Y1 | Y1 | Y | Y1* |
| input-method-unstable-v1 | unstable | Old input methods and input panel | N | Y | N | N | N* |
| input-timestamps-unstable-v1 | unstable | High-resolution input timestamps | N | N | N | N | N* |
| keyboard-shortcuts-inhibit-unstable-v1 | unstable | Let a client take the compositor shortcuts | Y | Y1 | Y1 | Y | Y1* |
| linux-explicit-synchronization-unstable-v1 | unstable | Old explicit sync with fences (replaced) | N | N | N | N | N* |
| pointer-constraints-unstable-v1 | unstable | Lock or confine the pointer | Y | Y1 | Y1 | Y | Y1* |
| pointer-gestures-unstable-v1 | unstable | Swipe, pinch and hold gestures | Y | Y3 | Y3 | Y | Y3* |
| primary-selection-unstable-v1 | unstable | Middle-click paste selection | Y | Y1 | Y1 | Y | Y1* |
| relative-pointer-unstable-v1 | unstable | Unaccelerated relative motion | Y | Y1 | Y1 | Y | Y1* |
| text-input-unstable-v3 | unstable | Text input from IME to client | Y | Y2 | Y1 | Y | Y2* |
| xdg-decoration-unstable-v1 | unstable | Server-side or client-side title bars | Y | Y2 | Y2 | Y | N* |
| xdg-foreign-unstable-v2 | unstable | Parent a window to another process's window | Y | Y | Y | Y | Y* |
| xdg-output-unstable-v1 | unstable | Logical output position, size and name | Y | Y3 | Y3 | Y | Y3* |
| xwayland-keyboard-grab-unstable-v1 | unstable | X11 keyboard grabs under Xwayland | N | Y | N | ? | Y (main) |
| input-method-unstable-v2 | unstable | Input methods (IME) | Y | N | Y1 | Y | N* |
| virtual-keyboard-unstable-v1 | unstable | Fake keyboard input | Y | N | Y1 | Y | N* |
| wayland-drm | legacy Mesa | Old Mesa buffer sharing (replaced by dmabuf) | Y2* | N | Y2 | N | N* |
| wlr-data-control-unstable-v1 | wlr | Clipboard managers (wlroots version) | Y | N | Y2 | Y | N* |
| wlr-export-dmabuf-unstable-v1 | wlr | Old screen capture as dmabuf | N | N | N | Y | N* |
| wlr-foreign-toplevel-management-unstable-v1 | wlr | List and control windows (taskbars) | Y | N | Y3 | Y | N* |
| wlr-gamma-control-unstable-v1 | wlr | Set output gamma tables (night light) | Y | N | Y1 | Y1 | N* |
| wlr-input-inhibitor-unstable-v1 | wlr | Grab all input (old lockers) | N | N | N | N | ? |
| wlr-layer-shell-unstable-v1 | wlr | Panels, wallpapers, overlays | Y | Y5 | Y5 | Y | N* |
| wlr-output-management-unstable-v1 | wlr | Configure outputs (modes, scale, position) | Y | N | Y4 | Y | N* |
| wlr-output-power-management-unstable-v1 | wlr | Turn outputs on and off (DPMS) | N | N | Y1 | Y | N* |
| wlr-screencopy-unstable-v1 | wlr | Screenshots and screen capture | Y | N | Y3 | Y | N* |
| wlr-virtual-pointer-unstable-v1 | wlr | Fake pointer input | Y | N | Y2 | Y | N* |
| xdg-session-management-v1 | staging | Save and restore app windows between sessions | N | Y | N | N | Y (main) |
| text-input-unstable-v1 | unstable | Old text input (legacy) | N | Y | Y1 | N | ? |
| xdg-foreign-unstable-v1 | unstable | Old xdg-foreign (replaced by v2) | N | N | N | Y | Y (main) |

What clients bind, read in their sources on 2026-11-14:

- **Mesa's Vulkan WSI** (`wsi_common_wayland.c`, main): `linux-drm-syncobj`, presentation time,
  `tearing-control`, `fifo-v1`, `commit-timing-v1` (with timestamps on), `color-management`,
  `linux-dmabuf` v3 or later. Every Vulkan client through Mesa, DXVK and VKD3D included.
- **SDL3** (`SDL_waylandvideo.c`, main): relative pointer, pointer constraints, keyboard shortcuts
  inhibit, idle inhibit, activation, fractional scale, cursor shape, and others. Whether it sends
  `content-type` was not settled.
- **Wine's `winewayland.drv`** (master): pointer constraints, relative pointer, `wlr-data-control`
  (not the `ext-` one), text input v3, toplevel icon, pointer warp, colour management. Whether
  Proton uses it by default is unknown.
- **Xwayland** (master): `xwayland-shell` (rootless), `tearing-control`, `drm-lease`, relative
  pointer, pointer constraints, `xwayland-keyboard-grab`, keyboard shortcuts inhibit,
  `linux-drm-syncobj`.
- **Flatpak** (main): `security-context-v1`, when built with it.
- **Chromium**, and so Electron applications such as Discord (not read): session management, idle
  notify and inhibit, `content-type`, pointer constraints, relative pointer, `xdg-foreign`,
  toplevel drag, `linux-drm-syncobj`, colour management, and others.
- **Firefox** (main): idle inhibit, pointer constraints, activation, colour management and
  representation, session management.
- **mpv** (master): `content-type`, `single-pixel-buffer`, presentation time, colour management,
  idle inhibit.
- **OBS** captures through the ScreenCast portal and PipeWire, not a Wayland protocol.
- Steam's client and Discord's own code were not read.
