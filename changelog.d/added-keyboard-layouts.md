- `keyboard layout` changes an agent's seat's keyboard layout while Yuna runs: XKB's layouts,
  their variants and its options, on the evdev rules and a pc105 keyboard, `us` at first. Every
  program of the seat gets the new keymap, X11 programs included, and `type`, `tap` and `press`
  go through it: `type` types each character the layout has with its own keys, AltGr held for
  level 3, and locks another of the layout's groups with the layout's own switch, such as
  `grp:alt_shift_toggle`'s Alt and Shift, then the group it started in again. A layout, variant
  or option XKB's rules do not know is refused, and so is a change while a key is held, with
  nothing changed. `keyboard get` tells the seat's layout, variant and options.
