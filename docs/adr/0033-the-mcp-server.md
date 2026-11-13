# 0033. The MCP server: a private Yuna on stdio, its own MCP layer

- Status: Accepted
- Date: 2026-11-13

## Context

M1.5 gives agents an MCP server ([ADR-0015](0015-agents-workspaces.md)): outputs and workspaces,
programs, captures, input on their own seat, presentation times, the clock. Read on 2026-11-13
([survey](../survey.md#agents-and-mcp)):

- MCP's revision 2026-07-28 is stateless: no `initialize`; each request carries its protocol
  version and the client's capabilities; a server answers `server/discover`, `tools/list`,
  `tools/call`, and must not rely on earlier requests of the same connection. Clients still on
  2025-11-25 send `initialize` first; OpenCode does by default.
- Claude Code, Codex and OpenCode start a server as a command speaking stdio, or reach one over
  Streamable HTTP. None documents Unix sockets.
- Other compositors' agent servers (kwin-mcp, swampcy, computer-use-mcp) are stdio servers that
  start their own private compositor.
- rmcp, the official Rust SDK, Apache-2.0 with older MIT code, conformant to both revisions,
  needs tokio and about 40 crates beside Yuna's 85; 3.0 to 3.10 in three and a half months. niri
  speaks its own IPC, JSON lines, inside calloop, without tokio.

The recommendation was challenged before the owner chose; the challenge turned the first one,
a shared Yuna on a socket, into the private Yuna below.

## Decision

The owner chose, on 2026-11-13:

- **A private Yuna on stdio.** An agent's MCP configuration runs `yuna mcp`: a headless Yuna of
  its own, which speaks MCP on its standard input and output, logs to standard error, and stops
  when its input closes. Its clock, its load, its crashes and its measures are that client's
  alone; no socket is open to other processes. The owner's desktop Yuna, which no agent starts,
  gets its agents' entry at Stage 3, with the authorisation ADR-0015 foresees.
- **Yuna's own MCP layer, on `serde` and `serde_json`.** A tools-only server: both revisions'
  entry, `tools/list`, `tools/call`, cancellation, JSON-RPC's errors, -32022 for a revision it
  does not speak. It reads and answers inside calloop's loop, on the main thread; a call that
  waits, for a frame or a window, answers later from the loop, never from a thread. It keeps no
  state of a connection but the revision a legacy client chose.

## Alternatives considered

- **A shared Yuna on a Unix socket, `yuna mcp` bridging to it**: one Yuna for every agent, and the
  desktop's path from the start. But agents share one drawing thread, about 14 frames a second
  each for ten agents at 4K on llvmpipe (estimated), and one clock; the Yuna a bridge started
  ends with it; any process of the user could take GPU outputs.
- **Streamable HTTP on loopback**: no bridge, but an HTTP server, tokio, origin checks and a
  token, on a port any user of the machine sees.
- **rmcp**: the revisions followed for Yuna, but a second runtime on a thread of its own, each
  tool a round trip to calloop, about 40 crates, and an API moving fast.

## Consequences

- Dependencies: `serde` with `derive`, `serde_json` (MIT OR Apache-2.0, dtolnay): about six crates.
- Yuna follows MCP's revisions itself; the official conformance suite tests servers over HTTP
  only, so Yuna's tests check both revisions' exchanges.
- Ten agents run ten Yuna; each costs nothing at rest.
- In `yuna mcp`, standard output carries MCP alone: programs an agent starts write their output
  to logs the agent can read, never to Yuna's standard output.
