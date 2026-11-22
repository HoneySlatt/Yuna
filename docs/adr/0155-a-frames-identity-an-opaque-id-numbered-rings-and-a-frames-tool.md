# 0155. A frame's identity: an opaque id, numbered rings, and a `frames` tool

- Status: Accepted
- Date: 2026-11-22

## Context

[ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md) item 11 asks
for a frame's identity from input to presentation:

- one id carried by input, commits, drawing, the GPU's start and end, the KMS commit and the
  presentation feedback;
- each wake named by its source.

[ADR-0135](0135-priority-against-a-game-amds-master-override-turned-on-at-2b-on-a-measure.md) asks
for the GPU priority granted in the frame's log.

Yuna today, at `5b9fa13`:

- **No id crosses input, commit, drawing and presentation.** An output's refresh number is the one
  number a frame has. It is the boundary a capture reports.
- **An armed refresh is aimed again under VRR** when something new comes before it.
- **A frame's causes are a set of bits and the clients whose commits it shows**, and only the rest
  auditor counts them. A window's first buffer counts as a layout change. A commit counts even when
  it brought nothing new.
- **Hidden windows' frame callbacks walk every window** each time their deadline fires.
- **The lead before a boundary** is the longest recent drawing timed on the processor, with a floor
  of 2 ms and a margin of 1 ms. Nothing reads the GPU's timestamps.
- **Nothing asks for or reads a GPU priority.** Smithay's EGL context can read the priority
  granted.
- **An agent reads a presentation time only through a capture.**
- **The parity script allows a frame at least 600 more instructions in `yuna` and half an
  allocation more** (the larger of a share and a floor, per figure), and compares the causes of
  drawing on both sides.

The field:

- **Hyprland's Aquamarine carries a 64-bit commit id** in the DRM commit's user data and matches
  presentation feedback with it.
- **Mutter and KWin pass the frame object itself.**
- **No rival carries an input's id.** Wayland gives none from an input event to the commit a
  client makes after it.
- **Only KWin keeps GPU timings on its frames.**
- **Weston alone has a bounded recorder**, a 5 MiB ring.

## Decision

The owner chose, on 2026-11-22:

- **`FrameId`, an opaque 64-bit number**, taken when an output's refresh is first armed. It is kept
  when the refresh is aimed again. The boundary aimed at, the time sampled and the boundary
  presented are fields of the frame, not its identity. At 2b the same number goes into the KMS
  commit's user data.
- **Two numbered rings and a ring of frames**, in `yuna` until `yuna-core` exists, one set per
  seat (each output, input and client belongs to one seat), each of fixed size, bounded in bytes:
  - inputs, each with its time, its device and the MCP call that sent it, if any;
  - commits, each with its client, its surface and whether it brought new content;
  - frames, each with its id, output, causes, the ranges of input and commit numbers it covers,
    when it was armed, drawn and presented, and the boundaries.

  Each input, commit or frame costs a constant amount, written in place: no allocation in a
  steady frame, within the parity script's allowance.
- **A `frames` tool in MCP** reads an output of the caller's seat, its frames after a cursor,
  with `missed`, as `events` does; the inputs and commits it shows are the seat's own. That pays the presentation times agents could read only through a capture. The `frame
  drawn` log line carries the id and causes.
- **The causes become exact.**
  - A window's first buffer counts as its client's commit.
  - A commit counts when it brought new content.
  - Hidden windows wait in a set ordered by when each is due.
  - `rest.rs`'s story that asserts a first buffer as a layout change changes its assertion.
  - If a parity scenario's causes move, the baseline moves in a commit that names them
    ([ADR-0142](0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)).
- **Wakes named fully.**
  - A `Waker`'s wake names the task that woke it.
  - An X11 wake names the X11 client behind it, from the pid the manager already reads.
- **The GPU priority granted** is read once when the renderer is made, and logged.
- **The GPU's timestamps come with the Vulkan renderer** (M2.8), which leads its deadline from
  them. GLES's timer queries would be thrown away at M2.9. The lead's floor and margin stay until
  then: that debt moves to M2.8. Each late frame is named by its record meanwhile.
- **Input to frame is recorded in order, not as causality.** A frame covers the inputs delivered
  and the commits that followed. Which input caused which commit is not knowable; the tool says so.

## Alternatives considered

- **(output, boundary aimed) as the id**: no new counter, but under VRR a re-aimed refresh would
  change it, and a late frame's capture would report another boundary.
- **Lists of inputs and commits in each frame's record**: simpler to read, but each list needs a
  cap, and an 8,000 Hz pointer gives some 33 inputs a frame at 240 Hz, copied into every output's
  record.
- **Tracing spans only**: little code, but logs are not a contract and agents cannot read them
  through MCP.
- **GPU timer queries on GLES now**: complete today, removed with GLES at M2.9.
- **The causes kept as they are**: no test or baseline moves, but two debts stay open.

## Consequences

- M2.4d builds the id, the rings, the record's writes on the frame path, the `frames` tool and its
  contract files, the exact causes, the hidden windows' set, the named wakes and the priority's
  log line.
- Frame events stay out of the event stream
  ([ADR-0153](0153-one-event-stream-marks-at-the-change-mcps-log-per-seat-the-bound.md)): frames are
  read one way.
- The reason a surface was not scanned out comes at 2b, with planes.

## Validation and reconsideration

Evidence: the frame path at `5b9fa13`; the parity thresholds; the sources of Aquamarine 0.16,
Hyprland 0.57.1, Mutter 51, KWin 6.8.2 and weston 16; a challenge of the first draft on
2026-11-22. Checked by stories that read a frame's inputs, commits, causes and presentation through
`frames`, a VRR story where a refresh is aimed again under one id, and the parity script's
per-frame figures. Proposed again if the record cannot meet the parity allowance, or if 2b's KMS
path needs an id of another shape.
