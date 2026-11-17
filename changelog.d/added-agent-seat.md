- `yuna --headless --agent-seat` serves an agent's seat on its socket rather than the owner's: a
  `us` keyboard, a pointer, and the virtual keyboard and pointer, `zwp_virtual_keyboard_v1` and
  `zwlr_virtual_pointer_v1`, which drive its programs as in a headless sway. They are offered on
  agents' seats alone, in `yuna mcp` too, never on the owner's.
