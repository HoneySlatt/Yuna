- An X11 window whose size hints ask a minimum larger than every output of its seat, up to
  X11's 32,767 pixels, is given no more than the seat's largest output on each axis, and opens,
  rather than being configured to a size nothing shows. Its minimum follows the seat's outputs as
  they are added, removed or given another mode; a seat without an output bounds it to nothing
  until it has one.
