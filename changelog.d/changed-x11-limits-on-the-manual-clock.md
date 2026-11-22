- On the manual clock (`yuna mcp --manual-clock`), the time limits of an X11 program's clipboard
  transfers and drags run on Yuna's clock rather than on real time: they pass only when the
  clock is advanced past them, and the `clock` tool's next deadline names the earliest. MCP's
  `rest` tool counts its wake as one more source, `clock_x11_manager` (ADR-0152).
