- `yuna mcp`'s `events` numbers each seat's events on their own: an event's `seq` grows with its
  seat's events, from 1, and is never reused while the seat lives, where it grew across the whole
  Yuna. An agent that reads several seats keeps one cursor for each: one seat's cursor means
  nothing on another, and an `after` past the seat's newest event is refused. `missed`, how many
  events a seat's log dropped after a cursor, is now exact (ADR-0153).
