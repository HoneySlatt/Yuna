# 0095. An agent's seat through MCP: its tools, its input, its keyboard and layouts, its closing

- Status: Superseded by
  [0117](0117-an-agents-seat-through-mcp-tools-input-keyboard-text-and-closing.md); supersedes
  [0037](0037-seats-tools-input-and-closing.md); supersedes in part
  [0043](0043-text-input-and-the-rest-of-slimeengines-examples.md) (how `type` types a character)
- Date: 2026-11-18

## Context

[ADR-0037](0037-seats-tools-input-and-closing.md) gave an agent's seat its tools, its input, a
fixed `us` keyboard and its closing; it rejected a layout chosen at the seat's creation, as
nothing needed one. [ADR-0043](0043-text-input-and-the-rest-of-slimeengines-examples.md) typed a
character outside `us` through the text field or a keymap swapped for the call. M1.11 makes
keyboard layouts a situation an agent tests ([ADR-0061](0061-yunas-uses.md)), and the agent's
tools grew: outputs changed, programs' errors and logs, events, programs stuck or slow
([ADR-0090](0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md),
[ADR-0092](0092-a-programs-protocol-trace-its-errors-and-its-log-through-mcp.md),
[ADR-0093](0093-a-call-that-waits-for-the-seats-events.md)).

Facts: Smithay's keyboard takes a new keymap at run time and sends it to every `wl_keyboard` of
the seat, Xwayland's included. evdev keymaps hold keys numbered above 255, which X11 never gets,
among them the key that switches to the next layout and one that types the euro sign alone.

## Decision

ADR-0037 stays as decided on 2026-11-13, but for its keyboard, which the owner chose on
2026-11-18 to change at run time:

- **The tools.** `seat` creates a seat, lists them, and closes one; creating gives its handle, a
  number never reused, its output and its socket. Every other tool but `clock` requires a seat;
  an id of another seat's output, window or program is refused as an unknown one. A manual-clock
  Yuna holds one seat at a time. `yuna mcp` opens no socket for the owner's seat and starts
  without an output.
- **Input, one action a call.** `pointer` moves, presses, releases, clicks and scrolls, in logical
  units of one of the seat's outputs from its top-left corner, its first by default, or in a
  capture's pixels; its result tells what is under the pointer and the cursor its client asked
  for. `keyboard` presses, releases and taps keys named as XKB names them, or by a keysym they
  type, a tap with modifiers ("ctrl+shift+t"), and types text. A press of a button focuses the
  window under it. A drag is several calls, so that the client answers between them.
- **The keyboard.** An agent's seat starts with `us` (evdev rules, pc105) on every machine.
  `keyboard layout` sets its layout, variant and options at any time, refused for a name XKB's
  rules do not know and while the seat holds a key; every program of the seat gets the new keymap,
  and `keyboard get` tells it.
- **How `type` types.** A character the layout reaches is typed by its keys: Shift, AltGr (Right
  Alt where the layout puts level 3 on it) and the layout's own switch between its groups,
  switched back at the end; only keys numbered 255 or less, a PC keyboard's first. A character it
  does not reach goes to the focused text field as `commit_string` when that field has enabled
  `text-input-v3`, through `inject_text_keysyms` otherwise. `tap` and `press` of a keysym press a
  key that types it alone first.
- **Closing a seat.** In order: the keys and buttons it holds are released, and its focus leaves;
  its socket is removed and its Wayland clients are disconnected, whoever started them; its
  processes get SIGTERM and SIGCONT, then SIGKILL 2 s later; `close` answers once they are gone,
  3 s at most, with what was ended. A process belongs to a seat by its lineage, its process group,
  its Wayland connection, or the `YUNA_SEAT` variable each program gets. A
  seat lives until it is closed or Yuna stops: no lease.

## Alternatives considered

- **A layout chosen at the seat's creation only**: simpler, but no test of a program that sees
  its layout change.
- **`type` through the keymap swap for any character outside `us`**: no key tables to read, but
  the path a layout test wants to check, the layout's own keys, would never run.
- **Every key of the keymap usable**: fewer chords, but keys past 255 never reach X11 programs.
- **The rest of ADR-0037's alternatives** (a first seat made at start, handles no one can guess,
  a list of steps a call, coordinates shared by all outputs, a cgroup per seat, a lease) were
  rejected then for the reasons it gives, which hold.

## Consequences

- A change of layout costs the state thread 2.6 to 3.2 ms in release, 5.1 ms at the limit of four
  layouts and fourteen options, 11 ms in debug there ([measurements](../measurements.md)).
- An X11 program does not get a group switched through a key past 255, used when a layout has no
  switching option of its own ([debt](../debt.md)).
- A client that handles a keymap change badly types wrong characters; Wayland asks clients to take
  a new keymap at any time.

## Validation and reconsideration

Evidence: stories on `fr`, `de` and `us,ru`, each character checked as the client reads it, a
parenthesis tapped, Caps Lock and Shift held, a release in another group, an unfocused client and
an X11 client given the new keymap, the refusals; each failing under a mutation of its code.
Proposed again when the desktop gives the owner's seat layouts of its own, at Stage 2.
