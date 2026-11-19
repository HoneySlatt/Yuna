- An X11 program's drag moved onto a Wayland window and released in the same instant can still
  drop into an X11 window hidden or covered under that point, when the program looks for its
  target before Yuna's X11 window manager, on its own thread, has laid its window over X11 for
  the drag: about 1 time in 12 on a quiet machine with Yuna's test program, which looks within a
  millisecond; a toolkit that looks sooner may lose more often. Such a drop is refused, never
  given to the Wayland window. Moving onto the Wayland window, then releasing in a later call,
  avoids it (ADR-0102).
