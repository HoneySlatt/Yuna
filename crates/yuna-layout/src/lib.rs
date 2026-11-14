//! Yuna's layout model: where every window is, on which workspace, output and seat, and at which
//! rectangle. It holds no Wayland or Smithay object, so that every behaviour is a test run without
//! a compositor ([ADR-0005](../../../docs/adr/0005-the-layouts.md)).
//!
//! The model is a tree that each seat owns a branch of: a [`Layout`] holds seats; a seat holds its
//! outputs, the workspaces left without an output, and its hidden special workspaces; an output
//! holds its workspaces, shows one of them, and may show a special workspace over it. An
//! operation of one seat reaches only that seat's branch, which is how an agent's seat never
//! reaches the owner's ([ADR-0015](../../../docs/adr/0015-agents-workspaces.md),
//! [ADR-0020](../../../docs/adr/0020-the-layout-models-shape.md)).
//!
//! Callers change the model through [`Layout`]'s operations and read it through
//! [`Layout::arrange`], [`Layout::focused`] and [`Layout::window`]. Operations that name a seat
//! act on what that seat focuses (key bindings); operations that name a window act on that window
//! (requests from its client). Every operation takes ids that may be stale and answers with an
//! error rather than a panic; none leaves the model inconsistent, which
//! [`Layout::check_invariants`] checks.
//!
//! The caller names windows, outputs and seats; the model names what it creates itself:
//! workspaces ([`WorkspaceId`]) and scrolling columns ([`ColumnId`]), from counters it owns,
//! never reusing an id. Each window [`Layout::arrange`] places says its workspace, its [`Layer`]
//! and its column; [`Layout::shown_workspaces`] gives the workspaces an output shows, each with
//! its camera: in scrolling columns, the view's position on the strip. So what animates the
//! model's jumps ([ADR-0052](../../../docs/adr/0052-motion-in-a-pure-crate.md)) can follow each
//! window, column and workspace from one arrangement to the next, and tell a strip that moved
//! under the view from a view that moved over the strip ([`WorkspaceView`]).
//!
//! Clients do not always take the size they are given. The compositor passes on what each one
//! asks and does: its minimum and maximum sizes ([`Layout::set_size_limits`]) and the size it
//! commits ([`Layout::commit_size`]). [`Layout::arrange`] gives each window the size it is asked,
//! for its configure, and the rectangle it is drawn in; [`Layout::size_on_open`] gives a new
//! window's first configure ([ADR-0026](../../../docs/adr/0026-real-clients-in-the-layout.md)).
//!
//! Behaviours follow niri for scrolling columns, focus, fullscreen and floating windows, and
//! Hyprland for dwindle, special workspaces and the pinned window
//! ([ADR-0019](../../../docs/adr/0019-the-layouts-behaviours.md)); workspaces are named, with
//! dynamic ones as an option ([ADR-0018](../../../docs/adr/0018-workspaces.md)).

pub mod geometry;

mod area;
mod dwindle;
mod floating;
mod ids;
mod layout;
mod scrolling;
mod sizing;
mod workspace;

pub use layout::{Layout, MAX_SIZE};
pub use scrolling::ColumnWidth;

use std::num::NonZeroU64;

use geometry::{Rect, Size};

/// A window, named by the caller. The caller keeps ids unique among the windows it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

/// An output, named by the caller. The caller gives the same id to the same monitor each time it
/// is plugged in, so that its workspaces come back to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputId(pub u64);

/// A seat: a pointer and a keyboard with their own focus, the owner's or an agent's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SeatId(pub u64);

/// A workspace's id, which the model gives every workspace it creates: named, unnamed or special.
/// Ids start at 1 and are never reused for the life of a [`Layout`], across seats. A workspace
/// keeps its id when it moves to another output and back, and when it loses its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspaceId(pub u64);

/// A column's id, which the model gives every column of scrolling columns it creates. Ids start
/// at 1 and are never reused for the life of a [`Layout`], across seats and workspaces. A column
/// keeps its id while it exists: when it moves along its strip, a window joins or leaves it, it
/// changes width, or it goes fullscreen and back. A window that leaves it for a column of its own
/// gets a new column, with a new id; a column whose last window leaves is gone. A workspace whose
/// tiling changes to dwindle and back gets new columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ColumnId(pub NonZeroU64);

/// The part of its workspace a window is placed in, which says how it moves: the columns of a
/// strip slide together under one camera, the others do not
/// ([ADR-0052](../../../docs/adr/0052-motion-in-a-pure-crate.md)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    /// A tiled window of scrolling columns: on the workspace's strip, in a column.
    Strip,
    /// A tiled window of a dwindle tree.
    Tiles,
    /// A floating window, not pinned.
    Floating,
    /// A floating window pinned over every workspace of its output.
    Pinned,
}

/// A direction on screen. Workspaces of an output are stacked vertically: up is the previous one,
/// down the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Towards the left.
    Left,
    /// Towards the right.
    Right,
    /// Upwards.
    Up,
    /// Downwards.
    Down,
}

/// How a workspace tiles its windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tiling {
    /// Columns on a strip that scrolls horizontally (niri).
    Scrolling,
    /// Each new window splits the focused one's space (Hyprland).
    Dwindle,
}

/// The model's settings, read from the configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The gap between tiled windows and around them, in logical units.
    pub gap: f64,
    /// The width of a new column.
    pub default_width: ColumnWidth,
    /// The widths a column cycles through.
    pub preset_widths: Vec<ColumnWidth>,
    /// The tiling of a new workspace.
    pub default_tiling: Tiling,
    /// Whether each output keeps one empty unnamed workspace at the end of its list, as niri does
    /// ([ADR-0018](../../../docs/adr/0018-workspaces.md)).
    pub dynamic_workspaces: bool,
    /// The size of a window opened floating without one of its own.
    pub default_floating_size: Size,
}

impl Default for Options {
    /// The defaults of the owner's configuration today: columns of half the width, presets of a
    /// third, a half, two thirds and the whole.
    fn default() -> Options {
        Options {
            gap: 16.0,
            default_width: ColumnWidth::Proportion(0.5),
            preset_widths: vec![
                ColumnWidth::Proportion(1.0 / 3.0),
                ColumnWidth::Proportion(0.5),
                ColumnWidth::Proportion(2.0 / 3.0),
                ColumnWidth::Proportion(1.0),
            ],
            default_tiling: Tiling::Scrolling,
            dynamic_workspaces: false,
            default_floating_size: Size { w: 800.0, h: 600.0 },
        }
    }
}

/// An output's place and mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputConfig {
    /// The top-left corner, in logical coordinates shared by all outputs.
    pub position: geometry::Point,
    /// The mode's width in physical pixels.
    pub physical_width: u32,
    /// The mode's height in physical pixels.
    pub physical_height: u32,
    /// The scale: physical pixels per logical unit.
    pub scale: f64,
}

/// How a window opens.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenOptions {
    /// Open floating rather than tiled.
    pub floating: bool,
    /// Open fullscreen.
    pub fullscreen: bool,
    /// A rule's size for the window when it floats: it is asked that size and keeps it, whatever
    /// its client commits. Without it, a floating window opens at its committed size, else at its
    /// fixed size, else at [`Options::default_floating_size`], and follows its commits.
    pub size: Option<Size>,
    /// The width of its column; [`Options::default_width`] without it.
    pub width: Option<ColumnWidth>,
    /// The named workspace to open on, created on the seat's active output if missing; without
    /// it, the parent's workspace for a dialog, else the seat's focused workspace.
    pub workspace: Option<String>,
    /// The window this one is a dialog of (xdg-shell's parent), on the same seat. A dialog opens
    /// floating, centred on its parent, and takes the focus only from it
    /// ([ADR-0026](../../../docs/adr/0026-real-clients-in-the-layout.md)).
    pub parent: Option<WindowId>,
    /// The smallest size its client asks; 0 on an axis for none.
    pub min_size: Size,
    /// The largest size its client asks; 0 on an axis for none. A window whose minimum and
    /// maximum heights are equal opens floating, as in niri.
    pub max_size: Size,
    /// The window geometry its client committed before it was mapped. Refused unless positive
    /// and at most 100,000 on each axis. A floating window opens centred at this size: without
    /// it, it opens at the default floating size and keeps that corner when its first commit
    /// comes, so the compositor opens a window at its first buffer, its size known.
    pub committed: Option<Size>,
}

/// Why an operation was refused. The model is unchanged when an operation returns one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// No seat has this id.
    UnknownSeat(SeatId),
    /// No output has this id.
    UnknownOutput(OutputId),
    /// No window has this id.
    UnknownWindow(WindowId),
    /// The seat has no workspace of this name.
    UnknownWorkspace(String),
    /// The id is already used.
    AlreadyExists,
    /// A number was not finite, or out of its range.
    InvalidValue,
    /// It still holds windows.
    NotEmpty,
    /// The operation does not apply here; the reason says why.
    Refused(&'static str),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::UnknownSeat(id) => write!(f, "no seat {}", id.0),
            LayoutError::UnknownOutput(id) => write!(f, "no output {}", id.0),
            LayoutError::UnknownWindow(id) => write!(f, "no window {}", id.0),
            LayoutError::UnknownWorkspace(name) => write!(f, "no workspace named {name:?}"),
            LayoutError::AlreadyExists => f.write_str("this id is already used"),
            LayoutError::InvalidValue => f.write_str("a value is not finite or out of range"),
            LayoutError::NotEmpty => f.write_str("it still holds windows"),
            LayoutError::Refused(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for LayoutError {}

/// A window placed on an output, as [`Layout::arrange`] and [`Layout::arrange_workspace`] give it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// The window.
    pub window: WindowId,
    /// Where it is drawn, in logical coordinates, on the output's physical grid: the window's
    /// top-left corner at the rectangle's, the window clipped to it. Its size is what the client
    /// committed; before its first commit, the size it is asked, or for a floating window that
    /// chooses its own, the size it opened at. A dwindle window is clipped to its slot, a
    /// fullscreen one to the output. Columns scrolled out of view lie outside the output. Empty only for a dwindle
    /// window whose slot is empty, in a tree deeper than its space.
    pub rect: Rect,
    /// The size its client is asked, in whole logical units: at least 1 × 1 for a tiled or
    /// fullscreen window; for a floating window that chooses its own size, 0 on each axis not
    /// fixed by its minimum and maximum.
    pub configure: Size,
    /// Whether it is tiled; floating otherwise.
    pub tiled: bool,
    /// Whether its client is told it is fullscreen: it is, or it is fake fullscreen.
    pub fullscreen: bool,
    /// Whether it has its seat's focus.
    pub activated: bool,
    /// The workspace that holds it: for [`Layout::arrange`], the main workspace its output shows
    /// or the special one shown over it; a pinned window is always on the main one, even under a
    /// special one. For [`Layout::arrange_workspace`], the one asked.
    pub workspace: WorkspaceId,
    /// The part of its workspace it is placed in. A window really fullscreen is tiled, so it keeps
    /// its tiling's layer, [`Layer::Strip`] or [`Layer::Tiles`]; a fake fullscreen one keeps its
    /// own layer; `fullscreen` says the rest.
    pub layer: Layer,
    /// Its column: some exactly for [`Layer::Strip`].
    pub column: Option<ColumnId>,
}

/// Where a window is and what state it is in, as [`Layout::window`] gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    /// The seat whose workspaces hold it.
    pub seat: SeatId,
    /// The output it is on; none when its workspace has no output or is a hidden special one.
    pub output: Option<OutputId>,
    /// Its workspace's name; none for an unnamed workspace.
    pub workspace: Option<String>,
    /// Whether its workspace is a special one.
    pub special: bool,
    /// Whether it floats.
    pub floating: bool,
    /// Whether it is fullscreen (really, not fake).
    pub fullscreen: bool,
    /// Whether its client is told it is fullscreen while its place does not change.
    pub fake_fullscreen: bool,
    /// Whether its column is full width (scrolling), or it is maximised (dwindle).
    pub full_width: bool,
    /// Whether it is pinned over every workspace of its output.
    pub pinned: bool,
}

/// A workspace of an output, as [`Layout::workspaces`] gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceInfo {
    /// Its id, which [`Layout::arrange_workspace`] takes.
    pub id: WorkspaceId,
    /// Its name; none for an unnamed workspace.
    pub name: Option<String>,
    /// Whether the output shows it.
    pub shown: bool,
    /// Its tiling.
    pub tiling: Tiling,
    /// Its windows: the tiled ones in reading order, then the floating ones bottom to top.
    pub windows: Vec<WindowId>,
}

/// A workspace an output shows, as [`Layout::shown_workspaces`] gives it: what following its
/// windows from one arrangement to the next needs
/// ([ADR-0052](../../../docs/adr/0052-motion-in-a-pure-crate.md)).
///
/// Each window of [`Layer::Strip`] has a place on its workspace's strip, its *strip position*:
/// `rect.x - working.x + view_x`, from its [`Placed::rect`], on the output's physical grid. An
/// operation changes it only when it moves the window's column along the strip, or the window to
/// another column. Focusing another column moves the view alone: `view_x` changes, every strip
/// position stays. A column that comes in, goes or changes width left of the focused one moves the
/// columns on its right along the strip, and `view_x` by as much, so that those columns stay where
/// they are on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceView {
    /// The workspace.
    pub id: WorkspaceId,
    /// The camera: in scrolling columns, the strip position shown at the working area's left
    /// edge, on the output's physical grid; none in dwindle, which has no strip.
    pub view_x: Option<f64>,
    /// The working area it is arranged in, on the output's physical grid.
    pub working: Rect,
}

/// The workspaces an output shows, as [`Layout::shown_workspaces`] gives them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShownWorkspaces {
    /// The workspace the output shows.
    pub main: WorkspaceView,
    /// The special workspace shown over it, if any.
    pub special: Option<WorkspaceView>,
}
