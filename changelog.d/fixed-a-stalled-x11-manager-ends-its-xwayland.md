- An X11 window manager that stops taking Yuna's commands, its Xwayland stopped or hung, no longer
  makes Yuna's memory grow: what Yuna owes it is bounded, and once it has taken none for 10 s,
  its Xwayland ends, as when the manager fails, and the seat's next X11 program starts a new one.
