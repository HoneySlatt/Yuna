//! Dwindle, Hyprland's layout, read in Hyprland's wiki at commit f20353c (2026-11-10) and its
//! source at commit 70ec30e (2026-11-11). The output is 1920 × 1080 at scale 1 with gaps of 16:
//! the space to share is 1888 × 1048 at (16, 16).

use crate::harness::*;
use yuna_layout::{Direction, Options, Tiling};

fn dwindle() -> Model {
    Model::with(Options {
        default_tiling: Tiling::Dwindle,
        ..Options::default()
    })
}

/// Windows 1, 2, 3: 1 on the left half, 2 above 3 on the right half.
fn three_windows() -> Model {
    let mut m = dwindle();
    for n in 1..=3 {
        m.open(n);
    }
    m
}

/// Hyprland: wiki configuring/layouts/dwindle-layout.md (split by aspect ratio,
/// `split_width_multiplier` 1, `preserve_split` false), src/layout/algorithm/tiled/dwindle/
/// DwindleAlgorithm.cpp.
#[test]
fn each_new_window_splits_the_focused_one_along_its_longer_side() {
    let mut m = dwindle();
    m.open(1);
    assert_rect(m.rect(1), rect(16.0, 16.0, 1888.0, 1048.0));
    m.open(2);
    assert_rect(m.rect(1), rect(16.0, 16.0, 936.0, 1048.0));
    assert_rect(m.rect(2), rect(968.0, 16.0, 936.0, 1048.0));
    // Window 2 is taller than wide: window 3 goes below it.
    m.open(3);
    assert_rect(m.rect(2), rect(968.0, 16.0, 936.0, 516.0));
    assert_rect(m.rect(3), rect(968.0, 548.0, 936.0, 516.0));
    assert_eq!(m.focused(), Some(3));
}

/// Hyprland: DwindleAlgorithm.cpp `removeTarget` (the sibling takes the parent's place) and
/// `getNextCandidate` (`input:focus_on_close` 0).
#[test]
fn a_closed_window_leaves_its_space_to_its_sibling_and_focus_to_the_nearest_window() {
    let mut m = three_windows();
    m.ok(|l| l.focus(OWNER, Direction::Up));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.close_window(w(2)));
    assert_rect(m.rect(3), rect(968.0, 16.0, 936.0, 1048.0));
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.close_window(w(3)));
    assert_rect(m.rect(1), rect(16.0, 16.0, 1888.0, 1048.0));
    assert_eq!(m.focused(), Some(1));
}

/// Hyprland: src/desktop/state/WindowQuery.cpp (directional focus is geometric). Yuna takes the
/// nearest window and, between equals, the topmost; Hyprland prefers the one used last
/// (ADR-0019).
#[test]
fn directional_focus_goes_to_the_nearest_window_on_that_side() {
    let mut m = three_windows();
    m.ok(|l| l.focus(OWNER, Direction::Left));
    assert_eq!(m.focused(), Some(1));
    m.ok(|l| l.focus(OWNER, Direction::Right));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.focus(OWNER, Direction::Down));
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.focus(OWNER, Direction::Down));
    assert_eq!(m.focused(), Some(3));
}

#[test]
fn moving_a_window_exchanges_it_with_its_neighbour() {
    let mut m = three_windows();
    let left = m.rect(1);
    m.ok(|l| l.move_focused(OWNER, Direction::Left));
    assert_rect(m.rect(3), left);
    assert_eq!(m.focused(), Some(3));
}

/// Hyprland: wiki configuring/core/dispatchers.md (fullscreen mode 1, maximised, keeps gaps).
#[test]
fn full_width_in_dwindle_maximises_one_window_over_the_others_until_focus_moves() {
    let mut m = three_windows();
    m.ok(|l| l.set_full_width(w(3), true));
    assert_rect(m.rect(3), rect(16.0, 16.0, 1888.0, 1048.0));
    assert_eq!(
        m.shown(MAIN).last(),
        Some(&3),
        "the maximised window is drawn over the others"
    );
    // Another window maximised takes over; focusing another window ends it.
    m.ok(|l| l.set_full_width(w(1), true));
    assert!(!m.info(3).full_width);
    m.ok(|l| l.focus(OWNER, Direction::Right));
    assert!(!m.info(1).full_width);
    assert_rect(m.rect(1), rect(16.0, 16.0, 936.0, 1048.0));
}

/// Hyprland: `misc:on_focus_under_fullscreen` 2 ends a maximised window when another takes
/// the focus.
#[test]
fn a_new_window_ends_a_maximised_one() {
    let mut m = three_windows();
    m.ok(|l| l.set_full_width(w(3), true));
    m.open(4);
    assert!(!m.info(3).full_width);
    assert_eq!(m.focused(), Some(4));
}

/// Yuna's choice against Hyprland's default (`misc:on_focus_under_fullscreen` 2, which ends the
/// fullscreen): a window opening behind a fullscreen one leaves it fullscreen and focused, so
/// that nothing takes a game's screen (ADR-0019).
#[test]
fn a_window_opening_behind_a_fullscreen_one_leaves_it_fullscreen_and_focused() {
    let mut m = dwindle();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.open(2);
    assert_eq!(m.focused(), Some(1));
    assert_eq!(m.shown(MAIN), vec![1]);
    m.ok(|l| l.set_fullscreen(w(1), false));
    assert_rect(m.rect(2), rect(968.0, 16.0, 936.0, 1048.0));
}

#[test]
fn switching_layouts_keeps_the_windows_order_focus_and_fullscreen() {
    let mut m = Model::new();
    for n in 1..=4 {
        m.open(n);
    }
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.ok(|l| l.set_fullscreen(w(3), true));
    m.ok(|l| l.set_tiling(OWNER, Tiling::Dwindle));
    let list = m.layout.workspaces(MAIN).unwrap()[0].clone();
    assert_eq!(list.tiling, Tiling::Dwindle);
    assert_eq!(list.windows, vec![w(1), w(2), w(3), w(4)]);
    assert_eq!(m.focused(), Some(3));
    assert!(m.info(3).fullscreen);
    m.ok(|l| l.set_tiling(OWNER, Tiling::Scrolling));
    let list = m.layout.workspaces(MAIN).unwrap()[0].clone();
    assert_eq!(list.windows, vec![w(1), w(2), w(3), w(4)]);
    assert_eq!(m.focused(), Some(3));
    assert!(m.info(3).fullscreen);
}

#[test]
fn preset_widths_change_nothing_in_dwindle() {
    let mut m = three_windows();
    let before = m.placed(MAIN);
    m.ok(|l| l.cycle_preset_width(w(3), true));
    m.ok(|l| l.change_width(w(3), 0.2));
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    assert_eq!(m.placed(MAIN), before);
}

/// Hyprland: one fullscreen state per window, maximised or fullscreen
/// (wiki configuring/core/dispatchers.md); one covering window per workspace, which has the focus.
#[test]
fn fullscreen_and_maximised_never_cover_at_once() {
    let mut m = dwindle();
    m.open(1);
    m.open(2);
    m.ok(|l| l.set_full_width(w(1), true));
    m.ok(|l| l.set_fullscreen(w(2), true));
    assert!(!m.info(1).full_width);
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert!(m.placement(2).activated);
    // Focusing window 2 ends window 1's fullscreen; window 2 may then be maximised.
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.ok(|l| l.focus_window(w(2)));
    assert!(!m.info(1).fullscreen);
    m.ok(|l| l.set_full_width(w(2), true));
    assert_eq!(m.shown(MAIN).last(), Some(&2));
}

/// Hyprland: DwindleAlgorithm.cpp `getClosestNode`, after `removeTarget` lays the tree out again.
#[test]
fn focus_after_a_close_goes_to_the_window_now_covering_the_closed_ones_centre() {
    let mut m = dwindle();
    for n in 0..4 {
        m.open(n);
    }
    m.ok(|l| l.focus_window(w(0)));
    let centre = m.rect(0).center();
    m.ok(|l| l.close_window(w(0)));
    let one = m.rect(1);
    assert!(
        one.x <= centre.x
            && centre.x <= one.right()
            && one.y <= centre.y
            && centre.y <= one.bottom()
    );
    assert_eq!(m.focused(), Some(1));
}

/// Hyprland: wiki configuring/core/dispatchers.md, "restore client maximized state".
#[test]
fn a_maximised_window_is_maximised_again_after_its_own_fullscreen() {
    let mut m = dwindle();
    m.open(1);
    m.open(2);
    m.ok(|l| l.set_full_width(w(2), true));
    m.ok(|l| l.set_fullscreen(w(2), true));
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert!(m.info(2).full_width);
    assert_rect(m.rect(2), rect(16.0, 16.0, 1888.0, 1048.0));
}

/// The owner's choice against Hyprland (ADR-0019): a background window may not take a fullscreen
/// game's screen, by maximise, fullscreen, or opening fullscreen.
#[test]
fn a_background_window_cannot_take_a_fullscreen_games_screen() {
    let mut m = dwindle();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.open(2);
    let held = yuna_layout::LayoutError::Refused("another window's fullscreen holds the screen");
    m.refused(held.clone(), |l| l.set_full_width(w(2), true));
    m.refused(held, |l| l.set_fullscreen(w(2), true));
    m.open_with(
        OWNER,
        3,
        yuna_layout::OpenOptions {
            fullscreen: true,
            ..yuna_layout::OpenOptions::default()
        },
    );
    assert!(!m.info(3).fullscreen);
    assert_eq!(m.focused(), Some(1));
    assert_eq!(m.shown(MAIN), vec![1]);
}
