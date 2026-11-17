- A panic of Yuna's state stops it at once, with status 101, rather than unwind through a state
  left half changed; panics on its other threads are handled as before. `yuna mcp` first ends
  every process under it, SIGTERM then SIGKILL 2 s later, 3 s at most, and removes its folder in
  `XDG_RUNTIME_DIR`, which it used to leave behind (ADR-0080).
  Its X displays' files in `/tmp` go too; `yuna --headless` leaves its Wayland socket and its
  lock, which the next start takes as stale.
