# 0082. The requirements of Stage 2's core, its bets made rules or deferred

- Status: Accepted; supersedes [0074](0074-the-requirements-of-stage-2s-core.md);
  superseded in part by [0137](0137-planes-yunas-own-allocator-scanout-and-the-cursor-plane-at-2b.md) (item 16, planes) and [0135](0135-priority-against-a-game-amds-master-override-turned-on-at-2b-on-a-measure.md) (item 12, priority)
- Date: 2026-11-17

## Context

At Stage 2's opening Yuna writes its own core on wayland-rs and leaves Smithay, along the path of
[ADR-0081](0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md): surfaces, xdg-shell,
seats, its scene and damage tracking, its Vulkan renderer hosting uwui
([ADR-0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md)),
its DRM and KMS backend, input and the session. Some properties cost little to design into such a
core and very much to add once it exists. The rivals show it: wlroots, KWin, Mutter, Hyprland and
niri each added a queue of pending commits late; KWin rebuilt its scene; Mutter, KWin and
aquamarine moved KMS work onto a thread after the fact; Hyprland pulled its backend out into
aquamarine.

Today the `yuna` crate is about 25,000 lines around one `State` of 52 fields, 43 of them public,
that implements 22 of Smithay's handler traits: Smithay's shape, in which any handler can change
anything. The project's bar counts architectural quality: explicit contracts,
controlled dependencies, owned state and task lifetimes, bounded queues, contained failures,
contracts evolved deliberately, judged by realistic change and failure scenarios.

The facts come from a challenge of 2026-11-16 that read the pinned sources of wlroots 0.20.2,
sway 1.12, Hyprland 0.57.1, aquamarine 0.16.0, niri 26.04, gamescope 3.17.0, KWin 6.8.2 and
Mutter 51.0 ([survey](../survey.md#stage-2s-core-what-the-rivals-reworked)).

[ADR-0074](0074-the-requirements-of-stage-2s-core.md) set these requirements on 2026-11-16. The
[audit of 2026-11-17](../audits/2026-11-17-architecture.md) and its challenge kept most of them,
and found a few bets among them, built before anything could measure them: copies between GPUs
and agents on a second GPU, with no machine of the owner's holding two; eviction of GPU memory, at
147 MiB for 150 windows ([measurements](../measurements.md)); a message catalogue, with no second
language asked; a fallback to the previous binary, which NixOS's held store paths make needless;
ten agents' Yunas each at full rate, which a headless test without `--gpu` cannot pass; the
twenty-odd crates created before their code. Two edges of the map broke its own rules: libinput
opens its devices through the session, a crate of the same level, and through a D-Bus call that
would make the state thread wait.

## Decision

The owner chose, on 2026-11-16, that Stage 2's core meets these requirements from its first
commit, and on 2026-11-17 kept them, its bets made rules or deferred (below). Four choices inside
them are made at Stage 2's opening design, each on a measure (below).

### The core's foundations

1. **A queue of pending commits per surface.** A commit applies only when its blockers clear: the
   buffer's GPU work (an implicit dma-buf fence or a `linux-drm-syncobj-v1` acquire point), a
   `fifo-v1` barrier, a `commit-timing-v1` target, a synchronised subsurface's parent, a
   transaction across clients so that one layout change shows in one frame, or a timeout.
   Every extension's double-buffered state rides the same queue. Blockers are file descriptors on
   the event loop: no thread ever waits. The queue is bounded, its behaviour at the bound decided.
   A buffer lives while a texture, a plane or a capture uses it; its release comes from its last
   user, a KMS flip included.
2. **One owner of the state.** Each protocol module owns only its own objects, reads the rest
   through a narrow read-only interface, and asks for changes with typed requests the core applies
   after dispatch. No shared mutable global state. `unsafe` only in the crates that own a foreign
   library; `#![forbid(unsafe_code)]` everywhere else.
3. **Crates with checked dependencies** (the map below), so that a contributor works on one part
   without the whole, and the core evolves part by part. The map's rules on edges are checked
   from the first commit; each crate is created when its code exists, not before, except
   `yuna-ipc`, which Hollow depends on.
4. **The scene** renders a target for each consumer (the output, a capture with never-captured
   windows blanked and colour filters kept out, an SDR capture of HDR), keeps snapshots (a closed
   window's last image, a resize's crossfade, previews for the overview, Alt-Tab and Hollow),
   tracks damage per output buffer and per plane, costs in proportion to what changed, and takes
   the number of frames in flight per output as a parameter (one, or two when the GPU climbs).
   Each is tested on the scene alone, as the layout's behaviours are.
5. **Colour**: the blending space and its precision are chosen at Stage 2's opening, not at
   Stage 5. Colour is one pipeline of standard steps (curves, matrices, lookup tables, a
   multiplier) that run in a shader or on the display hardware, on planes and after blending; the
   final steps run in a fixed order: night light, filters, dimming, display profile, encoding.

### Running and surviving

6. **A table of tasks**: each thread or task has an owner, a bounded inbox, its behaviour when
   full, its place in the stop order (at exit, at a session pause, at a GPU loss), and a failure
   either contained (an encoder, uwui, the X11 manager) or fatal (the state thread, the KMS
   thread). Threads exchange messages and immutable snapshots, never shared state. No blocking
   wait on the state thread. Every task can run inline under the manual clock, so tests stay
   deterministic. A watchdog in the keeper restarts Yuna when it hangs, not only when it crashes.
7. **Output changes tested whole**: a change of modes, positions, scale, VRR, HDR or mirroring,
   from a reload, a hotplug, a variant or a command, is tested against the kernel as one
   configuration, then applied whole or not at all. The configuration is a pure function of its
   layers, validated and applied in one round, with a generation number events carry. The last
   configuration kept is the last the hardware accepted
   ([ADR-0008](0008-configuration-in-kdl.md)).
8. **One path for pause, resume, sleep and device loss**: a VT switch, suspend and resume, the
   greeter's hand-over, a GPU reset ([ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)),
   a GPU or input device gone. While paused nothing draws or polls; on resume outputs are probed
   again and buffers imported again. The clock owns wall time and the time spent asleep.
9. **The lock, enforced by the scene and kept through a restart**: while locked every output,
   new ones included, shows only lock surfaces, no plane keeps another buffer, captures see only
   the lock, input reaches only the locker. Yuna restarted after a crash while locked comes back
   locked; the first frame after a resume is the lock.

### Input, latency and measure

10. **One ordered input chain**: accessibility filters, the lock, VT switching, Yuna's shortcuts
    and gestures, inhibitors, pointer constraints, input methods, then the client. Every source
    (libinput, agents' virtual devices, MCP, EIS later) enters it the same way, with its kernel
    timestamp. The cursor plane moves from the input path, not the state thread.
11. **A frame's identity from input to presentation**: one id carried by input, commits,
    drawing, the GPU's start and end on `CLOCK_MONOTONIC`, the KMS commit and the presentation
    feedback, with the reason a surface was not scanned out. Every wake-up is named by its source.
12. **Priority against a game**: whether Yuna's GPU queue and commit thread may run ahead of a
    game is decided at Stage 2's opening (below); RADV refuses a high-priority queue without a
    privilege on every tier ([measurements](../measurements.md)).

### Contracts and evolution

13. **Every public contract listed** with its version, its promise, its tested migrations and its
    way back: the configuration, the session-restore state, the theme file, Hollow's interface,
    MCP's revisions, the stores (remembered sizes, display profiles, crash reports), and the
    keeper's hand-over with Yuna. A file is read across versions keeping what it does not know;
    a backup precedes any migration. The keeper restarts the very build it started, held open by
    a file descriptor; a new build starts at the next login, so that a system update never
    restarts the session into an untried binary. An agent's Yuna never reads or writes the owner's
    state.
14. **One command type and one event stream** for key bindings, gestures, hot corners, the
    configuration's actions, Hollow, MCP and the tests, dispatched knowing the caller; events
    bounded per subscriber, a resync event at the bound.
15. **Client identity from the connection**: the peer's pidfd, a security context's engine, app
    and instance ids, an agent's or Hollow's role, recorded when the client connects; privileged
    protocols filtered per client from Stage 2; per-client budgets extending
    [ADR-0028](0028-bounds-on-what-one-client-may-ask.md). The keeper also holds the sockets
    sandboxed clients reconnect through.

### What the scene, the renderer and the backends carry from the start

16. **Planes**: per-surface dmabuf feedback with a scanout tranche, scanout only when transform
    and colour match or a plane converts, damage clips sent to KMS, the cursor's scale bound to the
    primary's on AMD, tearing as a commit that changes nothing else; the planes themselves come at
    Stage 4.
17. **Output transforms** (rotation, flips, a panel's own orientation) in the mapping from logical
    coordinates to pixels, tablets and touch mapped through the same transform.
18. **Accessibility**: zoom is an output transform in the scene; colour filters are final colour
    steps; their input half joins the input chain.
19. **Shaders**: a closed set of pipelines, compiled before first use on a thread of their own,
    never inside a frame; effects as parameters, not new pipelines; compiled again after a device
    loss.
20. **Several GPUs, as a rule of interfaces**: no global device; each output and each buffer names
    its device. Copies between devices, and dmabuf feedback per device, wait for a machine with two
    GPUs to measure them on.
21. **GPU memory counted**: every allocation counted per client and in total, a budget per tier;
    `wl_shm` uploads cover only the damaged region. Eviction waits for a measure that needs it.
22. **Test seams**: the KMS crate behind an interface, with a mock that refuses commits, returns
    busy, loses planes and connectors; a fake session; input injected at libinput's level; the GPU
    lost on demand; a WLCS shim; fuzz entry points per protocol module.
23. **Yuna's own text** in whole sentences, in one module, never built by joining pieces; a
    message catalogue waits for a second language.

### Headless and agents

Each agent runs its own headless Yuna ([ADR-0060](0060-development-first-agents-in-their-own-yuna.md)):
the core serves it as it serves the desktop.

24. **Agents' Yunas at normal priority**, never lowered for a game: the owner accepts that a game
    loses frames to agents' work while developing. Only the session's Yuna may get a high
    priority, chosen at Stage 2's opening.
25. **Budgets per instance, and many together tested for growth**: start to the first frame,
    memory and GPU memory have a budget per instance; ten instances started together grow nothing
    without end and share no bottleneck, checked by a soak test. The core loads nothing before its
    first frame that the frame does not need.
26. **The CPU renderer as a tier**: lavapipe, CI's and an agent's without a GPU, holds a budget of
    its own; it draws only what changed (a 4K composition costs lavapipe 10 to 12 ms,
    [measurements](../measurements.md)).
27. **Agents' caches**: their XDG folders are thrown away, so the shaders an instance needs come
    precompiled or from a cache the instances share read-only, never compiled again at each start.
28. **Agents on a second GPU** wait, with item 20's copies between devices, for a machine that
    has one.
29. **Parity**: no path in the core for headless alone; both modes run the same code, only the
    backend differs, so that what agents test is what the owner runs.

### The crates

"→" reads "depends on". Edges go only down a level; the crates of level 2 never depend on each
other and meet only in `yuna-core`. The session opens the input devices, and the core hands their
descriptors to `yuna-input` ahead of use: libinput's opening of a device answers from them,
never through `yuna-session` nor a D-Bus call that would make the state thread wait.

| Level | Crates | Depend on |
|---|---|---|
| 0 | `yuna-types`: ids, coordinate spaces as distinct types, transforms, scale in 120ths, time values, dma-buf descriptors | nothing |
| 1 | `yuna-layout`, `yuna-motion`, `yuna-clock`, `yuna-colour`, `yuna-scene`, `yuna-output`, `yuna-config`, `yuna-ipc` (the commands and events, the crate Hollow uses, under semver) | level 0, and within level 1 only scene → colour, output → clock and scene |
| 2 | one crate per outside library: `yuna-wayland`, `yuna-render` (Vulkan, uwui's hosting), `yuna-kms`, `yuna-headless`, `yuna-input`, `yuna-session`, `yuna-xwm`, `yuna-process` | levels 0 and 1 |
| 3 | `yuna-core`: the state, the loop, the frame scheduler, the command dispatcher, the `Compositor` interface the tests drive | everything below |
| 4 | `yuna`, `yuna-mcp`, `yuna-keeper` (types and IPC only), `yuna-portal`, `yuna-wlcs`, fuzz targets | the core's interface |

`yuna-bench` stands outside the map: it measures Yuna from outside as it measures the rivals
([ADR-0075](0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)), and
depends on no crate of Yuna's; Yuna's measuring examples and tests may use its readers as a
development dependency, never a crate's own code.

`scripts/check.sh` checks: the edges; one owning crate per outside library; `forbid(unsafe_code)`
outside those crates; no `static` with interior mutability nor `thread_local!` outside logging.
Each crate's documentation states what it promises, its bounds, its failure behaviour, and what it
never does (the scene never touches the GPU; the KMS crate never blocks its caller; the Wayland
crate never changes the core's state itself).

### Chosen at Stage 2's opening, each on a measure

- **The blending space and precision**, per tier: KWin's gamma 2.2 in [0, 1], which needs no
  float images, against linear light in 16-bit floats (on the desktop 56 against 75 µs a 4K
  composition; on the laptop 1.1 against 1.4 ms at 1080p).
- **Priority**: `CAP_SYS_NICE` for the session's Yuna only, never agents', or normal priority with
  a deadline lead that absorbs a GPU-bound game; measured first on the desktop as display master.
  A privilege: the owner decides.
- **wayland-rs's 4,096-byte buffer per client**, where the rivals raise libwayland's to 1 MiB: how
  long a client that stops reading survives an 8,000 Hz pointer, measured headless; then motion
  merged for a lagging client, a patch in a fork, or wayland-rs on libwayland
  ([ADR-0040](0040-the-wire-protocol-stays-wayland-rs.md)).
- **The plane allocator**: Yuna's own, or libliftoff, a dependency.

## Alternatives considered

- **Leave these to the stages that use them** (planes at Stage 4, colour at Stage 5,
  permissions at Stage 6): each stage simpler, but each would rework the core written at Stage 2,
  as the rivals did.
- **Today's shape, handler traits on one state**: the fastest to write; nothing is owned, and a
  contributor must know the whole.
- **A thread per protocol**: ruled out by wayland-server's single dispatch state.
- **One crate for the core**: fewer manifests; nothing checks its directions, and the Vulkan and
  KMS code build for every change.
- **ADR-0074 as written**: everything foreseen at once, but code built and kept that nothing can
  measure, an exit no headless test can pass, and a map against its own rules.
- **Its bets left to Stage 2's opening design**: decided nearer the code, but the map's faults
  stay written meanwhile, beside the four choices that design already measures.

## Consequences

- Stage 2's opening design starts from this list and the crate map; its scene, its colour space,
  priority, the client buffer and the plane allocator get ADRs of their own.
- The ROADMAP's bar holds the scenarios below; Stage 2's exit includes them.
- Smithay's shape stays in today's code until Stage 2 replaces it; its unbounded X11 command
  channel and its global `OPENED_DISPLAYS` are debt ([debt](../debt.md)).
- Several GPUs are a rule of the core's interfaces; what needs two GPUs waits for a machine that
  has them.
- What is deferred (copies between GPUs, agents on a second GPU, eviction, a message catalogue)
  comes back as a new decision when a machine, a measure or a second language asks for it.

## Validation and reconsideration

Evidence at the time: the rivals' sources read on 2026-11-16 (each item's paths in the survey),
the prototype's figures of [ADR-0073](0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md),
today's code, and the audit of 2026-11-17. The rivals' dates of rework are remembered, not read.
Judged by the scenarios of the ROADMAP's bar: a game frozen 50 ms under an 8,000 Hz pointer is not
ended; 100,000 damage requests a second from one client cost the others no boundary; a client's
GPU work late at the deadline shows its previous buffer; a column's resize shows in one frame; an
output unplugged while another changes mode leaves a valid state; a refused mode changes nothing;
a GPU reset during a capture and a scanned-out game is recovered; suspend while locked resumes on
the lock; a crash while locked restarts locked; a system update never restarts the session into
an untried binary; an older Yuna keeps a newer file's fields; a new protocol changes one crate;
the KMS thread run inline passes the same test; uwui's panic closes its interface only; an
agent's Yuna writes nothing of the owner's; a sandboxed client reconnects after a crash; ten
agents' Yunas started together grow nothing without end; an instance shows its first frame
within a budget. A requirement that a measure at Stage 2's opening shows wrong is put to the
owner again, with the figure.
