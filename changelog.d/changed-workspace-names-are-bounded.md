- `yuna mcp`'s `workspaces` refuses a workspace's name past 256 bytes, in `create`, `remove`,
  `focus` and `move_window`, and its schema gives the bound: `windows` repeats the name for each
  window, and a name bounded by a request's line alone made one reply gigabytes.
