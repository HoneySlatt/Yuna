# 0138. Agents' seats: each agent in its own Yuna, one seat per Yuna from Stage 2's core

- Status: Accepted; supersedes [0116](0116-agents-seats-each-agent-in-its-own-yuna.md)
- Date: 2026-11-19

## Context

[ADR-0116](0116-agents-seats-each-agent-in-its-own-yuna.md) gathered the rule of agents' seats and
left one question to Stage 2's core: keep several seats in one Yuna, or have `seat create` start
a sibling headless Yuna behind the same handles, on M1.11's figures of N Yunas against N seats
([measurements](../measurements.md)). Those figures: an agent's own Yuna costs, beside a seat of a
shared one, about 20 MiB more of anonymous memory on the GPU and 24 to 36 MiB more of the GPU's,
33 to 34 MiB more on llvmpipe; at 6 seats and more, in the quiet cases, N Yunas show every seat's
client 22 to 179 ms sooner, since they start side by side. Stage 2's core is written at Stage 2a
([ADR-0139](0139-stage-2-in-three-gated-parts-2as-path-revised.md)); it would have to isolate
focus, input, selections and outputs between seats if it kept several.

This ADR states the whole rule of agents' seats, ADR-0116's with the question answered.

## Decision

- **Each agent runs its own headless Yuna**, on the GPU when it measures; no agent's output or
  seat enters the owner's desktop Yuna (ADR-0060, 2026-11-14). The owner watches an agent's Yuna
  in the test output's window, at Stage 2 ([ADR-0061](0061-yunas-uses.md) point 5).
- **Stage 3, which was to put agents' outputs and seats in the owner's desktop Yuna, is removed**
  (ADR-0060). The later stages keep their numbers; their versions follow their order: games 0.3,
  the image 0.4, effects 0.5.
- **One seat per Yuna from Stage 2's core**, the owner chose on 2026-11-19. A Yuna holds the
  owner's seat, or one agent's seat. In `yuna mcp`, `seat create` starts a sibling headless Yuna
  serving that seat, behind the same handles and tools; closing the seat stops that Yuna. Until
  Stage 2's core, several seats in one Yuna stay as they are, with their tests, and gain no rule
  between two agents' seats (ADR-0078).
- **New features are written for one seat per Yuna** (ADR-0078) from now on, the core's included.
- **An agent has its own seat**: its pointer, keyboard and focus, apart from the owner's; an
  agent's seat never reaches the owner's (ADR-0015, 2026-11-11).
- **A seat through MCP** (ADR-0078, 2026-11-17). An agent creates a seat and gets its handle,
  which every tool that acts on a seat takes; a stale handle fails plainly. Each seat has its own
  Wayland socket: every client connected through it belongs to that seat, sees only that seat's
  `wl_seat` and outputs, opens its windows on that seat's workspaces, and gets only that seat's
  input. In `yuna mcp` the owner's seat announces no `wl_seat`. When its seat is closed, or its
  Yuna stops, an agent's programs are ended, their whole
  tree under Yuna as subreaper, and its outputs and workspaces removed.
- **A seat starts with an output of its own, drawn for real at full rate**, with frame callbacks
  and presentation feedback as on a shown output, so that measures hold (ADR-0015; ADR-0078); on
  llvmpipe by default, on the GPU's render node with `--gpu`
  ([ADR-0123](0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md)).
- **`yuna --headless --agent-seat` serves an agent's seat** (ADR-0089, 2026-11-17). With it, the
  owner's seat has no output, socket or `wl_seat`, and the agent's seat takes the outputs and a
  socket of its own in `XDG_RUNTIME_DIR`, named as the owner's would be: a `us` keyboard, a
  pointer, Yuna as its input method. Without it, the owner's seat stays testable without a screen,
  for Hollow and the clipboard managers of `ext-data-control-v1`.
- **The virtual keyboard and pointer are offered to the clients of agents' seats alone**, in
  `yuna mcp` too, each device acting on its client's own seat, never the owner's; a virtual pointer
  made without a seat is for its client's seat, which Yuna learns from a patch of its fork of
  Smithay ([patches](../smithay-patches.md)); a test proves it (ADR-0058,
  2026-11-14; ADR-0089). A virtual keyboard's keys switch its seat's keyboard to the device's
  keymap, and Yuna's own keys switch it back.
- **Seats isolate focus and input; Yunas isolate processes' faults.** An agent's programs belong
  to the same user as every other: a seat is not a security boundary between agents. A crash of
  one agent's Yuna ends that agent's seat alone.
- **An agent's Yuna never reads or writes the owner's state**
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 13).

## Alternatives considered

- **Several seats in one Yuna**: less memory per agent; but the new core must isolate focus,
  input, selections and outputs between seats, a crash ends every agent of that Yuna, and the
  desktop and the agents would no longer run the same code (ADR-0082 item 29).
- **Hidden workspaces throttled** ([ADR-0015](0015-agents-workspaces.md)): measures taken there
  would be wrong.
- **Agents on the owner's seat** ([ADR-0015](0015-agents-workspaces.md)): their input would reach
  the owner's windows.
- **Stage 3 as planned** ([ADR-0060](0060-development-first-agents-in-their-own-yuna.md)): a fault
  in an agent's path could end the owner's session.
- **`yuna --headless` always an agent's seat** ([ADR-0089](0089-yuna-for-the-owners-other-projects.md)):
  no test of the owner's seat without a screen.

## Consequences

- Stage 2's core holds one seat; the layout's branch per seat
  ([ADR-0111](0111-the-layout-models-shape-plain-ids-a-branch-per-seat-asked-and-committed-sizes.md))
  holds one.
- `yuna mcp` becomes a front for its sibling Yunas: each has its budgets of start and memory
  ([ADR-0100](0100-the-budgets-of-yuna-mcp-at-their-conditions.md)), and `seat create`'s budget
  then includes a Yuna's start. Ten agents cost, against ten seats in one Yuna, about 190 MiB more
  of processes' memory and 215 to 320 MiB more of the GPU's on the GPU, about 300 MiB more on
  llvmpipe at 1080p.
- What [ADR-0120](0120-the-mcp-server-a-private-yuna-on-stdio.md) and
  [ADR-0117](0117-an-agents-seat-through-mcp-tools-input-keyboard-text-and-closing.md) say of
  `yuna mcp` as one Yuna (its clock, its crashes, one seat at a time on a manual clock) and the
  `events` tool's numbering across the whole Yuna change with the front; its design is put to the
  owner at Stage 2a's seats and input milestone, as a new ADR superseding them in part.
- An agent's output costs GPU time and power while it draws; it draws only while its programs do.
- A program of an agent's seat may inject input on that seat.
- The seat filter of Yuna's fork of Smithay stays until Smithay leaves
  ([ADR-0112](0112-smithay-pinned-patched-in-a-public-fork-left-at-stage-2.md)).

## Validation and reconsideration

Evidence: M1.11's measure of N Yunas against N seats; the stories that a client of one seat sees no
other seat's `wl_seat` or outputs, that an agent's input never reaches the owner's seat, and of
virtual input. Checked at Stage 2a by ten agents' Yunas started together, each within its budgets.
Proposed again if a use needs two seats in one Yuna, such as a test of two seats' input together.
