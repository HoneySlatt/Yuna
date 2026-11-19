# 0078. Agents' seats in one Yuna, frozen until Stage 2's core

- Status: Superseded by [0116](0116-agents-seats-each-agent-in-its-own-yuna.md); supersedes
  [0035](0035-agents-seats.md)
- Date: 2026-11-17

## Context

[ADR-0035](0035-agents-seats.md) let one `yuna mcp` hold several agents' seats, so that Claude
Code's subagents could share their parent's server, and so that agents' seats could later sit in
the owner's desktop Yuna (Stage 3). [ADR-0042](0042-main-on-smithays-fork.md) filtered `wl_seat`
and `wl_output` per client for it, through a patch to Yuna's fork of Smithay.

[ADR-0060](0060-development-first-agents-in-their-own-yuna.md) removed Stage 3: each agent runs
its own headless Yuna, and none enters the owner's. What is left of the reason is subagents
sharing one server. Meanwhile seats cost in every part that knows a client:

- the seat is checked in about nine modules, at several layers (the MCP server, `Compositor`,
  `input.rs`), and each seat has its own socket, Xwayland
  ([ADR-0067](0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)), focus, grab, selections,
  activation and text input;
- a manual-clock Yuna, the deterministic mode, already holds one seat at a time
  ([ADR-0037](0037-seats-tools-input-and-closing.md));
- agents' seats have a fixed `us` keyboard, a pointer and Yuna as their input method, and in
  `yuna mcp` the owner's seat has no `wl_seat`: agents never test the seat the owner uses, against
  [ADR-0074](0074-the-requirements-of-stage-2s-core.md)'s parity;
- seats isolate focus and input, not processes: every seat's socket and log belong to the same
  user, and a program may connect to another seat's socket.

M1.10 and M1.11 would add more that knows seats: virtual keyboards and pointers, clipboard and
drag and drop between programs. Stage 2's core rewrites all of it. One sibling `yuna mcp` per seat
costs 61 to 71 ms to start and about 84 MiB of process memory, measured at M1.9
([measurements](../measurements.md)); several agents' Yunas together are not measured yet.

## Decision

The owner chose, on 2026-11-17, at an audit of the architecture:

- **The seats stay as they are, and nothing new depends on several seats in one Yuna.** An agent
  creates a seat through MCP and gets its handle, which every tool that acts on a seat takes; a
  stale handle fails plainly. Each seat has its own Wayland socket: every client connected through
  it belongs to that seat, sees only that seat's `wl_seat` and outputs, opens its windows on that
  seat's workspaces, and gets only that seat's input. A seat starts with an output of its own,
  drawn at full rate. In `yuna mcp` the owner's seat announces no `wl_seat`. When its seat is
  closed, or its Yuna stops, an agent's programs are ended, their whole tree under Yuna as
  subreaper, and its outputs and workspaces removed. All of this keeps its tests.
- **New features are written for one seat per Yuna.** M1.10's virtual keyboard and pointer, and
  M1.11's tools, work on the seat they are given; none adds a rule between two agents' seats of
  one Yuna. An agent's input never reaching the owner's seat stays tested.
- **M1.11 measures what one Yuna per seat would cost**: start, memory and GPU memory of N `yuna
  mcp` against one with N seats, for N up to 10.
- **Stage 2's core decides, on those figures,** whether it keeps several seats in one Yuna, or has
  `seat create` start a sibling headless Yuna behind the same handles, the MCP tools unchanged.

## Alternatives considered

- **Keep building on several seats**: subagents keep sharing one process, but every milestone
  adds to a mechanism Stage 2 rewrites, for a reason ADR-0060 removed, and agents keep testing a
  seat unlike the owner's.
- **One seat per Yuna now**: the simplest code at once, and Stage 2's core lighter, but a large
  removal of tested code before the measure that would justify it, on code Stage 2 replaces
  anyway.

## Consequences

- M1.10 and M1.11 change in [`PLAN.md`](../PLAN.md): no feature that needs two seats in one Yuna;
  M1.11 adds the measure.
- The seat filter of [ADR-0042](0042-main-on-smithays-fork.md) and its patch stay until Smithay
  leaves.
- Until Stage 2 decides, seats are an isolation of focus and input, not a security boundary
  between agents: an agent's programs may reach another seat's socket of the same Yuna.

## Validation and reconsideration

Evidence at the time: the code read at the audit (`mcp/mod.rs`, `compositor.rs`, `input.rs`,
`seats.rs`), the one-seat rule of a manual-clock Yuna, and M1.9's start and memory figures for one
Yuna. M1.11's measure of N Yunas against N seats is the evidence Stage 2's choice rests on. Proposed
again before then only if a use needs two seats in one Yuna, such as a test of two seats' input
together.
