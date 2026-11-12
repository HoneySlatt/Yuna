# 0028. Bounds on what one client may ask

- Status: Accepted
- Date: 2026-11-12

## Context

[ADR-0007](0007-surviving-a-crash.md) keeps every client's error to that client. Some requests
are valid one at a time and harmful in number or in size:

- The layout model walks a dwindle tree by recursion, as deep as it has windows: 10,000 windows
  in one tree work, 100,000 overflow the stack ([ADR-0020](0020-the-layout-models-shape.md)).
- Smithay's popup trees and Yuna's frame loop walk every popup of a window at each of its commits
  and each refresh; Smithay walks a popup's parents without a bound, and a client can make two
  popups each the other's parent.
- Smithay keeps every activation token not yet used.
- A positioner's values are any `i32`; sums of them overflow ([ADR-0027](0027-smithay-without-overflow-checks.md)).

Wayland has no error for "too many": the protocol's errors end the client, and its other windows
with it.

## Decision

- **A client may have 1,000 windows in the layout, and all clients 10,000.** A window over
  either cap is not shown, as a window the layout refuses: its client gets frame callbacks at the
  hidden windows' rate and keeps its other windows; the refusal is logged once per client.
- **A window may have 64 popups, nested at most 32 deep;** a positioner's values are bounded by
  2^20. A popup over a bound, or whose parents loop, is dismissed (`popup_done`), which xdg-shell
  allows at any time. Yuna walks a popup's parents itself, bounded, before Smithay does.
- **A client may keep 64 activation tokens not yet used;** a new one pushes out its oldest.
- Over a bound, Yuna refuses the one request and never ends the client: losing a window or a
  menu is better than losing the application and its user's data.

## Alternatives considered

- **No caps, the layout walked without recursion**: removes one reason, not the cost of linear
  walks at each commit, nor memory.
- **A protocol error over a cap**: simple, but it ends the client with all its windows.
- **Caps per output or per workspace**: closer to the stack's limit, but a client opens windows
  on one workspace anyway.

## Consequences

- A program that opens more than 1,000 windows sees the extra ones never shown.
- The caps are constants in the code (`MAX_WINDOWS_PER_CLIENT`, `MAX_WINDOWS`,
  `MAX_POPUPS_PER_WINDOW`, `MAX_POPUP_DEPTH`), each with its reason; raising one is measured
  against the layout's and the frame loop's budgets.
