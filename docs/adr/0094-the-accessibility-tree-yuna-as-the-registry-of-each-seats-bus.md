# 0094. The accessibility tree: Yuna as the registry of each seat's bus

- Status: Accepted
- Date: 2026-11-18

## Context

[ADR-0061](0061-yunas-uses.md) gives M1.11 the accessibility tree of a seat's programs through
MCP, its dependency put to the owner; the milestone is done when an agent finds a button by its
name through MCP alone. On Linux that tree is AT-SPI, over D-Bus; no Wayland protocol gives it.
Facts read on 2026-11-17 ([survey](../survey.md#the-agents-toolbox)):

- Toolkits find the AT-SPI bus through `AT_SPI_BUS_ADDRESS` first. GTK 4 and AccessKit export
  nothing until the registry, `org.a11y.atspi.Registry`, answers their `Embed`; Qt exports first.
  Chromium needs `ACCESSIBILITY_ENABLED`, Firefox `GNOME_ACCESSIBILITY`, AccessKit a session bus.
- `at-spi2-registryd` pings every application every 30 s, and Qt holds each key until the
  registry answers, up to 100 ms, while a listener is registered.
- A socket that listens with no daemon behind it makes GTK 4 wait at start with no time limit.
- Extents read in window coordinates, plus where the compositor draws the window, give a
  rectangle on Wayland; an application maps to its windows by its bus peer's pid.
- `yuna mcp`'s programs have no session bus (`DBUS_SESSION_BUS_ADDRESS=disabled:`).
- `zvariant`, which encodes D-Bus messages, was marked unmaintained by RustSec on 2026-10-16;
  its code moved into zbus 6 as `zbus::wire`.

## Decision

The owner chose, on 2026-11-18, one decision at a time:

- **Yuna is the registry.** It owns `org.a11y.atspi.Registry` on each seat's bus and answers what
  GTK 4, Qt 6 and the ATK bridge ask of it, every call at once; it registers no event, so that no
  toolkit sends events and Qt's keys never wait. No `at-spi2-registryd`, no ping.
- **One bus per seat, `dbus-daemon` from nixpkgs**, a runtime dependency on the package's path.
  It starts with the first program started with accessibility on that seat, never before; its
  address is given only once it answers; it belongs to the seat, ends with it and with Yuna, and a
  bus that dies is removed and started again on demand.
- **D-Bus through Yuna's own connection on `zbus` 6.3 without its default features**: zbus encodes
  the messages, Yuna's code connects, authenticates, sends and serves on its event loop, with no
  thread and no runtime. Six crates, all MIT.
- **Accessibility is off by default.** `spawn start` takes `accessibility: true`; without it a
  program runs as the owner's does, told by `NO_AT_BRIDGE` and `GTK_A11Y=none` not to look.
- **The `accessibility` tool** walks the trees of the seat's applications, bounded in nodes,
  depth, calls in flight and a deadline per application, and gives each matching node its role,
  name, states and a rectangle in the output's logical units, which `pointer` clicks.

## Alternatives considered

- **`at-spi2-registryd`**: the registry the toolkits are tested against, less code in Yuna, but
  112 MiB of closure more and a wake-up of every application every 30 s.
- **`zvariant`, its advisories ignored**: the encoder first chosen, but a crate with no further
  fixes, and a check weakened.
- **Yuna's own encoding**: no dependency, but a parser of untrusted bytes to write and harden.
- **zbus with its connection and runtime**: about fifteen crates and an executor beside calloop.
- **The `dbus` crate on libdbus**: two crates, but a C binding and an interface built for
  blocking calls.
- **Accessibility on by default**: no flag for an agent, but every program tested would differ
  from the owner's, Chromium's tree among the costliest.

## Consequences

- Each program started with accessibility is tested with it; an agent's measures are made
  without.
- AccessKit's applications (egui) export nothing, having no session bus; GTK 3, Firefox,
  Chromium and Java are not tested yet ([debt](../debt.md)).
- After a tree is read, Yuna and the bus are at rest: 0 wake-ups in 30 s
  ([measurements](../measurements.md)).
- Yuna answers D-Bus calls itself: its parser of messages has property tests on random bytes,
  and a fuzz target comes with Stage 2's ([ADR-0082](0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)).

## Validation and reconsideration

Evidence: a GTK 4 button, a Qt 6 button on Wayland and on X11 at scale 1.5, each found by its name
and pressed at its rectangle's centre; a bus that starts on demand, is its seat's alone, ends with
it and is started again after it dies; a stopped program that costs its deadline alone. Proposed
again when a Wayland protocol linking a surface to its accessible root is merged
(`xdg-dbus-annotation-v1`, open on 2026-11-11), or when the desktop needs a real registry for a
screen reader.
