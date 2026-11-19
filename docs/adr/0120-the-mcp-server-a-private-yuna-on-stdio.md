# 0120. The MCP server: a private Yuna on stdio, its own MCP layer

- Status: Accepted; supersedes [0033](0033-the-mcp-server.md),
  [0049](0049-vrr-and-hdr-outputs-through-mcp.md) and
  [0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md) (windows' drawn rectangles
  through MCP)
- Date: 2026-11-18

## Context

[ADR-0033](0033-the-mcp-server.md) gave agents an MCP server: a private Yuna on stdio, with Yuna's
own MCP layer. [ADR-0049](0049-vrr-and-hdr-outputs-through-mcp.md) gave two tools more,
[ADR-0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md) gave `windows` a drawn
rectangle, and [ADR-0055](0055-what-moves-first-and-animations-off-headless.md) turned animations
off in it. [ADR-0060](0060-development-first-agents-in-their-own-yuna.md) removed the Stage 3
that ADR-0033 counted on for an agents' entry into the owner's Yuna, without saying so in
ADR-0033's status. A reader needed four of them.

This ADR gathers the server's rule as it stands on 2026-11-18. It decides nothing new. The tools
themselves are listed in the contract file `crates/yuna/tests/contracts/tools-list.json`, under
[ADR-0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md); the seat's tools are
[ADR-0117](0117-an-agents-seat-through-mcp-tools-input-keyboard-text-and-closing.md)'s, and each
tool M1.11 added has its own ADR, from
[ADR-0090](0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md) to
[ADR-0101](0101-an-agents-clipboard-through-mcp.md).

ADR-0033 gave the owner's desktop Yuna an agents' entry at Stage 3, with the authorisation
[ADR-0015](0015-agents-workspaces.md) foresaw; ADR-0060, later, removed Stage 3, and no such entry
stands.

## Decision

- **A private Yuna on stdio** (ADR-0033, 2026-11-13). An agent's MCP configuration runs `yuna
  mcp`: a headless Yuna of its own, which speaks MCP on its standard input and output, logs to
  standard error, and stops when its input closes. Its clock, its load, its crashes and its
  measures are that client's alone; no socket is open to other processes. No agent reaches the
  owner's desktop Yuna: each runs its own
  ([ADR-0116](0116-agents-seats-each-agent-in-its-own-yuna.md)).
- **Yuna's own MCP layer, on `serde` and `serde_json`** (ADR-0033). A tools-only server: both
  revisions' entry (2026-07-28's stateless one, and `initialize` for clients still on 2025-11-25),
  `tools/list`, `tools/call`, cancellation, JSON-RPC's errors, -32022 for a revision it does not
  speak. It reads and answers inside calloop's loop, on the main thread; a call that waits, for a
  frame or a window, answers later from the loop, never from a thread. It keeps no state of a
  connection but the revision a legacy client chose.
- **Standard output carries MCP alone**: the programs an agent starts write their output to logs
  the agent can read, never to Yuna's standard output (ADR-0033).
- **Outputs like the owner's monitor** (ADR-0049, 2026-11-14). `outputs create` and `seat create`
  take `vrr_min`, in Hz: VRR over [vrr_min, rate], refused unless the rate is at least twice it;
  and `hdr`, a boolean: the PG32UCDM's colour volume, the one monitor Yuna knows. `outputs list`
  shows both, the volume by the monitor's name. `--output` takes the same:
  `3840x2160@240,1.5,vrr=48,hdr`. Without them an output is SDR at a fixed rate.
- **`windows` gives each window's place, its drawn rectangle, and when its motion ends**
  (ADR-0054, 2026-11-14). With `colour: true`, it gives for each window every surface of its tree
  that declared a description, its subsurfaces' and popups' included: the transfer function,
  primaries, luminances, target volume, max_cll and max_fall, and intent (ADR-0049).
  `Compositor::window_colours` gives the same to tests.
- **Animations are off in `yuna mcp`** unless `--animations` or `--reduced-motion` asks
  ([ADR-0109](0109-animations-and-effects-what-moves-reduced-motion-and-games.md)).

## Alternatives considered

- **A shared Yuna on a Unix socket, `yuna mcp` bridging to it** ([ADR-0033](0033-the-mcp-server.md)):
  agents would share one drawing thread and one clock, and any process of the user could take GPU
  outputs.
- **Streamable HTTP on loopback** ([ADR-0033](0033-the-mcp-server.md)): an HTTP server, tokio,
  origin checks and a token, on a port any user of the machine sees.
- **rmcp, the official Rust SDK** ([ADR-0033](0033-the-mcp-server.md)): a second runtime on a
  thread of its own, each tool a round trip to calloop, about 40 crates, an API moving fast.
- **Named monitor profiles** ([ADR-0049](0049-vrr-and-hdr-outputs-through-mcp.md)): a list to keep
  for one monitor.
- **Colours always in `windows`** ([ADR-0049](0049-vrr-and-hdr-outputs-through-mcp.md)): a heavier
  reply for every agent that does not ask.

## Consequences

- Yuna follows MCP's revisions itself; the official conformance suite tests servers over HTTP only,
  so Yuna's tests check both revisions' exchanges.
- Ten agents run ten Yunas; each costs nothing at rest, within the budgets of
  [ADR-0100](0100-the-budgets-of-yuna-mcp-at-their-conditions.md).
- A second monitor's volume, the laptop's, adds a name to `hdr`.

## Validation and reconsideration

Evidence: the tests of both revisions' exchanges (`tests/headless/mcp.rs`), and the contract file
checked against what the server lists (`tests/contracts.rs`).
