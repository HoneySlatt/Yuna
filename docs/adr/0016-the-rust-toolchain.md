# 0016. The Rust toolchain: an exact stable version, in one file

- Status: Accepted
- Date: 2026-11-11

## Context

Every build of Yuna, by an agent, the owner or CI, must use the same compiler, or a check could pass
in one place and fail in another. On 2026-11-11, the latest stable Rust is 1.99.1 (2026-10-16), and
1.100.0 is due on 2026-11-12. Smithay at the pinned revision needs 1.88 or later
([ADR-0017](0017-smithays-revision.md)). nixpkgs-unstable gives 1.99.1. SlimeEngine pins its
toolchain in `rust-toolchain.toml`, read by Nix through rust-overlay.

## Decision

- `rust-toolchain.toml` names the exact version, **1.99.1**, with clippy, rustfmt, rust-src and
  rust-analyzer. It is the one source of the version.
- The Nix development shell reads that file through **rust-overlay** (MIT), an input of `flake.nix`;
  rustup reads the same file for a contributor without Nix.
- `rust-version` in `Cargo.toml` names the same version: Yuna promises nothing to older compilers.
- The version is raised on purpose, at a stage's end with the other updates, or sooner when a
  milestone needs a newer feature or fix; the file and `rust-version` change in the same commit.
- Stable only. A nightly tool (Miri, cargo-fuzz) would need an ADR of its own.

## Alternatives considered

- **nixpkgs' own Rust, fixed by `flake.lock`**: no extra flake input, but the version changes
  unasked whenever nixpkgs is updated, and a contributor without Nix cannot tell which version to
  take.
- **Waiting a day for 1.100.0**: six weeks more before the next rise, but it held M1.1 back, and a
  `.0` release is sometimes followed by a `.1` soon after.

## Consequences

- One more flake input to update at each stage's end.
- CI downloads the toolchain from rust-lang's servers through Nix, as the owner's machine does.
