<p align="center">
  <img src="logo/yuna.svg" alt="Yuna's logo: a pink hibiscus flower on a purple ribbon, between two white wings" width="256">
</p>

# Yuna

A Wayland compositor in Rust, for Linux: scrolling columns, dwindle and floating windows, HDR, VRR
and animations, for daily use, games and films. It also runs without a screen, where agents and
tests drive it.

## State

Designed on 2026-11-11. The workspace and its checks exist; no backend yet, so Yuna cannot start a
session. The first stage builds the headless core.

## Build

On Linux, with Nix:

```sh
nix develop -c scripts/check.sh    # build, test and check everything
nix develop -c cargo run -- --help
```

[`CONTRIBUTING.md`](CONTRIBUTING.md) gives the detail.

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
