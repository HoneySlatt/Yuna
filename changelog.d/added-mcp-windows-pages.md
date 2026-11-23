- `yuna mcp`'s `windows` gives a page when asked: with `after`, a window's id, or `limit`, it
  lists the windows whose id is above `after`, by id, `limit` at most (128, its default and its
  most), and `next`, the cursor of the next page, null on the last. A window opened meanwhile
  comes on a later page; one closed meanwhile is not listed. A call that gives neither lists every
  window, as before (ADR-0153).
