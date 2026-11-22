- On the manual clock (`yuna mcp --manual-clock`), the time limits of an X11 program's clipboard
  transfers and drags run on Yuna's clock rather than on real time: they pass only when the
  clock is advanced past them, and the earliest is among the deadlines the `clock` tool gives
  (`next_deadline_ns`). One takes effect shortly after the `clock` call that passed it returns,
  once the X11 manager's thread has run. MCP's `rest` tool counts its wake as one more source,
  `clock_x11_manager` (ADR-0152).
