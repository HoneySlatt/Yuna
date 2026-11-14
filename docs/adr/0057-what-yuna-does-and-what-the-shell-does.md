# 0057. What Yuna does itself, and what it leaves to the shell

- Status: Accepted; amends 0004; amended by 0063, 0064 (brightness is Yuna's)
- Date: 2026-11-14

## Context

[ADR-0004](0004-the-compositor-alone-under-a-shell.md) makes Yuna a compositor alone, with a shell
on top. It names what Yuna draws: the overview, transitions, borders, effects, the screenshot tool
and Alt-Tab. It does not say where the rest of a desktop goes.

The owner's shell today is Amane on Umbriel. Read on 2026-11-14, it covers the bar, launchers,
notifications, the tray, a control centre, popups for sound, network and power, the lock and
login screens, and the on-screen displays for volume and brightness. Beside it, the session runs
tools that touch the compositor:

- workspaces read through a patch to Amane that speaks Umbriel's own socket;
- screenshots through `grim`, a dimmed layer surface, `magick` and `wl-copy`; recording through
  `wf-recorder`, encoded on the CPU, with the sound of the default output;
- the wallpaper through the `awww` daemon, rotated by a shell loop; the lock and login screens
  find the current wallpaper by parsing `awww query`;
- idle through `swayidle`: the backlight dimmed, outputs turned off by a compositor command, the
  lock before sleep; it knows a game or a film only if the program says so;
- a night light through `wlsunset`, a colour picker through `hyprpicker`;
- the theme script writes Umbriel's colour file; the bar's compact mode rewrites Umbriel's
  configuration;
- portals: Umbriel ships a separate portal for screen sharing, screenshots and input capture;
  the session adds GTK's file chooser and GNOME's keyring for secrets.

Nothing in Yuna's roadmap covered portals or screen sharing, which the owner uses every day.

## Decision

The owner chose, on 2026-11-14, in this order:

1. **The wallpaper is Yuna's.** Yuna draws it, rotates it on its own timer, and tells the shell
   which one is shown. The shell keeps its wallpaper picker, which asks Yuna to show one.
2. **Idle is Yuna's, whole.** Its delays are in the configuration. Yuna dims the outputs (the
   backlight where there is one, its own colour pipeline on a panel without one), turns them off,
   and runs the shell's lock command after a delay and before sleep, holding sleep back long
   enough to lock. `idle-inhibit`, the Inhibit portal and a fullscreen surface that draws keep it
   awake. Yuna also offers `ext-idle-notify-v1` to other clients.
3. **Captures are Yuna's; video leaves Yuna through PipeWire.** Yuna freezes the screen, draws the
   selection (a region, a window, an output) and its toolbar, and writes the image to the
   clipboard and a file, then sends a standard notification. For video, Yuna draws the selection,
   the recording indicator and its stop button, and streams the frames through PipeWire; an
   external recorder, started by Yuna, encodes them on the GPU and records the sound.
4. **Portals that need the compositor live in `yuna-portal`**, a program of its own in Yuna's
   repository: ScreenCast, Screenshot, GlobalShortcuts, Inhibit and Wallpaper. It takes frames
   from Yuna through the standard capture protocol, and runs PipeWire and D-Bus outside Yuna's
   process. Yuna draws the share picker, the same selection as for captures. Settings and
   Notification belong to the shell; Secret, FileChooser and the others to the session's usual
   services.
5. **The theme is a file Yuna includes; variants switch on command.** Yuna's configuration
   includes a colours-only file that a shell or a theme tool writes, reloaded live; Yuna's own
   interfaces take their colours from it. Its format is a public interface, documented and
   versioned. A *variant* is a named set of settings in the configuration, applied over it and
   switched by a Yuna command; the shell's compact mode is one. Nothing outside Yuna rewrites
   its configuration.
6. **Yuna announces what is fullscreen; the shell decides.** Yuna publishes a state, a game, a
   film, plain fullscreen or none, on its event stream. The shell chooses what follows, such as
   holding notifications back during a game.
7. **RemoteDesktop and InputCapture wait** for a concrete use. They go to the PLAN's ideas.
8. **Each part comes whole at Stage 2, finished at its own stage.** No stand-in protocol is built
   for a tool Yuna replaces:
   - Stage 2: idle; the wallpaper without blur; the theme file and variants; the fullscreen state,
     plain fullscreen only; `yuna-portal` and the share picker; captures and recording; a night
     light in SDR, through Yuna's colour matrix.
   - Stage 4: the state tells games apart.
   - Stage 5: night light, captures and sharing correct under HDR; the state tells films apart.
   - Stage 6: blur and glass over the wallpaper, its animated transitions, the colour picker,
     the capture tool's visual polish.

What stays the shell's: the bar, launchers, notifications, the tray, popups and control centre,
the volume and brightness displays, the lock screen's interface, the login screen, and theming
the other applications.

## Alternatives considered

- **The wallpaper in a separate daemon on `wlr-layer-shell`**: no code in Yuna and any tool works,
  but blur would read a client's surface every frame instead of blurring a still image once, and
  the daemon is one more client holding GPU memory.
- **Idle left to `swayidle` over standard protocols**: the known path, but it cannot see a game
  that does not ask, nor dim a panel without a backlight. Idle **without dimming** was simpler and
  lost the soft step before the outputs go off.
- **Recording encoded inside Yuna**, sound included: one program, but PipeWire's sound and a video
  encoder's libraries in the process the session depends on. **Recording left to the shell**: two
  selection interfaces, and video wrong under HDR.
- **Portals inside Yuna's process**: one program, but D-Bus and PipeWire code that could stall a
  frame or take the session down ([ADR-0007](0007-surviving-a-crash.md)). **An existing portal**
  (wlroots' generic one, or GNOME's through Mutter's interfaces, as niri does): the least code,
  but a picker in another desktop's look, missing portals depending on the one chosen, and nothing
  made for HDR.
- **Theme and compact mode sent by command only**: no file, but lost when Yuna restarts until the
  shell sends them again, with two sources of truth. **Themes managed by Yuna**: a theme spans
  dozens of applications and has no place in a compositor. A command that sets any key until the
  next reload, as `hyprctl keyword` does, is kept as an idea, with no use yet.
- **Yuna hides the shell's surfaces over a game itself**: works with any shell, but the shell does
  not know its notification was not seen. **The shell infers from the fullscreen state** of
  standard protocols: nothing in Yuna, but a game and a film look the same.
- **RemoteDesktop and InputCapture at Stage 2**: a complete desktop, but security-sensitive code
  with no user yet.
- **The current tools kept through stand-in protocols until Yuna's own**: a smaller Stage 2, but
  protocols built to be dropped (`wlr-gamma-control` cannot be correct under HDR), and captures,
  night light and sharing wrong under HDR until Stages 5 and 6. **Everything finished at Stage 2**,
  effects included: Stage 2 would carry Stage 5's and 6's work.

## Consequences

- Stage 2 grows: beside Yuna's own core and its DRM backend
  ([ADR-0039](0039-leaving-smithay-step-by-step.md)), it now carries idle, the wallpaper, the theme
  file and variants, the fullscreen state, `yuna-portal`, the picker, captures, recording and an SDR
  night light. Leaving Umbriel comes later for it.
- Yuna draws a small interface from Stage 2 (the picker and the capture toolbar), which likely needs
  text or icon rendering. Its dependency, like D-Bus's and PipeWire's for `yuna-portal`, is put to
  the owner when its milestone needs it. The recorder Yuna starts is chosen then too.
- ADR-0004's screenshot tool now includes recording's selection, but not its encoding: the
  roadmap's GPU-encoded recording is the external recorder's. Consent before capture moves to
  Stage 2, given through the picker.
- The theme file's format and the fullscreen state are public interfaces, with their versions.
- Amane loses its screenshot tool, its patch for Umbriel's workspaces
  ([ADR-0004](0004-the-compositor-alone-under-a-shell.md)) and its `awww` parsing; it gains a
  handler for the fullscreen state, if the owner wants one.
- `swayidle`, `awww`, `grim`, `wf-recorder`, `wlsunset` and `hyprpicker` leave the owner's session
  as their stages land.
