# Architecture

How the code is organised now, for a developer who has never seen it. Decisions and their reasons
are in [`adr/`](adr/README.md); the contracts of each item are in the code's documentation; what
comes next is in [`PLAN.md`](PLAN.md).

## Parts

One Cargo workspace; its crates live in `crates/`.

- **`yuna`** (`crates/yuna`): the program. Today it reads its command line, answers `--help` and
  `--version`, and refuses to start a session, since no backend exists yet. It depends on Smithay
  without using it, so that the pinned revision is locked and checked
  ([ADR-0017](adr/0017-smithays-revision.md)).

## Dependencies

- `yuna` → Smithay, at a pinned git revision, without its default features
  ([ADR-0011](adr/0011-built-on-smithay.md)).

## Main flows

None yet: the program starts, reads its arguments and exits.

## The tools around the code

- `flake.nix`: the development shell; `rust-toolchain.toml`: the toolchain
  ([ADR-0016](adr/0016-the-rust-toolchain.md)).
- `scripts/check.sh`: every automated check, which CI runs (`.github/workflows/ci.yml`).
- `deny.toml`: the dependency policy.

## Where to start reading

1. [`../crates/yuna/src/main.rs`](../crates/yuna/src/main.rs).
2. [`../scripts/check.sh`](../scripts/check.sh).
