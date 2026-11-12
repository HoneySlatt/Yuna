# Running the HDR prototype (M1.3)

What the owner does, by hand, to give [M1.3](PLAN.md#m13--hdr-through-smithay-a-prototype) its
verdict. Agents never run it: it takes the screen and the keyboard. This page is deleted with the
prototype once its findings are in the PLAN.

## What it does

The program lives on the branch `m1.3-hdr-prototype`, in `crates/hdr-prototype`. Run from a text
console, it asks logind for the monitor and the keyboards, sets the connector to HDR10
(`max bpc` 10, `Colorspace` `BT2020_RGB`, `HDR_OUTPUT_METADATA` with the PQ curve) through Yuna's
Smithay patch ([`smithay-patches.md`](smithay-patches.md)), and shows five patterns of known PQ
codes, drawn with OpenGL into 10-bit buffers. When it stops, it sets the three properties back to
what it found and shows a black frame with them; then it closes its logind session, and logind
gives the console back.

A test on the branch already checked, on the GPU, that the drawing puts every patch's exact codes
into an `Xbgr2101010` buffer. Left to see: the scanout format the display engine actually takes
(the log names it), the link's depth, and the monitor.

It stops by itself after 5 minutes. While it runs, the other monitors are dark and the keyboard
only drives the prototype.

## Before

1. Save your work and close games. Umbriel keeps running in the background.
2. Switch off Umbriel's night light and any gamma or colour tool. The program reads the display's
   colour transforms and refuses to run while one is set.
3. Run `systemd-inhibit --list`. If a line holds `handle-power-key`, a short press of the power
   button will do nothing; see *If something goes wrong*.
4. Run `cat /sys/class/drm/card1-DP-3/status`: it should say `connected`. With several monitors
   connected, Umbriel's list of outputs names the PG32UCDM's connector; if it is not `DP-3`, use
   its name after `--connector` below.
5. In a terminal of the graphical session, in a checkout of the branch `m1.3-hdr-prototype`:

   ```sh
   nix develop -c cargo build --release --locked -p yuna-hdr-prototype
   ```

   Cargo fetches Yuna's Smithay fork with your SSH key.
6. Have something to note on, and the monitor's joystick within reach: its OSD shows the signal it
   receives.

## Run

1. Press Ctrl+Alt+F3 and log in.
2. Go to the checkout, then:

   ```sh
   LIBSEAT_BACKEND=logind nix develop -c timeout --kill-after=10 360 \
     target/release/yuna-hdr-prototype --connector DP-3
   ```

   The program refuses to start from a graphical session. `timeout` kills it at 6 minutes if it
   hangs; it writes its log to `hdr-prototype.log` in the current folder.
3. The screen goes black, then shows pattern 1; the monitor should switch to HDR. The number of dim
   squares in the top-left corner is the pattern's number.
4. Keys: Right, Space or Page Down for the next pattern; Left or Page Up for the previous; 1 to 5
   for that pattern; C to switch `Colorspace` between `BT2020_RGB` and `Default` (the screen
   blanks a moment); Escape to give everything back and quit; Ctrl+Alt+F1 to F10 to do the same,
   then go to that console.
5. Look at each pattern (below), and at the OSD's signal information.
6. Press Escape. The console's text comes back. Then Ctrl+Alt and the F key of Umbriel's console.

## What to look at

Codes are 10-bit PQ; luminances in cd/m².

| # | Pattern | Expected | If not |
|---|---|---|---|
| 1 | 11 grey bars: 0, 0.1, 1, 5, 10, 50, 100, 203, 400, 600, 1,000 | The monitor's HDR indicator on; each bar brighter than the one on its left; the first as black as the background | A ramp up to a mid grey, nothing bright, HDR indicator off: the monitor does not read PQ. The top bars equal: the panel's peak or its mode's cap; note the mode |
| 2 | 8 small squares: 200, 400, 600, 800, 1,000, 1,500, 2,000, 4,000 | Brighter up to the panel's peak, then equal or nearly | Note the last square brighter than its left neighbour, and the monitor's HDR mode |
| 3 | 8 small squares near black: 0, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5 | In a dark room, the first invisible, then each a little brighter | Note the first square seen |
| 4 | Top: BT.2020's red, green, blue. Bottom: BT.709's, each driven like the one above it | The top row more saturated, green most. This shows the panel's gamut, whatever `Colorspace` says. Then press C twice: the colours change while `Colorspace` is `Default`; note whether the HDR indicator stays on | Nothing changes with C: the monitor ignores `Colorspace` |
| 5 | Grey ramp near 9 to 39 cd/m². Top: one step per 10-bit code. Bottom: the same at 8 bits | Top smooth or in faint narrow bands, bottom in 32 bands. This proves the drawing's 10 bits, not the link's: amdgpu is expected to dither an 8-bit link, which looks the same | Same bands top and bottom: something drops to 8 bits without dithering. The OSD's colour depth, if it shows one, is the evidence for the link |

## If something goes wrong

- **It refuses to start**: the console shows why. A colour transform set: switch off the night
  light in Umbriel. If it still refuses with nothing switched on, Umbriel leaves a transform
  anyway: bring back the log, the prototype then needs another way. A connector not found: check
  step 4 of *Before*.
- **Keys do nothing**: wait. The program gives everything back at 5 minutes; at 6, `timeout`
  kills it and logind gives the console back.
- **It was killed** (the log does not end with `done`): nothing set the HDR properties back. Umbriel's
  wlroots 0.20 knows all three properties and most likely sets them again when you return; if its
  colours look washed out or too bright, log out of Umbriel and back in, or restart.
- **Nothing answers after 6 minutes**: `timeout` has killed the program, or it is stuck in the
  kernel. First press Ctrl+Alt and Umbriel's F key: if the program is gone, the keyboard works
  again. Then, from another machine, `ssh` in and run `sudo systemctl reboot`. Then press the
  computer's power button; if that does nothing, hold it 5 seconds, which cuts the power. Magic
  SysRq allows only sync on this machine.

## What to bring back

- `hdr-prototype.log`.
- For each pattern: what it looked like, against the table; for pattern 4, whether C changed it
  and whether the HDR indicator stayed on.
- What the OSD said about the signal: HDR or not, and colour depth or format if it shows them.
- Any message on the console after the program stopped.
