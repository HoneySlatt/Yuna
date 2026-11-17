# 0080. What survives a failure, and Yuna's own panics

- Status: Accepted; supersedes [0007](0007-surviving-a-crash.md); supersedes in part
  [0058](0058-what-yuna-takes-from-the-owners-other-projects.md) (the GPU reset) and
  [0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)
  (uwui's crash boundary)
- Date: 2026-11-17

## Context

[ADR-0007](0007-surviving-a-crash.md) promised a restart within a second, no window lost of an
application that reconnects, and the others reopened in place.
[ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md) promised recovery from a GPU
reset keeping every window, and
[ADR-0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)
that a failure of uwui inside Yuna never takes it down. `AGENTS.md` said that one client's error
never takes Yuna down. Written without conditions, several cannot hold:

- Of the owner's applications, only Qt's reconnect to a compositor that restarts
  (`QT_WAYLAND_RECONNECT`); GTK's (Firefox, Brave), Electron's, SDL's, Steam and Wine do not
  ([survey](../survey.md#surviving-a-crash)). Every X11 program ends with its Xwayland, and a game
  in gamescope with it. "Reopened in place" means started again: what was not saved is lost.
- A GPU reset that loses video memory loses the clients' buffers and Yuna's own images: the
  windows survive, their pixels until each client draws again. KWin recovers from resets this
  way, and still fixes crashes on that path in 6.8.
- Inside one process, only a Rust panic that unwinds can be caught. An abort (memory exhausted,
  a stack overflow, a panic while unwinding), undefined behaviour in `unsafe` code, a GPU hang or
  an endless loop cannot.
- Today nothing catches a panic but the captures' encoders: a panic on the main thread stops
  Yuna, and `yuna mcp` then leaves its programs running and its folder behind
  ([debt](../debt.md#the-mcp-server)). Its dependencies panic in documented cases (wayland-backend
  when a request's new object is destroyed before the request returns).

[ADR-0074](0074-the-requirements-of-stage-2s-core.md) already gives every task of Stage 2's core
a failure either contained or fatal, a keeper that restarts Yuna when it crashes or hangs, and a
lock kept through a restart. [ADR-0061](0061-yunas-uses.md) brings windows back in place after a
reboot too; that stays its own.

## Decision

The owner chose, on 2026-11-17, at an audit of the architecture: **each kind of failure has its
own promise, its conditions and its test.**

| Failure | What survives | Checked by |
|---|---|---|
| A client's protocol error, or a request over a bound | That client alone is ended or refused ([ADR-0028](0028-bounds-on-what-one-client-may-ask.md)); every other client goes on | the misbehaving and random clients' tests, now |
| Yuna crashes: a panic of its state, an abort, a native error | On the desktop, Yuna is back within a second, without a black screen, the keeper holding the DRM device and the session. The windows of applications that reconnect are kept. Every other Wayland application, every X11 program and every game ends; those that restore their session (`xdg-session-management`), or whose command Yuna knows, are started again in place. What an application had not saved is its own | a test that kills Yuna and counts, by kind, what survives and what starts again |
| Yuna crashes while locked | It comes back locked | a test that kills Yuna |
| Yuna hangs | The keeper's watchdog restarts it, as a crash; the watchdog is armed only while Yuna has work, so that nothing wakes at rest | a test that hangs Yuna |
| The GPU resets | No restart: every window is kept, and its content comes back when its client draws again, or from Yuna's own copy where it keeps one. A client that cannot survive its own loss ends; the session stays | the GPU lost on demand, headless; a real reset by the owner |
| The GPU stays lost | The keeper's restart, as a crash | a test |
| uwui panics inside Yuna | The interface that panicked closes and is built again; frames go on | a test |
| uwui aborts, hangs or loses the GPU | A crash, a hang or a GPU reset of Yuna, as above | the tests above |

**Yuna's own panics:**

- **A panic of the state thread is fatal.** Yuna is built to unwind, and a panic hook writes the
  crash report ([ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)); when the
  panic comes from the state thread, Yuna then stops at once, never running on a state left half
  changed, and the keeper restarts it.
- **Only the tasks marked contained catch their panics**: today the captures' encoders; from
  Stage 2, the tasks that [ADR-0074](0074-the-requirements-of-stage-2s-core.md)'s table marks
  contained, uwui on a task of its own among them. Each fails its work, says so, and starts
  again.
- **From Stage 1, the fatal path ends what Yuna started**: before it stops, `yuna mcp` ends its
  programs and removes its folder, as when it stops on purpose.
- `AGENTS.md`'s rule reads: one client's protocol error ends that client alone; a panic of Yuna's
  state is fatal and Yuna comes back by its keeper.

## Alternatives considered

- **The promises as written**, kept as a goal: the ambition shown, but the bar holds criteria
  that always fail, and the keeper, the renderer and uwui's hosting would be designed for them.
- **Robustness alone**, a quick restart without session restore or recovery from a GPU reset:
  far less work at Stage 2, but a GPU reset, common on Linux, ends the owner's whole session, as
  ADR-0058 had refused.
- **`panic = "abort"` everywhere**: the simplest, nothing runs on a half-changed state, but no
  panic can be caught anywhere: a panic of uwui or of an encoder would end Yuna, unless uwui ran
  in a process of its own, which ADR-0073 refused.
- **Each client's panic caught where its requests are dispatched**, the client ended and Yuna
  going on: the most a crash could spare, but with today's shared state a panic can come halfway
  through a change across modules, and going on risks a silent corruption, worse than a restart.

## Consequences

- `AGENTS.md`, the ROADMAP's bar and its fixed decisions say what this table says.
- M1.11 delivers the fatal path of `yuna mcp`: its programs ended and its folder removed on a
  panic of its state, with a test that makes Yuna panic. A `yuna mcp` killed by SIGKILL still
  leaves them ([debt](../debt.md#the-mcp-server)).
- Stage 2's keeper, its watchdog, session restore and the GPU's recovery are measured against
  this table; uwui's hosting gives uwui a task, its own commands and a timeout on its GPU work,
  so that Yuna can draw uwui's last good image without waiting for it.
- Whether Yuna keeps its own copy of `wl_shm` buffers, which a GPU reset would need, against
  releasing them once uploaded ([debt](../debt.md)), is decided with Stage 2's renderer.

## Validation and reconsideration

Evidence at the time: the code read at the audit (no panic hook, no catch but the encoders'), the
survey's facts on reconnecting toolkits, KWin's reset path and its open crashes, and Rust's
unwinding rules. Reviewed reasoning, not measured: the second of a restart on the desktop
depends on the keeper holding the DRM device, which Stage 2 checks. Proposed again when Stage 2's
core applies a client's changes only after its dispatch, through typed requests
([ADR-0074](0074-the-requirements-of-stage-2s-core.md)): a panic in one protocol module could then
drop that client's requests with the state untouched, and each client's panic be caught safely.
