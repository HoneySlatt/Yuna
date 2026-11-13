# 0032. Input to present, read by mode

- Status: Accepted; amends the [ROADMAP](../ROADMAP.md#budgets)'s budget "Input to present"
- Date: 2026-11-13

## Context

The ROADMAP's budget "Input to present" reads: "at the 99th percentile, one frame, 4.17 ms on the
high tier", measured with input given through the compositor and presentation times read.

At a fixed refresh rate, a frame waits for the boundary grid. Input that comes at any moment
waits between nothing and a whole period before its frame can show: over 4.13 ms one time in a
hundred at 240 Hz. The client's own time and Yuna's drawing come on top. No compositor, gamescope
included, meets the figure read so at a fixed rate. Under VRR or with tearing there is no grid,
and one frame end to end is within reach. [ADR-0031](0031-frames-drawn-ahead-of-their-boundary.md)
decides when Yuna draws; this ADR decides what its budget measures.

## Decision

The owner chose, on 2026-11-13, to read the budget by mode:

- **At a fixed rate**, Yuna's own share is a count. No commit is presented after the first
  boundary it could reach under ADR-0031: the boundary its deadline aimed at, or the first after
  its drawing when the output was idle. The count is read from the presentation feedback and the
  frame log, and must be zero. Beside it, input to present end to end at the 99th percentile is
  recorded against gamescope and KWin on the same client. Its figure is set at M1.9 with the
  other budgets.
- **Under VRR or with tearing** (Stage 4): one frame end to end, 4.17 ms on the high tier, as
  before.

## Alternatives considered

- **Keeping the letter**: a budget missed by construction at a fixed rate stays debt for ever,
  and says nothing.
- **Against the rivals alone**: simple, but it cannot tell Yuna's share from the client's, and it
  waits for M1.9.

## Consequences

- Two readings instead of one; each can be met, and each says where a miss comes from.
- The count tests the lead's accuracy: a lead too short shows as missed frames, one too long
  as latency in the end-to-end figure.
- The headless figures are read on the real clock: on the manual clock, drawing takes no time
  unless a test makes it.
