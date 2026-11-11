# 0002. The headless mode first, then the desktop

- Status: Accepted
- Date: 2026-11-11

## Context

Both modes share the protocols, the window management and the drawing
([ADR-0001](0001-what-yuna-is.md)). The owner's main complaint about Umbriel is polish: behaviours
that half work. Agents need a better test compositor now, while uwui leaves SlimeEngine
(SlimeEngine's Stage 6d), and Aerith needs HDR under an SDR interface checked.

## Decision

Yuna is built headless first: the layout model, the protocols, the headless backend and the agents'
control come in Stage 1; the real screen and the desktop in Stage 2; then agents on the desktop,
games, the image, and effects, in the order of [`ROADMAP.md`](../ROADMAP.md).

A throwaway prototype early in Stage 1 shows HDR drawn through Smithay's DRM backend on the owner's
monitor, since no Smithay compositor has HDR yet ([ADR-0011](0011-built-on-smithay.md)).

## Alternatives considered

- **The desktop first**: Umbriel replaced sooner, but agents wait for years, and the desktop is built
  without the tests that keep its behaviours from regressing.

## Consequences

- The owner keeps Umbriel until Stage 2 ends.
- Each layout and window behaviour, and each bug the owner reports, becomes a test that runs without
  a screen, in CI.
- A failed HDR prototype reopens [ADR-0011](0011-built-on-smithay.md) in weeks, not years.
