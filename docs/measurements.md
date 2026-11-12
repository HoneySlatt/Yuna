# Measurements

What the benchmarks and the measurements by hand found, against the budgets of the quality bar in
[`ROADMAP.md`](ROADMAP.md#3-quality-bar). One section per milestone or machine, newest first. A
section names the machine, the build, the command, and the figures with their spread.

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
