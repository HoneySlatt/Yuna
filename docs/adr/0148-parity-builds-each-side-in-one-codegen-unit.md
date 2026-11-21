# 0148. Parity builds each side in one codegen unit

- Status: Accepted
- Date: 2026-11-21

## Context

[ADR-0142](0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)'s parity script
builds the base and the head for release, in the release profile's sixteen codegen units, and
compares, among its figures, the instructions a frame of the `yuna` binary, at a threshold of
0.6 %: three times the spread of runs of one build. The spread of two builds whose sources hardly
differ was never measured.

At M2.2's closing review, CI's parity found the `input` scenario's own instructions worse than the
baseline, +1.09 %, twice, though no change touched the pointer's path. Measured on the desktop on
2026-11-21 (`.delegation/` local, `parity-work`): `8e6dce1` against a head with the review's fixes,
+2.14 %, 565,886 to 577,989 instructions a frame. Callgrind's functions showed where: generic code
of `serde_json` and of `BTreeMap`, MCP's reading and writing of messages, inlined in one build and
called in the other, its source unchanged. A change in one file of a crate moves the crate's code
between its codegen units, and with them what LLVM inlines elsewhere. `input` feels it most: its
nine MCP calls a frame make MCP's JSON most of its own instructions. The same two trees built in
one codegen unit each: 541,964 and 542,024, +0.01 %.

Every milestone of Stage 2a rewrites Yuna's code, so every one would meet it.

## Decision

The owner chose, on 2026-11-21: **the parity script builds both sides in one codegen unit**
(`CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1`), the release profile of the shipped build unchanged. Its
figures then judge the sources' cost, not where the compiler split them. The baseline does not
move: the head's script builds the base the same way, so each run compares two builds made alike.
The thresholds, set from spreads of sixteen-unit builds, are measured again on one-unit builds at
the owner's go.

## Alternatives considered

- **One codegen unit for the release profile itself**: the parity script would measure the very
  binary shipped, and Yuna ran about 4 % fewer instructions; but every release build, the
  package's and CI's included, would be slower, and the shipped binary would change, its own
  measure to make.
- **A wider threshold for the scenario**: simple, but a real loss of 2 % in Yuna's code would pass.

## Consequences

- Each side's build takes longer: one unit compiles on one thread at the end of the build.
- The binary the parity script measures differs from the one shipped by its codegen units; its
  figures speak of the sources' cost, the shipped binary's own costing measured apart (the timed
  tier).
- The figures of every run made before this change were of sixteen-unit builds: they are not
  compared with those made after.

## Validation and reconsideration

Evidence: the two runs of 2026-11-21 above and CI's runs 44858859112 and 44862984369. Checked by
CI's parity at each push. Proposed again if a change elsewhere moves a one-unit build's own
instructions past their threshold again, or if the builds' time costs the job its bound.
