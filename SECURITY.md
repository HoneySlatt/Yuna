# Security

## Reporting a vulnerability

Report it privately, through GitHub's
[private vulnerability reporting](https://github.com/HoneySlatt/Yuna/security/advisories/new),
never in a public issue. Give the version or commit, the steps or a test that shows the problem,
and what an attacker gains.

You get an answer within a week. A fix comes in a release of its own, with a `security-…`
changelog fragment ([`changelog.d/`](changelog.d/README.md)) and an advisory that credits you, if
you wish.

## Supported versions

Only the latest release, and `main`. Yuna is in 0.x: a fix goes into the next release, never
into an older one.

## What Yuna protects

Yuna runs as its user, with no privilege, and asks for none. Today it runs only headless, for
agents and tests; the desktop session comes at Stage 2 ([ROADMAP](docs/ROADMAP.md)).

These are vulnerabilities:

- **A client harms another client or Yuna.** A Wayland or X11 client that crashes or stalls Yuna,
  reads another client's data (a buffer, a selection, its input), or grows Yuna's memory without
  bound. A client's own protocol error ends that client alone
  ([ADR-0080](docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)); what one client
  may ask is bounded ([ADR-0028](docs/adr/0028-bounds-on-what-one-client-may-ask.md),
  [ADR-0070](docs/adr/0070-bounds-on-what-one-x11-client-may-ask.md)).
- **An agent's seat reaches the owner's seat**, or another agent's: input, captures, selections
  or programs that cross from one seat to another
  ([ADR-0015](docs/adr/0015-agents-workspaces.md)).
- **Yuna exposes the user's files or sessions**: it reads or writes outside its runtime folder,
  passes secrets from its environment to its programs, or leaves sockets others can open.

These are not, today:

- Another process of the same user: it can open Yuna's sockets and read its cookies, as it can
  read the user's files ([ADR-0067](docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)).
- An MCP client of `yuna mcp`: it is the agent that started that Yuna, and it may do anything its
  seats allow, starting programs included ([ADR-0033](docs/adr/0033-the-mcp-server.md)).
- X11 programs on one seat reading one another's windows and input: X11 isolates nothing within a
  display. Each seat has its own Xwayland.
