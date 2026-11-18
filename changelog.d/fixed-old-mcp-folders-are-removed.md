- A `yuna mcp` that starts removes the folders of sockets and logs that one killed by SIGKILL
  left in `XDG_RUNTIME_DIR`: each live Yuna holds a lock on a file in its folder, which the
  kernel releases at any end, and a folder whose lock is free is removed, never by its pid.
