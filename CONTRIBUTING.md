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
  rust-src and rust-analyzer ([ADR-0125](docs/adr/0125-the-rust-toolchain-an-exact-stable-version-a-nightly-for-fuzzing.md));
- `cargo deny`, which checks the dependencies against [`deny.toml`](deny.toml);
- `lychee`, which checks the links between the Markdown files;
- git, which the link check uses to list the Markdown files;
- libxkbcommon, which Smithay's keyboard links;
- `YUNA_DBUS_DAEMON`, the absolute path of nixpkgs' `dbus-daemon`, which Yuna runs for each
  agent's seat's accessibility bus and the D-Bus code's tests run, never the host's; Yuna's
  package sets the same;
- `YUNA_UNSHARE`, the absolute path of util-linux's `unshare`, which the Opium test uses to put
  Opium in a network namespace, never the host's;
- `YUNA_DEV_SHELL`, which `scripts/check.sh` requires;
- what Yuna draws with, loaded at run time: `LD_LIBRARY_PATH` names libglvnd's EGL, and
  `__EGL_VENDOR_LIBRARY_FILENAMES` the shell's own Mesa, so that Yuna and its tests draw on that
  Mesa's llvmpipe, never on the host's drivers
  ([ADR-0123](docs/adr/0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md));
- `XKB_CONFIG_ROOT`, where the keyboard's default keymap comes from;
- the applications the tests run in Yuna, kept off the shell's own `PATH`: Alacritty, GTK 4's
  `gtk4-demo`, Qt's `qml`, `vkcube`, mpv and Wine, in `YUNA_APPLICATIONS_PATH`, with what the
  tests give them: `YUNA_XWAYLAND_PATH` (Xwayland's folder, Yuna's own `PATH` in the tests),
  `YUNA_LAVAPIPE_ICD` (Mesa's Vulkan on the CPU),
  `YUNA_FONTS_CONF` (DejaVu alone), `YUNA_QT_PLUGINS` and `YUNA_QML_IMPORTS` (for `qml`, which
  is not wrapped), and `YUNA_CLIENT_LIBRARY_PATH` (the libraries SlimeEngine's example loads);
- the sources of the owner's projects whose drivers the tests run, `YUNA_UWUI_SOURCE` and
  `YUNA_PLUTO_SOURCE`, at the revisions `flake.nix` names in their inputs' URLs.

A second shell, `nix develop .#bench`, adds what the rivals' bench runs, by their store paths, in
`YUNA_BENCH_*` variables: sway, KWin, dwl and mango (each also built without XWayland, for equal
work), Umbriel (built from its source at the revision `flake.lock` pins, the flake's `umbriel`
input), Hyprland, Mutter, wlr-randr, weston, ffmpeg, D-Bus, Xwayland, the Mesa they all draw
with, coreutils and bash. CI never fetches it.

A third, `nix develop .#parity`, is the default shell and valgrind, for the parity script; CI's
`parity` job alone fetches it.

`yuna --help` lists the variables Yuna's own code reads, `XDG_RUNTIME_DIR` and `YUNA_LOG` among
them, and its exit statuses; its contract,
[`crates/yuna/tests/contracts/help.txt`](crates/yuna/tests/contracts/help.txt), is their one
home. It leaves out `TMPDIR`, which `yuna mcp` reads as any program does, for its agents'
throwaway homes; the comment above `HELP` in
[`crates/yuna/src/main.rs`](crates/yuna/src/main.rs) names what else, and why. At
`YUNA_LOG=debug`, Yuna logs a line per frame drawn and per round of the loop with its counters;
the documentation of `main.rs` lists the lines tests and measures read.

The first `nix develop` downloads the toolchain, the shell's Mesa and the tests' applications.
[`docs/measurements.md`](docs/measurements.md) has only parts of it: the applications add
1.34 GB unpacked (M1.4, B3), and a cold check took about 13 s at M1.1, before Smithay and the
applications were in the shell. Neither the whole first download nor a cold check of today's
tree is measured.

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
nix develop -c cargo test --locked -p yuna --test headless <name>   # the stories whose name holds <name>
nix develop -c cargo fmt --all            # formats, as the check's `format` step wants
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
[ADR-0127](docs/adr/0127-motions-budget-no-frame-lost-to-motion.md) asks: it
runs Yuna in its own process with six `vkcube`, and plays a round of scrolls, column moves,
workspace switches and the special workspace's fade, against a still baseline of as many frames;
it prints, for each, the frames a second against the output's rate, the frames drawn at once and
missed, and where the misses fall; then the heaviest frame and the ADR's verdict. The verdict names
the mode it ran at, and counts as the ADR's only at 3840x2160@240,1.5 on `--gpu` with its clients on
the GPU too, `--client-vulkan` naming the GPU's Vulkan driver
([ADR-0144](docs/adr/0144-motions-budget-its-clients-draw-on-the-gpu.md)); anywhere else it is
recorded without a pass mark. It gives none when the baseline ran below 95 % of the output's
rate. Under `--reduced-motion` the round has no retarget: its load is jumps and fades. Its
documentation gives its options, `--gpu` among them:

```sh
nix develop -c cargo build --release -p yuna --example motion_time
nix develop -c target/release/examples/motion_time --output 1920x1080@240,1
# The budget's run, the GPU's Vulkan driver from the shell's Mesa:
nix develop -c bash -c 'target/release/examples/motion_time --gpu --output 3840x2160@240,1.5 \
  --client-vulkan "$(dirname "$YUNA_LAVAPIPE_ICD")/radeon_icd.x86_64.json"'
```

It runs alone among measurements, as `frame_time` does; on the owner's machine, inside
`agent-headless`, under the agents' lock of exclusive runs. The measures share
`crates/yuna/examples/common/`, whose [`mod.rs`](crates/yuna/examples/common/mod.rs) says which
file each one includes.

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

A keyboard layout's change on the state thread, its compile and key tables, is measured with
`crates/yuna/examples/layout_time.rs`, which changes an agent's seat's layout many times and
gives the median and the longest:

```sh
nix develop -c cargo run --release -p yuna --example layout_time
```

What agents' Yunas cost is measured with `crates/yuna/examples/instances.rs`
([ADR-0138](docs/adr/0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md)): one `yuna
mcp`'s start to its seat's first frame and its memory at rest; N `yuna mcp` with a seat each
against one with N seats, up to 10, each seat with Alacritty; and ten held at rest for 5 minutes,
which must grow nothing. It reads the other programs' load beside each phase, as `xwayland_time`
does. Its documentation gives its options, `--gpu` and `--phase` among them:

```sh
nix develop -c cargo build --release -p yuna --bin yuna --example instances
nix develop -c target/release/examples/instances --phase start
```

The rivals are measured with `crates/yuna-bench`
([ADR-0075](docs/adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md),
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md#the-rivals-bench); a table in
[`crates/yuna-bench/src/recipes.rs`](crates/yuna-bench/src/recipes.rs) gives how each compositor
runs). It runs sway, KWin, Yuna, dwl, mango, Umbriel, Hyprland and Mutter headless on
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
([ADR-0120](docs/adr/0120-the-mcp-server-a-private-yuna-on-stdio.md)), and `--animations` or
`--reduced-motion`, which `yuna --headless` takes too: both show each change at once without them
([ADR-0109](docs/adr/0109-animations-and-effects-what-moves-reduced-motion-and-games.md)). Both
take `--rest-audit` too: a line at `info` for each second in which something woke Yuna, naming
what did ([ADR-0096](docs/adr/0096-a-rest-auditor-naming-what-wakes-yuna-or-makes-it-draw.md)).
An agent's first call is `seat create`; every other tool but `clock` takes the seat's handle,
`rest` when it is to name only that seat's
([ADR-0117](docs/adr/0117-an-agents-seat-through-mcp-tools-input-keyboard-text-and-closing.md)). For Claude Code, for example:
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
stories, `tests/headless/mcp.rs` and `seats.rs`: their `Mcp` speaks to the server in the test's
own process, and `crate::client::Client` makes a Wayland window to click in. `nix develop` warns
that the git tree is dirty whenever a file is changed: it is harmless.
`crates/yuna/tests/consumers.rs` runs uwui's `measure.py` and Pluto's `pluto-input` against
`yuna --headless --agent-seat`, in about 5 s once `pluto-input` is built, about 10 s, in
`target/consumers/pluto-<key>`; its crates come from crates.io the first time. The key is a hash
of the Nix store paths in Pluto's source and in `PATH`, `LD_LIBRARY_PATH` and `PKG_CONFIG_PATH`,
so that a shell with other libraries builds it afresh. The folders of older keys stay: `rm -r
target/consumers` frees them. A consumer's revision is
raised in a commit of its own: the input's URL in `flake.nix`, then `nix flake lock`, then
these tests ([ADR-0126](docs/adr/0126-yuna-and-the-owners-other-projects.md)).
SlimeEngine's examples run in ignored tests, `window`, `editor` and M1.5's done-when story on
`interface`, given a folder that holds them, as SlimeEngine's weekly job runs them:

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

Opium's desktop window runs in an ignored test, `crates/yuna/tests/opium.rs`, given Opium's
`bin/opium`, as the weekly Opium job runs it
([ADR-0122](docs/adr/0122-real-applications-in-ci-and-rest-checked-on-the-program.md)): an agent,
through `yuna mcp` alone,
starts Opium on a throwaway profile, in a network namespace of its own, checks its window by
capture, clicks Songs in it, checks the new page by capture, and Yuna must rest once Opium is
idle. Opium's Nix package is built at the revision `.github/workflows/opium.yml` pins, from
Opium's own flake; the namespace is made by util-linux's `unshare`, which the shell names in
`YUNA_UNSHARE`, and needs unprivileged user namespaces.
The test takes about 20 s; Opium's build, about 15 minutes the first time:

```sh
nix build --out-link /tmp/opium github:HoneySlatt/Opium/<revision>
YUNA_OPIUM=/tmp/opium/bin/opium nix develop -c cargo test -p yuna --test opium -- --ignored
```

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

`scripts/check.sh` runs these steps, in order, and stops at the first failure; `scripts/check.sh
only <step>` runs one:

1. `rules`, the workspace's own: `rust-version` in `Cargo.toml` equals the toolchain's version,
   every crate takes the workspace's lints, and no test or example of `yuna` names Smithay
   ([ADR-0146](docs/adr/0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md));
2. `format`, `cargo fmt --check`;
3. `lints`, clippy over every target, warnings as errors, twice: the second time with the
   parity script's canaries (`--features yuna/parity-canary,…`), code no other build compiles;
4. `tests`, the build and the tests;
5. `rustdoc`, the documentation, `cargo doc`, warnings as errors;
6. `dependencies`, `cargo deny`: licences compatible with GPL-3.0-or-later; every RustSec advisory
   and every yanked crate, on any dependency, direct or not; the sources of the dependencies. It
   reads the advisory database from the network;
7. `links`, between the tracked Markdown files, offline, anchors included;
8. `lengths`, of `AGENTS.md` (1,500 words at most) and `docs/PLAN.md` (2,000), read at every
   session;
9. `indexes`: the ADRs' index written from the ADRs, and the changelog's fragments' form;
10. `script-tests`: the tests of those scripts, and of the path filters of SlimeEngine's and
    Opium's jobs.

`scripts/test-check.sh` proves that each step stops what it guards against
([ADR-0126](docs/adr/0126-yuna-and-the-owners-other-projects.md)): it clones the last
commit, breaks the clone one way at a time (a wrong `rust-version`, code not formatted, a
warning, one in the canaries' code, an `unsafe` block without its comment, a failing test, a contract or a protocol changed
without its file, a broken link, a licence refused, `AGENTS.md` too long, an ADR off the index, a
fragment misnamed, SlimeEngine's or Opium's job leaving out the drawing path), and expects the step that
guards against it to fail, for that reason. Run it after changing `check.sh` or a check, and
before a release; it takes about as long as `check.sh` twice, building in `target/test-check`.

Every cargo command of the check passes `--locked`: `Cargo.lock` changes only on purpose.

A new crate goes in `crates/` and its `Cargo.toml` holds the two lines `[lints]` and `workspace = true`, so
that the workspace's lints (the `// SAFETY:` comment, documentation) reach it; the check refuses it
otherwise.

### The parity script

`yuna-bench parity run` compares two builds of Yuna on figures the machine's load does not move
([ADR-0142](docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md),
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md#the-parity-script)). CI runs it at each push; run
it before pushing a step of Stage 2a, in the parity shell:

```sh
nix develop .#parity -c cargo run --release -p yuna-bench -- parity run --head-path . --before HEAD
nix develop .#parity -c cargo run --release -p yuna-bench -- parity run --head HEAD --before origin/main
```

The base is the baseline the head's `parity-baseline.toml` pins unless `--base <revision>` or
`--base-path <folder>` names another; the head is `HEAD` unless `--head <revision>` or
`--head-path <folder>` (a tree with uncommitted changes, built as it is) does. `--before
<revision>` names the push before: the revision the head's work starts from, `origin/main` for
commits not pushed yet, `HEAD` for a tree's uncommitted changes. It tells the script which moves
of the baseline are the head's own (below). It is needed against the pinned baseline, whose own
file cannot hold the move to itself; against a base `--base` names, without it no move is the
head's own. `--scenario` picks scenarios (`idle`, `partial`, `full`, `motion`,
`scaled`, `many`, `input`, and `operations` for the layout's and motion's), `--repeat <n>`
measures each figure `n` times on each side, `--jobs` runs that many at once (by default the
processors, 4 at most), `--seed` replays an order. It writes `target/parity/parity-<time>.json`
and its report, `.md`, beside it (`--out` for another file). Its work folder holds the trees,
their builds and each run's logs: by default `target/parity/work-<pid>-<time>`, removed at the
end unless a run failed, the exit status is 2 (a figure void and none worse, or an error), or
`--keep`; one `--work` names is never removed, and the trees it holds are used again only at the
same revision with no change. The revisions' worktrees are removed in every case, an error or a
panic included. The work folder cannot be under `/tmp`: each run has a `/tmp` of its own. Each
run is a user namespace, made by util-linux's `unshare` (the shell's `YUNA_UNSHARE`), which
Ubuntu 24.04 refuses until `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`. It
runs nothing on a display or a GPU.

It exits 0 when no figure is worse, 1 when one is, 2 when one is void or the run failed: the
report says which and why, its worse and void figures first. A worse figure also gets the push
before compared with the head, on its scenario, and the report says whether this push made it
worse or an earlier one did; that never changes the exit status. CI passes a push's
`github.event.before`, a pull request's base (`HEAD^1`, the first parent of the merge commit it
checks out), and for the schedule the head itself. A push whose
push before is not in the history, as after a force push, fails the job: run it by hand then,
with `--before` the commit it follows.

The figures' classes and thresholds are in
[`crates/yuna-bench/parity-thresholds.toml`](crates/yuna-bench/parity-thresholds.toml), each
threshold with the spread it came from. The head's file judges; a class the head changes, or a
threshold or budget it makes looser, against the base's makes its figure void, unless a new
move of the baseline names that figure. Measure a spread with `--base-path . --head-path .
--repeat 5`, and set the threshold at about three times it, with a floor.

A figure the head makes worse on purpose (Vulkan in place of GLES, a bound that costs memory)
moves the baseline. A commit cannot name itself, so the new baseline is an earlier commit that
holds the change, with the classes and thresholds it needs; the move is a later commit that
adds a `[[move]]` at the end of
[`crates/yuna-bench/parity-baseline.toml`](crates/yuna-bench/parity-baseline.toml) and sets its
`commit` to the new baseline, naming the figures moved as the report names them (or a kind of
figure alone, for every scenario), why, and the ADR or the owner's go that allows it. Push the
change and its move together: the change alone is judged against the old baseline. Nothing else
moves it. At the push that adds the move, CI also runs the head against the old baseline, the
one the push before pinned: every figure worse there, or whose class, threshold or budget the
head loosened against the old baseline's, must be named, else the run is void. Later pushes are
judged against the new baseline, and may not loosen its classes or thresholds: a threshold
loosened in the move's commit rather than the new baseline's would void every later push. A
move once pushed is never changed or dropped, and a new move may not pin the commit the move
before it pinned: the script refuses both. The first `[[move]]`, the first baseline, has no old
one to check against: `37d925a` set it by rewriting the placeholder move that the parity
script's own commit, `500e804`, held, a rewrite the script now refuses.

The move check runs at the push that adds the move, and there alone. One that fails, or that a
later push cancels before it ends, is never redone by CI: every later push's push before holds
the move. Run it again by hand, with `--before` the commit before the move, and push nothing on
top of a red or cancelled move check. No later push can correct a move; if the check fails by
hand too, the owner decides how to repair it.

The controls prove that the script sees what it watches. The tree against itself, the *self
control*, must give 0:

```sh
nix develop .#parity -c cargo run --release -p yuna-bench -- parity run --base-path . --head-path .
```

and each canary, built into the head, must give 1, flagging its figure:

```sh
nix develop .#parity -c cargo run --release -p yuna-bench -- parity run --base-path . --head-path . --canary spin
```

The canaries are `spin` (Yuna's own instructions each frame), `spin-total` (the same, about
twice the threshold of the total's instructions), `allocate` (an allocation each frame), `leak`
(64 bytes kept every 16 frames, 4 bytes a frame), `memory` (1 MiB kept), `wake` (a timer each
second), `frame-wake` (a wake and a context switch more each frame), `round` (instructions each
round of the event loop), `layout` (work in `Layout::focus`) and `motion` (work in an animated
`Motion::update`). They are the `parity-canary` features of
`yuna`, `yuna-layout` and `yuna-motion`, off in every shipped build; `YUNA_PARITY_CANARY` turns
Yuna's on, read once at its start.

## Continuous integration

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs `scripts/check.sh` in the same shell, on
each push to `main`, each pull request, and every Monday. The checks stay one job while one is
enough ([ADR-0114](docs/adr/0114-gpl-3-0-or-later-public-the-logo-apart.md)); beside them, the
`package` job builds Yuna's package, and the `parity` job runs
[the parity script](#the-parity-script), HEAD against the pinned baseline, with the whole history
checked out. A change of Markdown files alone skips them:
[`.github/workflows/docs.yml`](.github/workflows/docs.yml) runs `scripts/check.sh docs`: their
links, the length of those read at every session, the ADRs' index, the changelog's fragments and
the tests of the scripts that check them, with lychee, git and python3 alone, in a minute or two. `scripts/check.sh docs` runs the same locally.
[`.github/workflows/slimeengine.yml`](.github/workflows/slimeengine.yml) runs SlimeEngine's
`window`, `editor` and `interface` examples in Yuna every Thursday, on demand, and on a push that
changes anything its test builds or reads: it names what it leaves out, and
`scripts/test_workflows.py` checks that nothing it needs is among them; it builds the examples and cooks their
data once per pinned revision and keeps the folder in GitHub's cache
([ADR-0122](docs/adr/0122-real-applications-in-ci-and-rest-checked-on-the-program.md)). Its
times are in [`docs/measurements.md`](docs/measurements.md).
[`.github/workflows/opium.yml`](.github/workflows/opium.yml) runs Opium's desktop window in Yuna
every Saturday, on demand, and on a push that changes anything its test builds or reads, its
path filter checked the same way
([ADR-0122](docs/adr/0122-real-applications-in-ci-and-rest-checked-on-the-program.md)). It builds
Opium's Nix package at the revision it
pins, uncached, and turns off the AppArmor rule of GitHub's Ubuntu 24.04 that keeps an
unprivileged user from the network namespace the test runs Opium in. Its times go to
[`docs/measurements.md`](docs/measurements.md) once it has run on GitHub's runners; none is
recorded yet.

## Dependencies

A new dependency is asked of the owner first, with its reason, an alternative weighed and its
licence. Why each crate is there, and which of its features, is the comment beside it in the
manifests, the root `Cargo.toml`'s `[workspace.dependencies]` and each crate's own: their one
home. The owner's decisions:

- 2026-11-11: proptest, for the layout model's tests, and the compositor's from 2026-11-12
  ([ADR-0122](docs/adr/0122-real-applications-in-ci-and-rest-checked-on-the-program.md)); for the
  HDR prototype's branch alone (M1.3, deleted since), Smithay's `backend_drm`, `backend_gbm`,
  `backend_egl`, `renderer_gl`, `backend_session_libseat` and `backend_libinput` features with the
  system libraries they link.
- 2026-11-12, for the headless compositor (M1.4): Smithay's `wayland_frontend`, `desktop` and
  `renderer_gl` features; `tracing`, and `tracing-subscriber` with its `fmt` and `std` features
  alone; `wayland-client` and the client side of `wayland-protocols`, for tests only; in the
  shell, Mesa, libglvnd, libxkbcommon, xkeyboard-config, a font, and the applications the tests
  run (ADR-0122); the action `actions/cache`, for SlimeEngine's binary.
- 2026-11-13, for the MCP server (M1.5): `serde`, with its `derive` feature, and `serde_json`
  ([ADR-0120](docs/adr/0120-the-mcp-server-a-private-yuna-on-stdio.md)); `png` for captures
  ([ADR-0121](docs/adr/0121-captures-what-is-seen-at-the-logical-size.md)).
- 2026-11-14: `x11rb`, with only the extensions the X11 window manager uses, and for the tests' X11
  clients ([ADR-0066](docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md)).
- `wayland-protocols-misc` and `wayland-protocols-wlr`, already in `Cargo.lock` through Smithay,
  under MIT: the client side of the virtual keyboard, for the rivals' bench (M1.9) and the tests'
  drivers, and of the virtual pointer and `wlr-screencopy`, for the tests (M1.10;
  [ADR-0138](docs/adr/0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md),
  [ADR-0121](docs/adr/0121-captures-what-is-seen-at-the-logical-size.md)).
- 2026-11-18: `zbus`, its D-Bus wire format alone, without its default features, for the
  accessibility buses
  ([ADR-0094](docs/adr/0094-the-accessibility-tree-yuna-as-the-registry-of-each-seats-bus.md));
  `rustix`, named directly, the crate Smithay re-exports and `Cargo.lock` held already.
- 2026-11-19: util-linux in the shell, for the Opium test's `unshare` (ADR-0122).
- 2026-11-20 (M2.2): `wayland-server`, `calloop`, `xkbcommon`, `drm-fourcc`, and the server side of
  `wayland-protocols` and `wayland-protocols-wlr`, named directly in place of Smithay's re-exports,
  the crates `Cargo.lock` held already, under MIT
  ([ADR-0139](docs/adr/0139-stage-2-in-three-gated-parts-2as-path-revised.md), which needs no new
  ask for them).

Smithay is Yuna's public fork, read over HTTPS and pinned to one commit
([ADR-0112](docs/adr/0112-smithay-pinned-patched-in-a-public-fork-left-at-stage-2.md)); its
patches are listed in [`docs/smithay-patches.md`](docs/smithay-patches.md).
Raising the toolchain, Smithay, the other crates or `flake.lock` happens at a stage's end, each in a
commit of its own. `cargo update` takes a crate's new version only once it has been published for
three days, as `.cargo/config.toml` says ([ADR-0103](docs/adr/0103-crates-updates-wait-three-days.md)).

## Contracts

What other programs and projects rely on
([ADR-0079](docs/adr/0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)):

- the command line: `yuna --headless`, `yuna mcp`, their options, the `WAYLAND_DISPLAY=<socket>`
  line printed at start, the environment Yuna reads and its exit statuses, all as `yuna --help`
  gives them;
- the environment Yuna gives its programs, as
  [`crates/yuna/src/mcp/programs/environment.rs`](crates/yuna/src/mcp/programs/environment.rs)
  lists it: `YUNA_SEAT`, their seat's throwaway home
  ([ADR-0105](docs/adr/0105-an-agents-programs-on-a-throwaway-home.md)), and the accessibility
  variables;
- the MCP server: its revisions, its tools' names, arguments and results; `tools/list` holds the
  names, the arguments' schemas, and the results in each tool's description, so that a new field
  of a result is written there, and the contract file rewritten;
- the Nix package's outputs: `packages.x86_64-linux.yuna`, also `default`, whose `bin/yuna`
  draws with its own Mesa and finds Xwayland and dbus-daemon
  ([ADR-0126](docs/adr/0126-yuna-and-the-owners-other-projects.md));
- Hollow's interface, once it exists (Stage 2), under semantic versions in its own crate,
  `yuna-ipc` ([ADR-0082](docs/adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)).

Until 1.0 a contract may change, never silently: a change that breaks a consumer adds a
`changed-…` or `removed-…` fragment to [`changelog.d/`](changelog.d/README.md) that says what to do
instead; an addition, an `added-…` one. The logs' text and the library's API are not contracts.

[`crates/yuna/tests/contracts.rs`](crates/yuna/tests/contracts.rs) holds `yuna --help` and the MCP
server's `tools/list` as files in `crates/yuna/tests/contracts/`: a change to either fails it until
the file is rewritten, `YUNA_WRITE_CONTRACTS=1 cargo test -p yuna --test contracts`, and the
rewrite shows in the diff beside its fragment.

Stage 2a's parity net holds files the same way, though they are not contracts
([ADR-0139](docs/adr/0139-stage-2-in-three-gated-parts-2as-path-revised.md)).
`crates/yuna/tests/headless/captures_held.rs` compares captures of eight scenes with the PNGs in
`crates/yuna/tests/captures/`: a change to what Yuna draws fails it, saying for each class of pixel
how many changed and by how much, and writing the image captured and a difference image to a
temporary folder it names. A change made on purpose, a Mesa update or a blending an ADR decided,
is looked at in those images, then rewritten with `YUNA_WRITE_CAPTURES=1 cargo test -p yuna --test
headless captures_held`, the new PNGs in the diff. The protocol traces in
`crates/yuna/tests/traces/` work the same way: a change of protocol behaviour fails
`cargo test -p yuna --test headless traces::` with a diff whose hunks name their client and
object, and a change made on purpose is rewritten with `YUNA_WRITE_TRACES=1` and read in the diff.
A trace that varies between runs fails the five-times test: either its scenario must wait for an
answer it does not wait for yet, or `traces/normalise.rs` needs a table entry, citing the
protocol's XML, for the value that changes. During Stage 2a a step that fails either is reverted,
not rewritten.

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
