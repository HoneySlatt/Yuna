//! Floating windows (niri).

use crate::harness::*;
use yuna_layout::geometry::Size;
use yuna_layout::{Direction, OpenOptions};

fn floating(size: Option<Size>) -> OpenOptions {
    OpenOptions {
        floating: true,
        size,
        ..OpenOptions::default()
    }
}

/// niri: docs/wiki/Floating-Windows.md:5-7.
#[test]
fn floating_windows_open_centred_over_the_tiles_and_do_not_scroll() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(OWNER, 2, floating(Some(Size { w: 640.0, h: 480.0 })));
    assert_rect(m.rect(2), rect(640.0, 300.0, 640.0, 480.0));
    assert_eq!(
        m.shown(MAIN),
        vec![1, 2],
        "floating windows are drawn over the tiled ones"
    );
    assert!(!m.placement(2).tiled);
    // Tiled windows opening and scrolling move the strip, not the floating window.
    m.ok(|l| l.switch_focus_floating(OWNER));
    m.open(3);
    m.open(4);
    assert_rect(m.rect(2), rect(640.0, 300.0, 640.0, 480.0));
}

/// niri: docs/wiki/Floating-Windows.md:10-14.
#[test]
fn a_window_floats_and_tiles_back_and_floats_again_where_it_was() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.set_floating(w(2), true));
    // Centred, at its tiled size.
    assert_rect(m.rect(2), rect((1920.0 - 936.0) / 2.0, 16.0, 936.0, 1048.0));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.set_floating_rect(w(2), rect(10.0, 20.0, 300.0, 200.0)));
    m.ok(|l| l.set_floating(w(2), false));
    assert!(m.placement(2).tiled);
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.set_floating(w(2), true));
    assert_rect(m.rect(2), rect(10.0, 20.0, 300.0, 200.0));
}

/// niri: docs/wiki/Floating-Windows.md:10-14.
#[test]
fn focus_switches_between_floating_and_tiled_windows() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(OWNER, 2, floating(None));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.switch_focus_floating(OWNER));
    assert_eq!(m.focused(), Some(1));
    m.ok(|l| l.switch_focus_floating(OWNER));
    assert_eq!(m.focused(), Some(2));
    // The last floating window closing gives the focus back to the tiles.
    m.ok(|l| l.close_window(w(2)));
    assert_eq!(m.focused(), Some(1));
}

/// niri: src/layout/floating.rs:136-159 ("Numbers taken from Mutter").
#[test]
fn a_floating_window_may_hang_off_the_edge_but_stays_within_reach() {
    let mut m = Model::new();
    m.open_with(OWNER, 1, floating(None));
    // Partly off the right edge: allowed, as in niri.
    m.ok(|l| l.set_floating_rect(w(1), rect(1800.0, 100.0, 400.0, 300.0)));
    assert_rect(m.rect(1), rect(1800.0, 100.0, 400.0, 300.0));
    // Almost wholly off: a quarter of it, at most 75 units, stays on the working area.
    m.ok(|l| l.set_floating_rect(w(1), rect(5000.0, -5000.0, 400.0, 300.0)));
    assert_rect(
        m.rect(1),
        rect(1920.0 - 75.0, -(300.0 - 75.0), 400.0, 300.0),
    );
    // A window larger than the output keeps its size.
    m.ok(|l| l.set_floating_rect(w(1), rect(0.0, 0.0, 5000.0, 3000.0)));
    assert_eq!((m.rect(1).w, m.rect(1).h), (5000.0, 3000.0));
}

/// niri: docs/wiki/Floating-Windows.md:12-14.
#[test]
fn directional_focus_among_floating_windows_follows_their_places() {
    let mut m = Model::new();
    for (n, x) in [(1, 0.0), (2, 600.0), (3, 1200.0)] {
        m.open_with(OWNER, n, floating(None));
        m.ok(|l| l.set_floating_rect(w(n), rect(x, 100.0, 500.0, 400.0)));
    }
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.focus(OWNER, Direction::Left));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.focus(OWNER, Direction::Left));
    assert_eq!(m.focused(), Some(1));
    // Focusing raises: window 1 is now drawn last.
    assert_eq!(m.shown(MAIN).last(), Some(&1));
}

/// niri: src/layout/floating.rs:412-418 (a window leaving the tiles takes its own size).
#[test]
fn a_fullscreen_window_floated_takes_its_tiled_size_not_the_outputs() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.ok(|l| l.set_floating(w(1), true));
    assert!(!m.info(1).fullscreen);
    assert_eq!((m.rect(1).w, m.rect(1).h), (936.0, 1048.0));
}

/// niri: src/utils/mod.rs:525-535 (a window larger than the area shows from its top-left).
#[test]
fn a_new_floating_window_larger_than_the_output_opens_from_its_top_left_corner() {
    let mut m = Model::new();
    m.open_with(
        OWNER,
        1,
        floating(Some(Size {
            w: 3000.0,
            h: 2000.0,
        })),
    );
    assert_rect(m.rect(1), rect(0.0, 0.0, 3000.0, 2000.0));
}

/// niri: src/layout/workspace.rs:611; the size asked when it opened is kept.
#[test]
fn a_window_opened_floating_and_fullscreen_floats_back_at_the_size_it_asked() {
    let mut m = Model::new();
    m.open_with(
        OWNER,
        1,
        OpenOptions {
            floating: true,
            fullscreen: true,
            size: Some(Size { w: 640.0, h: 480.0 }),
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.set_fullscreen(w(1), false));
    assert_rect(m.rect(1), rect(640.0, 300.0, 640.0, 480.0));
}
