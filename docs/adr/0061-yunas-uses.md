# 0061. Yuna's uses: creation, capture, games, HDR per display, development

- Status: Accepted; amends 0007, 0010, 0058
- Date: 2026-11-14

## Context

On 2026-11-14 the owner listed what Yuna must serve: development, by hand and by agents; office
work and the web; games; content creation (live streaming, recording); artistic creation (music,
video, photos, drawing, games); films and series; and the compositor's own look, comfort and
performance. The owner's programs, read from the NixOS configuration and the owner's answers:

- creation: Kdenlive, GIMP, Inkscape, Blender, Krita or Clip Studio under Wine, DaVinci Resolve,
  music software and Windows plugins under Wine, Magnolia, Unreal or Unity, SlimeEngine; no
  graphics tablet, none planned;
- content: OBS for live streams on Twitch and YouTube, game recordings, Discord's screen sharing;
- games: Steam, Heroic, PCSX2, RPCS3, Dolphin, gamemode;
- films: mpv, Jellyfin, the browsers;
- development: Zed, VS Code, Neovim, Brave, Firefox, Chromium, Podman, Docker, QEMU with
  virt-manager.

The roadmap named no rival for creation or for capture. It put FSR upscaling inside the
compositor at Stage 4, an HDR tone mapping that starts from the monitor's EDID alone, the colour
picker at Stage 6, a window's restore after a crash only, and every effect off in game mode
([ADR-0010](0010-animations-and-effects.md)). An agent's MCP server ([ADR-0033](0033-the-mcp-server.md))
gives seats, outputs, input, captures, presentation times and the clock, and nothing to read a
program's protocol or log.

## Decision

The owner chose, on 2026-11-14:

1. **Creation is a criterion of the bar, against KWin**: the colour shown against the colour
   asked, measured; a graphics tablet's latency, pressure and tilt, measured once a tablet is at
   hand. `tablet-v2`, `pointer-warp` and pointer constraints come with Stage 2 through
   [ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)'s rule. Windows of Wine and of
   plugins float, stay above their parent and follow it, by rules and `xdg-foreign`.
2. **Capture and streaming are a criterion of the bar, against gamescope and KWin**: a capture
   costs a game no frame and keeps its direct scanout; frames leave on the GPU. A capture of HDR
   content keeps it HDR or converts it to SDR, chosen at each capture (Stage 5). A rule marks a
   window **never captured**: every capture, by any tool, shows a flat area in its place (Stage 2).
3. **FSR leaves the compositor.** Stage 4 checks that gamescope runs well nested in Yuna, for the
   games that need its scaling, and lets the display hardware scale a smaller game where it can.
   No inverse tone mapping of SDR games, no performance overlay drawn by Yuna (MangoHud serves),
   no guard of a fullscreen game's focus.
4. **HDR follows a profile per display**: the EDID first, corrected by a guided adjustment by eye
   (peak, black, saturation) or by a probe, kept per display by its serial number; the tone mapping
   aims at the profile (Stage 5).
5. **A test output in a window** (Stage 2): a virtual output of the size, scale, HDR and VRR asked,
   shown in a window of the desktop and driven by the owner's mouse and keyboard; the same window
   shows an agent's headless Yuna, to watch it.
6. **Picture-in-picture windows** (a video detached from a browser, a mini player) float above
   the others on every workspace, recognised where a client says so, by a rule otherwise
   (Stage 2).
7. **The agents' toolbox**, a milestone of Stage 1: each program's Wayland protocol trace and its
   log through MCP; the accessibility tree of a seat's windows through MCP, its dependency put to
   the owner; situations on demand (outputs plugged and unplugged, scale and mode changed, a slow
   or stuck client, keyboard layouts, the clipboard and drag and drop between programs); a call that
   waits for an event instead of a loop that asks; budgets for `yuna mcp`'s start, its memory and
   the instances that run together. The loss of the GPU on demand comes with Stage 2's recovery.
   VKMS, the kernel's virtual display, is studied at Stage 2's start, so that agents test DRM and
   KMS code without a monitor.
8. **Windows come back in place after a reboot**, not only after a crash: each window's workspace,
   output and column, for applications that speak `xdg-session-management`. A setting, on by
   default (Stage 2).
9. **Effects stay on in games by default**: settings choose what a game turns off (blur, glass,
   animations, each on its own).
10. **The colour picker stays at Stage 6.**

## Alternatives considered

- **Creation without a criterion**, only its protocols listed: nothing would measure the colour.
- **Capture without a budget**: a live stream could cost the game frames unseen.
- **FSR in the compositor**: it serves games without an upscaler of their own and small machines,
  but recent games' own upscalers are better and keep their interface sharp; the game must be lied
  to about the screen's size; it breaks direct scanout. **Inverse tone mapping, a performance
  overlay, a focus guard**: declined by the owner.
- **HDR from the EDID alone**, or **the EDID with values corrected by hand** as KWin does: EDIDs
  are often wrong or incomplete, and a value by hand needs the owner to know it.
- **The colour picker at Stage 2**: declined by the owner; `hyprpicker`'s support of Yuna's
  capture protocol is not checked.
- **Restore after a crash only** ([ADR-0007](0007-surviving-a-crash.md)): a reboot would lose the
  layout.
- **Every effect off in game mode** (ADR-0010): a fullscreen game is scanned out and Yuna draws
  nothing then, so turning effects off there changes nothing; the owner wants to choose.
- **VKMS not studied**: DRM code tested only by the owner by hand.

## Consequences

- The bar gains two references and their budgets: creation (colour, tablet) and capture.
- Stage 1 gains a milestone, the agents' toolbox, before its audit.
- Stage 2 gains: the creation protocols, never-captured windows, the test output, picture in
  picture, the restore after a reboot, the GPU's loss on demand, `keyboard-shortcuts-inhibit` (by
  ADR-0058's rule, since the owner uses it daily for virtual machines), VKMS's study.
- Stage 4 loses FSR; Stage 5 gains the profile per display and HDR captures.
- ADR-0058's tablet budget waits for a tablet. ADR-0060's idea of a window that watches an agent
  becomes part of the test output.
- The accessibility tree, the profile's guided adjustment and the test output each need a design
  of their own at their milestone.
