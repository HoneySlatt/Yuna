- A virtual keyboard holding more than about 1,018 keys no longer gets the clients of its seat
  ended, whose `wl_keyboard.enter` grew past a Wayland message: a seat holds 64 keys and 16
  pointer buttons at most. A virtual keyboard's or pointer's press past them, or of a key its
  keymap does not have, is dropped; MCP's `keyboard` and `pointer` presses past them are refused
  (ADR-0028).
