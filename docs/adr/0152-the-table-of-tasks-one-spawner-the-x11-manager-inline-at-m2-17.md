# 0152. The table of tasks: one spawner, one class, the X11 manager inline at M2.17

- Status: Accepted
- Date: 2026-11-22

## Context

[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 6 asks
for a table of tasks: each thread or task with an owner, a bounded inbox, its behaviour when full,
its place in the stop order, a failure contained or fatal, and the ability to run inline under the
manual clock. [ADR-0135](0135-priority-against-a-game-amds-master-override-turned-on-at-2b-on-a-measure.md)
adds a scheduling class per task. The PLAN puts the table at M2.4.

Yuna's threads at `5b9fa13`:

- **The state thread** runs the event loop; its panic is fatal.
- **`yuna-x11-<seat>`**, the X11 window manager, one per running Xwayland.
  - Its inbox is a `Mutex` the state thread also locks, bounded at 8 MiB; past the bound Yuna ends
    that Xwayland.
  - Its outbox is a channel of 256 events; when it is full the thread blocks, and Yuna reads in
    slices.
  - It makes 17 synchronous X11 round trips.
  - Its transfers' and drags' time limits read `Instant`, the real clock, also when Yuna runs on
    the manual clock.
  - It is never joined.
- **`yuna-capture`**, at most four PNG encoders in `yuna mcp`.
  - Results come back on an unbounded channel, which `MAX_ENCODERS` bounds in practice.
  - The encoders are never joined.
- **`yuna-wlcs-server`**, WLCS's module thread, on the real clock by
  [ADR-0149](0149-wlcs-nixpkgs-1-8-1-a-shim-on-the-interface-every-tests-outcome-expected.md).

On the real clock, Yuna's own timers read `CLOCK_MONOTONIC`, as `Instant` does. The state thread
waits without a bound in `kill_and_reap`, also while it serves. The parity script counts the
context switches of the threads named `yuna-…` ([debt](../debt.md)); Linux keeps 15 bytes of a
thread's name.

Running the X11 manager inline means its round trips become replies it waits for without
blocking: that is M2.17's work, "the X11 manager, a task". Inline in tests alone, with blocking
round trips, would give tests a path production lacks (ADR-0082 item 29), and an X11 client that
grabs the server would freeze Yuna's whole loop. Xwayland itself waits on the compositor only
before it reports ready (Xwayland 24.1.15's sources), so it would not deadlock.

The two retry stories (debt) race the manager's thread against the story; no clock removes that
race.

## Decision

The owner chose, on 2026-11-22:

- **One thread spawner and one table**, in `yuna` until `yuna-core` exists. Every thread Yuna
  starts goes through one function that names it `yuna-…` within 15 bytes, and through the table.
  The table gives each task its owner, its inbox and bound, its behaviour when full, its place in
  the stop order, whether its failure is contained or fatal, its scheduling class and its clock.
  The spawner of programs, which clears their capabilities, is another: M2.18's.
  - Clippy's `disallowed-methods` refuses `std::thread::spawn`, `Builder::spawn`,
    `Builder::spawn_scoped` and `Scope::spawn` elsewhere, as the rest auditor's sources are
    refused ([ADR-0096](0096-a-rest-auditor-naming-what-wakes-yuna-or-makes-it-draw.md)).
  - Test crates, examples and `yuna-bench` allow it at their root, and test modules inside
    `src/` expect it at theirs: their threads are not Yuna's.
  - `yuna-wlcs`'s thread is a harness, kept out of the core's table type, and listed in the
    table's documentation. Its one spawn expects the lint, with its reason. Its name,
    `yuna-wlcs-server` (ADR-0149), is 16 bytes: Linux keeps `yuna-wlcs-serve`, which the
    `yuna-…` prefix still matches.
- **One scheduling class, normal, for every task now.** An agent's Yuna is never lowered for a
  game (ADR-0082 item 24, [ADR-0115](0115-priorities-uwui-first-the-owners-activity-measures-that-check-their-load.md)),
  and `yuna mcp`'s encoders serve an agent's Yuna. Instead, the encoders are capped at the
  smaller of `MAX_ENCODERS` (4) and the machine's cores less one, at least one, so that two cores
  keep one for the state thread. The
  KMS thread's class comes at 2b (ADR-0135).
- **The X11 manager keeps its thread until M2.17.**
  - On the real clock it keeps `Instant`, the same clock as Yuna's.
  - On the manual clock it reads the time Yuna sends it at each advance and with each batch of
    commands. It tells Yuna its next limit, so that an advance passes it. This pays the debts of
    the transfers' and drags' limits.
  - What remains is said: a stamp may lag by one step, and a limit's effect comes after the
    advance that passed it returns.
  - Running it inline, and the two retry stories, move to M2.17, with its shared lock and its
    blocking reap while Yuna serves. At M2.17 it runs the same way in production and in tests.
- **No inline switch now.** An encoder is a pure function a test calls. The switch between a
  thread and inline comes with the first task that needs it (M2.17's manager, 2b's KMS thread),
  chosen per task, apart from the clock, as Mutter's KMS thread is.
- **The stop order.** Encoders are joined within a bound. Each X11 manager is released, its
  Xwayland killed, and then the manager is joined within a bound. Reaping waits on the pidfd within
  a bound before `waitid`. A thread past its bound is detached and a warning logged; a process past
  its bound is left to its pidfd's source, or to the process's end at exit. Every other handle is
  joined, or detached with its reason written. The order at a session pause and at a GPU loss is
  "none" until 2b brings both.
- **Bounded in their types.** The encoders' results channel is bounded by `MAX_ENCODERS` in its
  type.

- **What item 6 asks, and when.** Item 6 holds whole. M2.4b meets the table, the owners, the
  bounded inboxes and their behaviour when full, the stop order at exit, and each failure. M2.17
  meets the X11 manager's inline running, its inbox shared with the state thread, and the state
  thread's blocking reap while it serves. 2b meets the stop order at a pause and at a GPU loss,
  and the keeper's watchdog.

## Alternatives considered

- **The manager inline now**, its round trips made pending replies: deterministic X11 stories at
  once, but M2.17's core pulled into M2.4 on code M2.17 rewrites.
- **Inline in tests only**: a path tests alone take, and a stalled X server stalls all of Yuna.
- **The manager's time sent by Yuna on the real clock too**: one path, but transfers stamp their
  progress on the manager's own pipe events, so a transfer moving while Yuna sends nothing would
  be cut.
- **Encoders at a lower priority** (`SCHED_BATCH` or a nice), so that a capture never costs a game
  a frame: against item 24, and `SCHED_BATCH` needs a system call rustix lacks.

## Consequences

- M2.4b builds the spawner, the table in the core's documentation and code, the clippy rule and its
  `test-check.sh` case, the manager's manual time, the bounded joins and reap, and the encoders'
  cap.
- The PLAN and the debt say that the manager runs inline at M2.17, with the two retry stories.
- The parity script's filter by `yuna-…` names then holds for every thread of Yuna's.

## Validation and reconsideration

Evidence: the threads' inventory at `5b9fa13`; Xwayland 24.1.15's sources; Mutter 51's KMS thread
switch; a challenge of the first draft on 2026-11-22. Checked by a `test-check.sh` case where a
bare `thread::spawn` fails the lints, and by the X11 drag and transfer stories stepping the manual
clock past their limits. Proposed again if a task needs a class other than normal before 2b, or if
M2.17 finds the manager cannot run inline.
