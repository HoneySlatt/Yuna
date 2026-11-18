- A program `yuna mcp` starts without `accessibility: true` now gets `NO_AT_BRIDGE=1` and
  `GTK_A11Y=none`, so that its toolkit never looks for an accessibility bus; give either in
  `spawn start`'s `env` to choose otherwise.
