- After an X11 program's drag released onto a Wayland window, the window Yuna lays over X11 for
  the drag stays until the pointer comes over an X11 window, or the drag ends; a click on an X11
  window given with that motion, as MCP's `click` given a place does, can reach Yuna's window
  rather than the X11 window, and be lost, when it reaches Xwayland before Yuna's X11 window
  manager, on its own thread, has taken that window away (ADR-0102).
