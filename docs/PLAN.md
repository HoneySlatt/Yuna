# Plan

What is being worked on now: the milestones of the current stage, their status and how each is
checked. Direction beyond this stage is in [`ROADMAP.md`](ROADMAP.md); decisions are in
[`adr/`](adr/README.md). A *stage* is a step of the roadmap; a *milestone* is a small, verifiable
step of a stage.

Every milestone ends with the definition of done in [`AGENTS.md`](../AGENTS.md#definition-of-done).

## Current

- Stage 1 is done, 0.1 tagged `v0.1.0` on 2026-11-19 ([history](history.md)). No 0.1.1: the
  owner chose on 2026-11-19 to carry `yuna mcp`'s missed start budgets and motion's judgement into
  M2.1.
- Stage 2's opening design is done (2026-11-19): ADR-0133 to ADR-0141, 2a's milestones below,
  the debt placed ([debt](debt.md)).
- **Done on 2026-11-21/22**: M2.1 (ADR-0142, ADR-0143); M2.2 (ADR-0145 to ADR-0148); M2.3
  (ADR-0149, ADR-0150); M2.4's design (ADR-0151 to ADR-0155); M2.4a; M2.4b, whose timed parity
  run waits for the owner's go (ADR-0142).
- **M2.4c, one event stream and the pidfd**, done on 2026-11-25, with ADR-0156 (the pidfd
  checked by `poll`) and ADR-0157 (the geometry and `WindowShell` in `yuna-types`): the
  refusals' sentences in `yuna-ipc` alone, the pidfd at accept and each client's program kept,
  `windows` paged, the event stream from marks with its oracle. Its timed parity run and
  `events_look`'s release figures wait for the owner's go. **Next: M2.4d, a frame's identity**,
  in a new session.
- The owner works on `main` directly: push only a state where `scripts/check.sh` passes, and
  follow CI to green after each push.
- The owner's desktop runs niri since 2026-11-16: Stage 2's goal is to replace it.

## Stage 2a — Yuna's own base, headless

Along [ADR-0139](adr/0139-stage-2-in-three-gated-parts-2as-path-revised.md)'s path. Every
milestone keeps parity (ADR-0139): the tests unchanged in what they assert, WLCS's passing tests
kept, traces and captures compared, the baselines' figures no worse; a step that regresses is
reverted. New code meets [ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
from its first commit. Each Smithay behaviour [debt](debt.md) lists becomes a test in the
milestone that replaces its module. A new dependency is put to the owner at its milestone's start.

- **M2.1 Baselines.** A parity script: at each push in CI, figures the load does not move
  (callgrind's instructions for a frame and the layout's operations, `Pss`, wakes, DHAT's
  allocations a frame, ADR-0143); in an agent's runs on the GPU, DRM memory; once per milestone, timed runs
  interleaved, with a build against itself and an injected slowdown as controls; start time, GPU
  memory, a partial-damage frame, a soak. Protocol traces of scripted clients, times kept on the
  manual clock. Captures from a buffer the processor maps. `yuna mcp`'s start at M1.11's commit
  and main, side by side; motion judged on a quiet machine; the same 4K scene on llvmpipe and on
  the Vulkan prototype's lavapipe. The tests the debt says are missing. *Done when* the script
  flags the injected slowdown and passes main against itself.
- **M2.2 The interface.** `yuna-types`; the `Compositor` interface free of Smithay's types;
  `yuna-ipc`'s command type in place of `change_layout`; direct dependencies in place of
  Smithay's re-exports; the tests' helpers made one; `bool::toggle` and the one `DropGuard`
  (ADR-0140). Then, in its own commit with its story, 1 MiB per client and a bound on the clients
  a seat connects ([ADR-0136](adr/0136-a-client-that-stops-reading-1-mib-and-its-motion-merged.md)).
  *Done when* no test or example names a Smithay type and M2.1's figures hold.
- **M2.3 WLCS.** `yuna-wlcs`, WLCS in CI, its passing tests recorded as a list. *Done when* CI
  fails if a passing test fails.
- **M2.4a The map's checks and the contracts.** ADR-0151: `unsafe_code` denied, expected per
  module; `crates/map.toml` and `scripts/crate-map.py` check the edges, the outside libraries'
  owners, the `unsafe` counts and every global by name; `thread_local!` refused by clippy; `yuna`
  transitional, its lists and its files naming Smithay only shrinking; `forbid(unsafe_code)` in
  every pure crate; the two debug `OnceLock`s gone. Every public contract listed with its version
  (ADR-0079); `Refusal` a variant per reason, the layout's included. *Done when* a wrong edge, a
  second owner of an outside library, `unsafe` added to a listed module or in an unlisted one, an
  unlisted `static` or `static mut`, an aliased `thread_local!` and rustix in a level-1 crate each
  fail the check, and no `Refusal` holds text.
- **M2.4b The table of tasks.** ADR-0152: one spawner, every thread `yuna-…` in 15 bytes, the
  table with each task's owner, inbox, bound, stop order, failure, class (normal) and clock;
  encoders capped at the cores less one; joins and reaps bounded; the X11 manager on Yuna's manual
  time, its inline running and the two retry stories at M2.17. *Done when* an unregistered thread
  fails the check and the X11 drag and transfer stories step the manual clock past their limits.
- **M2.4c One event stream and the pidfd.** ADR-0153 and ADR-0157: marks at the change points,
  `yuna_ipc::Event` built once a round, MCP's log on it numbered per seat, `windows` paged, the
  comparison a test oracle. ADR-0156: `SO_PEERPIDFD` at accept, the program resolved from its
  record. *Done when* a window opened and closed in one round is told, a title changed a thousand
  times in a round is one event, `missed` is exact, `windows` pages, and a client's program comes
  from its pidfd, or from today's walk marked inexact when the kernel refuses it.
- **M2.4d A frame's identity.** ADR-0155: an opaque `FrameId`, rings of inputs, commits and
  frames, the `frames` tool, exact causes, hidden windows by due time, wakes naming their task or
  X11 client, the GPU priority granted logged. *Done when* a story reads a frame's inputs,
  commits, causes and presentation through `frames`, a re-aimed refresh keeps its id, a `Waker`'s
  and an X11 wake name their task or client, the priority granted is logged, and the parity
  figures hold or the baseline moves by a named commit.
- **M2.5 The boundary.** Its contract first, an ADR, put to the owner: roles, each module's
  double-buffered state applied at commit, checks before a commit, notices after, a read-only
  view and typed requests; the queue's crate placed in the map. Implemented over Smithay's cached
  state; colour management and the X11 shell moved onto it. The layout's index and a faster
  hasher, measured. *Done when* both modules keep no state in Smithay's surface data.
- **M2.6 The queue and the store, alone.** A pure crate: blockers as descriptors, transactions
  with their timeout, the bounds of what ADR-0131 leaves unbounded; property tests. *Done when*
  the column of three and the late fence pass as scenarios on the model.
- **M2.7 The scene, alone.**
  [ADR-0133](adr/0133-yunas-scene-retained-its-damage-found-at-each-latch.md) whole: batches,
  views, latches, the lock, cursors and drag icons, output transforms, the colour steps' shape.
  *Done when* each requirement is a test and the latch is measured at 15, 150 and 1,500 drawn
  windows and 10,000 on one strip.
- **M2.8 The Vulkan renderer, behind a switch.** The binding and the shader compiler put to the
  owner first. It draws the scene, blends as
  [ADR-0134](adr/0134-blending-gamma-2-2-everywhere-its-precision-set-by-the-content.md) says,
  uploads damaged regions, counts GPU memory, names each buffer's device, precompiles its
  pipelines, takes its queue priority as a parameter and leads its deadline from the GPU's
  timestamps ([ADR-0135](adr/0135-priority-against-a-game-amds-master-override-turned-on-at-2b-on-a-measure.md));
  GLES the default. *Done when* ADR-0133's pixel test and ADR-0134's ramp test pass on lavapipe,
  an agent's script finds RADV and lavapipe alike, and an `A2B10G10R10` composition is timed per
  tier.
- **M2.9 Vulkan the default.** GLES, EGL and their globals removed. *Done when* the processor
  tier meets its budget, Yuna's own code allocates nothing a frame, and the start budget holds.
- **M2.10 Captures and the GPU's loss.** Readback off the loop; `wlr-screencopy` with dmabufs and
  damage; the GPU lost on demand; amdgpu's hang of another device, an owner's run. *Done when* a
  capturing agent loses no frame against Stage 1's 6 % and a capture fails retryably on a loss.
- **M2.11 uwui hosted, against a stub.** Yuna's side of ADR-0106. *Done when* a stub client's
  images, semaphores, timeout, panic, hang and GPU budget are tested.
- **M2.12 Seats and input.** One seat per Yuna, `seat create` starting a sibling Yuna, its front's
  ADR put to the owner first
  ([ADR-0138](adr/0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md)); one input chain;
  a keyboard state per device; keymaps compiled off the state thread; drags and their grabs,
  X11's included; a congested client's motion merged, its departure named in MCP; fuzz targets.
  *Done when* the frozen game's scenario passes and ten agents' Yunas meet their budgets.
- **M2.13 xdg-shell and popups.** Yuna's own, on the boundary; reactive popups; menus across
  outputs. *Done when* Smithay's popup behaviours the debt lists are tests that pass.
- **M2.14 Selections and text.** Data devices, data control filtered per client, the X11 bridge,
  text input bounded. *Done when* their stories and the D-Bus fuzz target run.
- **M2.15 The other protocols.** `linux-dmabuf` with a scanout tranche, presentation, viewporter,
  fractional scale, activation, the captures' protocols; per-client budgets. *Done when* the
  damage flood scenario passes.
- **M2.16 The store, wired behind a switch.** Both stores built and tested in CI. *Done when*
  recorded request streams give the same results in both and memory a window meets its budget.
- **M2.17 The X11 manager, a task.** Its watchdog, bounds and restacking; its round trips
  answered without blocking, so that it runs inline on the manual clock, and the state thread no
  longer locks its inbox nor waits on its reap (ADR-0152). *Done when* its debt's stories pass on
  the own core, the two retry stories without retrying.
- **M2.18 The agents' runtime.** In `yuna-process`: a spawner that clears ambient capabilities,
  programs ended with their Yuna, pidfds, folders swept, replies bounded in bytes, accessibility
  for AccessKit, corrected coordinates. *Done when* a killed `yuna mcp` leaves no program and
  Codex and OpenCode run its story.
- **M2.19 The switch, and Smithay removed.** Yuna's store the default a week under agents' use;
  Smithay, its fork and `smithay-patches.md` removed. *Done when* no `State` shared by handlers
  remains, ADR-0139's gate scenarios pass, and 2a's gate review is done.

M2.18 may run any time after M2.4d. A milestone that turns out too large is split with the owner.

## Status

| Stage | Goal | Status |
|---|---|---|
| [1](history.md#stage-1--the-headless-core--01) | Agents and tests drive Yuna without a screen; every layout behaviour a test | Done, 0.1 |
| 2a | Yuna's own base, headless ([ROADMAP](ROADMAP.md)) | M2.1 to M2.4c done; M2.4d next |
| 2b, 2c | A desktop to log in to; the features | After 2a's gate |

## Ideas

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
- Each of `yuna`'s entries in `crates/map.toml` naming its ADR, which the map's script checks
  exists, so that the check refuses an entry added without one.

## Known debt

In [`debt.md`](debt.md).
