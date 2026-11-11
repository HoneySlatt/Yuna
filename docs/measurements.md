# Measurements

What the benchmarks and the measurements by hand found, against the budgets of the quality bar in
[`ROADMAP.md`](ROADMAP.md#3-quality-bar). One section per milestone or machine, newest first. A
section names the machine, the build, the command, and the figures with their spread.

## M1.2 — The layout model (2026-11-11)

What one operation of the layout model costs, followed by `arrange`, which a compositor calls to
draw the result: what a key press costs the model. No budget of the bar names it; it is measured
against the frame at 240 Hz, 4.17 ms on the high tier.

Machine: the high tier's desktop, Ryzen 7 5800X, the program pinned to one core (`taskset -c 6`),
with the owner's consent. Another project's fuzzer ran on another core at times; the load average
was 3.1 to 3.3. Build: the release profile, Rust 1.99.1, the code of the commit that adds this
section's crate. Command: `nix develop -c cargo run --release -p yuna-layout --example timing`,
three runs of 5,000 samples per row; the 4K output at scale 1.5, the middle window focused.

Medians and 99th percentiles in microseconds, the lowest and highest of the three runs:

| Operation, then `arrange` | Windows | Scrolling: median | p99 | Dwindle: median | p99 |
|---|---|---|---|---|---|
| `arrange` alone | 100 | 3.7–4.2 | 5.4–6.3 | 4.4–4.7 | 4.7–5.8 |
| Focus left, then right | 100 | 4.2–4.4 | 4.3–6.1 | 7.4–7.8 | 10.1–10.6 |
| Open a window, then close it | 100 | 4.4–4.6 | 6.3–6.8 | 10.8–11.3 | 11.5–15.8 |
| Move the focused window | 100 | 4.1–4.3 | 6.2–6.7 | 7.3–7.7 | 7.7–10.4 |
| Fullscreen on, then off | 100 | 4.9–5.2 | 5.2–8.0 | 5.5–5.8 | 5.7–6.1 |
| Float, then tile | 100 | 7.1–7.3 | 8.5–11.6 | 14.3–14.8 | 17.0–18.7 |
| Switch the tiling and back | 100 | 11.4–12.0 | 14.8–20.8 | 11.0–11.2 | 15.4–15.7 |
| `arrange` alone | 1,000 | 34–36 | 54–56 | 44–45 | 52–60 |
| Open a window, then close it | 1,000 | 40–41 | 48–64 | 100–103 | 115–143 |
| Float, then tile | 1,000 | 64–65 | 75–103 | 130–136 | 145–185 |
| Switch the tiling and back | 1,000 | 115 | 140–209 | 116–118 | 136–195 |

At 100 windows, every operation stays under 21 µs at the 99th percentile, half a percent of a
frame at 240 Hz. Ten times the windows cost about ten times the time: the model is linear. The
first measure was not: `arrange` cost 490 µs at 1,000 windows, and switching the tiling 2.4 ms,
from searches repeated for every window; both were fixed before these figures.

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
