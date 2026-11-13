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
- amdgpu doubles frames below the panel's minimum (BTR, `freesync.c`); KMS exposes only a
  `VRR_ENABLED` boolean.

## Games

- No desktop compositor upscales games; gamescope offers FSR, NIS and others, a frame limiter and
  HDR.
- Tearing control: KWin, Hyprland, sway, jay; not niri or COSMIC. Games are recognised by the
  `content-type-v1` hint (Hyprland, KWin, Umbriel).
- XWayland at fractional scale: KWin lets applications scale themselves; Hyprland's
  `force_zero_scaling`; Umbriel's native resolution; jay's downscaled mode.
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

- niri: each animation a spring (damping ratio, stiffness, epsilon) or a duration and a curve; a
  restarted animation starts from the current value.
- Hyprland: Bézier curves, and springs since 0.57; an interrupted spring keeps its velocity.
- KWin: animations retargeted while running; springs in some effects since 2026-07.

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
