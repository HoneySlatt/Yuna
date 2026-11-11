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
- **Floating and pinned windows hide while a fullscreen window holds the focus**, in both layouts,
  as in niri. Hyprland draws them over the fullscreen window.
- **A floating window going fullscreen joins the tiles**, in both layouts, and floats back where it
  was on leaving fullscreen, as in niri. Hyprland keeps it floating.
- **A new dwindle window always goes right or below.** Hyprland's default (`force_split` 0) lets
  the cursor choose; the layout model has no cursor. This is Hyprland's `force_split` 2.
- **Directional focus in dwindle and among floating windows** takes the nearest window on that
  side, the topmost between equals; it stops at the output's edge. Hyprland prefers the neighbour
  used last, and goes on to the next monitor or wraps.
- **Switching workspace hides the special workspace shown on that output**, so that the seat sees
  what it asked for; the sources read do not say what Hyprland does. Moving a window to a special
  workspace is Hyprland's silent form: the window goes, the view stays.

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

- Each scenario names its source in niri or Hyprland, so that a newcomer can check it.
- A behaviour that differs from both references needs a line in the departures above.
