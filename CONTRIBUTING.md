# Contributing

How to set up the environment, what each command does, and how a change is made and checked. The
rules for agents are in [`AGENTS.md`](AGENTS.md); how the code is organised, in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Environment

Yuna is built on Linux, in the Nix development shell that `flake.nix` defines. It needs Nix with
flakes; nothing else is installed by hand.

```sh
nix develop                 # enter the shell
nix develop -c <command>    # or run one command in it
```

The shell gives:

- the Rust toolchain named in [`rust-toolchain.toml`](rust-toolchain.toml), with clippy, rustfmt,
  rust-src and rust-analyzer ([ADR-0016](docs/adr/0016-the-rust-toolchain.md));
- `cargo deny`, which checks the dependencies against [`deny.toml`](deny.toml);
- `lychee`, which checks the links between the Markdown files;
- `YUNA_DEV_SHELL`, which `scripts/check.sh` requires.

Nix sees only the files git knows: `git add` a new file before `nix develop` reads it. Plain
`nix develop`, never `nix develop path:.`, which copies the whole folder, `target/` included, into
the store.

Without Nix, rustup reads `rust-toolchain.toml` and installs the same toolchain; `cargo deny` and
`lychee` are then yours to install.

## Commands

```sh
nix develop -c scripts/check.sh           # every check; what CI runs
nix develop -c cargo run -- --help        # run Yuna
nix develop -c cargo test --workspace     # the tests alone
```

`scripts/check.sh` runs, in order, and stops at the first failure:

1. formatting, `cargo fmt --check`;
2. clippy over every target, warnings as errors;
3. the build and the tests;
4. the documentation, `cargo doc`, warnings as errors;
5. `cargo deny`: licences compatible with GPL-3.0-or-later, RustSec advisories, the sources of the
   dependencies; it reads the advisory database from the network;
6. the links between the tracked Markdown files, offline, anchors included;
7. the length of `AGENTS.md` (1,500 words at most) and `docs/PLAN.md` (2,000), read at every
   session.

Every command passes `--locked`: `Cargo.lock` changes only on purpose.

## Continuous integration

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs `scripts/check.sh` in the same shell, on
each push to `main`, each pull request, and every Monday. The repository is private and its CI
minutes are counted: CI stays one job ([ADR-0009](docs/adr/0009-gpl-private-until-ready.md)). Its
times are in [`docs/measurements.md`](docs/measurements.md).

## Dependencies

A new dependency is asked of the owner first, with its reason, an alternative weighed and its
licence. Smithay is pinned to one revision ([ADR-0017](docs/adr/0017-smithays-revision.md)).
Raising the toolchain, Smithay, the other crates or `flake.lock` happens at a stage's end, each in a
commit of its own.

## Making a change

1. Pick the current milestone in [`docs/PLAN.md`](docs/PLAN.md).
2. Make the change, with its tests and its documentation.
3. Run `scripts/check.sh`; measure what the milestone touches.
4. Commit one logical change at a time, in English, the subject in the imperative.
