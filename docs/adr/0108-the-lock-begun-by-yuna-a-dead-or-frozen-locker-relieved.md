# 0108. The lock: begun by Yuna, a dead or frozen locker relieved, logind's `Unlock`

- Status: Accepted; supersedes [0057](0057-what-yuna-does-and-what-the-shell-does.md) (idle's
  lock before sleep),
  [0086](0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md) and
  [0088](0088-a-locker-that-stops-answering-counts-as-dead-its-answer-from-the-thread-that-draws-it.md);
  supersedes in part [0064](0064-what-the-best-desktops-taught-yuna.md) (the dead-locker screen)
- Date: 2026-11-18

## Context

The lock was decided in five ADRs.
[ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md) had idle run the shell's lock command
before sleep, "holding sleep back long enough to lock", without saying how.
[ADR-0064](0064-what-the-best-desktops-taught-yuna.md) gave a screen shown when a locker dies,
and [ADR-0065](0065-what-hollow-asks-of-yuna.md) let a new locker relieve a dead one.
[ADR-0086](0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md) answered
Hollow's audit: Yuna begins every lock, holds logind's sleep inhibitor, names the locker that may
relieve, and takes logind's `Unlock`. [ADR-0087](0087-a-locker-that-stops-answering-counts-as-dead.md),
then [ADR-0088](0088-a-locker-that-stops-answering-counts-as-dead-its-answer-from-the-thread-that-draws-it.md),
made a frozen locker count as dead. ADR-0086 was nearly whole; ADR-0088 had to be read beside it.
ADR-0086 left the D-Bus library to Stage 2b; ADR-0094 has since chosen how Yuna speaks D-Bus.

This ADR gathers the lock's rule as it stands on 2026-11-18. It decides nothing new. Hollow's
lock screen holds the lock through `ext-session-lock-v1`, which leaves a session locked when its
locker dies: "the compositor must not unlock the session in response".

## Decision

- **Yuna begins every lock itself** (ADR-0086, 2026-11-17). At a lock command, from a key, from
  idle after its delay, from logind's `Lock`, or before sleep (ADR-0057, 2026-11-14), Yuna enters
  its locked state at once: every output, new ones included, shows black, then the lock's
  surfaces alone; input reaches no client but the locker; captures see only the lock. It then runs
  the shell's lock command, and the locker takes the lock over as a relief. A lock and a relief
  are one path, and the session is never seen between the command and the locker's first frame.
- **The scene enforces the lock, through a restart.** No plane keeps another buffer while locked;
  a Yuna restarted after a crash while locked comes back locked; the first frame after a resume is
  the lock ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
  item 9; [ADR-0080](0080-what-survives-a-failure-and-yunas-own-panics.md)).
- **Yuna holds logind's sleep delay inhibitor**, and before sleep releases it only once the black
  frame is presented on every output (ADR-0086). This is how idle holds sleep back long enough to
  lock.
- **When a locker dies, the session stays locked and Yuna shows its dead-locker screen**
  (ADR-0064, 2026-11-14), which says how to unlock from a virtual terminal: `loginctl
  unlock-session` (ADR-0086).
- **A dead locker is relieved by the locker the configuration names**, Hollow by default, known by
  its connection's identity (ADR-0082 item 15); no other client may relieve it (ADR-0086). The
  relief was first open to any new locker (ADR-0065, 2026-11-14).
- **A locker that does not answer Yuna's ping within a delay counts as dead** (ADR-0088,
  2026-11-17). The ping is xdg-shell's `xdg_wm_base.ping`, which asks a client to answer at once;
  the locker binds `xdg_wm_base` for it. Yuna ends its connection, so that it cannot come back mid-unlock, and shows the
  dead-locker screen; the named locker relieves it, else logind's `Unlock`.
- **Yuna pings the locker when it should answer, never on a timer at rest** (ADR-0088): when the
  lock begins, and when the owner presses a key or moves the pointer on the lock screen, at most
  once a delay. A session locked and left alone wakes nothing. The delay is generous, about ten
  seconds, set at Stage 2b from what a busy locker takes; a locker counted dead by mistake costs a
  relief, never the lock.
- **The locker binds `xdg_wm_base` and answers its ping from the thread that draws the lock
  screen** (ADR-0088), never from a thread kept apart for it, which would answer while the screen
  is frozen. That thread never blocks on a disk, the network or D-Bus. Hollow accepted it (its
  ADR-0019), uwui's lock target answering as it reads the connection's events.
- **The locker unlocks; logind's `Unlock` does too when no locker lives** (ADR-0086). Yuna takes
  the signal only from the sender that owns logind's name, and only for its own session; while a
  locker lives, Yuna leaves the signal to it.
- **The lock's state is on Hollow's interface** (ADR-0086): locked or not, and whether a locker
  lives, given whole at connection and as events.
- **At Stage 2b**, with the session
  ([ADR-0081](0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md)) (ADR-0086). Yuna
  speaks D-Bus through its own connection on `zbus` 6.3, without its default features: zbus
  encodes the messages, and Yuna's code connects, authenticates, sends and serves on its event
  loop, with no thread and no runtime
  ([ADR-0094](0094-the-accessibility-tree-yuna-as-the-registry-of-each-seats-bus.md), 2026-11-18).

## Alternatives considered

- **The dead-locker screen alone**, no relief ([ADR-0065](0065-what-hollow-asks-of-yuna.md)):
  unlocking would need leaving the graphical session.
- **Only a locker unlocks**, logind's `Unlock` refused
  ([ADR-0086](0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md)): a locker
  that crashes at each start leaves only a reboot, which Hollow's ADR-0016 refused.
- **The lock begun by the locker**, Yuna holding sleep back until its first frame
  ([ADR-0086](0086-the-lock-begun-by-yuna-relieved-by-the-locker-and-loginds-unlock.md)): the
  session shows between the command and that frame.
- **A living locker never touched**
  ([ADR-0088](0088-a-locker-that-stops-answering-counts-as-dead-its-answer-from-the-thread-that-draws-it.md)):
  the owner stays before a frozen screen that says nothing.
- **A ping on a timer while locked**
  ([ADR-0088](0088-a-locker-that-stops-answering-counts-as-dead-its-answer-from-the-thread-that-draws-it.md)):
  a locked session at rest wakes Yuna.

## Consequences

- No state of `hollow` leaves the owner without a way back that the screen shows.
- Hollow answers the ping from the thread that draws its lock screen.
- A locker slower than the delay is ended and relieved: a flicker, the session still locked.
- Yuna listens to logind, which Stage 2b's session needs anyway.
- A process of the owner's user may unlock a session whose locker is dead, as far as polkit lets
  it call `loginctl unlock-session`. The owner's agents run as that user: Stage 2b checks what
  polkit allows an agent's service, and if it allows the unlock, a polkit rule for the owner's
  NixOS configuration is put to the owner.

## Validation and reconsideration

Checked at Stage 2b, in headless Yuna with a fake logind on a private bus: a locker killed while
locked, no frame of the session presented, the named locker relieving it and no other client; a
locker that stops answering ended after the delay on the owner's input, one that answers never
ended, a locked session left alone waking nothing; a forged `Unlock` from another sender refused;
`Unlock` while a locker lives left to it; a sleep right after the lock command, the black frame
presented on every output before the inhibitor is released; a crash while locked, Yuna back
locked. Proposed again if polkit lets another user unlock the session, if Hollow withdraws its
ADR-0016, or if the delay ends lockers that would have recovered, as measured on the laptop.
