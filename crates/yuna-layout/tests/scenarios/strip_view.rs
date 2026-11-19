//! Where the view of a scrolling strip goes when windows open, close and take the focus: the
//! camera's exact position, read from [`yuna_layout::Layout::shown_workspaces`]. The output is
//! 1920 × 1080 at scale 1 with gaps of 16, so a column of half the width is 936 wide and the next
//! one starts 952 further along the strip.

use crate::harness::*;
use yuna_layout::Direction;

/// The strip position shown at the left edge of the working area.
fn view_x(m: &Model) -> f64 {
    m.layout
        .shown_workspaces(MAIN)
        .unwrap()
        .main
        .view_x
        .expect("scrolling columns have a camera")
}

/// Opens windows 1 to 4: the view scrolls to show 4, whose right edge sits a gap from the output's.
fn four_columns() -> Model {
    let mut m = Model::new();
    for n in 1..=4 {
        m.open(n);
    }
    assert_eq!(view_x(&m), 1888.0);
    m
}

/// niri: src/layout/scrolling.rs:769 (code). The view goes back to where it was relative to the
/// focused column, not just to a position that shows it.
#[test]
fn a_window_closed_right_after_it_opened_puts_the_view_back_exactly() {
    let mut m = four_columns();
    m.ok(|l| l.close_window(w(4)));
    assert_eq!(m.focused(), Some(3));
    assert_eq!(view_x(&m), 936.0);
}

/// Focusing the window that already has the focus is not focus moving on: the window that just
/// opened still gives the view back when it closes.
#[test]
fn focusing_the_focused_window_again_keeps_the_view_to_give_back() {
    let mut m = four_columns();
    m.ok(|l| l.focus_window(w(4)));
    m.ok(|l| l.close_window(w(4)));
    assert_eq!(m.focused(), Some(3));
    assert_eq!(view_x(&m), 936.0);
}

/// Focusing a window of another column is focus moving on, though the window's own column does not
/// change its focus: the window that opened no longer gives the view back.
#[test]
fn focusing_another_column_forgets_the_view_to_give_back() {
    let mut m = four_columns();
    m.ok(|l| l.focus_window(w(3)));
    assert_eq!(view_x(&m), 1888.0);
    m.ok(|l| l.close_window(w(4)));
    assert_eq!(m.focused(), Some(3));
    assert_eq!(view_x(&m), 1888.0);
}

/// Moving the focus with the keys is focus moving on too.
#[test]
fn moving_the_focus_forgets_the_view_to_give_back() {
    let mut m = four_columns();
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.ok(|l| l.close_window(w(4)));
    assert_eq!(m.focused(), Some(3));
    assert_eq!(view_x(&m), 1888.0);
}

/// Moving the focused column along the strip is focus moving on, too: the window that opened is
/// not at the index it was, so nothing is given back when it closes.
#[test]
fn moving_a_column_forgets_the_view_to_give_back() {
    let mut m = four_columns();
    m.ok(|l| l.move_focused(OWNER, Direction::Left));
    assert_eq!(
        m.layout.workspaces(MAIN).unwrap()[0].windows,
        [w(1), w(2), w(4), w(3)]
    );
    m.ok(|l| l.close_window(w(4)));
    assert_eq!(m.focused(), Some(3));
    assert_eq!(view_x(&m), 1888.0);
}

/// niri: Development:-Design-Principles.md:16-21. A column closing on the left of the focused one
/// takes its whole width and gap off the view, so that the focused column stays where it is on
/// screen; with the focus in the middle of the strip the view is not put right by fitting.
#[test]
fn a_column_closing_left_of_the_focused_one_takes_its_width_off_the_view() {
    let mut m = four_columns();
    m.ok(|l| l.focus(OWNER, Direction::Left));
    let focused = m.rect(3);
    assert_eq!(focused.x, 16.0);
    m.ok(|l| l.close_window(w(1)));
    assert_eq!(m.focused(), Some(3));
    assert_eq!(view_x(&m), 1888.0 - 952.0);
    assert_rect(m.rect(3), focused);
}

/// Closing the focused column in the middle of the strip focuses the one that takes its place, the
/// column on its right.
#[test]
fn closing_the_focused_middle_column_focuses_the_next_one() {
    let mut m = four_columns();
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.ok(|l| l.focus(OWNER, Direction::Left));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.close_window(w(2)));
    assert_eq!(m.focused(), Some(3));
}

/// Closing the focused last column, once focus moved on from the window that opened, focuses the
/// new last one.
#[test]
fn closing_the_focused_last_column_after_the_focus_moved_focuses_the_new_last() {
    let mut m = four_columns();
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.ok(|l| l.focus(OWNER, Direction::Right));
    m.ok(|l| l.close_window(w(4)));
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.focus(OWNER, Direction::Right));
    assert_eq!(m.focused(), Some(3), "it is the last column");
}

/// A workspace with no window has its view a gap before the strip's start, and so has one that
/// became empty.
#[test]
fn an_empty_strip_has_its_view_a_gap_before_its_start() {
    let mut m = Model::new();
    assert_eq!(view_x(&m), -16.0);
    m.open(1);
    m.ok(|l| l.close_window(w(1)));
    assert_eq!(view_x(&m), -16.0);
}

/// Windows stacked in a column: closing one of them keeps the focused window focused, whatever
/// its place before or after the closed one.
#[test]
fn closing_a_window_of_a_shared_column_keeps_the_focus_on_the_same_window() {
    let stacked = || {
        let mut m = Model::new();
        m.open(1);
        m.open(2);
        m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
        m.open(3);
        m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
        assert_eq!(m.focused(), Some(3));
        m
    };
    // The focused window is the last one: a window above it closes.
    let mut m = stacked();
    m.ok(|l| l.close_window(w(1)));
    assert_eq!(m.focused(), Some(3));
    // The focused window is the last one and closes: focus goes to the one above it.
    let mut m = stacked();
    m.ok(|l| l.close_window(w(3)));
    assert_eq!(m.focused(), Some(2));
    // A window below the focused one closes.
    let mut m = stacked();
    m.ok(|l| l.focus(OWNER, Direction::Up));
    m.ok(|l| l.focus(OWNER, Direction::Up));
    assert_eq!(m.focused(), Some(1));
    m.ok(|l| l.close_window(w(3)));
    assert_eq!(m.focused(), Some(1));
    // The focused one in the middle closes: the one below takes its place.
    let mut m = stacked();
    m.ok(|l| l.focus(OWNER, Direction::Up));
    m.ok(|l| l.close_window(w(2)));
    assert_eq!(m.focused(), Some(3));
}

/// Closing the focused fullscreen column when the column that takes its place is fullscreen too:
/// the view kept for the closed column's return from fullscreen is not the new one's. Columns of
/// 300 units all fit in view, so the view kept for column 3 is far from where 4 will want it.
#[test]
fn closing_a_focused_fullscreen_column_forgets_the_view_it_kept_for_leaving_fullscreen() {
    let mut m = Model::with(yuna_layout::Options {
        default_width: yuna_layout::ColumnWidth::Fixed(300.0),
        ..yuna_layout::Options::default()
    });
    for n in 1..=4 {
        m.open(n);
    }
    assert_eq!(view_x(&m), -16.0, "the four columns fit");
    m.ok(|l| l.focus_window(w(3)));
    m.ok(|l| l.set_fullscreen(w(3), true));
    m.ok(|l| l.set_fullscreen(w(4), true));
    m.ok(|l| l.close_window(w(3)));
    assert_eq!(m.focused(), Some(4));
    m.ok(|l| l.set_fullscreen(w(4), false));
    // Column 4 is the third, at 632 on the strip: the view brings it in a gap from the left edge.
    assert_eq!(view_x(&m), 616.0);
}

/// Another fullscreen column closing while the focused one is fullscreen keeps the view kept for
/// the focused one's return from fullscreen.
#[test]
fn closing_another_fullscreen_column_keeps_the_view_kept_for_the_focused_one() {
    let mut m = Model::with(yuna_layout::Options {
        default_width: yuna_layout::ColumnWidth::Fixed(300.0),
        ..yuna_layout::Options::default()
    });
    for n in 1..=4 {
        m.open(n);
    }
    m.ok(|l| l.focus_window(w(2)));
    m.ok(|l| l.set_fullscreen(w(2), true));
    m.ok(|l| l.set_fullscreen(w(4), true));
    m.ok(|l| l.close_window(w(4)));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert_eq!(
        view_x(&m),
        -16.0,
        "the view is where it was before column 2 went fullscreen"
    );
}
