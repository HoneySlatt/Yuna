- `type` reads the keyboard's state as its programs read it: with Caps Lock on, or Shift held, a
  letter still types as written. It types with a PC keyboard's keys first, `(` and `)` with Shift
  and 9 or 0 rather than the keypad's parentheses, and never with a key past 255, which X11
  programs do not get. A tap or a press of `parenleft` still presses a key that types `(` alone,
  and a release lets go of the key held under that name even after another group is locked.
