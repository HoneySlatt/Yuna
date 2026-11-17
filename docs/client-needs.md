# What the owner's other projects need of Yuna

Each project's needs, as the project stated them, and Yuna's answer: the decision that covers it,
the stage that builds it, or what is still to design
([ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)). Yuna never changes
another project's repository; each project keeps its own copy of what it asked.

## Hollow

The owner's shell, on uwui ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md),
[ADR-0063](adr/0063-vulkan-alone-and-uwui-for-what-yuna-draws.md)). One process: the bar,
popups, notifications, the launcher, on-screen displays, the tray, the lock screen on
`ext-session-lock-v1` and the polkit agent, PAM in a helper it starts. Its greeter is a program
of its own. Keys are bound in Yuna's configuration. Hollow writes no code before uwui's first
stage. Stated on 2026-11-14, rows 14 to 18 on 2026-11-17; Hollow's copy is `docs/needs.md` in its repository.

| # | Hollow needs | Yuna's answer |
|---|---|---|
| 1 | An event naming each program that captures or casts, for a "screen shared" indicator | Decided ([ADR-0065](adr/0065-what-hollow-asks-of-yuna.md)), Stage 2 |
| 2 | A greeter mode: Yuna under greetd as the greeter's compositor, without command socket, MCP, global shortcuts, portals or XWayland, switching virtual terminals kept, the hand-over to the session without flicker | Decided ([ADR-0065](adr/0065-what-hollow-asks-of-yuna.md)), Stage 2 |
| 3 | A new `ext-session-lock-v1` client taking over from a dead one, beside the dead-locker screen; every output blanked at once when locking | Decided ([ADR-0086](adr/0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md)), Stage 2b |
| 4 | Key bindings that send a named event to Hollow on its interface, no process started per key, under Yuna's rules for games, shortcut inhibitors and the lock screen | Part of Hollow's interface ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md)); its form designed at Stage 2 |
| 5 | A way for the on-screen keyboard to send keys (Enter, arrows), since the virtual keyboard stays off the owner's seat, and a signal to show or hide it with the focus of a text field | To design with the Steam Deck's touch, Stage 4 ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md) point 13, [ADR-0058](adr/0058-what-yuna-takes-from-the-owners-other-projects.md)); an EI path, as COSMIC has, is a candidate |
| 6 | The keyboard layout and its changes, as an event | To add to Hollow's interface, Stage 2 |
| 7 | A command to log out | To add to Hollow's interface, Stage 2 |
| 8 | The start and end of a theme switch, so that Yuna's cross-fade waits for programs to repaint | With the cross-fade, Stage 6 ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md) point 16) |
| 9 | A layer for desktop widgets above Yuna's wallpaper | `wlr-layer-shell`'s background and bottom layers, drawn above Yuna's wallpaper ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)), Stage 2 |
| 10 | Stable output names, for settings per output, beside the primary output | Output ids already follow the monitor ([ADR-0020](adr/0020-the-layout-models-shape.md)); their names given to the shell, Stage 2 |
| 11 | Care of the bar on the owner's OLED monitor, the most static thing shown | Decided with Stage 5's OLED care ([ROADMAP](ROADMAP.md)) |
| 12 | Blur behind Hollow's surfaces (`ext-background-effect-v1` or layer rules), and which of its surfaces are never captured | Blur with the effects, Stage 6 ([ADR-0010](adr/0010-animations-and-effects.md)); never-captured surfaces of the shell, Stage 2 ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md) point 11) |
| 14 | The lock's state on Hollow's interface, given whole at connection: locked or not, and whether its locker lives | Decided ([ADR-0086](adr/0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md)), Stage 2b |
| 15 | The relief of a dead locker reserved to Hollow's role | Decided: reserved to the locker the configuration names, Hollow by default ([ADR-0086](adr/0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md)), Stage 2b |
| 16 | logind's `Unlock`, its sender checked against logind's name, unlocks the session when no locker lives | Decided ([ADR-0086](adr/0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md)), Stage 2b |
| 17 | At the lock command and before sleep, Yuna holds logind's sleep delay inhibitor and blanks every output itself, before any client; the locker then takes over | Decided: Yuna begins every lock, the locker relieves it ([ADR-0086](adr/0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md)), Stage 2b |
| 18 | The dead-locker screen says how to unlock from a virtual terminal | Decided ([ADR-0086](adr/0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md)), Stage 2b |

Yuna asks of Hollow: its locker binds `xdg_wm_base` and answers its ping from a thread that never
blocks; a locker that does not answer is counted dead ([ADR-0087](adr/0087-a-locker-that-stops-answering-counts-as-dead.md)). Asked on 2026-11-17.

Already decided, as Hollow needs them: workspaces, the focus, outputs and the primary output, the
fullscreen state (a game, a film), urgent windows, the bell, the wallpaper shown, brightness and
window previews ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md),
[ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md)).

Useful early: `wlr-layer-shell` and `ext-session-lock-v1` in headless Yuna before Hollow's
surfaces stage. Until then, Hollow tests in a headless sway.
