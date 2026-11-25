- `rest` and `--rest-audit` count a window's first buffer, which opens it, as its client's commit
  on its own seat's outputs rather than as a layout change, so that a client that opens windows
  again and again is named for the frames it caused; another seat's frames no longer name it. A
  commit that attached no buffer, a frame callback or a subsurface moved, is a cause of the frame
  that answers it only when nothing else caused what that frame drew (ADR-0158).
