# Audit of the architecture, 2026-11-17

The owner asked whether Yuna's foundations are sound, understandable, stable and maintainable,
what was forgotten, and which decisions must come early to avoid costlier changes later; and
whether an outside contributor can work on it.

## Scope

- **Code and documents** at `be11269` (M1.9 merged): `crates/yuna`, `crates/yuna-layout`,
  `AGENTS.md`, `README.md`, `CONTRIBUTING.md`, `docs/` and the ADRs, `scripts/check.sh` and the
  workflows. The owner's global rules and project method as distributed on 2026-11-16.
- **Flows traced** in the code: a window's life, a commit to its presentation, input to focus, a
  capture to MCP's answer, a seat created and closed with its programs.
- **Dimensions**: architecture and evolvability, stability and recovery, contracts, the
  contributor's path. Not covered: performance and rest, measured again by no one here.
- **Evidence**: reading the code and the documents, the line cited; two delegated readings of the
  code; one challenge of the recommendations by a fresh agent on the strongest model. Nothing was
  built, run or measured during the audit; the figures cited are earlier ones, from
  [measurements](../measurements.md).

## Verdict

- The headless core is sound for Stage 1: a pure layout model under property tests, a clock that
  owns every deadline, rest checked on the program, about 290 tests of the compositor that speak
  the real protocol, misbehaving and random clients, real applications in CI, per-client bounds,
  and no `unwrap`, `expect` or `panic!` in production code.
- The compositor has Smithay's shape, one `State` that every handler changes; ADR-0039 and
  [ADR-0074](../adr/0074-the-requirements-of-stage-2s-core.md) already replace it at Stage 2.
  Refactoring it now would be wasted: the `Compositor` facade and the protocol-level tests are the
  boundary worth keeping.
- What made Stage 2 risky was not today's code but three gaps: no path from today's core to
  Stage 2's, a stage of about seventy items with one exit, and robustness promises written
  without conditions; all three are now decided, below.
- Outside contributors can build, test and read Yuna, but the process is written for agents and
  the owner. M1.12 already plans most of the fix.
- Not knowable without running: stability over long runs, how often a dependency panics, the
  behaviour after a lost GPU context, the tests' fragility under load, and anything on hardware.

## Findings, by cause

Severity first, then urgency; each with where it is followed.

1. **No migration path to Stage 2's core** (highest severity, due at Stage 2's opening).
   ADR-0039 says the program works at each commit on main; ADR-0074 wants its crates and
   requirements from the core's first commit; leaf protocols moved two globals in nine
   milestones, both still keeping their state in Smithay's surface data. Smithay's handlers find
   their state in Smithay's surfaces, so `wl_compositor`, subsurfaces and the commit queue can only
   move last, at once. A path is needed: harden the parity net on today's core, a temporary
   boundary implemented over Smithay first, the scene and renderer behind it, the protocols from
   the top down, the root swapped behind a build switch, Smithay removed; each step merged with
   the tests unchanged, a WLCS baseline, the applications and the budgets no worse, reverted when
   one regresses. Decided:
   [ADR-0081](../adr/0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md).
2. **Stage 2 is one stage with one exit** (high, due at Stage 2's opening). Its own core, its
   Vulkan renderer hosting uwui, DRM, input and the session, and about seventy features end on
   "a week in place of niri". Real hardware's first answer comes after everything rests on the
   core. Proposed: 2a, Yuna's own base headless, Smithay gone; 2b, a desktop the owner can log
   in to; then the owner's choice of scope; 2c, the features. Decided:
   [ADR-0081](../adr/0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md).
3. **Robustness promises without conditions** (high). Most of the owner's applications do not
   reconnect; a GPU reset may lose the windows' pixels; only an unwinding panic can be caught in
   one process; nothing caught a panic but the captures' encoders. Decided:
   [ADR-0080](../adr/0080-what-survives-a-failure-and-yunas-own-panics.md).
4. **Several agents' seats in one Yuna lost their reason** when ADR-0060 removed Stage 3, while
   every milestone adds to them. Decided:
   [ADR-0078](../adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md).
5. **Contracts without a policy**, just before M1.10 gives Yuna consumers in other repositories.
   Decided: [ADR-0079](../adr/0079-yunas-contracts-in-0-x-changes-announced-never-silent.md).
6. **ADR-0074 holds a few bets among its constraints** (medium, at Stage 2's opening). Most of it
   is what rivals paid to add late, and stays. Proposed for the owner: keep the crate map as
   rules on edges and create each crate when its code exists (`yuna-ipc` apart); reduce multi-GPU
   to "each buffer names its device" and GPU memory to counting; defer copies between GPUs,
   eviction, the message catalogue, the fallback to the previous binary (on NixOS, holding the
   running build is simpler) and "ten instances at full rate" as an exit, which a headless test
   without `--gpu` cannot pass. Two edges of the map are wrong already: input needs the session's
   devices, and that D-Bus call would block the state thread. Decided:
   [ADR-0082](../adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md).
7. **Two rules pull against each other on `wl_shm` buffers**: releasing them once uploaded
   ([debt](../debt.md#architecture)) against keeping a copy to recover from a GPU reset.
   Decided: [ADR-0083](../adr/0083-a-windows-last-wl-shm-buffer-kept-until-its-next.md).
8. **uwui's hosting has no contract**: its task, its own commands, a timeout on its GPU work, its
   accessibility tree. ADR-0073 lends it the host's encoder, which leaves no way to time it out;
   uwui wins every conflict (ADR-0003). Decided:
   [ADR-0084](../adr/0084-uwui-hosted-on-its-own-task-commands-and-images.md), uwui's side to
   accept.
9. **Bounds stated but not enforced, and smaller drifts** (low): the encoders' bound, the
   unbounded log of a failing frame, retired globals on real time, `keyboard tap` on an error,
   SlimeEngine's path list, two sources for a window's output, GLES beyond `render.rs`. Each is
   in the [debt](../debt.md) with the milestone that pays it.
10. **The documents outgrow the code** (medium, M1.12): `ARCHITECTURE.md` went from 9,200 to
    17,800 words in two days and repeats the code's constants; amendment chains and references
    to the removed Stage 3 remain. M1.12 already consolidates the chains; it should also bring
    `ARCHITECTURE.md` to one page of parts, flows, invariants, ownership and failure boundaries,
    the detail in each module's documentation.
11. **The outside contributor's path**: no process for a pull request from outside, no
    `SECURITY.md` (planned at M1.12), dependencies "asked of the owner", ADR numbers reserved by
    hand between sessions. Open decision D8: whether outside contributions are taken before 0.1.

## Scenarios

| Scenario | Result |
|---|---|
| A new leaf protocol or layout policy | One module, or `yuna-layout` alone; tested by the random requests and the scenarios |
| Replacing part of Smithay | About 35 sites read Smithay's surface data; the protocol-level tests are the net; no path (finding 1) |
| A window moved between outputs of different scales | Scale told before the configure, tested; two sources for its output (finding 9) |
| An output or seat removed during a capture | Captures failed, waiting calls answered; an encoding finishes and is dropped |
| A client's invalid or excessive requests | That client alone, by ADR-0028's bounds; tested; not fuzzed yet |
| Saturated captures, MCP, logs, processes | Bounded, but the encoders' bound and the server's logs are not (finding 9) |
| A cancelled call, a session closed mid-work | Answered or dropped; waiting calls get no answer when Yuna stops |
| A lost GPU, headless | Every frame fails and logs; windows kept; no recovery (Stage 2) |
| A panic of Yuna | The process stops; `yuna mcp` left its programs (ADR-0080 pays it at M1.11) |
| A configuration reload, an interrupted write | No configuration or saved state exists yet (ADR-0074, Stage 2) |
| Repeated creation and destruction | Known growth: Smithay's seat records, tokens of clients gone; no soak test exists |

Isolation of focus and input between seats is not a security boundary: seats share a user, and
a program may reach another seat's socket (ADR-0078).

## Decisions

Taken by the owner on 2026-11-17:

- [ADR-0078](../adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md): several seats
  in one Yuna frozen; M1.11 measures N Yunas against N seats; Stage 2's core decides.
- [ADR-0079](../adr/0079-yunas-contracts-in-0-x-changes-announced-never-silent.md): the contracts
  listed; in 0.x every change announced; `tools/list` and `--help` held by tests.
- [ADR-0080](../adr/0080-what-survives-a-failure-and-yunas-own-panics.md): a promise for each
  kind of failure; a panic of the state thread fatal, of a contained task caught.
- [ADR-0081](../adr/0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md): Stage 2 in
  three gated parts, Smithay left along a written path, parity at every step.
- [ADR-0082](../adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md):
  ADR-0074's requirements kept, its bets made rules of interfaces or deferred, its map's two
  faulty edges corrected.
- [ADR-0083](../adr/0083-a-windows-last-wl-shm-buffer-kept-until-its-next.md): a window's last
  `wl_shm` buffer kept until its next, so that a GPU reset loses no window's content.
- [ADR-0084](../adr/0084-uwui-hosted-on-its-own-task-commands-and-images.md): uwui on its own
  task, commands and images, Yuna compositing the last image done; its accessibility tree out.

Open, in order, each put to the owner one at a time:

| | Decision | Due before |
|---|---|---|
| D8 | Outside contributions before 0.1, and how they are reviewed | M1.12 |

## Fixed during the audit

- `scripts/adr-index.py` reads an ADR superseded in part, as the project method now writes it,
  with its test.
- `render.rs`, `lib.rs` and `ARCHITECTURE.md` no longer say that `render.rs` alone knows GLES.
- The ADRs the decisions supersede, `AGENTS.md`, the ROADMAP, the PLAN, `CONTRIBUTING.md`, the
  debt and the code's references follow ADR-0078 to ADR-0080.
