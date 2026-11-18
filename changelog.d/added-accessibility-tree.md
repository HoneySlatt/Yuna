- `yuna mcp`'s `accessibility` tool reads the accessibility trees of a seat's programs started
  with `spawn start` and `accessibility: true`, and finds a node by its name, role and states: its
  window, process, description and rectangle on its output, in `pointer`'s units, so that an agent
  clicks a button it found by name. Each agent's seat has an accessibility bus of its own,
  `dbus-daemon`, started with its first such program and ended with the seat, on which Yuna is
  the AT-SPI registry: the one `YUNA_DBUS_DAEMON` names, nixpkgs' own in Yuna's package, else
  the first on `PATH`. Tested with GTK 4, and Qt on Wayland and on X11; Chromium and Electron
  also need `--force-renderer-accessibility` for a page's content.
