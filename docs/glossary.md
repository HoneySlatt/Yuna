# Glossary

Terms Yuna uses in a sense of its own, or that a newcomer may not know.

- **Agent's output**: a virtual output given to one agent, never shown unless the owner asks, drawn
  at full rate ([ADR-0015](adr/0015-agents-workspaces.md)).
- **Development shell**: the environment `nix develop` opens from `flake.nix`, with the pinned
  toolchain and every tool the checks use; CI runs in it too.
- **Dwindle**: a layout where each new window splits the space of the focused one in two.
- **Fake fullscreen**: an application told it is fullscreen while its column keeps its size.
- **Game mode**: the state Yuna enters when a game is fullscreen: effects and animations off,
  notifications silent, VRR in its band.
- **Headless mode**: Yuna running without a physical screen, on virtual outputs.
- **Seat**: a set of input devices, a pointer and a keyboard, with its own focus. Each agent has
  one, apart from the owner's.
- **Special workspace**: a hidden workspace shown over the current one by a key.
- **The games' band**: the high, narrow range of refresh rates Yuna keeps a game's VRR in, by
  showing frames several times ([ADR-0014](adr/0014-vrr-without-flicker.md)).
- **VRR**: variable refresh rate; the monitor refreshes when a frame is ready rather than at a fixed
  rate.
