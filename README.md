<p align="center">
  <img src="logo/yuna.svg" alt="Yuna's logo: a pink hibiscus flower on a purple ribbon, between two white wings" width="256">
</p>

# Yuna

A Wayland compositor in Rust, for Linux: scrolling columns, dwindle and floating windows, HDR, VRR
and animations, for daily use, games and films. It also runs without a screen, where agents and
tests drive it.

## State

Stage 1, the headless core, is in its audit before release 0.1 ([ROADMAP](docs/ROADMAP.md)).
Yuna runs without a screen: virtual outputs drawn offscreen, windows placed by its layout model,
frames that follow damage, animations, emulated VRR and HDR outputs, and X11 programs through
Xwayland. Real applications (Alacritty, GTK 4, Qt 6, Vulkan, mpv, Wine) run in it in CI, and it
rests when they do; SlimeEngine's examples and Opium, the owner's music player, run in it every
week, Opium driven by an agent.

`yuna mcp` is an agent's toolbox: each agent gets a seat of its own, starts programs there on a
throwaway home, finds their windows and their accessibility trees, clicks, types and drags,
between X11 and Wayland programs too, captures what they show, reads and sets the clipboard,
waits for events, and asks what woke Yuna. Other projects drive their programs in
`yuna --headless --agent-seat` as in a headless sway. The desktop session comes at Stage 2.

## Build

On Linux, with Nix:

```sh
nix develop -c scripts/check.sh    # build, test and check everything
nix develop -c cargo run -- --help
nix develop -c cargo run -- --headless    # prints WAYLAND_DISPLAY=<socket>; Ctrl-C stops it
```

Yuna's package runs without the shell, with its own Mesa and Xwayland:

```sh
nix run github:HoneySlatt/Yuna -- --headless --agent-seat
```

[`CONTRIBUTING.md`](CONTRIBUTING.md) gives the detail.

## Contributing

Issues are welcome: bugs, protocols an application needs, ideas. Pull requests are taken from
0.1 on, as [`CONTRIBUTING.md`](CONTRIBUTING.md#from-outside-the-project) says
([ADR-0085](docs/adr/0085-outside-contributions-issues-now-pull-requests-from-0-1.md)). A
vulnerability is reported privately, as [`SECURITY.md`](SECURITY.md) says, never in an issue.

## Where to read

Each part, its folder, and the section of [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) that
tells how it fits:

| Part | Folder | In ARCHITECTURE |
|---|---|---|
| The layout model | [`crates/yuna-layout`](crates/yuna-layout/src/lib.rs) | [The layout model](docs/ARCHITECTURE.md#the-layout-model) |
| Motion | [`crates/yuna-motion`](crates/yuna-motion/src/lib.rs) | [Motion](docs/ARCHITECTURE.md#motion) |
| The headless compositor | [`crates/yuna/src`](crates/yuna/src/lib.rs) | [The headless compositor](docs/ARCHITECTURE.md#the-headless-compositor) |
| XWayland | [`crates/yuna/src/xwayland`](crates/yuna/src/xwayland/mod.rs) | [XWayland](docs/ARCHITECTURE.md#xwayland) |
| The MCP server | [`crates/yuna/src/mcp`](crates/yuna/src/mcp/mod.rs) | [The MCP server](docs/ARCHITECTURE.md#the-mcp-server) |
| Accessibility | [`crates/yuna/src/mcp/accessibility`](crates/yuna/src/mcp/accessibility/mod.rs), [`crates/yuna/src/dbus`](crates/yuna/src/dbus/mod.rs) | [Accessibility](docs/ARCHITECTURE.md#accessibility) |
| The rivals' bench | [`crates/yuna-bench`](crates/yuna-bench/src/lib.rs) | [The rivals' bench](docs/ARCHITECTURE.md#the-rivals-bench) |
| The tests | `crates/*/tests`, and each module's own | [Tests](docs/ARCHITECTURE.md#tests) |

## Read next

- [`docs/ROADMAP.md`](docs/ROADMAP.md): what Yuna is, its quality bar and its stages.
- [`docs/PLAN.md`](docs/PLAN.md): the work in progress.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): how the code is organised.
- [`docs/adr/`](docs/adr/README.md): the decisions and why.
- [`docs/survey.md`](docs/survey.md): the facts of the field behind them, with sources.
- [`AGENTS.md`](AGENTS.md): the rules for agents working here.

## Licence

The code is under GPL-3.0-or-later, in [`LICENSE`](LICENSE). The logo is not under it: its author keeps its rights, and
[`logo/README.md`](logo/README.md) says how it may be shown.
