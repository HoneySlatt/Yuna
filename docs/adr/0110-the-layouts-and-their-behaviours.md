# 0110. The layouts and their behaviours: niri and Hyprland as references, games first

- Status: Accepted; supersedes [0005](0005-the-layouts.md),
  [0019](0019-the-layouts-behaviours.md) and [0026](0026-real-clients-in-the-layout.md) (its
  behaviours); supersedes in part [0061](0061-yunas-uses.md) (point 3's focus guard),
  [0062](0062-the-compositors-comfort-and-look.md) (point 2, tabbed columns) and
  [0064](0064-what-the-best-desktops-taught-yuna.md) (points 1 and 3, focus marked and several
  fullscreen windows)
- Date: 2026-11-19

## Context

Which layouts Yuna has, and how they behave, was decided in five ADRs.
[ADR-0005](0005-the-layouts.md) chose the layouts and left tab groups out;
[ADR-0062](0062-the-compositors-comfort-and-look.md) brought tabbed columns in.
[ADR-0019](0019-the-layouts-behaviours.md) made niri and Hyprland the references and listed the
departures from them; [ADR-0026](0026-real-clients-in-the-layout.md) added the behaviours of real
clients and departures that ADR-0019's list lacked, though ADR-0019 asks every departure to be
listed there; [ADR-0064](0064-what-the-best-desktops-taught-yuna.md) allowed several fullscreen
windows on a workspace and marked focus rather than letting it be stolen.

ADR-0005 left tab groups out and ADR-0019 listed tabbed columns as left out; ADR-0062 brought them
in at the owner's request. ADR-0019 had one fullscreen window per workspace; ADR-0064 allows
several. [ADR-0061](0061-yunas-uses.md) point 3 declined "a guard of a fullscreen game's focus" on
the day ADR-0064 point 1 decided that a window never takes the focus during a fullscreen game; on
2026-11-19 the owner settled that ADR-0064 point 1 holds. The later rule, or the owner's, stands
below.

This ADR gathers the rule as it stands on 2026-11-19. It decides nothing new. The layout model's
shape is [ADR-0111](0111-the-layout-models-shape-plain-ids-a-branch-per-seat-asked-and-committed-sizes.md)'s;
workspaces are [ADR-0018](0018-workspaces.md)'s.

## Decision

- **The layouts, per workspace** (ADR-0005, 2026-11-11): scrolling columns, at least as good as
  niri's; dwindle, each new window splitting the space of the focused one; floating windows, on
  every workspace, for games, dialogs and plugin windows; special workspaces, hidden, shown over
  the current one by a key; fullscreen, full-width columns, fake fullscreen (the application
  believes it is fullscreen, its column unchanged), preset column widths, a pinned window over
  every workspace, terminal swallowing, per-application rules.
- **Tabbed columns** (ADR-0062, 2026-11-14): a column of scrolling columns can show its windows as
  tabs, one visible, with a tab bar Yuna draws; at Stage 2, as a layout behaviour, tested in the
  layout model. Master, stacking and Hyprland's groups stay out; one is added only on the owner's
  request, in an ADR (ADR-0005).
- **The references** (ADR-0019, 2026-11-11): scrolling columns, focus, fullscreen and floating
  windows behave as niri's, read in niri 26.04's documentation and tests; dwindle,
  special workspaces and the pinned window as Hyprland's defaults, read in its wiki at commit
  f20353c and its source at commit 70ec30e. Yuna departs from them only to fix a defect, to serve
  games first, or on the owner's decision. Each departure is listed below and tested.
- **Real clients in scrolling columns and floating windows, as niri** (ADR-0026, 2026-11-12, read
  in niri at commit 7f81a7f and Hyprland at commit 70ec30e). A
  window is drawn at the size its client committed. A column is as wide as its widest window and
  respects every window's minimum and maximum width. Heights are shared, each window getting at
  least its minimum; a window whose minimum and maximum heights are equal keeps that height;
  minimums that do not fit overflow the column. When a column changes width, the focused column
  stays where it is on screen. A fullscreen window smaller than its output is centred. A window
  with a parent, or with a fixed height, opens floating, centred on its parent, at its own size. A
  new window takes the focus unless a fullscreen column holds it; a dialog takes it only from its
  parent; a valid activation request focuses its window, but for the departures below. A dialog
  opens on its parent's workspace.
- **Real clients in dwindle, as Hyprland** (ADR-0026): no minimum enforced; a window whose maximum
  is smaller than the space it would split floats.

**Departures:**

1. **Nothing takes a fullscreen game's screen** (ADR-0019; ADR-0026). While a window's fullscreen
   holds the focus, a window opening goes behind it and does not take the focus, even if it asks to
   open fullscreen; another window's client asking to be maximised or fullscreen is refused;
   Hyprland's default (`misc:on_focus_under_fullscreen` 2) ends the fullscreen instead. A window
   keeps its own maximised state under its fullscreen and finds it again after. A fullscreen
   window's own dialog takes the focus and shows over it, the window staying fullscreen. While the
   window, its dialog or a floating window over it has the focus, an activation request is refused
   unless it comes from that window or one of its dialogs, nested ones included; the requesting
   window is marked urgent.
2. **Focus is marked, not stolen** (ADR-0064, 2026-11-14; confirmed by the owner on 2026-11-19
   over ADR-0061 point 3): a window that asks for the focus never takes it during a fullscreen game
   or while the owner types; it is marked urgent, by a border and an event. A rule per application
   allows it (Stage 2).
3. **Several windows fullscreen on one workspace**, as Steam Big Picture needs (ADR-0064,
   Stage 2); while a game's fullscreen holds the focus, departure 1 still holds.
4. **Floating and pinned windows hide while a fullscreen window holds its workspace's focus**
   (ADR-0019), in both layouts, whether the fullscreen window is on the workspace shown or on the
   special workspace over it, as in niri; Hyprland draws them over it. A pinned window stays
   hidden while the seat works in a special workspace shown over that fullscreen window.
5. **A floating window going fullscreen joins the tiles** (ADR-0019), in both layouts, as in niri.
   It floats back where it was when it leaves fullscreen itself (its client or the fullscreen
   key). Fullscreen ended another way leaves it tiled.
6. **A new dwindle window always goes right or below** (ADR-0019): Hyprland's `force_split` 2; the
   layout model has no cursor.
7. **Directional focus in dwindle and among floating windows** takes the nearest window on that
   side, the topmost or leftmost between equals, and stops at the output's edge (ADR-0019).
8. **Focusing a window beneath a special workspace hides it** (ADR-0019), and so does following a
   window moved to the workspace beneath; switching workspace keeps it shown, as Hyprland
   (`binds:hide_special_on_workspace_change` false). Moving a window to a
   special workspace is Hyprland's silent form: the window goes, the view stays.
9. **Only the window that has its seat's focus is told it is activated** (ADR-0026, the owner's
   choice on 2026-11-12), as in Hyprland: a hidden terminal does not blink its cursor and wake
   Yuna.
10. **A dwindle window larger than its space is clipped to it** (ADR-0026), so that it never covers
    its neighbours.

- **The overview** (Stage 6) is Umbriel's, itself niri's improved (ADR-0019, the owner's choice on
  2026-11-11); its details are read in Umbriel when Stage 6 is designed.
- **Not built yet** (ADR-0019): niri's centring modes other than "never", per-window heights in a
  column, maximize-to-edges, focus that crosses outputs at a strip's end, moving and renaming
  workspaces; dwindle's resizable splits. Each comes when a milestone or the owner asks for it.
- **A scenario that rewrites a behaviour of niri or Hyprland names its source** (ADR-0019), so that
  a newcomer can check it; a behaviour that differs from both needs a line in the departures.

## Alternatives considered

- **Every layout Umbriel has** ([ADR-0005](0005-the-layouts.md)): breadth at the cost of polish,
  the owner's complaint.
- **Scrolling only, as niri** ([ADR-0005](0005-the-layouts.md)): the owner uses dwindle and
  special workspaces.
- **Umbriel's behaviours, or every behaviour put to the owner**
  ([ADR-0019](0019-the-layouts-behaviours.md)): behaviours the owner finds half finished, or
  dozens of questions.
- **Activations obeyed always, as niri** ([ADR-0026](0026-real-clients-in-the-layout.md)): any
  program could take a game's screen.
- **niri's activated windows** ([ADR-0026](0026-real-clients-in-the-layout.md)): hidden windows
  that believe they are active keep drawing.
- **Minimums enforced in dwindle** ([ADR-0026](0026-real-clients-in-the-layout.md)): a deep tree
  would overflow its output.
- **A focus steal allowed always, or never** ([ADR-0064](0064-what-the-best-desktops-taught-yuna.md)):
  the other options of its point 1.

## Consequences

- A slow client resizing moves its column's neighbours when it commits, not before.
- A dwindle window whose minimum exceeds its space shows only its top-left part.
- While a fullscreen game's dialog has the focus, the other floating windows of its workspace show
  over the game too, as they do whenever the fullscreen window does not hold the focus.

## Validation and reconsideration

Evidence: the layout model's scenarios and property tests, each scenario naming its source. Proposed
again if the owner asks for a layout left out, or finds a behaviour copied from a reference wrong.
