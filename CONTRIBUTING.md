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
- git, which the link check uses to list the Markdown files;
- libxkbcommon, which Smithay's keyboard links;
- `YUNA_DEV_SHELL`, which `scripts/check.sh` requires;
- what Yuna draws with, loaded at run time: `LD_LIBRARY_PATH` names libglvnd's EGL, and
  `__EGL_VENDOR_LIBRARY_FILENAMES` the shell's own Mesa, so that Yuna and its tests draw on that
  Mesa's llvmpipe, never on the host's drivers
  ([ADR-0022](docs/adr/0022-the-headless-backend-in-yuna.md));
- `XKB_CONFIG_ROOT`, where the keyboard's default keymap comes from.

Yuna itself reads two variables: `XDG_RUNTIME_DIR`, the folder its Wayland socket goes in, which
`yuna --headless` requires; and `YUNA_LOG`, the level of its logs on standard error: `error`,
`warn`, `info` (the default), `debug` (one line per frame drawn) or `trace`.

Nix sees only the files git knows: `git add` a new file before `nix develop` reads it. Plain
`nix develop`, never `nix develop path:.`, which copies the whole folder, `target/` included, into
the store.

Without Nix, rustup reads `rust-toolchain.toml` and installs the same toolchain, enough to build,
run and test Yuna. `scripts/check.sh` still refuses to run outside the shell: there, nothing would
pin the versions of `cargo deny` and `lychee`, and a check could pass for the wrong reason.

## Commands

```sh
nix develop -c scripts/check.sh           # every check; what CI runs
nix develop -c cargo run -- --help        # Yuna's options
nix develop -c cargo run -- --headless --output 1920x1080@60,1   # Yuna without a screen
nix develop -c cargo test --workspace     # the tests alone
nix develop -c cargo run --release -p yuna-layout --example timing   # the layout's timings
```

`yuna --headless` prints `WAYLAND_DISPLAY=<socket>`; a client started with that variable connects
to it. It draws on llvmpipe; `--gpu` draws on the GPU's render node instead, and is never used by
the tests. SIGTERM or Ctrl-C stops it and removes its socket.

The compositor's tests (`crates/yuna/tests/headless/`) run it in their own process, on the manual
clock, with clients written with `wayland-client`; they need the shell's Mesa.

The layout model's property tests (`crates/yuna-layout/tests/properties.rs`) shrink a failure to
the shortest sequence of operations that still fails, and keep its seed in a
`.proptest-regressions` file beside the test. Commit that file with the fix: the case then runs
at every check. The timings are measured on an idle machine, one measurement at a time, and go to
[`docs/measurements.md`](docs/measurements.md).

`scripts/check.sh` runs, in order, and stops at the first failure:

1. the workspace's own rules: `rust-version` in `Cargo.toml` equals the toolchain's version, and
   every crate takes the workspace's lints;
2. formatting, `cargo fmt --check`;
3. clippy over every target, warnings as errors;
4. the build and the tests;
5. the documentation, `cargo doc`, warnings as errors;
6. `cargo deny`: licences compatible with GPL-3.0-or-later; every RustSec advisory and every yanked
   crate, on any dependency, direct or not; the sources of the dependencies. It reads the advisory
   database from the network;
7. the links between the tracked Markdown files, offline, anchors included;
8. the length of `AGENTS.md` (1,500 words at most) and `docs/PLAN.md` (2,000), read at every
   session.

Every cargo command of the check passes `--locked`: `Cargo.lock` changes only on purpose.

A new crate goes in `crates/` and its `Cargo.toml` holds the two lines `[lints]` and `workspace = true`, so
that the workspace's lints (the `// SAFETY:` comment, documentation) reach it; the check refuses it
otherwise.

## Continuous integration

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs `scripts/check.sh` in the same shell, on
each push to `main`, each pull request, and every Monday. The repository is private and its CI
minutes are counted: CI stays lean, one job ([ADR-0009](docs/adr/0009-gpl-private-until-ready.md)). Its
times are in [`docs/measurements.md`](docs/measurements.md).

## Dependencies

A new dependency is asked of the owner first, with its reason, an alternative weighed and its
licence. The owner accepted proptest, for the layout model's tests only, on 2026-11-11, and, for
the HDR prototype's branch only (M1.3), the same day, Smithay's `backend_drm`, `backend_gbm`,
`backend_egl`, `renderer_gl`, `backend_session_libseat` and `backend_libinput` features with the
system libraries they link. For the headless compositor (M1.4), the owner accepted on 2026-11-12
Smithay's `wayland_frontend`, `desktop` and `renderer_gl` features; `tracing` and
`tracing-subscriber` with its `fmt` and `std` features alone; `wayland-client` and the client side
of `wayland-protocols`, for tests only; and, in the shell, Mesa, libglvnd, libxkbcommon,
xkeyboard-config, a font, and the applications the tests run
([ADR-0025](docs/adr/0025-applications-in-ci.md)). Smithay is pinned to one revision
([ADR-0017](docs/adr/0017-smithays-revision.md)); its patches live in Yuna's fork
([ADR-0021](docs/adr/0021-smithays-fork.md), [`docs/smithay-patches.md`](docs/smithay-patches.md)).
Raising the toolchain, Smithay, the other crates or `flake.lock` happens at a stage's end, each in a
commit of its own.

## Making a change

1. Pick the current milestone in [`docs/PLAN.md`](docs/PLAN.md).
2. Make the change, with its tests and its documentation.
3. Run `scripts/check.sh`; measure what the milestone touches.
4. Commit one logical change at a time, in English, the subject in the imperative.
