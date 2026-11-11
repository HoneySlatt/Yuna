# 0001. What Yuna is: one compositor, two modes

- Status: Accepted
- Date: 2026-11-11

## Context

The owner uses Umbriel (C++23 on wlroots) as a daily compositor, beside niri and Hyprland before it.
Its faults, in the owner's words: polish (a one-column fullscreen half working, odd windows in
dwindle), scrolling close to niri's but not as good, unreliable HDR, more power drawn than dwl and
mango, animations less beautiful than Hyprland's, C++. Its strength: X11 applications crisp at a
scale of 1.5. The owner is a heavy gamer and a cinephile, on an ASUS PG32UCDM (QD-OLED, 3840 × 2160,
240 Hz) whose VRR flickers on the desktop under every compositor tried.

The owner's agents build applications (uwui, SlimeEngine, Aerith, Opium, Slatty, Pluto, Magnolia)
and must test them without touching the owner's screen, input or focus. Today they use a series of
headless compositors that emulate neither VRR nor HDR and throttle what is not shown.

## Decision

Yuna is a Wayland compositor in Rust for Linux, one program with two modes:

- **Headless**, for agents and tests: virtual outputs (3840 × 2160 at 240 Hz and scale 1.5 among
  them), emulated VRR timing and HDR colour descriptions, a controlled clock, captures and input
  driven over MCP, one isolated seat per agent.
- **The desktop**, the owner's daily compositor, at the top of its field: the layouts of
  [ADR-0005](0005-the-layouts.md), animations and effects ([ADR-0010](0010-animations-and-effects.md)),
  reliable HDR and colour, VRR without flicker ([ADR-0014](0014-vrr-without-flicker.md)), games and
  films served first, the lowest power of its rivals, and hidden workspaces where agents work on the
  owner's GPU ([ADR-0015](0015-agents-workspaces.md)).

It is made for its owner first. Every feature the owner marked indispensable is listed in
[`ROADMAP.md`](../ROADMAP.md), with its stage.

## Alternatives considered

- **A headless test compositor alone, Umbriel kept as the desktop**: the cheaper option, recommended
  first; the owner chose both, for a desktop of their own in Rust.
- **Two programs**: two copies of the protocols and the window management, whose behaviours would
  drift; the tests of one would prove nothing about the other.
- **Patching Umbriel**: C++, another team's direction, and HDR and power problems in its base.

## Consequences

- A project of years: the desktop's full bar is reached stage by stage
  ([ADR-0002](0002-the-headless-mode-first.md)).
- Every window behaviour is a test in the headless mode before it is code on the desktop.
- Umbriel stays the owner's desktop until Yuna's Stage 2 replaces it.
