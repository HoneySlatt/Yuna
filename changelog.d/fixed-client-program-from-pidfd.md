- `yuna mcp` names a client's program from the client's own process: a pidfd of the peer, taken
  when Yuna accepts the connection, tells whether the peer had exited, so a client whose process
  exited first, its pid perhaps taken by another, is named by no program rather than by another
  process's. The program is found once, at the accept, and kept: `client_left` and `rest` name it
  even once the program is forgotten. On a kernel before Linux 6.5, which gives no such pidfd,
  Yuna finds the program from `/proc` alone, as before, and logs a warning once (ADR-0156).
