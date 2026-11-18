# 0101. An agent's clipboard through MCP

- Status: Accepted
- Date: 2026-11-18

## Context

M1.11 gives agents the clipboard between programs as a situation they test
([ADR-0061](0061-yunas-uses.md)); [ADR-0091](0091-the-clipboard-between-x11-and-wayland-who-reads-and-its-bounds.md)
bridges it between a seat's X11 and Wayland programs. An agent also needs to check what a program
copied and to give a program something to paste, through MCP alone. Smithay lets the compositor
hold a seat's selection and read a client's into a pipe; a read is answered only when the program
that copied writes it. A request's line is 1 MiB at most ([ADR-0033](0033-the-mcp-server.md)).

## Decision

Chosen while building M1.11, as implementation:

- **`clipboard get`** gives the MIME types of a seat's clipboard or primary selection, and the data
  of one, the first text type by default: as text when it is UTF-8, else in base64; 64 KiB by
  default, 1 MiB at most, `truncated` past it. It waits 5 s at most for the program that copied,
  then answers what came with `timed_out`; it counts among the waiting calls.
- **`clipboard set`** makes Yuna hold the seat's selection with an agent's text, 256 KiB at most so
  that any text but control characters fits a request's line once escaped as JSON. Yuna writes it
  to each reader's pipe without blocking, 8 writes a seat at most, each ended after 5 s without
  progress, as ADR-0091's transfers. The text ends when a program copies another, or with
  `clear`, or with its seat.
- **`clipboard clear`** empties the seat's selection whoever holds it; one an X11 program owns is
  emptied on Wayland's side only.

## Alternatives considered

- **A program that copies started by the agent** (`wl-copy`): no code in Yuna, but a program to
  ship and find, and the agent's text on a command line.
- **`set` up to 1 MiB**: unreachable through a line of 1 MiB once the text is escaped.
- **`get` waiting as long as `windows`, 50 s**: a program that never writes would hold a waiting
  call far past what a paste takes.

## Consequences

- `clipboard` is part of the MCP contract ([ADR-0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)).
- A refusal by the program that copied cannot be told from an empty copy, and `get` lists 64 types
  at most ([debt](../debt.md)).
- Yuna's writes of an agent's text live on today's shared `State`, beside the selections Smithay
  keeps there; Stage 2's core gives them a task of their own.

## Validation and reconsideration

Evidence: stories of a Wayland and an X11 copy read exactly, the agent's text pasted by a Wayland
and an X11 program from both selections, a program that never writes, a reader that never reads
and one that reads slowly, 3 MiB read to its bound, binary data in base64, a seat closed while a
read waits and while writes run. Proposed again if an agent needs data larger than a line holds.
