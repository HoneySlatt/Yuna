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
    run_armed_deadlines(&mut compositor);
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
        Command::MoveWindowToWorkspace {
            window: None,
            workspace: "two".to_owned(),
        },
        Command::MoveWindowToSpecial {
            window: None,
            special: "scratch".to_owned(),
        },
    ] {
        assert_eq!(
            compositor.command(owner(), command.clone()),
            Err(Refusal::NoFocusedWindow),
            "{command:?}"
        );
    }
    assert_eq!(
        compositor.next_deadline(),
        None,
        "a refused command draws nothing"
    );
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
        Err(Refusal::WindowNotOnSeat(WindowId(99), OWNER_SEAT))
    );
    assert_eq!(
        compositor.command(owner(), Command::CloseWindow(Some(WindowId(99)))),
        Err(Refusal::WindowNotOnSeat(WindowId(99), OWNER_SEAT))
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

/// A caller reaches its own seat's windows and outputs alone: another seat's are refused as if
/// they did not exist, so that an agent's seat never reaches the owner's nor learns what it holds
/// ([ADR-0138](../../../../docs/adr/0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md)).
#[test]
fn a_caller_reaches_neither_the_windows_nor_the_outputs_of_another_seat() {
    let mut compositor = manual_compositor();
    let mut owners = Client::connect(&mut compositor);
    open_and_commit(&mut compositor, &mut owners, RED);
    run_armed_deadlines(&mut compositor);
    let owners_window = compositor.windows()[0].id;
    let owners_output = compositor.outputs()[0];
    let agent = compositor.add_seat().expect("an agent's seat");
    let agents_output = compositor
        .add_output(agent, yuna::OutputMode::default())
        .expect("the agent's output");
    let as_agent = yuna_ipc::Caller::on_seat(agent);
    let window = Some(owners_window);
    for command in [
        Command::FocusWindow(owners_window),
        Command::CloseWindow(window),
        Command::SetFullWidth { window, on: true },
        Command::SetFullscreen { window, on: true },
        Command::SetFloating { window, on: true },
        Command::MoveWindowToWorkspace {
            window,
            workspace: "spy".to_owned(),
        },
        Command::MoveWindowToSpecial {
            window,
            special: "spy".to_owned(),
        },
    ] {
        assert_eq!(command.window(), window, "{command:?} names the window");
        assert_eq!(
            compositor.command(as_agent, command.clone()),
            Err(Refusal::WindowNotOnSeat(owners_window, agent)),
            "{command:?}"
        );
    }
    for command in [
        Command::FocusOutput(owners_output),
        Command::AddWorkspace {
            name: "spy".to_owned(),
            output: Some(owners_output),
        },
    ] {
        assert_eq!(
            command.output(),
            Some(owners_output),
            "{command:?} names the output"
        );
        assert_eq!(
            compositor.command(as_agent, command.clone()),
            Err(Refusal::OutputNotOnSeat(owners_output, agent)),
            "{command:?}"
        );
    }
    assert_eq!(
        Refusal::WindowNotOnSeat(owners_window, agent).to_string(),
        format!("no window {} on seat {}", owners_window.0, agent.0)
    );
    let layout = compositor.layout();
    assert!(
        !layout
            .window(owners_window)
            .expect("still there")
            .fullscreen
    );
    assert_eq!(layout.focused(OWNER_SEAT), Some(owners_window));
    compositor
        .command(as_agent, Command::FocusOutput(agents_output))
        .expect("the agent's own output");
}
