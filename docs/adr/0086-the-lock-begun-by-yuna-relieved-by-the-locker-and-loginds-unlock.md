# 0086. The lock begun by Yuna, relieved by the locker, and logind's `Unlock`

- Status: Accepted; supersedes in part [0065](0065-what-hollow-asks-of-yuna.md) (a locker's
  relief)
- Date: 2026-11-17

## Context

Hollow's lock screen holds the session's lock through `ext-session-lock-v1`. When a locker dies,
the protocol leaves the session locked: "the compositor must not unlock the session in
response". [ADR-0065](0065-what-hollow-asks-of-yuna.md) then shows Yuna's dead-locker screen and
lets a new locker relieve the dead one, which alone unlocks.
[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) keeps the
lock in the scene, on every output, through a restart of Yuna.
[ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md) has Yuna run the shell's lock command
after a delay and before sleep, "holding sleep back long enough to lock", without saying how.

Hollow's audit of 2026-11-17 found the case that breaks: Hollow crashes at each start while the
session is locked; systemd stops restarting it; Yuna shows its dead-locker screen; `loginctl
unlock-session` from a virtual terminal does nothing, since nothing answers logind's `Unlock`; a
restarted Yuna comes back locked. Only a reboot is left, and unsaved work is lost. Hollow's
ADR-0016 answers with four layers, the last two Yuna's, and asks Yuna for five things (rows 14 to
18 of Hollow's `docs/needs.md`, in [`client-needs.md`](../client-needs.md)). Hollow also asked
that every output be blanked at once when locking, which no answer of Yuna's covered.

## Decision

The owner chose, on 2026-11-17:

- **Yuna begins every lock itself.** At a lock command, from a key, from idle, from logind's
  `Lock`, or before sleep, Yuna enters its locked state at once: every output, new ones
  included, shows black, then the lock's surfaces alone; input reaches no client but the locker;
  captures see only the lock. It then runs the shell's lock command, and the locker takes the
  lock over as a relief. A lock and a relief are one path, and the session is never seen between
  the command and the locker's first frame.
- **Yuna holds logind's sleep delay inhibitor**, and before sleep releases it only once the black
  frame is presented on every output.
- **A dead locker is relieved by the locker the configuration names**, Hollow by default, known
  by its connection's identity (ADR-0082's client identity); no other client may relieve it. The
  dead-locker screen says how to unlock from a virtual terminal: `loginctl unlock-session`.
- **The locker unlocks; logind's `Unlock` does too when no locker lives.** Yuna takes the signal
  only from the sender that owns logind's name, and only for its own session; while a locker
  lives, Yuna leaves the signal to it.
- **The lock's state is on Hollow's interface**: locked or not, and whether a locker lives, given
  whole at connection and as events.
- **At Stage 2b**, with the lock ([ADR-0081](0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md)).
  The D-Bus library is chosen by that milestone ([ADR-0040](0040-the-wire-protocol-stays-wayland-rs.md)).

## Alternatives considered

- **Everything but logind's `Unlock`**: the strictest, only a locker unlocks; but a locker that
  crashes at each start leaves only a reboot, which Hollow's ADR-0016 refused.
- **The lock begun by the locker**, Yuna holding sleep back until its first frame: less in Yuna,
  but the session shows between the command and that frame, and a lock and a relief are two
  paths.

## Consequences

- Hollow's recovery from a virtual terminal holds once Stage 2b builds this.
- Yuna listens to logind, which Stage 2b's session needs anyway.
- A process of the owner's user may unlock a session whose locker is dead, as far as polkit lets
  it call `loginctl unlock-session`. The owner's agents run as that user: Stage 2b checks what
  polkit allows an agent's service, and if it allows the unlock, a polkit rule for the owner's
  NixOS configuration is put to the owner.
- Whether a locker alive but frozen counts as dead, after a delay, is left open: Hollow has not
  decided it, and it is put to the owner apart.

## Validation and reconsideration

Checked at Stage 2b, in headless Yuna with a fake logind on a private bus: a locker killed while
locked, no frame of the session presented, the named locker relieving it and no other client; a
forged `Unlock` from another sender refused; `Unlock` while a locker lives left to it; a sleep
right after the lock command, the black frame presented on every output before the inhibitor is
released; a crash while locked, Yuna back locked. Proposed again if polkit lets another user
unlock the session, or if Hollow withdraws its ADR-0016.
