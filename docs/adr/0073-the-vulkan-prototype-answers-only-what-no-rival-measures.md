# 0073. The Vulkan prototype answers only what no rival measures

- Status: Accepted; narrows [0063](0063-vulkan-alone-and-uwui-for-what-yuna-draws.md)'s prototype
- Date: 2026-11-15

## Context

[ADR-0063](0063-vulkan-alone-and-uwui-for-what-yuna-draws.md) chose Vulkan alone and kept a
throwaway prototype at M1.9, to set the frame's budgets and to check the laptop's Vulkan driver.
It no longer chooses a renderer.

Much of what a large prototype would measure, the rivals already give:

- wlroots' Vulkan renderer blends each frame in 16-bit floats and encodes PQ for HDR outputs;
  sway on its GLES and Vulkan renderers gives one compositor on both, on every tier, the figure
  the survey found missing ([survey](../survey.md#an-own-base-and-vulkan)).
- Blur's budget belongs to Stage 6, measured with Yuna's own renderer's timers.

What no rival measures is what Stage 2's renderer needs to know of each machine: which client
buffers it can import, how it synchronises with them, how a `wl_shm` buffer reaches the GPU, and
what a composition costs at the least. The laptop's risk is also its display engine, which a
prototype on the render node cannot see: jay reports modifiers failing on AMD before RX 5000,
which Mesa's source contradicts for the laptop's GPU.

## Decision

The owner chose, on 2026-11-15:

- **The prototype answers only these**, headless on the render node, on each tier (the desktop
  and lavapipe by an agent; the laptop and the Steam Deck by the owner):
  - the import of each format and modifier the driver offers, compressed ones included, from
    images exported by Vulkan itself;
  - explicit synchronisation: sync files imported and exported, timeline semaphores;
  - a `wl_shm` buffer copied through a staging buffer against its memory imported
    (`VK_EXT_external_memory_host`);
  - the CPU time to record and submit a composition of 1, 6 and 15 windows, and its GPU time by
    timestamp queries, at the output's own size: the floor of a Vulkan frame on that tier;
  - whether a high-priority GPU queue is granted without privileges.
- **The laptop's display is checked by the owner**: `drm_info` and `vulkaninfo`, then sway on
  its Vulkan renderer on the laptop's panel with `vkcube` and mpv for a few minutes, and again
  with modifiers turned off.
- **The code** is Rust on `ash` 0.38, its only new dependency, on a branch of its own that is
  never merged; its commit is named beside its figures. The branch is kept until Stage 2's
  renderer lands, then deleted.
- **A frame's budgets** come from the rivals at equal work
  ([ADR-0072](0072-budgets-set-against-the-peers-the-same-day.md)); the prototype's floor says
  how far below them Vulkan alone can go on each tier.

## Alternatives considered

- **A large prototype**, adding blur, 16-bit blending with PQ, and a comparison with Yuna's
  current GLES: more figures, but ones the rivals give already or that no decision reads.
- **No prototype**, sway's Vulkan on the laptop only: the least work, but import, synchronisation
  and `wl_shm`'s path would stay unknown until Stage 2.
- **`vulkanalia`**, maintained and on Vulkan 1.4: as good for a throwaway; `ash` is the binding
  most Rust graphics code uses. The binding of Yuna's renderer is chosen at Stage 2.
- **GBM to allocate the buffers**: closer to what EGL clients send, but one more dependency;
  sway's run on each tier covers real clients' buffers.

## Consequences

- Stage 6's blur and Stage 5's HDR passes have no prototype figure; their budgets come with
  their stages.
- The owner runs the prototype and the display check on the laptop and the Steam Deck.
- If the laptop cannot import modifiers or run sway's Vulkan renderer on its panel, the lowest
  tier returns to the owner, as ADR-0063 says.
