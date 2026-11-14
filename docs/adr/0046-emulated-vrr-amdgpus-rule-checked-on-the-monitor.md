# 0046. Emulated VRR: amdgpu's rule, checked against the owner's monitor

- Status: Accepted; amends [0023](0023-frames-presentation-and-the-clock.md) (a VRR output's
  refreshes), [0031](0031-frames-drawn-ahead-of-their-boundary.md) (no deadline at the floor);
  amended by [0051](0051-emulated-vrr-as-the-monitors-traces-showed.md) (the panel's rules, the
  replay)
- Date: 2026-11-14

## Context

M1.6 makes a virtual output behave as the owner's monitor does under VRR. The monitor, a
PG32UCDM on an RX 6900 XT, announces 48 to 240 Hz in its EDID. Read on 2026-11-14 in Linux master
`97d73c82`, whose rule is the same as in 6.17 (`drivers/gpu/drm/amd/display/modules/freesync/freesync.c`):

- A refresh lasts at least 1/max (4.17 ms) and at most 1/min (20.8 ms). A flip that comes
  sooner waits; a later one ends the refresh at once.
- Below the range, with a range of at least 2×, amdgpu inserts refreshes (*BTR*): from the time
  between the last two flips it picks n refreshes of equal length near the range's middle, with
  hysteresis. A film at 24 fps gets 3 refreshes of 13.9 ms, 30 fps 3 of 11.1 ms, 48 fps 2 of
  10.4 ms. An inserted refresh has a fixed length: a flip that comes during one waits for its
  end, a rule read in the code, not written anywhere. With a range under 2×, another rule holds
  the panel at its top rate after five slow frames.
- KWin, Mutter, niri and gamescope aim a frame at the last presentation plus 1/max, and report
  the page flip's time. Mutter and gamescope hold a floor for some content; none limits how fast
  the rate changes.
- Aerith, the owner's video client, tells VRR from presentation intervals alone: at a fixed rate
  they are whole numbers of refreshes.

## Decision

The owner chose, on 2026-11-14:

- **A virtual output may run under VRR**, over a range [min, its rate], with its rate at least
  twice the minimum. It emulates amdgpu's DisplayPort Adaptive-Sync: the refresh's least and
  greatest length, BTR as `freesync.c` computes it (n, its drift margin, its hysteresis), and
  the fixed length of inserted refreshes. Other paths of the driver are not emulated.
- **A frame is presented at the first refresh it can reach**: when its drawing ends, unless the
  refresh in progress is shorter than 1/max or is an inserted one; then at its end. Refreshes the
  panel makes by itself are computed when needed, never armed as timers: an output at rest wakes
  nothing. Its feedback counts every refresh in `seq`, gives 1/max as `refresh` and the VSYNC
  flag, as `wp_presentation` version 2 allows.
- **The frame loop aims at the earliest refresh a frame can reach**, minus the output's lead
  ([ADR-0031](0031-frames-drawn-ahead-of-their-boundary.md)), and draws at once when that
  refresh could begin now. A frame is missed when it is presented after the refresh it aimed at.
- **The emulation is checked against the monitor.** A throwaway program, which the owner runs from
  a text console with the monitor's VRR on, flips at steady rates from 23.976 to 300 fps and
  records the kernel's flip times. The trace, with its kernel's version, becomes a test fixture:
  Yuna's emulation replays it within a tolerance set from the trace's own jitter. The program is
  then deleted, as M1.3's was.
- No policy turns VRR on or off yet: an output is created with it or without it. ADR-0014's
  policy is Stage 4's.

## Alternatives considered

- **The same emulation without the trace**: nothing for the owner to run, but "as on the owner's
  monitor" would rest on a reading of driver code, and on rules no source writes.
- **A simple panel that repeats at its minimum**: less code, but a 24 fps film lands where a
  repeat and its next frame collide, and its intervals jitter where the monitor's do not.
- **ADR-0014's policy now** (fixed desktop, the games' band, films at multiples): Stage 4's and
  Stage 5's work, which needs games and films recognised.

## Consequences

- Each output keeps the times of its recent refreshes instead of a fixed grid alone; the frame
  loop, captures and presentation read both kinds of output through one interface.
- The trace needs the owner's session at the monitor, and the monitor's VRR on for its length:
  it may flicker meanwhile.
- `wp_fifo_v1`, `wp_commit_timing_v1`, `wp_tearing_control_v1` and presentation-time version 3,
  with its `variable_rate` flag (not in wayland-protocols 0.32.13), are not offered yet: games'
  stage.
- A kernel that changes `freesync.c` may change the monitor's behaviour; the fixture names its
  kernel, and a new trace replaces it.
