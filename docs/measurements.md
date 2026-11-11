# Measurements

What the benchmarks and the measurements by hand found, against the budgets of the quality bar in
[`ROADMAP.md`](ROADMAP.md#3-quality-bar). One section per milestone or machine, newest first. A
section names the machine, the build, the command, and the figures with their spread.

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
