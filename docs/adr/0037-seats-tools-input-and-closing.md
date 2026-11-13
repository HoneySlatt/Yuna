# 0037. An agent's seat through MCP: its tools, its input, its keyboard, its closing

- Status: Accepted; amended by [0043](0043-text-input-and-the-rest-of-slimeengines-examples.md)
  (how `type` types a character outside `us`)
- Date: 2026-11-13

## Context

[ADR-0035](0035-agents-seats.md) gives each agent a seat of its own in `yuna mcp`: a handle, a
Wayland socket, an output, its programs ended when it closes. It leaves open which tools take the
handle, what pointer and keyboard input look like, which keyboard layout a seat has, and how a
seat closes. The design's challenge asked that ids be checked against the seat that owns them,
that a manual-clock Yuna have one seat, and that input held be released
([PLAN](../PLAN.md#m15--agents-control)).

Facts it rests on, read in Smithay `8fd9fe0` and wayland-server 0.31.14 on 2026-11-13:

- `SeatState::new_seat` makes a seat with no `wl_seat` global; `ListeningSocket::bind_absolute`
  listens at any path.
- A client must ask `start_drag` after the button press that starts a drag; a menu opened on a
  press asks its grab with the press's serial. Both need the client to answer between two
  events.
- `KeyboardHandle::inject_text_keysyms` types any keysym: it gives the focused client a
  keymap where each keysym has a key, taps them, then gives the seat's keymap back. KWin types
  text this way.
- [ADR-0034](0034-captures.md) makes a capture's pixel one logical unit of its output.
- Every process an agent starts is under Yuna, its subreaper; a process that double-forks and
  calls `setsid` leaves its parents and its process group. A cgroup per seat needs systemd's
  delegation, which CI and a session without systemd lack.

## Decision

The owner chose, on 2026-11-13:

- **The tools.** `seat` creates a seat, lists them, and closes one. Creating gives its handle,
  a number never reused, its output and its socket. `outputs`, `spawn`, `windows`, `workspaces`,
  `pointer` and `keyboard` require a seat; `clock` alone acts on the whole Yuna. An id of
  another seat's output, window or program is refused as an unknown one. A manual-clock Yuna
  holds one seat at a time. `yuna mcp` opens no socket for the owner's seat and starts without
  an output.
- **Input, one action a call.** `pointer` moves, presses, releases, clicks and scrolls, in
  logical units of one of the seat's outputs from its top-left corner, its first output by
  default; its result tells what is under the pointer and the cursor its client asked for.
  `keyboard` presses, releases and taps keys named as XKB names them, a tap with modifiers
  ("ctrl+shift+t"), and types text. A press of a button focuses the window under it. A drag is
  several calls, so that the client answers between them.
- **The keyboard.** An agent's seat has a fixed layout, `us` (evdev rules, pc105), on every
  machine. `type` types through it, Shift where a character needs it; a character it lacks is
  typed through Smithay's `inject_text_keysyms`.
- **Closing a seat.** In order: the keys and buttons it holds are released, and its focus
  leaves; its socket is removed and its Wayland clients are disconnected, whoever started them;
  its processes get SIGTERM, then SIGKILL 2 s later; `close` answers once they are gone, 3 s at
  most, with what was ended. A process belongs to a seat by its lineage, its process group, its
  Wayland connection, or the `YUNA_SEAT` variable each program gets, read in
  `/proc/<pid>/environ`. A seat lives until it is closed or Yuna stops: no lease.

## Alternatives considered

- **A first seat made at start, `seat` optional**: one call fewer for a lone agent, but a
  subagent that forgets the handle acts on its parent's seat without a word, against
  [ADR-0015](0015-agents-workspaces.md).
- **Handles no one can guess**: all agents of one Yuna are one MCP client with the same rights;
  it protects nothing.
- **A list of steps a call**: fewer calls, but the client cannot answer between steps, so a drag
  and drop, or a menu opened on a press, fails.
- **Coordinates shared by all outputs**: an agent would add each output's place, which a capture
  does not show.
- **Characters outside `us` refused until `text-input-v3`** (C5): no keymap change, but no accent
  before C5, and never in an application without `text-input-v3`.
- **A layout chosen at the seat's creation**: nothing needs it yet.
- **A cgroup per seat**: no process escapes, but it needs systemd's delegation.
- **A seat closed after 30 minutes without a call**: nothing left running by a subagent that
  forgets, but an agent that thinks long loses its seat and its programs.

## Consequences

- Every call names its seat: an agent's first call is `seat create`.
- A client that handles a keymap change badly types wrong characters outside `us`; Wayland asks
  clients to take a new keymap at any time.
- A process that empties its environment and leaves its group and its parents escapes its seat's
  closing, though not Yuna's stopping, which ends every process under it ([debt](../debt.md)).
- An abandoned seat keeps its programs until Yuna stops.
