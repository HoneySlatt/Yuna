# 0013. Our own code; niri and jay as references

- Status: Accepted
- Date: 2026-11-11

## Context

niri is under GPL-3.0-or-later, so its code could be reused; its layout code (about 23,700 lines) is
built for scrolling columns and one seat. jay is under GPL-3.0-only; copying from it would make Yuna
GPL-3.0-only ([ADR-0009](0009-gpl-private-until-ready.md)), and its colour module does neither tone
nor gamut mapping.

## Decision

- Yuna writes its own code: the layout model, the animations, the colour pipeline and the rest.
- **niri is the close reference** for scrolling, animations, the test clock and its testing method.
  Its behaviours and test scenarios are read and rewritten as Yuna's tests; Yuna matches them, then
  goes past them by measure.
- **jay is a reading reference only**, for HDR, VRR, tearing and XWayland scaling. No code is copied
  from it.

## Alternatives considered

- **niri's layout code taken whole**: fastest, but a structure for one layout and one seat.
- **niri's proven parts taken, our own structure**: faster than writing all, at the cost of tracking
  what came from where.
- **jay's colour module taken**: a working base, but GPL-3.0-only, and tone mapping grafted onto
  another author's code.

## Consequences

- More time than reusing niri's code; a structure designed for every layout and for agents' seats.
- An agent that reads niri's or jay's code does not paste it; a passage close to its source is
  rewritten or credited.
