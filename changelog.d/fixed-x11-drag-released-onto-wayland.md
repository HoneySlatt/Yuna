- An X11 program's drag moved onto a Wayland window and released in the same instant, as MCP's
  `release` given a place does, drops into an X11 window hidden or covered under that point far
  less often: Yuna keeps the window it lays over X11 for the drag after the release, until the
  source drops or leaves, the pointer moves again, or five seconds run out. A program that looks
  for its target before Yuna has laid that window still finds the X11 window under it (Known
  limitations; ADR-0102).
