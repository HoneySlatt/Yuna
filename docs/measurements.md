# Measurements

What the benchmarks and the measurements by hand found, against the budgets of the quality bar in
[`ROADMAP.md`](ROADMAP.md#3-quality-bar). One section per milestone or machine, newest first. A
section names the machine, the build, the command, and the figures with their spread.

## M1.4, Phase B4 — A frame's cost, rest at 4K, the layout again (2026-11-12)

Machine: the high tier's desktop, Ryzen 7 5800X (16 threads), RX 6900 XT, Linux 7.3.1; Mesa
26.2.6 from the development shell, for Yuna and its clients. One measurement at a time, each
series started after 30 s with no build, game or other measure and under 1.5 cores busy; the
load averages below rise from the series' own runs. Everything ran through `agent-headless`;
`--gpu` drew on the render node, never on a display device. Build: release, from the sources
of commit `29f9f54`, built before it was made, with `frame_time` as it was before its fix to
stop its clients at its end; debug for rest, as the test runs. The scripts that ran each series
and their outputs are kept locally, in `.delegation/m1.4-b4-measures/`; the layout's runs A and
B and the rest run were started by hand, with the commands given here.

### A frame on the 4K output

Command: `nix develop -c cargo build --release -p yuna --bin yuna --example frame_time`, then
`nix develop -c agent-headless -- target/release/examples/frame_time` with the options below
(`--gpu`; `--client-vulkan` with the shell's Mesa's `radeon_icd.x86_64.json`):
Yuna's default output, 3840 × 2160 at 240 Hz and scale 1.5; two `vkcube` windows, side by side
in columns of half the width, so that the whole output changes at each frame; 10 s measured
after 3 s of warm-up; three runs of each row, the lowest and highest given. Load average at the
series' start 0.9, then up to 8.7.

What the figures are ([ADR-0030](adr/0030-a-frames-gpu-time-from-the-kernel.md)):

- **A frame's time** is Yuna's own: from when it starts to build the frame, its clients' new
  buffers imported (a `wl_shm` buffer copied into a texture), to when the drawing is done, as
  logged in `frame drawn` (`took_us`). Its presentation follows from it
  ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)).
- **CPU time** is the time each of Yuna's threads ran on a core, read in
  `/proc/<pid>/task/<tid>/schedstat` before and after, divided by the frames drawn. On llvmpipe,
  its 16 `llvmpipe-N` threads rasterize: their time is the work a GPU would do. On the GPU,
  `yuna:cs` and `yuna:gdrv` are radeonsi's threads, which submit Yuna's commands.
- **GPU time** is the busy time of the GPU's engines for Yuna's DRM client, from the kernel's
  `drm-engine-<engine>` counters in `/proc/<pid>/fdinfo`, divided by the frames drawn.

| Yuna on | Clients' buffers | Frames a second | A frame's time: median | p99 | longest | Yuna's CPU per frame | GPU per frame |
|---|---|---|---|---|---|---|---|
| llvmpipe | `wl_shm`, from lavapipe | 115.4–117.4 | 6,851–7,225 µs | 8,572–9,108 µs | 9.3–10.7 ms | 2.3–2.4 ms, the main thread | 72.0–73.9 ms of llvmpipe's threads, on the CPU |
| the GPU, `--gpu` | `wl_shm`, from lavapipe | 240.0 | 1,375–1,393 µs | 1,648–1,853 µs | 2.3–3.7 ms | 1,234–1,254 µs: the main thread 1,126–1,144 µs | 770–775 µs (gfx) |
| the GPU, `--gpu` | dmabufs, from radv | 239.9–240.0 | 198–207 µs | 235–247 µs | 459–503 µs | 162–178 µs: the main thread 140–153 µs | 117 µs (gfx) |

- On llvmpipe, every frame takes longer than the 4.17 ms period: each is presented a boundary
  late, and Yuna draws about 116 frames a second for 240. Yuna then keeps 8.7 to 8.8 of the 16
  threads busy, almost all of it llvmpipe's, and its two clients 3.4 more (28.5 to 29.4 ms of CPU per frame). The bar's
  budget for an agent's output, full rate while its programs draw, is missed there
  ([debt](debt.md)).
- On the GPU, no frame took longer than the period. With `wl_shm` clients, each frame copies
  both clients' buffers into textures; with dmabufs nothing is copied, and a frame costs 0.17 ms
  of Yuna's CPU and 0.12 ms of the GPU. The difference, about 1.1 ms of CPU and 0.65 ms of GPU,
  is most likely the copy: not measured apart. The `vkcube` clients
  themselves took 171 to 184 µs of CPU and 87 to 88 µs of GPU per frame on radv, 24.8 to 25.2 ms
  of CPU on lavapipe.
- No budget of the bar names a frame's cost yet: M1.9 sets it from the rivals'.

`--gpu`, run for the first time, works: Yuna announces `zwp_linux_dmabuf_v1` version 6 with its
default feedback. A `vkcube` on radv, with `WAYLAND_DEBUG=1`, binds it, asks the default
feedback, gets its main device, its format table and one tranche, then creates its buffers with
`create_params`; Yuna imports and draws them, and holds 16 dma-bufs for the two clients. With
lavapipe's clients, it holds none, only their `wl_shm` memfds.

At scale 1 (`--output 3840x2160@240,1`), on llvmpipe, one run at a load average of 5.3: 65.8
frames a second; a frame 12.1 ms at the median, 17.8 ms at the 99th percentile; Yuna's CPU 92.6
ms per frame, the main thread 5.5 ms and llvmpipe's threads 87.1 ms; the clients' 68.6
ms. Scale 1.5 is lighter most likely because `vkcube` follows no fractional scale: it draws at its
logical size, and Yuna scales it up. At scale 1 its buffers have 2.25 times the pixels, and the
clients' CPU and Yuna's main thread, which copies them, take about 2.4 times as long. Not
verified apart.

### Rest on the 4K output

`YUNA_APPLICATIONS_OUTPUT=3840x2160@240,1.5 nix develop -c agent-headless -- cargo test --locked
-p yuna --test applications -- --nocapture`, one run, Steam open on the desktop with no game:
Alacritty, `gtk4-demo` and `qml` with their windows open, and `vkcube` after it drew 120 frames
and left, each leave Yuna with 0 context switches in its 34 or 35 threads over 10 s, and no
frame, frame callback, configure, commit or wake-up, as at 1920 × 1080 and 60 Hz in B3.
`vkcube` left to draw fails the check: 1,408 frames in 10 s, its window half of the output.

### The layout again, on a quiet machine

The command of M1.2 below: `taskset -c 6 target/release/examples/timing`, three runs, load
average 0.6 to 0.8, nothing else running. Medians and 99th percentiles in microseconds, the
lowest and highest of the three runs:

| Operation, then `arrange` | Windows | Scrolling: median | p99 | Dwindle: median | p99 |
|---|---|---|---|---|---|
| Arrange alone | 100 | 6.3 | 6.5–8.8 | 6.7–9.7 | 6.8–13.0 |
| Focus left, then right | 100 | 9.2–9.3 | 9.4 | 9.6–9.8 | 12.0–15.0 |
| Open a window, then close it | 100 | 10.9–11.0 | 14.2–15.3 | 13.4–13.6 | 15.5–17.8 |
| Move the focused window, then back | 100 | 9.2–9.3 | 9.4–12.4 | 10.1–10.2 | 13.1–13.8 |
| Fullscreen on, then off | 100 | 10.8–10.9 | 14.0–17.9 | 6.9–7.0 | 7.5–10.2 |
| Float, then tile | 100 | 15.7–24.2 | 15.9–32.1 | 18.3–18.7 | 21.5–27.6 |
| Switch the tiling, then back | 100 | 10.8–17.2 | 11.6–21.1 | 8.6–8.7 | 8.9–13.8 |
| Arrange alone | 1,000 | 65.1–66.1 | 71.7–112 | 63.8–65.2 | 69.3–71.4 |
| Focus left, then right | 1,000 | 93.9–154 | 102–169 | 92.2–94.2 | 100–124 |
| Open a window, then close it | 1,000 | 111–112 | 118–142 | 126–127 | 143–154 |
| Move the focused window, then back | 1,000 | 93.9–95.5 | 101–162 | 93.5–95.2 | 103–106 |
| Fullscreen on, then off | 1,000 | 110–111 | 119–184 | 65.9–67.6 | 72.8–88.8 |
| Float, then tile | 1,000 | 159–161 | 169–251 | 175–178 | 186–212 |
| Switch the tiling, then back | 1,000 | 108–109 | 118–121 | 83.3 | 90.6–97.8 |

Against [ADR-0020](adr/0020-the-layout-models-shape.md)'s budgets at the 99th percentile,
25 µs at 100 windows and 300 µs at 1,000: runs 2 and 3 meet them on every row. Run 1 misses
"float, then tile" at 100 windows, 32.1 µs in scrolling columns and 27.6 µs in dwindle, and is
slower on other rows too, for a cause not found. Two more runs, right after a frame measure on
llvmpipe, at a load average of 7 to 8 and with every median about 15 % higher, missed that row
again, 28.2 µs in scrolling columns, 30.5 and 31.1 µs in dwindle, and no other. That row's
median, 15.7 to 24.2 µs, leaves too little room under its budget: it stays debt
([debt](debt.md)). At 1,000 windows every run of every row meets its budget; the worst, 270 µs.
In runs 2 and 3, the medians at 100 windows are within a few percent of Phase A's below.

## M1.4, Phase B3 — Real applications in CI (2026-11-12)

Machine: the high tier's desktop, Ryzen 7 5800X (16 threads), not alone on the machine; one
measurement at a time. Build: debug for the tests, release for SlimeEngine's example. Each
application in its own `yuna --headless` at 1920 × 1080, 60 Hz, scale 1, on llvmpipe, through
`agent-headless`.

| Application | Shown after | Before going quiet | Then, in 10 s |
|---|---|---|---|
| Alacritty 0.17 | 0.4 s | 3 frames, 2 callbacks, 2 configures, 3 commits | 0 context switches in Yuna's 34 threads; no frame, callback, configure, commit or wake-up |
| `gtk4-demo` (GTK 4.24, Vulkan on lavapipe) | 1.1 s | 2 frames, 1 callback, 2 configures, 3 commits | the same, 35 threads |
| `qml` (Qt 6.12, EGL on llvmpipe), still file | 0.4 s | 3 frames, 2 callbacks, 2 configures, 3 commits | the same, 34 threads |
| `vkcube` 1.4.363, 120 frames, then leaves | | 121 commits, 120 frames, 119 callbacks | the same, 35 threads, after it left |
| SlimeEngine's `window` (`dc4ceaa`), 120 frames, then leaves | | 121 commits, 121 frames, 119 callbacks | the same, 35 threads, after it left |
| `vkcube` left to draw (the proof) | | | 600 frames, 600 callbacks, 600 commits, 1,200 wake-ups, 33,770 context switches: the check fails |

All five draw through `wl_shm`; none uses `linux-dmabuf`.

| What | Runs | Measured |
|---|---|---|
| `cargo test --test applications`, its two tests at once | 5 | 15.3 s each |
| The same, on two cores (`taskset -c 0,1`), within `scripts/check.sh` | 2 | 15.7 and 16.1 s |
| `scripts/check.sh`, warm, on two cores | 1 | 1 min 11 s |
| The EGL race test before its fix, on two cores, 100 rounds a run | 20 | failed 9 times, hung once |
| The same, on sixteen threads | 20 | never failed |
| The same after the fix, on two cores | 27 | never failed |
| SlimeEngine's `window` example, release, from nothing, in Yuna's shell | 1 | 58 s; a 26 MB binary |
| What the applications add to the shell | 1 | 403 store paths, 1.34 GB unpacked |

On GitHub's `ubuntu-24.04` runner, for commit `9ea2271`: CI's one job took 2 min 41 s (run
43243317324), the applications' test 15.6 s of it, against 3 min 23 s at B2, for a cause not
measured: one run each. SlimeEngine's workflow took 4 min 11 s when it built the
example (run 43243317407; the build 2 min 43 s) and 1 min 38 s with the binary from its cache
(run 43244291340). Yuna has 11 threads there, against 34 or 35 on the 16-thread desktop.

## M1.4, Phase B2 — The protocols and the robustness tests (2026-11-12)

Machine: the high tier's desktop, Ryzen 7 5800X (16 threads), load average 6 to 9 from other
work; one measurement at a time. Build: debug, at the commit that adds this section.

| What | Runs | Measured |
|---|---|---|
| The two random-request tests, 80 cases each | 1 | 5.8 s |
| The compositor's in-process tests (89), `cargo test --test headless` | 3 | 8.8 to 9.3 s |
| The same, before the random tests were split in two | 3 | 16.1 to 17.4 s |

`yuna --headless` (release) with Alacritty 0.17 inside, through `agent-headless`, on llvmpipe:
at 1920 × 1080 at 60 Hz, scale 1, and at 2880 × 1620 at 60 Hz, scale 1.5, 3 frames drawn,
then 0 context switches in 10 s for every thread of Yuna and of Alacritty. Alacritty draws
through `wl_shm` and takes the fractional scale (120, then 180) and a viewport.

How often the random requests that need the keyboard's focus took effect, over 1,000 cases,
500 of each start: grabs granted 592 of 785 (75 %), tokens asked validly 631 of 3,650 (17 %),
activations that moved the focus 104 of 798 (13 %), selections taken 585 of 2,755 (21 %). From
busy starts alone: 75 %, 31 %, 20 % and 37 %; from the sentinel alone, where a random client
rarely has the keyboard before it breaks the protocol: 80 %, 4 %, 1 % and 5 %.

CI's one job on GitHub's `ubuntu-24.04` runner, without a cache: 3 min 23 s for commit
`0b398f0` (run 43192826739), against 2 min 47 s at B1; the B2 tests and their proptests run
in it.

## M1.4, Phase B1 — CI with Smithay (2026-11-12)

CI's one job on GitHub's `ubuntu-24.04` runner, without a cache, from its start to its end:
2 min 47 s for commit `2850e0b`, the first that builds Smithay's `wayland_frontend`,
`desktop` and `renderer_gl` and runs the compositor's 20 in-process stories on llvmpipe; 1 min
35 s to 1 min 51 s for the commits before it. The applications of B3 were not in the shell yet.

## M1.4, Phase A — The layout model with real clients (2026-11-12)

The same measure as M1.2's below, after the model learned clients' limits and committed sizes
([ADR-0026](adr/0026-real-clients-in-the-layout.md)) and dwindle stopped recursing.

Machine: the high tier's desktop, with the owner's consent, 18 minutes after a restart. Not idle:
another agent's `cargo check` of SlimeEngine ran on other cores during the runs, which spreads the
99th percentiles; the medians agree to a few percent across runs. Build: the release profile, Rust
1.99.1, commit `447d014`. Command: `nix develop -c cargo build --release -p yuna-layout --example
timing`, then `taskset -c 6 target/release/examples/timing`, three runs.

Medians and 99th percentiles in microseconds, the lowest and highest of the three runs:

| Operation, then `arrange` | Windows | Scrolling: median | p99 | Dwindle: median | p99 |
|---|---|---|---|---|---|
| Arrange alone | 100 | 6.4–6.7 | 9.7–18.2 | 6.7–6.9 | 9.4–13.0 |
| Focus left, then right | 100 | 9.4–9.7 | 13.4–23.6 | 9.7–9.9 | 13.4–17.7 |
| Open a window, then close it | 100 | 11.0–11.3 | 16.2–40.4 | 13.4–13.6 | 17.5–45.3 |
| Move the focused window, then back | 100 | 9.3–9.7 | 13.2–20.7 | 10.2–10.4 | 14.0–41.5 |
| Fullscreen on, then off | 100 | 10.9–11.2 | 15.7–23.3 | 7.0–7.2 | 7.6–10.6 |
| Float, then tile | 100 | 15.8–16.6 | 20.6–49.6 | 18.6–18.8 | 22.9–55.0 |
| Switch the tiling, then back | 100 | 10.9–11.1 | 14.2–19.4 | 8.6–8.8 | 12.6–15.2 |
| Arrange alone | 1,000 | 65.6–68.3 | 134–178 | 64.6–67.3 | 123–135 |
| Focus left, then right | 1,000 | 95.1–97.0 | 176–202 | 93.2–95.7 | 159–171 |
| Open a window, then close it | 1,000 | 111–115 | 202–299 | 128–130 | 209–335 |
| Move the focused window, then back | 1,000 | 94.6–97.3 | 185–284 | 94.2–96.7 | 165–179 |
| Fullscreen on, then off | 1,000 | 111–114 | 225–460 | 67.3–69.2 | 108–127 |
| Float, then tile | 1,000 | 160–167 | 295–322 | 178–181 | 282–304 |
| Switch the tiling, then back | 1,000 | 110–111 | 178–316 | 84.3–86.3 | 146–312 |

Against [ADR-0020](adr/0020-the-layout-models-shape.md)'s budgets, 25 µs at 100 windows and
300 µs at 1,000, at the 99th percentile: the best run of every row meets them; the worst runs miss
them on several rows, by up to 55 µs at 100 windows and 460 µs at 1,000. Whether the misses are
the other build's or the model's is not known until a quiet machine measures again. The medians
rose from M1.2's: about 1.7 times on scrolling columns at 1,000 windows, where the view now reads
every column's drawn width; dwindle's barely.

## M1.3 — HDR through Smithay, the owner's run (2026-11-12)

A verdict by the owner's eye and the monitor's own indicator; nothing was timed. Moved here from
the PLAN when M1.4 closed.

The owner ran the prototype from a text console under KDE, once KWin's colour settings were off:
KWin had left a `GAMMA_LUT`, which the prototype refuses. It set `max bpc` 10, `Colorspace`
`BT2020_RGB` and `HDR_OUTPUT_METADATA` (each tested with a modeset), scanned out `Xbgr2101010` at
3840 × 2160 and 240 Hz, and gave all three back at exit. The monitor switched to HDR. Pattern 1's
steps rose in order from a true black; above the panel's peak, pattern 2's squares kept rising up
to 4,000 cd/m², so the monitor compresses highlights rather than clips them (Stage 5's tone
mapping must know); pattern 3 showed from 0.005 cd/m²; BT.2020's primaries were more saturated
than BT.709's. A change of `Colorspace` is a modeset: the screen goes black about a second while
amdgpu trains the DisplayPort link again, its first DPCD writes failing. Not observed: whether
the monitor reads `Colorspace`, pattern 5's bands, the depth the monitor shows. The program and
its branch are deleted, its run steps with them; the patch stays in the fork for Stage 5
([patches](smithay-patches.md)).

## M1.2 — The layout model (2026-11-11)

These figures describe the model before M1.4 gave it real clients' sizes (see above).

What one operation of the layout model costs, followed by `arrange`, which a compositor calls to
draw the result: what a key press costs the model. No budget of the bar names it; it is measured
against the frame at 240 Hz, 4.17 ms on the high tier.

Machine: the high tier's desktop, Ryzen 7 5800X, with the owner's consent. The program ran pinned
to one core. The machine was not idle: two builds and an audio test of other projects ran on other
cores, at low priority for the builds, with a load average of 11 to 12, which raised some figures:
`arrange` alone on 1,000 windows in dwindle measured 44 µs at a load of 3 on the same path before
the fixes, 59 µs here. The second round of review's fixes came after, on paths these rows do not
time but for a check of a few steps before fullscreen and maximise. Build: the release profile, Rust 1.99.1,
the code of the commit that adds this section. Command:
`nix develop -c cargo build --release -p yuna-layout --example timing`, then
`taskset -c 6 target/release/examples/timing`; three runs of 5,000 samples per row, on the 4K
output at scale 1.5. Each operation goes one way, then back, and the program checks first that
it changes what is shown or focused. In dwindle, where each window halves the one before, only the
first dozen windows have a size; the third one is focused.

Medians and 99th percentiles in microseconds, the lowest and highest of the three runs:

| Operation, then `arrange` | Windows | Scrolling: median | p99 | Dwindle: median | p99 |
|---|---|---|---|---|---|
| `arrange` alone | 100 | 4.1–6.7 | 6.4–7.1 | 5.0–5.2 | 6.7–7.1 |
| Focus left, then right | 100 | 4.9–7.8 | 8.0–8.4 | 8.5–8.8 | 11.3–12.3 |
| Open a window, then close it | 100 | 4.8–8.2 | 8.2–8.7 | 14.3–14.7 | 19.5–20.4 |
| Move the focused window, then back | 100 | 4.7–4.8 | 7.7–8.3 | 8.6–8.8 | 11.5–11.9 |
| Fullscreen on, then off | 100 | 4.9–5.0 | 7.8–8.4 | 5.2–5.3 | 6.7–7.3 |
| Float, then tile | 100 | 7.2–7.5 | 12.1–12.7 | 17.3–17.7 | 23.6–24.2 |
| Switch the tiling, then back | 100 | 8.9–9.2 | 15.0–16.0 | 9.0–9.2 | 14.0–15.0 |
| `arrange` alone | 1,000 | 39–40 | 61–66 | 59–60 | 80–84 |
| Focus left, then right | 1,000 | 44–51 | 69–80 | 92–95 | 118–123 |
| Open a window, then close it | 1,000 | 45–50 | 73–80 | 147–148 | 194–208 |
| Move the focused window, then back | 1,000 | 44–49 | 71–79 | 93–96 | 119–129 |
| Fullscreen on, then off | 1,000 | 46–48 | 74–82 | 58 | 76–79 |
| Float, then tile | 1,000 | 67–68 | 102–109 | 196–197 | 266–283 |
| Switch the tiling, then back | 1,000 | 87–95 | 135–152 | 88–99 | 146–153 |

At 100 windows, every operation stays under 25 µs at the 99th percentile, about half a percent of
a frame at 240 Hz. Ten times the windows cost about ten times the time: the model is linear in the
seat's windows. The first measures were not: `arrange` cost 490 µs at 1,000 windows and switching
the tiling 2.4 ms, from searches repeated for every window; they were fixed. A first set of
figures was withdrawn after the closing review found that some of its rows timed operations that
changed nothing.

## M1.1 — The workspace and its checks (2026-11-11)

No budget of the bar applies yet; these figures are the base later milestones compare against.

Machine: the high tier's desktop, Ryzen 7 5800X (16 threads), the repository on a local NVMe drive.
Build: the debug profile at commit `2f52384`, with Rust 1.99.1, cargo-deny 0.20.2 and lychee 0.24.2.
Command: `nix develop -c scripts/check.sh`, timed from
outside, the development shell already built and the crates already downloaded.

| What | Runs | Measured |
|---|---|---|
| The check, cold: `target/` emptied by `cargo clean` | 4 | 11.1 to 15.3 s, median 13.1 s |
| The check, warm: nothing changed | 5 | 1.1 to 1.3 s, median 1.2 s |

CI, on GitHub's `ubuntu-24.04` runner, without a cache: run 43084961577 of commit `2f52384`, the
first, a single run.

| What | Measured |
|---|---|
| The job, from start to end | 82 s, of which 5 s to install Nix and 72 s for the check |
| Minutes counted | 2, by GitHub's rule that rounds each job up to the minute |
