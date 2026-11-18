- `yuna mcp`'s `clipboard` reads, sets and clears a seat's clipboard or primary selection, as its
  X11 and Wayland programs copy and paste them (ADR-0033, ADR-0091). `get` gives the MIME types
  offered and the data of one, by default the first text type, as text when it is UTF-8, else in
  base64; 64 KiB by default and 1 MiB at most, `truncated` past them; it waits 5 s at most for
  the program that copied, then answers with what came and `timed_out`. `set` makes Yuna hold
  up to 256 KiB of text, offered as `text/plain;charset=utf-8`, `text/plain` and `UTF8_STRING`,
  until a program copies another or `clear`, which empties the selection whoever copied it.
  Yuna writes it to each program that pastes, at its pace, and stops after 5 s without
  progress.
