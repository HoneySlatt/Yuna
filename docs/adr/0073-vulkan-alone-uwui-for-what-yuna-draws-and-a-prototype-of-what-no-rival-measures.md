# 0073. Vulkan alone, uwui for what Yuna draws, and a prototype of what no rival measures

- Status: Superseded by [0106](0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md); supersedes
  [0063](0063-vulkan-alone-and-uwui-for-what-yuna-draws.md); superseded in part by
  [0080](0080-what-survives-a-failure-and-yunas-own-panics.md) (uwui's crash boundary) and
  [0084](0084-uwui-hosted-on-its-own-task-commands-and-images.md) (how Yuna hosts uwui)
- Date: 2026-11-16

## Context

[ADR-0063](0063-vulkan-alone-and-uwui-for-what-yuna-draws.md) decided, on 2026-11-14, that Yuna
draws with Vulkan alone and hosts uwui for what it draws and for Hollow, and kept a throwaway
prototype at M1.9 to set the frame's budgets and check the laptop's Vulkan driver. M1.9's design
narrowed that prototype; this ADR states the whole rule now in force.

Yuna's own interfaces have grown ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md),
[ADR-0062](0062-the-compositors-comfort-and-look.md)): the share picker, the capture bar, tabbed
columns' tab bar, the overview, Alt-Tab, messages. Without a toolkit, Yuna would write text, icons
and layout of its own, twice the work uwui does. uwui is built to be hosted: the host lends its
device, queue, encoder and target. The owner's shell is Hollow: public, the owner's first, on Yuna
alone, optional above it.

Much of what a large prototype would measure, the rivals already give: wlroots' Vulkan renderer
blends in 16-bit floats and encodes PQ, so sway on its GLES and Vulkan renderers gives one
compositor on both, on every tier ([survey](../survey.md#an-own-base-and-vulkan)); blur's budget
belongs to Stage 6. What no rival measures is what Stage 2's renderer needs to know of each machine:
which client buffers it imports, how it synchronises with them, how a `wl_shm` buffer reaches the
GPU, and what a composition costs at the least. The laptop's risk is also its display engine: jay
reports modifiers failing on AMD before RX 5000, which Mesa's source contradicts for the laptop.

## Decision

The owner chose, on 2026-11-14 and 2026-11-15; this ADR states the rule whole on 2026-11-16:

- **Yuna draws with Vulkan alone**, with no GLES fallback. Yuna's own Vulkan renderer comes at
  Stage 2's opening, with its own core and scene
  ([ADR-0039](0039-leaving-smithay-step-by-step.md),
  [ADR-0074](0074-the-requirements-of-stage-2s-core.md)); until then the headless compositor keeps
  Smithay's GLES renderer, and nothing is built on it that Vulkan would not keep. CI draws on
  lavapipe. The colour pipeline, the layout and the animations do not depend on the renderer.
- **uwui draws Hollow, its lock and login screens, and Yuna's own interfaces.** Yuna hosts uwui on
  its Vulkan device, under three rules:
  - **a crash boundary**: a failure of uwui inside Yuna drops the interface and builds it again; it
    never takes Yuna down ([ADR-0007](0007-surviving-a-crash.md)); a GPU left lost falls to
    [ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)'s recovery;
  - **a measured budget**: uwui's time in each of Yuna's frames is counted and checked in CI;
  - **games first**: over a fullscreen game, uwui draws nothing unless the owner opens an
    interface.
  This reverses the first decision of SlimeEngine's ADR-0199, which kept uwui out of the owner's
  shell, its lock and login screens and what a compositor draws itself; SlimeEngine records the
  change in an ADR of its own, and ADR-0199's other decisions stand.
- **A throwaway prototype answers only what no rival measures**, headless on the render node, on
  each tier (the desktop and lavapipe by an agent; the laptop and the Steam Deck by the owner):
  - the import of each format and modifier the driver offers, compressed ones included, from
    images exported by Vulkan itself;
  - explicit synchronisation: sync files imported and exported, timeline semaphores;
  - a `wl_shm` buffer copied through a staging buffer against its memory imported
    (`VK_EXT_external_memory_host`);
  - the CPU time to record and submit a composition of 1, 6 and 15 windows, and its GPU time by
    timestamp queries, at the output's own size: the floor of a Vulkan frame on that tier;
  - whether a high-priority GPU queue is granted without privileges.
- **The laptop's display is checked by the owner**: `drm_info` and `vulkaninfo`, then sway on its
  Vulkan renderer on the laptop's panel with `vkcube` and mpv, and again with modifiers off.
- **The prototype's code** is Rust on `ash` 0.38, its only new dependency, on a branch of its own
  that is never merged; its commit is named beside its figures. The branch is kept until Stage 2's
  renderer lands, then deleted.
- **A frame's budgets** come from the rivals at equal work
  ([ADR-0072](0072-budgets-set-against-the-peers-the-same-day.md)); the prototype's floor says how
  far below them Vulkan alone can go on each tier.

## Alternatives considered

- **The renderer chosen on the prototype's figures at M1.9** (ADR-0041): Yuna's hosting of uwui,
  which lends a Vulkan device, would wait on it.
- **Vulkan with a GLES fallback** for older machines: wider reach, but two renderers to keep.
- **Yuna's interfaces without uwui**: no dependency of the compositor on a toolkit, but a small
  toolkit written again inside Yuna.
- **Yuna's interfaces in a separate process drawn by uwui**: a uwui fault could never reach Yuna,
  but the overview and Alt-Tab need every window's image live, across processes.
- **Another toolkit for Hollow** (Quickshell, Iced, GTK4): usable at once, but outside the owner's
  stack, or not drawn to the owner's bar.
- **A large prototype**, adding blur, 16-bit blending with PQ, and a comparison with Yuna's current
  GLES: more figures, but ones the rivals give already or that no decision reads.
- **No prototype**, sway's Vulkan on the laptop only: import, synchronisation and `wl_shm`'s path
  would stay unknown until Stage 2.
- **`vulkanalia`** for the prototype, maintained and on Vulkan 1.4: as good for a throwaway; `ash`
  is the binding most Rust graphics code uses. The binding of Yuna's renderer is chosen at Stage 2.
- **GBM to allocate the prototype's buffers**: closer to what EGL clients send, but one more
  dependency; sway's run on each tier covers real clients' buffers.

## Consequences

- AGENTS.md's rule that only the final drawing is OpenGL became Vulkan.
- Yuna depends on uwui for what it draws; uwui gains the targets of a shell, a lock screen and a
  compositor's own interfaces (layer-shell and session-lock surfaces, a hosted device), whose cost
  is uwui's, under [ADR-0003](0003-a-parallel-project-uwui-first.md)'s rule that uwui comes first.
- ADR-0057's "text or icon rendering" dependency for the picker is uwui.
- Stage 6's blur and Stage 5's HDR passes have no prototype figure; their budgets come with their
  stages.
- If the laptop could not import modifiers or show sway's Vulkan buffers on its panel, the lowest
  tier would return to the owner; on 2026-11-16 it did both ([measurements](../measurements.md)).

## Validation and reconsideration

Evidence at the time: the prototype's runs on the desktop, lavapipe, the laptop and the Steam Deck,
and the laptop's display check ([measurements](../measurements.md)). Proposed again if Stage 2's
renderer finds a figure the prototype did not measure that changes the frame's budget, or if a
machine of the lowest tier cannot run Vulkan.
