# Glossary

Terms Yuna uses in a sense of its own, or that a newcomer may not know.

- **Agent's output**: a virtual output given to one agent, never shown unless the owner asks, drawn
  at full rate ([ADR-0015](adr/0015-agents-workspaces.md)).
- **Column**: in scrolling columns, windows stacked from top to bottom that share one width.
- **Development shell**: the environment `nix develop` opens from `flake.nix`, with the pinned
  toolchain and every tool the checks use; CI runs in it too.
- **Dwindle**: a layout where each new window splits the space of the focused one in two.
- **Dynamic workspaces**: an option of the layout: each output keeps one empty unnamed workspace
  last, as niri does ([ADR-0018](adr/0018-workspaces.md)).
- **Fake fullscreen**: an application told it is fullscreen while its column keeps its size.
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
- **llvmpipe**: Mesa's OpenGL driver that draws on the CPU; the headless mode's default
  ([ADR-0022](adr/0022-the-headless-backend-in-yuna.md)).
- **Logical coordinates**: the layout's units, shared by all outputs: an output of 3840 × 2160
  pixels at scale 1.5 measures 2560 × 1440 logical units. Rectangles are snapped to the
  **physical grid**, the output's own pixels.
- **Pinned window**: a floating window shown over whichever workspace its output shows.
- **PQ**: SMPTE ST 2084, the perceptual quantizer: the curve that turns an absolute luminance, up
  to 10,000 cd/m², into an HDR signal. A **PQ code** is that signal as a 10-bit integer, 0 to
  1,023.
- **Refresh boundary**: the moment an output could start showing a new frame; a virtual output's
  come every `1 / rate` seconds of Yuna's clock ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)).
- **Seat**: a set of input devices, a pointer and a keyboard, with its own focus. Each agent has
  one, apart from the owner's.
- **Special workspace**: a hidden workspace shown over the current one by a key.
- **Strip**: in scrolling columns, the row of columns of a workspace; the **view** is the part of
  it the output shows, and scrolls to keep the focused column whole.
- **Virtual output**: an output no monitor shows, drawn offscreen at its own size, rate and scale
  ([ADR-0022](adr/0022-the-headless-backend-in-yuna.md)).
- **VRR**: variable refresh rate; the monitor refreshes when a frame is ready rather than at a fixed
  rate.
- **Working area**: the part of an output tiled windows go in; the whole output until panels
  reserve zones of it.
