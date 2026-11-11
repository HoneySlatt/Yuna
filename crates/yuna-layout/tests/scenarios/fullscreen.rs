//! Fullscreen, fake fullscreen, and how they meet columns and floating windows (niri).

use crate::harness::*;
use yuna_layout::{ColumnWidth, Direction, OpenOptions, Options};

const OUTPUT: yuna_layout::geometry::Rect =
    yuna_layout::geometry::Rect::new(0.0, 0.0, 1920.0, 1080.0);

/// niri: docs/wiki/Fullscreen-and-Maximize.md:32-47.
#[test]
fn a_fullscreen_window_covers_the_whole_output_and_is_told_so() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    let p = m.placement(1);
    assert_rect(p.rect, OUTPUT);
    assert!(p.fullscreen);
    assert!(m.info(1).fullscreen);
}

/// niri: src/layout/tests/fullscreen.rs:464 (the view goes -16, 116, -16).
#[test]
fn leaving_fullscreen_puts_the_view_back_where_it_was() {
    let mut m = Model::with(Options {
        default_width: ColumnWidth::Fixed(100.0),
        ..Options::default()
    });
    m.open(1);
    m.open(2);
    assert_eq!((m.rect(1).x, m.rect(2).x), (16.0, 132.0));
    m.ok(|l| l.set_fullscreen(w(2), true));
    // The view sits on window 2: window 1 is 100 + 16 to its left.
    assert_eq!((m.rect(1).x, m.rect(2).x), (-116.0, 0.0));
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert_eq!((m.rect(1).x, m.rect(2).x), (16.0, 132.0));
}

/// niri: src/layout/scrolling.rs:2841.
#[test]
fn a_window_sharing_its_column_leaves_it_to_go_fullscreen_and_keeps_focus() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.ok(|l| l.set_fullscreen(w(2), true));
    assert_eq!(m.focused(), Some(2));
    assert_rect(m.rect(2), OUTPUT);
    let list = &m.layout.workspaces(MAIN).unwrap()[0].windows;
    assert_eq!(list, &[w(1), w(2)]);
    // Window 1 has its column to itself again, out of view on the left.
    assert_eq!(m.rect(1).h, 1048.0);
    assert!(m.rect(1).right() <= 0.0);
}

/// niri: docs/wiki/Fullscreen-and-Maximize.md:49-57, docs/wiki/Development:-Design-Principles.md:65-73.
#[test]
fn a_fullscreen_column_scrolls_away_like_any_other_and_stays_fullscreen() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.open_with(
        OWNER,
        9,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.set_fullscreen(w(2), true));
    // Focused fullscreen: the floating window hides.
    assert_eq!(m.shown(MAIN), vec![1, 2]);
    m.ok(|l| l.focus(OWNER, Direction::Left));
    assert_eq!(m.focused(), Some(1));
    assert!(m.info(2).fullscreen);
    assert_rect(m.rect(1), rect(16.0, 16.0, 936.0, 1048.0));
    // Focus left the fullscreen column: the floating window shows again.
    assert_eq!(m.shown(MAIN), vec![1, 2, 9]);
}

/// niri: src/layout/workspace.rs:616 (code).
#[test]
fn a_new_window_does_not_take_the_focus_from_a_fullscreen_one() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.open(2);
    assert_eq!(m.focused(), Some(1));
    assert_rect(m.rect(1), OUTPUT);
}

/// niri: docs/wiki/Fullscreen-and-Maximize.md:67-77, src/layout/tests/fullscreen.rs:205.
#[test]
fn fake_fullscreen_tells_the_window_without_moving_it() {
    let mut m = Model::new();
    m.open(1);
    let before = m.rect(1);
    m.ok(|l| l.set_fake_fullscreen(w(1), true));
    assert!(m.placement(1).fullscreen);
    assert!(!m.info(1).fullscreen);
    assert_rect(m.rect(1), before);
    m.ok(|l| l.set_fake_fullscreen(w(1), false));
    assert!(!m.placement(1).fullscreen);
}

/// niri: src/layout/mod.rs:3497 and 3543 (code), src/layout/tests/fullscreen.rs:205 and 220.
#[test]
fn fullscreen_and_fake_fullscreen_exclude_each_other() {
    let mut m = Model::new();
    m.open(1);
    let before = m.rect(1);
    // Fake fullscreen on a fullscreen window: it leaves real fullscreen.
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.ok(|l| l.set_fake_fullscreen(w(1), true));
    assert!(!m.info(1).fullscreen && m.info(1).fake_fullscreen);
    assert_rect(m.rect(1), before);
    // Real fullscreen on a fake one: it stops being fake.
    m.ok(|l| l.set_fullscreen(w(1), true));
    assert!(m.info(1).fullscreen && !m.info(1).fake_fullscreen);
    // A client leaving fullscreen while fake: it is told it is not fullscreen any more.
    m.ok(|l| l.set_fake_fullscreen(w(1), true));
    m.ok(|l| l.set_fullscreen(w(1), false));
    assert!(!m.placement(1).fullscreen);
}

/// niri: src/layout/scrolling.rs:4305-4308 (code).
#[test]
fn a_window_joining_a_fullscreen_column_ends_its_fullscreen() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    assert!(!m.info(1).fullscreen);
    assert_eq!(m.rect(1).x, m.rect(2).x);
}

/// niri: src/layout/floating.rs:1354, docs/wiki/Fullscreen-and-Maximize.md:51-53,
/// src/layout/tests.rs:3525.
#[test]
fn a_floating_window_goes_fullscreen_through_the_tiles_and_floats_back_where_it_was() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.set_floating_rect(w(2), rect(100.0, 50.0, 640.0, 480.0)));
    m.ok(|l| l.set_fullscreen(w(2), true));
    assert!(!m.info(2).floating);
    assert_rect(m.rect(2), OUTPUT);
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert!(m.info(2).floating);
    assert_rect(m.rect(2), rect(100.0, 50.0, 640.0, 480.0));
    assert_eq!(m.focused(), Some(2));
}

/// Yuna's rule (ADR-0019): a window floats back only when it leaves fullscreen itself; fullscreen
/// ended another way leaves it tiled.
#[test]
fn a_floating_window_floats_back_only_when_it_leaves_fullscreen_itself() {
    let mut m = Model::with(Options {
        default_tiling: yuna_layout::Tiling::Dwindle,
        ..Options::default()
    });
    m.open(1);
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.set_fullscreen(w(2), true));
    m.ok(|l| l.focus_window(w(1)));
    assert!(!m.info(2).fullscreen && !m.info(2).floating);
    // Now tiled, a fullscreen round trip keeps it tiled (niri: workspace.rs:1303-1310).
    m.ok(|l| l.set_fullscreen(w(2), true));
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert!(!m.info(2).floating);
}

/// niri: src/layout/workspace.rs:611 (a window opened floating and fullscreen floats after).
#[test]
fn a_window_opened_floating_and_fullscreen_floats_on_leaving_fullscreen() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            floating: true,
            fullscreen: true,
            ..OpenOptions::default()
        },
    );
    assert!(m.info(2).fullscreen);
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert!(m.info(2).floating);
}

/// niri: src/layout/scrolling.rs:48-53 (the view kept relative to its column).
#[test]
fn leaving_fullscreen_after_a_column_on_the_left_closed_shows_the_columns_as_before() {
    let mut m = Model::new();
    for n in 1..=3 {
        m.open(n);
    }
    m.ok(|l| l.set_fullscreen(w(3), true));
    m.ok(|l| l.close_window(w(1)));
    m.ok(|l| l.set_fullscreen(w(3), false));
    assert_eq!((m.rect(2).x, m.rect(3).x), (16.0, 968.0));
}
