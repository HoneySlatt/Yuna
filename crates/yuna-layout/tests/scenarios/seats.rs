//! Seats: an agent's seat never reaches the owner's (ADR-0015, ADR-0020).

use crate::harness::*;
use yuna_layout::{Direction, LayoutError, OpenOptions, Options, OutputId};

const AGENT_OUTPUT: OutputId = OutputId(10);

/// The owner with windows 1 and 2 on `MAIN`; an agent with its own 4K output.
fn owner_and_agent() -> Model {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.add_seat(AGENT));
    m.ok(|l| l.add_output(AGENT, AGENT_OUTPUT, output(10_000.0, 3840, 2160, 1.5)));
    m
}

#[test]
fn an_agent_works_on_its_own_output_and_the_owner_sees_no_change() {
    let mut m = owner_and_agent();
    let owner_before = (m.placed(MAIN), m.focused());
    m.open_with(AGENT, 100, OpenOptions::default());
    m.open_with(AGENT, 101, OpenOptions::default());
    m.ok(|l| l.focus(AGENT, Direction::Left));
    m.ok(|l| l.set_fullscreen(w(100), true));
    m.ok(|l| l.focus_workspace(AGENT, "tests"));
    m.ok(|l| l.toggle_special(AGENT, "scratch"));
    assert_eq!(m.info(100).output, Some(AGENT_OUTPUT));
    assert_eq!(m.info(100).seat, AGENT);
    assert_eq!((m.placed(MAIN), m.focused()), owner_before);
}

#[test]
fn an_agent_cannot_reach_the_owners_outputs() {
    let mut m = owner_and_agent();
    m.refused(
        LayoutError::Refused("the output belongs to another seat"),
        |l| l.focus_output(AGENT, MAIN),
    );
    m.refused(
        LayoutError::Refused("the output belongs to another seat"),
        |l| l.add_workspace(AGENT, "spy", Some(MAIN)),
    );
    // A workspace name is the seat's own: the agent's "1" is not the owner's "1".
    m.ok(|l| l.add_workspace(OWNER, "1", None));
    m.ok(|l| l.focus_workspace(AGENT, "1"));
    assert_eq!(m.workspaces(MAIN).0, vec!["-".to_owned(), "1".to_owned()]);
    m.open_with(AGENT, 100, OpenOptions::default());
    assert_eq!(m.info(100).output, Some(AGENT_OUTPUT));
}

#[test]
fn a_seat_without_output_keeps_its_windows_until_one_comes() {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(AGENT));
    m.open_with(AGENT, 1, OpenOptions::default());
    assert_eq!(m.info(1).output, None);
    m.refused(
        LayoutError::Refused("the window's workspace has no output"),
        |l| l.focus_window(w(1)),
    );
    m.ok(|l| l.add_output(AGENT, AGENT_OUTPUT, output(0.0, 1920, 1080, 1.0)));
    assert_eq!(m.info(1).output, Some(AGENT_OUTPUT));
    assert_eq!(m.layout.focused(AGENT), Some(w(1)));
}

#[test]
fn a_seat_goes_only_once_its_windows_are_closed() {
    let mut m = owner_and_agent();
    m.open_with(AGENT, 100, OpenOptions::default());
    m.refused(LayoutError::NotEmpty, |l| l.remove_seat(AGENT));
    m.ok(|l| l.close_window(w(100)));
    m.ok(|l| l.remove_seat(AGENT));
    assert!(m.layout.output_rect(AGENT_OUTPUT).is_err());
}
