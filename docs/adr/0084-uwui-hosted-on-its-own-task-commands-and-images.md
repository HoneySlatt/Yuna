# 0084. uwui hosted on its own task, commands and images

- Status: Accepted; supersedes in part
  [0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)
  (how Yuna hosts uwui)
- Date: 2026-11-17

## Context

uwui draws Yuna's own interfaces (the share picker, the overview, Alt-Tab, messages), Hollow and
its lock screen
([ADR-0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)).
Inside Yuna it runs in Yuna's process, on Yuna's Vulkan device. ADR-0073 has the host lend uwui
its device, queue, encoder and target: uwui then records into the frame Yuna composes.

[ADR-0080](0080-what-survives-a-failure-and-yunas-own-panics.md) asks the opposite: uwui on a
task of its own, with its own commands and a timeout on its GPU work, so that Yuna draws uwui's
last good image without waiting for it, and a panic of uwui closes one interface only. Recording
into Yuna's encoder leaves no way to time uwui out, lets a slow interface delay the whole output's
frame, and mixes uwui's GPU time with Yuna's, which ADR-0073 asks to count apart.

Nothing said either how a screen reader would one day read Yuna's interfaces, which the ROADMAP
leaves to later. uwui comes first on any conflict
([ADR-0003](0003-a-parallel-project-uwui-first.md)): what Yuna asks of uwui's hosted mode is
uwui's to accept.

## Decision

The owner chose, on 2026-11-17, at an audit of the architecture:

- **uwui runs on a task of its own** in Yuna's process, one of the tasks
  [ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)'s table
  marks contained: its panic is caught there, closes the interface and builds it again.
- **Each open interface has its own image** on Yuna's Vulkan device. uwui records into it with
  command buffers of its own, submits them on its own, and signals a timeline semaphore when the
  image is done.
- **Yuna composites the last image done, and never waits.** Work past its timeout keeps the
  image before on screen; a lost device falls to
  [ADR-0080](0080-what-survives-a-failure-and-yunas-own-panics.md)'s recovery.
- **uwui's GPU time is read on its own submissions**, against its budget, checked in CI.
- **uwui's accessibility tree leaves its hosted mode**, so that Yuna can give it to a screen
  reader when its interfaces are read, later.
- **The hosted interface is a contract between the two projects**: versioned on uwui's side,
  a revision of uwui pinned in Yuna. Yuna asks it of uwui, which accepts or answers first.
- ADR-0073's other rules stand: Vulkan alone, uwui drawing nothing over a fullscreen game unless
  the owner opens an interface, its time counted in each frame.

## Alternatives considered

- **uwui recording into Yuna's encoder, inline in its frame**, as ADR-0073 had it: the cheapest,
  no image of its own, the lowest latency; but no timeout, no last good image, a slow interface
  delaying the output, and uwui's time not measured apart.
- **The same with uwui on a Vulkan device of its own**, a separate GPU context: a GPU hang of
  uwui might spare Yuna's context, the one isolation from a GPU fault short of another process;
  but images shared by dma-buf between two devices, synchronised by sync files, more memory and
  code, on a driver behaviour not checked: that amdgpu spares an innocent context.
- **uwui in a process of its own**: refused by ADR-0073, the overview and Alt-Tab needing every
  window's image live.

## Consequences

- An open interface costs an image: small for the picker or a message, about 33 MB for an
  overview at 4K.
- An interface's change may show a frame later than if it were drawn inline.
- uwui's hosted mode draws into an image it is given, with its own commands, and gives out its
  accessibility tree: uwui's work, on uwui's calendar, before Stage 2's renderer hosts it.
- Stage 2a measures, with the GPU lost on demand, whether amdgpu lets another device of the same
  process survive a hang it did not cause.

## Validation and reconsideration

Evidence at the time: ADR-0073's and ADR-0080's rules, and Vulkan's: a timeline semaphore lets
one submission be waited on without blocking another, and a device's loss destroys its objects.
Not measured: the latency an image of its own adds, the memory of an overview's image on the
laptop. Proposed again if 2a's measure shows amdgpu spares an innocent device, which would make a
device of uwui's own worth its cost, or if uwui refuses the hosted contract.
