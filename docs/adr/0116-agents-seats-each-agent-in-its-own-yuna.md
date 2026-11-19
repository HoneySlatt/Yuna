# 0116. Agents' seats: each agent in its own Yuna, apart from the owner's seat

- Status: Accepted; supersedes [0015](0015-agents-workspaces.md),
  [0058](0058-what-yuna-takes-from-the-owners-other-projects.md) (virtual input on agents'
  seats), [0060](0060-development-first-agents-in-their-own-yuna.md) (each agent in its own
  Yuna), [0078](0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md) and
  [0089](0089-yuna-for-the-owners-other-projects.md) (an agent's seat in `yuna --headless`,
  virtual input)
- Date: 2026-11-18

## Context

Agents' seats were decided in six ADRs. [ADR-0015](0015-agents-workspaces.md) gave each agent its
own seat and outputs, on the headless mode and on the desktop.
[ADR-0035](0035-agents-seats.md), then
[ADR-0078](0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md), made seats through MCP
and froze them until Stage 2's core.
[ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md) offered virtual input on
agents' seats only. [ADR-0060](0060-development-first-agents-in-their-own-yuna.md) removed
Stage 3, which was to bring agents into the owner's desktop Yuna: each agent runs its own.
[ADR-0089](0089-yuna-for-the-owners-other-projects.md) gave `yuna --headless` an agent's seat.
ADR-0015 still described the desktop's agents and their peek.

This ADR gathers the rule as it stands on 2026-11-18. It decides nothing new. An agent's tools on
its seat are [ADR-0117](0117-an-agents-seat-through-mcp-tools-input-keyboard-text-and-closing.md)'s;
the server they come through,
[ADR-0120](0120-the-mcp-server-a-private-yuna-on-stdio.md)'s.

ADR-0015 put agents' outputs in the owner's desktop Yuna too, with a peek from the overview and a
change of the owner's global rules at Stage 3; ADR-0060 removed Stage 3. ADR-0015 drew agents'
outputs on the GPU; [ADR-0022](0022-the-headless-backend-in-yuna.md) chose llvmpipe by default.
The later rule is the one below.

## Decision

- **Each agent runs its own headless Yuna**, on the GPU when it measures; no agent's output or
  seat enters the owner's desktop Yuna (ADR-0060, 2026-11-14). The owner watches an agent's Yuna
  in the test output's window, at Stage 2 ([ADR-0061](0061-yunas-uses.md) point 5).
- **Stage 3, which was to put agents' outputs and seats in the owner's desktop Yuna, is removed**
  (ADR-0060). The later stages keep their numbers, which the ADRs cite; their versions follow their
  order: games 0.3, the image 0.4, effects 0.5.
- **An agent has its own seat**: its pointer, keyboard and focus, apart from the owner's; an
  agent's seat never reaches the owner's (ADR-0015, 2026-11-11).
- **A seat through MCP** (ADR-0078, 2026-11-17, as ADR-0035 first set it on 2026-11-13). An agent
  creates a seat and gets its handle, which every tool that acts on a seat takes; a stale handle
  fails plainly. Each seat has its own Wayland socket: every client connected through it belongs
  to that seat, sees only that seat's `wl_seat` and outputs, opens its windows on that seat's
  workspaces, and gets only that seat's input. In `yuna mcp` the owner's seat announces no
  `wl_seat`. When its seat is closed, or its Yuna stops, an agent's programs are ended, their
  whole tree under Yuna as subreaper, and its outputs and workspaces removed.
- **A seat starts with an output of its own, drawn for real at full rate**, with frame callbacks
  and presentation feedback as on a shown output, so that measures hold (ADR-0015; ADR-0078); on
  llvmpipe by default, on the GPU's render node with `--gpu`
  ([ADR-0123](0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md)).
- **`yuna --headless --agent-seat` serves an agent's seat** (ADR-0089, 2026-11-17). With it, the
  owner's seat has no output, socket or `wl_seat`, and one agent's seat takes the outputs and a
  socket of its own in `XDG_RUNTIME_DIR`, named as the owner's would be: a `us` keyboard, a
  pointer, Yuna as its input method. Without it nothing changes, so that the owner's seat stays
  testable without a screen, for Hollow and the clipboard managers of `ext-data-control-v1`.
- **The virtual keyboard and pointer are offered to the clients of agents' seats alone**, in
  `yuna mcp` too, each device acting on its client's own seat, never the owner's; a test proves it
  (ADR-0058, 2026-11-14; ADR-0089). A virtual keyboard's keys switch its seat's keyboard to the
  device's keymap, and Yuna's own keys switch it back. A virtual pointer made without a seat is
  for its client's seat, which Yuna learns from `VirtualPointerDevice::client()`, a patch of Yuna's
  fork of Smithay ([patches](../smithay-patches.md)) (ADR-0089, the implementer's choices).
- **New features are written for one seat per Yuna** (ADR-0078). Several seats in one Yuna stay as
  they are, with their tests, but none gains a rule between two agents' seats. Stage 2's core
  decides, on M1.11's figures of N Yunas against N seats, whether it keeps several seats in one
  Yuna or has `seat create` start a sibling headless Yuna behind the same handles.
- **Seats isolate focus and input, not processes** (ADR-0078). Every seat's socket and log belong
  to the same user, and an agent's programs may reach another seat's socket of the same Yuna:
  seats are not a security boundary between agents.
- **An agent's Yuna never reads or writes the owner's state**
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 13).

## Alternatives considered

- **Hidden workspaces throttled, as elsewhere** ([ADR-0015](0015-agents-workspaces.md)): measures
  taken there would be wrong.
- **Agents on the owner's seat** ([ADR-0015](0015-agents-workspaces.md)): their input would reach
  the owner's windows.
- **Stage 3 as planned** ([ADR-0060](0060-development-first-agents-in-their-own-yuna.md)): a fault
  in an agent's path could end the owner's session.
- **Keep building on several seats in one Yuna, or one seat per Yuna now**
  ([ADR-0078](0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)): work on a mechanism
  Stage 2 rewrites, or a large removal before the measure that would justify it.
- **`yuna --headless` always an agent's seat** ([ADR-0089](0089-yuna-for-the-owners-other-projects.md)):
  no test of the owner's seat without a screen.

## Consequences

- An agent's output costs GPU time and power while it draws; it draws only while its programs do.
- Several agents run several headless Yunas: each has its budgets of start and memory
  ([ADR-0100](0100-the-budgets-of-yuna-mcp-at-their-conditions.md)).
- A program of an agent's seat may inject input on that seat.
- The seat filter of Yuna's fork of Smithay stays until Smithay leaves
  ([ADR-0112](0112-smithay-pinned-patched-in-a-public-fork-left-at-stage-2.md)).

## Validation and reconsideration

Evidence: the stories that a client of one seat sees no other seat's `wl_seat` or outputs, that an
agent's input never reaches the owner's seat, and of virtual input; M1.11's measure of N Yunas
against N seats. Proposed again before Stage 2's core only if a use needs two seats in one Yuna,
such as a test of two seats' input together.
