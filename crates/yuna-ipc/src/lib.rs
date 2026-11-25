//! Yuna's commands and events: what a caller may ask of the compositor, why the compositor
//! refuses one
//! ([ADR-0146](../../../docs/adr/0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md)),
//! and what it tells has happened
//! ([ADR-0153](../../../docs/adr/0153-one-event-stream-marks-at-the-change-mcps-log-per-seat-the-bound.md)).
//!
//! One command type serves every caller: the tests and MCP today, key bindings, gestures, the
//! configuration's actions and Hollow later
//! ([ADR-0082](../../../docs/adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md),
//! item 14). A [`Command`] is relative to its [`Caller`]: it acts on the caller's seat, and a
//! command on a window with no window named acts on the window the caller's seat focuses, as a key
//! binding does. The compositor answers with a [`Refusal`] when it does not apply a command; a
//! refused command changes nothing.
//!
//! One event stream serves every reader in the same way (ADR-0082, item 14): an [`Event`] says
//! what happened on a seat, a window opened or placed, the focus moved, a client left; its
//! documentation gives when the compositor builds them, and their order. MCP's `events` tool tells
//! them today, Hollow later.
//!
//! # What it promises
//!
//! - Data only: no behaviour beyond naming what a command targets ([`Command::window`],
//!   [`Command::output`]) and the seat of an event ([`Event::seat`]), no wire format. Hollow's
//!   protocol is decided with Hollow's interface (Stage 2c).
//! - A contract in 0.x
//!   ([ADR-0079](../../../docs/adr/0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)):
//!   each change is announced in the changelog, as MCP's `events` tells the events already. Its
//!   enums are `#[non_exhaustive]`, so that a command, a reason or a kind of event added is not a
//!   break for the programs that match on them.
//! - Level 1 of the crate map, on `yuna-types` alone
//!   ([ADR-0151](../../../docs/adr/0151-the-crate-map-checked-by-rustc-and-a-script-yuna-transitional.md)).
//!
//! A command added states, in [`Command::window`] and [`Command::output`], the window and the
//! output it names, which the compositor checks are its caller's seat's before running it; the
//! compiler asks for it, since both list every variant.

#![forbid(unsafe_code)]

mod event;

use std::fmt;

use yuna_types::{Direction, OutputId, SeatId, Tiling, WindowId};

pub use event::{Event, MAX_EVENT_TEXT, ProtocolError};

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

/// Why a command was refused: one variant per reason, so that a caller tells them apart without
/// reading a sentence. Its text ([`Display`](fmt::Display)) is the sentence callers show, MCP's
/// agents included, and Yuna's one copy of it: the layout's reasons hold no text, and the
/// compositor turns each into its variant here (ADR-0082 item 23). Each reason the layout gives
/// has its variant, whether a command reaches it today or not.
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
    /// The output named is another seat's.
    OutputOfAnotherSeat,
    /// The caller's seat has no output, where a special workspace is shown on its active one
    /// ([`Command::ToggleSpecial`]).
    SeatWithoutOutput,
    /// The window's workspace is on no output, or its seat has none, so the window cannot be
    /// focused ([`Command::FocusWindow`], and the moves of a window named, which focus it first).
    WorkspaceWithoutOutput,
    /// The window is tiled, where only a floating window can be placed.
    WindowTiled,
    /// The window is pinned and was asked to be fullscreen; the caller unpins it first, as in
    /// Hyprland.
    PinnedNotFullscreen,
    /// In dwindle, another window's fullscreen holds the screen, and the window asked to be
    /// fullscreen or maximised may not take it ([`Command::SetFullscreen`],
    /// [`Command::SetFullWidth`]).
    ScreenHeld,
    /// The window to pin or unpin is not on the workspace its output shows: it is on a special
    /// workspace, a hidden one, or one without an output.
    PinnedOutsideShownWorkspace,
    /// The window to pin or unpin is not floating; a fullscreen window is tiled.
    PinnedNotFloating,
    /// The window is on no output, where it is placed on its output.
    WindowWithoutOutput,
    /// The window's output does not show it: its workspace is not shown, or a fullscreen window
    /// covers it.
    WindowNotShown,
    /// The compositor has no dispatch for this command. Yuna dispatches each of its commands, so
    /// none gives it today; [`Command`] being `#[non_exhaustive]`, the dispatch still answers for
    /// a command it does not know.
    UnknownCommand,
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
            Refusal::OutputOfAnotherSeat => f.write_str("the output belongs to another seat"),
            Refusal::SeatWithoutOutput => f.write_str("the seat has no output"),
            Refusal::WorkspaceWithoutOutput => f.write_str("the window's workspace has no output"),
            Refusal::WindowTiled => f.write_str("the window is tiled"),
            Refusal::PinnedNotFullscreen => f.write_str("a pinned window cannot be fullscreen"),
            Refusal::ScreenHeld => f.write_str("another window's fullscreen holds the screen"),
            Refusal::PinnedOutsideShownWorkspace => {
                f.write_str("only a window of a shown workspace can be pinned")
            }
            Refusal::PinnedNotFloating => f.write_str("only a floating window can be pinned"),
            Refusal::WindowWithoutOutput => f.write_str("the window is on no output"),
            Refusal::WindowNotShown => f.write_str("the window is not shown"),
            Refusal::UnknownCommand => f.write_str("Yuna does not know this command"),
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

    /// The sentence of every reason, which MCP's agents read: changing one changes the contract.
    #[test]
    fn every_refusal_has_its_sentence() {
        let sentences = [
            (Refusal::UnknownSeat(SeatId(3)), "no seat 3"),
            (Refusal::UnknownOutput(OutputId(2)), "no output 2"),
            (Refusal::UnknownWindow(WindowId(7)), "no window 7"),
            (
                Refusal::UnknownWorkspace("two".to_owned()),
                "no workspace named \"two\"",
            ),
            (
                Refusal::WindowNotOnSeat(WindowId(7), SeatId(3)),
                "no window 7 on seat 3",
            ),
            (
                Refusal::OutputNotOnSeat(OutputId(2), SeatId(3)),
                "no output 2 on seat 3",
            ),
            (Refusal::NoFocusedWindow, "no window is focused"),
            (Refusal::AlreadyExists, "this id is already used"),
            (
                Refusal::InvalidValue,
                "a value is not finite or out of range",
            ),
            (Refusal::NotEmpty, "it still holds windows"),
            (
                Refusal::OutputOfAnotherSeat,
                "the output belongs to another seat",
            ),
            (Refusal::SeatWithoutOutput, "the seat has no output"),
            (
                Refusal::WorkspaceWithoutOutput,
                "the window's workspace has no output",
            ),
            (Refusal::WindowTiled, "the window is tiled"),
            (
                Refusal::PinnedNotFullscreen,
                "a pinned window cannot be fullscreen",
            ),
            (
                Refusal::ScreenHeld,
                "another window's fullscreen holds the screen",
            ),
            (
                Refusal::PinnedOutsideShownWorkspace,
                "only a window of a shown workspace can be pinned",
            ),
            (
                Refusal::PinnedNotFloating,
                "only a floating window can be pinned",
            ),
            (Refusal::WindowWithoutOutput, "the window is on no output"),
            (Refusal::WindowNotShown, "the window is not shown"),
            (Refusal::UnknownCommand, "Yuna does not know this command"),
        ];
        for (refusal, sentence) in sentences {
            // Names every variant: one added fails to compile here, which points to the list
            // above, where its sentence goes.
            match refusal {
                Refusal::UnknownSeat(_)
                | Refusal::UnknownOutput(_)
                | Refusal::UnknownWindow(_)
                | Refusal::UnknownWorkspace(_)
                | Refusal::WindowNotOnSeat(..)
                | Refusal::OutputNotOnSeat(..)
                | Refusal::NoFocusedWindow
                | Refusal::AlreadyExists
                | Refusal::InvalidValue
                | Refusal::NotEmpty
                | Refusal::OutputOfAnotherSeat
                | Refusal::SeatWithoutOutput
                | Refusal::WorkspaceWithoutOutput
                | Refusal::WindowTiled
                | Refusal::PinnedNotFullscreen
                | Refusal::ScreenHeld
                | Refusal::PinnedOutsideShownWorkspace
                | Refusal::PinnedNotFloating
                | Refusal::WindowWithoutOutput
                | Refusal::WindowNotShown
                | Refusal::UnknownCommand => {}
            }
            assert_eq!(refusal.to_string(), sentence);
        }
    }
}
