# 0020. The layout model's shape

- Status: Accepted
- Date: 2026-11-11

## Context

The layout model holds no Wayland object and is tested alone ([ADR-0005](0005-the-layouts.md)).
It must serve several layouts and several seats from its first line, and keep an agent's seat
from ever reaching the owner's ([ADR-0015](0015-agents-workspaces.md)). niri's layout is generic
over a trait its windows implement, holds client sizes and animations, and has one seat
([ADR-0013](0013-our-own-code-niri-and-jay-as-references.md)). The design was challenged before
any code was written (M1.2).

## Decision

- **One crate, `yuna-layout`, with no dependency.** Its tests use proptest, which the owner
  accepted for tests only.
- **Plain ids.** The caller names windows, outputs and seats with `u64` ids. It gives the same
  output id to the same monitor each time it is plugged in (from its connector and EDID, or the
  name the configuration uses), so that workspaces find their home again; it never reuses a
  window id. The model knows nothing else of a window than its id and its layout state.
- **A tree that each seat owns a branch of.** A seat holds its outputs, its workspaces without an
  output and its hidden special workspaces; an output holds its workspaces and the special one
  shown over it. Workspace names are per seat. An operation names a seat (a key binding) or a
  window (a client's request) and reaches only that seat's branch, so isolation is the shape of
  the data, not a check. This isolates seats in the layout only; keeping an agent's programs from
  acting through the owner's clients is M1.5's work. The refusal is mutual: the owner cannot pull
  an agent's window onto an owner's output either, until the owner asks for it.
- **Logical coordinates in `f64`**, shared by all outputs, as niri; gaps, the working area and
  every rectangle given out are snapped to the output's physical pixels, so that they stay crisp
  at scale 1.5, and re-snapped when a workspace moves to another scale.
- **Nothing runs between operations.** There is no clock and no animation state. An operation and
  `arrange` take time linear in the seat's windows: under 25 µs for 100, under 300 µs for 1,000
  ([measurements](../measurements.md)).
- **Bad input is an error, never a panic, and a refused operation changes nothing.**
  `check_invariants` states every rule; the tests call it after every operation.

### Left for later milestones, and why the shape holds

The challenge asked for these now; each adds to the model without changing its tree:

- **What a client asked and what it did** (M1.4): a commit operation, columns placed from the
  sizes clients commit, configure sizes in whole logical units, clients that ignore their size in
  the property tests. Today a window is assumed to take the size it is given.
- **Where a window opens and whether it takes focus** (M1.4): next to its parent for dialogs, and
  a focus policy, a fullscreen game's own dialog taking focus.
- **The camera** (M1.7): the view offset and unsnapped positions given out, so that animations and
  gestures move one camera rather than every window.
- **Arranging a hidden workspace** (M1.5, for the owner's peek), **windows being dragged**
  (Stage 2), **options changed live** (with the configuration).

## Alternatives considered

- **Generic over a window trait, as niri**: the model would call into clients and hold their
  state; tests would need fake windows. Plain ids keep it pure.
- **Integer physical pixels per output**: exact arithmetic, but a window moving between outputs of
  different scales, and coordinates shared across outputs, become conversions everywhere.
- **Seats sharing outputs**: nothing needs it, and whose focus a workspace holds becomes unclear.
- **The challenger's requested and committed sizes now**: right for M1.4, where real clients
  exist to test it against; built now, it would be guessed.

## Consequences

- The compositor keeps the map from Wayland objects to ids, and from monitors to stable output
  ids.
- Each later milestone listed above extends the model; none needs to change its tree.
- A dwindle tree deeper than its space allows gives windows of zero size; M1.4, with clients'
  minimum sizes, decides what happens then (Hyprland floats a window that does not fit).
- The dwindle tree is walked by recursion, as deep as it has windows: 10,000 windows in one tree
  work, 100,000 overflow the stack. M1.4 bounds the windows a client may open, or walks the tree
  without recursion.
