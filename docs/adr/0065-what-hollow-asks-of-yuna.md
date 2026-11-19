# 0065. What Hollow asks of Yuna: a sharing event, a greeter mode, a locker's relief

- Status: Superseded by [0107](0107-what-yuna-does-itself-and-what-it-leaves-to-the-shell.md)
  (the capture event, the greeter mode); amends [0064](0064-what-the-best-desktops-taught-yuna.md)
  (the shell's sharing indicator, the dead locker); superseded in part by
  [0086](0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md) (a locker's
  relief)
- Date: 2026-11-14

## Context

Hollow is the owner's shell, one process on uwui: bar, popups, notifications, launcher, on-screen
displays, tray, the lock screen on `ext-session-lock-v1` and the polkit agent; its greeter is a
program of its own ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md),
[ADR-0063](0063-vulkan-alone-and-uwui-for-what-yuna-draws.md)). Its design session listed on
2026-11-14 what it needs of Yuna; `docs/client-needs.md` holds the list and Yuna's answer to
each. Three of its points change or add to a decision.
[ADR-0064](0064-what-the-best-desktops-taught-yuna.md) declined the list of casts and, with it,
the shell's "screen shared" dots, and gives a screen shown when a locker dies. niri lets a new
locker take over from a dead one; `ext-session-lock-v1` allows it.

## Decision

The owner chose, on 2026-11-14:

- **Yuna tells the shell who captures.** An event names each program capturing or casting an
  output, a region or a window, when it starts and stops, so that Hollow shows a "screen
  shared" indicator. The list of casts with their outlines and a stop command stays declined
  (Stage 2).
- **The login screen runs on Yuna, in a greeter mode.** Under greetd, Yuna runs as the greeter's
  compositor: no command socket, no MCP, no global shortcuts, no portals, no XWayland; switching
  virtual terminals kept. The session's Yuna then reuses the active mode, so that the hand-over
  shows no flicker (Stage 2).
- **A new locker may relieve a dead one.** When the lock client dies, the session stays locked
  and Yuna shows its dead-locker screen; a new `ext-session-lock-v1` client may then take the
  lock over, and only it unlocks the session (Stage 2).

## Alternatives considered

- **No sharing indicator** (ADR-0064): less to build, but the owner would not see that a program
  watches the screen.
- **The greeter on another compositor** (cage, a sway of its own): nothing to build in Yuna, but
  two compositors to maintain in the session and a flicker at each hand-over.
- **The dead-locker screen alone**: simpler, but unlocking would need leaving the graphical
  session; a restarted Hollow could not give the lock screen back.

## Consequences

- Stage 2 gains the capture event, the greeter mode and the locker's relief.
- A program that captures cannot hide from the shell's indicator; the never-captured surfaces of
  ADR-0064 stay out of what it receives.
