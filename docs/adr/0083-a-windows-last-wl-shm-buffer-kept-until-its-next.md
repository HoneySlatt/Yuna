# 0083. A window's last `wl_shm` buffer kept until its next

- Status: Accepted; supersedes in part
  [0077](0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md) (the clients'
  buffers Yuna holds)
- Date: 2026-11-17

## Context

A client that draws on the CPU hands Yuna its image in shared memory (`wl_shm`); Yuna copies it
into a texture. Yuna may then give the buffer back at once (`wl_buffer.release`), as wlroots'
compositors, Umbriel and Hyprland do, or keep it until the client sends the next, as KWin, Mutter
and Yuna today do. At 15 windows Yuna holds 30.4 MiB of clients' buffers, KWin and Mutter
39.6 MiB, the others none ([measurements](../measurements.md)).

Two decisions pulled apart:
[ADR-0077](0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md) called the buffers
Yuna keeps debt, to be released once uploaded at Stage 2;
[ADR-0080](0080-what-survives-a-failure-and-yunas-own-panics.md) brings a window's content back
after a GPU reset "from Yuna's own copy where it keeps one".

With Vulkan, a lost device destroys every texture of Yuna's, whether or not video memory is lost.
A client's dmabuf is the client's and can be imported again; a `wl_shm` window can be drawn again
only from its buffer, and a client at rest, a terminal waiting, does not draw by itself. On the
laptop, whose GPU shares its 4 GiB, a kept buffer and its texture are two copies in memory: RADV
refuses a memfd's pages as a host pointer (measured at M1.9).

## Decision

The owner chose, on 2026-11-17, at an audit of the architecture:

- **Yuna keeps each window's last `wl_shm` buffer until its client sends the next**, and releases
  it then, or when the surface or the client goes. A texture lost with the GPU is uploaded again
  from it, whole; otherwise uploads cover only the damaged region
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)).
- **The buffers Yuna holds are counted**, per client and in total, beside the process's memory,
  as ADR-0077 records them; they are a choice, no longer debt.
- **One path on every tier.** If the laptop's memory misses its budget, the remedy is eviction,
  which kept buffers leave possible (a hidden window's texture dropped and uploaded again from its
  buffer), not a release that would lose the windows at a reset.

## Alternatives considered

- **Release once uploaded**, as wlroots: the least memory held, one copy on the laptop; but after
  any GPU reset every `wl_shm` window stays empty until its client draws again, which a
  `configure` may prompt but nothing guarantees, and eviction is ruled out for good.
- **Release on the laptop, keep on the desktop**: each tier its fit, but two paths to write and
  test against ADR-0082's one path for pause and loss, and empty windows after a reset on the
  laptop.

## Consequences

- ADR-0080's promise holds for every window, those at rest included.
- A client with a single buffer must allocate a second to draw while Yuna holds the first, as
  most clients do already.
- The debt's item on the buffers Yuna keeps is replaced by this decision.
- On the laptop a `wl_shm` window costs its buffer and its texture; measured at Stage 2b.

## Validation and reconsideration

Evidence at the time: M1.9's figures of the buffers compositors hold, its prototype's refusal of a
memfd as a host pointer on RADV, and Vulkan's rule that a lost device's objects are all destroyed.
Not measured: the laptop's memory with Yuna's Vulkan renderer. Proposed again if that measure
misses the laptop's budget and eviction cannot bring it back, or if a driver imports `wl_shm`
pages as a host pointer, which would make the kept buffer the texture's only copy.
