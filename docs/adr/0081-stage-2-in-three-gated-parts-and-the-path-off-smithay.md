# 0081. Stage 2 in three gated parts, and the path off Smithay

- Status: Superseded by [0139](0139-stage-2-in-three-gated-parts-2as-path-revised.md); supersedes in part
  [0039](0039-leaving-smithay-step-by-step.md) (how the core moves)
- Date: 2026-11-17

## Context

[ADR-0039](0039-leaving-smithay-step-by-step.md) has Yuna leave Smithay step by step, on main,
the program working at each commit, and moves the core at Stage 2's opening: surfaces,
xdg-shell, seats and popups, with Yuna's own scene. [ADR-0074](0074-the-requirements-of-stage-2s-core.md)
sets what that core meets from its first commit. Neither says how today's code becomes it, and
the ROADMAP's Stage 2 holds, with one exit, Yuna's own core and Vulkan renderer hosting uwui,
DRM, input and the session, and about seventy features.

What the code imposes, read at the [audit of 2026-11-17](../audits/2026-11-17-architecture.md):

- Leaf protocols moved first, as ADR-0039 asked: two globals in nine milestones
  (`color-management-v1`, the X11 shell), both still keeping their state in Smithay's surfaces.
  Yuna's handlers can sit on Smithay's surfaces (about 35 sites read them); Smithay's handlers
  cannot sit on Yuna's, since each finds its state in Smithay's own surface data. The surfaces,
  subsurfaces and the queue of commits therefore move last, at once.
- About 290 tests drive the compositor through the real protocol, with `wayland-client`: they
  hold Yuna's behaviour whatever its base. The `Compositor` interface they drive still names
  Smithay's types, and they change the layout through `change_layout`, a raw `&mut Layout`.
- ADR-0039 rejected one long branch that changes everything at once, where each failure has
  three suspects.

## Decision

The owner chose, on 2026-11-17: **Stage 2 comes in three parts, each closed by a review at its
gate, and Yuna leaves Smithay along a written path.**

- **2a, Yuna's own base, headless.** The path, each step a series of commits merged on main:
  1. **The parity net, on today's core**, as Stage 2's first milestone: the `Compositor`
     interface free of Smithay's types; typed commands in place of `change_layout`; Smithay's
     re-exports replaced by direct dependencies on wayland-server, calloop and rustix; WLCS's
     baseline recorded.
  2. **A temporary boundary**: the read-only view of surfaces and the typed requests of
     [ADR-0074](0074-the-requirements-of-stage-2s-core.md), implemented over Smithay first.
  3. **The scene and the Vulkan renderer** behind that boundary, uwui hosted in it.
  4. **The protocols, from the top down**: seats and input, xdg-shell and popups, then the rest.
  5. **The surfaces' store and the queue of commits**, Yuna's own behind a build switch, both
     stores built until the switch is the default.
  6. **Smithay removed**, its fork and `smithay-patches.md` with it.

  The new code meets ADR-0074 from its first commit; the temporary boundary lives in today's
  code and leaves with it.
- **Parity at every step**: the tests unchanged; WLCS at or above its baseline; the real
  applications of [ADR-0025](0025-applications-in-ci.md) and the owner's other projects' drivers;
  the protocol traces of scripted clients compared between the two cores, serials and times
  masked; captures within a stated tolerance; rest, memory and a frame's cost no worse than the
  old core's, measured the same day.
- **A step that makes any of these worse is reverted, not patched forward.** Smithay's surfaces'
  store is deleted only after a week as the default under agents' use.
- **2a's gate**: Smithay out of `Cargo.lock`, parity held, the headless scenarios of the
  ROADMAP's bar passing.
- **2b, a desktop the owner can log in to**: DRM and KMS, libinput and the session, one output
  at a fixed rate, the lock, the keeper's restart and the recovery from a GPU reset
  ([ADR-0080](0080-what-survives-a-failure-and-yunas-own-panics.md)), the configuration read, the
  package's NixOS and home-manager modules, Amane on top. Its gate: the owner works in it an
  evening, by the steps its milestone writes, and the scenarios on the KMS mock pass.
- **The owner then sets 2c's scope**, from 2a's and 2b's figures and from what the evening
  showed.
- **2c, the features** the ROADMAP lists. Stage 2's exit stays: a week in place of niri, the
  budgets met on the high tier and the laptop, the architecture and stability scenarios passing,
  no Smithay.

## Alternatives considered

- **Stage 2 whole, as written**: the plan exists, and contracts that end on real hardware (a
  frame's identity to the flip, the lock over planes) are designed whole. But the hardware
  answers only once seventy features rest on the core, nothing sets a moment to revisit the
  scope, and without a path the core risks the long branch ADR-0039 refused. The contracts are
  still designed whole at Stage 2's opening and tested through ADR-0074's seams.
- **The gates without a written path**, the order left to Stage 2's opening design: nothing to
  decide in detail now, but the stage's riskiest question stays open until its last moment.

## Consequences

- The ROADMAP's Stage 2 is written in its three parts, each with its gate.
- Stage 2's first milestone is the parity net on today's code, the one change to today's core
  that prepares the next: [`AGENTS.md`](../../AGENTS.md) forbids it during Stage 1.
- A temporary boundary is written over Smithay and thrown away; two surfaces' stores are built
  side by side for a while.
- Each gate adds a review; the stage's audit still closes it.

## Validation and reconsideration

Evidence at the time: the code read at the audit (the uses of Smithay's surface data, the
`Compositor` interface, the tests), and jay's 27 months to its 1.0 on a base of its own
([survey](../survey.md#an-own-base-and-vulkan)). Not measured: how long each step takes. Proposed
again if a step cannot keep parity without the next, or if 2a's figures show the path costs a
budget the old core met.
