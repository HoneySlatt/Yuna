- The clipboard and the primary selection pass between a seat's X11 programs and its Wayland
  programs, both ways, through Yuna's X11 window manager (ADR-0066): text as `UTF8_STRING`,
  `STRING` and `TEXT`, any MIME type as it is named, large ones through INCR. An X11 program
  reads the Wayland programs' selections only while one of the seat's X11 windows has the
  keyboard; a copy made before the seat's Xwayland started is pasted in X11 too. A transfer that
  stalls 5 s ends. Drag and drop between them comes later.
