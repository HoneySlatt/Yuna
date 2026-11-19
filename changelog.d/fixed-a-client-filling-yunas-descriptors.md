- One client making Yuna hold descriptors until its table is full, `wl_shm` pools for instance,
  no longer stops `yuna --headless` at the next connection: the owner's socket pauses and takes
  the connection once descriptors are free, as an agent's seat's socket did already. Yuna also
  raises its soft limit of open files to the hard limit at start, as KWin and Mutter do, and
  gives the programs it starts, Xwayland among them, the limit it started with. A bound on the
  descriptors one client may make Yuna hold is still to come.
