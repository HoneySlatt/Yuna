# 0096. A rest auditor naming what wakes Yuna or makes it draw

- Status: Accepted
- Date: 2026-11-18

## Context

[ADR-0062](0062-the-compositors-comfort-and-look.md) gives M1.11 a rest auditor: at rest, Yuna
names each program, or part of itself, that wakes it or makes it draw, through a command, an MCP
tool and a log line. [ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
asks Stage 2's core to name every wake-up by its source. Until then Yuna counted its wakes, frames,
commits and configures in total, and one event source served every client: a wake of the display
said that some client had written, not which.

Yuna has no channel for commands from another program until Stage 2's `yuna-ipc`; wayland-rs
tells nothing of a client's messages, and the owner chose on 2026-11-18 not to fork it before
Stage 2 ([ADR-0092](0092-a-programs-protocol-trace-its-errors-and-its-log-through-mcp.md)).

## Decision

The owner chose on 2026-11-18 that the command is a start option until `yuna-ipc`; the rest was
chosen while building M1.11:

- **Every event source is inserted through one helper that names it** by a closed set of kinds
  (the clients' display, a seat's socket, the clock's deadline by what it is due for, signals,
  MCP's input and output, programs' output and pace, Xwayland's sources, waiting calls' timers,
  and so on), and counts each round it had something to do. Clippy's `disallowed-methods` refuses
  any other insertion.
- **Clients are named without a fork**: Yuna keeps a second descriptor of each client's socket in
  an epoll of its own, and after a wake of the display asks it, without waiting, which clients
  have something to read. Each counts with its pid, its program and its seat; a client that leaves
  folds into one count of clients gone.
- **Every frame is counted by its causes**: a client's commit (which client), a layout change, a
  motion, a capture, an output's first frame or change. Arming a refresh requires a cause.
- **`rest`**, an MCP tool, gives the counts since Yuna started, or, with `wait_ms`, those of a
  window of real time, its own timer left out; a seat's, or every seat's.
- **`--rest-audit`**, for `yuna --headless` and `yuna mcp`, logs one line for each second in which
  something woke Yuna, naming its sources; a quiet second arms nothing, so that at rest nothing
  runs and nothing is logged.

## Alternatives considered

- **A fork of wayland-rs with an observer of each client's messages**: clients named exactly,
  without a second descriptor each, but the fork the owner declined before Stage 2.
- **A log line always on**: no option to pass, but a line a second for every animating client, on
  the desktop too.
- **The tool only, and a debug line**: less code, ADR-0062 half met.
- **Counting each callback**: simpler, but calloop calls a channel back once per message, so it
  counts messages, not wakes.

## Consequences

- Naming one ready client costs under 1 µs at 10 and at 100 clients; each client costs one more
  descriptor ([measurements](../measurements.md)).
- X11 programs are named as their seat's Xwayland, not one by one; a window's first buffer counts
  as a layout change ([debt](../debt.md)).
- The counts live in a struct of their own owned by today's shared `State`, fed by three hooks;
  Stage 2's core names its wakes in its own task table
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 6).

## Validation and reconsideration

Evidence: stories of a second at rest naming nothing, an animating program named with its commits
as the frames' cause, a program's output, Xwayland's idle stop, a waiting call's timer with the
`rest` call's own left out, and `--rest-audit` logging while a client animates and nothing after;
clippy refusing a direct insertion. Proposed again at Stage 2's opening, with `yuna-ipc`, when the
command becomes one, and with the fork of wayland-rs weighed there.
