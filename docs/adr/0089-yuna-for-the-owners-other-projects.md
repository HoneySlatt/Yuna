# 0089. Yuna for the owner's other projects: an agent's seat, wlr-screencopy, its own Mesa

- Status: Superseded by [0116](0116-agents-seats-each-agent-in-its-own-yuna.md) (an agent's seat in
  `yuna --headless`, virtual input), [0121](0121-captures-what-is-seen-at-the-logical-size.md)
  (`wlr-screencopy-unstable-v1`), [0126](0126-yuna-and-the-owners-other-projects.md) (the package's
  Mesa, the consumers)
- Date: 2026-11-17

## Context

[ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md) makes Yuna 0.1 the test
compositor of the owner's other projects, and M1.10 delivers it: uwui's `scripts/measure.py`
and Pluto's bench drive a program in Yuna as in a headless sway, without changing their
repositories ([PLAN](../history.md#stage-1--the-headless-core--01)). Read on 2026-11-17,
at uwui `fb77f0d` and Pluto `4624243`:

- `measure.py --display` measures on a compositor it does not start, through
  `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`. Its helper, `scripts/wayland.py`, moves and clicks
  through `zwlr_virtual_pointer_v1`, finds the outputs through `xdg-output`, and counts the
  frames through `wlr-screencopy-unstable-v1` version 3, copies with damage, and nothing else.
- Pluto's bench drives its subjects through `pluto-input`, one virtual keyboard
  (`zwp_virtual_keyboard_v1`, its own keymap) and one virtual pointer for a whole run, created
  for the client's `wl_seat`. The rest of the bench, `swaymsg` and sway's window events, is
  sway's own and out of reach.
- Every client of `yuna --headless`'s socket was on the owner's seat: a keyboard of the
  machine's layout, no pointer, and, by ADR-0058, no virtual input ever.
- Yuna offered `ext-image-copy-capture-v1` alone ([ADR-0038](0038-captures-coordinates-windows-and-wayland.md));
  Smithay has no handler of wlr-screencopy. Its virtual pointer did not say which client made
  it, which a pointer made without a seat needs.
- Yuna draws with EGL loaded at run time, through libglvnd, and its development shell points it
  at the shell's own Mesa ([ADR-0022](0022-the-headless-backend-in-yuna.md)). The flake had no
  package.
- The consumers' tests run in `scripts/check.sh`, so locally and in CI, at a pinned revision
  ([ADR-0079](0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)).

## Decision

The owner chose, on 2026-11-17, one decision at a time:

- **`yuna --headless --agent-seat` serves an agent's seat.** With it, the owner's seat has no
  output, socket or `wl_seat`, as in `yuna mcp`, and one agent's seat takes the outputs and a
  socket of its own in `XDG_RUNTIME_DIR`, named as the owner's would be: a `us` keyboard, a
  pointer, Yuna as its input method. Without it nothing changes, so that the owner's seat stays
  testable without a screen, for Hollow and the clipboard managers of `ext-data-control-v1`.
  The virtual keyboard and pointer are offered to the clients of agents' seats alone, in `yuna
  mcp` too, each device acting on its client's own seat, never the owner's
  ([ADR-0078](0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md): no rule between
  two agents' seats).
- **Yuna offers `wlr-screencopy-unstable-v1` version 3**, its own handler on the captures
  `ext-image-copy-capture-v1` uses, under the same rules: every client sees it, a client copies
  only its own seat's outputs, one copy a refresh at most, a copy with damage waits at rest
  without a wake-up, and its frames count among the capture sessions' bounds. `wl_shm` buffers
  alone; no cursor, Yuna drawing none. Its server side is `wayland-protocols-wlr`, already in
  Cargo.lock through Smithay, under MIT; its client side joins the tests.
- **The Nix package carries its own Mesa.** `yuna` is wrapped to load the package's Mesa through
  libglvnd, llvmpipe without `--gpu`, the GPU's driver with it, and to find Xwayland, so that it
  runs the same on NixOS, on another distribution with Nix, and in a consumer's CI on Ubuntu,
  drawing what Yuna's own tests draw. It costs about 1.4 GB to fetch once a revision of nixpkgs,
  Mesa's 1.1 GB of it. The desktop's session takes the system's drivers through the NixOS module
  at Stage 2.
- **The consumers are flake inputs, pinned by revision.** uwui and Pluto are `flake = false`
  inputs of Yuna's flake, their revision in the URL, given to the development shell by a
  variable each; the tests run `measure.py` with the shell's Python and build `pluto-input`
  with cargo from Pluto's own lock. A revision is raised in a commit of its own.

Two choices within these, the implementer's: Yuna's fork of Smithay gains
`VirtualPointerDevice::client()` ([patches](../smithay-patches.md)), and a virtual keyboard's
keys switch its seat's keyboard to the device's keymap, and Yuna's own keys switch it back.

## Alternatives considered

- **`--headless` always an agent's seat**: nothing to pass, and one step towards one seat per
  Yuna, but no test of the owner's seat without a screen, and a change of contract: the seat
  named `agent-<n>`, its layout fixed.
- **Virtual input on the owner's seat when headless**: the least code, but against ADR-0078's
  test that agents' input never reaches the owner's seat; it would need a new ADR.
- **No wlr-screencopy**: one capture protocol, the standard one, but `measure.py` counts no
  frame in Yuna until uwui changes, and M1.10's goal is half met.
- **The system's drivers in the package**: a light package, the desktop's form, but it does
  not start where no driver is installed, a CI on Ubuntu among them, and draws differently from
  one machine to the next. **Both outputs**: two to keep and test, one unused before Stage 2.
- **A clone of each consumer inside the test**: a lighter shell, but the test needs the network
  at every new revision, and without it would fail or be skipped; the flake inputs need it once,
  with the shell, and `pluto-input`'s crates once, from crates.io, as Yuna's own. **The consumers built by Nix**: the most
  reproducible, but Nix would fetch the 597 crates of Pluto's lock to build six.

## Consequences

- A project moves to Yuna by starting `yuna --headless --agent-seat` where it started sway; its
  driver and its measure need no change. `swaymsg` and sway's IPC have no equivalent: a script
  that reads sway's window tree waits for Yuna's own interface.
- A program of an agent's seat may inject input on that seat; the owner's seat is never shown
  the globals, and a test proves it.
- Yuna keeps one more capture protocol, deprecated upstream in favour of
  `ext-image-copy-capture-v1`, until no tool the owner uses speaks it.
- The package's closure is large; the shell fetches the same Mesa already.
- `scripts/check.sh` fetches about 13 MB of the consumers' sources with the shell, and builds
  `pluto-input` once.

## Validation and reconsideration

Checked by the tests that run uwui's `measure.py --display` and Pluto's bench's driver,
`pluto-input`, against
`yuna --headless --agent-seat` (`crates/yuna/tests/consumers.rs`), by the stories of virtual
input (`tests/headless/virtual_input.rs`) and of wlr-screencopy (`tests/headless/wlr_screencopy.rs`),
and by CI's build of the package. Proposed again when uwui leaves wlr-screencopy, when Stage 2's
core decides how seats live ([ADR-0078](0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)),
or when the desktop's session needs the system's drivers.
