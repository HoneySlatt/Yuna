# 0106. Vulkan alone, and uwui hosted for what Yuna draws

- Status: Accepted; supersedes [0012](0012-opengl-first.md),
  [0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)
  and [0084](0084-uwui-hosted-on-its-own-task-commands-and-images.md)
- Date: 2026-11-18

## Context

Yuna's renderer and what it draws itself were decided in steps.
[ADR-0012](0012-opengl-first.md) drew through Smithay's GLES first and kept the colour pipeline,
the layout and the animations apart from the renderer.
[ADR-0041](0041-vulkan-decided-on-a-measure.md) left the renderer to a prototype.
[ADR-0063](0063-vulkan-alone-and-uwui-for-what-yuna-draws.md) chose Vulkan alone and uwui for
what Yuna draws, and
[ADR-0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)
restated it with a narrower prototype.
[ADR-0084](0084-uwui-hosted-on-its-own-task-commands-and-images.md) changed how Yuna hosts uwui,
and [ADR-0080](0080-what-survives-a-failure-and-yunas-own-panics.md) gave uwui's failures their
promise. A reader needed four of them, and ADR-0012 still read "Accepted". ADR-0012 also had each
milestone that draws measure a frame's GPU and CPU time and its power; that gave way to budgets set
against the rivals ([ADR-0072](0072-budgets-set-against-the-peers-the-same-day.md)), whose figures
[ADR-0077](0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md) holds.

This ADR gathers them into the rule in force on 2026-11-18. It decides nothing new; each part
names the ADR and the date that decided it.

## Decision

- **Vulkan alone.** Yuna draws with Vulkan, with no GLES fallback (ADR-0063, 2026-11-14). Its own
  Vulkan renderer comes at Stage 2's opening, with its own core and scene
  ([ADR-0081](0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md)). Until then the
  headless compositor keeps Smithay's GLES renderer, and nothing is built on it that Vulkan would
  not keep. CI draws on lavapipe (ADR-0063). The binding of Yuna's renderer is chosen at Stage 2
  (ADR-0073, 2026-11-16).
- **The pipeline apart from the renderer.** The colour and HDR pipeline, the layout and the
  animations do not depend on the renderer; only the final drawing is Vulkan (ADR-0012,
  2026-11-11; ADR-0063).
- **uwui draws Hollow, its lock and login screens, and Yuna's own interfaces**: the share picker,
  the capture bar, the tab bar of tabbed columns, the overview, Alt-Tab, messages (ADR-0063). This
  reverses the first decision of SlimeEngine's ADR-0199, which kept uwui out of a shell and of
  what a compositor draws; SlimeEngine records the change in an ADR of its own, and ADR-0199's
  other decisions stand.
- **uwui runs on a task of its own** in Yuna's process, one of the tasks that
  [ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)'s table
  marks contained (ADR-0084, 2026-11-17). Each open interface has its own image on Yuna's Vulkan
  device. uwui records into it with command buffers of its own, submits them itself, and signals
  a timeline semaphore when the image is done.
- **Yuna composites the last image done, and never waits** (ADR-0084). Work past its timeout keeps
  the image before on screen. What survives uwui's failures is ADR-0080's: a panic closes the
  interface, which is built again, and frames go on; an abort, a hang or a lost GPU is a crash, a
  hang or a GPU reset of Yuna.
- **A measured budget.** uwui's GPU time is read on its own submissions, against a budget checked
  in CI (ADR-0063; ADR-0084).
- **Games first.** Over a fullscreen game, uwui draws nothing unless the owner opens an interface
  (ADR-0063).
- **The hosted interface is a contract between the two projects** (ADR-0084): versioned on uwui's
  side, a revision of uwui pinned in Yuna. Yuna asks it of uwui, which accepts or answers first,
  uwui coming first on any conflict
  ([ADR-0115](0115-priorities-uwui-first-the-owners-activity-measures-that-check-their-load.md)).
  uwui's accessibility tree leaves its hosted mode, so that Yuna can give it to a screen reader
  later.
- **A frame's budgets come from the rivals at equal work** (ADR-0072; ADR-0077), checked by the
  bench's series at each stage's end.
- **M1.9's prototype answered only what no rival measures** (ADR-0073), headless on the render node,
  on each tier: the import of each format and modifier the driver offers, compressed ones included;
  explicit synchronisation, sync files and timeline semaphores; a `wl_shm` buffer copied through a
  staging buffer against its memory imported (`VK_EXT_external_memory_host`); the CPU and GPU time
  of a composition of 1, 6 and 15 windows, the floor of a Vulkan frame on that tier; whether a
  high-priority queue is granted without privileges. The owner checked the laptop's display:
  `drm_info` and `vulkaninfo`, then sway on its Vulkan renderer on the laptop's panel. Its figures
  are in the [measurements](../measurements.md). Its code, on `ash` 0.38, is on a branch never
  merged, kept until Stage 2's renderer lands, then deleted.

## Alternatives considered

- **GLES first, Vulkan on a measure**
  ([ADR-0012](0012-opengl-first.md), [ADR-0041](0041-vulkan-decided-on-a-measure.md)): HDR on GLES
  would be Yuna's alone to write, and hosting uwui needs a Vulkan device.
- **Vulkan with a GLES fallback** for older machines
  ([ADR-0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)):
  wider reach, but two renderers to keep.
- **wgpu** ([ADR-0012](0012-opengl-first.md)): it cannot import clients' dmabufs, their formats
  and their fences without bypassing its own safety.
- **Yuna's interfaces without uwui**
  ([ADR-0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)):
  a small toolkit written again inside Yuna.
- **uwui in a process of its own**
  ([ADR-0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)):
  the overview and Alt-Tab need every window's image live, across processes.
- **uwui recording into Yuna's frame, inline**
  ([ADR-0084](0084-uwui-hosted-on-its-own-task-commands-and-images.md)): the lowest latency, but
  no timeout, no last good image, and a slow interface delays the whole output.
- **uwui on a Vulkan device of its own**
  ([ADR-0084](0084-uwui-hosted-on-its-own-task-commands-and-images.md)): a GPU hang of uwui might
  spare Yuna's context, but images shared across devices, on a driver behaviour not checked.

## Consequences

- Yuna depends on uwui for what it draws. uwui gains the targets of a shell, a lock screen and a
  compositor's interfaces (layer-shell and session-lock surfaces, a hosted image), on uwui's
  calendar, before Stage 2's renderer hosts it.
- An open interface costs an image: small for the picker, about 33 MB for an overview at 4K. An
  interface's change may show a frame later than if it were drawn inline.
- Stage 5's HDR passes and Stage 6's blur have no prototype figure; their budgets come with their
  stages.
- Stage 2a measures, with the GPU lost on demand, whether amdgpu lets another device of the same
  process survive a hang it did not cause.

## Validation and reconsideration

Evidence: the prototype's runs on the desktop, lavapipe, the laptop and the Steam Deck, and the
laptop's display check ([measurements](../measurements.md)). Not measured: the latency an
interface's own image adds, an overview's image on the laptop. Proposed again if Stage 2's
renderer finds a figure the prototype did not measure that changes a frame's budget, if a machine
of the lowest tier cannot run Vulkan, if 2a's measure shows amdgpu spares an innocent device, or
if uwui refuses the hosted contract.
