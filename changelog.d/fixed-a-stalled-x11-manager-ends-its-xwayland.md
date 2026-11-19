- An X11 window manager that stops taking Yuna's commands, its Xwayland stopped or hung, no longer
  makes Yuna's memory grow: what Yuna owes it is bounded, and once it has taken none for 10 s,
  and still none 1 s later, its Xwayland ends, as when the manager fails, and the seat's next X11
  program starts a new one. Its watchdog is disarmed as soon as the manager takes what it owes:
  nothing is left armed at rest, and a manager that takes them within a second of Yuna running
  again, after Yuna itself was stopped, is not taken for stalled.
