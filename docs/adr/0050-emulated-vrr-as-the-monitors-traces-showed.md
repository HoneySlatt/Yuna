# 0050. Emulated VRR, as the monitor's traces showed

- Status: Accepted; amends [0045](0045-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)
  (the panel's rules, what the replay checks)
- Date: 2026-11-14

## Context

[ADR-0045](0045-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md) emulates amdgpu's VRR from
a reading of `freesync.c`, to be checked against a trace of the owner's monitor. The owner
recorded two, from a text console, Linux 7.3.1: one of steady rates, while other builds loaded
the machine; one of probes on a quiet machine, frames late and early by 0.5 to 16 ms, gaps and
changes of rate ([measurements](../measurements.md)).

- Under BTR, only the first `n − 1` of the `n` refreshes amdgpu inserts have their fixed
  length; the last is variable again. A frame late within it is shown at once, as ADR-0045
  says. The first trace's late frames, held about as long again as they were late, came from
  the loaded machine: the quiet trace does not show it.
- A flip shows a vertical blank after it is submitted at the soonest, 0.5 ms; one that comes
  less than that before a refresh begins misses it.
- After the panel's own refresh, when no frame came for 1/min, the driver holds a new frame 7
  to 12 ms more. amdgpu's commit path waits there; the code read does not say for how long.
- `freesync.c` also has a workaround that holds the top rate after flips close to a vertical
  update, and defers its writes of the refresh's range to a worker that waits on the flip's
  lock: neither is emulated.

## Decision

The owner chose, on 2026-11-14, to model the late frames before closing M1.6, then to close it
with these rules once the probes showed what was understood and what was not:

- **The panel's rules**: under BTR, the first `n − 1` inserted refreshes have a fixed length,
  and a frame ready during one shows at its end; the last is variable, from the shortest
  refresh to the longest. A frame shows a vertical blank after it is ready at the soonest: the
  owner's mode's 269 of 2,429 lines, 461 µs at 240 Hz.
- **A refresh that draws nothing is paced**, a shortest refresh after the one before, so that
  callbacks asked without damage never loop; something new to draw, a buffer, damage or a
  change of the layout, is never paced by it.
- **The replay checks every frame of both traces**, the panel set back on the monitor's
  history after each, against a tolerance of 500 µs, the spread of the kernel's commit path the
  traces show.
- **What is not modelled is debt**, to take up at Stage 4, with games' VRR: the hold after the
  panel's own refresh, BTR's count after a perturbation, and the workaround above.
  [ADR-0014](0014-vrr-without-flicker.md)'s band keeps games above the panel's floor, where
  these do not arise.

## Alternatives considered

- **Simulating amdgpu's commit path** event by event (vertical updates, the deferred worker, the
  commit's waits), to model the remaining 12 % of frames: more traces and more reading, for
  regimes Yuna's own policy will avoid.
- **Closing on the first trace alone**, the late frames left as debt: a rule recorded that the
  quiet trace showed wrong.

## Consequences

- 91 % of the steady rates' frames and 82 % of the probes' start within 500 µs of the monitor's;
  96 % and 94 % at the monitor's refresh number.
- A VRR output's frames are presented a vertical blank after their drawing ends.
- Both traces, reduced, are fixtures of `vrr.rs`; a new kernel or monitor needs new traces.
