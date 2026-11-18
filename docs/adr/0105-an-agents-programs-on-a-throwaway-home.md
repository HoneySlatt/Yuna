# 0105. An agent's programs on a throwaway home

- Status: Accepted
- Date: 2026-11-18

## Context

`yuna mcp` starts the programs an agent asks for (`spawn start`) with an environment of their own
([ADR-0095](0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)):
Yuna's variables that find programs, libraries, locale and drivers, the seat's sockets, a
runtime folder of their own. Stage 1's audit found, on 2026-11-18, that those kept variables
include `HOME` and the XDG base folders (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`,
`XDG_STATE_HOME`): a program an agent starts reads and writes the user's real settings, data
and caches, which neither the tool's description nor the documents said. A browser started so
may find the user's profile in use and hand its command to the user's own copy, on the user's
desktop. Yuna's own tests already give every application a throwaway home
(`tests/common/application_environment.rs`).

## Decision

The owner chose, on 2026-11-18:

- **Each agent's seat has a throwaway home**: a folder made when its first program starts, in
  which `HOME` points, with `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` and
  `XDG_STATE_HOME` inside it. The seat's programs share it, so that a program finds what another
  of the seat wrote, as on one user's desktop.
- **It is on disk, not in the runtime folder**: in `TMPDIR`, or `/tmp`, readable by the user
  alone, so that a program's caches never fill the user's runtime folder, a small memory-backed
  file system the user's session needs.
- **It is removed** when its seat closes and when Yuna stops, and, after a Yuna killed without
  its cleanup, by the next `yuna mcp`, as the seats' runtime folder is.
- **An agent that names one of these variables in `spawn`'s `env` gets its own value**: a test
  profile of its choosing, kept as it is.
- `XDG_CONFIG_DIRS` and `XDG_DATA_DIRS`, the system's folders where programs find their schemas,
  icons and desktop files, stay as they are.

## Alternatives considered

- **The behaviour kept and written down**: no change for agents, but each must remember to pass
  a home of its own, and one that forgets touches the user's data.
- **A home per program**: complete isolation, but programs of one seat that share settings, a
  launcher and what it starts, would not see one another's.
- **The home in the seat's runtime folder**: removed with it for free, but on memory a program's
  cache could fill.

## Consequences

- A change to `spawn`'s contract ([ADR-0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)),
  announced in the changelog: a program no longer sees the user's settings unless the agent
  passes them.
- A program that needs the user's own data, an account already signed in, must be given its
  home by the agent, deliberately.
- Disk used by a seat's programs is bounded by nothing but the disk; the folder goes with the
  seat.

## Validation and reconsideration

A test starts a program that prints its environment and writes a file in its home: `HOME` and the
four XDG variables point inside the seat's throwaway home, the file is there, and the folder is
gone after `seat close`; an `env` naming `HOME` wins. Proposed again if agents need a persistent
profile across seats.
