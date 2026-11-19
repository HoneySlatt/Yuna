# 0133. Yuna's scene: retained, changed by batches, its damage found at each latch

- Status: Accepted
- Date: 2026-11-19

## Context

[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 4 asks
Stage 2's scene for a target per consumer (the output, a capture with never-captured windows
blanked, an SDR capture of HDR), snapshots, damage per output buffer and per plane, a cost in
proportion to what changed, and frames in flight as a parameter, each tested on the scene alone.
Its crate map puts `yuna-scene` at level 1: it depends on `yuna-types` and `yuna-colour` alone and
cannot read the surfaces, which belong to level 2. The ROADMAP left the scene to Stage 2's opening.

Today Yuna rebuilds, each frame, a list of Smithay's render elements, and Smithay's
`OutputDamageTracker` diffs it with the last by id and commit counter (`crates/yuna/src/render.rs`).
That path allocates about 83 times a still frame on llvmpipe at 1080p, half of it in the elements'
lists ([measurements](../measurements.md)).

The rivals, read in their sources on 2026-11-19 ([survey](../survey.md#scenes-damage-and-planes)):
retained trees damaged on each change (wlroots' `wlr_scene`, KWin's items drawn by several views,
Mutter's actors, jay's tree with a live and a rendered timeline), or lists rebuilt each frame
(Smithay and niri, Hyprland, gamescope). None has a measured cost for an idle or one-change frame.
A model of a latch ([measurements](../measurements.md#the-scenes-latch-a-model)), measured on the
desktop under load, walked 150 nodes in 1.5 µs and diffed them
by slot in 0.9 µs: the processor does not decide between the shapes.

## Decision

The owner chose, on 2026-11-19, **a retained scene in `yuna-scene`, changed by batches of typed
operations, whose damage each view finds at each latch by comparing its new list with its last.**

- **Nodes.** A tree of surfaces (a buffer by id, never a GPU object), solid fills, snapshots and
  groups. Each holds its position, size, source rectangle, transform, opacity, corner radius,
  clip, opaque region and colour description. uwui's images are nodes like any other. Each
  seat's cursor is a node.
- **Batches.** The core changes the scene only through typed operations, applied as one batch
  between two latches. A layout transaction is one batch: the core holds it until its surfaces
  have committed or its timeout passes, a late surface excluded, so that a column's three windows
  show their new size in one frame ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
  item 1). The layout is the live state; the scene is what is drawn.
- **Motion as group transforms.** `yuna-motion`'s camera, a workspace switch's offset, a fade's
  opacity and each window's own offset are transforms of groups, sampled for each output just
  before its latch; rounding to pixels belongs to each view's mapping.
- **Views.** One per consumer: an output, a capture, an SDR capture of HDR, later a plane, or a
  live view rooted at one window (a window's capture, picture in picture, previews). A view holds
  its mapping from logical space to its pixels (scale, output transform, zoom), its filter (a
  capture's exclusions), its final colour steps and target colour description, its priority and
  rate, and its frames in flight. Output views come first; a capture runs at most at its
  consumer's rate and is skipped when the renderer is behind. A capture whose mapping, filter and
  colour equal its output's copies that output's image rather than drawing again.
- **The latch.** At a frame, a view walks its visible nodes, skipping whole groups outside it, and
  produces an immutable render list: draw operations (textures with source, destination,
  transform, opacity, radius and colour description; fills), each entry naming its node, buffer,
  whether it is opaque, its visible region, its update rate and whether an effect touches it, so
  that planes (direct scanout at 2b, overlays at Stage 4) need no rework. The list carries the frame's id
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 11),
  counted references to its buffers, released by the renderer's reply and by KMS at the flip, and
  each surface's damage in buffer coordinates for damaged uploads (item 21). It goes to the
  renderer's task by message; the scene never waits on it. Stage 6's effects that read what lies
  beneath (blur) will need lists in several passes; nothing is built for them before.
- **Damage found at the latch.** The view compares its new list with its last one, node by node
  through a slot index, adds each surface's own damage since that list, takes away what opaque
  nodes in front cover, and keeps the result in the view's device pixels, with a ring per buffer
  for its age. Every damaged region knows its node: a video on a plane damages no other view.
  Nothing records damage when an operation is applied, so none can be forgotten.
- **The lock is a state of the scene.** Every latch applies it before the view's filter: black on
  every output from the first instant, new ones included, then the locker's surfaces alone, or
  Yuna's dead-locker screen ([ADR-0108](0108-the-lock-begun-by-yuna-a-dead-or-frozen-locker-relieved.md)).
  Every list and every buffer drawn from it carries `locked`; the KMS crate never shows or
  restores a buffer drawn unlocked while the session is locked.
- **The cursor.** Its image goes through the scene, for composited frames and captures; its
  position also goes straight from the input path to the KMS thread (item 10). Its plane comes
  at Stage 2b ([ADR-0137](0137-planes-yunas-own-allocator-scanout-and-the-cursor-plane-at-2b.md)).
- **Snapshots** hold the frozen part of a list with references to its buffers, an image drawn
  only when an effect needs one, and the never-captured mark of the window they froze; their
  memory is counted, and a GPU reset replaces or ends them.
- **Frames in flight per tier.** On lavapipe, one, with one buffer: a view does not latch while
  the renderer holds it, and waiting latches merge. On a GPU, two when the GPU climbs, as KWin
  does; one under VRR or tearing.
- **Tested alone**, as the layout is: each requirement of item 4 a test on the scene; and a pixel
  test on lavapipe, in which a frame drawn from the previous one with the damage found equals the
  same frame drawn whole, over random sequences of operations at scales 1, 1.25 and 1.5.

## Alternatives considered

- **A retained tree whose operations record their damage in a journal** (wlroots, KWin): no walk
  to diff, but a region in a journal does not say whose it is, so a plane's video would redraw the
  primary layer beneath it, a hidden client would redraw what covers it, a state no frame shows is
  damaged too (wlroots' issue 3187, open since 2021), and a forgotten damage is a bug no test can
  rule out.
- **A list rebuilt each frame and diffed** (today's Smithay path, niri): never stale and simple,
  but the core rebuilds every element of every window each frame, and the scene could not be a
  pure crate tested alone: it would need the surfaces, which sit a level below.
- **No damage, everything recomposited** (gamescope): fails rest and power on the laptop.

## Consequences

- The scene, its views and its lists are pure data: the core and the renderer meet through
  messages, and the scene's tests need no GPU and no Wayland.
- The latch walks the visible nodes of each view at each frame, measured at 15, 150 and 1,500
  drawn windows and at 10,000 on one strip with groups outside the view skipped.
- Overlays at Stage 4 split an output view's list; holes for underlays need a "replace" blend in the
  renderer's closed set of pipelines (item 19).
- The cursor and drag icons, absent from captures today ([debt](../debt.md)), are drawn by the
  scene.

## Validation and reconsideration

Evidence: the rivals' sources ([survey](../survey.md#scenes-damage-and-planes)) and the latch model
of 2026-11-19 ([measurements](../measurements.md#the-scenes-latch-a-model));
measured, synthetic and disturbed, an order of magnitude only. Checked by the scene's tests, the
pixel test, and ADR-0082's scenarios of the column's resize and the lock. Proposed again if the
latch's cost at 1,500 drawn windows passes a tenth of a 240 Hz frame, or if overlays at Stage 4 need
damage the lists cannot give.
