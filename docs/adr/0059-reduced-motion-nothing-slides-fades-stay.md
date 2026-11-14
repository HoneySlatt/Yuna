# 0059. Reduced motion: nothing slides, fades stay

- Status: Accepted; amends [0055](0055-what-moves-first-and-animations-off-headless.md) (reduced
  motion now)
- Date: 2026-11-14

## Context

[ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md) brings reduced motion into
M1.7: the owner's rules ask that an effect lighten when reduced motion is asked.
[ADR-0055](0055-what-moves-first-and-animations-off-headless.md) left it for later, as a kind
chosen per animation. Apple's platforms replace slides and zooms with cross-fades when motion is
reduced; GNOME and Android turn animations off.

## Decision

The owner chose, on 2026-11-14:

- **Under reduced motion, nothing crosses the screen**: the scrolling camera, the windows a layout
  operation moves and the workspace switch show their result at once.
- **Fades stay**, over half their duration: the special workspace's fade moves nothing, and
  appearing at once is no gentler.
- **It is a setting of its own**, beside `off`: `Settings::reduced_motion` in `yuna-motion`, and
  `--reduced-motion` for `yuna --headless` and `yuna mcp`, in place of `--animations`. The
  desktop reads the shell's or the session's setting at Stage 2.

## Alternatives considered

- **Cross-fades in place of slides** (Apple): gentler, but the frame before must be kept and both
  drawn, more GPU time where the rule asks for less, and a larger milestone.
- **Softer springs**: shorter and without bounce, but things still cross the screen, which is what
  reduced motion is asked against.
- **Everything at once** (GNOME, Android): the simplest, but the special workspace then appears
  abruptly.

## Consequences

- Reduced motion costs less than motion: it draws fewer frames.
- Stage 6's effects follow the same rule: what moves stops, what fades stays.
