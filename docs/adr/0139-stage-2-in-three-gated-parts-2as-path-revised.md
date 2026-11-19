# 0139. Stage 2 in three gated parts; 2a's path off Smithay revised

- Status: Accepted; supersedes [0081](0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md);
  supersedes in part [0112](0112-smithay-pinned-patched-in-a-public-fork-left-at-stage-2.md) (when WLCS's baseline is recorded)
- Date: 2026-11-19

## Context

[ADR-0081](0081-stage-2-in-three-gated-parts-and-the-path-off-smithay.md) split Stage 2 into three
gated parts and wrote 2a's path off Smithay: the parity net on today's core, a temporary
boundary over Smithay, the scene and the Vulkan renderer, the protocols from the top down, the
surfaces' store and the queue of commits last, then Smithay removed. Stage 2's opening design, on
2026-11-19, challenged that path against the code and found:

- **The boundary as written cannot carry Yuna's own protocol modules.** A read-only view of
  surfaces and typed requests holds no double-buffered state, yet both of today's own modules keep
  theirs in Smithay's surface data (`colour/protocol.rs`, `xwayland/shell.rs` implement
  `Cacheable`), and so do xdg-shell, viewporter and fractional scale. Built on it, each protocol
  would be written on Smithay's cached state, then again on Yuna's queue.
- **The parity net came after the first change, and saw too little.** The interface's change came
  before any baseline; the figures named (rest, memory, a frame's cost) miss start time, which
  went from 58.7 to 110.3 ms between M1.11 and M1.13 unseen, its cause not isolated, GPU memory, frames with partial
  damage, and soaks; and "no worse" had no rule against the machine's noise.
- **The renderer step changed the scene and the drawing at once**: two suspects, which ADR-0039
  rejected.
- **On the processor tier, a full-damage frame may be slower on Vulkan**: lavapipe composes 4K in
  9.6 to 12 ms in the prototype, today's llvmpipe path draws a full 4K frame in 6.9 to 7.2 ms
  (different scenes).

## Decision

The owner chose, on 2026-11-17, Stage 2 in three parts, each closed by a review at its gate, the
stage's audit still closing Stage 2; and on 2026-11-19 the revised path for 2a.

- **2a, Yuna's own base, headless.** The path, each step a series of commits merged on main:
  1. **Baselines first**, before any change: load-insensitive figures at each push in CI
     (instructions by callgrind, `Pss`, wakes, allocations a frame), and the kernel's DRM memory
     in an agent's runs on the GPU, which CI lacks; timed runs
     compared once per milestone, interleaved, with a run of a build against itself and an
     injected slowdown as controls; start time, GPU memory, a frame with partial damage, a soak;
     protocol traces of scripted clients, times kept on the manual clock and masked on the real
     one; captures read from a buffer the processor maps; tests that pin today's behaviour where
     the debt says none does.
  2. **The interface**: the `Compositor` interface free of Smithay's types; typed commands, which
     become `yuna-ipc`'s command type, in place of `change_layout`; Smithay's re-exports replaced
     by direct dependencies; then WLCS's baseline, kept as the list of tests that pass.
  3. **The crate map's checks, the table of tasks, a frame's identity and one event stream**
     ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
     items 3, 6, 11, 14).
  4. **The boundary**, its contract written first in an ADR: roles, each module's double-buffered
     state applied at commit, checks before a commit that may end a client with a protocol error,
     notices after it, a read-only view and typed requests; implemented over Smithay's cached
     state first; both of today's own modules moved onto it.
  5. **The queue of commits and the surfaces' store, as a pure crate**, tested alone, beside the
     scene ([ADR-0133](0133-yunas-scene-retained-its-damage-found-at-each-latch.md)), also tested
     alone. The boundary's ADR places that crate in
     [ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)'s map.
  6. **The Vulkan renderer behind a switch**, GLES the default until Vulkan matches it in the same
     build; then Vulkan the default and GLES removed. uwui's hosting tested against a stub, off the
     gate's path.
  7. **The protocols, from the top down**: seats and input, xdg-shell and popups, selections and
     text, the rest; XWayland's parts carried in the step that replaces what they sit on.
  8. **The store wired behind a switch**, both stores built and tested in CI, recorded request
     streams replayed into both.
  9. **Smithay removed** once Yuna's store has been the default a week under agents' use, its
     fork and `smithay-patches.md` with it.

  The new code meets ADR-0082 from its first commit. Each new dependency is put to the owner at
  the start of the step that needs it; crates already in `Cargo.lock` that Yuna reaches through
  Smithay become direct dependencies at step 2 without being asked again.
- **WLCS, the Wayland conformance suite, runs against Yuna before the core moves**
  ([ADR-0039](0039-leaving-smithay-step-by-step.md)): Smithay's CI runs four of its groups, and
  Yuna's own core at least matches that. Its baseline is recorded at step 2, after the baselines
  of step 1, as the list of tests that pass.
- **Parity at every step**: the tests unchanged in what they assert; WLCS's passing tests a
  superset of the baseline's; the real applications and the owner's other projects' drivers; the
  traces compared; captures within a tolerance per class (opaque, blended, area-filtered); the
  baselines' figures no worse by the stated rule. **A step that makes any of these worse is
  reverted, not patched forward.**
- **The processor tier is judged on its own budget**
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
  item 26) and on frames with partial damage, the common case, not on a full frame: a full 4K
  frame on lavapipe may be slower than today's llvmpipe path, its figure written. The same scene
  is measured on both paths at step 1; a real loss for agents comes back to the owner.
- **2a's gate**: Smithay out of `Cargo.lock`; parity held; these scenarios of the ROADMAP's bar
  passing headless: a game frozen 10 s under an 8,000 Hz pointer
  ([ADR-0136](0136-a-client-that-stops-reading-1-mib-and-its-motion-merged.md)); one client's
  100,000 damage requests a second; a client's GPU work late at the deadline; a column of three
  windows resized in one frame; the GPU lost during a capture; a new protocol in one crate; an
  agent's Yuna touching nothing of the owner's; ten times the windows, outputs and rate; ten
  agents' Yunas together; one agent's start. The scenarios that need the KMS mock, the lock, the
  keeper or the share picker pass at 2b and 2c.
- **2b, a desktop the owner can log in to**: DRM and KMS, libinput and the session, one output at
  a fixed rate, direct scanout and the cursor plane
  ([ADR-0137](0137-planes-yunas-own-allocator-scanout-and-the-cursor-plane-at-2b.md)), the lock,
  the keeper's restart and the recovery from a GPU reset
  ([ADR-0080](0080-what-survives-a-failure-and-yunas-own-panics.md)), the configuration read, the
  package's NixOS and home-manager modules, Amane on top. Its gate: the owner works in it an
  evening, by the steps its milestone writes, and the scenarios on the KMS mock pass.
- **The owner then sets 2c's scope**, from 2a's and 2b's figures and from what the evening showed.
- **2c, the features** the ROADMAP lists. Stage 2's exit: a week in place of niri, the budgets met
  on the high tier and the laptop, the architecture and stability scenarios passing, no Smithay.

## Alternatives considered

- **ADR-0081's path as written**: fewer changes to the plan; some protocols written twice, and
  regressions able to pass unseen.
- **Strict parity on the processor tier**: the safest for agents, but Vulkan alone could fail
  there and keep GLES as a fallback, against [ADR-0106](0106-vulkan-alone-and-uwui-hosted-for-what-yuna-draws.md).
- **Every dependency of 2a decided now**: a complete plan at once, on facts weeks old when the
  renderer starts.
- **Stage 2 whole, or gates without a written path** (ADR-0081): rejected then for the reasons it
  gives.

## Consequences

- The PLAN holds 2a's milestones in this order; a milestone that turns out too large is split
  with the owner's agreement.
- Two switches live for a while: the renderer's and the store's; CI builds both sides of each.
- The boundary's ADR comes before its milestone's code.
- The queue's model exists long before it is wired, tested alone.

## Validation and reconsideration

Evidence: the challenge of 2026-11-19 against the code at `a551df7` (the modules on Smithay's
cached state, the figures of M1.11 and M1.13, the prototype's lavapipe figures); jay's 27 months
to its 1.0 on a base of its own ([survey](../survey.md#an-own-base-and-vulkan)). Not measured: how
long each step takes. Proposed again if a step cannot keep parity without the next, or if 2a's
figures show the path costs a budget the old core met.
