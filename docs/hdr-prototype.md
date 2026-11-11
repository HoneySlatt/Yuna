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
what it found, then Smithay puts back the whole display state it found, and logind gives the
console back. A test on the branch already checked, on the GPU, that every patch reaches a 10-bit
buffer with its exact codes; only the monitor's side is left to see.

It stops by itself after 5 minutes. While it runs, the other monitors are dark and the keyboard
only drives the prototype.

## Before

1. Close games and anything fullscreen. Umbriel keeps running in the background.
2. In a terminal of the graphical session, in a checkout of the branch `m1.3-hdr-prototype`:

   ```sh
   nix develop -c cargo build --release -p yuna-hdr-prototype
   ```

   Cargo fetches Yuna's Smithay fork with your SSH key.
3. Have something to note on, and the monitor's joystick within reach: its OSD shows the signal it
   receives.

## Run

1. Press Ctrl+Alt+F3 and log in.
2. Go to the checkout, then:

   ```sh
   nix develop -c timeout --kill-after=10 360 target/release/yuna-hdr-prototype
   ```

   The program refuses to start from a graphical session. `timeout` kills it at 6 minutes if it
   hangs; it writes its log to `hdr-prototype.log` in the current folder.
3. The screen goes black, then shows pattern 1; the monitor should switch to HDR. The number of dim
   squares in the top-left corner is the pattern's number.
4. Keys: Right, Space or Page Down for the next pattern; Left or Page Up for the previous; 1 to 5
   for that pattern; Escape or Q to give everything back and quit; Ctrl+Alt+F1 to F10 to do the
   same, then go to that console.
5. Look at each pattern (below), and at the OSD's signal information.
6. Press Escape. The console's text comes back; its last lines are the log's. Then Ctrl+Alt and
   the F key of Umbriel's console.

## What to look at

Codes are 10-bit PQ; luminances in cd/m².

| # | Pattern | Expected | If not |
|---|---|---|---|
| 1 | 11 grey bars: 0, 0.1, 1, 5, 10, 50, 100, 203, 400, 600, 1,000 | The monitor's HDR indicator on; each bar brighter than the one on its left; the first as black as the background | Bars like an SDR ramp, the brightest ones equally white: the monitor does not read PQ |
| 2 | 8 small squares: 200, 400, 600, 800, 1,000, 1,500, 2,000, 4,000 | Brighter up to the panel's peak, then equal or nearly | Note the last square brighter than its left neighbour, and the monitor's HDR mode |
| 3 | 8 small squares near black: 0, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5 | In a dark room, the first invisible, then each a little brighter | Note the first square seen |
| 4 | Top: BT.2020's red, green, blue. Bottom: BT.709's, at the same level | The top row more saturated than the bottom, green most | Rows alike: the monitor ignores `Colorspace`, or clips both |
| 5 | Grey ramp near 9 to 39 cd/m². Top: one step per 10-bit code. Bottom: the same at 8 bits | Top bands a quarter as wide as the bottom ones | Same widths: something drops to 8 bits (or dithering hides both) |

## If something goes wrong

- **Keys do nothing**: wait. The program gives everything back at 5 minutes; at 6, `timeout`
  kills it and logind gives the console back.
- **It was killed** (the log does not end with `done`): nothing set the HDR properties back. Umbriel's
  wlroots 0.20 knows all three properties and most likely sets them again when you return; if its
  colours look washed out or too bright, log out of Umbriel and back in, or restart.
- **Nothing answers after 6 minutes**: press the computer's power button; logind shuts down
  cleanly. Magic SysRq allows only sync on this machine.

## What to bring back

- `hdr-prototype.log`.
- For each pattern: what it looked like, against the table.
- What the OSD said about the signal: HDR or not, and colour depth or format if it shows them.
- Any message on the console after the program stopped.
