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
//! Behaviours follow niri for scrolling columns, focus, fullscreen and floating windows, and
//! Hyprland for dwindle, special workspaces and the pinned window
//! ([ADR-0019](../../../docs/adr/0019-the-layouts-behaviours.md)); workspaces are named, with
//! dynamic ones as an option ([ADR-0018](../../../docs/adr/0018-workspaces.md)).

pub mod geometry;

mod area;
mod dwindle;
mod floating;
mod layout;
mod scrolling;
mod workspace;

pub use layout::Layout;
pub use scrolling::ColumnWidth;

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
    /// The size a floating window opens at; [`Options::default_floating_size`] without it.
    pub size: Option<Size>,
    /// The width of its column; [`Options::default_width`] without it.
    pub width: Option<ColumnWidth>,
    /// The named workspace to open on, created on the seat's active output if missing; the seat's
    /// focused workspace without it.
    pub workspace: Option<String>,
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

/// A window placed on an output, as [`Layout::arrange`] gives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// The window.
    pub window: WindowId,
    /// Where it is drawn, in logical coordinates, on the output's physical grid. Columns scrolled
    /// out of view lie outside the output.
    pub rect: Rect,
    /// Whether it is tiled; floating otherwise.
    pub tiled: bool,
    /// Whether its client is told it is fullscreen: it is, or it is fake fullscreen.
    pub fullscreen: bool,
    /// Whether it has its seat's focus.
    pub activated: bool,
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
    /// Its name; none for an unnamed workspace.
    pub name: Option<String>,
    /// Whether the output shows it.
    pub shown: bool,
    /// Its tiling.
    pub tiling: Tiling,
    /// Its windows: the tiled ones in reading order, then the floating ones bottom to top.
    pub windows: Vec<WindowId>,
}
