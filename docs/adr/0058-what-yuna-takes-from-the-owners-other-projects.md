# 0058. What Yuna takes from the owner's other projects

- Status: Accepted; amends 0007, 0010, 0016, 0023, 0046
- Date: 2026-11-14

## Context

On 2026-11-14 the owner's other projects were read for what they need of a compositor and for
the practices that make them good: SlimeEngine, uwui, Aerith, Magnolia, Opium, Pluto and Slatty.
What bears on Yuna:

- **No list of protocols.** Each milestone names the protocols it adds; no document lists them
  all. `pointer-constraints` and `relative-pointer`, without which a game cannot aim with the
  mouse, appear nowhere in Yuna's documents.
- **Hidden games.** [ADR-0023](0023-frames-presentation-and-the-clock.md) gives a hidden window
  at most one frame callback a second, as niri does. SlimeEngine measured what that does on niri
  (its ADR-0021): a game in Vulkan's FIFO mode, hidden on another workspace, waits a second for
  each image, and its simulation falls to 8 steps a second. Mesa's FIFO waits on frame callbacks
  because niri offers neither `fifo-v1` nor `commit-timing-v1`, which Yuna had put at the games'
  stage ([ADR-0046](0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)). Hyprland gives
  hidden windows no callback, with a rule that keeps chosen ones drawing.
- **Measuring in other compositors.** Five of the projects measure their frames in a headless
  sway or Weston, and record the same limits: presentation times faked, Weston at 60 Hz, no VRR
  emulated. Yuna's honest presentation times, controlled clock and emulated VRR answer all three.
  Pluto and uwui drive the programs they measure through sway's virtual keyboard and pointer
  (`zwp_virtual_keyboard_v1`, `zwlr_virtual_pointer_v1`), and no project can add Yuna to its
  shell: Yuna has no package.
- **GPU resets.** SlimeEngine records that a GPU reset ended niri each time, and the session with
  it. [ADR-0007](0007-surviving-a-crash.md) covers Yuna's own crash; its restart keeps only the
  windows of applications that reconnect, and few do.
- **Packaging.** Yuna's flake has development shells only. Slatty and Opium build their package in
  CI.
- **Reduced motion.** The owner's global rules ask that an effect lighten when reduced motion or
  reduced transparency is asked; the ROADMAP kept only "room" for it.
- **Method.** Aerith, Opium and Magnolia fuzz what they read from outside; Slatty, SlimeEngine and
  Aerith write a crash report on the machine; Aerith's `test-check.sh` proves that each check
  fails when it should.

## Decision

The owner chose, on 2026-11-14:

1. **`docs/protocols.md` lists every protocol, and a test keeps it true.** One row each: offered,
   planned or refused; its stage; who needs it (games, the owner's applications and projects);
   what niri, KWin and Hyprland do. By Stage 2, Yuna offers every protocol that niri or KWin
   offers and an application the owner uses needs; a refused one says why. A test compares the
   rows marked offered with the globals Yuna announces headless.
2. **`fifo-v1` and `commit-timing-v1` come at Stage 2, and a hidden game keeps drawing.** A hidden
   window still gets one frame callback a second, but a game does not: recognised by
   `content-type`, by a rule of the configuration or by being fullscreen, it is paced at a rate
   set in the configuration, 60 frames a second by default, up to its output's full rate.
3. **Yuna 0.1 serves the owner's other projects as their test compositor.** Stage 1 adds a Nix
   package of Yuna; the virtual keyboard and pointer on agents' seats only, never on the owner's
   ([ADR-0015](0015-agents-workspaces.md)); and `docs/client-needs.md`, what each project needs
   of Yuna and Yuna's answer. Each project moves to Yuna in its own sessions; Yuna never changes
   another repository.
4. **Yuna recovers from a GPU reset without restarting, from Stage 2.** It notices its context
   lost, recreates its renderer, imports the clients' buffers again and keeps every window; a
   client that cannot survive its own loss closes, the session stays. If recovery fails, ADR-0007's
   restart takes over. The path is tested headless by forcing it; a real reset is checked by hand.
5. **The package, a NixOS module and a home-manager module live in Yuna's flake**, built by CI at
   each push: the package from Stage 1; the session, `yuna-portal`'s services and the portals'
   configuration in the NixOS module, and the KDL configuration written from Nix in the
   home-manager module, at Stage 2.
6. **Reduced motion and reduced transparency come with each animation and each effect**: with
   M1.7 for motion, at Stage 6 for blur and glass. The rest of accessibility stays later, named in
   the ROADMAP: Yuna's own interfaces read by a screen reader, a magnifier, high contrast, sticky
   and slow keys.
7. **Three practices of the method:**
   - **Fuzzing** of what clients send, from Stage 2, when Yuna writes its own core
     ([ADR-0039](0039-leaving-smithay-step-by-step.md)): `cargo fuzz` in a development shell of its
     own, on a nightly toolchain pinned by date and used for nothing else, run weekly in CI. This
     is the ADR [ADR-0016](0016-the-rust-toolchain.md) asks for a nightly tool.
   - **A crash report on the machine**, from Stage 2 with ADR-0007's restart: version, message,
     backtrace and the log's end, the home folder written `~`, never sent anywhere.
   - **`scripts/test-check.sh`**, now: it breaks a scratch copy in each way the checks guard
     against and expects the named check to stop it.

## Alternatives considered

- **A list of protocols without its test**: lighter, but it would drift. **No list**: gaps stay
  unseen until an application fails.
- **`fifo-v1` and `commit-timing-v1` at Stage 2 with no exception for games**: simpler, as niri,
  but a hidden game still slows. **Nothing before Stage 4**: the owner's week on Yuna would meet
  the defect. A default of 30 frames a second for a hidden game was dropped: a game that ties its
  simulation to its frames would run at half speed.
- **The other projects drive their programs through Yuna's MCP**: no new protocol in Yuna, but
  five projects rewrite their drivers and depend on Yuna to drive anything. **Later**: the
  projects keep measuring with faked presentation times.
- **ADR-0007's restart on a GPU reset**: less code, but most windows lost. **Later**: the session
  ends on each reset until then; recovery is cheaper to design while the core is written.
- **The modules in the owner's NixOS configuration**: nobody else could use them, and nothing
  would check them against Yuna. **A session file alone**: everything else by hand.
- **Accessibility at the stage each part is born** (Yuna's interfaces readable from Stage 2, the
  magnifier and filters at Stage 6): the owner kept the minimum the global rules ask for.
- **A `SECURITY.md` and a dated list of the applications and games checked**, as Slatty keeps:
  declined by the owner.

## Consequences

- Stage 1 grows by a milestone before its audit: the package, the virtual keyboard and pointer on
  agents' seats, `docs/client-needs.md`, `docs/protocols.md` and its test, `scripts/test-check.sh`.
- M1.7 builds reduced motion with its springs and curves.
- Stage 2 grows again, beside [ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md)'s parts:
  pointer constraints and relative pointer, `fifo-v1`, `commit-timing-v1` and the hidden games'
  rate, GPU-reset recovery, the NixOS and home-manager modules, fuzzing, crash reports.
- ADR-0023's rule for hidden windows gains its exception for games; ADR-0046's "games' stage" for
  `fifo-v1` and `commit-timing-v1` becomes Stage 2.
- The virtual keyboard and pointer let any client of an agent's seat inject input on that seat;
  they are never offered on the owner's seat, and a test proves it.
- A game played in a window, with no `content-type` and no rule, falls to one frame a second when
  hidden; the rule is its remedy.
