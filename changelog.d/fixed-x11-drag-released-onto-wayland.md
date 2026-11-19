- An X11 program's drag moved onto a Wayland window and released in the same instant, as MCP's
  `release` given a place does, no longer drops into an X11 window hidden or covered under that
  point: Yuna keeps the window it lays over X11 for the drag until the source drops or leaves,
  or its five seconds run out (ADR-0102).
