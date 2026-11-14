# 0055. What moves first, and animations off headless

- Status: Accepted; amends [0033](0033-the-mcp-server.md) (`--animations`); amended by
  [0059](0059-reduced-motion-nothing-slides-fades-stay.md) (reduced motion now)
- Date: 2026-11-14

## Context

[ADR-0010](0010-animations-and-effects.md) lists motion, fades and appearances, and exotic open
and close animations with the effects. The [ROADMAP](../ROADMAP.md) lands a behaviour when it is
as good as the best rival's, or waits: niri and Hyprland fade windows in and out. A closing
window's fade needs its last buffer kept after its client destroyed it. Workspaces are stacked
vertically ([ADR-0018](0018-workspaces.md)). `yuna --headless` runs other projects' tests
([ADR-0038](0038-captures-coordinates-windows-and-wayland.md)), and agents click on what they
capture; Yuna's applications' tests already turn GTK's animations off.

## Decision

The owner chose, on 2026-11-14:

- **M1.7 moves**, on springs, the scrolling camera, every window a layout operation moves
  (scrolling, dwindle, floating) and the workspace switch; and **fades**, on a curve, the special
  workspace in and out. Windows open and close together with the effects (Stage 6).
- **Fullscreen is instant**, in and out: a game wants its output at once.
- **A workspace switch slides** along ADR-0018's vertical stack, and moves only the workspace
  left and the one shown, however far apart they are; a switch retargeted mid-way slides on from
  where it is, the workspaces in between left out.
- **Animations are off** in `yuna --headless` and `yuna mcp`: an operation shows its result at
  once, as before. `--animations` turns them on, in both. The desktop will have them on. `off`
  stays a setting of its own, for game mode and agents; reduced motion later chooses a kind per
  animation.
- **Settings** are one `Settings` value in the compositor's configuration, each animation a
  spring or a curve with its defaults, until the configuration file is read (Stage 2).

## Alternatives considered

- **An opening fade alone**: windows that appear softly and vanish abruptly.
- **Opening and closing now**: complete at once, but a closing window's buffer kept after its
  surface is gone is its own work, with its own edge cases; it comes with the effects.
- **Fullscreen animated** (niri): about 0.4 s of motion before a game has its output.
- **The whole stack crossing the view** on a far switch (niri): Mod+1 to Mod+9 shows and wakes
  seven workspaces nobody asked for.
- **A cross-fade switch** (Hyprland's example): both workspaces drawn whole and blended, twice a
  slide's fill, heavier on the laptop.
- **On by default headless**: other projects' tests and agents would see motion half-way.

## Consequences

- The existing stories run with animations off and are unchanged; M1.7's own tests turn them on.
- Agents always run in a headless Yuna of their own
  ([ADR-0060](0060-development-first-agents-in-their-own-yuna.md)): its animations are that
  Yuna's, off unless the agent asks.
