//! Workspaces: named, with an output of their own, and dynamic ones as an option (ADR-0018).

use crate::harness::*;
use yuna_layout::{Direction, OpenOptions, Options, Tiling};

/// The owner's desk, as Yuna starts: the configuration declares the workspaces, then the
/// monitors come. The 4K one at scale 1.5 on the left holds workspaces 1, 3 and 5; the 1080p one
/// on its right, 2 and 4.
fn desk() -> Model {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    for (name, out) in [
        ("1", MAIN),
        ("3", MAIN),
        ("5", MAIN),
        ("2", SIDE),
        ("4", SIDE),
    ] {
        m.ok(|l| l.add_workspace(OWNER, name, Some(out)));
    }
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 3840, 2160, 1.5)));
    m.ok(|l| l.add_output(OWNER, SIDE, output(2560.0, 1920, 1080, 1.0)));
    m
}

/// niri: src/layout/tests.rs:2730, src/layout/tests.rs:2746.
#[test]
fn a_workspace_declared_for_an_output_not_yet_plugged_waits_on_the_active_one() {
    let mut m = Model::new();
    m.ok(|l| l.add_workspace(OWNER, "later", Some(SIDE)));
    assert_eq!(
        m.workspaces(MAIN).0,
        vec!["-".to_owned(), "later".to_owned()]
    );
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    assert_eq!(m.workspaces(SIDE), (vec!["later".into()], 0));
    assert_eq!(m.workspaces(MAIN), (vec!["-".into()], 0));
}

#[test]
fn named_workspaces_live_on_their_output_and_switching_follows_them() {
    let mut m = desk();
    m.ok(|l| l.focus_workspace(OWNER, "1"));
    assert_eq!(
        m.workspaces(MAIN),
        (vec!["1".into(), "3".into(), "5".into()], 0)
    );
    assert_eq!(m.workspaces(SIDE), (vec!["2".into(), "4".into()], 0));
    m.ok(|l| l.focus_workspace(OWNER, "4"));
    assert_eq!(m.layout.active_output(OWNER), Some(SIDE));
    assert_eq!(m.workspaces(SIDE).1, 1);
    m.open(1);
    assert_eq!(m.info(1).output, Some(SIDE));
    assert_eq!(m.info(1).workspace.as_deref(), Some("4"));
    // The 4K output still shows workspace 1.
    assert_eq!(m.workspaces(MAIN).1, 0);
}

/// Hyprland: workspaces are created on demand; Yuna puts them on the seat's active output.
#[test]
fn switching_to_an_unknown_name_creates_the_workspace_on_the_active_output() {
    let mut m = desk();
    m.ok(|l| l.focus_output(OWNER, SIDE));
    m.ok(|l| l.focus_workspace(OWNER, "music"));
    assert_eq!(
        m.workspaces(SIDE),
        (vec!["2".into(), "4".into(), "music".into()], 2)
    );
}

/// niri: docs/wiki/Configuration:-Named-Workspaces.md:15-38.
#[test]
fn named_workspaces_stay_when_empty_and_unnamed_ones_go() {
    let mut m = Model::new();
    m.ok(|l| l.add_workspace(OWNER, "web", None));
    assert_eq!(m.workspaces(MAIN), (vec!["-".into(), "web".into()], 0));
    // The unnamed workspace, empty, goes as soon as it is not shown.
    m.ok(|l| l.focus_workspace(OWNER, "web"));
    assert_eq!(m.workspaces(MAIN), (vec!["web".into()], 0));
    m.ok(|l| l.add_workspace(OWNER, "mail", None));
    m.ok(|l| l.focus_workspace(OWNER, "mail"));
    assert_eq!(m.workspaces(MAIN), (vec!["web".into(), "mail".into()], 1));
}

/// niri: docs/wiki/Workspaces.md:3-11, src/layout/tests.rs:1977, src/layout/tests.rs:2184.
#[test]
fn dynamic_workspaces_keep_one_empty_workspace_last() {
    let mut m = Model::with(Options {
        dynamic_workspaces: true,
        ..Options::default()
    });
    assert_eq!(m.workspaces(MAIN), (vec!["-".into()], 0));
    m.open(1);
    assert_eq!(m.workspaces(MAIN), (vec!["-".into(), "-".into()], 0));
    m.ok(|l| l.focus_workspace_relative(OWNER, Direction::Down));
    m.open(2);
    assert_eq!(m.workspaces(MAIN).0.len(), 3);
    // Workspace 0 empties while not shown: it goes, and the others move up.
    m.ok(|l| l.close_window(w(1)));
    assert_eq!(m.workspaces(MAIN), (vec!["-".into(), "-".into()], 0));
    assert_eq!(m.info(2).output, Some(MAIN));
}

/// niri: docs/wiki/Workspaces.md:7.
#[test]
fn an_empty_workspace_in_the_middle_goes_when_left() {
    let mut m = Model::with(Options {
        dynamic_workspaces: true,
        ..Options::default()
    });
    m.open(1);
    m.ok(|l| l.focus_workspace_relative(OWNER, Direction::Down));
    m.open(2);
    m.ok(|l| l.focus_workspace_relative(OWNER, Direction::Up));
    m.ok(|l| l.close_window(w(1)));
    // Shown, it stays while empty.
    assert_eq!(m.workspaces(MAIN).0.len(), 3);
    m.ok(|l| l.focus_workspace_relative(OWNER, Direction::Down));
    assert_eq!(m.workspaces(MAIN), (vec!["-".into(), "-".into()], 0));
}

#[test]
fn workspaces_are_stacked_and_relative_switches_stop_at_the_ends() {
    let mut m = desk();
    m.ok(|l| l.focus_workspace(OWNER, "3"));
    m.ok(|l| l.focus_workspace_relative(OWNER, Direction::Up));
    assert_eq!(m.workspaces(MAIN).1, 0);
    m.ok(|l| l.focus_workspace_relative(OWNER, Direction::Up));
    assert_eq!(m.workspaces(MAIN).1, 0);
    // Left and right mean nothing for workspaces.
    m.ok(|l| l.focus_workspace_relative(OWNER, Direction::Right));
    assert_eq!(m.workspaces(MAIN).1, 0);
}

/// niri: src/layout/monitor.rs:805-816 (a moved window keeps its column's width and full width).
#[test]
fn a_window_moved_to_another_workspace_keeps_its_column_width_and_full_width() {
    let mut m = desk();
    m.ok(|l| l.focus_workspace(OWNER, "1"));
    m.open(1);
    m.ok(|l| l.cycle_preset_width(w(1), true));
    let width = m.rect(1).w;
    m.ok(|l| l.move_to_workspace(OWNER, "3"));
    assert_eq!(m.rect(1).w, width);
    m.ok(|l| l.set_full_width(w(1), true));
    m.ok(|l| l.move_to_workspace(OWNER, "5"));
    assert!(m.info(1).full_width);
}

/// niri: `move-window-to-workspace` follows the window by default.
#[test]
fn a_window_moved_to_a_workspace_takes_the_seat_with_it_and_keeps_floating() {
    let mut m = desk();
    m.ok(|l| l.focus_workspace(OWNER, "1"));
    m.open(1);
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.set_floating_rect(w(2), rect(100.0, 100.0, 400.0, 300.0)));
    m.ok(|l| l.move_to_workspace(OWNER, "2"));
    assert_eq!(m.info(2).workspace.as_deref(), Some("2"));
    assert!(m.info(2).floating);
    assert_rect(m.rect(2), rect(2560.0 + 100.0, 100.0, 400.0, 300.0));
    assert_eq!(m.layout.active_output(OWNER), Some(SIDE));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.move_to_workspace_relative(OWNER, Direction::Down));
    assert_eq!(m.info(2).workspace.as_deref(), Some("4"));
}

#[test]
fn a_removed_workspace_loses_its_name_keeps_its_windows_and_goes_once_empty_and_left() {
    let mut m = desk();
    m.ok(|l| l.focus_workspace(OWNER, "3"));
    m.open(1);
    // niri: docs/wiki/Configuration:-Named-Workspaces.md:37.
    m.ok(|l| l.remove_workspace(OWNER, "3"));
    assert_eq!(m.info(1).workspace, None);
    m.ok(|l| l.close_window(w(1)));
    assert_eq!(
        m.workspaces(MAIN),
        (vec!["1".into(), "-".into(), "5".into()], 1)
    );
    m.ok(|l| l.focus_workspace(OWNER, "5"));
    assert_eq!(m.workspaces(MAIN), (vec!["1".into(), "5".into()], 1));
}

#[test]
fn each_workspace_has_its_own_tiling() {
    let mut m = desk();
    m.ok(|l| l.focus_workspace(OWNER, "1"));
    m.ok(|l| l.set_tiling(OWNER, Tiling::Dwindle));
    let list = m.layout.workspaces(MAIN).unwrap();
    assert_eq!(list[0].tiling, Tiling::Dwindle);
    assert_eq!(list[1].tiling, Tiling::Scrolling);
}
