//! Yuna's commands: what a caller may ask of the compositor, and why the compositor refuses one
//! ([ADR-0146](../../../docs/adr/0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md)).
//!
//! One command type serves every caller: the tests and MCP today, key bindings, gestures, the
//! configuration's actions and Hollow later
//! ([ADR-0082](../../../docs/adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md),
//! item 14). A [`Command`] is relative to its [`Caller`]: it acts on the caller's seat, and a
//! command on a window with no window named acts on the window the caller's seat focuses, as a key
//! binding does. The compositor answers with a [`Refusal`] when it does not apply a command; a
//! refused command changes nothing.
//!
//! # What it promises
//!
//! - Data only: no behaviour beyond naming what a command targets ([`Command::window`],
//!   [`Command::output`]), no wire format. Hollow's protocol is decided with Hollow's interface
//!   (Stage 2c), the events at Stage 2a's M2.4.
//! - A contract in 0.x
//!   ([ADR-0079](../../../docs/adr/0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)):
//!   once Hollow uses it, each change is announced in the changelog. Its enums are
//!   `#[non_exhaustive]`, so that a command or a reason added is not a break for the programs that
//!   match on them.
//! - Level 1 of the crate map, on `yuna-types` alone
//!   ([ADR-0145](../../../docs/adr/0145-the-crate-map-motion-on-the-layout-the-configuration-on-the-commands.md)).
//!
//! A command added states, in [`Command::window`] and [`Command::output`], the window and the
//! output it names, which the compositor checks are its caller's seat's before running it; the
//! compiler asks for it, since both list every variant.

#![forbid(unsafe_code)]

use std::fmt;

use yuna_types::{Direction, OutputId, SeatId, Tiling, WindowId};

/// Who asks a command, which decides what it may reach: its seat now; the lock's permission and
/// Hollow's role when they come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Caller {
    /// The seat the caller acts for: its commands reach that seat's outputs, workspaces and
    /// windows alone.
    pub seat: SeatId,
}

impl Caller {
    /// A caller acting for `seat`: a test, an agent through MCP, or the seat's own bindings.
    pub fn on_seat(seat: SeatId) -> Caller {
        Caller { seat }
    }
}

/// What a caller may ask. Workspaces are named by their names on the caller's seat. A window
/// given as `None` is the window the caller's seat focuses.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
    /// Focuses a window, showing its workspace on its output.
    FocusWindow(WindowId),
    /// Moves the focus to the next window on screen in a direction, or to the next output.
    Focus(Direction),
    /// Focuses an output: the window it shows focused, if any.
    FocusOutput(OutputId),
    /// Shows a workspace and focuses it.
    FocusWorkspace(String),
    /// Shows the workspace above (`Up`) or below (`Down`) the one shown, on the focused output.
    FocusWorkspaceRelative(Direction),
    /// Moves the focused window, or its column, one place in a direction.
    MoveFocused(Direction),
    /// Creates an empty named workspace, on `output` or the focused output.
    AddWorkspace {
        /// Its name, unique on the caller's seat.
        name: String,
        /// The output it goes to; the focused one when none.
        output: Option<OutputId>,
    },
    /// Removes an empty named workspace.
    RemoveWorkspace(String),
    /// Moves a window to a workspace; a window named is focused first, as the move follows the
    /// focus.
    MoveWindowToWorkspace {
        /// The window; the focused one when none.
        window: Option<WindowId>,
        /// The workspace's name.
        workspace: String,
    },
    /// Shows a special workspace over the focused output's, or hides it.
    ToggleSpecial(String),
    /// Moves a window to a special workspace; a window named is focused first.
    MoveWindowToSpecial {
        /// The window; the focused one when none.
        window: Option<WindowId>,
        /// The special workspace's name.
        special: String,
    },
    /// Sets how the focused workspace tiles its windows.
    SetTiling(Tiling),
    /// Makes a window's column as wide as its output, or not.
    SetFullWidth {
        /// The window; the focused one when none.
        window: Option<WindowId>,
        /// Full width when true.
        on: bool,
    },
    /// Makes a window fullscreen on its output, or not.
    SetFullscreen {
        /// The window; the focused one when none.
        window: Option<WindowId>,
        /// Fullscreen when true.
        on: bool,
    },
    /// Makes a window float over its workspace, or tiles it again.
    SetFloating {
        /// The window; the focused one when none.
        window: Option<WindowId>,
        /// Floating when true.
        on: bool,
    },
    /// Asks a window's client to close it; the window stays until the client does.
    CloseWindow(Option<WindowId>),
}

impl Command {
    /// The window the command names, if it names one: what the compositor checks is the
    /// caller's seat's before it runs the command (ADR-0146). A command on the focused window
    /// names none. The match lists every variant, so that a new one states its window here.
    pub fn window(&self) -> Option<WindowId> {
        match *self {
            Command::FocusWindow(window) => Some(window),
            Command::MoveWindowToWorkspace { window, .. }
            | Command::MoveWindowToSpecial { window, .. }
            | Command::SetFullWidth { window, .. }
            | Command::SetFullscreen { window, .. }
            | Command::SetFloating { window, .. }
            | Command::CloseWindow(window) => window,
            Command::Focus(_)
            | Command::FocusOutput(_)
            | Command::FocusWorkspace(_)
            | Command::FocusWorkspaceRelative(_)
            | Command::MoveFocused(_)
            | Command::AddWorkspace { .. }
            | Command::RemoveWorkspace(_)
            | Command::ToggleSpecial(_)
            | Command::SetTiling(_) => None,
        }
    }

    /// The output the command names, if it names one, checked as [`Command::window`] is. The
    /// match lists every variant, so that a new one states its output here.
    pub fn output(&self) -> Option<OutputId> {
        match *self {
            Command::FocusOutput(output) => Some(output),
            Command::AddWorkspace { output, .. } => output,
            Command::FocusWindow(_)
            | Command::Focus(_)
            | Command::FocusWorkspace(_)
            | Command::FocusWorkspaceRelative(_)
            | Command::MoveFocused(_)
            | Command::RemoveWorkspace(_)
            | Command::MoveWindowToWorkspace { .. }
            | Command::ToggleSpecial(_)
            | Command::MoveWindowToSpecial { .. }
            | Command::SetTiling(_)
            | Command::SetFullWidth { .. }
            | Command::SetFullscreen { .. }
            | Command::SetFloating { .. }
            | Command::CloseWindow(_) => None,
        }
    }
}

/// Why a command was refused. Its text is the sentence callers show, MCP's agents included.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// No seat has this id.
    UnknownSeat(SeatId),
    /// No output has this id.
    UnknownOutput(OutputId),
    /// No window has this id.
    UnknownWindow(WindowId),
    /// The caller's seat has no workspace of this name.
    UnknownWorkspace(String),
    /// The window is not one of the caller's seat: it is another seat's, or none at all, which
    /// the caller is not told apart.
    WindowNotOnSeat(WindowId, SeatId),
    /// The output is not one of the caller's seat, another seat's or none.
    OutputNotOnSeat(OutputId, SeatId),
    /// A command on the focused window, and the caller's seat focuses none.
    NoFocusedWindow,
    /// The name or id is already used.
    AlreadyExists,
    /// A value is not finite, or out of its range.
    InvalidValue,
    /// The workspace still holds windows.
    NotEmpty,
    /// The command does not apply here; the reason says why.
    Refused(&'static str),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::UnknownSeat(id) => write!(f, "no seat {}", id.0),
            Refusal::UnknownOutput(id) => write!(f, "no output {}", id.0),
            Refusal::UnknownWindow(id) => write!(f, "no window {}", id.0),
            Refusal::UnknownWorkspace(name) => write!(f, "no workspace named {name:?}"),
            Refusal::WindowNotOnSeat(window, seat) => {
                write!(f, "no window {} on seat {}", window.0, seat.0)
            }
            Refusal::OutputNotOnSeat(output, seat) => {
                write!(f, "no output {} on seat {}", output.0, seat.0)
            }
            Refusal::NoFocusedWindow => f.write_str("no window is focused"),
            Refusal::AlreadyExists => f.write_str("this id is already used"),
            Refusal::InvalidValue => f.write_str("a value is not finite or out of range"),
            Refusal::NotEmpty => f.write_str("it still holds windows"),
            Refusal::Refused(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for Refusal {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_says_what_was_not_found_by_its_id_or_name() {
        assert_eq!(
            Refusal::UnknownWindow(WindowId(7)).to_string(),
            "no window 7"
        );
        assert_eq!(
            Refusal::UnknownOutput(OutputId(2)).to_string(),
            "no output 2"
        );
        assert_eq!(Refusal::UnknownSeat(SeatId(3)).to_string(), "no seat 3");
        assert_eq!(
            Refusal::UnknownWorkspace("two".to_owned()).to_string(),
            "no workspace named \"two\""
        );
        assert_eq!(Refusal::NoFocusedWindow.to_string(), "no window is focused");
    }
}
