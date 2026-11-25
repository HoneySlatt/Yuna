# 0158. A frame's identity, and each task's wake through a source of its own

- Status: Accepted; supersedes [0155](0155-a-frames-identity-an-opaque-id-numbered-rings-and-a-frames-tool.md)
- Date: 2026-11-25

## Context

[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 11 asks
for a frame's identity from input to presentation, and each wake named by its source.
[ADR-0135](0135-priority-against-a-game-amds-master-override-turned-on-at-2b-on-a-measure.md) asks
for the GPU priority granted in the frame's log. ADR-0155 decided how. M2.4d, building it at
`d709896`, found facts that change parts of it; this ADR states the rule whole.

- **No task of the table uses a `Waker`.** The X11 manager wakes Yuna through its channel, named
  `x11_manager` with its seat; the encoders through theirs, named `encoders`. The one holder of a
  `Waker` is WLCS's shim thread, a test seam outside the table
  ([ADR-0152](0152-the-table-of-tasks-one-spawner-the-x11-manager-inline-at-m2-17.md)).
- **MCP's `pointer` and `keyboard` see no request id**, and a field added to their results costs
  an allocation a call: the parity scenario `input` makes 8 such calls a period, against an
  allowance of half an allocation a frame
  ([ADR-0142](0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)).
- **A window's opening arms every output of every seat.** Naming its client's commit as the cause
  everywhere would name it in other seats' frames.
- **A commit without a buffer can still change what is drawn**: a subsurface moved, a viewport.
  Counting only commits that attached a buffer would leave such frames without a cause.
- **An output can be armed again inside its own refresh**, by the arrangement the refresh runs:
  the clock disarms a deadline before running it.
- **An X11 client's identity is in its windows' ids**: the X server gives each client a range, its
  bits above the resource mask. A window's record may not exist yet when its next events come.

## Decision

The owner chose, on 2026-11-22 (ADR-0155) and on 2026-11-25 (the wakes of tasks):

- **`FrameId`, an opaque 64-bit number**, taken when an output's refresh is armed while none is.
  It is kept when the refresh is aimed again, sooner for new content or under VRR. A refresh
  takes its id out as it starts, so that one armed during it gets another. The boundary aimed at,
  the time sampled and the boundary presented are fields of the frame, not its identity. At 2b
  the same number goes into the KMS commit's user data. A capture reports the id of the frame it
  copied.
- **Two numbered rings and a ring of frames**, in `yuna` until `yuna-core` exists, one set per
  seat, each of fixed size made with the seat, bounded in bytes, written in place: no allocation
  once the seat is made.
  - Inputs, each with its time, its kind, and its device: the compositor's interface, Yuna itself
    (a pointer put on what is under it after windows moved), a client's virtual keyboard or
    pointer, or an agent's MCP call, named by the JSON-RPC id the agent chose when it is an
    integer. Each delivery names its device explicitly. Motions of one device that no frame has
    covered yet merge into one entry, counted.
  - Commits, each with its client, its surface, its window and its output, and whether it
    attached a buffer.
  - Frames, each with its id, output, outcome, causes, the ranges of input and commit numbers it
    covers, when it was armed, ran and ended, the boundaries it aimed at and was presented at.
- **A `frames` tool in MCP** reads the caller's seat's frames after a cursor, of one output when
  asked, with `missed`, as `events` does. A reply is bounded by the entries it lists. A
  presentation is told once the clock reached it; a frame whose output was removed first is told
  discarded. The `frame drawn` log line carries the id and the causes.
- **The causes become exact.**
  - A window's first buffer counts as its client's commit, on its seat's outputs; other seats'
    outputs arrange again with no cause.
  - A commit that attached a buffer is a cause. One that attached none is a cause only of a frame
    drawn with no other cause.
  - Hidden windows wait in a set ordered by when each is due.
  - If a parity scenario's causes move, the baseline moves in a commit that names them.
- **Wakes named fully.**
  - Each task of the table wakes Yuna through an event source of its own, which names it:
    `Task::wake` gives every task's insertion its wake, and a test checks that no two tasks share
    a source and none uses `waker`. A `Waker` stays the test seams' alone.
  - An X11 wake names the X11 client behind it, from its windows' ids and the pid the manager
    reads when the window is created.
- **The GPU priority granted** is read once when the renderer is made, logged, and given by
  `frames`.
- **The GPU's timestamps come with the Vulkan renderer** (M2.8), which leads its deadline from
  them. The lead's floor and margin stay until then. Each late frame is named by its record
  meanwhile.
- **Input to frame is recorded in order, not as causality.** A frame covers the seat's inputs and
  commits recorded after its output's previous frame, up to its refresh. Which input caused which
  commit is not knowable; the tool says so.

## Alternatives considered

- **(output, boundary aimed) as the id**: no new counter, but under VRR a re-aimed refresh would
  change it, and a late frame's capture would report another boundary.
- **Lists of inputs and commits in each frame's record**: simpler to read, but each list needs a
  cap, and an 8,000 Hz pointer gives some 33 inputs a frame at 240 Hz.
- **Tracing spans only**: little code, but logs are not a contract and agents cannot read them.
- **GPU timer queries on GLES now**: removed with GLES at M2.9.
- **A `Waker` named at its making**: an interface for one test seam, which is no task.
- **A number of MCP's own for each call, given back in `pointer`'s and `keyboard`'s results**: an
  agent model could read it, but each result would allocate one more key, which `input` would
  measure as a regression.
- **Only commits with a buffer as causes**: frames that a moved subsurface draws would have none.
- **The device as a field set around a delivery**: Yuna's own motions inside a refresh would be
  named after whatever call set it last.

## Consequences

- M2.4d builds the id, the rings, the record's writes on the frame path, the `frames` tool and its
  contract, the exact causes, the hidden windows' set, the named wakes and the priority's log.
- Frame events stay out of the event stream
  ([ADR-0153](0153-one-event-stream-marks-at-the-change-mcps-log-per-seat-the-bound.md)).
- An agent model, which sees no JSON-RPC id, finds its input among a frame's by its kind, its
  device and its time; a client of MCP that sees its ids finds it by its id.
- The reason a surface was not scanned out comes at 2b, with planes.

## Validation and reconsideration

Evidence: the frame path at `d709896`; four readers' reports and a challenge of the build plan on
2026-11-25; the parity thresholds. Checked by stories that read a frame's inputs, commits, causes
and presentation through `frames`, a VRR story where a refresh is aimed again under one id, a
story where a refresh armed during its own arrangement gets a second id, and the parity script's
per-frame figures. Proposed again if the record cannot meet the parity allowance, or if 2b's KMS
path needs an id of another shape.
