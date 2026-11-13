# 0039. Leaving Smithay step by step

- Status: Accepted; amends [0011](0011-built-on-smithay.md) (Smithay is left by the end of Stage
  2), [0013](0013-our-own-code-niri-and-jay-as-references.md) (Smithay becomes a reading
  reference)
- Date: 2026-11-13

## Context

The owner asked for a study: replace Smithay with a base Yuna writes itself, fitted to Yuna, and
GLES with Vulkan ([survey](../survey.md#an-own-base-and-vulkan)). The owner's reasons: control and
fit, performance and latency, HDR, colour and effects, and independence from Smithay's pace and
from a growing fork. Time is not a constraint.

What the study found, at M1.5's C4:

- What sets Yuna apart lives in the layers Smithay owns: agents' seats, per-client bounds
  ([ADR-0028](0028-bounds-on-what-one-client-may-ask.md)), honest presentation, HDR.
  [`debt.md`](../debt.md) names Smithay in about two dozen items: a popup's role reset, a
  selection's serial unchecked, `xdg_wm_base.role` that Yuna cannot send, activation tokens
  unbounded, a panic without libEGL, overflow checks off
  ([ADR-0027](0027-smithay-without-overflow-checks.md)), a `DrmDevice` that restores immutable
  properties when dropped.
- Smithay has no `color-management-v1`, no tearing and no HDR; its main author waits for its
  Vulkan renderer, a draft, before colour. It has made no release since 2025-06-24.
- Yuna uses Smithay's protocol handlers, popups, damage tracking, render elements, GLES on EGL and
  calloop. It does not use Smithay's DRM, libinput, session, udev, XWayland or layer-shell code
  yet: those come at M1.8 and Stage 2. About 1,050 lines of Yuna name a Smithay item, 10 % of its
  code at C4.
- Yuna's stories speak the real protocol through `wayland-client`: they hold Yuna's behaviour
  whatever its base, and are the parity net of each step.
- jay, the one compositor with a base all its own, took 27 months to its 1.0, 97 % of it by one
  author. On its figures, a base of Yuna's own by the end of Stage 2 is about 98,000 lines, the
  standard protocols alone about 39,000: [ADR-0011](0011-built-on-smithay.md)'s "two to three
  times longer", in figures.

## Decision

The owner chose, on 2026-11-13:

- **Yuna leaves Smithay step by step, on main**, the program working at each commit. Smithay is
  out of `Cargo.lock` by the end of Stage 2, before 0.2.
- **Leaf protocols move first, as their milestone touches them.** The first is
  `color-management-v1` at M1.6, which Smithay lacks: Yuna's own handler on wayland-server
  ([ADR-0040](0040-the-wire-protocol-stays-wayland-rs.md)).
- **The core moves at the opening of Stage 2**: surfaces, xdg-shell, seats and popups, with Yuna's
  own scene and damage tracking, decided then in an ADR of their own.
- **What Yuna does not use yet is written once, by Yuna**: DRM and KMS on drm-rs, libinput,
  the session, at Stage 2. Whether M1.8 writes its own X11 window manager or uses Smithay's is
  decided in M1.8's design.
- **Smithay becomes a reading reference**, beside niri and jay
  ([ADR-0013](0013-our-own-code-niri-and-jay-as-references.md)). It is under MIT, so a passage
  close to its source is allowed, credited in a comment; jay's code stays never copied.
- The renderer is decided on a measure ([ADR-0041](0041-vulkan-decided-on-a-measure.md)); until
  Smithay leaves, Yuna stays on its fork, patched where a gap costs Yuna now
  ([ADR-0042](0042-main-on-smithays-fork.md)).

## Alternatives considered

- **All at once, right after M1.5**, down to the wire protocol, as jay did (the owner's first
  scope): one long branch changing the event loop, the wire and every handler at once, main on
  Smithay meanwhile. Each failure would have three suspects, and 0.1 would wait for it.
- **Staying on Smithay**, its fork growing ([ADR-0011](0011-built-on-smithay.md) as it was):
  cheapest now, but HDR, tearing and Vulkan stay on Smithay's calendar, and its debt with them.

## Consequences

- Until Stage 2, Yuna's own handlers and Smithay's live on one wayland-server display; Yuna's
  own handlers find a surface's state through Smithay's until the core moves.
- Stage 2 opens with the core's move, before the desktop's DRM, and its exit adds: no Smithay in
  `Cargo.lock` ([ROADMAP](../ROADMAP.md)).
- WLCS, the Wayland conformance suite, runs against Yuna before the core moves: Smithay's CI runs
  four of its groups, and Yuna's own core must at least match that.
- [ADR-0017](0017-smithays-revision.md) and [ADR-0021](0021-smithays-fork.md) are superseded once
  Smithay is gone; [`smithay-patches.md`](../smithay-patches.md) is deleted then.
- Driver behaviour that Smithay met for its users (DRM atomic commits, multi-GPU, sessions)
  becomes Yuna's to meet; Smithay's code and issues are read for it.
- A client's `wl_shm` pool truncated while drawn raises SIGBUS: Smithay's guard
  (`wayland/shm/pool.rs`) is rewritten with Yuna's core, and every upload of a pool runs inside it.
