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

/// niri: docs/wiki/Floating-Windows.md:16, src/layout/workspace.rs:1487 (code).
#[test]
fn a_floating_window_stays_inside_the_working_area() {
    let mut m = Model::new();
    m.open_with(OWNER, 1, floating(None));
    m.ok(|l| l.set_floating_rect(w(1), rect(1800.0, -50.0, 400.0, 300.0)));
    assert_rect(m.rect(1), rect(1520.0, 0.0, 400.0, 300.0));
    m.ok(|l| l.set_floating_rect(w(1), rect(0.0, 0.0, 5000.0, 5000.0)));
    assert_rect(m.rect(1), rect(0.0, 0.0, 1920.0, 1080.0));
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
