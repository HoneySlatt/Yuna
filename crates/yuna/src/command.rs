//! The dispatch of `yuna-ipc`'s commands: each [`Command`] turned into the layout's operations for
//! its [`Caller`]'s seat, or into a request to a window's client
//! ([ADR-0146](../../../docs/adr/0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md)).

use yuna_ipc::{Caller, Command, Refusal};
use yuna_layout::{Layout, LayoutError};
use yuna_motion::Change;
use yuna_types::{OutputId, SeatId, WindowId};

use crate::state::State;

/// Runs `command` for `caller` (`Compositor::command`). What changed the layout arms every
/// output's next refresh; a command refused before it changed anything arms nothing.
pub(crate) fn run(state: &mut State, caller: Caller, command: Command) -> Result<(), Refusal> {
    if let Command::CloseWindow(window) = command {
        check_reach(&state.layout, caller.seat, &command)?;
        let window = target(&state.layout, caller.seat, window)?;
        return state
            .close_window(window)
            .map_err(|_| Refusal::UnknownWindow(window));
    }
    check_reach(&state.layout, caller.seat, &command)?;
    let applied = apply_to_layout(&mut state.layout, caller.seat, command);
    if applied.changed {
        state.mark_layout_changed(Change::Animated);
    }
    applied.result
}

/// What a command did to the layout: its answer, and whether the layout changed. A refused
/// operation leaves the layout as it was, but a move of a window named focuses it first, and
/// that focus stays when the move is refused.
struct Applied {
    result: Result<(), Refusal>,
    changed: bool,
}

impl Applied {
    /// One operation's answer: the layout changed if it was done.
    fn of(result: Result<(), LayoutError>) -> Applied {
        Applied {
            changed: result.is_ok(),
            result: result.map_err(refusal),
        }
    }
}

/// The layout's operations for `command` on `seat`'s behalf.
fn apply_to_layout(layout: &mut Layout, seat: SeatId, command: Command) -> Applied {
    let done = match command {
        Command::FocusWindow(window) => layout.focus_window(window),
        Command::Focus(direction) => layout.focus(seat, direction),
        Command::FocusOutput(output) => layout.focus_output(seat, output),
        Command::FocusWorkspace(name) => layout.focus_workspace(seat, &name),
        Command::FocusWorkspaceRelative(direction) => {
            layout.focus_workspace_relative(seat, direction)
        }
        Command::MoveFocused(direction) => layout.move_focused(seat, direction),
        Command::AddWorkspace { name, output } => layout.add_workspace(seat, &name, output),
        Command::RemoveWorkspace(name) => layout.remove_workspace(seat, &name),
        Command::MoveWindowToWorkspace { window, workspace } => {
            return move_named(layout, window, |layout| {
                layout.move_to_workspace(seat, &workspace)
            });
        }
        Command::ToggleSpecial(name) => layout.toggle_special(seat, &name),
        Command::MoveWindowToSpecial { window, special } => {
            return move_named(layout, window, |layout| {
                layout.move_to_special(seat, &special)
            });
        }
        Command::SetTiling(tiling) => layout.set_tiling(seat, tiling),
        Command::SetFullWidth { window, on } => {
            return on_window(layout, seat, window, |layout, window| {
                layout.set_full_width(window, on)
            });
        }
        Command::SetFullscreen { window, on } => {
            return on_window(layout, seat, window, |layout, window| {
                layout.set_fullscreen(window, on)
            });
        }
        Command::SetFloating { window, on } => {
            return on_window(layout, seat, window, |layout, window| {
                layout.set_floating(window, on)
            });
        }
        // Not a change of the layout: answered by `run`.
        Command::CloseWindow(_) => Ok(()),
        // `Command` is `#[non_exhaustive]` for the programs outside the workspace that match on
        // it; a variant added there is dispatched here in the same change, and tested.
        _ => Err(LayoutError::Refused("Yuna does not know this command")),
    };
    Applied::of(done)
}

/// Refuses a command that names a window or an output of another seat than `seat`'s, or one the
/// layout does not hold, alike: a caller reaches its own seat alone (ADR-0138, ADR-0146).
fn check_reach(layout: &Layout, seat: SeatId, command: &Command) -> Result<(), Refusal> {
    if let Some(window) = named_window(command)
        && layout.seat_of_window(window) != Some(seat)
    {
        return Err(Refusal::WindowNotOnSeat(window, seat));
    }
    if let Some(output) = named_output(command)
        && layout.seat_of_output(output) != Some(seat)
    {
        return Err(Refusal::OutputNotOnSeat(output, seat));
    }
    Ok(())
}

/// The window `command` names, if it names one.
fn named_window(command: &Command) -> Option<WindowId> {
    match *command {
        Command::FocusWindow(window) => Some(window),
        Command::MoveWindowToWorkspace { window, .. }
        | Command::MoveWindowToSpecial { window, .. }
        | Command::SetFullWidth { window, .. }
        | Command::SetFullscreen { window, .. }
        | Command::SetFloating { window, .. }
        | Command::CloseWindow(window) => window,
        _ => None,
    }
}

/// The output `command` names, if it names one.
fn named_output(command: &Command) -> Option<OutputId> {
    match *command {
        Command::FocusOutput(output) => Some(output),
        Command::AddWorkspace { output, .. } => output,
        _ => None,
    }
}

/// Does `step` to the window a command names, or to the one `seat` focuses.
fn on_window(
    layout: &mut Layout,
    seat: SeatId,
    window: Option<WindowId>,
    step: impl FnOnce(&mut Layout, WindowId) -> Result<(), LayoutError>,
) -> Applied {
    match target(layout, seat, window) {
        Ok(window) => Applied::of(step(layout, window)),
        Err(refused) => Applied {
            result: Err(refused),
            changed: false,
        },
    }
}

/// Focuses `window` when one is named, then moves the focused window with `step`.
fn move_named(
    layout: &mut Layout,
    window: Option<WindowId>,
    step: impl FnOnce(&mut Layout) -> Result<(), LayoutError>,
) -> Applied {
    let focused = match window {
        Some(window) => match layout.focus_window(window) {
            Ok(()) => true,
            Err(error) => return Applied::of(Err(error)),
        },
        None => false,
    };
    let moved = Applied::of(step(layout));
    Applied {
        changed: moved.changed || focused,
        result: moved.result,
    }
}

/// The window a command names, or the one `seat` focuses.
fn target(layout: &Layout, seat: SeatId, window: Option<WindowId>) -> Result<WindowId, Refusal> {
    match window {
        Some(window) => Ok(window),
        None => layout.focused(seat).ok_or(Refusal::NoFocusedWindow),
    }
}

/// The refusal a caller sees for what the layout refused.
pub(crate) fn refusal(error: LayoutError) -> Refusal {
    match error {
        LayoutError::UnknownSeat(seat) => Refusal::UnknownSeat(seat),
        LayoutError::UnknownOutput(output) => Refusal::UnknownOutput(output),
        LayoutError::UnknownWindow(window) => Refusal::UnknownWindow(window),
        LayoutError::UnknownWorkspace(name) => Refusal::UnknownWorkspace(name),
        LayoutError::AlreadyExists => Refusal::AlreadyExists,
        LayoutError::InvalidValue => Refusal::InvalidValue,
        LayoutError::NotEmpty => Refusal::NotEmpty,
        LayoutError::Refused(reason) => Refusal::Refused(reason),
    }
}
