# 0126. Yuna and the owner's other projects: their test compositor, its package, shared practices

- Status: Accepted; supersedes [0058](0058-what-yuna-takes-from-the-owners-other-projects.md) (the
  protocols' list, the test compositor, the package and modules, the method's practices) and
  [0089](0089-yuna-for-the-owners-other-projects.md) (the package's Mesa, the consumers)
- Date: 2026-11-19

## Context

On 2026-11-14 the owner's other projects (SlimeEngine, uwui, Aerith, Magnolia, Opium, Pluto,
Slatty) were read for what they need of a compositor and for the practices that make them good.
[ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md) drew seven decisions from it, a
few amended or replaced since: [ADR-0061](0061-yunas-uses.md) applied its protocols' rule,
[ADR-0080](0080-what-survives-a-failure-and-yunas-own-panics.md) replaced its recovery from a GPU
reset, and [ADR-0089](0089-yuna-for-the-owners-other-projects.md) carried out its test compositor
without its status saying so. ADR-0058 declined a `SECURITY.md`; the PLAN put it in Stage 1's
audit on 2026-11-16, [ADR-0085](0085-outside-contributions-issues-now-pull-requests-from-0-1.md)
counted on it on 2026-11-17, and the owner confirmed it on 2026-11-19. ADR-0058 also had recovery
from a GPU reset fall back on ADR-0007's restart, and its crash report come with that restart;
ADR-0080 replaced ADR-0007, holds the GPU's recovery, and has its keeper's restart where
ADR-0007's was. A reader needed several ADRs and the PLAN.

This ADR gathers what remains of ADR-0058, with ADR-0089's package and consumers, as it stands on
2026-11-19. It decides nothing new. ADR-0058's other parts live elsewhere: hidden games and
`fifo-v1` in [ADR-0118](0118-frames-presentation-times-and-the-clock.md), the GPU reset in
ADR-0080, reduced motion in
[ADR-0109](0109-animations-and-effects-what-moves-reduced-motion-and-games.md), virtual input and
the agent's seat of `yuna --headless` in
[ADR-0116](0116-agents-seats-each-agent-in-its-own-yuna.md), fuzzing's toolchain in
[ADR-0125](0125-the-rust-toolchain-an-exact-stable-version-a-nightly-for-fuzzing.md), and
`wlr-screencopy` in [ADR-0121](0121-captures-what-is-seen-at-the-logical-size.md).

## Decision

- **`docs/protocols.md` lists every protocol, and a test keeps it true** (ADR-0058, 2026-11-14).
  One row each: offered, planned or refused; its stage; who needs it (games, the owner's
  applications and projects); what niri, KWin and Hyprland do. By Stage 2, Yuna offers every
  protocol that niri or KWin offers and an application the owner uses needs; a refused one says
  why. A test compares the rows marked offered with the globals Yuna announces headless.
- **Yuna 0.1 serves the owner's other projects as their test compositor** (ADR-0058). Each project
  moves to Yuna in its own sessions, starting `yuna --headless --agent-seat` where it started a
  headless sway, its driver and its measure unchanged (ADR-0089, 2026-11-17).
  `docs/client-needs.md` holds what each project needs of Yuna and Yuna's answer. Yuna never
  changes another repository.
- **The Nix package carries its own Mesa** (ADR-0089). `yuna` is wrapped to load the package's
  Mesa through libglvnd, llvmpipe without `--gpu`, the GPU's driver with it, and to find Xwayland,
  so that it runs the same on NixOS, on another distribution with Nix, and in a consumer's CI on
  Ubuntu, drawing what Yuna's own tests draw.
- **The package, a NixOS module and a home-manager module live in Yuna's flake**, built by CI at
  each push (ADR-0058): the package from Stage 1; at Stage 2, the session, `yuna-portal`'s services
  and the portals' configuration in the NixOS module, and the KDL configuration written from Nix in
  the home-manager module. The desktop's session takes the system's drivers through the NixOS
  module (ADR-0089).
- **The consumers are flake inputs, pinned by revision** (ADR-0089). uwui and Pluto are
  `flake = false` inputs of Yuna's flake, their revision in the URL, given to the development shell
  by a variable each; the tests run uwui's `measure.py` with the shell's Python and build Pluto's
  `pluto-input` with cargo from Pluto's own lock. A revision is raised in a commit of its own.
- **A crash report on the machine**, from Stage 2 with the keeper's restart (ADR-0058): version,
  message, backtrace and the log's end, the home folder written `~`, never sent anywhere. A panic
  hook writes it (ADR-0080).
- **`scripts/test-check.sh`** breaks a scratch copy in each way the checks guard against, and
  expects the named check to stop it (ADR-0058).
- **Yuna has a `SECURITY.md`** (the owner's decision, put in the PLAN on 2026-11-16 and confirmed
  on 2026-11-19), against ADR-0058's refusal of 2026-11-14. A dated list of the applications and
  games checked, as Slatty keeps, stays declined (ADR-0058).

## Alternatives considered

- **A list of protocols without its test, or no list**
  ([ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)): it would drift, or gaps stay
  unseen until an application fails.
- **The other projects driving their programs through Yuna's MCP**
  ([ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)): five projects would rewrite
  their drivers and depend on Yuna to drive anything.
- **The modules in the owner's NixOS configuration**
  ([ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)): nobody else could use them,
  and nothing would check them against Yuna.
- **The system's drivers in the package** ([ADR-0089](0089-yuna-for-the-owners-other-projects.md)):
  a light package, but it does not start where no driver is installed, a CI on Ubuntu among them.
- **A clone of each consumer inside the test, or the consumers built by Nix**
  ([ADR-0089](0089-yuna-for-the-owners-other-projects.md)): the network at every new revision, or
  Nix fetching the 597 crates of Pluto's lock to build six.

## Consequences

- `swaymsg` and sway's IPC have no equivalent: a script that reads sway's window tree waits for
  Yuna's own interface.
- The package costs about 1.4 GB to fetch once a revision of nixpkgs, Mesa's 1.1 GB of it; the
  shell fetches the same Mesa already.
- `scripts/check.sh` fetches about 13 MB of the consumers' sources with the shell, and builds
  `pluto-input` once.
- Stage 2 carries the NixOS and home-manager modules, fuzzing and crash reports.

## Validation and reconsideration

Evidence: the tests that run uwui's `measure.py --display` and Pluto's `pluto-input` against
`yuna --headless --agent-seat` (`crates/yuna/tests/consumers.rs`), the protocols' test, and CI's
build of the package. Proposed again when the desktop's session needs the system's drivers.
