//! What a change of state does to the other windows and to the focus: floating and tiling a
//! window, full width and fullscreen on a window the focus is not on, a dialog closing over a
//! fullscreen parent, and where a dialog opens on an output that does not start at the origin.
//! The output is 1920 × 1080 at scale 1 with gaps of 16.

use crate::harness::*;
use yuna_layout::geometry::{Point, Size};
use yuna_layout::{Direction, OpenOptions, OutputConfig};

fn floating() -> OpenOptions {
    OpenOptions {
        floating: true,
        ..OpenOptions::default()
    }
}

fn order(m: &Model) -> Vec<u64> {
    let (_, shown) = m.workspaces(MAIN);
    m.layout.workspaces(MAIN).unwrap()[shown]
        .windows
        .iter()
        .map(|w| w.0)
        .collect()
}

/// Floating a window that floats changes nothing: not its place, not its stacking.
#[test]
fn floating_a_floating_window_changes_nothing() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(OWNER, 2, floating());
    m.open_with(OWNER, 3, floating());
    m.ok(|l| l.set_floating_rect(w(2), rect(100.0, 50.0, 800.0, 600.0)));
    let before = m.placed(MAIN);
    m.ok(|l| l.set_floating(w(2), true));
    assert_eq!(m.placed(MAIN), before);
    assert_eq!(m.focused(), Some(3));
}

/// Tiling a window that is tiled changes nothing: it does not go to the end of the strip.
#[test]
fn tiling_a_tiled_window_changes_nothing() {
    let mut m = Model::new();
    for n in 1..=3 {
        m.open(n);
    }
    let before = m.placed(MAIN);
    m.ok(|l| l.set_floating(w(1), false));
    assert_eq!(order(&m), [1, 2, 3]);
    assert_eq!(m.placed(MAIN), before);
    assert_eq!(m.focused(), Some(3));
}

/// A floating window that has not the focus tiles without taking it: with a fullscreen window
/// focused, nothing takes its screen.
#[test]
fn tiling_a_floating_window_without_the_focus_leaves_the_focus_where_it_was() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.open_with(OWNER, 2, floating());
    assert_eq!(
        m.focused(),
        Some(1),
        "nothing floats over a fullscreen game's focus"
    );
    m.ok(|l| l.set_floating(w(2), false));
    assert_eq!(order(&m), [1, 2]);
    assert_eq!(m.focused(), Some(1));
}

/// A floating window that has the focus tiles and keeps it.
#[test]
fn tiling_the_focused_floating_window_keeps_the_focus_on_it() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(OWNER, 2, floating());
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.set_floating(w(2), false));
    assert_eq!(m.focused(), Some(2));
    assert!(m.placement(2).tiled);
}

/// Full width and fullscreen of a tiled window that is not the focused one leave the focus where
/// it is; on the focused tiled window, they bring the focus back to the tiled side.
#[test]
fn full_width_and_fullscreen_move_the_focus_to_the_tiled_side_only_for_its_window() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.open_with(OWNER, 3, floating());
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.set_full_width(w(1), true));
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.set_fullscreen(w(1), true));
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.set_fullscreen(w(1), false));
    assert_eq!(m.focused(), Some(3));

    // The window the tiled side has focused: the focus goes to the tiled side.
    m.ok(|l| l.set_full_width(w(2), true));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.switch_focus_floating(OWNER));
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.set_fullscreen(w(2), true));
    assert_eq!(m.focused(), Some(2));

    // Leaving fullscreen does not move it.
    m.ok(|l| l.switch_focus_floating(OWNER));
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert_eq!(m.focused(), Some(3));
}

/// A dialog closing while its parent, which does not have the focus, is fullscreen does not take
/// the focus to the parent: only the dialog that held the focus gives it back.
#[test]
fn a_dialog_closing_without_the_focus_leaves_the_focus_alone_over_a_fullscreen_parent() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.set_fullscreen(w(1), true));
    assert_eq!(m.focused(), Some(2));
    m.open_with(
        OWNER,
        3,
        OpenOptions {
            parent: Some(w(1)),
            ..OpenOptions::default()
        },
    );
    assert_eq!(
        m.focused(),
        Some(2),
        "a dialog takes the focus only from its parent"
    );
    m.ok(|l| l.close_window(w(3)));
    assert_eq!(m.focused(), Some(2));
}

/// A window opened fullscreen and tiled goes back to its place in the strip on leaving
/// fullscreen; only one opened floating floats again.
#[test]
fn a_window_opened_fullscreen_and_tiled_stays_tiled_on_leaving_fullscreen() {
    let mut m = Model::new();
    m.open_with(
        OWNER,
        1,
        OpenOptions {
            fullscreen: true,
            ..OpenOptions::default()
        },
    );
    assert!(m.info(1).fullscreen);
    m.ok(|l| l.set_fullscreen(w(1), false));
    assert!(!m.info(1).floating);
    assert!(m.placement(1).tiled);
    assert_rect(m.rect(1), rect(16.0, 16.0, 936.0, 1048.0));
}

/// A floating window follows its client's commits until the compositor resizes it: moving it to a
/// size it already has is not a resize, so it is still asked no size of its own.
#[test]
fn moving_a_floating_window_that_follows_its_commits_does_not_fix_its_size() {
    let mut m = Model::new();
    m.open_with(OWNER, 1, floating());
    m.ok(|l| l.commit_size(w(1), Size { w: 500.0, h: 400.0 }));
    m.ok(|l| l.set_floating_rect(w(1), rect(100.0, 50.0, 500.0, 400.0)));
    assert_eq!(m.placement(1).configure, Size::default());
    assert_rect(m.rect(1), rect(100.0, 50.0, 500.0, 400.0));
}

/// A dialog opens centred on its parent whatever the output's origin: the output is at (1920, 40).
#[test]
fn a_dialog_opens_centred_on_its_parent_on_an_output_that_is_not_at_the_origin() {
    let mut m = Model::bare(Default::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| {
        l.add_output(
            OWNER,
            MAIN,
            OutputConfig {
                position: Point { x: 1920.0, y: 40.0 },
                physical_width: 1920,
                physical_height: 1080,
                scale: 1.0,
            },
        )
    });
    m.open(1);
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            parent: Some(w(1)),
            size: Some(Size { w: 400.0, h: 300.0 }),
            ..OpenOptions::default()
        },
    );
    let (parent, dialog) = (m.rect(1), m.rect(2));
    assert_eq!((parent.x, parent.y), (1920.0 + 16.0, 40.0 + 16.0));
    let centre = |r: yuna_layout::geometry::Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
    assert_eq!(centre(dialog), centre(parent));
}

/// A dialog bigger than the room beside its parent is kept on the working area: its right edge on
/// the output's, and its bottom edge on the output's.
#[test]
fn a_dialog_is_kept_on_the_working_area_when_centring_it_would_leave_it() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    // Window 2, 968 to 1904 across: a dialog 1000 wide centred on it would end at 1936.
    m.open_with(
        OWNER,
        3,
        OpenOptions {
            parent: Some(w(2)),
            size: Some(Size {
                w: 1000.0,
                h: 300.0,
            }),
            ..OpenOptions::default()
        },
    );
    assert_rect(m.rect(3), rect(920.0, 390.0, 1000.0, 300.0));

    // A window in the lower half of a column: a dialog 600 high centred on it would end below
    // the output's bottom edge.
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    assert_rect(m.rect(2), rect(16.0, 548.0, 936.0, 516.0));
    m.open_with(
        OWNER,
        3,
        OpenOptions {
            parent: Some(w(2)),
            size: Some(Size { w: 400.0, h: 600.0 }),
            ..OpenOptions::default()
        },
    );
    assert_rect(m.rect(3), rect(284.0, 480.0, 400.0, 600.0));
}

/// niri: fake fullscreen tells the client it is fullscreen without changing its place; turning it
/// off on a window that is really fullscreen is no request to leave fullscreen.
#[test]
fn fake_fullscreen_leaves_a_really_fullscreen_window_only_when_turned_on() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.set_fullscreen(w(2), true));
    m.ok(|l| l.set_fake_fullscreen(w(2), false));
    assert!(m.info(2).fullscreen, "off leaves a real fullscreen alone");
    assert!(!m.info(2).fake_fullscreen);
    m.ok(|l| l.set_fake_fullscreen(w(2), true));
    assert!(!m.info(2).fullscreen, "on takes it out of real fullscreen");
    assert!(m.info(2).fake_fullscreen);
    assert_rect(m.rect(2), rect(968.0, 16.0, 936.0, 1048.0));
}
