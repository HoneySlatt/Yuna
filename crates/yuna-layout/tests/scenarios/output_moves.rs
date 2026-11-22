//! What happens to an output's neighbours, the seat's active output and the workspaces when an
//! output comes, goes or changes mode, and where a window sent to a special workspace lands.

use crate::harness::*;
use yuna_layout::geometry::Point;
use yuna_layout::{LayoutError, Options, OutputConfig};
use yuna_types::{Direction, OutputId};

const THIRD: OutputId = OutputId(3);

/// The owner's seat with `MAIN`, `SIDE` and `THIRD`, each 1920 × 1080, side by side from the
/// origin.
fn three_outputs() -> Model {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.add_output(OWNER, THIRD, output(3840.0, 1920, 1080, 1.0)));
    m
}

/// A new mode for an output moves the seat's outputs on its right, by as much as its right edge
/// moved: bigger pushes them away, smaller draws them near; those on its left stay.
#[test]
fn an_output_changing_mode_moves_the_outputs_on_its_right_by_as_much() {
    let mut m = three_outputs();
    let left = OutputId(4);
    m.ok(|l| l.add_output(OWNER, left, output(-1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.configure_output(MAIN, 2560, 1440, 1.0));
    assert_eq!(m.layout.output_rect(MAIN).unwrap().right(), 2560.0);
    assert_eq!(m.layout.output_rect(SIDE).unwrap().x, 2560.0);
    assert_eq!(m.layout.output_rect(THIRD).unwrap().x, 4480.0);
    assert_eq!(m.layout.output_rect(left).unwrap().x, -1920.0);
    m.ok(|l| l.configure_output(MAIN, 1280, 720, 1.0));
    assert_eq!(m.layout.output_rect(SIDE).unwrap().x, 1280.0);
    assert_eq!(m.layout.output_rect(THIRD).unwrap().x, 3200.0);
    assert_eq!(m.layout.output_rect(left).unwrap().x, -1920.0);
}

/// Removing an output left of the seat's active output keeps that output active; removing one on
/// its right does too. Yuna's own rule, written on `Layout::remove_output`: its workspaces move to
/// the seat's active output. No local copy of niri's source exists to check it against, and no niri
/// test was read for it.
#[test]
fn removing_an_output_left_of_the_active_one_keeps_the_active_output() {
    let mut m = three_outputs();
    m.ok(|l| l.focus_output(OWNER, SIDE));
    m.open(1);
    m.ok(|l| l.remove_output(MAIN));
    assert_eq!(m.layout.active_output(OWNER), Some(SIDE));
    assert_eq!(m.info(1).output, Some(SIDE));
    m.ok(|l| l.remove_output(THIRD));
    assert_eq!(m.layout.active_output(OWNER), Some(SIDE));
}

/// Removing the active output makes the one that took its place the active one, or the one before
/// it when it was the last. Yuna's own rule, written on `Layout::remove_output` (no local copy of
/// niri's source exists to check it against, and no niri test was read for it).
#[test]
fn removing_the_active_output_makes_the_next_one_active() {
    let mut m = three_outputs();
    m.ok(|l| l.focus_output(OWNER, SIDE));
    m.open(1);
    m.ok(|l| l.remove_output(SIDE));
    assert_eq!(m.layout.active_output(OWNER), Some(THIRD));
    assert_eq!(m.info(1).output, Some(THIRD));
    // The last one removed while active: the one before it is active.
    m.ok(|l| l.remove_output(THIRD));
    assert_eq!(m.layout.active_output(OWNER), Some(MAIN));
    assert_eq!(m.info(1).output, Some(MAIN));
}

/// The workspace an output shows, taken back by the output it is at home on, gives way to
/// another workspace of the output it leaves: the one before it.
#[test]
fn a_workspace_shown_and_taken_back_by_its_home_output_leaves_the_one_before_it_shown() {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.add_workspace(OWNER, "mine", Some(MAIN)));
    m.ok(|l| l.add_workspace(OWNER, "away", Some(SIDE)));
    m.ok(|l| l.focus_output(OWNER, MAIN));
    m.open(1);
    m.ok(|l| l.remove_output(SIDE));
    m.ok(|l| l.focus_workspace(OWNER, "away"));
    let (names, shown) = m.workspaces(MAIN);
    assert_eq!(names[shown], "away");
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    let (names, shown) = m.workspaces(MAIN);
    assert_eq!(names, ["-", "mine"]);
    assert_eq!(shown, 1, "the workspace before it is shown");
    assert_eq!(m.workspaces(SIDE).0, ["away"]);
}

/// The same when it is the first workspace of the output it leaves: the one after it is shown.
#[test]
fn a_first_workspace_shown_and_taken_back_leaves_the_one_after_it_shown() {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.add_workspace(OWNER, "away", Some(SIDE)));
    m.ok(|l| l.remove_output(SIDE));
    // MAIN's own empty workspace goes once another is shown, leaving "away" first.
    m.ok(|l| l.focus_workspace(OWNER, "away"));
    let (names, shown) = m.workspaces(MAIN);
    assert_eq!((names[0].as_str(), shown), ("away", 0), "{names:?}");
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    assert_eq!(m.workspaces(SIDE).0, ["away"]);
    let (names, shown) = m.workspaces(MAIN);
    assert_eq!((names.len(), shown), (1, 0), "{names:?}");
}

/// A workspace that was on a removed output, now on another, and that a window is moved into there
/// takes that other output as its home: it does not go back when the first one returns.
#[test]
fn a_window_moved_into_a_workspace_that_waits_on_another_output_makes_it_stay() {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.focus_output(OWNER, SIDE));
    m.open(1);
    m.ok(|l| l.focus_output(OWNER, MAIN));
    m.ok(|l| l.remove_output(SIDE));
    assert_eq!(m.info(1).output, Some(MAIN));
    // Window 1's workspace is now the second of MAIN's; a window comes to it from the first.
    m.open(2);
    m.ok(|l| l.move_to_workspace_relative(OWNER, Direction::Down));
    assert_eq!(m.info(2).output, Some(MAIN));
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    assert_eq!(m.info(1).output, Some(MAIN));
    assert_eq!(m.info(2).output, Some(MAIN));
}

/// A window moved to a special workspace that does not exist yet while another special workspace
/// is shown on another output goes to a new hidden one, not to the one shown.
#[test]
fn moving_to_a_special_workspace_finds_the_one_of_that_name() {
    let mut m = Model::new();
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.focus_output(OWNER, SIDE));
    m.ok(|l| l.toggle_special(OWNER, "a"));
    assert_eq!(m.layout.shown_special(SIDE).unwrap().as_deref(), Some("a"));
    m.ok(|l| l.focus_output(OWNER, MAIN));
    m.open(1);
    m.ok(|l| l.move_to_special(OWNER, "b"));
    let info = m.info(1);
    assert_eq!((info.special, info.workspace.as_deref()), (true, Some("b")));
    assert_eq!(info.output, None, "it is hidden");

    m.open(2);
    m.ok(|l| l.move_to_special(OWNER, "a"));
    let info = m.info(2);
    assert_eq!((info.special, info.workspace.as_deref()), (true, Some("a")));
    assert_eq!(info.output, Some(SIDE), "it is shown on the other output");
}

/// A window on a special workspace left hidden when the seat's last output goes cannot be given the
/// focus: there is no output to show it on.
#[test]
fn a_window_on_a_hidden_special_workspace_cannot_be_focused_without_an_output() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.move_to_special(OWNER, "scratch"));
    assert_eq!(m.info(1).output, None);
    m.ok(|l| l.remove_output(MAIN));
    m.refused(LayoutError::WorkspaceWithoutOutput, |l| {
        l.focus_window(w(1))
    });
}

/// The outputs a mode change moves are those whose left edge lies right of the changed output's:
/// `SIDE` (at 1920) changing mode moves `THIRD` (at 3840) and leaves `MAIN` (at 0) and an output
/// stacked below it, at the same left edge, where they are. This is Yuna's rule, documented on
/// `Layout::configure_output`.
#[test]
fn a_mode_change_moves_only_the_outputs_right_of_the_changed_one() {
    let mut m = three_outputs();
    let below = OutputId(4);
    m.ok(|l| {
        l.add_output(
            OWNER,
            below,
            OutputConfig {
                position: Point {
                    x: 1920.0,
                    y: 1080.0,
                },
                physical_width: 1920,
                physical_height: 1080,
                scale: 1.0,
            },
        )
    });
    m.ok(|l| l.configure_output(SIDE, 2560, 1440, 1.0));
    assert_eq!(m.layout.output_rect(SIDE).unwrap().right(), 4480.0);
    assert_eq!(m.layout.output_rect(THIRD).unwrap().x, 4480.0);
    assert_eq!(m.layout.output_rect(MAIN).unwrap().x, 0.0);
    assert_eq!(m.layout.output_rect(below).unwrap().x, 1920.0);
}
