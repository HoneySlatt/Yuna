# 0118. Frames, presentation times and the clock

- Status: Accepted; supersedes [0023](0023-frames-presentation-and-the-clock.md),
  [0031](0031-frames-drawn-ahead-of-their-boundary.md),
  [0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md) (motion's frames, what a
  client is told) and [0058](0058-what-yuna-takes-from-the-owners-other-projects.md) (hidden
  games, `fifo-v1` and `commit-timing-v1`)
- Date: 2026-11-18

## Context

When Yuna draws a frame, what it reports and what it tells clients was decided in five ADRs.
[ADR-0023](0023-frames-presentation-and-the-clock.md) drew frames on damage, at a boundary, on a
clock tests control. [ADR-0031](0031-frames-drawn-ahead-of-their-boundary.md) drew them ahead of
their boundary. [ADR-0046](0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md) gave a VRR
output its own refreshes. [ADR-0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md)
sampled motion at presentation and told clients apart from what is drawn.
[ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md) point 2 kept hidden games
drawing. The ROADMAP cited ADR-0023 and ADR-0031 alone.

This ADR gathers the rule as it stands on 2026-11-18. It decides nothing new. A virtual output has
no vertical blank: Yuna decides when it refreshes, and the quality bar reads motion and input to
present from presentation times read headless, so those times must be true. A VRR output's
refreshes are [ADR-0119](0119-emulated-vrr-as-amdgpu-and-the-owners-monitor-do.md)'s.

ADR-0023 drew at the boundary and presented a boundary later; ADR-0031 replaced it. ADR-0023 gave
every hidden window one callback a second; ADR-0058 excepted games. The later rule is the one below.

## Decision

- **Frames follow damage** (ADR-0023, 2026-11-12). A client's commit, or a change of the layout,
  arms its output's next deadline; Yuna draws what changed there. A deadline is armed only while
  something waits: at rest, none is. The layout is arranged once, at the first output's refresh
  after it changed, not at each commit (ADR-0023; ADR-0031, 2026-11-13).
- **An output's next deadline** (ADR-0031): **at once** when the output is idle, at the end of the
  round that read the commit; otherwise at **b(k) − lead**, for the first boundary k after the
  last one a refresh aimed at whose deadline is not yet past.
- **Idle** means no frame waits for its boundary, and the boundary after the output's last refresh
  has passed (ADR-0031). A refresh that drew nothing counts, so that a client asking frame
  callbacks without damage is answered once a period, never in a loop. An output's creation counts
  as a refresh at boundary 0.
- **The lead** is the longest drawing of the last 0.5 s, plus a margin, at most one period
  (ADR-0031). A drawing is timed from its deadline to its end, at least a floor. Floor and margin
  start at KWin's, 2 ms and 1 ms, and are set from the miss rate measured headless
  ([measurements](../measurements.md)).
- **A presentation time is when the frame could have shown** (ADR-0023; ADR-0031): the boundary it
  aimed at if its drawing ended by then, else the first boundary after, and the frame counts as
  missed. The sequence number counts boundaries. Times are `CLOCK_MONOTONIC`.
- **Frame callbacks go out right after the refresh**, stamped with its end. **Presentation
  feedback** is taken when the frame is drawn, held by its output, and sent at its boundary,
  through a deadline of its own (ADR-0031).
- **The clock owns every deadline** (ADR-0023). In the program, it is the monotonic clock, and it
  arms one timer at the earliest deadline. In tests, time moves only when the test advances it,
  and every deadline passed then runs, in order; MCP steps the same clock. Virtual time starts at
  the real monotonic time and never runs ahead of it while real clients are connected.
- **Each frame samples every motion at the boundary it aims at** (ADR-0054, 2026-11-14). A frame
  drawn at once on an idle output samples the boundary it can reach after its expected drawing, at
  most one period ahead. While a motion runs, each frame samples a later time than the one before.
  A motion starts at the refresh that arranges the change. While a motion of an output is
  unfinished, each refresh arms the next, whether it drew, found nothing changed, or failed; the
  last frame shows the target exactly, then nothing is armed.
- **What is drawn follows the motion; what a client is told follows its place** (ADR-0054). A
  window is told it is visible (`suspended` cleared, `enter`) when its place enters the output,
  and hidden only when its output's motions have ended with neither its place nor its drawn
  rectangle on it. A window that only passes through the view is drawn from its last buffer and
  told nothing; each window gets at most one such change per operation. Frame callbacks go at full
  rate to every window a frame draws, told or not. The pointer and clicks hit-test what is drawn,
  at each pointer event; between events, the pointer's focus is refreshed once when its output's
  motions end. Popups follow their window's drawn rectangle; keeping them on the output uses its
  place.
- **Hidden windows** get at most one frame callback a second and xdg-shell's `suspended` state
  (ADR-0023, the owner's choice on 2026-11-12). Hidden means on a workspace not shown, or in a
  column scrolled out of view, by the window's place. Windows on an agent's output are shown, at
  full rate ([ADR-0116](0116-agents-seats-each-agent-in-its-own-yuna.md)).
- **A hidden game keeps drawing** (ADR-0058, 2026-11-14). A game, recognised by `content-type`, by a
  rule of the configuration or by being fullscreen, is paced while hidden at a rate set in the
  configuration, 60 frames a second by default, up to its output's full rate.
- **`fifo-v1` and `commit-timing-v1` come at Stage 2** (ADR-0058), each adding its "not before"
  to the same deadline (ADR-0031), as blockers of a surface's queue of commits
  ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 1).
- **The DRM backend** (Stage 2) adds the mode's vertical blank to the margin, renders before
  committing, at the commit's own deadline, and times a drawing by its fence, since the main thread
  must not wait on the GPU (ADR-0031).

## Alternatives considered

- **A free-running timer at the output's rate**
  ([ADR-0023](0023-frames-presentation-and-the-clock.md)): it wakes 240 times a second at rest.
- **Presentation at the boundary where drawing starts**
  ([ADR-0023](0023-frames-presentation-and-the-clock.md)): it never shows a missed frame.
- **A clock that only says the time** ([ADR-0023](0023-frames-presentation-and-the-clock.md)): a
  test could move time but fire nothing.
- **No callback for hidden windows**, as Hyprland
  ([ADR-0023](0023-frames-presentation-and-the-clock.md)): games and Vulkan clients that block on
  their next frame freeze.
- **Drawing at once at every commit**, as niri, or **at the deadline alone**, as KWin
  ([ADR-0031](0031-frames-drawn-ahead-of-their-boundary.md)): an animating client shown two
  boundaries later, or an isolated key paying the whole lead.
- **A lead without a cap, or set by hand**
  ([ADR-0031](0031-frames-drawn-ahead-of-their-boundary.md)): one slow drawing delaying every frame
  for half a second, or a lead right for one machine and one load.
- **Feedback sent right after drawing** ([ADR-0031](0031-frames-drawn-ahead-of-their-boundary.md)):
  a client waiting for presentation released before the frame shows.
- **Telling clients from the drawn rectangle**
  ([ADR-0054](0054-motion-sampled-at-presentation-drawn-apart-from-told.md)): a window crossing an
  edge would get 10 to 20 configures a second.
- **A hidden game at 30 frames a second by default**
  ([ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)): a game that ties its
  simulation to its frames would run at half speed.

## Consequences

- A test runs the frame loop to an exact time and reads what was drawn and presented.
- A hidden window that keeps asking for frames wakes Yuna once a second; a game played in a window
  with no `content-type` and no rule falls to that rate when hidden, and the rule is its remedy.
- At 240 Hz, estimated: an isolated commit is presented about 2.3 ms after it, an animating
  client's about one period after. Each animated frame costs three wake-ups: the commit, the
  deadline and the boundary.
- Two latch points coexist, at once when idle and b − lead when not; tests pin a steady client's
  cadence, alone and beside an animating one. At a low rate, a commit that just missed a deadline
  waits up to two periods, 66 ms at 30 Hz.
- On llvmpipe at 4K, where a drawing takes longer than a period, frames are presented late, and the
  presentation times say so.
- The output's buffer holds the last frame drawn, which may not be presented yet.
- During a motion, the pointer's focus may name a window that has slid from under it until the
  pointer moves or the motion ends; a click always reaches what it shows.

## Validation and reconsideration

Evidence: the frame loop's tests on the manual clock, and the miss rate measured headless that sets
the lead's floor and margin ([measurements](../measurements.md)). The figures at 240 Hz are
estimated, not measured.
