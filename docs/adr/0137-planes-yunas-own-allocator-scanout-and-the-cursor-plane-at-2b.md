# 0137. Planes: Yuna's own allocator; direct scanout and the cursor plane at Stage 2b

- Status: Accepted; supersedes in part [0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) (item 16, planes)
- Date: 2026-11-19

## Context

A plane lets the display hardware show a client's buffer without Yuna drawing it: less GPU time
and power, and a frame sooner. KWin measured a Framework 13 at 13.3 W without overlays and 11.5 W
with them, for 4K video in Firefox. [ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
item 16 had the scene and the backends carry what planes need from Stage 2, the planes themselves
at Stage 4, and left the allocator, Yuna's own or libliftoff, to Stage 2's opening. The ROADMAP put
direct scanout at Stage 4 and named no cursor plane.

Read on 2026-11-19 ([survey](../survey.md#scenes-damage-and-planes)): only gamescope uses
libliftoff by default, with no deadline, and refusing any partial composition; wlroots uses it
only when an environment variable asks. KWin decides from what its
scene knows and tests once, with a ladder of fallbacks, and keeps overlays off but on Intel.
Weston places top to bottom with underlays and incremental tests. Smithay's `DrmCompositor` (MIT,
Rust) places per element with tests. jay, Mutter and Hyprland use the primary and cursor planes
alone. libliftoff 0.5.0 is C under MIT. It has had no release since 2024-05-28 and one commit in
the last year. It has no underlays and no kernel colour pipelines (its issue 85). Two fixes for
several outputs have waited since 2024 (MR !85, !86). No Rust binding exists.

Stage 2's exit asks for less power than dwl and a week in place of niri; both scan out a
fullscreen window. Without a cursor plane, each pointer move draws the output again.

## Decision

The owner chose, on 2026-11-19:

- **Yuna writes its own plane allocator**, at Stage 4, in `yuna-kms`, tested on the KMS mock. It
  decides from what the scene knows, as KWin does: opaque, unobstructed, no effect, a dmabuf, its
  update rate. It places top to bottom with a running composited region and underlays, as Weston
  does. It reuses the last plan while nothing structural changes, and caches failed
  configurations by buffer, plane and colour, as jay does. It tests a new configuration on the KMS
  thread, off the frame's path, and keeps the last good one meanwhile. Each plane's colour steps
  are known per plane (Stage 5). libliftoff and Smithay's `DrmCompositor` are read as references,
  never copied ([ADR-0113](0113-our-own-code-niri-jay-and-smithay-as-references.md)).
- **Direct scanout and the cursor plane come at Stage 2b.** A fullscreen surface is shown on the
  primary plane when its transform and colour match the output's. The cursor plane is moved from
  the input path ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
  item 10), its scale bound to the primary's on AMD. Damage clips are sent to KMS.
- **What the scene and the backends carry from Stage 2a**: render lists whose entries say what a
  plane needs ([ADR-0133](0133-yunas-scene-retained-its-damage-found-at-each-latch.md)), and
  per-surface dmabuf feedback with a scanout tranche, switched only after a delay, as wlroots
  waits 30 frames. Tearing is a commit that changes nothing else; the cursor plane is off while
  tearing.
- **Overlays, underlays and the allocator's search wait for Stage 4**, with the games.

## Alternatives considered

- **libliftoff**: a proven search on AMD (the Steam Deck), less code; but C behind an FFI crate of
  its own, no Rust binding, a stalled release, no underlays nor colour pipelines, and fixes for
  several outputs pending while 2c brings several outputs.
- **Everything at Stage 4**: 2b smaller; but at Stage 2 every frame of a fullscreen game is
  composited and every pointer move draws the output, against Stage 2's power budget.

## Consequences

- Stage 2b grows by direct scanout of one surface on the primary plane and the cursor plane; the
  ROADMAP says so.
- Yuna writes and maintains the allocator: Weston's takes about 2,300 lines, libliftoff 2,800.
- A plane's test commits on AMD may be slow (KWin's note; niri keeps overlays off for
  performance issues it does not name); 2b measures one on the desktop and on the laptop.

## Validation and reconsideration

Evidence: the rivals' sources and libliftoff's tracker, read on 2026-11-19. Checked at 2b by the
KMS mock's tests and the owner's evening, at Stage 4 by the games' figures. Proposed again if
libliftoff gains underlays, colour pipelines and several outputs, and Yuna's own allocator costs
more to keep right.
