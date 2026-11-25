- `yuna mcp`'s `events` tells a window that opens and closes before Yuna builds the events again,
  within one round or between two calls: `window_opened`, with what the window was, then
  `window_closed`, where it told nothing. Yuna marks what changes where it changes, and builds
  the events of what changed alone, rather than describe every window to find it (ADR-0153).
