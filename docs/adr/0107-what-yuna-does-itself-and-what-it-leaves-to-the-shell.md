# 0107. What Yuna does itself, and what it leaves to the shell

- Status: Accepted; supersedes [0004](0004-the-compositor-alone-under-a-shell.md),
  [0057](0057-what-yuna-does-and-what-the-shell-does.md) (but for idle's lock, the theme file and
  the variants), [0065](0065-what-hollow-asks-of-yuna.md) (the capture event, the greeter mode);
  supersedes in part [0064](0064-what-the-best-desktops-taught-yuna.md) (the shell's interface,
  window previews, brightness, the list of casts)
- Date: 2026-11-18

## Context

The line between Yuna and the shell above it was drawn in five ADRs.
[ADR-0004](0004-the-compositor-alone-under-a-shell.md) made Yuna a compositor alone, speaking the
protocols any shell needs. [ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md) gave Yuna
the wallpaper, idle, captures, portals, the theme and the fullscreen state.
[ADR-0063](0063-vulkan-alone-and-uwui-for-what-yuna-draws.md) put Yuna's interfaces on uwui and
named Hollow, the owner's shell. [ADR-0064](0064-what-the-best-desktops-taught-yuna.md) made
brightness Yuna's and gave the shell an interface of its own, and
[ADR-0065](0065-what-hollow-asks-of-yuna.md) added the capture event and the greeter mode. The
statuses of ADR-0004 and ADR-0057 still named ADR-0063, superseded since.

ADR-0004 had Yuna's own parts written without uwui, and the owner's own shell decided after uwui's
first stage; ADR-0063 reversed both, and its rule is the one below.

This ADR gathers that rule as it stands on 2026-11-18. It decides nothing new. The lock has its
own ADR, [ADR-0108](0108-the-lock-begun-by-yuna-a-dead-or-frozen-locker-relieved.md), and the
theme file and the variants are the configuration's,
[ADR-0129](0129-configuration-in-kdl-its-includes-and-variants.md).

## Decision

- **Yuna is a compositor; a shell runs on top** (ADR-0004, 2026-11-11). Yuna speaks the standard
  protocols a shell needs (`wlr-layer-shell`, `ext-session-lock-v1`, `ext-workspace-v1`,
  `ext-foreign-toplevel-list-v1`, foreign toplevel management, image capture), so that Amane or
  any other shell runs on top without a patch.
- **Hollow is the owner's shell**: public, on uwui, on Yuna alone, optional above it (ADR-0063,
  2026-11-14). Yuna gives it a documented interface of commands and events: workspaces, focus,
  outputs, the primary output, the fullscreen state, urgent windows, the bell, previews of
  windows (ADR-0064, 2026-11-14), and who captures (below). It is specified with Hollow's design
  and versioned as a public contract
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 13).
- **Yuna draws only what must be inside a compositor** (ADR-0004), with uwui
  ([ADR-0106](0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md)): the overview,
  transitions, borders, effects, Alt-Tab, the wallpaper, the captures' selection and toolbar, the
  recording indicator, the share picker, and its own messages (ADR-0057, 2026-11-14; ADR-0064).
- **The wallpaper is Yuna's** (ADR-0057). Yuna draws it, rotates it on its own timer, and tells
  the shell which one is shown. The shell keeps its wallpaper picker, which asks Yuna to show one.
- **Idle is Yuna's, whole** (ADR-0057). Its delays are in the configuration. Yuna dims the outputs
  (the backlight where there is one, its own colour pipeline on a panel without one), turns them
  off, and locks the session after a delay and before sleep, as ADR-0108 says. `idle-inhibit`,
  the Inhibit portal and a fullscreen surface that draws keep it awake. Yuna offers
  `ext-idle-notify-v1` to other clients.
- **Brightness is Yuna's** (ADR-0064): the laptop's backlight and external displays through
  DDC/CI, by a command. The shell shows the slider and its display.
- **Captures are Yuna's; video leaves Yuna through PipeWire** (ADR-0057). Yuna freezes the
  screen, draws the selection (a region, a window, an output) and its toolbar, writes the image to
  the clipboard and a file, then sends a standard notification. For video, Yuna draws the
  selection, the recording indicator and its stop button, and streams the frames through
  PipeWire; an external recorder, started by Yuna, encodes them on the GPU and records the sound.
  Consent before a capture is given through the picker. How a capture is made is
  [ADR-0121](0121-captures-what-is-seen-at-the-logical-size.md)'s.
- **Yuna tells the shell who captures** (ADR-0065, 2026-11-14): an event names each program
  capturing or casting an output, a region or a window, when it starts and stops, so that Hollow
  shows a "screen shared" indicator. The list of casts with their outlines and a stop command
  stays declined (ADR-0064).
- **Portals that need the compositor live in `yuna-portal`** (ADR-0057), a program of Yuna's
  repository: ScreenCast, Screenshot, GlobalShortcuts, Inhibit and Wallpaper. It takes frames from
  Yuna through the standard capture protocol and runs PipeWire and D-Bus outside Yuna's process.
  Settings and Notification belong to the shell; Secret, FileChooser and the others to the
  session's usual services.
- **Yuna announces what is fullscreen; the shell decides** (ADR-0057). Yuna publishes a state, a
  game, a film, plain fullscreen or none, on its event stream. The shell chooses what follows,
  such as holding notifications back during a game.
- **The login screen runs on Yuna, in a greeter mode** (ADR-0065). Under greetd, Yuna runs as the
  greeter's compositor: no command socket, no MCP, no global shortcuts, no portals, no XWayland;
  switching virtual terminals kept. The session's Yuna then reuses the active mode, so that the
  hand-over shows no flicker.
- **What stays the shell's** (ADR-0057): the bar, launchers, notifications, the tray, popups and
  the control centre, the volume and brightness displays, the lock screen's interface, the login
  screen, and theming the other applications.
- **RemoteDesktop and InputCapture wait** for a concrete use, in the PLAN's ideas (ADR-0057).
- **Each part comes whole at its stage; no stand-in protocol is built for a tool Yuna replaces**
  (ADR-0057):
  - Stage 2: idle, brightness, the wallpaper without blur, the theme file and variants, the
    fullscreen state (plain fullscreen only), `yuna-portal` and the share picker, captures and
    recording, the capture event, the greeter mode, and a night light in SDR through Yuna's
    colour matrix (ADR-0057; ADR-0064; ADR-0065).
  - Stage 4: the fullscreen state tells games apart.
  - Stage 5: the night light, captures and sharing correct under HDR; the state tells films apart.
  - Stage 6: blur and glass over the wallpaper, its animated transitions, the colour picker
    ([ADR-0061](0061-yunas-uses.md) point 10), the capture tool's polish.

## Alternatives considered

- **A compositor and its own shell inside** ([ADR-0004](0004-the-compositor-alone-under-a-shell.md)):
  a second large project, and a lock screen whose security becomes Yuna's.
- **The wallpaper in a separate daemon on `wlr-layer-shell`**
  ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md)): blur would read a client's surface
  every frame, and one more client holds GPU memory.
- **Idle left to `swayidle`** ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md)): it
  cannot see a game that does not ask, nor dim a panel without a backlight.
- **Recording encoded inside Yuna, or left to the shell**
  ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md)): PipeWire and an encoder in the
  session's process, or two selection interfaces and video wrong under HDR.
- **Portals inside Yuna, or an existing portal**
  ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md)): D-Bus could stall a frame, or a
  picker in another desktop's look with nothing made for HDR.
- **Yuna hiding the shell's surfaces over a game itself**
  ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md)): the shell would not know its
  notification was not seen.
- **The current tools kept through stand-in protocols**
  ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md)): protocols built to be dropped,
  wrong under HDR.
- **The shell owning brightness** ([ADR-0064](0064-what-the-best-desktops-taught-yuna.md)): the
  other option of its point 10.
- **The list of casts with their outlines and a stop command**
  ([ADR-0064](0064-what-the-best-desktops-taught-yuna.md)): declined by the owner.
- **The greeter on another compositor** ([ADR-0065](0065-what-hollow-asks-of-yuna.md)): two
  compositors in the session and a flicker at each hand-over.

## Consequences

- Stage 2 carries most of the parts above, beside Yuna's own core.
- The fullscreen state and Hollow's interface are public contracts, with their versions.
- A program that captures cannot hide from the shell's indicator; the windows never captured
  stay out of what it receives.
- Amane loses its screenshot tool, its patch for Umbriel's workspaces (replaced by
  `ext-workspace-v1`) and its parsing of `awww`. `swayidle`, `awww`, `grim`, `wf-recorder`,
  `wlsunset` and `hyprpicker` leave the owner's session as their stages land.
- `yuna-portal`'s D-Bus and PipeWire libraries, and the recorder Yuna starts, are put to the
  owner when their milestone needs them.

## Validation and reconsideration

Reviewed reasoning, not measured: the readings of Amane, Umbriel and the session on 2026-11-14,
and the surveys of other desktops that ADR-0064 rests on. Proposed again if Hollow's design asks
of Yuna what this division does not give, or if a shell other than Hollow needs what only
Hollow's interface offers.
