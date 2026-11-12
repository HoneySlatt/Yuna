# 0027. Smithay built without overflow checks in debug builds

- Status: Accepted
- Date: 2026-11-12

## Context

A client places a popup with an `xdg_positioner`: an anchor rectangle, a size and an offset, all
`i32`. Smithay `8fd9fe0` adds them with plain `i32` sums in its `get_popup` handler
(`PositionerState::get_geometry`, `src/wayland/shell/xdg/mod.rs:591`), before Yuna sees the
popup. Rust checks integer overflow in debug builds and wraps in release builds. So a client that
sends an anchor near `i32::MAX` panics a debug build of Yuna, and every client goes with it,
against [ADR-0007](0007-surviving-a-crash.md); a release build wraps, and Yuna's own bound on
positioner values then dismisses the popup. Debug builds are what `cargo run` and the tests run,
and what agents run headless under clients they do not control.

Yuna cannot check the values first: Smithay dispatches the request itself.

## Decision

Smithay is built without overflow checks in every profile:
`[profile.dev.package.smithay] overflow-checks = false` in the workspace's `Cargo.toml`. It
behaves in a debug build as in a release build. Yuna's own crates keep their overflow checks.
The owner chose this on 2026-11-12.

## Alternatives considered

- **A patch in Yuna's fork** with saturating sums: fixes the cause, but moves main to the private
  fork, with a read key for CI ([ADR-0021](0021-smithays-fork.md)), for one line, and the patch is
  rebased at each raise of the revision.
- **Debt alone**: nothing changes, and a debug build stays one bad request from falling.

## Consequences

- Another overflow inside Smithay wraps in debug builds too, as it already does in release
  builds: it no longer shows as a panic in tests. Yuna's tests of absurd values check what Yuna
  does with the result.
- When main moves to the fork for another reason, a saturating patch to `get_geometry` can replace
  this setting; the setting is then removed.
