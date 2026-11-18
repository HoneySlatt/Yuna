- `yuna mcp`'s `events` tool: an agent waits for what happens on its seat rather than asking in a
  loop, or reads what happened since a cursor it keeps. Windows opened, closed, retitled, placed
  and unresponsive, the seat's focus, programs that exited, Wayland connections that ended, with
  the protocol error Yuna sent and the program they belonged to, and outputs added, removed and
  changed; each seat keeps its last 1,024 events, and a reply tells how many a cursor too old
  missed (ADR-0033).
