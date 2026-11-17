# Contributing

How to set up the environment, what each command does, and how a change is made and checked. The
rules for agents are in [`AGENTS.md`](AGENTS.md); how the code is organised, in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Environment

Yuna is built on Linux, in the Nix development shell that `flake.nix` defines. It needs Nix with
flakes; nothing else is installed by hand.

```sh
nix develop                 # enter the shell
nix develop -c <command>    # or run one command in it
```

The shell gives:

- the Rust toolchain named in [`rust-toolchain.toml`](rust-toolchain.toml), with clippy, rustfmt,
  rust-src and rust-analyzer ([ADR-0016](docs/adr/0016-the-rust-toolchain.md));
- `cargo deny`, which checks the dependencies against [`deny.toml`](deny.toml);
- `lychee`, which checks the links between the Markdown files;
- git, which the link check uses to list the Markdown files;
- libxkbcommon, which Smithay's keyboard links;
- `YUNA_DEV_SHELL`, which `scripts/check.sh` requires;
- what Yuna draws with, loaded at run time: `LD_LIBRARY_PATH` names libglvnd's EGL, and
  `__EGL_VENDOR_LIBRARY_FILENAMES` the shell's own Mesa, so that Yuna and its tests draw on that
  Mesa's llvmpipe, never on the host's drivers
  ([ADR-0022](docs/adr/0022-the-headless-backend-in-yuna.md));
- `XKB_CONFIG_ROOT`, where the keyboard's default keymap comes from;
- the applications the tests run in Yuna, kept off the shell's own `PATH`: Alacritty, GTK 4's
  `gtk4-demo`, Qt's `qml`, `vkcube`, mpv and Wine, in `YUNA_APPLICATIONS_PATH`, with what the
  tests give them: `YUNA_XWAYLAND_PATH` (Xwayland's folder, Yuna's own `PATH` in the tests),
  `YUNA_LAVAPIPE_ICD` (Mesa's Vulkan on the CPU),
  `YUNA_FONTS_CONF` (DejaVu alone), `YUNA_QT_PLUGINS` and `YUNA_QML_IMPORTS` (for `qml`, which
  is not wrapped), and `YUNA_CLIENT_LIBRARY_PATH` (the libraries SlimeEngine's example loads).

A second shell, `nix develop .#bench`, adds what the rivals' bench runs, by their store paths, in
`YUNA_BENCH_*` variables: sway, KWin, dwl and mango (each also built without XWayland, for equal
work), Umbriel (built from its source at the revision `flake.lock` pins, the flake's `umbriel`
input), Hyprland, Mutter, wlr-randr, weston, ffmpeg, D-Bus, Xwayland, the Mesa they all draw
with, coreutils and bash. CI never fetches it.

Yuna itself reads two variables: `XDG_RUNTIME_DIR`, the folder its Wayland socket goes in, which
`yuna --headless` requires; and `YUNA_LOG`, the level of its logs on standard error: `error`,
`warn`, `info` (the default), `debug` (a line per frame drawn, and per round of the loop with its
counters) or `trace`.

Nix sees only the files git knows: `git add` a new file before `nix develop` reads it. Plain
`nix develop`, never `nix develop path:.`, which copies the whole folder, `target/` included, into
the store.

Without Nix, rustup reads `rust-toolchain.toml` and installs the same toolchain, enough to build,
run and test Yuna. `scripts/check.sh` still refuses to run outside the shell: there, nothing would
pin the versions of `cargo deny` and `lychee`, and a check could pass for the wrong reason.

## Commands

```sh
nix develop -c scripts/check.sh           # every check; what CI runs
nix develop -c cargo run -- --help        # Yuna's options
nix develop -c cargo run -- --headless --output 1920x1080@60,1   # Yuna without a screen
nix develop -c cargo run -- mcp            # an MCP server on standard input and output
nix develop -c cargo test --workspace     # the tests alone
nix develop -c cargo run --release -p yuna-layout --example timing   # the layout's timings
```

A frame's cost is measured with `crates/yuna/examples/frame_time.rs`: it runs the `yuna` built
beside it with `vkcube` drawing in two windows, and reads Yuna's frames, CPU and GPU time
([ADR-0030](docs/adr/0030-a-frames-gpu-time-from-the-kernel.md)); its documentation gives its
options, `--gpu` among them:

```sh
nix develop -c cargo build --release -p yuna --bin yuna --example frame_time
nix develop -c target/release/examples/frame_time
```

It needs no screen of its own. On the owner's machine, agents run it inside `agent-headless`,
one measure at a time.

Motion's cost is measured with `crates/yuna/examples/motion_time.rs`, as
[ADR-0056](docs/adr/0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md) asks: it
runs Yuna in its own process with six `vkcube`, and plays a round of scrolls, column moves,
workspace switches and the special workspace's fade, against a still baseline of as many frames;
it prints, for each, the frames a second against the output's rate, the frames drawn at once and
missed, and where the misses fall; then the heaviest frame and the ADR's verdict. The verdict names
the mode it ran at, and counts as the ADR's only at 3840x2160@240,1.5 on `--gpu`; anywhere else it
is recorded without a pass mark. It gives none when the baseline ran below 95 % of the output's
rate. Under `--reduced-motion` the round has no retarget: its load is jumps and fades. Its
documentation gives its options, `--gpu` among them:

```sh
nix develop -c cargo build --release -p yuna --example motion_time
nix develop -c target/release/examples/motion_time --output 1920x1080@240,1
```

It runs alone among measurements, as `frame_time` does; on the owner's machine, inside
`agent-headless`, under the agents' lock of exclusive runs. The measures share
`crates/yuna/examples/common/`; the `vkcube` clients of these two are in its `vkcube.rs`.

XWayland's cost is measured with `crates/yuna/examples/xwayland_time.rs`: an X11 program's wait
for Xwayland, cold and after an idle exit; Xwayland's memory and Yuna's with one X11 window; and,
with `--area-filter`, a frame's time for a fullscreen X11 window drawn through the area filter
against one copied pixel for pixel. Beside each phase it reads what the machine's other programs
use, its own processes left out, and marks a phase they disturb; run inside `agent-headless`, it
cannot read their GPU time, and says so. Its documentation gives its options, `--gpu` among
them:

```sh
nix develop -c cargo build --release -p yuna --bin yuna --example xwayland_time
nix develop -c target/release/examples/xwayland_time
nix develop -c target/release/examples/xwayland_time --area-filter
```

The rivals are measured with `crates/yuna-bench`
([ADR-0075](docs/adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md),
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md#the-rivals-bench), where a table gives how each
compositor runs). It runs sway, KWin, Yuna, dwl, mango, Umbriel, Hyprland and Mutter headless on
the GPU's render node, at 3840 × 2160, 240 Hz and scale 1.5, each in a sandbox of its own,
through the same scenarios, and reads their costs from outside. niri and cosmic-comp cannot run
headless: they are measured on the owner's tour alone. It needs the bench shell and the user's
systemd manager, which makes each sandbox (`systemd-run --user --scope`):

```sh
nix develop .#bench -c cargo run --release -p yuna-bench -- headless --compositor sway --scenario rest
nix develop .#bench -c cargo run --release -p yuna-bench -- report target/yuna-bench/run-*.json
```

`headless` takes lists separated by commas: `--compositor` (`sway`, `sway-vulkan`, `kwin`,
`yuna`, `dwl`, `mango`, `umbriel`, `hyprland`, `mutter`; `sway,kwin,yuna` by default),
`--scenario` (`rest`, `game`, `film`, `pacing`, `motion`), `--windows` (`rest`'s, 1, 15 and 150
by default) and `--configuration` (`as-shipped`, `equal-work`); `--runs` (3 by default), run in a
random order whose seed it records, `--seed` to replay one. `--seconds` makes every measured
window shorter, to try it: such figures are not comparable. With Yuna among the compositors, it
builds it for release first. On 2026-11-16, every recipe and scenario at equal work, three times,
189 cases, took 3 h 07 min; as shipped, once each, 63 cases, 1 h 03 min.

It writes `target/yuna-bench/run-<time>.json`, the run file (`--out` for another folder), before
its first case and again after each, through a temporary file and a rename: a run cut short
keeps every case it ran, and `report` summarises it. At its end it writes the summary in
Markdown beside it. It keeps the run's folder of logs in `/tmp/yuna-bench-<pid>-<time>` when a
case fails, or with `--keep`. It takes the agents' exclusive lock itself, waiting for any other
exclusive run: `agents-exclusive-run.lock` in `$OWNER_RUNTIME_DIR`, or in `/run/user/<uid>` when
that is unset, never in an agents' service's own `XDG_RUNTIME_DIR`. Under `run.sh --exclusive`,
which locks that file when `OWNER_RUNTIME_DIR` is set, it uses the lock its parent holds; with
the variable unset, `run.sh` locks a file in its service's own folder, and the bench takes the
owner's itself. The run file says which file it locked, and how. No descriptor it inherited
reaches a compositor or a client. Killed outright, it leaves its sandboxes running until its next
start kills them.

Its sandbox detects; it does not prevent. The rivals run as the owner's user, without
namespaces, kept off the owner's display, devices, runtime folder and session bus by their
configuration alone: a cleared environment, and a runtime folder and session bus of their own.
The bench checks each compositor and its clients once the compositor is ready, again once a
scenario's clients are up, before anything is measured, and every 250 ms in each measured window
for the device nodes. A process that opens a `/dev/dri/card*` node or an input device, reaches a
socket in the owner's runtime folder (their Wayland display and session bus among them) or an
X11 display served outside its sandbox, or names that folder in its environment, has its case
refused and stopped, and nothing of it measured; what it did before the check found it is not
undone. To see which socket a socket is connected to, the checks read the kernel's socket
diagnostics (`sock_diag`); without them, they see only the sockets bound in the owner's runtime
folder, and each case's run says so. The bench itself never starts anything on the owner's
display.

The rivals on wlroots' headless backend (sway, dwl, mango, Umbriel) draw a frame 4 ms after each
commit, since wlroots counts whole milliseconds: up to 250 frames a second for the 240 Hz mode,
fewer as each frame takes time to draw. The others draw at the mode's rate. The summary gives
the rates their runs of `game` reached, and how it ranks is in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md#the-rivals-bench).

`yuna mcp` is what an agent's MCP configuration runs: a command speaking MCP on its standard
input and output, with `--gpu`, one `--output` for each seat's first output, `--manual-clock`
([ADR-0033](docs/adr/0033-the-mcp-server.md)), and `--animations` or `--reduced-motion`, which
`yuna --headless` takes too: both show each change at once without them
([ADR-0055](docs/adr/0055-what-moves-first-and-animations-off-headless.md),
[ADR-0059](docs/adr/0059-reduced-motion-nothing-slides-fades-stay.md)). An agent's first call is `seat
create`; every other tool but `clock` takes the seat's handle
([ADR-0037](docs/adr/0037-seats-tools-input-and-closing.md)). For Claude Code, for example:
`claude mcp add --transport stdio yuna -- <path to yuna> mcp`. It stops when its input closes,
and ends the programs it started then.

`yuna --headless` prints `WAYLAND_DISPLAY=<socket>`; a client started with that variable connects
to it. It draws on llvmpipe; `--gpu` draws on the GPU's render node instead, and is never used by
the tests. SIGTERM or Ctrl-C stops it and removes its socket. To run one of the tests'
applications in it by hand, without a screen of your own:
`PATH=$YUNA_APPLICATIONS_PATH:$PATH WAYLAND_DISPLAY=<socket> vkcube --wsi wayland`.

The compositor's tests (`crates/yuna/tests/headless/`) run it in their own process, on the manual
clock, with clients written with `wayland-client`; they need the shell's Mesa. The
applications' tests (`crates/yuna/tests/applications.rs`) run the program with real applications
and check that it rests; they need the shell's applications, and take about 45 s, Qt's `qml` on
X11 and Wine's notepad the longest, which each wait for Xwayland's end.
Each `yuna` has one output, 1920 × 1080 at 60 Hz, or the mode `YUNA_APPLICATIONS_OUTPUT` names.
`crates/yuna/tests/mcp.rs` runs `yuna mcp` with the programs it starts, Alacritty among them,
in about 4 s; the MCP server's exchanges and the seats' input are tested in the compositor's
tests (`mcp.rs` and `seats.rs` there).
SlimeEngine's examples run in ignored tests, `window`, `editor` and M1.5's done-when story on
`interface`, given a folder that holds them, as the weekly job runs them:

```sh
nix develop -c cargo test -p yuna --test applications -- --nocapture
YUNA_APPLICATIONS_OUTPUT=3840x2160@240,1.5 nix develop -c cargo test -p yuna --test applications
YUNA_SLIMEENGINE_EXAMPLES=<folder> nix develop -c cargo test -p yuna --test applications -- --ignored
```

The folder is made as `.github/workflows/slimeengine.yml` makes it, in this shell, from
SlimeEngine's checkout at the revision it pins: the `window`, `interface` and `editor` binaries
(`cargo build --release --locked -p slime-examples --example window --example interface
--example editor`), `cooked/`, the examples' data cooked (`cargo run --release --locked -p
slime-cook -- cook examples/assets <folder>/cooked`), and `assets/`, a copy of
`examples/assets`.

The property tests, the layout model's (`crates/yuna-layout/tests/properties.rs`) and the
compositor's random requests (`crates/yuna/tests/headless/random_requests.rs`), shrink a failure
to the shortest sequence that still fails, and keep its seed in a `.proptest-regressions` file
beside the test. Commit that file with the fix: the case then runs at every check. A seed
replays its case only while the test's generator stays the same; when you change the random
requests' generator, write each sequence found into `sequences_found_before` in that file, and
drop the seeds. The random requests run in four tests, two of them with animations on, each as
many cases as fitted about 10 s of the check at M1.4
([`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md#tests)); `PROPTEST_CASES` runs that many in
each, for a search before a release or after a change to the protocols' handling. With
`--nocapture`, each test prints how often the requests that need the keyboard's focus took
effect:

```sh
PROPTEST_CASES=2500 nix develop -c cargo test -p yuna --test headless random_requests -- --nocapture
```

The timings are measured on an idle machine, one measurement at a time, and go to
[`docs/measurements.md`](docs/measurements.md).

`scripts/check.sh` runs, in order, and stops at the first failure:

1. the workspace's own rules: `rust-version` in `Cargo.toml` equals the toolchain's version, and
   every crate takes the workspace's lints;
2. formatting, `cargo fmt --check`;
3. clippy over every target, warnings as errors;
4. the build and the tests;
5. the documentation, `cargo doc`, warnings as errors;
6. `cargo deny`: licences compatible with GPL-3.0-or-later; every RustSec advisory and every yanked
   crate, on any dependency, direct or not; the sources of the dependencies. It reads the advisory
   database from the network;
7. the links between the tracked Markdown files, offline, anchors included;
8. the length of `AGENTS.md` (1,500 words at most) and `docs/PLAN.md` (2,000), read at every
   session.

Every cargo command of the check passes `--locked`: `Cargo.lock` changes only on purpose.

A new crate goes in `crates/` and its `Cargo.toml` holds the two lines `[lints]` and `workspace = true`, so
that the workspace's lints (the `// SAFETY:` comment, documentation) reach it; the check refuses it
otherwise.

## Continuous integration

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs `scripts/check.sh` in the same shell, on
each push to `main`, each pull request, and every Monday. CI stays one job while one is enough
([ADR-0036](docs/adr/0036-yuna-is-public.md)). A change of Markdown files alone skips it:
[`.github/workflows/docs.yml`](.github/workflows/docs.yml) runs `scripts/check.sh docs`: their
links, the length of those read at every session, the ADRs' index, the changelog's fragments and
the tests of the scripts that check them, with lychee, git and python3 alone, in a minute or two. `scripts/check.sh docs` runs the same locally.
[`.github/workflows/slimeengine.yml`](.github/workflows/slimeengine.yml) runs SlimeEngine's
`window`, `editor` and `interface` examples in Yuna every Thursday, on demand, and on a push that
changes the drawing path or what the agent's story uses; it builds the examples and cooks their
data once per pinned revision and keeps the folder in GitHub's cache
([ADR-0029](docs/adr/0029-the-rest-of-animated-applications.md),
[ADR-0043](docs/adr/0043-text-input-and-the-rest-of-slimeengines-examples.md)). Its
times are in [`docs/measurements.md`](docs/measurements.md).

## Dependencies

A new dependency is asked of the owner first, with its reason, an alternative weighed and its
licence. The owner accepted proptest, for the layout model's tests, on 2026-11-11, and, for
the HDR prototype's branch only (M1.3, deleted since), the same day, Smithay's `backend_drm`, `backend_gbm`,
`backend_egl`, `renderer_gl`, `backend_session_libseat` and `backend_libinput` features with the
system libraries they link. Proptest serves the compositor's tests too since 2026-11-12
([ADR-0025](docs/adr/0025-applications-in-ci.md)). For the headless compositor (M1.4), the owner accepted on 2026-11-12
Smithay's `wayland_frontend`, `desktop` and `renderer_gl` features; `tracing` and
`tracing-subscriber` with its `fmt` and `std` features alone; `wayland-client` and the client side
of `wayland-protocols`, for tests only; and, in the shell, Mesa, libglvnd, libxkbcommon,
xkeyboard-config, a font, and the applications the tests run
([ADR-0025](docs/adr/0025-applications-in-ci.md)); on 2026-11-12 too, the action
`actions/cache`, for SlimeEngine's binary. For the MCP server (M1.5), the owner accepted on
2026-11-13 `serde`, with its `derive` feature, and `serde_json`
([ADR-0033](docs/adr/0033-the-mcp-server.md)), and `png` for captures, which C4 adds
([ADR-0034](docs/adr/0034-captures.md)). Smithay is Yuna's public fork, read over HTTPS and
pinned to one commit ([ADR-0017](docs/adr/0017-smithays-revision.md),
[ADR-0045](docs/adr/0045-the-fork-over-https-a-branch-per-base.md)); its patches are listed in
[`docs/smithay-patches.md`](docs/smithay-patches.md).
Raising the toolchain, Smithay, the other crates or `flake.lock` happens at a stage's end, each in a
commit of its own.

## Contracts

What other programs and projects rely on
([ADR-0079](docs/adr/0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)):

- the command line: `yuna --headless`, `yuna mcp`, their options, the `WAYLAND_DISPLAY=<socket>`
  line printed at start, the exit codes;
- the environment Yuna reads (`XDG_RUNTIME_DIR`, `YUNA_LOG`) and gives its programs (`YUNA_SEAT`);
- the MCP server: its revisions, its tools' names, arguments and results;
- the Nix package's outputs, once it exists (M1.10).

Until 1.0 a contract may change, never silently: a change that breaks a consumer adds a
`changed-…` or `removed-…` fragment to [`changelog.d/`](changelog.d/README.md) that says what to do
instead; an addition, an `added-…` one. The logs' text and the library's API are not contracts.

## From outside the project

([ADR-0085](docs/adr/0085-outside-contributions-issues-now-pull-requests-from-0-1.md))

- **Issues** are welcome now: bugs, with the steps and the test data that show them; protocols an
  application needs; ideas.
- **Pull requests** are taken from 0.1 on. Open an issue first for a change that touches an ADR,
  a [contract](#contracts) or a dependency.
- A pull request passes `scripts/check.sh` and meets the definition of done in
  [`AGENTS.md`](AGENTS.md#definition-of-done): its tests and its documents come with it.
- A reviewing agent prepares the review; the owner decides.
- An ADR you propose is numbered `NNNN`; it takes its number when merged.
- Pull requests are squashed. The commit's message is written as the repository's are, and holds
  no AI attribution; you stay its author.
- While Stage 2a replaces the core ([ROADMAP](docs/ROADMAP.md)), the code it is replacing takes
  fixes only.
- Your contribution comes under GPL-3.0-or-later, as the rest of Yuna; there is no licence
  agreement to sign.

## Making a change

1. Pick the current milestone in [`docs/PLAN.md`](docs/PLAN.md).
2. Make the change, with its tests and its documentation.
3. Run `scripts/check.sh`; measure what the milestone touches.
4. A change to a [contract](#contracts) adds its changelog fragment.
5. Commit one logical change at a time, in English, the subject in the imperative.
