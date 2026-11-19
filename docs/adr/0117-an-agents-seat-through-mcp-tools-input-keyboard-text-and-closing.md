# 0117. An agent's seat through MCP: its tools, input, keyboard, text and closing

- Status: Accepted; supersedes
  [0043](0043-text-input-and-the-rest-of-slimeengines-examples.md) (text input on an agent's
  seat) and
  [0095](0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)
- Date: 2026-11-18

## Context

[ADR-0037](0037-seats-tools-input-and-closing.md) gave an agent's seat its tools, its input, a
fixed `us` keyboard and its closing.
[ADR-0043](0043-text-input-and-the-rest-of-slimeengines-examples.md) made Yuna the input method of
agents' seats. [ADR-0095](0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)
restated ADR-0037 with keyboard layouts changed at run time, but left ADR-0043's input method,
preedit and editor rules in ADR-0043, and its text still said "ADR-0037 stays as decided". A
reader needed both. This ADR gathers the rule as it stands on 2026-11-18; it decides nothing new.

The seat itself is [ADR-0116](0116-agents-seats-each-agent-in-its-own-yuna.md)'s. The tools added
by M1.11, outputs changed, programs' errors and logs, events, programs stuck or slow, the
accessibility tree, the clipboard, are their own ADRs', from
[ADR-0090](0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md) to
[ADR-0101](0101-an-agents-clipboard-through-mcp.md).

## Decision

- **The tools** (ADR-0095, 2026-11-18, as ADR-0037 set them on 2026-11-13). `seat` creates a
  seat, lists them, and closes one; creating gives its handle, a number never reused, its output
  and its socket. Every other tool but `clock` requires a seat; an id of another seat's output,
  window or program is refused as an unknown one. A manual-clock Yuna holds one seat at a time.
  `yuna mcp` opens no socket for the owner's seat and starts without an output.
- **Input, one action a call** (ADR-0095). `pointer` moves, presses, releases, clicks and scrolls,
  in logical units of one of the seat's outputs from its top-left corner, its first by default, or
  in a capture's pixels; its result tells what is under the pointer and the cursor its client
  asked for. `keyboard` presses, releases and taps keys named as XKB names them, or by a keysym
  they type, a tap with modifiers ("ctrl+shift+t"), and types text. A press of a button focuses
  the window under it. A drag is several calls, so that the client answers between them.
- **The keyboard** (ADR-0095). An agent's seat starts with `us` (evdev rules, pc105) on every
  machine. `keyboard layout` sets its layout, variant and options at any time, refused for a name
  XKB's rules do not know and while the seat holds a key; every program of the seat gets the new
  keymap, and `keyboard get` tells it.
- **Yuna is the input method of every agent's seat** (ADR-0043, 2026-11-13): each is made with
  Smithay's compositor input method on, so that a client's text field is entered and may be
  enabled. The owner's seat has none: a real input method comes with the desktop.
- **How `type` types** (ADR-0095). A character the layout reaches is typed by its keys: Shift,
  AltGr (Right Alt where the layout puts level 3 on it) and the layout's own switch between its
  groups, switched back at the end; only keys numbered 255 or less, a PC keyboard's first. A
  character it does not reach goes to the focused text field as `commit_string` when that field
  has enabled `text-input-v3`, through `inject_text_keysyms` otherwise. `tap` and `press` of a
  keysym press a key that types it alone first.
- **`keyboard preedit`** shows text being composed in the focused text field, its cursor at its
  end; empty text clears it. It is refused when no text field is enabled. The next `type` replaces
  it, as an input method's commit does (ADR-0043).
- **`keyboard` tells whether a text field is enabled** where its keyboard is, in each result
  (ADR-0043).
- **Closing a seat** (ADR-0095). In order: the keys and buttons it holds are released, and its
  focus leaves; its socket is removed and its Wayland clients are disconnected, whoever started
  them; its processes get SIGTERM and SIGCONT, then SIGKILL 2 s later; `close` answers once they
  are gone, 3 s at most, with what was ended. A process belongs to a seat by its lineage, its
  process group, its Wayland connection, or the `YUNA_SEAT` variable each program gets. A seat
  lives until it is closed or Yuna stops: no lease.

## Alternatives considered

- **A first seat made at start, `seat` optional**
  ([ADR-0037](0037-seats-tools-input-and-closing.md)): a subagent that forgets the handle acts on
  its parent's seat without a word.
- **Handles no one can guess** ([ADR-0037](0037-seats-tools-input-and-closing.md)): all agents of
  one Yuna are one MCP client with the same rights; it protects nothing.
- **A list of steps a call** ([ADR-0037](0037-seats-tools-input-and-closing.md)): the client cannot
  answer between steps, so a drag, or a menu opened on a press, fails.
- **Coordinates shared by all outputs** ([ADR-0037](0037-seats-tools-input-and-closing.md)): an
  agent would add each output's place, which a capture does not show.
- **A cgroup per seat, or a seat closed after a while without a call**
  ([ADR-0037](0037-seats-tools-input-and-closing.md)): systemd's delegation, which CI lacks, or an
  agent that thinks long losing its programs.
- **A layout chosen at the seat's creation only**
  ([ADR-0095](0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)):
  no test of a program that sees its layout change.
- **A keymap swapped for any character outside the layout, or every key of the keymap usable**
  ([ADR-0095](0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)):
  the layout's own keys never tested, or keys past 255 that never reach X11 programs.
- **Everything `type` types sent as `commit_string`**
  ([ADR-0043](0043-text-input-and-the-rest-of-slimeengines-examples.md)): no key would reach the
  application, so its shortcuts would go untested.
- **`text-input-v3` offered with no input method**
  ([ADR-0043](0043-text-input-and-the-rest-of-slimeengines-examples.md)): it would do nothing
  until the desktop.

## Consequences

- Every call names its seat: an agent's first call is `seat create`.
- Every client of an agent's seat that binds `text-input-v3` gets `enter` when its surface has the
  keyboard, as under any input method.
- A client that handles a keymap change badly types wrong characters; Wayland asks clients to take
  a new keymap at any time.
- A change of layout costs the state thread 2.6 to 3.2 ms in release, 5.1 ms at the limit of four
  layouts and fourteen options, 11 ms in debug there ([measurements](../measurements.md)).
- An X11 program does not get a group switched through a key past 255 ([debt](../debt.md)).
- A process that empties its environment and leaves its group and its parents escapes its seat's
  closing, though not Yuna's stopping ([debt](../debt.md)). An abandoned seat keeps its programs
  until Yuna stops.

## Validation and reconsideration

Evidence: stories on `fr`, `de` and `us,ru`, each character checked as the client reads it, Caps
Lock and Shift held, an X11 client given the new keymap, the refusals, each failing under a
mutation of its code; stories of `text-input-v3`, `preedit` and closing. Proposed again when the
desktop gives the owner's seat layouts of its own, at Stage 2.
