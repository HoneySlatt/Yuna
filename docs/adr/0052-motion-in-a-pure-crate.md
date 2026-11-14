# 0052. Motion in a pure crate, the layout's jumps turned into offsets

- Status: Accepted; amends [0020](0020-the-layout-models-shape.md) (workspace ids, the camera)
- Date: 2026-11-14

## Context

[ADR-0010](0010-animations-and-effects.md) moves windows on springs and fades on curves,
interrupted without a jump, checked frame by frame on the test clock.
[ADR-0020](0020-the-layout-models-shape.md) keeps the layout model instant: no clock, no
animation state; it left "the camera" to M1.7. niri keeps its animations inside its layout,
one camera per workspace and an offset per moved column that decays to 0
([survey](../survey.md#animations)). Yuna's layout already snaps every rectangle to the physical
grid, and its workspaces are positions in a list that `tidy` shifts when an empty one goes.
The design was challenged before any code was written (M1.7).

## Decision

The owner chose, on 2026-11-14:

- **A crate of its own, `yuna-motion`**, which depends on `yuna-layout` alone. It holds the
  springs and curves, and the motion of an arrangement: given each output's arrangement from the
  layout and a time, it gives what each frame draws (window, rectangle, alpha) and which windows
  are told they are visible. Its state is keyed by window, workspace and output ids. It is
  tested alone, with random sequences of operations, as the layout is. `crates/yuna` adapts it to
  Smithay and draws its list.
- **The layout still jumps.** An operation moves the model to its new state at once. After each
  arrangement, `yuna-motion` turns each window's jump into an offset, its drawn position minus
  its new place, that decays to 0 on a spring.
- **One camera per scrolling workspace**: the view's position on the strip, `view_x`, which the
  layout gives out with each shown workspace. When an operation shifts the strip under the view
  (a column added left of the focus), the camera and the offsets are re-expressed from a column
  present before and after, so that what the operation leaves in place does not move on screen.
  Dwindle and floating have no camera.
- **A drawn position is rounded once**: the strip position plus the offset minus the camera,
  rounded to the physical grid, so that still columns move as one rigid strip. Hit testing uses
  the same numbers.
- **Workspaces get ids** from the model, from a counter, never reused; each placed window says
  its workspace and its layer (strip, tiles, floating, pinned). This adds to ADR-0020's plain
  ids: the model creates workspaces, so it names them.
- **What does not slide.** A window that changes workspace, output or layer drops its offset and
  moves with its new container; floating to tiled within one workspace keeps it, through screen
  coordinates. Pinned windows stay above a workspace switch. Outputs added, removed or rescaled
  are instant. A window moved to a hidden workspace leaves at once.
- **Sizes jump.** A window's size follows its client's commit; only its position moves. Resizes
  are animated with the effects ([ROADMAP](../ROADMAP.md), Stage 6).

## Alternatives considered

- **Animation state in the layout model**, as niri: one place for all, but time enters the pure
  model, and ADR-0020's tests and budgets would carry it.
- **A module of `crates/yuna`**: less plumbing, but every test builds Smithay and GL, random
  sequences become costly, and the logic that decides whether anything jumps is tied to Smithay,
  which Yuna leaves ([ADR-0039](0039-leaving-smithay-step-by-step.md)).
- **The camera relative to the focused window**: a window that changes column (consume, expel)
  would move its new column on screen unless two springs cancel exactly.
- **Rounding the camera and positions separately**: still columns are on the grid either way;
  a window moving over a moving camera would err by up to a pixel instead of half.

## Consequences

- A frame's drawn list is plain data: Yuna's own scene (Stage 2) and Vulkan
  ([ADR-0041](0041-vulkan-decided-on-a-measure.md)) replace what draws it, not the motion.
- A width change shows, while positions slide, columns that overlap or a window's old buffer by
  the background, until its client commits; the drawing order says which is above.
- Stage 2's gestures hold the camera's value under the finger and hand their velocity to the
  spring at release; the layout will then answer where a view position lands.
