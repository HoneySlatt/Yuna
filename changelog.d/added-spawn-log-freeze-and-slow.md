- `spawn log` reads a program's log through MCP, by byte offsets since it started or its newest
  bytes; `spawn list` gives each program's last protocol error and how much of the time it may
  run.
- `spawn freeze`, `thaw` and `slow` make a program stuck or slow: its process group stopped,
  continued, or run a share of each 100 ms, on real time. Every path that ends programs with
  SIGTERM sends SIGCONT after it, so that a stopped program ends at once rather than at SIGKILL.
