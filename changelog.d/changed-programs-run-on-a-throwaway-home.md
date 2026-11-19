- A program `yuna mcp` starts through `spawn` no longer runs on the user's home: each seat's
  programs share a throwaway home in `TMPDIR`, or `/tmp`, which `HOME` names, with
  `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` and `XDG_STATE_HOME` inside it; it goes
  when the seat closes and when Yuna stops, and one a killed Yuna left goes when the next
  `yuna mcp` starts. Their folder is made again when a cleaner of the temporary folder removes
  it while Yuna runs, and under a random name when another user took its own. `XDG_CONFIG_DIRS`
  and `XDG_DATA_DIRS` are kept. Name any of these variables in `spawn start`'s `env` to give a
  program a home of your choosing (ADR-0105).
