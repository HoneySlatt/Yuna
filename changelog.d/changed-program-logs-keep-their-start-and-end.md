- A program's log in `yuna mcp` keeps the first 256 KiB of its output and its newest 1 MiB,
  rather than its first 1 MiB: both a failure to start and the end show. `log_bytes` and
  `log_dropped` count what it keeps and what it dropped between.
