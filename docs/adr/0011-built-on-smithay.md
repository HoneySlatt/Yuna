# 0011. Built on Smithay, its patches kept in Yuna

- Status: Accepted
- Date: 2026-11-11

## Context

From the [survey](../survey.md#foundations) and a challenge of the recommendation:

- **Smithay** (MIT, about 126,000 lines, 60 authors in six months) is the base of niri, cosmic-comp
  and others. It has 48 protocol modules, atomic DRM, planes and direct scanout, explicit sync,
  libei, an X11 window manager for XWayland, several seats, and runs the WLCS conformance suite in
  CI. It has no HDR or colour management, no Vulkan renderer (its main author is writing one), no
  tearing (pull requests open) and no headless output backend. Its `AI.md` asks that AI-written code
  be flagged in commit messages and advises against language models; its main author hesitates over
  outside HDR work, "especially if AI-generated".
- **jay** (GPL-3.0-only, about 301,000 lines, one author) has its own stack: Vulkan renderer, HDR,
  VRR and tearing modes, headless outputs, special workspaces, XWayland downscaled at fractional
  scale; an i3-like layout, no animation system, no tone mapping, 1,166 `unsafe` blocks in `src/`
  with 6 safety comments.
- **niri** (GPL-3.0-or-later) has no HDR, tearing or explicit sync, one seat, and refuses
  contributions written by language models. **cosmic-comp** (GPL-3.0-only) has no HDR and is tied to
  COSMIC's protocols.

## Decision

Yuna is built on **Smithay**, at a pinned revision updated at each stage's end. What Smithay lacks is
written in Yuna, or in a **fork of Smithay kept in Yuna's control and as small as possible**: the
owner's rules forbid naming an AI in a commit, and Smithay asks for it, so Yuna never contributes
upstream. The first such patches: the connector and plane colour properties for HDR, tearing, a
headless output backend.

## Alternatives considered

- **A fork of jay**: the hardest parts done, but its layout and code are the weakest fit, and a fork
  of 301,000 lines of one author's code is Yuna's to maintain alone.
- **A fork of niri**: scrolling done, but one seat, no HDR, and a permanent split.
- **A fork of cosmic-comp**: tied to COSMIC.
- **From zero, as jay did**: two to three times longer.

## Consequences

- HDR on Smithay is unproven: Stage 1's prototype measures it first
  ([ADR-0002](0002-the-headless-mode-first.md)).
- Each Smithay update may break Yuna's patches; they are listed with their reason in
  `docs/smithay-patches.md` once the first exists.
