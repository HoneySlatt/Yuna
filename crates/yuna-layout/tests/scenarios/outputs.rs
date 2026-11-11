//! Outputs coming and going: workspaces move and come back, windows are never lost (niri).

use crate::harness::*;
use yuna_layout::{OpenOptions, Options};

/// `MAIN` with workspaces "a" and "b", "b" shown with window 1; `SIDE` with "c".
fn two_outputs() -> Model {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1280, 800, 1.0)));
    m.ok(|l| l.add_workspace(OWNER, "a", Some(MAIN)));
    m.ok(|l| l.add_workspace(OWNER, "b", Some(MAIN)));
    m.ok(|l| l.add_workspace(OWNER, "c", Some(SIDE)));
    m.ok(|l| l.focus_workspace(OWNER, "c"));
    m.ok(|l| l.focus_workspace(OWNER, "b"));
    m.open(1);
    m
}

/// niri: src/layout/tests.rs:2681, src/layout/tests.rs:1991.
#[test]
fn a_removed_output_hands_its_workspaces_over_and_the_other_keeps_what_it_shows() {
    let mut m = two_outputs();
    m.ok(|l| l.remove_output(MAIN));
    assert_eq!(
        m.workspaces(SIDE),
        (vec!["c".into(), "a".into(), "b".into()], 0)
    );
    assert_eq!(m.info(1).output, Some(SIDE));
    assert_eq!(m.layout.active_output(OWNER), Some(SIDE));
}

/// niri: docs/wiki/Workspaces.md:25-26, src/layout/tests.rs:2681, src/layout/tests.rs:2062.
#[test]
fn a_returning_output_takes_back_its_workspaces_and_shows_the_one_it_showed() {
    let mut m = two_outputs();
    m.ok(|l| l.remove_output(MAIN));
    // A window opening on "b" while it is away does not change a named workspace's home.
    m.ok(|l| l.focus_workspace(OWNER, "b"));
    m.open(2);
    m.ok(|l| l.focus_workspace(OWNER, "c"));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    assert_eq!(m.workspaces(MAIN), (vec!["a".into(), "b".into()], 1));
    assert_eq!(m.workspaces(SIDE), (vec!["c".into()], 0));
    assert_eq!(m.info(2).output, Some(MAIN));
}

/// niri: src/layout/tests.rs:2041, docs/wiki/Development:-Design-Principles.md (invisible state).
#[test]
fn an_unnamed_workspace_used_elsewhere_stays_there() {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.focus_output(OWNER, MAIN));
    m.open(1);
    m.ok(|l| l.remove_output(MAIN));
    // Window 2 opens on the unnamed workspace that came from MAIN: it now belongs to SIDE.
    m.ok(|l| l.focus_window(w(1)));
    m.open(2);
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    assert_eq!(m.info(1).output, Some(SIDE));
}

/// niri: src/layout/tests.rs:2361, src/layout/mod.rs:2458.
#[test]
fn without_any_output_the_workspaces_wait_with_their_windows() {
    let mut m = two_outputs();
    m.ok(|l| l.remove_output(SIDE));
    m.ok(|l| l.remove_output(MAIN));
    assert_eq!(m.info(1).output, None);
    assert_eq!(m.focused(), None);
    // A window opening now waits with them.
    m.open(2);
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    let names = m.workspaces(MAIN).0;
    for name in ["a", "b", "c"] {
        assert!(names.iter().any(|n| n == name), "{name} came back");
    }
    assert_eq!(m.info(1).output, Some(MAIN));
    assert_eq!(m.info(2).output, Some(MAIN));
}

#[test]
fn a_pinned_window_stays_in_view_when_its_output_goes() {
    let mut m = two_outputs();
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.set_pinned(w(2), true));
    m.ok(|l| l.remove_output(MAIN));
    assert_eq!(m.info(2).output, Some(SIDE));
    assert!(m.info(2).pinned);
    assert!(m.shown(SIDE).contains(&2));
}
