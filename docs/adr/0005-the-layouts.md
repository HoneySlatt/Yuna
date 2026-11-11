# 0005. The layouts: scrolling columns, dwindle, floating, special workspaces

- Status: Accepted
- Date: 2026-11-11

## Context

Umbriel offers scrolling, dwindle, master and stacking layouts, tab groups and scratchpads; the
owner finds several of them half finished. niri offers one layout, columns on a strip, with floating
windows beside it, and does it best. The owner uses scrolling columns, dwindle, floating windows,
Hyprland's hidden special workspace, fullscreen and full-width columns daily, and neither master nor
tab groups.

## Decision

Yuna has, per workspace:

- **scrolling columns**, at least as good as niri's;
- **dwindle**, each new window splitting the space of the focused one;
- **floating windows**, on every workspace, for games, dialogs and plugin windows;
- **special workspaces**, hidden, shown over the current one by a key;
- **fullscreen**, **full-width columns**, **fake fullscreen** (the application believes it is
  fullscreen, its column unchanged), **preset column widths**, **a pinned window** over every
  workspace, **terminal swallowing**, per-application rules.

The layout model is a pure part of Yuna, with no Wayland object in it, so that every behaviour is a
property test or a scenario run without a compositor.

## Alternatives considered

- **Every layout Umbriel has**: breadth at the cost of polish, the owner's complaint.
- **Scrolling only, as niri**: the best polish, but the owner uses dwindle and special workspaces.

## Consequences

- Master, stacking and tab groups are left out; one is added only on the owner's request, in an ADR.
- The model is designed for several layouts and several seats from its first line
  ([ADR-0015](0015-agents-workspaces.md)).
