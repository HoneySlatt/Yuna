# 0112. Smithay: pinned, patched in Yuna's public fork, left at Stage 2

- Status: Accepted, its upstream base raised on 2026-11-18 to `7ff0ede4`, as
  [`smithay-patches.md`](../smithay-patches.md) records; supersedes [0011](0011-built-on-smithay.md),
  [0017](0017-smithays-revision.md), [0021](0021-smithays-fork.md),
  [0036](0036-yuna-is-public.md) (the fork's visibility),
  [0039](0039-leaving-smithay-step-by-step.md) (Smithay as Yuna's base, and leaving it),
  [0042](0042-main-on-smithays-fork.md) and
  [0045](0045-the-fork-over-https-a-branch-per-base.md)
- Date: 2026-11-18

## Context

How Yuna stands on Smithay was decided in eight ADRs. [ADR-0011](0011-built-on-smithay.md) built
Yuna on Smithay with a fork of its own; [ADR-0017](0017-smithays-revision.md) pinned the revision;
[ADR-0021](0021-smithays-fork.md) made the fork private, reached over SSH, rebased at each raise;
[ADR-0022](0022-the-headless-backend-in-yuna.md) took the headless backend out of the fork's
first patches; [ADR-0036](0036-yuna-is-public.md) made the fork public;
[ADR-0042](0042-main-on-smithays-fork.md) moved main onto it;
[ADR-0045](0045-the-fork-over-https-a-branch-per-base.md) reached it over HTTPS and stopped
rewriting it; [ADR-0039](0039-leaving-smithay-step-by-step.md) chose to leave Smithay step by
step. A reader needed most of them, and no ADR stated the practice that
[`smithay-patches.md`](../smithay-patches.md) follows.

ADR-0011 counted among the fork's first patches the connector and plane colour properties for
HDR, tearing, and a headless output backend. ADR-0022 made the backend Yuna's own code; ADR-0039,
having Yuna write its own DRM and KMS at Stage 2, left tearing and the planes' colour to that code.
ADR-0017 and ADR-0021 moved main to the fork with its first patch, over SSH, rebasing
the fork; ADR-0042 and ADR-0045 replaced that. ADR-0039 had Smithay out by Stage 2's end; ADR-0081
puts it at 2a's gate. The later rule is the one below.

This ADR gathers the rule as it stands on 2026-11-18. It decides nothing new. How Yuna's core
replaces Smithay's at Stage 2 is
[ADR-0081](0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md)'s, which stays whole;
reading Smithay's code as a reference is
[ADR-0113](0113-our-own-code-niri-jay-and-smithay-as-references.md)'s.

## Decision

- **Yuna is built on Smithay until Stage 2, and leaves it step by step, on main**, the program
  working at each commit (ADR-0011, 2026-11-11; ADR-0039, 2026-11-13). Smithay is out of
  `Cargo.lock` at Stage 2a's gate, before 0.2 (ADR-0081, 2026-11-17).
- **How it leaves** (ADR-0039). Leaf protocols move first, as their milestone touches them: the
  first was `color-management-v1` at M1.6, Yuna's own handler on wayland-server
  ([ADR-0040](0040-the-wire-protocol-stays-wayland-rs.md)). The X11 window manager is Yuna's own,
  on x11rb ([ADR-0066](0066-yunas-own-x11-window-manager-on-x11rb.md), 2026-11-14). The core
  (surfaces, xdg-shell, seats and popups, with Yuna's own scene and damage tracking) moves at
  Stage 2a, along ADR-0081's path. What Yuna does not use yet is written once, by Yuna, at
  Stage 2: DRM and KMS on drm-rs, libinput, the session. Until then Yuna's handlers and Smithay's
  live on one wayland-server display.
- **WLCS, the Wayland conformance suite, runs against Yuna before the core moves** (ADR-0039):
  Smithay's CI runs four of its groups, and Yuna's own core at least matches that. Its baseline is
  recorded at Stage 2's first milestone (ADR-0081).
- **A pinned revision** (ADR-0017, 2026-11-11). Smithay is a dependency of
  `[workspace.dependencies]`, without its default features; each crate turns on what it uses.
  `Cargo.lock` holds the exact commit, and `cargo deny` checks its licences, advisories and
  sources. The upstream base is `8fd9fe0c74e5c13a1fc9b6797d3960c8381bda55`, master of 2026-11-11.
  Raising it at a stage's end is routine: the commit that raises it says why, and
  `smithay-patches.md` names the base and the pins. Only a change of policy needs a new ADR.
- **What Smithay lacks is written in Yuna, or in a fork kept as small as possible** (ADR-0011).
  A patch is as generic as it can be and leaves the meaning to Yuna (ADR-0021, 2026-11-11); each
  is listed with its reason in [`smithay-patches.md`](../smithay-patches.md), one commit per patch
  (ADR-0021). The first, `DrmSurface::set_connector_property`, sends the connector properties HDR
  needs (ADR-0021); main moved to the fork with two more, a filter of the `wl_seat` and `wl_output`
  globals per client and saturating sums in a popup positioner's arithmetic (ADR-0042, in place of
  [ADR-0027](0027-smithay-without-overflow-checks.md)'s build setting); the later ones are listed
  there. **Yuna never contributes upstream**:
  Smithay asks that AI-written code be flagged, which the owner's rules forbid (ADR-0011).
- **The fork is public**: `HoneySlatt/smithay`, a repository of its own, not a GitHub fork in
  Smithay's network (ADR-0036, 2026-11-13). GitHub Actions are off there; Yuna's CI checks what
  Yuna uses (ADR-0021).
- **Main depends on the fork since M1.6** (ADR-0042, 2026-11-13), by its HTTPS URL,
  `https://github.com/HoneySlatt/smithay.git`, at an exact commit: no SSH, no
  `net.git-fetch-with-cli`, no key in CI; `deny.toml` allows that source alone (ADR-0045,
  2026-11-14).
- **The fork's history is never rewritten** (ADR-0045). Each upstream base has its own branch:
  `yuna` holds the patches on `8fd9fe0`; a raise copies them onto a new branch,
  `yuna-<upstream commit's first 8 characters>`, which becomes the fork's default. Every commit a
  Yuna ever pinned stays on a branch. A patch's fix found later is a commit of its own, never
  squashed.

## Alternatives considered

- **A fork of jay, of niri or of cosmic-comp, or a base from zero**
  ([ADR-0011](0011-built-on-smithay.md)): another author's code to maintain alone, one seat and a
  permanent split, a tie to COSMIC, or two to three times longer.
- **Staying on Smithay, its fork growing** ([ADR-0039](0039-leaving-smithay-step-by-step.md)): HDR,
  tearing and Vulkan on Smithay's calendar, and its debt with them.
- **Leaving Smithay all at once** ([ADR-0039](0039-leaving-smithay-step-by-step.md)): one long
  branch where each failure has three suspects.
- **Smithay's 0.7.0 release, or niri's revision** ([ADR-0017](0017-smithays-revision.md)): a year
  and a half behind, or without the guard against bad sizes.
- **A patch that knows HDR** ([ADR-0021](0021-smithays-fork.md)): a larger fork, and colour
  decisions inside Smithay.
- **Smithay built without overflow checks** ([ADR-0027](0027-smithay-without-overflow-checks.md)):
  replaced by a saturating patch once main was on the fork.
- **A rebase of the fork at each raise, a tag per pin**
  ([ADR-0045](0045-the-fork-over-https-a-branch-per-base.md)): a force-push of a public branch at
  each raise.

## Consequences

- Each raise of the revision copies the patches onto a new branch, runs Smithay's tests with the
  features Yuna uses, then Yuna's checks (`smithay-patches.md`).
- Driver behaviour that Smithay met for its users (atomic commits, several GPUs, sessions) becomes
  Yuna's to meet; Smithay's code and issues are read for it.
- A client's `wl_shm` pool truncated while drawn raises SIGBUS: Smithay's guard is rewritten with
  Yuna's core, and every upload of a pool runs inside it.
- When Smithay leaves, its fork, its seat filter and `smithay-patches.md` go with it, and this ADR
  is superseded.
- Building Yuna needs network access to GitHub, as for any git dependency.

## Validation and reconsideration

Evidence: `smithay-patches.md`, each patch with the test of Yuna's that covers it, and the parity
checks of each step of Stage 2a (ADR-0081). Proposed again if Stage 2a cannot keep parity on its
path off Smithay.
