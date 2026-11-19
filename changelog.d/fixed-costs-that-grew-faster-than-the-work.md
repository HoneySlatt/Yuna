- One commit of a window reads its tree of subsurfaces once, rather than once for each of its
  surfaces: a window that commits 4,000 subsurfaces side by side holds Yuna about a fifteenth as
  long.
- Windows refused by the caps on windows no longer lengthen every arrangement, and a new window's
  first configure no longer copies its seat's layout: on a seat of 4,000 windows, it takes
  microseconds rather than milliseconds.
- A client's capture that cannot be read logs a warning once, then at debug level until a capture
  of the same output is read again, rather than a warning at each refresh.
