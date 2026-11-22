# 0149. WLCS: nixpkgs' 1.8.1, a shim on the interface, every test's outcome expected

- Status: Accepted
- Date: 2026-11-21

## Context

[ADR-0139](0139-stage-2-in-three-gated-parts-2as-path-revised.md) has WLCS run against Yuna before
the core moves. WLCS is the Wayland conformance suite of Canonical's Mir. Its baseline is the list
of tests that pass, and parity keeps that list. Smithay's CI runs four of WLCS's groups, and
Yuna's own core must match at least that ([ADR-0039](0039-leaving-smithay-step-by-step.md)).
[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 22 names
a WLCS shim among the test seams. M2.3 builds it.

The facts, read on 2026-11-21 (the survey's local proofs are in `.delegation/m2.3-*`):

- **What WLCS is.**
  - It is a program that loads a compositor's *integration module*, a shared library, into its
    own process, and calls the module's C functions.
  - Through them it starts and stops a compositor for each test, connects clients through a
    socket the module returns, places a window at absolute coordinates, and moves a pointer or a
    touch.
  - Its tests run one after another in that one process, so a crash of the compositor ends the
    run.
  - It sets a test aside as *skipped* only when the module's descriptor omits the protocol the
    test needs; otherwise the test fails.
  - Its exit code ignores skips, and is 0 when a filter matches nothing.
- **Its version and licence.**
  - Its latest release is 1.8.1 (2025-08-14), the commit Smithay pins. nixpkgs packages it, at the
    revision Yuna's `flake.lock` pins.
  - Its `main` branch is 316 commits ahead, and adds a keyboard hook and 68 tests on protocols Yuna
    offers.
  - Its code and its headers are GPL-3.0-only. Yuna is GPL-3.0-or-later, so the two may run as one
    program, which is never shipped. Its rules for skipping tests are the same on `main`.
- **What others do.**
  - Only Mir and Smithay run WLCS in CI, as far as their public sources show.
  - Smithay runs four filtered groups, and keeps no list.
  - Mir lists 33 tests it expects to fail, and fails its build when one of them passes.
  - Smithay's module, `wlcs_anvil`, sends each request to its compositor without waiting for it,
    and leaves touch empty.
- **What Yuna's interface lacked.**
  - It had no way to wake a blocked `dispatch` from another thread.
  - It had no identity for a client, and no lookup of the window of a client's `wl_surface`.
  - It had no way to move a floating window.
  - It opens new windows tiled, while WLCS places client-sized windows freely.
  - It has no touch.
- **What the design's challenge found.** Its local proofs are `.delegation/m2.3-challenge`.
  - Yuna hands keyboard focus and activation at the next refresh, while WLCS asserts right after
    one roundtrip, so a module that answers before Yuna has refreshed makes tests flaky by
    construction.
  - Tests that only assert that nothing reached a surface pass against a touch that does nothing.
  - Making a tiled window floating to place it changes its size. Under Stage 2's transactions, it
    would wait for the very client that blocks on the placement.
- **The first run of the module**, at M2.3.
  - WLCS lists 1,228 tests. 3 are disabled, so 1,225 run.
  - gtest reported 177 passed, 716 skipped (protocols Yuna does not offer) and 332 failed. There
    was no crash, in 61 processes and 72 s, on the desktop, the machine shared.
  - 28 of the 177 passes are touch tests that pass vacuously, which leaves 149.
  - 259 of the failures come from WLCS 1.8.1 itself: its surface builder attaches a buffer before
    the first configure, which xdg-shell forbids and Yuna refuses.

## Decision

The owner chose, on 2026-11-21:

- **WLCS from nixpkgs, version 1.8.1**, as pinned by `flake.lock`.
  - The development shell names its runner, headers and version (`YUNA_WLCS_RUNNER`,
    `YUNA_WLCS_INCLUDE`, `YUNA_WLCS_VERSION`).
  - It is taken from the shell's own nixpkgs, never another, because the module is loaded into
    WLCS's process, and both must load the same glibc and libstdc++.
  - WLCS moves when `flake.lock` moves, and the expected outcomes are recorded again in that commit.
  - A newer WLCS, its `main` and its keyboard tests, is put to the owner before M2.12 (seats and
    input).
- **`yuna-wlcs`, a module on Yuna's interface.**
  - It is a cdylib, at level 4 of the crate map, that declares WLCS's C interface itself. It adds
    no new crate: neither Smithay's MIT `wlcs` crate nor WLCS's text.
  - It calls two functions of the libwayland-client that WLCS already loaded.
  - Its `unsafe` code is the crate map's named exception
    ([ADR-0150](0150-the-crate-map-yuna-wlcs-owns-wlcss-interface.md)).
  - Each WLCS compositor is one thread that owns a `Compositor`: on the processor, on the real clock
    (WLCS's tests wait for real frame callbacks), with no socket, no Xwayland and no animations,
    one agent's seat, and one 1920×1080 output at (0, 0).
  - **Every request settles**: the module waits for Yuna to have no deadline due within two
    refresh periods, before and after the request, at most 0.5 s each, and only then answers
    WLCS. Two, because a frame asked just after another aims at the boundary after next.
  - A hook unanswered in 10 s, or a panic, aborts the process with its message.
- **Hidden test seams on `Compositor`**, each documented with what it may break:
  - a `Waker`, whose wake is a named source of the rest tool;
  - a `ClientKey` with `window_of_surface`;
  - `set_seat_opens_floating`;
  - `place_window`, a pure move of a floating window, kept on the output by the layout's own rule.
  - **The WLCS seat opens its windows floating.** WLCS's baseline therefore judges Yuna's floating
    path, not its tiling policy, which the layout's own tests cover.
- **An honest descriptor.**
  - The module declares exactly the globals and versions an agent's seat offers, so the tests of
    protocols Yuna lacks are skipped, not failed.
  - A test keeps the descriptor equal to the registry an agent's client reads.
- **Markers.**
  - A test that calls a hook Yuna cannot honour (touch) makes the module print
    `yuna-wlcs: unsupported: …`. The test is then *unsupported*, never passing.
  - A fault of the module itself prints `yuna-wlcs: error: …` and fails, whatever was expected.
- **Every test's outcome expected.**
  - `crates/yuna-wlcs/expected.txt` names the WLCS version, then each test with one outcome:
    `pass`, `fail`, `skip`, `unsupported` or `flaky` (with its reason).
  - A pass is only gtest's `[       OK ]` line, with no marker and no crash.
  - The check fails when:
    - a `pass` test does not pass;
    - any test crashes or hangs, expected or not;
    - a test is missing from the file, or no longer exists;
    - the file names another WLCS version;
    - a test not expected to pass passes, and passes again alone in a fresh process. A single
      pass is only reported.
  - `flaky` tests are run and reported, never judged.
  - Removing a test from `pass` is a deliberate commit that cites the owner's go or an ADR, as a
    move of the parity baseline does
    ([ADR-0142](0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)).
- **Recorded on CI's runner and on the desktop.**
  - `yuna-bench wlcs record` runs each test alone, many times.
  - A test with one outcome over every run gets it. One whose outcome varies is `flaky`, a debt
    item.
  - A crash or a hang is never recorded: it is fixed first.
- **WLCS's own CI job.**
  - A `wlcs` job in `ci.yml`, beside `check` and `parity`, runs every test as `wlcs check --full`.
  - It runs a sentinel group first, then one process per gtest suite, several at once. It resumes
    a suite after a crash, kills a test silent for 60 s, stops after 5 hangs or timeouts in a row,
    and meets a deadline.
  - `scripts/check.sh` runs the tests expected to pass (`wlcs check --listed`), for a check before
    a push.

## Alternatives considered

- **WLCS's `main` branch, pinned**: 68 more tests, the keyboard among them, but it is unreleased,
  renames files, and is built in CI, as nixpkgs' cache does not hold it. It is put to the owner
  before M2.12.
- **Smithay's MIT `wlcs` crate**: four new crates, one release in 2023, and nobody checked where
  its declarations came from.
- **Windows opened tiled, then made floating to be placed**: it also tests the tiled opening, but
  the window keeps its tile's size, stops following its client, and would block under Stage 2's
  transactions.
- **A list of the passing tests alone**: what ADR-0139 wrote, but a crash in a test outside the list
  would fail nothing, and vacuous passes would enter it.
- **A new pass failing at once**, as Mir does: a test that passes one time in ten would turn
  unrelated pushes red.
- **A new pass only reported**: never a spurious red, but a test that starts to pass stays
  unguarded until a review sees it.
- **WLCS inside `scripts/check.sh` whole**: one command for everything, but WLCS would share the
  `check` job's 20 minutes, which a broad regression of WLCS alone would exceed.
- **One process per test at every push**: each test isolated, but 1,225 starts, 716 of them for
  skipped tests. It is kept for the record, where a marker must belong to one test.

## Consequences

- WLCS's baseline holds Yuna to what passes on Smithay's core today. Each milestone that replaces a
  module keeps its `pass` tests, and the failures the first run found are debt placed where their
  module is rewritten ([debt](../debt.md)).
- Of Smithay's four groups, Yuna fails one test, `BadBufferTest.test_truncated_shm_file`, which
  cannot pass on this build. Its truncation is an `assert` that the release build compiles out.
- The WLCS seat's windows cannot maximise, since floating windows do not. Its two maximise tests
  fail for that reason.
- The module's thread runs on the real clock. It is the one task that cannot run inline on the
  manual clock (ADR-0082 item 6), and it is a test harness, not one of Yuna's tasks.
- A protocol added later turns its tests from skipped to run: the commit that adds it records the
  outcomes again.
- `window_of_surface` takes a Wayland object id: M2.13's own xdg-shell keeps that seam.

## Validation and reconsideration

Evidence: the survey of WLCS and of the code, the challenge, the first run and the checkers'
reviews, all on 2026-11-21 (`.delegation/m2.3-*`, local), and the CI figures in
[`measurements.md`](../measurements.md). Checked by the descriptor and ABI tests, the seams' tests,
the runner's tests on a fake WLCS, and `scripts/test-check.sh`'s cases. Proposed again if a newer
WLCS is adopted, if the `wlcs` job's time passes its 20 minutes, or if flaky tests grow past a
handful.
