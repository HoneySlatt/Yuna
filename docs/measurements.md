# Measurements

What the benchmarks and the measurements by hand found, against the budgets of the quality bar in
[`ROADMAP.md`](ROADMAP.md#3-quality-bar). One section per milestone or machine, newest first. A
section names the machine, the build, the command, and the figures with their spread.

## M1.6 — Presentation times under emulated VRR (2026-11-14)

On the manual clock, the VRR stories (`crates/yuna/tests/headless/vrr.rs`) pin the emulation on a
1920 × 1080 output of 48 to 240 Hz, after six frames of warm-up from a pause of 1 s: at 100 fps,
every presentation 10 ms after the last and one refresh later; at 300 fps, none sooner than
1/240 s, the frames that could not be shown discarded; at 24 fps, 41.667 ms apart within 1 µs,
three refreshes later each, as amdgpu's BTR inserts them; at 23.976 fps, at most 2 of 41
intervals a whole number of 1/240 s periods, against all of them on a fixed 240 Hz output.
Checked against the owner's monitor: not yet, the trace waits for its run
([ADR-0045](adr/0045-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)).

On the real clock: machine as in M1.5's B4 below, Linux 7.3.1. Build: release, commit `e04dee7`
with the log's `presented_at_us`. One run, with the owner's consent, under the agents' lock of
exclusive runs, no game running; other agents' builds ran meanwhile. Command: `nix develop -c
agent-headless -- target/release/examples/frame_time --output 1920x1080@240,1,vrr=48 --seconds
20`: two `vkcube` on lavapipe, Yuna on llvmpipe, 20 s after 3 s of warm-up.

- 4,515 frames drawn, 225.7 a second; a frame's time median 2,343 µs, p99 7,792 µs, longest
  40.4 ms.
- Between two presentations, 4,514 intervals: shortest 4,166 µs, p1 4,166 µs, median 4,167 µs,
  p99 8,391 µs, longest 52.0 ms; none sooner than 1/240 s.
- 15 frames presented after the refresh they aimed at; Yuna's own CPU, llvmpipe apart, 1,316 µs
  a frame.

## M1.5, C5 — An agent's calls and captures (2026-11-13)

Machine and Mesa as in B4 below. Build: release, commit `6023949`. One series, run with
`.delegation/m1.5-c5-measures/series.sh` (local) under the agents' lock of exclusive runs, after
30 s with no build, game or other measure of this session; other sessions' work kept the load
average at 3.4 to 4.1 at the start, and the series' own runs raised it to 13.5 at its end.
Command: `nix develop -c target/release/examples/agent_time <row>`, the rows `--output
3840x2160@240,1.5`, `--output 1920x1080@240,1` and `--gpu --output 3840x2160@240,1.5`, outside
`agent-headless`, since `yuna mcp` opens no display; three runs of each row. Two `vkcube` on
lavapipe, started through `spawn`, cover the output; each window lasts 10 s, after 3 s of warm-up. The ranges
give the three runs' medians, and the highest p99 and longest. `--gpu` drew on the RX 6900 XT's
render node, with the owner's consent, no game running.

An MCP call, timed by the client from its request written to its reply read, 1,000 calls of
each, on every row:

| Calls | Median | p99 | Longest |
|---|---|---|---|
| `clock now`, `windows`, `pointer move`, `keyboard get`, at rest | 53–70 µs | 89 µs | 442 µs, but the first call after a seat's creation, 2.3–22.6 ms |
| `clock now`, two `vkcube` drawing, llvmpipe at 4K | 61–80 µs | 8.9 ms | 10.1 ms |
| the same, llvmpipe at 1080p and 240 Hz | 64–69 µs | 2.1 ms | 2.8 ms |
| the same, `--gpu` at 4K | 87–91 µs | 2.0 ms | 2.9 ms |

Captures of the whole output at the logical size, one after the other for 10 s, against 10 s
without:

| Output | Frames in 10 s, without → with | Missed, without → with | A capture's call: median, p99 | Its copy on the main thread: median, p99, longest | Its encoding: median, p99 | At the physical size, 5 calls: median |
|---|---|---|---|---|---|---|
| llvmpipe, 3840 × 2160 at 240 Hz, scale 1.5 | 952–959 → 594–610 | all → all | 149–151 ms, 168 ms | 47.2–47.8 ms, 50.6 ms, 50.6 ms | 77.8–78.7 ms, 95.9 ms | 99–104 ms |
| llvmpipe, 1920 × 1080 at 240 Hz, scale 1 | 2,391–2,399 → 1,901–1,909 | 0.1–0.4 % → 12.4–13.2 % | 46.0–46.4 ms, 55.3 ms | 8.3–8.4 ms, 9.9 ms, 10.6 ms | 29.6–30.1 ms, 36.8 ms | 21–24 ms |
| `--gpu`, 3840 × 2160 at 240 Hz, scale 1.5 | 2,400–2,403 → 2,253–2,266 | 0–1 → 6–8, 0.3–0.4 % | 76.4–77.2 ms, 108 ms | 3.8–4.1 ms, 21.0 ms, 39.0 ms | 67.3–67.8 ms, 77.9 ms | 33–38 ms |

- An MCP call costs about 60 µs at rest. Under load it most likely waits for the frame being
  drawn, the loop serving calls between frames: its p99 is about one drawing, 8 ms on llvmpipe at 4K,
  2 ms at 1080p or on the GPU. A seat's first call waits for the output's first frame.
- A capture's copy holds the main thread for as long as the renderer reads the pixels back:
  47 ms at 4K on llvmpipe, 8 ms at 1080p, 4 ms at 4K on the GPU, up to 39 ms. While an agent
  captures without pause, the output draws 6 % fewer frames on the GPU, 20 % fewer at 1080p and
  37 % fewer at 4K on llvmpipe, and misses 12.4 to 13.2 % of its frames at 1080p on llvmpipe.
  The bar's budget for an agent's output, full rate while its programs draw, is missed while it captures
  ([debt](debt.md#captures)).
- The encoding, on its own thread, takes longer than the copy: 30 ms at 1080p, 68 to 79 ms at 4K.
  At 4K a logical image, 2,560 × 1,440, takes longer than a physical one, 3,840 × 2,160: its
  pixels are averaged over each unit first. No capture at the physical size passed 5 MB.
- An agent waits about 150 ms for a 4K capture on llvmpipe, 77 ms on the GPU, 46 ms at 1080p.

### `yuna mcp` with a real MCP client

Claude Code 2.1.318, `claude -p` with Opus 5.5 and `yuna mcp` as its only MCP server, Yuna's
release build of commit `6023949`, on llvmpipe, asked in plain words to do the done-when's
story. In 14 turns, its own: it created a seat, started SlimeEngine's `interface`, captured its
window, clicked its text field through that capture's id, read that a field was enabled,
selected all, typed `Grüße`, checked it by a capture, clicked "view", read the bar's new text,
"pressed view, 1 presses", and closed the seat; every check passed. What it asked for is in the
PLAN's ideas; the revision of MCP it spoke was not recorded.

### The weekly SlimeEngine job

On GitHub's `ubuntu-24.04` runner, run 43508415512 on `m1.5` at commit `a44f801`, by hand: 7 min,
of which 5 min 39 s built the three examples and cooked their data, its cache empty, and 70 s
ran the tests, 40 s of them building. The story's field asked for an input method 203 ms after
its click; `window` and `editor` left Yuna at rest.

## M1.5 — Frames drawn ahead of their boundary (2026-11-13)

Machine and Mesa as in B4 below. Build: release, commit `87d0857`. One series, run with
`.delegation/m1.5-frames-measures/series.sh` (local), after 30 s under 1.5 cores busy with no
build, game or other measure; the machine had started 10 minutes before, and the load average
was 6.4 at the series' start, up to 10.1 at its end, from the series' own runs. Command:
`nix develop -c agent-headless -- target/release/examples/frame_time --output <mode>`, on
llvmpipe, two `vkcube` on lavapipe covering the output, 10 s measured after 3 s of warm-up, three
runs of each row. A frame *missed* was presented after the boundary it aimed at; the lead is the
log's `lead_us` ([ADR-0031](adr/0031-frames-drawn-ahead-of-their-boundary.md)).

| Output | Frames a second | A frame's time: median | p99 | longest | Lead: median | Missed |
|---|---|---|---|---|---|---|
| 1920 × 1080 at 240 Hz, scale 1 | 237.2–239.1 | 1,986–2,280 µs | 3,443–3,994 µs | 6.0–7.3 ms | 4,097–4,166 µs | 9–21 of 2,373–2,392, 0.4–0.9 % |
| 3840 × 2160 at 240 Hz, scale 1.5 | 103.2–108.4 | 7,518–7,689 µs | 9,154–9,537 µs | 11.3–15.3 ms | 4,166 µs | all |

- At 1080p the output holds its rate. Its lead sits at its cap, one period: within any half
  second, one drawing, timed from its deadline, took longer than 3.17 ms, the period minus the
  margin. The frames missed are those whose drawing took longer than the period; no margin within
  the cap would have saved them. [ADR-0032](adr/0032-input-to-present-read-by-mode.md)'s count,
  zero, is missed here; its high tier draws on the GPU, not measured. An animating client's commit is then presented about two
  periods after it, as before ADR-0031: on llvmpipe, drawing time varies too much for less.
- At 4K every drawing takes longer than the period, as B4 found: every frame is presented late,
  and the feedback says so. 103 to 108 frames a second here against B4's 115 to 117: the drawings
  took 7.5 to 7.7 ms here, 6.9 to 7.2 ms in B4, at a higher load. Not compared under the same
  conditions; no cause found beyond the load.
- Floor and margin stay KWin's, 2 ms and 1 ms: where they matter, on a GPU, where a drawing takes
  0.2 ms, nothing was measured; `--gpu` waits for the owner's consent ([debt](debt.md)).
- Nothing was drawn at once but one frame: the clients draw continuously.

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

### CI

On GitHub's `ubuntu-24.04` runner, for commit `a29bca8`, one run each: CI's one job took 2 min
47 s (run 43306298277), the applications' test 15.7 s of it, against 2 min 41 s at B3;
SlimeEngine's workflow, which the push started since it changes the drawing path, 1 min 32 s
with the example's binary from its cache (run 43306298246).

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
