# Measurements

What the benchmarks and the measurements by hand found, against the budgets of the quality bar in
[`ROADMAP.md`](ROADMAP.md#3-quality-bar). One section per milestone or machine, newest first. A
section names the machine, the build, the command, and the figures with their spread.

## M1.11 — `yuna mcp` with a real MCP client (2026-11-18)

Claude Code 2.1.318, `claude -p` with Sonnet 5.5 and `yuna mcp` as its only MCP server, no other
tool, Yuna's Nix package (llvmpipe, 1920 × 1080 at 60 Hz, scale 1), the programs on a throwaway
home, asked in plain words to do M1.11's done-when.

- **At `547fdf3`**: the protocol error found (`wl_subcompositor`, object 5, code 0, "Surface already
  has a role.", through `events` and `spawn list`) and its log read; then Yuna died of SIGSEGV as
  soon as `gtk4-demo` drew with the machine's GPU, accessibility on or off
  ([ADR-0098](adr/0098-on-llvmpipe-dmabufs-offered-for-no-device-and-checked-as-llvmpipe-maps-them.md)).
  17 turns, 0.14 USD.
- **At `85b9d90`, after the fix**: all five steps passed in 30 turns, 0.19 USD: the error found; the
  "Message Dialog" button found in the accessibility tree and pressed at its centre, the dialog
  opened and closed by its Cancel button; the window moved to the second output through a named
  workspace, the output removed, the window hidden with its workspace until shown, then captured;
  the seat closed with nothing left running.
- What got in its way: no tool moves a window to an output; a removed output's windows are hidden
  with their workspace, which only a refused capture tells; a click tells nothing of what it
  pressed. They are in the PLAN's ideas.

## M1.11: the rest auditor (2026-11-18)

What the rest auditor costs Yuna (`rest.rs`), measured by `crates/yuna/examples/rest_cost.rs` in
a release build of the rest auditor on `36994d8`, uncommitted. One agent seat, one output of
640 × 480 at 60 Hz and scale 1 that nothing draws on, on a manual clock; the clients connect
through socket pairs and send `wl_display.sync`, or commit a surface without a role. Each case
timed 10,000 times. Machine: Ryzen 7 5800X (16 threads), Linux 7.3.1. **Not disturbed**: the
one-minute load average read 0.30 before and after, no process above 3.2 % of a core; the
five-minute average, 3.04, still fell from other sessions' builds before
(`.delegation/m1.11-rest-auditor-2/18`, local).

| Clients | Case | Median | p99 | Longest |
|---|---|---|---|---|
| 10 | naming, one ready | 0.43 µs | 0.44 µs | 7.3 µs |
| 10 | naming, all ready | 0.99 µs | 1.01 µs | 25 µs |
| 10 | round, one ready | 6.67 µs | 9.55 µs | 26 µs |
| 10 | round, one commit | 6.51 µs | 6.78 µs | 23 µs |
| 10 | round, 100 commits | 81.7 µs | 88.3 µs | 215 µs |
| 100 | naming, one ready | 0.48 µs | 0.50 µs | 7.2 µs |
| 100 | naming, all ready | 6.60 µs | 6.91 µs | 36 µs |
| 100 | round, one ready | 7.01 µs | 9.69 µs | 14 µs |
| 100 | round, one commit | 6.79 µs | 7.09 µs | 26 µs |
| 100 | round, 100 commits | 82.7 µs | 87.9 µs | 116 µs |

- **Naming**: after a wake of the display, a wait of no time on the auditor's epoll and a count
  for each client ready. With one client ready, as when one program draws, it costs under half a
  microsecond at 10 and at 100 clients: the epoll gives the ready clients alone, never a walk of
  all. Every client ready costs about 65 ns a client.
- **A round**, one client's request read and answered, every count it feeds included: the naming
  is 6 to 7 % of it. The round grows by 0.3 µs from 10 to 100 clients.
- **A commit** costs 0.76 µs at 10 clients and 0.77 µs at 100: the batch of 100 against one,
  divided by 99, Yuna's whole handling of a commit of a surface without a role, its client named
  and its commit counted, included. Its client is read once for the seat's commits and the
  auditor together; the auditor's own share, a map's lookup by the client's number, is not
  measured apart, nor a round without the auditor, which cannot be turned off.
- Two earlier runs, on `5200b93` and disturbed at loads of 6.55 and 24.5, gave the naming with one
  client ready at 0.47 µs and 0.68 µs at 10 clients (`.delegation/m1.11-rest-auditor/02`, `/11`,
  local).

## M1.11 — `yuna mcp`, one and many (2026-11-18)

What agents' Yunas cost: one `yuna mcp` alone, and N `yuna mcp` with a seat each against one
`yuna mcp` with N seats, the figures Stage 2's choice of
[ADR-0078](adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md) and the ROADMAP's
budgets of `yuna mcp` rest on
([ADR-0082](adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md), item
25). Measured by `crates/yuna/examples/instances.rs`, whose documentation says how each figure is
read, in a release build of `264087e`, the example uncommitted. Machine as in M1.9: Ryzen 7 5800X
(16 threads), 32 GB, RX 6900 XT on amdgpu, Linux 7.3.1, Mesa 26.2.6. Each seat's output is
1920 × 1080 at 60 Hz and scale 1; the starts and the seats together have one series more at
Yuna's default, 3840 × 2160 at 240 Hz and scale 1.5. Yuna draws on llvmpipe, `yuna mcp`'s
default, or with `--gpu` on the render node. Each seat's client is Alacritty 0.17.0, in the applications' tests' environment, its cursor
still and its child `sleep infinity`: it draws its window and rests. It draws on llvmpipe on both
of Yuna's devices, so that it is the same client in every run. No Xwayland started: Alacritty is a
Wayland client.

```sh
nix develop -c cargo build --release -p yuna --bin yuna --example instances
nix develop -c env TMPDIR=<folder> target/release/examples/instances --phase start
nix develop -c env TMPDIR=<folder> target/release/examples/instances --gpu --phase together
nix develop -c env TMPDIR=<folder> target/release/examples/instances --phase together --output 3840x2160@240,1.5
nix develop -c env TMPDIR=<folder> target/release/examples/instances --phase soak
```

Judged against [ADR-0100](adr/0100-the-budgets-of-yuna-mcp-at-their-conditions.md)'s budgets, at
their conditions, one seat and no program: every quiet start and every instance at rest meets
them, at 1080p and at 4K, but the start at 4K on the GPU, measured only disturbed (126 ms at the
longest, [debt](debt.md)). With a program open, as the seats together run, no budget applies: the
program's buffers are its own. Ten instances together grow nothing in five minutes.

**Disturbed.** Other sessions' test suites ran through most of the runs, from 04:28 to 05:59 UTC,
at times on all 16 threads. A window is disturbed when the other programs used more than 2 cores
or 10 % of the GPU on average (`yuna_bench::machine`); each table says which. A disturbed window's
times are not a budget's evidence. Its memory figures stand, with one caveat. `Pss` gives each
process a share of the pages it maps with others, and other sessions' tests load the shell's Mesa
from the same store path, so they lower Yuna's `Pss_File`. Each reading counts those processes,
"others on its Mesa". `Pss_Anon`, which no other program shares, does not move with them. No page
was swapped out at any reading that reads `SwapPss`, from run `06` on. The proofs are kept locally
in `.delegation/m1.11-instances/`: each run's command, its output, and the machine's state before
and after.

### One start

Each of the 20 starts of a kind is a new `yuna mcp`, its `seat create` written at once. The
figures are the time from the spawn to the reply, read as it arrives, and to the end of the seat's
output's first frame, at the time of Yuna's `frame drawn` log line. Then Yuna at rest with no
client: its own memory (`Pss_Anon` + `Pss_File`,
[ADR-0077](adr/0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md)), of it the
anonymous part, and its GPU memory (DRM `resident-vram` + `resident-gtt`). *Cold*: a new home each
time, so Mesa's shader cache is empty, as an agent's thrown-away folders leave it (ADR-0082, item
27). *Warm*: one home, which a first start filled.

| Device, output | Start | Reply: median, longest | First frame: median, longest | Own memory, median (anonymous) | GPU memory | Others on its Mesa | Other programs | Run |
|---|---|---|---|---|---|---|---|---|
| llvmpipe, 1080p | cold | 75.3, 88.7 ms | 105.5, 113.5 ms | 116.6 MiB (40.7) | none | at 5 of 20 starts | 1.19 cores, quiet | `10` |
| llvmpipe, 1080p | warm | 51.7, 70.6 ms | 67.2, 87.5 ms | 72.6 MiB (34.3) | none | 15 of 20 | 3.80, disturbed | `10` |
| GPU, 1080p | cold | 46.8, 52.2 ms | 58.7, 60.9 ms | 83.3 MiB (19.4) | 26.5 MiB | 0 of 20 | 0.08, quiet | `14` |
| GPU, 1080p | warm | 44.0, 430.7 ms | 56.6, 445.5 ms | 81.2 MiB (18.8) | 26.5 MiB | 5 of 20 | 3.61, disturbed | `14` |
| llvmpipe, 4K | cold | 80.6, 88.6 ms | 102.2, 110.6 ms | 93.7 MiB (64.8) | none | 20 of 20 | 0.64, quiet | `15` |
| llvmpipe, 4K | warm | 61.7, 124.1 ms | 68.3, 136.6 ms | 97.2 MiB (58.8) | none | 20 of 20 | 6.19, disturbed | `15` |
| GPU, 4K | cold | 77.0, 119.7 ms | 79.9, 126.0 ms | 59.4 MiB (19.5) | 50.1 MiB | 20 of 20 | 9.90, disturbed | `16` |
| GPU, 4K | warm | 45.5, 79.7 ms | 48.3, 84.7 ms | 51.9 MiB (18.8) | 50.1 MiB | 19 of 20 | 3.98, disturbed | `16` |

- The quiet cold starts draw their first frame in 106 ms on llvmpipe at 1080p, 114 ms at the
  longest; 102 ms at 4K; 59 ms on the GPU at 1080p, 61 ms at the longest. The reply comes 12 to
  30 ms before the frame. An empty output's first frame is a clear, which costs llvmpipe no more
  at 4K than at 1080p.
- A warm shader cache saves llvmpipe 34 to 38 ms of the first frame and 6 MiB of anonymous
  memory, though its warm runs were disturbed.
- Alone on its Mesa at 15 of 20 starts on llvmpipe and at all 20 on the GPU, Yuna's own memory is
  116.6 MiB on llvmpipe and 83.3 MiB on the GPU, at 1080p. In the other rows, the other processes'
  shares lower it, to between 51.9 and 97.2 MiB; its anonymous part does not move with them. On
  the GPU it equals the desktop Yuna's of M1.9, 83.6 to 84.0 MiB with 15 windows.
- Earlier runs, disturbed by 2.0 to 15.6 cores, drew their cold first frames in 108 to 118 ms on
  llvmpipe and in 73 to 185 ms on the GPU, at 1080p (`01`, `02`, `04`, `06`, `11`). Their
  anonymous memory is the same within 0.4 MiB, in each run but `01`, which did not read it.

### N seats together

For N = 1, 2, 4, 6, 8 and 10, the N seats start together, as N `yuna mcp` with a seat each, then
as one `yuna mcp` with N seats: N spawns, or N `seat create` written at once, then each seat's
Alacritty through `spawn` once its seat has replied. The time runs from the first spawn to the end
of the frame that first shows the last seat's Alacritty. Then 60 s at rest. † marks a disturbed
case.

| N | llvmpipe, 1080p: N Yunas | 1 Yuna | GPU, 1080p: N Yunas | 1 Yuna | llvmpipe, 4K: N Yunas | 1 Yuna | GPU, 4K: N Yunas | 1 Yuna |
|---|---|---|---|---|---|---|---|---|
| 1 | 281 ms | 291 ms | 253 ms † | 548 ms † | 391 ms † | 372 ms † | 414 ms † | 334 ms † |
| 2 | 294 ms | 308 ms † | 447 ms † | 477 ms † | 386 ms † | 418 ms † | 249 ms † | 357 ms † |
| 4 | 411 ms | 356 ms † | 714 ms † | 546 ms † | 394 ms † | 552 ms | 395 ms † | 638 ms † |
| 6 | 372 ms | 394 ms | 973 ms † | 1,753 ms † | 408 ms | 587 ms | 649 ms † | 329 ms † |
| 8 | 355 ms | 450 ms | 4,202 ms † | 399 ms † | 455 ms | 767 ms † | 327 ms | 361 ms |
| 10 | 410 ms | 521 ms † | 1,899 ms † | 547 ms † | 584 ms † | 1,399 ms † | 371 ms | 467 ms |

Runs `17`, `09`, `12` and `13`, in the order of the columns; a first series on llvmpipe at 1080p,
`05`, was disturbed in 11 of its 12 cases.

- At N = 1 both columns are the same set-up: 281 and 291 ms give the noise. At 6 seats and more,
  in each case quiet in both columns, N Yunas show every seat's client sooner than one Yuna, by
  22 to 179 ms. They start side by side, where one Yuna makes its seats one after another: at 8
  seats on llvmpipe at 1080p, one Yuna's last seat replied at 151 ms, the 8 Yunas' last at 81 ms
  (`17`); at 10 seats on the GPU at 4K, 135 and 75 ms (`13`).
- When other programs take the cores, the N Yunas compete for them, and some disturbed cases
  turn the other way: on the GPU at 1080p, 8 Yunas took 4.2 s, one Yuna 0.4 s (`09`).

The memory at rest. Per agent is the growth from 1 to 10, divided by 9.

| Device, output | The Yunas' anonymous: N Yunas, 1 → 10 | 1 Yuna, 1 → 10 seats | Per agent: its own Yuna | a seat | Everything at 10, `Pss`: N Yunas, 1 Yuna | The GPU's: N Yunas, 1 → 10 | 1 Yuna, 1 → 10 seats |
|---|---|---|---|---|---|---|---|
| llvmpipe, 1080p (`17`) | 45.3 → 451.3 MiB | 45.3 → 155.1 MiB | 45.1 MiB | 12.2 MiB | 1,084.4, 784.9 MiB | none | none |
| GPU, 1080p (`09`) | 20.1 → 205.5 MiB | 20.6 → 24.9 MiB | 20.6 MiB | 0.5 MiB | 852.7, 663.2 MiB | 36.9 → 368.9 MiB | 36.9 → 154.0 MiB |
| llvmpipe, 4K (`12`) | 80.8 → 810.4 MiB | 80.9 → 507.5 MiB | 81.1 MiB | 47.4 MiB | 1,794.8, 1,446.0 MiB, the second beside 16 others on its Mesa | none | none |
| GPU, 4K (`13`) | 20.8 → 203.1 MiB | 19.8 → 24.8 MiB | 20.3 MiB | 0.6 MiB | 1,197.4, 1,013.9 MiB | 83.6 → 835.6 MiB | 83.6 → 516.1 MiB |

- "Everything" is the processes under the Yunas: the Yunas, each Alacritty and its `sleep`. Each
  Alacritty holds about 46 MiB of anonymous memory, the same in both cases.
- An agent's own Yuna costs more than a seat of a shared one. On llvmpipe it costs 33 to 34 MiB
  more of anonymous memory, at both sizes. On the GPU it costs 20 MiB more of anonymous memory,
  and 24 MiB at 1080p or 36 MiB at 4K more of the GPU's. At ten agents, everything's `Pss` is
  300 MiB more on llvmpipe at 1080p, and 190 MiB more on the GPU with 215 MiB more of the GPU's
  memory; at 4K, 184 and 320 MiB more on the GPU.
- A seat of a shared Yuna costs 12 MiB at 1080p and 47 MiB at 4K of the process's memory on
  llvmpipe, and 13 MiB and 48 MiB of the GPU's on the GPU: about its output's buffer and its
  window's texture, by their sizes.
- Shared library pages count once across the Yunas: at ten, each Yuna's `Pss` is 53.6 MiB on
  llvmpipe and 28.9 MiB on the GPU at 1080p, against 90.6 and 63.6 MiB for a Yuna alone with its
  client.
- At rest, every one of the 60 windows of 60 s, in the five series, drew 0 frames, and everything
  under the Yunas used 0.0 ms of CPU. The anonymous memory after the window equalled the memory
  before it in 59 of them, and was 0.1 MiB less in the last.

### Ten together for 5 minutes

Ten `yuna mcp`, each with a seat and Alacritty, at 1080p, held at rest for 300 s, their memory read
every 30 s. No other process mapped the shell's Mesa at any reading.

| Device | Everything, `Pss` (anonymous): at 0 s, at 300 s | The Yunas' own: at 0 s, at 300 s | The GPU's | Furthest from 0 s at a reading | Frames, CPU | Other programs | Run |
|---|---|---|---|---|---|---|---|
| llvmpipe | 1,089.9 (915.2), 1,088.8 (915.2) MiB | 501.4, 500.7 MiB | none | −1.0 MiB | 0, 0.0 ms | 11.48 cores, disturbed | `07` |
| GPU | 851.5 (666.7), 851.4 (666.7) MiB | 251.4, 251.3 MiB | 368.9 MiB at each reading | +0.5 MiB | 0, 0.0 ms | 15.41 cores, disturbed | `08` |

Nothing grows: the anonymous memory and the GPU's are the same at every reading, and the file
pages move by a megabyte at most. At rest the ten use nothing, so nothing they share is contended;
started together, the last of ten quiet Yunas replied at 96 ms, against 70 ms for one alone
(`17`).

## M1.11 — The events' look (2026-11-18)

What `yuna mcp`'s look for events costs a round (`mcp/events.rs`), measured by
`crates/yuna/examples/events_look.rs` in a release build of branch `m1.11-events` after
`6e271a3`, its correction round uncommitted. One agent seat, one output of 1920 × 1080 at 60 Hz
and scale 1, on a manual clock; the windows from clients of the example's own, 1,000 a client at
most, each 64 × 64 pixels. Each case timed 1,000 times, the look alone. Machine: Ryzen 7 5800X
(16 threads), Linux 7.3.1. **Disturbed**: the one-minute load average read 15.8 before and 16.5
after; other sessions' tests ran meanwhile, one of SlimeEngine at 730 % of a core and another
worktree's Yuna tests (`.delegation/m1.11-events-2/13`, local).

| Windows | Case | Median | p99 | Longest |
|---|---|---|---|---|
| 100 | no change | 0.1 µs | 0.1 µs | 0.5 µs |
| 100 | a frame | 0.2 µs | 0.4 µs | 1.7 µs |
| 100 | a title | 130 µs | 209 µs | 2.5 ms |
| 1,000 | no change | 0.1 µs | 0.1 µs | 0.5 µs |
| 1,000 | a frame | 0.2 µs | 0.4 µs | 0.8 µs |
| 1,000 | a title | 4.4 ms | 8.4 ms | 28 ms |
| 2,000 | no change | 0.1 µs | 0.1 µs | 0.7 µs |
| 2,000 | a frame | 0.2 µs | 0.3 µs | 0.6 µs |
| 2,000 | a title | 11 ms | 29 ms | 38 ms |

- **No change** and **a frame**, one window's buffer committed and its output refreshed, as a game
  draws: the look reads the compositor's description generation and stops, at any count.
- **A title**: every window and output described and compared, one event recorded. The comparison
  is linear; describing the windows is not: `Layout::window` walks the windows of its workspace
  for each window, so the cost grows with their square. At 1,000 windows a look after a change
  takes a 240 Hz frame's time ([debt](debt.md#the-mcp-server)).
- The checker's measure of the first comparison alone, which searched every window's list for
  each window seen before, gave 2.41 ms at 2,000 windows, disturbed too
  (`.delegation/m1.11-events-check/01`, local).

## M1.11 — A keyboard layout's change (2026-11-18)

`crates/yuna/examples/layout_time.rs`, on M1.11's keyboard layouts before their commit (base
`6e271a3`): Yuna in the example's own process, on the manual clock, with one agent's seat and no
client. It times `Compositor::set_keyboard_layout` from the call to its return, on the state
thread: XKB's compile from the names, Yuna's reading of the keys, Smithay's compile from the
keymap's text and its keymap file. 20 changes to each layout in turn, after one change of each
to warm the file cache. Then `Compositor::type_text`, 20 calls of a line: plain, and with Num
Lock on, a state whose keys `type` reads again at each call. Machine: Ryzen 7 5800X, Linux
7.3.1. Other programs used 0.05 cores in the debug run and 0.07 in the release run: not
disturbed.

```sh
nix develop -c cargo run -p yuna --example layout_time
nix develop -c cargo run --release -p yuna --example layout_time
```

A change, median and longest:

| Layout | Release | Debug |
|---|---|---|
| `fr` | 2.72, 2.75 ms | 4.26, 4.31 ms |
| `de` | 2.78, 2.81 ms | 4.31, 4.35 ms |
| `us,ru`, `grp:alt_shift_toggle` | 3.15, 3.17 ms | 6.45, 6.54 ms |
| `us` | 2.57, 2.62 ms | 4.08, 4.11 ms |
| The limit: `us,fr,de,ru`, 14 `grp:` options in 247 bytes | 5.04, 5.09 ms | 11.04, 11.13 ms |
| All 100 changes | 2.78, 5.09 ms | 4.31, 11.13 ms |

- The first change to each layout, from a cold file cache: 2.5 to 5.1 ms in release, 4.0 to
  11.1 ms in debug.
- XKB's compile alone, the same in both builds: 1.4 to 2.8 ms from the names, 0.7 to 0.9 ms from
  the keymap's text.
- **Over 10 ms**: only the limit, in a debug build. Its key tables, four groups of them, are most
  of the difference from release.

`type`, one call, median and longest:

| Line | Release, plain | Release, Num Lock | Debug, plain | Debug, Num Lock |
|---|---|---|---|---|
| 63 characters on `us` | 0.05, 0.06 ms | 0.17, 0.19 ms | 0.40, 0.43 ms | 1.46, 1.47 ms |
| The same and `é ж`, 67 characters on the limit | 0.06, 0.07 ms | 0.46, 0.48 ms | 0.46, 0.49 ms | 3.69, 3.75 ms |

Earlier runs, on code since changed, before one compile took the place of two and the scans
shared their xkb states:

| Build | Median, longest | Other programs | Disturbed |
|---|---|---|---|
| Debug | 9.46, 23.99 ms | 10.5 cores | yes |
| Debug | 10.07, 17.95 ms | 13.3 cores | yes |
| Debug | 4.51, 10.11 ms | 2.9 cores | yes |
| Release | 5.54, 12.70 ms | 11.3 cores | yes |
| Debug | 4.01, 6.40 ms | 0.02 cores | no |
| Release | 2.76, 3.66 ms | 0.08 cores | no |

Not measured: a change with clients connected, each of which is sent the keymap's file; a
change while a program types.

## M1.11: accessibility at rest (2026-11-18)

Whether anything runs in Yuna or in a seat's accessibility bus once an agent has read a tree.
`yuna mcp`, debug build of the M1.11 accessibility work on `6e271a3`, on the desktop (Ryzen 7
5800X, Linux 7.3.1, dbus 1.16.2 and GTK 4.24.0 of the shell). The test
`yuna_and_the_bus_rest_once_a_tree_is_read` (`crates/yuna/tests/mcp.rs`) starts `gtk4-demo
--run=dialog` with `accessibility: true` on llvmpipe and lavapipe, finds its "Message Dialog"
button, waits until Yuna and the daemon have gone 6 s without a context switch, then reads the
context switches of every thread of each, `/proc/<pid>/task/*/status`, before and after 30 s.

| What | Settled after the walk | Yuna over 30 s | dbus-daemon over 30 s |
|---|---|---|---|
| A walk of 24 nodes | 5.1 s | 0 context switches (289 → 289) | 0 (54 → 54) |

- The daemon wakes once after a walk, up to its reply timeout, 5 s, later: its timer of calls
  waiting for a reply stays armed when the replies come, fires once, finds nothing and stops
  (dbus 1.16.2, `bus/expirelist.c`). A first run that waited 2 s of quiet saw that one wake,
  dbus-daemon 1 context switch and Yuna 0 over 30 s.
- Disturbed: other sessions' builds kept the load average at 28 to 40 during both runs. A process
  at rest does not switch context under load; nothing here is a time.
- Not measured: a walk's time on a large tree, Qt's programs at rest, several programs on one
  bus.

## M1.10 — A program's presentation in Yuna against a headless sway (2026-11-17)

uwui's own `scripts/measure.py`, at the revision `flake.nix` pins (`fb77f0d`), measures `vkcube`
in each compositor, from outside, as uwui measures its programs: the frames the compositor
presents, counted through `wlr-screencopy`'s copies with damage, each timed by the compositor's
`ready`; its threads' CPU from `/proc`; the GPU's load and power from amdgpu. In sway, its
default mode: a headless sway 1.12 of its own, GLES2 on the GPU. In Yuna, `--display`, on `yuna
--headless --agent-seat --gpu`, release build of commit `d8ef6d1`. Both at 3840 × 2160, 240 Hz,
scale 1.5; `vkcube --wsi wayland` on RADV, its window alone on the output; 2 s, then 10 s
measured. Machine as in M1.9: Ryzen 7 5800X, RX 6900 XT, Linux 7.3.1, Mesa 26.2.6. Two runs,
from the bench's shell (`nix develop .#bench`): the processor idle at 99 to 100 % by `vmstat`
before each, no game, the GPU at 0 %; another session's `cargo test` was started but idle.

| Run | Compositor | Frames in 10 s | Rate | p50 | p95 | p99 | Longest | Compositor's CPU | GPU busy | GPU power |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | sway | 2,439 | 243.9 Hz | 4.10 ms | 4.11 ms | 4.13 ms | 4.39 ms | 5.8 % of a core | 3.9 % | 36.2 W |
| 1 | Yuna | 2,400 | 240.0 Hz | 4.17 ms | 4.17 ms | 4.17 ms | 4.17 ms | 5.2 % | 5.4 % | 35.6 W |
| 2 | sway | 2,442 | 244.2 Hz | 4.09 ms | 4.11 ms | 4.13 ms | 4.70 ms | 5.4 % | 2.8 % | 36.7 W |
| 2 | Yuna | 2,400 | 240.0 Hz | 4.17 ms | 4.17 ms | 4.17 ms | 4.17 ms | 5.0 % | 3.4 % | 35.5 W |

- Yuna presents at its output's rate exactly, every interval one refresh of 240 Hz, on its own
  clock ([ADR-0023](adr/0023-frames-presentation-and-the-clock.md)). wlroots' headless output, sway's,
  draws whole milliseconds after each commit, 4 ms at 240 Hz, so about 244 frames a second
  ([survey](survey.md#the-rivals-measured)), which uwui's measurements record as 243 Hz.
- The compositor's CPU includes the copies `measure.py` asks, one a frame, of a single pixel;
  its share is close in both. The GPU figures are the card's whole, a few percent either way.
- Not measured: a program at its own rate below the output's, a pointer in motion, Xwayland's
  programs; the figures of one machine.

## M1.9 — The rivals headless, at equal work (2026-11-16)

The bench (`crates/yuna-bench`, [ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)),
commit `627f160`, every recipe at equal work (effects, wallpaper and XWayland off), every scenario,
three runs of each case in a random order (seed `1794809641392719660`), 189 cases from 06:14 UTC,
none failed, none disturbed by other programs. Machine as in M1.4's B4: Ryzen 7 5800X, RX 6900 XT,
Linux 7.3.1, Mesa 26.2.6, every compositor also loading nixpkgs' libgbm 26.1.3; the GPU's level `high`, as the owner set it. Every compositor on the
render node, one output at 3840 × 2160, 240 Hz, scale 1.5, read back from the compositor. Versions:
sway 1.12 on wlroots 0.20.2 (GLES2, and Vulkan as sway-vulkan), dwl 0.9, mango 0.18.4, Umbriel
`d171ccb`, Hyprland 0.57.1 (inside a dwl parent counted outside its core, without realtime
scheduling), KWin 6.8.2, Mutter 51.0, Yuna on Smithay's GLES. Command:

```sh
nix develop .#bench -c cargo run --release -p yuna-bench -- headless \
  --compositor sway,sway-vulkan,kwin,yuna,dwl,mango,umbriel,hyprland,mutter \
  --configuration equal-work --scenario rest,game,film,pacing,motion --runs 3
```

The lowest and highest of the three runs. wlroots' headless backend draws 4 ms after each commit,
so its rivals run at up to 250 frames a second where the others hold 240: costs a second compare
only within one pace; costs a frame, from exact counts, compare across.

**A game**: mpv fullscreen, a 240-frame-a-second clip drawn with Vulkan on radv, 10 s; frames
counted by mpv in the window itself.

| Compositor | Frames a second | CPU a frame | GPU a frame | PSS |
|---|---|---|---|---|
| Yuna | 240 | 160–166 µs | 191–196 µs | 69.4–69.5 MiB |
| Umbriel | 242 | 198–200 µs | 203–207 µs | 86.3–86.6 MiB |
| Hyprland | 239 | 210–214 µs | 336–343 µs | 126 MiB |
| KWin | 240 | 244–247 µs | 332–336 µs | 189–192 MiB |
| Mutter | 240 | 248–249 µs | 289–292 µs | 91.6–91.7 MiB |
| sway | 244 | 150–153 µs | 215–218 µs | 76.8–77.0 MiB |
| sway on Vulkan | 242 | 169–170 µs | 290–296 µs | 62.3 MiB |
| dwl | 244 | 150–152 µs | 215–217 µs | 70.5–70.6 MiB |
| mango | 244 | 156–157 µs | 216–219 µs | 78.2–78.5 MiB |

**At rest**, still windows, 60 s after 5 s: no compositor used GPU time, and those whose frames
were counted drew none (Mutter's were not counted; those counted by capture, a lower bound, showed
none). Wake-ups a second: Umbriel 10.1 (its timer for hidden windows), Hyprland 3.5 to 3.6, mango
0.03; Yuna, sway, sway on Vulkan and dwl used no CPU time at all in the window, so none of their
threads woke; KWin and Mutter used 0.07 to 0.15 ms in each, their wake-ups not all seen.

Memory after the rest window, in MiB, the lowest and highest of the three runs, and its growth a
window from 1 to 150 windows (the means' difference over 149; for Yuna and Hyprland, whose figures
are near, the range over every pair of runs in brackets): the process's own (`Pss_Anon` +
`Pss_File`); the clients' `wl_shm` buffers it holds mapped (`Pss_Shmem`); its GPU memory, the
kernel's DRM `resident-vram` + `resident-gtt`. A window's size depends on each compositor's layout,
which the series did not record.

| Compositor | Process: 1 / 15 / 150 windows | A window | Clients' buffers at 15 | A window | GPU: 1 / 15 / 150 windows | A window |
|---|---|---|---|---|---|---|
| Yuna | 83.6–83.7 / 83.6–84.0 / 84.9–85.1 | 0.0087 (0.0075–0.0103) | 30.4 | 0.31 | 83.6 / 115.1 / 147.1 | 0.43 |
| Umbriel | 102.1–102.4 / 102.5–102.8 / 105.5–105.7 | 0.0225 | 0 | 0 | 124.1 / 364.0 / 2,526.6 | 16.1 |
| Hyprland | 131.7–131.9 / 131.8 / 132.7–132.9 | 0.0063 (0.0054–0.0079) | 0 | 0 | 427–429 / 1,159–1,161 / 772–774 | 2.31 |
| KWin | 206.8–208.7 / 207.3–209.4 / 211.9–212.8 | 0.029 | 39.6 | 2.64 | 59.9 / 115.4 / 523.0 | 3.11 |
| Mutter | 107.2–107.3 / 108.3 / 112.2 | 0.034 | 39.6 | 2.64 | 109.5 / 164.3 / 577.9 | 3.14 |
| sway | 93.9–94.0 / 94.4–94.7 / 97.3–97.5 | 0.023 | 0 | 0 | 163.7 / 260.2 / 874.3 | 4.77 |
| sway on Vulkan | 84.2 / 84.4–84.5 / 86.8 | 0.017 | 0 | 0 | 255.2 / 347.9 / 1,402.7 | 7.70 |
| dwl | 85.4–85.5 / 86.9–87.2 / 89.4–89.5 | 0.027 | 0 | 0 | 164.0 / 196–200 / 633.9 | 3.15 |
| mango | 94.6–94.8 / 95.6–96.0 / 100.3–100.5 | 0.038 | 0 | 0 | 757 / 1,482 / 3,785–3,896 | 20.7 |

**Start**, from the compositor's start to the sign of its being ready (a log line, a D-Bus name or
a reply, each recipe's own), polled every 50 ms, so accurate to about 50 ms and not one method
across compositors; the lowest, median and highest of the series' 18 to 21 starts each: Yuna 61,
71, 71 ms; dwl 76, 76, 126 ms; sway on Vulkan 101, 106, 111 ms; sway 86, 126, 127 ms; mango 121,
131, 131 ms; Mutter 126, 192, 237 ms; Umbriel 186, 236, 287 ms; KWin 237, 247, 247 ms; Hyprland
with its parent 321, 331, 336 ms.

**Pacing**, `weston-presentation-shm` in a small window, 10 s: no presentation more than 1.5
periods after the last, for any. Yuna, KWin, Mutter and Hyprland present at 4,166 µs (their p99
4,166, 4,166, 4,212 to 4,223 and 4,218 to 4,235 µs); the wlroots rivals at 4,010 to 4,093 µs.

**A film**, mpv playing a 24-frame-a-second clip, 10 s: every compositor showed its 240 frames,
none dropped or delayed; 5 to 10 ms of CPU a second.

**Motion**, the recipe's actions through its own commands, 8 actions: no comparison of frames
across backends (ADR-0075); Yuna and Mutter take no outside command and were not run. Yuna's own
motion is M1.7's, below.

- **Yuna costs the least of the peers in a game**: 160 to 166 µs of CPU and 191 to 196 µs of GPU a
  frame, against Umbriel's 198 to 200 and 203 to 207 µs, the next. Frames counted by mpv in the
  window make a cost a frame comparable across backends; sway and dwl, the floor
  ([ADR-0072](adr/0072-budgets-set-against-the-peers-the-same-day.md)), spend 150 µs of CPU and
  215 µs of GPU a frame. sway costs the GPU more on Vulkan (290 µs) than on GLES (215 µs).
- **Yuna's own memory is the least of the peers'**, 84 MiB at 15 windows against Umbriel's 102.5,
  and so is its GPU memory's growth; its GPU memory at 15 windows is 0.3 MiB below KWin's, the
  least of the peers', in every run. Its
  proportional set size grows with its clients' buffers, which it keeps mapped until they are
  replaced where the wlroots rivals release them once copied: 0.31 MiB a window, from code reading
  and these figures, the mechanism not traced further.
- These are headless figures, the costs the clock does not decide; frames missed on a real
  display, input to present and watts come from the owner's tour at Stage 2's exit
  ([ADR-0075](adr/0075-the-rivals-measured-headless-now-on-a-real-display-at-stage-2s-exit.md)).

**As shipped**, one run of each case from 10:21 UTC (commit `3943e48`, the same code), for context
and never ranked: 18 of its 63 cases were disturbed by other programs, by more than 2 cores or
10 % of the GPU, and are marked so in the run file. This run held the lock of the agents' service
it was started from, not the one all agents share, so other agents' timed work was not held off. In the game, the undisturbed ones: Umbriel, its blur and
shadows on, 248 µs of CPU and 169 µs of GPU a frame; Hyprland 224 and 348 µs; KWin 276 and 339 µs;
dwl 165 and 208 µs; sway 182 and 211 µs. Mutter's game drew 38.5 frames a second as shipped, its
cause not looked for. The run file is kept locally with the bench's others.

## M1.9 — The Vulkan prototype, on the desktop and lavapipe (2026-11-16)

The throwaway prototype of [ADR-0073](adr/0073-vulkan-alone-uwui-for-what-yuna-draws-and-a-prototype-of-what-no-rival-measures.md),
commit `86984e3` of branch `prototype/vulkan`, run from GitHub as the owner runs it
([owner's runs](owner-runs.md#1-the-vulkan-prototype-on-the-laptop-and-the-steam-deck)), at
3840 × 2160, under the agents' exclusive lock:
`nix run 'github:HoneySlatt/Yuna/prototype/vulkan#vulkan-prototype' -- report --size 3840x2160`,
on device 0, then with `VK_ICD_FILENAMES` on the shell's lavapipe. Machine as in M1.4's B4: RX 6900
XT through RADV, the system's Mesa 26.2.6, Linux 7.3.1; the GPU's performance level `high`, sclk
2,530 MHz, mclk 1,000 MHz, read and left as the owner set it. Microseconds: median / 99th
percentile / longest.

Other programs disturbed parts of both runs, by ADR-0060's rule, the prototype's own estimate: on
RADV six of its eight timed parts, the 1920 × 1080 upload and every composition but the one window
in 16-bit floats, others using 2.0 to 3.6 logical CPUs and at most 2.3 % of the GPU, so the load
was the processor's and the GPU's timestamps were not contended; on lavapipe, which draws on the
processor, three of ten, both staging uploads and the one window in 16-bit floats, whose 99th
percentile below is about three times the other rows', its longest five times. The RADV CPU
figures below come from disturbed parts. Neither run was
repeated quiet.

- **The driver**: both report Vulkan 1.4.354 and offer every extension asked (modifiers, dma-buf
  import and export, foreign queues, sync files, timeline semaphores, host memory), but for
  lavapipe's `VK_EXT_physical_device_drm`: it has no render node.
- **Modifiers**: on RADV, every modifier offered is exported, imported into a second device and
  sampled correctly: 7 for `B8G8R8A8_UNORM` and `A2B10G10R10_UNORM_PACK32`, 6 for
  `R16G16B16A16_SFLOAT`, DCC ones included; two DCC modifiers allow at most 2560 × 2560. lavapipe:
  linear only.
- **Explicit sync**: a semaphore and a fence exported as sync files while the client's work still
  ran, imported into the second device and waited; a timeline semaphore. All pass on both.
- **`wl_shm`'s path**: RADV refuses a memfd's pages as a host pointer
  (`VK_ERROR_INVALID_EXTERNAL_HANDLE`) while anonymous memory imports: on AMD a `wl_shm` buffer
  goes through a staging copy, from system memory over PCIe.

| Upload of one `wl_shm` buffer | CPU | GPU |
|---|---|---|
| RADV, 1920 × 1080, staging | 342 / 416 / 433 | 319 / 395 / 405 |
| RADV, 3840 × 2160, staging | 1,461 / 1,716 / 1,853 | 1,263 / 1,278 / 1,288 |
| lavapipe, 3840 × 2160, staging | 1,650 / 2,297 / 2,944 | 1,962 / 2,437 / 3,056 |
| lavapipe, 3840 × 2160, host pointer | 3.3 / 12.8 / 16.6 | 1,592 / 2,051 / 2,206 |

- **The composition's floor**: windows as textured quads with rounded corners and a shadow,
  sampled from imported dma-bufs, blended over a clear, 1,000 frames; CPU to record and submit,
  GPU by timestamps.

| Windows | Output | RADV: CPU | RADV: GPU | lavapipe: GPU |
|---|---|---|---|---|
| 1 | `R16G16B16A16_SFLOAT` | 6.4 / 25.8 / 65.0 | 71.1 / 71.4 / 74.3 | 12,276 / 42,151 / 69,128, disturbed |
| 1 | `B8G8R8A8_UNORM` | 8.4 / 41.8 / 67.5 | 53.6 / 55.2 / 56.5 | 9,571 / 10,751 / 12,009 |
| 15 | `R16G16B16A16_SFLOAT` | 11.7 / 36.0 / 60.6 | 74.8 / 86.4 / 110.6 | 12,462 / 13,713 / 14,127 |
| 15 | `B8G8R8A8_UNORM` | 10.2 / 36.2 / 76.9 | 56.2 / 60.3 / 74.6 | 9,917 / 11,607 / 13,567 |

- **A high-priority queue**: refused on RADV without privileges (`VK_ERROR_NOT_PERMITTED_KHR`) at
  `HIGH` and `REALTIME`; lavapipe grants both.
- On the desktop, a composition at 4K costs Vulkan about 56 µs of GPU in 8 bits and 75 µs in
  16-bit floats, and 10 µs of CPU, whatever the number of windows up to 15: 1.3 to 1.8 % of a
  240 Hz period. Yuna's current GLES path drew M1.4's two `vkcube` windows in 117 µs of GPU and
  162 to 178 µs of CPU; the scenes differ, so this is the order of the gap, not its measure. A 4K
  `wl_shm` upload costs 1.3 ms of GPU, 30 % of a period: the cost of clients that do not send
  dma-bufs. On lavapipe a 4K composition takes 10 to 12 ms, more than a 240 Hz period.
- The laptop and the Steam Deck, the owner's runs, follow.

### The laptop (2026-11-16, the owner's run)

The same commit, `report --device 0 --size 1920x1080`, run by the owner on the lowest tier, whose
panel is 1600 × 900 (below): an upper bound for it. AMD
Athlon Silver 3050U (2 logical CPUs, 3.2 GiB visible), Radeon Graphics "RAVEN2" through RADV,
Mesa 26.1.5, Linux 7.1.5; the GPU's level `auto`. No timed part disturbed.

- **The driver**: Vulkan 1.4.354, every extension asked but `VK_EXT_host_image_copy`.
- **Modifiers**: every one offered imports into a second device and samples correctly, DCC ones
  included: 10 for `B8G8R8A8_UNORM` and `A2B10G10R10_UNORM_PACK32`, 9 for `R16G16B16A16_SFLOAT`.
  jay's note that modifiers fail on AMD before RX 5000 does not hold for this GPU's rendering;
  its display engine is checked by the owner's display check.
- **Explicit sync**: all three checks pass, the sync files still pending at import.
- **`wl_shm`**: the memfd's pages refused as a host pointer, as on the desktop; a 1920 × 1080
  upload through staging costs 1,321 µs of CPU and 2,201 µs of GPU (medians), 13 % of a 60 Hz
  period; 3840 × 2160, 8.9 ms of GPU.
- **The composition's floor** at 1920 × 1080: 1 to 15 windows cost 1,089 to 1,199 µs of GPU in 8
  bits and 1,362 to 1,430 µs in 16-bit floats (medians; 99th percentiles 1.18 to 2.43 ms), and 28 to
  44 µs of CPU: 7 to 9 % of a 60 Hz period. The GPU's clock rose from 400 to 1,100 MHz during the
  parts (`auto`), so the first frames ran slower.
- **A high-priority queue**: refused, as on the desktop.

### The laptop's display (2026-11-16, the owner's run)

The [display check](owner-runs.md#2-the-laptops-display-through-a-vulkan-compositor): sway 1.12
on wlroots 0.20.2's Vulkan renderer (`WLR_RENDERER=vulkan`), from a virtual terminal, on the
laptop's panel, a Chimei Innolux eDP panel at 1600 × 900 and 60 Hz (40 Hz also offered), with
`vkcube` and mpv's test pattern for about 3 min 20 s. The owner saw both smooth. wlroots drew
with RADV "RAVEN2" (Mesa 26.1.5) and the panel showed its buffers allocated with a compressed
modifier, `GFX9 64KB_S_X DCC` in `XR24`: the display engine scans out what Vulkan draws, DCC
included. The only errors came as sway left the display. The second run, without modifiers
(`WLR_DRM_NO_MODIFIERS=1`), did not start: its session was not the active one within 10 s; it
was a fallback for modifiers failing, which they did not, and was not run again.

### The Steam Deck (2026-11-16, the owner's run)

The same commit and command, at 1920 × 1080 rather than the panel's 1280 × 800: an upper bound
for the panel. AMD Custom APU 0932 (8 logical CPUs, 14.5 GiB), "VANGOGH" through RADV, Mesa
26.2.0-devel, Linux 7.2.7-valve2, under NixOS; the GPU's level `auto`.

- **The driver**: Vulkan 1.4.350, the same extensions as the desktop's.
- **Modifiers**: 7, 7 and 6 offered, all imported and sampled correctly, DCC ones included.
- **Explicit sync**: all three checks pass.
- **`wl_shm`**: the memfd refused as a host pointer; a 1920 × 1080 upload through staging, 691 µs
  of CPU and 247 µs of GPU; 3840 × 2160, 1,306 µs of GPU. The 1920 × 1080 part is marked
  disturbed: one reading of the GPU, at 95 % busy over a part of 0.3 s, against the prototype's
  own 21 %; another program is not established, the estimate being one reading.
- **The composition's floor** at 1920 × 1080: 315 to 348 µs of GPU in 8 bits and 394 to 429 µs in
  16-bit floats (medians), 12 to 34 µs of CPU: at most 4 % of a period at 90 Hz, the fastest the Deck's panels refresh.
- **A high-priority queue**: refused.

## M1.8 — A flood of X11 windows and menus (2026-11-15)

What one X11 program flooding its seat costs the other windows (ADR-0070): the story
`x11_misbehaving::five_thousand_windows_and_menus_hold_the_caps_and_stall_no_frame`, in a release
build of branch `m1.8` after `f0a39dc` with the closing review's fixes, run alone under the
agents' exclusive lock (`.delegation/m1.8-lead/01`, local). One X11 program maps 5,000 windows,
another 5,000 override-redirect windows; a Wayland sentinel on the same seat asks a frame
callback each frame. Load: no other process above 5 % of a core and the GPU 0 % busy before and
after; the one-minute load average read 6.0, the release build that had just ended. Quiet.

| What | Figure |
|---|---|
| 5,000 windows sent | 29 ms |
| 1,000 open, the cap of one X11 client | 2.68 s |
| 5,000 menus mapped, 64 shown, the cap | 5.43 s |
| The sentinel's frames meanwhile | 354, the longest gap between two 43 ms |
| The longest round of Yuna's loop | 8.6 ms of CPU |

- Before ADR-0070's bounds, one round took up to 455 ms (the story's first runs, in a debug build,
  without a proof kept).
- CI runs the story in a debug build, where it checks only that the sentinel gets 4 callbacks a
  second or more; the bound of 16 ms a round is checked in a release build alone, by hand.

## M1.8 — XWayland's start and memory, the area filter on the GPU (2026-11-15)

Machine and Mesa as in M1.4's B4 below; Xwayland 24.1.14 from the development shell. Build:
release, of branch `m1.8` at `d1f8f46` with the applications' tests' change, before its X11
menus (`ae57ce4`) were merged. One run of each, in order, under the agents' exclusive lock, outside
`agent-headless`, whose sandbox hides other processes' GPU files; `yuna --headless` opens no
display. `--gpu` drew on the render node, where Xwayland then draws with glamor. Commands, after
`nix develop -c cargo build --release -p yuna --bin yuna --example xwayland_time`:

```sh
nix develop -c target/release/examples/xwayland_time [--gpu]
nix develop -c target/release/examples/xwayland_time --area-filter [--gpu]
```

`xwayland_time` (`crates/yuna/examples/xwayland_time.rs`) runs `yuna --headless` on Yuna's
default output, 3840 × 2160 at 240 Hz and scale 1.5. A start is an X11 program's wait, from its
`connect` to the reply to its first request; *cold* is the first X11 program in a new Yuna, 30
Yunas; *warm* is in one Yuna once the Xwayland before ended when idle, 30 times. Yuna's own
figure is from Xwayland's spawn to its manager owning `WM_S0`, logged in whole milliseconds.

The program reads, every 2 s, what the machine's other programs use: their processor time, the
machine's busy time less the program's own and its descendants', Yuna and Xwayland among them;
and their GPU time on the drawing engines, `gfx` and `compute`, from the DRM statistics of the
8 other processes holding GPU files. A phase is disturbed when they used more than 2 cores or
10 % of that GPU time on average. Every phase below is quiet by that rule; the other programs'
GPU time was nil in each, their processor time is given.

| XWayland's start | Drawing | Median | 90th percentile | Longest | Yuna's spawn to `WM_S0`: median, p90 | Other programs: cores on average, at most |
|---|---|---|---|---|---|---|
| Cold | llvmpipe | 28.63 ms | 29.56 ms | 31.70 ms | 24 ms, 25 ms | 0.15, 0.74 |
| Warm | llvmpipe | 28.64 ms | 30.90 ms | 65.79 ms | 24 ms, 26 ms | 0.58, 5.76 |
| Cold | GPU | 75.53 ms | 78.86 ms | 87.52 ms | 73 ms, 76 ms | 0.40, 3.13 |
| Warm | GPU | 75.47 ms | 79.70 ms | 227.08 ms | 73 ms, 77 ms | 0.95, 11.50 |

| Memory, one X11 window of 256 × 256 at rest | llvmpipe | GPU |
|---|---|---|
| Xwayland's PSS (its RSS) | 35.5 MiB (45.4 MiB) | 60.8 MiB (91.0 MiB) |
| Yuna's RSS before the first X11 program | 130.9 MiB | 82.5 MiB |
| Yuna's RSS with the window shown | 167.7 MiB | 83.3 MiB |
| Yuna's RSS once Xwayland ended | 152.7 MiB | 83.5 MiB |
| Other programs: cores on average, at most | 0.95, 2.71 | 0.19, 0.26 |

With `--area-filter`, a fullscreen X11 window is filled with a new colour without pause, so
that each frame draws it whole; a frame's time is Yuna's `took_us`, as the test's figure below,
over 10 s after 3 s of warm-up, on 3840 × 2160 outputs at 60 Hz. The area filter's case has a
second output at 1.5, which makes XWayland's scale 1.5. Yuna's and Xwayland's GPU time a frame
are the kernel's, on the `gfx` engine.

| What is drawn | Drawing | Frames in 10 s | A frame: median | 90th percentile | Longest | GPU a frame: Yuna, Xwayland | Other programs: cores |
|---|---|---|---|---|---|---|---|
| 5760 × 3240 to 3840 × 2160, area filter | GPU | 601 | 422 µs | 478 µs | 824 µs | 319 µs, 414 µs | 0.68, 3.44 |
| 3840 × 2160 copied at 1.5 | GPU | 598 | 246 µs | 326 µs | 5,104 µs | 138 µs, 303 µs | 1.78, 5.20 |
| 5760 × 3240 to 3840 × 2160, area filter | llvmpipe | 194 | 40,891 µs | 42,366 µs | 80,931 µs | | 1.00, 3.32 |
| 3840 × 2160 copied at 1.5 | llvmpipe | 301 | 14,943 µs | 15,526 µs | 16,232 µs | | 0.87, 3.10 |

- **A start costs an X11 program 29 ms on llvmpipe, 76 ms on the GPU**, by the medians, cold or
  warm alike: each Xwayland starts afresh. Yuna's own figure, from Xwayland's spawn to its
  manager owning `WM_S0`, is 24 and 73 ms of it; the other 2 to 5 ms run from the program's
  connection to the spawn, and from `WM_S0` to the program's reply. On the GPU, Xwayland starts
  glamor on the render node, which llvmpipe's shared memory spares it: the likely cause of the
  difference, not isolated. The longest warm starts, 66 and 227 ms, came in the phases where
  other programs once reached 5.8 and 11.5 cores over 2 s; that the two coincide is likely, not
  checked.
- **Xwayland holds 35 MiB on llvmpipe, 61 MiB on the GPU**, by its proportional set size, with
  one small window; 45 and 91 MiB resident. A proportional set size divides shared pages among
  the processes that map them: an earlier run beside other programs read 48 MiB on the GPU for
  the same resident size. On llvmpipe, Yuna's resident set grows by 37 MiB while
  Xwayland runs and keeps 22 MiB of it after Xwayland ends; on the GPU it grows by less than
  1 MiB ([debt](debt.md#xwayland)).
- **On the GPU, the area filter costs about 180 µs a frame more than the plain copy** at 4K:
  422 µs against 246 µs by the median, 319 µs against 138 µs of Yuna's GPU time; 4 % of a
  240 Hz period. Both drew every refresh at 60 Hz. ADR-0068's question, the filter's cost on the
  GPU, is answered for this machine. The kernel's GPU time a frame was the same in the runs that
  other programs disturbed: 319 µs and 137 µs.
- **On llvmpipe, the program's frames cost more than the test's below**, 40.9 ms against 22.1 ms
  for the area filter, most likely because Xwayland draws in shared memory there, and Yuna
  uploads its 75 MB buffer into a texture at each frame, which the test, drawing a texture it
  made once, does not; not isolated. The ratio to the copy, 2.7, is near the test's 3.3.
- Earlier runs the same day, beside a game and other sessions' builds, are not kept: they gave
  95 to 141 ms for a start on the GPU, and 1,096 µs against 775 µs for the area filter.

## M1.8 — The X11 element's area filter, on llvmpipe (2026-11-14)

Machine as in M1.4's B4 below: Ryzen 7 5800X (16 threads), Linux 7.3.1, Mesa 26.2.6 from the
development shell. Build: the test profile (debug), from commit `30b9999` with the X11 element's
changes not yet committed; the drawing is llvmpipe's, built for release by Nix. Two runs, under
the agents' exclusive lock; the table gives the first. Command:

```sh
nix develop -c cargo test -p yuna --lib --locked x11_element_drawing_time -- --ignored --nocapture --test-threads=1
```

The test `x11_element_drawing_time_at_4k` (`crates/yuna/src/render/x11_surface.rs`) draws a
3840 × 2160 output whole, 200 times after 10 to warm up, with one X11 window of noise, XWayland's
scale 1.5. A frame's time is `render::draw`'s: from the start of the drawing to the end of its
wait for the renderer, the background cleared included. Two cases on an output at 1: a window as
wide as X11 sees that output, 5760 × 3240 X11 pixels, which covers it, the heaviest a window gets;
and 3840 × 2160 X11 pixels, drawn at 2560 × 1440. Each is drawn through the area filter, then
through the renderer's own bilinear filter, Smithay's default shader, at the same size. The plain
copy is 3840 × 2160 X11 pixels on an output at 1.5.

| What is drawn | A frame: median | 90th percentile | longest |
|---|---|---|---|
| The background alone, at 1 | 1,558 µs | 1,694 µs | 1,904 µs |
| 5760 × 3240 to 3840 × 2160, area filter | 22,135 µs | 24,123 µs | 29,026 µs |
| 5760 × 3240 to 3840 × 2160, bilinear | 8,011 µs | 8,471 µs | 9,198 µs |
| 3840 × 2160 to 2560 × 1440, area filter | 15,875 µs | 16,366 µs | 17,485 µs |
| 3840 × 2160 to 2560 × 1440, bilinear | 4,729 µs | 4,937 µs | 5,181 µs |
| 3840 × 2160 copied at 1.5 | 6,743 µs | 7,289 µs | 12,517 µs |

- **On llvmpipe, the area filter costs about three times the bilinear filter**: 20.6 ms against
  6.5 ms for a window over the whole output, the background's 1.6 ms taken off, and 14.3 ms
  against 3.2 ms at 2560 × 1440. It reads up to 3 × 3 texels a pixel at a ratio of 2/3, each a
  sample of llvmpipe's bilinear filter, where the default shader takes one sample: most likely
  the cause, not isolated. A frame with a window over a whole 4K output at 1 takes longer than a
  60 Hz period on llvmpipe. Only an agent's seat with outputs of two scales pays it there; an
  agent's single output is at XWayland's scale, where its windows are copied.
- **The load**: before the first run, a load average of 1.80 over a minute (7.52 and 8.67 over 5
  and 15 minutes, from earlier builds), the processor 95.5 % idle; after it, 3.94, most likely
  the run's own, llvmpipe's 16 threads busy for its 17 s, and the processor 93.4 % idle. The
  exclusive lock kept other measurements out; other sessions' builds were not watched during the
  run.
- **The second run**, on the element as it is to be committed (its damage and tests changed
  since, not its drawing), gave every median within 2 % of the first: 21,785 µs and 7,974 µs
  over the whole output, 15,865 µs and 4,731 µs at 2560 × 1440, 6,701 µs copied, 1,543 µs for
  the background. It counts as disturbed: its load average was 5.11 over a minute, from the
  checks run just before it, though the processor was 98.9 % idle at its start.
- **Not measured on the GPU here**: tests never take the GPU, and no measurement program drew X11
  windows yet. `xwayland_time --area-filter` measures it since, in the section above.

## M1.7 — Motion (2026-11-14)

Machine and Mesa as in M1.4's B4 below. Build: release, commit `92fb627`. One run each, in
order, under the agents' exclusive lock, inside `agent-headless`; `--gpu` drew on the render
node. Command: `target/release/examples/motion_time` ([ADR-0056](adr/0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md)):
Yuna in the example's process on the real clock, six `vkcube`, four of them in columns as wide
as the output; 10,000 frames still, then 10,000 while a round of 14 operations repeats
(scrolls, a column moved and back, a workspace switch down and up, the special workspace in and
out, a scroll turned back after 30 ms), each started once the motion before ended. A frame
*missed* was presented after the boundary it aimed at. The machine's load was read each second
beside the runs, as [ADR-0060](adr/0060-development-first-agents-in-their-own-yuna.md) asks.

| Output, drawing, clients | Phase | Frames a second | Missed | A frame's time: median | p99 | longest |
|---|---|---|---|---|---|---|
| 3840 × 2160 at 240 Hz, scale 1.5, GPU, radv | still | 237.6 | 34 of 10,000, 0.34 % | 214 µs | 2,752 µs | 10,956 µs |
| | motion | 240.0 | 0 of 10,000 | 226 µs | 587 µs | 1,306 µs |
| 1920 × 1080 at 240 Hz, scale 1, llvmpipe, lavapipe | still | 233.6 | 190 of 10,000, 1.90 % | 2,206 µs | 4,538 µs | 22,056 µs |
| | motion | 149.4 | 5,755 of 10,000, 57.6 % | 4,108 µs | 7,694 µs | 36,022 µs |

- **On the GPU at 4K, ADR-0056's budget is met**: motion missed no frame, against 34 for the
  still output. The heaviest frame, the second after the special workspace shown, took 1.3 ms
  of a 4.17 ms period. A frame cost Yuna 201 µs of CPU and 116 µs of GPU in motion, against
  236 µs and 112 µs still.
- **The still phase on the GPU was disturbed**: another session's SlimeEngine tests kept the
  processor up to 74 % busy during it (10 to 44 % most seconds), against 2 to 7 % during the
  motion phase. Its 34 misses may owe to them; motion's zero meets the budget even counted
  alone.
- **On llvmpipe at 1080p, motion misses more than half its frames**, recorded without a pass
  mark, most likely because every client in view draws on the CPU while windows slide in (35 ms
  of client CPU a frame, against 14 ms still; not isolated by a run of its own), and llvmpipe's
  rasterizer and the `vkcube` on lavapipe share
  the 16 threads; the machine sat at 51 to 77 % busy, the runs' own load, quiet before. Misses
  fall 485 in a motion's first three frames, 5,270 elsewhere. Yuna's main thread took 2.7 ms a
  frame in motion against 1.0 ms still. This is motion's cost on llvmpipe, its cause not
  isolated ([debt](debt.md#motion)).
- **The two phases on the GPU did not run under the same load**, as ADR-0056 asks: during the
  still phase a link (`ld.mold`, 5 to 11 cores) and SlimeEngine's tests ran; the motion phase was
  quiet. The verdict holds, since motion missed none. The figures are of commit `92fb627`;
  `eed33e0` changed only frames drawn at once after rest, which the motion phase had none of.

### The layout and the motion, on one core

`taskset -c 6`, release, under the exclusive lock, the load read beside each run. The layout's
`timing` example ([M1.2](#m12--the-layout-model-2026-11-11), its command below) and `yuna-motion`'s
`motion_timing` (`crates/yuna-motion/examples/motion_timing.rs`). Medians and 99th percentiles
in microseconds.

**What M1.7 cost the layout**, two runs each, back to back, of main before M1.7 (`da44c97`) and
of `b14d618`, at a load average of 4 to 5: in scrolling columns `arrange` at 100 windows went
from 6.9 to 8.6, focusing from 10.2 to 12.8; dwindle 10 to 15 % more. Sizing the arrangement's
lists once and a column id that is never 0 (`769ce2d`) gave it back: 7.0 and 7.1 for `arrange`
in scrolling columns, dwindle as main's.

**The layout at `769ce2d`**, one run, the processor 3 % busy beside it, load average 1.2 to 3.7:

| Operation, then `arrange` | Windows | Scrolling: median | p99 | Dwindle: median | p99 |
|---|---|---|---|---|---|
| Arrange alone | 100 | 6.95 | 7.83 | 7.71 | 9.96 |
| Focus left, then right | 100 | 10.5 | 14.2 | 10.8 | 14.7 |
| Open a window, then close it | 100 | 12.6 | 16.2 | 14.4 | 15.7 |
| Move the focused window, then back | 100 | 10.6 | 13.8 | 10.9 | 13.9 |
| Fullscreen on, then off | 100 | 12.4 | 15.5 | 8.05 | 8.71 |
| Float, then tile | 100 | 18.1 | 19.1 | 20.5 | 23.9 |
| Switch the tiling, then back | 100 | 12.5 | 13.8 | 9.68 | 12.7 |
| Arrange alone | 1,000 | 75.4 | 86.6 | 80.3 | 88.1 |
| Float, then tile | 1,000 | 181 | 194 | 203 | 220 |

Every row meets [ADR-0020](adr/0020-the-layout-models-shape.md)'s budgets, 25 µs at 100
windows and 300 µs at 1,000. Under a load average of 4 to 5, "float, then tile" in dwindle
reached 25.1 to 25.4 µs at the 99th percentile, as main's own code did, 24.4 to 25.3
([debt](debt.md#the-layout-model)).

**The motion at `769ce2d`**, the same run's conditions; each update one 240 Hz refresh after
the last, interrupting the motion it started:

| What is timed | Windows | Median | p99 |
|---|---|---|---|
| `Motion::update`: focus 2 columns left, then back | 100 | 29.9 | 50.9 |
| `Motion::update`: open a column left, then close it | 100 | 40.5 | 57.8 |
| `Motion::frame`, while a column opens | 100 | 4.7 | 6.9 |
| `Motion::update`: focus 2 columns left, then back | 1,000 | 317 | 377 |
| `Motion::update`: open a column left, then close it | 1,000 | 435 | 517 |
| `Motion::frame`, while a column opens | 1,000 | 48.5 | 75.5 |

An update runs once per operation, beside the layout's; a frame's sampling once per refresh,
about 5 µs of a 4.17 ms period at 100 windows. No budget is set for them yet: M1.9 sets the bar's
budgets from the rivals'. "Float, then tile" could not be profiled: the agents' isolation
refuses `perf` its events.

## M1.6 — mpv in the applications' tests (2026-11-14)

On GitHub's `ubuntu-24.04` runner, for commit `f9f32bb`: CI's one job took 3 min 39 s, against
2 min 41 s at M1.5's end; mpv's 348 MB come with it. The applications' tests took 32.6 s there
(commit `71cfee2`).

Machine and Mesa as in M1.4's B4 below, load average 1.5 to 3.6. Build: debug, mpv's
two tests (commit `32bfc5a`) on `e04dee7`. Command: `nix develop -c cargo test -p yuna --test applications`, with
and without `-- --skip mpv`, on all 16 threads and on two cores (`taskset -c 0,1`), run with
`run.sh --exclusive` (its proof does not record the lock), outside `agent-headless`, since `yuna mcp` and `yuna --headless`
open no display. The figure is libtest's own time for the whole file.

| What | Runs | Without mpv → with |
|---|---|---|
| The applications' tests, all threads | 3 + 3 | 16.43 s → 16.45–16.46 s |
| The same on two cores, where libtest runs two tests at a time | 2 + 2 | 31.4–31.7 s → 47.3–47.6 s |
| mpv's two tests alone, two cores | 1 | 16.4 s |
| Both mpv cases in one test, run at once, on two cores (measured, not kept) | 2 | 32.3 s |
| What mpv 0.42.0 adds to the shell | 1 | 348 MB, 52 store paths |

- Each mpv test takes about 16 s: mpv shows its 24 frames, Yuna drew 23 to 27 frames from 69
  to 81 commits, then 2 s of quiet, the read, mpv's exit, 2 s of quiet and 10 s of rest, with
  no context switch in Yuna's 35 threads.
- On all threads they run beside the others and add nothing measurable. On two cores they add
  16 s; in one test, both at once, they would add 1 s. CI's own time comes with the first push.

## M1.6 — Presentation times under emulated VRR (2026-11-14)

On the manual clock, the VRR stories (`crates/yuna/tests/headless/vrr.rs`) pin the emulation on a
1920 × 1080 output of 48 to 240 Hz, after six frames of warm-up from a pause of 1 s: at 100 fps,
every presentation 10 ms after the last and one refresh later; at 300 fps, none sooner than
1/240 s, the frames that could not be shown discarded; at 24 fps, 41.667 ms apart within 1 µs,
three refreshes later each, as amdgpu's BTR inserts them; at 23.976 fps, at most 2 of 41
intervals a whole number of 1/240 s periods, against all of them on a fixed 240 Hz output.
Checked against the owner's monitor ([ADR-0046](adr/0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md),
[ADR-0051](adr/0051-emulated-vrr-as-the-monitors-traces-showed.md)): the owner ran the throwaway
`vrr-trace` twice from a text console on 2026-11-14, Linux 7.3.1, mode 3840 × 2160 at 240.016 Hz,
VRR on: steady rates from 23.976 to 300 fps while other builds loaded the machine, then probes
on a quiet machine, frames late and early by 0.5 to 16 ms, gaps and changes of rate. Reduced, they
are `crates/yuna/tests/fixtures/vrr-trace-pg32ucdm*.txt`. Per frame, the monitor showed 3
refreshes at 24, 25 and 30 fps, 2 at 48, one from 60, at most 240 Hz above; under BTR, the first
`n − 1` of a fixed length (13.9 ms at 24 fps), the last variable; after a phase's last frame, 20.83 ms refreshes (12.5 ms
during the idle seconds that open each run, a state the console left). A flip
came 0.5 ms after its submission when nothing held it (0.5 to 0.95 ms), 1.2 ms at 240 fps and
3.0 ms at 300, held by the flip before. After the panel's own refresh, a frame waited 7 to 12 ms
more. Replayed through `vrr.rs`, every frame, the panel set back on the monitor's history after
each: a frame's start within 500 µs of the monitor's for 91 % of the steady rates' 8,319 frames
and 82 % of the probes' 7,330; its refresh's number for 96 % and 94 %. The model before the
traces gives 63 % for the steady rates ([debt](debt.md#vrr)).

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
