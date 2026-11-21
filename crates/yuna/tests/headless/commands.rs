//! `yuna-ipc`'s commands through `Compositor::command`
//! ([ADR-0146](../../../../docs/adr/0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md)):
//! what a command with no window acts on, and what a caller sees when one is refused. Each
//! operation's own behaviour is the layout model's, tested there and in the stories that use it.

use yuna::OWNER_SEAT;
use yuna_ipc::{Command, Refusal};
use yuna_types::{Direction, WindowId};

use crate::client::Client;
use crate::{RED, manual_compositor, open_and_commit, owner, run_armed_deadlines};

/// A command on a window that names none acts on the window the caller's seat focuses, as a key
/// binding does; the others keep their state.
#[test]
fn a_command_with_no_window_acts_on_the_focused_one() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    open_and_commit(&mut compositor, &mut client, RED);
    open_and_commit(&mut compositor, &mut client, RED);
    run_armed_deadlines(&mut compositor);
    let focused = compositor
        .layout()
        .focused(OWNER_SEAT)
        .expect("the last window opened is focused");
    let other = compositor
        .windows()
        .iter()
        .map(|window| window.id)
        .find(|&id| id != focused)
        .expect("a second window");

    compositor
        .command(
            owner(),
            Command::SetFloating {
                window: None,
                on: true,
            },
        )
        .expect("the focused window floats");

    let layout = compositor.layout();
    assert!(layout.window(focused).expect("still there").floating);
    assert!(!layout.window(other).expect("still there").floating);
}

/// A command on the focused window when the seat focuses none is refused, and says so.
#[test]
fn a_command_on_the_focused_window_when_none_is_focused_is_refused() {
    let mut compositor = manual_compositor();
    for command in [
        Command::SetFullWidth {
            window: None,
            on: true,
        },
        Command::SetFullscreen {
            window: None,
            on: true,
        },
        Command::CloseWindow(None),
    ] {
        assert_eq!(
            compositor.command(owner(), command.clone()),
            Err(Refusal::NoFocusedWindow),
            "{command:?}"
        );
    }
    assert_eq!(Refusal::NoFocusedWindow.to_string(), "no window is focused");
}

/// A refusal says what the layout refused, in the words MCP gives its agents.
#[test]
fn a_refused_command_says_why_in_the_layouts_words() {
    let mut compositor = manual_compositor();
    let refused = compositor
        .command(owner(), Command::RemoveWorkspace("nowhere".to_owned()))
        .expect_err("no such workspace");
    assert_eq!(refused, Refusal::UnknownWorkspace("nowhere".to_owned()));
    assert_eq!(refused.to_string(), "no workspace named \"nowhere\"");
    assert_eq!(
        compositor.command(owner(), Command::FocusWindow(WindowId(99))),
        Err(Refusal::UnknownWindow(WindowId(99)))
    );
    assert_eq!(
        compositor.command(owner(), Command::CloseWindow(Some(WindowId(99)))),
        Err(Refusal::UnknownWindow(WindowId(99)))
    );
}

/// The workspace below and above, on the focused output, as a key binding steps through them.
#[test]
fn a_relative_focus_steps_through_the_outputs_workspaces() {
    let mut compositor = manual_compositor();
    let output = compositor.outputs()[0];
    for name in ["one", "two"] {
        compositor
            .command(
                owner(),
                Command::AddWorkspace {
                    name: name.to_owned(),
                    output: Some(output),
                },
            )
            .expect("a new workspace");
    }
    let shown = |compositor: &yuna::Compositor| {
        compositor
            .layout()
            .workspaces(output)
            .expect("the output's workspaces")
            .into_iter()
            .find(|workspace| workspace.shown)
            .and_then(|workspace| workspace.name)
    };
    compositor
        .command(owner(), Command::FocusWorkspace("one".to_owned()))
        .expect("workspace one");
    compositor
        .command(owner(), Command::FocusWorkspaceRelative(Direction::Down))
        .expect("the workspace below");
    assert_eq!(shown(&compositor).as_deref(), Some("two"));
    compositor
        .command(owner(), Command::FocusWorkspaceRelative(Direction::Up))
        .expect("the workspace above");
    assert_eq!(shown(&compositor).as_deref(), Some("one"));
}

/// A refused command changes nothing, so nothing is drawn for it: no refresh is armed, and the
/// description agents read does not change.
#[test]
fn a_refused_command_arms_no_refresh() {
    let mut compositor = manual_compositor();
    run_armed_deadlines(&mut compositor);
    assert_eq!(compositor.next_deadline(), None);
    let generation = compositor.description_generation();
    for command in [
        Command::RemoveWorkspace("nowhere".to_owned()),
        Command::FocusWindow(WindowId(99)),
        Command::SetFloating {
            window: None,
            on: true,
        },
    ] {
        assert!(compositor.command(owner(), command).is_err());
    }
    assert_eq!(compositor.next_deadline(), None);
    assert_eq!(compositor.description_generation(), generation);
}
