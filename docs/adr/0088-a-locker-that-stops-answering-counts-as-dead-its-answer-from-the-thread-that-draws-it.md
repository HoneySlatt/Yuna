# 0088. A locker that stops answering counts as dead, its answer from the thread that draws it

- Status: Accepted; supersedes [0087](0087-a-locker-that-stops-answering-counts-as-dead.md)
- Date: 2026-11-17

## Context

[ADR-0086](0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md) brings a
locked session back when its locker dies: the named locker relieves it, or logind's `Unlock`
unlocks it when no locker lives. It left open a locker alive but frozen, in an endless loop or
with its interface's thread blocked. Such a locker is not dead, so nothing relieves it, and
logind's `Unlock` is left to it: the owner faces a frozen lock screen. A way out exists by hand,
from a virtual terminal, ending `hollow` and then running `loginctl unlock-session`, but nothing on
screen says so.

xdg-shell's `xdg_wm_base.ping` asks a client to answer at once, and a client that does not is
taken as frozen; GNOME and KWin show their "not responding" dialogs this way.
[ADR-0064](0064-what-the-best-desktops-taught-yuna.md) has Yuna show a frozen application at
Stage 2. Nothing may wake at rest ([ROADMAP](../ROADMAP.md)).

[ADR-0087](0087-a-locker-that-stops-answering-counts-as-dead.md) took this decision asking the
locker to answer "from a thread that never blocks", which a locker could read as a thread kept
apart to answer pings; Hollow, accepting it, pointed out that such a thread would answer while
the lock screen is frozen. This ADR states the rule whole with the thread named.

## Decision

The owner chose, on 2026-11-17:

- **A locker that does not answer Yuna's ping within a delay counts as dead.** Yuna ends its
  connection, so that it cannot come back mid-unlock, and shows the dead-locker screen; ADR-0086's
  path follows: the named locker relieves it, else logind's `Unlock`.
- **Yuna pings the locker when it should answer, never on a timer at rest**: when the lock
  begins, and when the owner presses a key or moves the pointer on the lock screen, at most once a
  delay. A session locked and left alone wakes nothing.
- **The delay is generous**, about ten seconds, set at Stage 2b from what a busy locker takes; a
  locker counted dead by mistake costs a relief, never the lock.
- **The locker binds `xdg_wm_base` and answers its ping from the thread that draws the lock
  screen**, never from a thread kept apart for it: one apart would answer while the screen is
  frozen, and the check would never find what it exists for. That thread never blocks on a disk,
  the network or D-Bus, so a working screen always answers and a frozen one does not. Hollow
  accepted it (its ADR-0019), uwui's lock target answering as it reads the connection's events.
- At Stage 2b, with ADR-0086.

## Alternatives considered

- **A living locker never touched**, the way out by hand documented: nothing to build, no locker
  ended that would have recovered; but the owner, not knowing to reach a virtual terminal, stays
  before a frozen screen that says nothing.
- **A ping on a timer while locked**: a frozen locker found even with nobody at the machine, but
  a locked session at rest wakes Yuna, against the bar; nobody needs the lock back until someone
  is there.

## Consequences

- No state of `hollow` leaves the owner without a way back that the screen shows.
- Hollow gains a need of its own: answering the ping from the thread that draws its lock screen.
- A locker slower than the delay is ended and relieved: a flicker, the session still locked.

## Validation and reconsideration

Checked at Stage 2b, in headless Yuna: a locker that stops answering is ended after the delay on
the owner's input, the dead-locker screen shown, the named locker relieving it; a locker that
answers is never ended; a locked session left alone wakes nothing. Proposed again if the delay
ends lockers that would have recovered, as measured on the laptop.
