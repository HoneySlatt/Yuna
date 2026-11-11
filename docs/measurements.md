# Measurements

What the benchmarks and the measurements by hand found, against the budgets of the quality bar in
[`ROADMAP.md`](ROADMAP.md#3-quality-bar). One section per milestone or machine, newest first. A
section names the machine, the build, the command, and the figures with their spread.

## M1.2 — The layout model (2026-11-11)

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
