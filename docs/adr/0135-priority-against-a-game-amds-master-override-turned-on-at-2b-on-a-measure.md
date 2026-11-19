# 0135. Priority against a game: the mechanism now, turned on at Stage 2b on a measure

- Status: Accepted; supersedes in part [0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) (item 12, priority)
- Date: 2026-11-19

## Context

When Yuna composites a game (windowed, or fullscreen without direct scanout), a composition that
runs late shows the game's finished frame a refresh later. Yuna's composition is small (56 to
75 µs at 4K on the desktop, 1.1 to 1.4 ms at 1080p on the laptop, measured in M1.9): priority
changes when it runs, not what it costs.
[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 12 left
the choice to Stage 2's opening, "measured first on the desktop as display master", which cannot
happen before Stage 2b; item 24 keeps agents' Yunas at normal priority.

Read on 2026-11-19 in Linux 6.18.60 and Mesa 26.2.6 ([survey](../survey.md#blending-and-a-compositors-priority)):

- amdgpu grants a context above normal priority to `CAP_SYS_NICE`, or to the DRM master's own open
  file; RADV makes its contexts on the render node, never master, so a Vulkan queue at HIGH needs
  `CAP_SYS_NICE`, as M1.9 found (`VK_ERROR_NOT_PERMITTED`).
- amdgpu's `AMDGPU_SCHED` ioctl, allowed to the DRM master alone, raises every context of an
  amdgpu file it is given by descriptor, the render node's included, without any capability. No
  rival uses it.
- A high-priority graphics pipe exists on RDNA2 (the desktop's RX 6900 XT, the Steam Deck); on the
  laptop's Raven, high priority only orders the kernel's software queue.
- i915 and Xe grant priority to `CAP_SYS_NICE` alone.
- KWin holds `CAP_SYS_NICE` by a file capability and runs its commit thread at `SCHED_RR`; Mutter
  runs its KMS thread at a high nice through rtkit, having found realtime could get it killed in a
  long atomic commit; jay keeps the capability in one thread for life. A NixOS security wrapper
  points at one build, while Yuna's keeper restarts the very build it started; threads a
  privileged thread starts, Mesa's among them, inherit the capability. Nobody measured what a game
  gains or loses.

## Decision

The owner chose, on 2026-11-19, **the mechanism now, turned on for the session's Yuna at Stage 2b
only if a measure asks for it.**

- **Agents' Yunas run at normal priority, always** (item 24). They are never DRM master.
- **No privilege by default.**
- **On AMD, the master's override.** The session's Yuna, DRM master from 2b, raises its
  contexts with `AMDGPU_SCHED`. The override raises every context of the file it is given, and
  RADV keeps one file per physical device: on Yuna's one device
  ([ADR-0106](0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md)), captures, uploads and
  uwui are raised with the composition. Raising the composition alone would need a device of its
  own and images crossing devices, which ADR-0106 rejects for uwui: if 2b's measure shows the rest
  costing a game, that choice comes back to the owner then.
- **On Intel, `CAP_SYS_NICE`** only if the owner chooses it for a machine; none of the owner's
  machines has an Intel GPU. If it is ever used: one thread holds it for Yuna's life and takes
  typed requests, devices are enumerated outside it, a test reads every thread's capabilities,
  and the keeper is the binary that holds it.
- **The KMS thread** runs at a high nice through rtkit; Yuna's NixOS module (2b) turns rtkit on,
  and without it the thread keeps the normal nice. Realtime only with a soft `RLIMIT_RTTIME`
  below the hard one, a `SIGXCPU` handler that demotes the thread, and realtime dropped around
  probes and mode sets.
- **What Stage 2a's core carries**: the device's queue priority as a parameter, a change rebuilding
  the device through the one path for device loss (item 8); separate queues for the composition and
  for everything else; a deadline lead taken from the GPU's timestamps, as KWin's render journal,
  the base of every option; the priority granted in the frame's log (item 11); a scheduling class
  per task in the table of tasks (item 6); a spawner that clears ambient capabilities and resets
  the scheduling of every program Yuna starts.
- **The measure at 2b**, an owner's run: the desktop, the laptop (also with `amdgpu.mcbp=1`) and
  the Steam Deck; a GPU-bound windowed game and a GPU-bound fullscreen game Yuna composites, a
  build loading the processor beside them, a capture running; normal priority against the
  override. It passes when no composited commit is presented after the first boundary it could
  reach ([ADR-0076](0076-input-to-present-read-by-mode.md)) and the game's frame times are no
  worse than at normal priority. The result is an ADR; it is measured again at Stage 4, with
  overlay planes.

## Alternatives considered

- **`CAP_SYS_NICE` by a security wrapper, held by one thread** (jay): works on Intel too; but a
  privilege the owner must grant, inherited by Mesa's threads, at odds with the keeper's pinned
  build.
- **Normal priority always, a deadline lead alone**: the simplest; nothing keeps a GPU-bound game
  from delaying a composition.
- **Deciding on and off now**: no measure exists before Yuna is display master.

## Consequences

- The override is AMD's alone and untried by any rival: 2b checks it does what the kernel's source
  says, on the owner's kernels (7.3.1 on the desktop, 7.1.5 on the laptop).
- The ioctl needs `unsafe` in the KMS crate, which owns drm-rs.
- The ROADMAP's open decision on priority closes but for its on and off, which 2b's measure takes.

## Validation and reconsideration

Evidence: amdgpu's `amdgpu_ctx.c`, `amdgpu_sched.c` and `amdgpu_drv.c` in Linux 6.18.60, read and
checked; the rivals' sources; M1.9's refusal of a high-priority queue on every tier. Proposed
again if the override does not hold on the owner's kernels, or if 2b's measure shows a game losing
frames to Yuna's priority.
