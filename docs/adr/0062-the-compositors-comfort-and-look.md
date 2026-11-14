# 0062. The compositor's comfort and look

- Status: Accepted; amends 0005, 0010
- Date: 2026-11-14

## Context

On 2026-11-14 a survey compared Yuna's plan with the comfort, look and performance features of
niri 26.04, Hyprland 0.57.2, KWin 6.8.3, GNOME 51, macOS 27 and Windows 11, from their own
documentation. Yuna's plan already holds most of them: blur and glass, corners, shadows,
borders, motion on springs, the overview, Alt-Tab with previews, gestures, window rules, live
reload, captures, HDR, VRR, OLED care, direct scanout, tearing. What others have and Yuna's plan
lacked: hot corners (the owner uses one in Umbriel, the top-left corner opening the overview);
tabbed columns (niri) and groups (Hyprland), which [ADR-0005](0005-the-layouts.md) left out until
the owner asked; snap zones for floating windows (KWin, Windows, macOS); focus that follows the
mouse; floating windows' sizes remembered (Hyprland); a shake to find the cursor (macOS, KWin);
animations that change a shape, such as macOS's Liquid Glass and KWin's magic lamp; scripting
(KWin, Hyprland's Lua) and plugins (Hyprland). The survey also offered five ideas none of them
has.

## Decision

The owner chose, on 2026-11-14:

1. **Hot corners**, at Stage 2.
2. **Tabbed columns**: a column of scrolling columns can show its windows as tabs, one visible,
   with a tab bar Yuna draws; at Stage 2, as a layout behaviour, tested in the layout model.
3. **Snap zones for floating windows**: edges and zones from the configuration, shown while a
   window is dragged; at Stage 6.
4. **Focus follows the mouse** and **floating windows' sizes remembered** per application, each a
   setting; at Stage 2.
5. **Shapes that morph**, at Stage 6: a window that changes shape into its place, glass that
   merges, measured as every effect and switched off under reduced motion. They join the open,
   close and resize animations that M1.7 leaves to Stage 6.
6. **No scripting inside Yuna**: an outside program drives Yuna through its commands and event
   stream. Scripts inside Yuna stay an idea, for a need commands cannot serve: run on events only,
   never at each frame, in a language bounded in time.
7. **A rest auditor**, in M1.11: at rest, Yuna names each program, or part of itself, that wakes it
   or makes it draw; a command, an MCP tool and a log line.
8. **An OLED wear map**, at Stage 5, as the method of OLED care: per output, how long each region
   has shown the same bright, still content, to choose what to dim slowly or shift by a pixel. It
   is read only while Yuna draws anyway, never at rest.
9. **A capture yields to a game**: rather than cost a game a frame, a capture lowers its own rate,
   and says so.

Kept as ideas, not planned: an effect governor, which would lighten the costliest effect when a
frame nears its deadline and restore it after; a replay capsule for agents, which would keep the
last seconds of input, protocol trace and frames to replay them on the test clock, after M1.11.

## Alternatives considered

- **No tabs, as ADR-0005 decided**: the owner asked for them.
- **Snap zones at Stage 2**: Stage 2 is already the largest stage; zones are polish.
- **Finding the cursor by a shake, and its size**: declined by the owner; an enlarged cursor is
  drawn by Yuna and ends direct scanout while it lasts.
- **Wobbly windows**: not chosen.
- **Scripts inside Yuna**: microseconds an event, but a script that loops would freeze the
  screen, a script reads every key, and its interface is public for good. **Plugins**: a plugin
  can take Yuna down ([ADR-0007](0007-surviving-a-crash.md)).
- **The effect governor now**: no missed frame from an effect on any machine, but quality that
  changes by itself and can hide a slow effect; kept for later by the owner.

## Consequences

- ADR-0005's left-out tab groups come in as tabbed columns; the layout model gains them with their
  scenarios. ADR-0010's "a few exotic animations" become shapes that morph.
- Stage 2 gains hot corners, tabbed columns, focus following the mouse and remembered sizes;
  Stage 5 the wear map; Stage 6 snap zones and morphing; M1.11 the rest auditor; the capture
  budget gains its yielding.
- The PLAN's ideas gain scripts inside Yuna, the effect governor and the replay capsule.
