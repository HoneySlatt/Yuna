# 0063. Vulkan alone, and uwui for what Yuna draws and for Hollow

- Status: Accepted; amends 0004, 0012, 0057; supersedes 0041
- Date: 2026-11-14

## Context

[ADR-0012](0012-opengl-first.md) drew Yuna through Smithay's GLES renderer first, and
[ADR-0041](0041-vulkan-decided-on-a-measure.md) left Vulkan alone, Vulkan with a GLES fallback,
or GLES to a prototype's figures at M1.9.

[ADR-0004](0004-the-compositor-alone-under-a-shell.md) wrote the parts Yuna draws itself without
uwui, and left the owner's own shell on uwui to after uwui's first stage. SlimeEngine's ADR-0199
(2026-11-06) had narrowed uwui: neither the owner's shell, nor its login and lock screens, nor
what the compositor draws itself, so that uwui's core would not be pulled towards the system.

Yuna's own interfaces have grown since ([ADR-0057](0057-what-yuna-does-and-what-the-shell-does.md),
[ADR-0062](0062-the-compositors-comfort-and-look.md)): the share picker, the capture bar, tabbed
columns' tab bar, the overview, Alt-Tab, messages. Without a toolkit, Yuna would write text,
icons and layout of its own, twice the work uwui does. uwui is built to be hosted: the host lends
its device, queue, encoder and target. The owner's shell now has a name, Hollow: public, the
owner's first, on Yuna alone, optional above it.

## Decision

The owner chose, on 2026-11-14:

- **Yuna draws with Vulkan alone**, with no GLES fallback. M1.9 keeps a prototype, to set the
  frame's budgets and to check the laptop's Vulkan driver, but no longer chooses the renderer.
  Yuna's own Vulkan renderer comes at Stage 2's opening, with its own core and scene
  ([ADR-0039](0039-leaving-smithay-step-by-step.md)); until then the headless compositor keeps
  Smithay's GLES renderer, and nothing is built on it that Vulkan would not keep. CI draws on
  lavapipe. The colour pipeline, the layout and the animations still do not depend on the
  renderer.
- **uwui draws Hollow, its lock and login screens, and Yuna's own interfaces.** Yuna hosts uwui
  on its Vulkan device, under three rules:
  - **a crash boundary**: a failure of uwui inside Yuna drops the interface and builds it again;
    it never takes Yuna down ([ADR-0007](0007-surviving-a-crash.md)). A GPU left lost falls to
    [ADR-0058](0058-what-yuna-takes-from-the-owners-other-projects.md)'s recovery;
  - **a measured budget**: uwui's time in each of Yuna's frames is counted and checked in CI;
  - **games first**: over a fullscreen game, uwui draws nothing unless the owner opens an
    interface.
- This reverses the first decision of SlimeEngine's ADR-0199, which SlimeEngine records in an ADR
  of its own; its other decisions stand.

## Alternatives considered

- **The prototype's choice at M1.9** (ADR-0041): the renderer chosen on figures, but Yuna's
  hosting of uwui, which lends a Vulkan device, would wait on it.
- **Vulkan with a GLES fallback** for older machines: wider reach, but two renderers to keep.
- **Yuna's interfaces without uwui**: no dependency of the compositor on an application toolkit,
  but a small toolkit written again inside Yuna.
- **Yuna's interfaces in a separate process drawn by uwui**: a uwui fault could never reach Yuna,
  but the overview and Alt-Tab need every window's image live, across processes.
- **Hollow, its lock and login on uwui, Yuna's interfaces decided at Stage 6**: less risk now,
  but two ways of drawing until then.
- **Another toolkit for Hollow** (Quickshell, Iced, GTK4): usable at once, but outside the owner's
  stack, or not drawn to the owner's bar.

## Consequences

- AGENTS.md's rule that only the final drawing is OpenGL becomes Vulkan.
- Yuna depends on uwui for what it draws; uwui gains the targets of a shell, a lock screen and a
  compositor's own interfaces (layer-shell and session-lock surfaces, a hosted device), whose cost
  is uwui's, under ADR-0003's rule that uwui comes first.
- The laptop's Vulkan driver is checked at M1.9; jay reports modifiers failing on AMD before RX
  5000 ([survey](../survey.md#an-own-base-and-vulkan)). If the laptop cannot run Yuna's Vulkan,
  the lowest tier returns to the owner.
- ADR-0057's "text or icon rendering" dependency for the picker becomes uwui.
- The ROADMAP's open decision on the owner's shell closes: Hollow, on uwui, on Yuna alone.
