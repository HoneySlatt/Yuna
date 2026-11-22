# 0150. The crate map: `yuna-wlcs` owns WLCS's interface

- Status: Accepted; supersedes [0145](0145-the-crate-map-motion-on-the-layout-the-configuration-on-the-commands.md)
- Date: 2026-11-21

## Context

[ADR-0145](0145-the-crate-map-motion-on-the-layout-the-configuration-on-the-commands.md) states the
map of Stage 2's crates, which `scripts/check.sh` checks from M2.4. It allows `unsafe` code only in
the level-2 crates, one for each outside library. It allows a `static` with interior mutability
only for logging.

`yuna-wlcs`, WLCS's integration module
([ADR-0149](0149-wlcs-nixpkgs-1-8-1-a-shim-on-the-interface-every-tests-outcome-expected.md)), sits
at level 4. It needs both:

- **`unsafe` code.** It exports the C structure WLCS looks up, `wlcs_server_integration`, an
  `unsafe` attribute in Rust 2024. Its hooks take WLCS's raw pointers. It calls two functions of
  libwayland-client, `wl_display_get_fd` and `wl_proxy_get_id`, to name a client's descriptor and
  a surface's id. A separate level-2 crate could hold the declarations, but the exported structure
  and its hooks stay in the module WLCS loads.
- **One `Once`.** It installs the module's logging and its panic hook, once for WLCS's process.

The motion → layout and config → ipc edges ADR-0145 allowed are unchanged. Its reasons stand:
motion turns the layout's jumps into frames
([ADR-0052](0052-motion-in-a-pure-crate.md)), and the configuration's actions are commands of
`yuna-ipc`'s one type
([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 14).

## Decision

The owner chose, on 2026-11-21, a **named exception**:

- `yuna-wlcs` is the one crate that owns WLCS's C interface and those two calls of
  libwayland-client. It may hold `unsafe` code, each block with its `// SAFETY:` comment, and a
  test that compares its declarations with WLCS's installed headers.
- It may hold one `Once`, the one that installs its logging and its panic hook.

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
| 4 | `yuna`, `yuna-mcp`, `yuna-keeper` (types and IPC only), `yuna-portal`, `yuna-wlcs`, fuzz targets | the core's interface |

`yuna-bench` stands outside the map, as ADR-0082 says. Each crate is created when its code exists,
except `yuna-ipc`, which Hollow depends on. Until `yuna-core` exists, the core's interface is the
`yuna` crate's library, which `yuna-wlcs` depends on.

`scripts/check.sh` checks:

- the edges;
- one owning crate per outside library;
- `forbid(unsafe_code)` outside those crates and `yuna-wlcs`;
- no `static` with interior mutability nor `thread_local!` outside logging, with `yuna-wlcs`'s one
  `Once` the one exception.

Each crate's documentation states what it promises, its bounds, its failure behaviour, and what it
never does.

## Alternatives considered

- **A level-2 crate, `yuna-wlcs-sys`, holding every `unsafe` block**: the map stays regular. But it
  adds a crate for about 150 lines, and the exported structure and its hooks stay `unsafe` in the
  module anyway.
- **The map kept strict, motion → layout and config → ipc removed**: rejected at ADR-0145, for the
  reasons it gives.

## Consequences

- M2.4's check of the map names `yuna-wlcs` among the crates that may hold `unsafe` code, and names
  its `Once`.
- A change of WLCS's interface fails the declarations' test before it reaches a run.

## Validation and reconsideration

Evidence: the code of `crates/yuna-wlcs` and its ABI test, read on 2026-11-21. Proposed again if
another level-4 crate needs `unsafe` code, or if another level-1 edge turns out needed.
