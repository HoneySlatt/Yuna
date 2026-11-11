# 0019. The layouts' behaviours: niri and Hyprland as references

- Status: Accepted
- Date: 2026-11-11

## Context

Hundreds of small choices make a layout feel right or half finished: where focus goes when a
window closes, which way dwindle splits, what a column becomes after fullscreen. The owner's
complaint about Umbriel is behaviours that half work ([ADR-0005](0005-the-layouts.md)). niri is the
reference for scrolling columns ([ADR-0013](0013-our-own-code-niri-and-jay-as-references.md));
Hyprland invented the dwindle, special workspace and pinned window the owner uses.

## Decision

- **Scrolling columns, focus, fullscreen and floating windows behave as niri's**, read in niri
  26.04's documentation and tests and rewritten as Yuna's scenarios.
- **Dwindle, special workspaces and the pinned window behave as Hyprland's defaults**, read in its
  wiki at commit f20353c (2026-11-10) and its source at commit 70ec30e (2026-11-11).
- Yuna departs from them only to fix a defect, to serve games first, or on the owner's decision.
  Each departure is listed here and tested.

### Departures

- **A window opening behind a fullscreen one in dwindle** leaves it fullscreen and focused.
  Hyprland's default (`misc:on_focus_under_fullscreen` 2) ends the fullscreen; a window opening
  must not take a game's screen. niri does the same as Yuna in columns.
- **Floating and pinned windows hide while a fullscreen window holds the focus**, in both layouts
  and whether the fullscreen window is on the workspace shown or on the special workspace over
  it, as in niri; the owner chose this for pinned windows. Hyprland draws them over the
  fullscreen window.
- **A floating window going fullscreen joins the tiles**, in both layouts, as in niri; Hyprland
  keeps it floating. It floats back where it was when it leaves fullscreen itself (its client or
  the fullscreen key), as in niri. Fullscreen ended another way (focus moving in dwindle, a window
  joining its column, a tiling switch) leaves it tiled: niri has no such paths in dwindle, and a
  window jumping out of the tiles on a focus change would surprise.
- **A new dwindle window always goes right or below.** Hyprland's default (`force_split` 0) lets
  the cursor choose; the layout model has no cursor. This is Hyprland's `force_split` 2.
- **Directional focus in dwindle and among floating windows** takes the nearest window on that
  side, the topmost between equals; it stops at the output's edge. Hyprland prefers the neighbour
  used last, and goes on to the next monitor or wraps.
- **Focusing a window beneath a special workspace hides it**, and so does following a window
  moved to the workspace beneath, so that the focused window shows; switching workspace keeps it
  shown, as Hyprland (`binds:hide_special_on_workspace_change` false). Moving a window to a
  special workspace is Hyprland's silent form: the window goes, the view stays.

### Not built yet

niri's centring modes other than "never" (the owner's setting), per-window heights in a column,
maximize-to-edges, focus that crosses outputs at a strip's end, moving and renaming workspaces;
dwindle's resizable splits. Each comes when a milestone or the owner asks for it. Tabbed columns
are left out ([ADR-0005](0005-the-layouts.md)).

## Alternatives considered

- **Umbriel's behaviours**: nothing to relearn, but the owner finds them half finished, and its
  code has no tests that say what it means to do.
- **Every behaviour put to the owner**: everything to taste, at the cost of dozens of questions.

## Consequences

- A scenario that rewrites a behaviour of niri or Hyprland names its source, so that a newcomer
  can check it; the others test Yuna's own rules, such as its errors and seat isolation.
- A behaviour that differs from both references needs a line in the departures above.
