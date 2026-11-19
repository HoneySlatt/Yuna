# 0125. The Rust toolchain: an exact stable version, a pinned nightly for fuzzing alone

- Status: Accepted; supersedes [0016](0016-the-rust-toolchain.md) and
  [0058](0058-what-yuna-takes-from-the-owners-other-projects.md) (fuzzing's nightly toolchain)
- Date: 2026-11-18

## Context

[ADR-0016](0016-the-rust-toolchain.md) pinned an exact stable Rust in one file, and asked that a
nightly tool have an ADR of its own; [ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)
point 7 was that ADR, for fuzzing, inside a decision on other matters. A reader needed both. This
ADR gathers the rule as it stands on 2026-11-18; it decides nothing new.

Every build of Yuna, by an agent, the owner or CI, must use the same compiler, or a check could
pass in one place and fail in another.

## Decision

- **`rust-toolchain.toml` names the exact stable version**, with clippy, rustfmt, rust-src and
  rust-analyzer; it is the one source of the version (ADR-0016, 2026-11-11). It named 1.99.1 at
  first.
- **The Nix development shell reads that file through rust-overlay** (MIT), an input of
  `flake.nix`; rustup reads the same file for a contributor without Nix (ADR-0016).
- **`rust-version` in `Cargo.toml` names the same version**: Yuna promises nothing to older
  compilers. The file and `rust-version` change in the same commit, which `scripts/check.sh`
  checks (ADR-0016).
- **The version is raised on purpose**, at a stage's end with the other updates, or sooner when a
  milestone needs a newer feature or fix. A raise is routine: its commit says why. Only a change of
  policy needs a new ADR (ADR-0016).
- **Stable only, but for fuzzing** (ADR-0016; ADR-0058, 2026-11-14). From Stage 2, when Yuna writes
  its own core, `cargo fuzz` runs on what clients send, in a development shell of its own, on a
  nightly toolchain pinned by date and used for nothing else, weekly in CI. Any other nightly tool,
  such as Miri, needs an ADR of its own.

## Alternatives considered

- **nixpkgs' own Rust, fixed by `flake.lock`** ([ADR-0016](0016-the-rust-toolchain.md)): the
  version would change unasked whenever nixpkgs is updated, and a contributor without Nix could not
  tell which to take.

## Consequences

- One more flake input to update at each stage's end.
- CI downloads the toolchain from rust-lang's servers through Nix, as the owner's machine does.
- From Stage 2, a second, nightly toolchain in a shell of its own, for the weekly fuzzing job.

## Validation and reconsideration

Checked by `scripts/check.sh`, which compares `rust-version` with `rust-toolchain.toml`.
