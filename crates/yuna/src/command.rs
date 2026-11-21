//! The dispatch of `yuna-ipc`'s commands: each [`Command`] turned into the layout's operations for
//! its [`Caller`]'s seat, or into a request to a window's client
//! ([ADR-0146](../../../docs/adr/0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md)).

use yuna_ipc::{Caller, Command, Refusal};
use yuna_layout::{Layout, LayoutError};
use yuna_motion::Change;
use yuna_types::{SeatId, WindowId};

use crate::state::State;

/// Runs `command` for `caller` (`Compositor::command`).
pub(crate) fn run(state: &mut State, caller: Caller, command: Command) -> Result<(), Refusal> {
    if let Command::CloseWindow(window) = command {
        let window = target(&state.layout, caller.seat, window)?;
        return state
            .close_window(window)
            .map_err(|_| Refusal::UnknownWindow(window));
    }
    let result = apply_to_layout(&mut state.layout, caller.seat, command);
    state.mark_layout_changed(Change::Animated);
    result
}

/// The layout's operations for `command` on `seat`'s behalf.
fn apply_to_layout(layout: &mut Layout, seat: SeatId, command: Command) -> Result<(), Refusal> {
    let changed = match command {
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
            focus_named(layout, window).and_then(|()| layout.move_to_workspace(seat, &workspace))
        }
        Command::ToggleSpecial(name) => layout.toggle_special(seat, &name),
        Command::MoveWindowToSpecial { window, special } => {
            focus_named(layout, window).and_then(|()| layout.move_to_special(seat, &special))
        }
        Command::SetTiling(tiling) => layout.set_tiling(seat, tiling),
        Command::SetFullWidth { window, on } => {
            let window = target(layout, seat, window)?;
            layout.set_full_width(window, on)
        }
        Command::SetFullscreen { window, on } => {
            let window = target(layout, seat, window)?;
            layout.set_fullscreen(window, on)
        }
        Command::SetFloating { window, on } => {
            let window = target(layout, seat, window)?;
            layout.set_floating(window, on)
        }
        // Not a change of the layout: answered by `run`.
        Command::CloseWindow(_) => Ok(()),
        // `Command` is `#[non_exhaustive]` for the programs outside the workspace that match on
        // it; a variant added there is dispatched here in the same change, and tested.
        _ => Err(LayoutError::Refused("Yuna does not know this command")),
    };
    changed.map_err(refusal)
}

/// Focuses `window` when one is named: a move of the focused window then moves it.
fn focus_named(layout: &mut Layout, window: Option<WindowId>) -> Result<(), LayoutError> {
    window.map_or(Ok(()), |window| layout.focus_window(window))
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
