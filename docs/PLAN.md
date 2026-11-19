# Plan

What is being worked on now: the milestones of the current stage, their status and how each is
checked. Direction beyond this stage is in [`ROADMAP.md`](ROADMAP.md); decisions are in
[`adr/`](adr/README.md). A *stage* is a step of the roadmap; a *milestone* is a small, verifiable
step of a stage.

Every milestone ends with the definition of done in [`AGENTS.md`](../AGENTS.md#definition-of-done).

## Current

- Stage 1, the headless core, is done: its milestones, M1.1 to M1.13, are in
  [`history.md`](history.md); its [audit](audits/2026-11-18-stage-1.md) closed it. 0.1 is tagged
  `v0.1.0` on 2026-11-19, the owner choosing to tag before two of the audit's last steps.
- **Still open from M1.13**, after the tag: the budgets measured again on the release candidate,
  and the closing review by a fresh reviewer. What they find goes to a 0.1.1, or to the debt.
- **Next: Stage 2's opening design**, in a new session ([ROADMAP](ROADMAP.md),
  [ADR-0081](adr/0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md),
  [ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)):
  the decisions the ROADMAP leaves to it, 2a's milestones, the parity net first, and the debt
  items no stage follows yet, which it places, as the owner chose on 2026-11-19
  ([debt](debt.md)). The rest of Rust 1.100's new features are put to the owner there, each on a
  measure.
- The owner works on `main` directly, since 2026-11-17: push only a state where
  `scripts/check.sh` passes, and follow CI to green after each push.
- The owner's desktop runs niri since 2026-11-16: Stage 2's goal is to replace it.

## Status

| Stage | Goal | Status |
|---|---|---|
| [1](history.md#stage-1--the-headless-core--01) | Agents and tests drive Yuna without a screen; every layout behaviour a test | Done, 0.1 |
| 2 | The desktop, in three gated parts ([ROADMAP](ROADMAP.md)) | Opening design next |

## Ideas

- The layout's maps hashed by a faster hasher than the standard library's SipHash: 22.8 % of a
  "float, then tile" at 100 windows hashes `WindowId`s (measured at M1.13).
- Scripts inside Yuna, on events only, bounded in time; an effect governor that lightens the
  costliest effect near a frame's deadline; a replay capsule for agents, after M1.11
  ([ADR-0062](adr/0062-the-compositors-comfort-and-look.md)).
- Layout behaviours the owner uses in Umbriel and ADR-0005 does not list: maximize-to-edges
  (Mod+Alt+M) and per-window heights in a column (Mod+Ctrl+Shift+R, Mod+Shift+Minus/Equal).
- The RemoteDesktop and InputCapture portals, for remote control or a mouse shared between
  machines, once a concrete use asks for them, with consent each session, an indicator and a
  shortcut that stops them ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)).
- A settings application, apart from Hollow, on uwui, opened from its control centre: it sets
  Yuna through Yuna's commands or an include of its own, and is the first client of an
  output-management protocol ([ADR-0064](adr/0064-what-the-best-desktops-taught-yuna.md)).
- A command that sets one configuration key until the next reload, as `hyprctl keyword` does,
  beside variants ([ADR-0057](adr/0057-what-yuna-does-and-what-the-shell-does.md)).
- Directional focus that goes on to the next output at a strip's end (niri's `-or-monitor`
  actions).
- From Claude Code's run of M1.11: a window moved to an output by one call; a removed output's
  windows shown, or said hidden. (A click's result telling what it pressed: `pointer`'s result
  names the window under the pointer already, the newcomer test found.)

- A second Xwayland at scale 1 for games, on its own display, so that a game on an output of a
  smaller scale always draws at its mode ([ADR-0069](adr/0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md)).

## Known debt

In [`debt.md`](debt.md).
