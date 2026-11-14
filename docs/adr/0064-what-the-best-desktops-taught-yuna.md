# 0064. What the best desktops taught Yuna

- Status: Accepted; amends 0019, 0057
- Date: 2026-11-14

## Context

On 2026-11-14 three surveys read what Windows 11 with PowerToys, macOS 27 and its window tools,
KDE Plasma 6.8, GNOME 50 and 51, COSMIC, Cinnamon, XFCE, Budgie, elementary, Deepin, Hyprland
0.57, niri 26.04, sway, i3, river, Wayfire, dwl, mango, Qtile, bspwm, xmonad, PaperWM and
gamescope do that Yuna's plan lacked; two more read what the shells of Linux, macOS and Windows
need from a compositor. Each fact is sourced in the readers' saved pages; the facts the
decisions below rest on are in their text. Yuna's own shell is Hollow
([ADR-0063](0063-vulkan-alone-and-uwui-for-what-yuna-draws.md)).

## Decision

The owner chose, on 2026-11-14.

**Taken without a choice, as the bar asks:** custom output modes and modelines (ADR-0014 needs
239.76 and 225 Hz); a start and a restart that reuse the active mode, with no black flash; a
short wait before an output unplugged and plugged back counts as gone; Yuna's command socket
readable by the owner alone (mode 0600); per-device input settings (acceleration, scrolling,
tapping, drag lock, touchpad off while typing or with a mouse); the reason a frame is not
scanned out or torn, beside the rest auditor; a shortcut that always ends a shortcuts inhibitor,
so a stuck client cannot hold the session; the cursor sent beside a cast as PipeWire metadata;
colour filters kept out of captures; a colour depth limit per output; hot corners, Super-drag
and gestures standing down over a fullscreen game; the screen shown when a locker dies; optional
configuration includes, with `~` expanded; window previews for the shell; a documented interface
of commands and events for Hollow (workspaces, focus, outputs, the primary output, the fullscreen
state, urgent windows, the bell).

**Chosen:**

1. **Focus is marked, not stolen.** A window that asks for the focus never takes it during a
   fullscreen game or while the owner types; it is marked urgent (a border, an event). A rule per
   application allows it (Stage 2).
2. **Super-drag** moves a window with the left button and resizes it from the nearest edge with
   the right, anywhere in it; a game keeps the key (Stage 2).
3. **Several windows fullscreen on one workspace**, as Steam Big Picture needs; while a game's
   fullscreen holds the focus, ADR-0019's departure still holds: nothing takes its screen
   (Stage 2).
4. **A frozen application is shown**, with a choice to wait or to end it, drawn by Yuna (Stage 2).
5. **A primary output**, with an order of fallbacks, told to the shell (Stage 2).
6. **Back to the previous workspace** by its own key, and workspaces walked from the most
   recent, as settings (Stage 2).
7. **The laptop's lid** turns its panel off, or sleeps when no other output is plugged (Stage 2).
8. **The cursor hidden** while typing and after a delay without motion, as settings (Stage 2).
9. **Custom gestures**, **shortcuts per application**, and **keys passed to X11 applications**
   for push-to-talk and OBS's hotkeys, off by default, since it lets them read keys (Stage 2).
10. **Brightness is Yuna's**: the laptop's backlight and external displays through DDC/CI, by a
    command; the shell shows the slider and its display (Stage 2).
11. **Sharing**: the target changed while a cast runs, starting blank; the never-captured rule on
    the shell's surfaces too (Stage 2); capture and new keyboards asked, allowed or refused per
    program (Stage 6).
12. **The bell** is an event to the shell, and marks the window urgent (Stage 2).
13. **The Steam Deck's touch**: touch, edge swipes, the on-screen keyboard started when needed,
    the keyboard being the shell's (Stage 4).
14. **A living-room session**: one application fullscreen, the session ending with it, chosen at
    the login screen (Stage 4).
15. **HDR always on, or only for fullscreen HDR content**, a setting (Stage 5).
16. **A cross-fade on a change of theme or variant**: the screen frozen a quarter of a second
    while clients repaint, then faded to the new look; corners and shadows by rule for windows
    that draw their own frame; effects lighter on battery (Stage 6).

## Alternatives considered

Declined by the owner: minimising and showing the desktop (special workspaces serve);
workspaces shared by all outputs; a wallpaper per workspace and one that follows the hour; a
sticky edge and jumps between outputs; a material sampled from the wallpaper in place of blur;
marks and jump labels; a pinned live thumbnail of a window; integer scaling of games (gamescope
nested serves); the list of casts with their outline and a stop command, and with it the shell's
"screen shared" dots; key remapping and dual-role keys. Left as an idea: a settings application
apart from Hollow, and an output-management protocol as its first need. A focus steal allowed always, or never, and the shell owning
brightness, were the other options of points 1 and 10.

## Consequences

- Stage 2 grows again; Stage 4 gains the Deck's touch and the living-room session; Stage 6 the
  permissions, the theme's fade and the look rules.
- ADR-0019's single fullscreen per workspace becomes several. ADR-0057's shell gets its contract
  as an interface of Yuna's, specified with Hollow's design; brightness, which ADR-0057 left to
  the shell, is Yuna's.
- Every input feature names what it does over a fullscreen game and under a pointer lock.
