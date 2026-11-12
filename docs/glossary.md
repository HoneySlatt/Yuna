# Glossary

Terms Yuna uses in a sense of its own, or that a newcomer may not know.

- **Activation token**: a string a client asks Yuna for and hands on, a launcher to the program
  it starts, so that a window can be activated with it (`xdg-activation-v1`); valid only when
  asked from the surface that has the keyboard focus, a window or its open menu, and for 10 s.
- **Agent's output**: a virtual output given to one agent, never shown unless the owner asks, drawn
  at full rate ([ADR-0015](adr/0015-agents-workspaces.md)).
- **Asked size**: the size a window's client is told in a configure, in whole logical units; 0 on
  an axis lets the client choose it ([ADR-0026](adr/0026-real-clients-in-the-layout.md)).
- **Column**: in scrolling columns, windows stacked from top to bottom that are asked one width;
  the column is as wide as the widest of them as drawn.
- **Committed size**: the window geometry a client last committed; the layout draws the window at
  it, and places scrolling columns by it ([ADR-0026](adr/0026-real-clients-in-the-layout.md)).
- **Data control**: `ext-data-control-v1`, which lets a client, such as a clipboard manager, read
  and set the selections without the keyboard focus.
- **Development shell**: the environment `nix develop` opens from `flake.nix`, with the pinned
  toolchain and every tool the checks use; CI runs in it too.
- **Dialog**: a window that names a parent, as xdg-shell's `set_parent`; it opens floating, centred
  on its parent, and takes the focus only from it.
- **Dismissed popup**: a popup Yuna told `popup_done`, as xdg-shell lets a compositor do: it
  leaves its window, is no longer drawn, takes no keyboard grab, and waits for its client to
  destroy it.
- **Dmabuf**: a buffer a client shares with Yuna by file descriptor, through `linux-dmabuf`,
  most often in GPU memory; Yuna offers the protocol only where its renderer imports dmabufs.
- **DRM usage statistics**: what the kernel counts for each client of a GPU, read in the
  `fdinfo` of its render node: each engine's busy time in nanoseconds (`drm-engine-gfx`), the
  measure of a frame's GPU time ([ADR-0030](adr/0030-a-frames-gpu-time-from-the-kernel.md)).
- **Dwindle**: a layout where each new window splits the space of the focused one in two.
- **Dynamic workspaces**: an option of the layout: each output keeps one empty unnamed workspace
  last, as niri does ([ADR-0018](adr/0018-workspaces.md)).
- **Fake fullscreen**: an application told it is fullscreen while its column keeps its size.
- **Fractional scale**: an output scale that is not a whole number, such as 1.5. A client that
  follows `wp_fractional_scale_v1` draws its buffer at its output's physical size and sets its
  viewport (`wp_viewporter`) to its logical size: one buffer pixel per physical pixel.
- **Full width**: a column that spans the working area, gaps aside; in dwindle, a window
  maximised over the working area. Umbriel calls it "maximize".
- **Game mode**: the state Yuna enters when a game is fullscreen: effects and animations off,
  notifications silent, VRR in its band.
- **The games' band**: the high, narrow range of refresh rates Yuna keeps a game's VRR in, by
  showing frames several times ([ADR-0014](adr/0014-vrr-without-flicker.md)).
- **HDR10**: high dynamic range output as monitors take it: the PQ curve, BT.2020 primaries, at
  least 10 bits per channel, and static metadata sent with the picture (`HDR_OUTPUT_METADATA`).
- **Headless mode**: Yuna running without a physical screen, on virtual outputs.
- **Hidden window**: a window on a workspace not shown, or in a column scrolled out of view; it gets
  at most one frame callback a second ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)).
- **Home output**: the output a workspace goes back to when that output returns after being
  unplugged ([ADR-0018](adr/0018-workspaces.md)).
- **lavapipe**: Mesa's Vulkan driver that draws on the CPU; the applications' tests give it to
  every Vulkan client.
- **llvmpipe**: Mesa's OpenGL driver that draws on the CPU; the headless mode's default
  ([ADR-0022](adr/0022-the-headless-backend-in-yuna.md)).
- **Logical coordinates**: the layout's units, shared by all outputs: an output of 3840 × 2160
  pixels at scale 1.5 measures 2560 × 1440 logical units. Rectangles are snapped to the
  **physical grid**, the output's own pixels.
- **Manual clock**: the compositor's clock in tests: it starts at the real monotonic time and moves
  only when a test advances it, running each deadline it passes, in order
  ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)).
- **Pinned window**: a floating window shown over whichever workspace its output shows.
- **Popup grab**: a popup's request for the keyboard until it is dismissed, as an open menu
  holds it; granted only with a serial Yuna's seat sent, and only to a popup not dismissed. While
  it holds, the keyboard stays within its window.
- **PQ**: SMPTE ST 2084, the perceptual quantizer: the curve that turns an absolute luminance, up
  to 10,000 cd/m², into an HDR signal. A **PQ code** is that signal as a 10-bit integer, 0 to
  1,023.
- **Primary selection**: the text last selected, pasted with the middle button; a second selection
  beside the clipboard, from X11.
- **Refresh boundary**: the moment an output could start showing a new frame; a virtual output's
  come every `1 / rate` seconds of Yuna's clock ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)).
- **Seat**: a set of input devices, a pointer and a keyboard, with its own focus. Each agent has
  one, apart from the owner's.
- **Selection**: what a client offers to paste, as a list of MIME types: the clipboard or the
  primary selection. Each seat has its own.
- **Sentinel**: in the compositor's tests of misbehaving clients and random requests, a
  well-behaved client that joined first; its frame callbacks, at the exact boundary, show that the
  others did not stop its frames.
- **Special workspace**: a hidden workspace shown over the current one by a key.
- **Strip**: in scrolling columns, the row of columns of a workspace; the **view** is the part of
  it the output shows, and scrolls to keep the focused column whole.
- **Suspended**: an `xdg_toplevel` state telling a client its window cannot be seen; Yuna sends it
  to hidden windows, to clients whose `xdg_wm_base` version has it (6 and later).
- **Urgent**: said of a window that asked for the focus and did not get it, because its request
  came without a valid activation token or a fullscreen window held the screen; it stays urgent
  until it gets the focus or closes. Nothing shows it yet.
- **Virtual output**: an output no monitor shows, drawn offscreen at its own size, rate and scale
  ([ADR-0022](adr/0022-the-headless-backend-in-yuna.md)).
- **VRR**: variable refresh rate; the monitor refreshes when a frame is ready rather than at a fixed
  rate.
- **Wake-up**: a round of the compositor's event loop in which something had work for it: a
  client, a new connection or the clock's timer. At rest there are none.
- **Working area**: the part of an output tiled windows go in; the whole output until panels
  reserve zones of it.
