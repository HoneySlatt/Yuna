# 0007. Surviving a crash

- Status: Accepted; amended by 0058, 0061
- Date: 2026-11-11

## Context

When a Wayland compositor crashes, its clients lose their connection and most exit. KWin's
`kwin_wayland_wrapper` keeps the Wayland socket open and restarts KWin; Qt 6 applications reconnect
when `QT_WAYLAND_RECONNECT` is set; GTK and SDL do not yet. The `xdg-session-management-v1` protocol,
in wayland-protocols' staging since 2026-03, lets a compositor put windows back after a restart;
KWin and Mutter implement it ([survey](../survey.md#surviving-a-crash)).

## Decision

- **Robust by design**: property tests, fuzzed protocol handling, every client's error kept to that
  client.
- **A small process holds the Wayland socket** and restarts Yuna within a second after a crash.
- **Applications that reconnect survive**; the others are reopened in place through session
  restore.

## Alternatives considered

- **Zero windows lost**: a guardian process keeping every client's connection while the rest
  restarts. Unique on Linux, but its cost and feasibility are unknown.
- **Robustness alone**: nothing planned for after a crash.

## Consequences

- Restart time and windows restored are budgets of the quality bar.
- Yuna's state needed to restore windows is saved continuously, with atomic writes.
