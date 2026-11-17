- `wlr-screencopy-unstable-v1`, version 3, beside `ext-image-copy-capture-v1`: tools that copy an
  output or a region of it through the older protocol, uwui's measures among them, work in
  Yuna. A client copies its own seat's outputs into `wl_shm` buffers; a copy with damage waits
  for something new without waking Yuna at rest.
