- `yuna mcp` encodes at most as many captures at once as the machine has cores less one, four at
  most and one at least, so that a machine of two cores keeps one for Yuna's own work; a capture
  asked while they all run is refused as before (ADR-0152).
