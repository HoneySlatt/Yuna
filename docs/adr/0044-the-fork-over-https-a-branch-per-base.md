# 0044. The fork over HTTPS, a branch per base

- Status: Accepted; amends [0021](0021-smithays-fork.md) (how main reaches the fork, how the fork
  moves to a new base)
- Date: 2026-11-14

## Context

[ADR-0042](0042-main-on-smithays-fork.md) moves main onto Yuna's fork of Smithay at M1.6.
[ADR-0021](0021-smithays-fork.md) wrote for a private fork: an SSH URL, git's own client for the
owner's credentials, and a rebase of the branch `yuna` at each raise of Smithay's revision
([ADR-0017](0017-smithays-revision.md)). The fork is public since
[ADR-0036](0036-yuna-is-public.md), and reads over HTTPS without a key.

A rebase rewrites the branch: the commits an older Yuna pins, a tag such as `v0.1.0` among them,
are then on no branch, and GitHub may collect them. The M1.6 design's challenge found it.

## Decision

- Main depends on the fork by its HTTPS URL, `https://github.com/HoneySlatt/smithay.git`, at an
  exact commit. No SSH, no `net.git-fetch-with-cli`, no key in CI. `deny.toml` allows that source
  alone.
- **The fork's history is never rewritten.** Each upstream base has its own branch: `yuna`
  holds the patches on `8fd9fe0`; a raise of the revision copies the patches onto a new branch,
  `yuna-<upstream commit's first 8 characters>`, which becomes the fork's default. Every commit a
  Yuna ever pinned stays on a branch. A patch's fix found later is a commit of its own, never
  squashed.
- The patches are generic and listed in [`smithay-patches.md`](../smithay-patches.md), as
  ADR-0021 says.

## Alternatives considered

- **Rebase `yuna` and tag each pinned commit**: one branch, but a tag per pin to keep, and a
  force-push of a public branch at each raise.
- **Rebase and accept lost pins**: an old Yuna would no longer build.

## Consequences

- The fork grows one branch per raise of Smithay's revision, a few until Smithay leaves
  ([ADR-0039](0039-leaving-smithay-step-by-step.md)).
- Building Yuna needs nothing but network access to GitHub, as for any git dependency.
