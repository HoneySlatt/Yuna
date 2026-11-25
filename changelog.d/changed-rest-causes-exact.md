- `rest` and `--rest-audit` count a window's first buffer, which opens it, as its client's commit
  rather than as a layout change, and a commit that brought nothing new, a frame callback alone,
  as no cause of the frame that answers it: a client that opens windows again and again is named
  for the frames it caused (ADR-0158).
