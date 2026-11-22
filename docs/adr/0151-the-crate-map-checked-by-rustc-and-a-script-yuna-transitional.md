# 0151. The crate map, checked by rustc and a script; `yuna` transitional until M2.19

- Status: Accepted; supersedes [0150](0150-the-crate-map-yuna-wlcs-owns-wlcss-interface.md);
  supersedes in part [0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
  (item 2's `unsafe` code and globals)
- Date: 2026-11-22

## Context

[ADR-0150](0150-the-crate-map-yuna-wlcs-owns-wlcss-interface.md) states the map of Stage 2's crates
and what `scripts/check.sh` checks from M2.4: the edges, one owning crate per outside library,
`forbid(unsafe_code)` outside those crates and `yuna-wlcs`, and no `static` with interior
mutability nor `thread_local!` outside logging. Nothing checks any of it yet
([debt](../debt.md)). The code at `5b9fa13`:

- **The crates.** `yuna-types`, `yuna-ipc`, `yuna-layout` and `yuna-motion` are pure. `yuna`, Stage
  1's crate, is levels 2, 3 and 4 at once: Smithay and every outside library, the core, the MCP
  server and the program, 54,000 lines of sources. Until `yuna-core` exists, its library is the
  core's interface. `yuna-wlcs` is at level 4. `yuna-bench` stands outside the map. Every edge
  fits ADR-0150's map.
- **`unsafe`.** Only `yuna-types` and `yuna-ipc` forbid it. `yuna`'s sources hold 15 blocks, 3
  `extern` blocks and 1 `unsafe fn` (`mmap`, `_exit`, `sigaction`, `close_range`, `pre_exec`, EGL,
  a keymap); its tests 2; `yuna-bench` 2; `yuna-wlcs` 23 blocks, 1 `extern` block, 23 `unsafe fn`
  (18 of them WLCS's hooks) and its exported structure. `yuna-types` and `yuna-ipc` forbid
  `unsafe` code; `yuna-layout` and `yuna-motion` hold none but do not forbid it. Each block has its `// SAFETY:` comment, which clippy's `undocumented_unsafe_blocks`
  enforces.
- **Globals.** `yuna`'s sources hold 9 statics with interior mutability and 1 `thread_local!`;
  none is logging. They serve:
  - the two signal handlers' pipes (`stop_signal.rs`);
  - the panic hook's mark of the state thread, and the homes' folder the fatal path removes
    (`fatal.rs`, `homes.rs`);
  - the limit of open files the process started with (`process.rs`);
  - EGL's displays kept for the process (`render.rs`, leaves with EGL at M2.9);
  - the client state Smithay's API asks of a client Yuna never inserted (`state.rs`, leaves with
    Smithay at M2.19);
  - two debug-only `OnceLock`s that read test variables (`xwayland/manager/`).
- **What a text scan misses.** A scan of `unsafe` misses blocks a macro makes. A scan of statics
  by the name of their type misses a newtype, an alias, a `Condvar`. Rust 2024's lint does not
  refuse a `static mut` used through `&raw`. Cargo refuses a crate's own lint level beside
  `[lints] workspace = true`, which `check.sh` requires of every manifest, so a workspace
  `forbid` could never be relaxed for the crates allowed `unsafe`.
- [ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 2
  asks for no shared mutable global state, and `#![forbid(unsafe_code)]` outside the crates that
  own a foreign library.
- Clippy has no lint for statics with interior mutability. Its `disallowed-macros` refuses
  `std::thread_local`, aliases included.

## Decision

The owner chose, on 2026-11-22:

- **rustc finds the `unsafe` code.** The workspace sets `unsafe_code = "deny"`. Each module allowed
  `unsafe` code says so with `#[expect(unsafe_code, reason = "…")]`, so that a module that no
  longer holds any fails the lints. Every pure crate, those of levels 0 and 1, holds
  `#![forbid(unsafe_code)]`. Tests and `yuna-bench` follow the same rule. A module's count is of
  every `unsafe` the lint names: blocks, `unsafe fn`, `extern` blocks and `unsafe` attributes.
- **A script checks the rest**, `scripts/crate-map.py`, on `cargo metadata` (offline) and a map
  file, `crates/map.toml`. `scripts/check.sh` runs it, so CI does. The file holds each crate's
  level, the edges allowed within level 1, the outside libraries and their owning crate, each
  module allowed `unsafe` with its count, and each global by file and name with its reason. The
  script fails when:
  - an edge breaks the map;
  - a second crate of the map depends on an owned outside library, by a normal dependency;
  - a module names `unsafe_code` without its entry, or an entry matches nothing;
  - a module holds more `unsafe` than its entry counts;
  - a `static`, of any type, `static mut` and statics inside functions included, or a
    `thread_local!` is not listed by name, or an entry matches nothing.
  Its unit tests and `scripts/test-check.sh` prove each failure.
- **Clippy refuses `thread_local!`** (`disallowed-macros`); the allowed one expects the lint.
- **Who may use what.** Every crate may use `yuna-types`, the shared vocabulary. rustix, the
  system calls, from level 2 up: no pure crate reads a clock or a descriptor. serde, serde_json,
  png and tracing from any level. Each library that binds C, the kernel or a wire protocol has one
  owner: Smithay, wayland-server and the protocols' server side, calloop, xkbcommon, x11rb and
  zbus belong to `yuna` today, and later to their level-2 crates. Development dependencies (the
  tests' clients, proptest) and `yuna-bench` do not count: they are not the core.
- **`yuna` is transitional until M2.19.** For the edges it is level 3: its library is the core's
  interface until `yuna-core` exists, which `yuna-wlcs` depends on. It may own the outside
  libraries it uses today, and hold its `unsafe` code and its globals, each listed. The files of
  `yuna` that name Smithay are listed too: no new module is built on Smithay's types. These three
  lists only shrink; one grows only by an ADR that names the entry. A new outside library goes to its level-2
  crate, never into `yuna`. Each milestone names the crate its new code goes to.
- **Globals: three kinds beside logging**, each entry named in the map file:
  - the fatal path and signal handlers, which run where no state can be passed;
  - state the operating system keeps for the whole process, such as its limit of open files;
  - what a library's interface forces until that library leaves: EGL's displays until M2.9,
    Smithay's client state until M2.19.
  A static that never changes, such as `yuna-wlcs`'s exported structure, needs no kind, only its
  entry and reason. The two debug-only `OnceLock`s are removed at once: the variables are read where the manager
  starts and passed down. `yuna-wlcs`'s `Once` stays the one in that crate
  ([ADR-0149](0149-wlcs-nixpkgs-1-8-1-a-shim-on-the-interface-every-tests-outcome-expected.md)).
  Tests, examples and `yuna-bench` are outside this rule: they are not the core.

The map, whole:

"→" reads "depends on". Edges go only down a level, and within level 1 only along the edges the
table names; the crates of level 2 never depend on each other and meet only in `yuna-core`. The
session opens the input devices, and the core hands their descriptors to `yuna-input` ahead of
use: libinput's opening of a device answers from them, never through `yuna-session` nor a D-Bus
call that would make the state thread wait.

| Level | Crates | Depend on |
|---|---|---|
| 0 | `yuna-types`: ids, coordinate spaces as distinct types, transforms, scale in 120ths, time values, dma-buf descriptors | nothing |
| 1 | `yuna-layout`, `yuna-motion`, `yuna-clock`, `yuna-colour`, `yuna-scene`, `yuna-output`, `yuna-config`, `yuna-ipc` (the commands and events, the crate Hollow uses, under semver) | level 0, and within level 1 only motion → layout, config → ipc, scene → colour, output → clock and scene |
| 2 | one crate per outside library: `yuna-wayland`, `yuna-render` (Vulkan, uwui's hosting), `yuna-kms`, `yuna-headless`, `yuna-input`, `yuna-session`, `yuna-xwm`, `yuna-process` | levels 0 and 1 |
| 3 | `yuna-core`: the state, the loop, the frame scheduler, the command dispatcher, the `Compositor` interface the tests drive | everything below |
| 4 | `yuna`, `yuna-mcp`, `yuna-keeper` (types and IPC only), `yuna-portal`, `yuna-wlcs`, fuzz targets | the core's interface, and the types it names (`yuna-types`, `yuna-ipc`) |

Motion depends on the layout because it turns the layout's jumps into frames
([ADR-0052](0052-motion-in-a-pure-crate.md)); the configuration depends on `yuna-ipc` because its
actions are commands of `yuna-ipc`'s one type (ADR-0082 item 14).

`yuna-bench` stands outside the map: it measures Yuna from outside, depends on no crate of
Yuna's, and Yuna's tests and examples may use it as a development dependency. Each crate is
created when its code exists, except `yuna-ipc`, which Hollow depends on. `unsafe` code is
allowed in the level-2 crates, in `yuna-wlcs` (WLCS's C interface and two calls of
libwayland-client, with a test that compares its declarations with WLCS's installed headers), and
in `yuna` while it is transitional. Each crate's documentation states what
it promises, its bounds, its failure behaviour, and what it never does.

## Alternatives considered

- **A text scan alone** (the first draft): no change to the lints, but it misses `unsafe` from
  macros, statics behind a newtype or an alias, and `static mut` through `&raw`.
- **`cargo-deny`'s `bans` with `wrappers`**: the edges with no new code, but no levels, its own
  messages, and `unsafe` and globals still need another tool.
- **A Rust check crate**: typed, but a crate outside the map, and parsing statics needs `syn`, a
  new dependency.
- **`yuna` paid now**: its `unsafe` code and globals moved into level-2 crates. The map strict at
  once, but crates made before their milestone, and code moved that M2.8 to M2.19 rewrite, which
  moves the parity script's counts.
- **A global allowed only for logging** (ADR-0150): signal handlers and the panic hook have no
  other way to reach what they need.

## Consequences

- M2.4a builds the map file, the script, the lint levels, their `test-check.sh` cases, adds
  `#![forbid(unsafe_code)]` to `yuna-layout` and `yuna-motion`, and removes the two debug
  `OnceLock`s.
- Adding `unsafe` code or a global to `yuna` needs an ADR that names it; M2.4c's pidfd is the first
  ([ADR-0154](0154-a-clients-pidfd-at-accept-through-so-peerpidfd.md)).
- Each milestone that empties a module of `yuna` removes its entries; M2.19 removes `yuna`'s
  transitional status.
- What the script cannot see is stated in its documentation: statics made inside a procedural
  macro or a dependency's macro, such as tracing's.

## Validation and reconsideration

Evidence: the code at `5b9fa13`, read by M2.4's readers; a challenge of the first draft on
2026-11-22, which found the scan's gaps and Cargo's refusal of a crate's own lint level. Each
failure the script claims is a `test-check.sh` case. Proposed again if another level-4 crate needs
`unsafe` code, another level-1 edge turns out needed, or a global fits none of the three kinds.
