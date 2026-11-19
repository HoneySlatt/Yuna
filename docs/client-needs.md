# What the owner's other projects need of Yuna

Each project's needs, as the project stated them, and Yuna's answer: the decision that covers it,
the stage that builds it, or what is still to design
([ADR-0126](adr/0126-yuna-and-the-owners-other-projects.md)). Yuna never changes
another project's repository; each project keeps its own copy of what it asked.

Every project but Hollow and Magnolia measures or tests in a headless compositor today: sway,
Weston, KWin or Mutter. Each moves to Yuna in its own sessions, by starting `yuna --headless
--agent-seat` where it started sway: an agent's seat, its virtual keyboard and pointer, and
`wlr-screencopy` ([ADR-0126](adr/0126-yuna-and-the-owners-other-projects.md)). Yuna's tests run
uwui's and Pluto's drivers that way, at a pinned revision (`crates/yuna/tests/consumers.rs`).
What sway's IPC (`swaymsg`) gives, the outputs and the window tree, has no equivalent before
Hollow's interface (Stage 2). [`protocols.md`](protocols.md) says what each protocol below is in
Yuna. Read in each repository on 2026-11-17.

## The projects that measure and test

| Project | What it needs of a compositor | Yuna's answer |
|---|---|---|
| uwui | `measure.py` in a headless sway at 3840 × 2160, 240 Hz, scale 1.5, with Xwayland: a virtual pointer, copies with damage (`wlr-screencopy` v3) timed by the compositor, `xdg-output` | `measure.py --display` in `yuna --headless --agent-seat`, whose default output is that one; tested (ADR-0126) |
| uwui | `swaymsg` for the output's rate and the window tree; `grim` for a `shot` step | No IPC before Stage 2; `grim` 1.5 speaks both capture protocols Yuna offers, not run in Yuna's tests |
| uwui | sway's headless output refreshes at 243 Hz, not 240; no headless compositor emulates VRR | Yuna's outputs keep their rate on its clock ([ADR-0118](adr/0118-frames-presentation-times-and-the-clock.md)) and emulate VRR (M1.6) |
| uwui | Hosted by Yuna for what Yuna draws, a failure of it never taking Yuna down | [ADR-0106](adr/0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md), Stage 2 |
| Pluto | Its rivals' bench drives each subject in a headless sway through `pluto-input`: one virtual keyboard of its own keymap and one virtual pointer for a run | Offered on agents' seats; tested at a pinned revision (ADR-0126) |
| Pluto | The bench waits on sway's window events and tree (`swaymsg`), and shoots with `grim` | No IPC before Stage 2; `grim` as uwui's |
| Pluto | Its own measures run in a headless Weston and read no presentation time | Yuna's presentation times are honest ([ADR-0118](adr/0118-frames-presentation-times-and-the-clock.md)) |
| SlimeEngine | Its examples in headless Weston, sway, KWin and Mutter; wlroots' virtual keyboard and pointer for sway (libei for KWin and Mutter, XTest for X11), `wlrctl` for its input-to-present measure | Virtual input on agents' seats (ADR-0116); libei not offered; its examples already run in Yuna's weekly job through `yuna mcp` ([ADR-0122](adr/0122-real-applications-in-ci-and-rest-checked-on-the-program.md)) |
| SlimeEngine | `ext-data-control-v1` | Offered to the owner's seat alone: a test of it runs `yuna --headless` without `--agent-seat` |
| SlimeEngine | `wp_fifo_v1` and `wp_commit_timing_v1`, so that FIFO works without frame callbacks; a hidden window not waiting a second for each frame | Stage 2, a hidden game paced at its configured rate ([ADR-0118](adr/0118-frames-presentation-times-and-the-clock.md)) |
| SlimeEngine | `xdg-foreign`; pointer constraints and relative pointer, checked by hand | Stage 2c ([ADR-0061](adr/0061-yunas-uses.md)) |
| SlimeEngine | Survive a GPU reset, which ended niri and the session | Recovery without a restart, Stage 2b ([ADR-0080](adr/0080-what-survives-a-failure-and-yunas-own-panics.md)) |
| Aerith | `color-management-v1`, `presentation-time`, `fractional-scale-v1`, `viewporter`, `linux-dmabuf-v1` | Offered; `linux-dmabuf-v1` when the renderer imports a format: with `--gpu`, and on llvmpipe its formats of one plane ([ADR-0123](adr/0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md)) |
| Aerith | `fifo-v1`, `content-type-v1` | Stage 2c |
| Aerith | Its high tier's series measured on Yuna, and Yuna's refusals of colour descriptions in its relay's union | Once Yuna is the owner's desktop (Stage 2); HDR drawn at Stage 5 |
| Opium | Measures in a headless Weston, which paces transitions badly, at scale 1.5; window events through Umbriel's CLI, gone | Yuna paces each output on its clock; window events with Hollow's interface, Stage 2 |
| Opium | A small window kept above the others, for which Wayland has no protocol | Not decided |
| Opium | Its desktop window run in Yuna for Stage 1's exit: opened on a throwaway profile, clicked and checked by capture by an agent through `yuna mcp` alone ([ADR-0104](adr/0104-stage-1s-exit-opium-in-aeriths-place-and-its-exceptions-written.md)) | Done: `crates/yuna/tests/opium.rs`, which the weekly Opium job runs ([ADR-0122](adr/0122-real-applications-in-ci-and-rest-checked-on-the-program.md)) |
| Slatty | Measures in a headless sway at 3840 × 2160, 240 Hz, scale 1.5, with Xwayland; presentation feedback, wished | `yuna --headless`'s default output is that one, with Xwayland; `presentation-time` offered |
| Slatty | Runs in gamescope on the Steam Deck | gamescope nested in Yuna, Stage 4 |
| Magnolia | Nothing tested in a compositor. Through uwui: the next frame's predicted presentation time, a locked relative pointer, each window paced by its own output, `xdg-foreign`, popups past the window's edge | Each output paced at its own rate ([ADR-0118](adr/0118-frames-presentation-times-and-the-clock.md)); popups as `xdg-shell` places them; the rest at Stage 2c |

## Hollow

The owner's shell, on uwui ([ADR-0107](adr/0107-what-yuna-does-itself-and-what-it-leaves-to-the-shell.md),
[ADR-0106](adr/0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md)). One process: the bar,
popups, notifications, the launcher, on-screen displays, the tray, the lock screen on
`ext-session-lock-v1` and the polkit agent, PAM in a helper it starts. Its greeter is a program
of its own. Keys are bound in Yuna's configuration. Hollow writes no code before uwui's first
stage. Stated on 2026-11-14, rows 14 to 18 on 2026-11-17; Hollow's copy is `docs/needs.md` in its
repository. Each row keeps its number in Hollow's copy; its row 13, the layer shell and the
session lock in headless Yuna before Hollow's surfaces, has no stage there, and is not copied
here.

| # | Hollow needs | Yuna's answer |
|---|---|---|
| 1 | An event naming each program that captures or casts, for a "screen shared" indicator | Decided ([ADR-0107](adr/0107-what-yuna-does-itself-and-what-it-leaves-to-the-shell.md)), Stage 2 |
| 2 | A greeter mode: Yuna under greetd as the greeter's compositor, without command socket, MCP, global shortcuts, portals or XWayland, switching virtual terminals kept, the hand-over to the session without flicker | Decided ([ADR-0107](adr/0107-what-yuna-does-itself-and-what-it-leaves-to-the-shell.md)), Stage 2 |
| 3 | A new `ext-session-lock-v1` client taking over from a dead one, beside the dead-locker screen; every output blanked at once when locking | Decided ([ADR-0108](adr/0108-the-lock-begun-by-yuna-a-dead-or-frozen-locker-relieved.md)), Stage 2b |
| 4 | Key bindings that send a named event to Hollow on its interface, no process started per key, under Yuna's rules for games, shortcut inhibitors and the lock screen | Part of Hollow's interface ([ADR-0107](adr/0107-what-yuna-does-itself-and-what-it-leaves-to-the-shell.md)); its form designed at Stage 2 |
| 5 | A way for the on-screen keyboard to send keys (Enter, arrows), since the virtual keyboard stays off the owner's seat, and a signal to show or hide it with the focus of a text field | To design with the Steam Deck's touch, Stage 4 ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md) point 13, [ADR-0116](adr/0116-agents-seats-each-agent-in-its-own-yuna.md)); an EI path, as COSMIC has, is a candidate |
| 6 | The keyboard layout and its changes, as an event | To add to Hollow's interface, Stage 2 |
| 7 | A command to log out | To add to Hollow's interface, Stage 2 |
| 8 | The start and end of a theme switch, so that Yuna's cross-fade waits for programs to repaint | With the cross-fade, Stage 6 ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md) point 16) |
| 9 | A layer for desktop widgets above Yuna's wallpaper | `wlr-layer-shell`'s background and bottom layers, drawn above Yuna's wallpaper ([ADR-0107](adr/0107-what-yuna-does-itself-and-what-it-leaves-to-the-shell.md)), Stage 2 |
| 10 | Stable output names, for settings per output, beside the primary output | Output ids already follow the monitor ([ADR-0111](adr/0111-the-layout-models-shape-plain-ids-a-branch-per-seat-asked-and-committed-sizes.md)); their names given to the shell, Stage 2 |
| 11 | Care of the bar on the owner's OLED monitor, the most static thing shown | Decided with Stage 5's OLED care ([ROADMAP](ROADMAP.md)) |
| 12 | Blur behind Hollow's surfaces (`ext-background-effect-v1` or layer rules), and which of its surfaces are never captured | Blur with the effects, Stage 6 ([ADR-0109](adr/0109-animations-and-effects-what-moves-reduced-motion-and-games.md)); never-captured surfaces of the shell, Stage 2 ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md) point 11) |
| 14 | The lock's state on Hollow's interface, given whole at connection: locked or not, and whether its locker lives | Decided ([ADR-0108](adr/0108-the-lock-begun-by-yuna-a-dead-or-frozen-locker-relieved.md)), Stage 2b |
| 15 | The relief of a dead locker reserved to Hollow's role | Decided: reserved to the locker the configuration names, Hollow by default ([ADR-0108](adr/0108-the-lock-begun-by-yuna-a-dead-or-frozen-locker-relieved.md)), Stage 2b |
| 16 | logind's `Unlock`, its sender checked against logind's name, unlocks the session when no locker lives | Decided ([ADR-0108](adr/0108-the-lock-begun-by-yuna-a-dead-or-frozen-locker-relieved.md)), Stage 2b |
| 17 | At the lock command and before sleep, Yuna holds logind's sleep delay inhibitor and blanks every output itself, before any client; the locker then takes over | Decided: Yuna begins every lock, the locker relieves it ([ADR-0108](adr/0108-the-lock-begun-by-yuna-a-dead-or-frozen-locker-relieved.md)), Stage 2b |
| 18 | The dead-locker screen says how to unlock from a virtual terminal | Decided ([ADR-0108](adr/0108-the-lock-begun-by-yuna-a-dead-or-frozen-locker-relieved.md)), Stage 2b |

Yuna asks of Hollow: its locker binds `xdg_wm_base` and answers its ping from the thread that
draws the lock screen, never from one kept apart; a locker that does not answer is counted dead
([ADR-0108](adr/0108-the-lock-begun-by-yuna-a-dead-or-frozen-locker-relieved.md)). Asked and accepted on 2026-11-17 (Hollow's ADR-0019).

Already decided, as Hollow needs them: workspaces, the focus, outputs and the primary output, the
fullscreen state (a game, a film), urgent windows, the bell, the wallpaper shown, brightness and
window previews ([ADR-0107](adr/0107-what-yuna-does-itself-and-what-it-leaves-to-the-shell.md)).

Useful early: `wlr-layer-shell` and `ext-session-lock-v1` in headless Yuna before Hollow's
surfaces stage. Until then, Hollow tests in a headless sway.
