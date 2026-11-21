- `yuna mcp`'s `events` tells a seat's agent `client_refused` when the seat's socket closes
  connections past its bound of clients, with the bound and how many were refused since its last
  such event, at most once a second; refusals after the last told wait for a later one, and are
  not told if none comes (ADR-0147).
