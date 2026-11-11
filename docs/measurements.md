# Measurements

What the benchmarks and the measurements by hand found, against the budgets of the quality bar in
[`ROADMAP.md`](ROADMAP.md#3-quality-bar). One section per milestone or machine, newest first. A
section names the machine, the build, the command, and the figures with their spread.

## M1.1 — The workspace and its checks (2026-11-11)

No budget of the bar applies yet; these figures are the base later milestones compare against.

Machine: the high tier's desktop, Ryzen 7 5800X (16 threads), the repository on a local NVMe drive.
Build: the debug profile of M1.1's workspace. Command: `nix develop -c scripts/check.sh`, timed from
outside, the development shell already built and the crates already downloaded.

| What | Runs | Measured |
|---|---|---|
| The check, cold: `target/` emptied by `cargo clean` | 4 | 11.1 to 15.3 s, median 13.1 s |
| The check, warm: nothing changed | 5 | 1.1 to 1.3 s, median 1.2 s |
