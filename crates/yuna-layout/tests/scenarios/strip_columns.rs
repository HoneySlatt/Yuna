//! Moving through and reshaping the columns of a scrolling strip: focus and moves inside a column,
//! fullscreen taking a window out of a shared column, widths and heights. The output is 1920 × 1080
//! at scale 1 with gaps of 16: a column of half the width is 936 wide.

use crate::harness::*;
use yuna_layout::{ColumnWidth, OpenOptions, Options};
use yuna_types::{Direction, Size, Tiling};

/// Windows 1, 2 and 3 in one column, from top to bottom, the focus on 3.
fn stacked() -> Model {
    let mut m = Model::new();
    for n in 1..=3 {
        m.open(n);
        if n > 1 {
            m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
        }
    }
    assert_eq!(order(&m), [1, 2, 3]);
    assert_eq!(m.focused(), Some(3));
    m
}

/// The windows of the shown workspace in reading order.
fn order(m: &Model) -> Vec<u64> {
    let (_, shown) = m.workspaces(MAIN);
    m.layout.workspaces(MAIN).unwrap()[shown]
        .windows
        .iter()
        .map(|w| w.0)
        .collect()
}

fn size(w: f64, h: f64) -> Size {
    Size { w, h }
}

/// Focus moves down and up a column one window at a time and stops at its ends. Yuna's rule, as
/// `Scrolling::focus` documents it (ADR-0110 makes niri the reference for scrolling columns; no niri
/// test was read for this scenario).
#[test]
fn focus_moves_down_and_up_a_column_and_stops_at_its_ends() {
    let mut m = stacked();
    m.ok(|l| l.focus(OWNER, Direction::Up));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.focus(OWNER, Direction::Up));
    assert_eq!(m.focused(), Some(1));
    m.ok(|l| l.focus(OWNER, Direction::Up));
    assert_eq!(m.focused(), Some(1));
    m.ok(|l| l.focus(OWNER, Direction::Down));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.focus(OWNER, Direction::Down));
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.focus(OWNER, Direction::Down));
    assert_eq!(m.focused(), Some(3));
}

/// The focused window moves up and down its column, and the focus follows it; at the top or the
/// bottom nothing moves. Yuna's rule, as `Scrolling::move_focused` documents it (no niri test was
/// read for this scenario).
#[test]
fn the_focused_window_moves_up_and_down_its_column_and_keeps_the_focus() {
    let mut m = stacked();
    m.ok(|l| l.move_focused(OWNER, Direction::Up));
    assert_eq!(order(&m), [1, 3, 2]);
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.move_focused(OWNER, Direction::Up));
    assert_eq!(order(&m), [3, 1, 2]);
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.move_focused(OWNER, Direction::Up));
    assert_eq!(order(&m), [3, 1, 2], "it is at the top");
    m.ok(|l| l.move_focused(OWNER, Direction::Down));
    assert_eq!(order(&m), [1, 3, 2]);
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.move_focused(OWNER, Direction::Down));
    assert_eq!(order(&m), [1, 2, 3]);
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.move_focused(OWNER, Direction::Down));
    assert_eq!(order(&m), [1, 2, 3], "it is at the bottom");
    assert_eq!(m.focused(), Some(3));
}

/// The focused column moves along the strip and keeps the focus; at the first or the last place it
/// stays. Yuna's rule, as `Scrolling::move_focused` documents it (no niri test was read for this
/// scenario).
#[test]
fn the_focused_column_moves_along_the_strip_and_keeps_the_focus() {
    let mut m = Model::new();
    for n in 1..=3 {
        m.open(n);
    }
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.ok(|l| l.move_focused(OWNER, Direction::Right));
    assert_eq!(order(&m), [1, 3, 2]);
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.move_focused(OWNER, Direction::Right));
    assert_eq!(order(&m), [1, 3, 2], "it is the last column");
    m.ok(|l| l.move_focused(OWNER, Direction::Left));
    m.ok(|l| l.move_focused(OWNER, Direction::Left));
    assert_eq!(order(&m), [2, 1, 3]);
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.move_focused(OWNER, Direction::Left));
    assert_eq!(order(&m), [2, 1, 3], "it is the first column");
    m.ok(|l| l.focus(OWNER, Direction::Right));
    assert_eq!(m.focused(), Some(1));
}

/// niri: src/layout/scrolling.rs:2841. A window leaving a shared column for fullscreen takes a
/// column of its own right of it: the focus stays on the window that had it, wherever that is.
#[test]
fn fullscreen_taking_a_window_out_of_a_column_keeps_the_focus_where_it_was() {
    // The column of 1 and 2, then the column of 3, which has the focus: 3 stays focused.
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.open(3);
    m.ok(|l| l.set_fullscreen(w(2), true));
    assert_eq!(order(&m), [1, 2, 3]);
    assert_eq!(m.focused(), Some(3));

    // The focus on a window of the same column, not the one taken out: it stays there.
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.ok(|l| l.focus(OWNER, Direction::Up));
    m.ok(|l| l.set_fullscreen(w(2), true));
    assert_eq!(order(&m), [1, 2]);
    assert_eq!(m.focused(), Some(1));

    // The focus on a column left of the one whose window goes fullscreen, which holds the
    // window last focused in it.
    let mut m = Model::new();
    m.open(3);
    m.open(2);
    m.open(1);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.ok(|l| l.focus(OWNER, Direction::Left));
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.set_fullscreen(w(1), true));
    assert_eq!(order(&m), [3, 2, 1]);
    assert_eq!(m.focused(), Some(3));
}

/// A focused window taken out of the middle of its column for fullscreen keeps the seat's focus, and
/// the column it left has its focus on the window that was below it.
#[test]
fn fullscreen_taking_a_window_out_of_the_middle_keeps_the_column_focus() {
    let mut m = stacked();
    m.ok(|l| l.focus(OWNER, Direction::Up));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.set_fullscreen(w(2), true));
    assert_eq!(order(&m), [1, 3, 2]);
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.focus(OWNER, Direction::Left));
    assert_eq!(
        m.focused(),
        Some(3),
        "the column keeps the window it had focused"
    );
}

/// A window taken out of the top of its column for fullscreen leaves the seat's focus on the window
/// that had it, which stays in the column.
#[test]
fn fullscreen_taking_the_top_window_out_keeps_the_column_focus_on_its_window() {
    let mut m = stacked();
    m.ok(|l| l.focus(OWNER, Direction::Up));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.set_fullscreen(w(1), true));
    assert_eq!(order(&m), [2, 3, 1]);
    assert_eq!(m.focused(), Some(2));
}

/// niri: src/layout/tests.rs:3389. Cycling goes by the list when the column's width is a preset:
/// the next one, wrapping at the end, whatever the widths are, and by the widths when it is not.
#[test]
fn cycling_presets_goes_by_the_list_from_a_preset_and_by_width_from_any_other() {
    let third = ColumnWidth::Proportion(1.0 / 3.0);
    let half = ColumnWidth::Proportion(0.5);
    let whole = ColumnWidth::Proportion(1.0);
    let mut m = Model::with(Options {
        preset_widths: vec![half, third, whole],
        ..Options::default()
    });
    m.open(1);
    let width = |m: &Model| m.rect(1).w;
    assert_eq!(width(&m), 936.0);
    // From a width that is no preset: the first one wider than it.
    m.ok(|l| l.cycle_preset_width(w(1), true));
    assert_eq!(width(&m), 1888.0);
    // From a preset: the next in the list, wrapping.
    m.ok(|l| l.cycle_preset_width(w(1), true));
    assert_eq!(width(&m), 936.0);
    m.ok(|l| l.cycle_preset_width(w(1), true));
    assert_eq!(width(&m), 619.0);
    m.ok(|l| l.cycle_preset_width(w(1), false));
    assert_eq!(width(&m), 936.0);
    m.ok(|l| l.cycle_preset_width(w(1), false));
    assert_eq!(width(&m), 1888.0);
    m.ok(|l| l.cycle_preset_width(w(1), false));
    assert_eq!(width(&m), 619.0);
}

/// Leaving full width, a column cycles by the widths again, from the width it spans, not from the
/// preset it had before.
#[test]
fn cycling_from_full_width_goes_by_width_not_by_the_preset_the_column_had() {
    let third = ColumnWidth::Proportion(1.0 / 3.0);
    let half = ColumnWidth::Proportion(0.5);
    let whole = ColumnWidth::Proportion(1.0);
    let mut m = Model::with(Options {
        preset_widths: vec![half, third, whole],
        ..Options::default()
    });
    m.open(1);
    m.ok(|l| l.cycle_preset_width(w(1), true));
    m.ok(|l| l.cycle_preset_width(w(1), true));
    assert_eq!(m.rect(1).w, 936.0, "the half, the first preset");
    // Forward from full width: nothing is wider than 1888, so the list starts again.
    m.ok(|l| l.set_full_width(w(1), true));
    assert_eq!(m.rect(1).w, 1888.0);
    m.ok(|l| l.cycle_preset_width(w(1), true));
    assert_eq!(m.rect(1).w, 936.0);
    // Backward from full width: the last preset narrower than 1888, a third.
    m.ok(|l| l.set_full_width(w(1), true));
    m.ok(|l| l.cycle_preset_width(w(1), false));
    assert_eq!(m.rect(1).w, 619.0);
}

/// Backward from a width that is no preset goes to the last preset narrower than it, a third from a
/// half; forward from it, to the first one wider, two thirds. niri's rule for presets, as
/// `Scrolling::cycle_preset_width` documents it (niri: `src/layout/scrolling.rs:4796-4842`, as the
/// scenario `preset_widths_cycle_and_a_manual_change_restarts_the_cycle` in `scrolling.rs` cites it).
#[test]
fn cycling_backward_from_no_preset_goes_to_the_last_narrower_one() {
    let mut m = Model::new();
    m.open(1);
    assert_eq!(m.rect(1).w, 936.0);
    m.ok(|l| l.cycle_preset_width(w(1), false));
    assert_eq!(m.rect(1).w, 619.0);
    // Forward from no preset: the first one wider, two thirds.
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.cycle_preset_width(w(1), true));
    assert_eq!(m.rect(1).w, 1253.0);
}

/// Widening a column of a fixed width converts it to the share it is of the working area, gaps
/// counted as a proportion counts them, then adds the change. Yuna's rule, as
/// `ColumnWidth::as_proportion` documents it (no niri test was read for this scenario).
#[test]
fn widening_a_fixed_width_column_adds_to_the_share_it_already_is() {
    let mut m = Model::new();
    m.open_with(
        OWNER,
        1,
        OpenOptions {
            width: Some(ColumnWidth::Fixed(700.0)),
            ..OpenOptions::default()
        },
    );
    assert_eq!(m.rect(1).w, 700.0);
    // (700 + 16) / 1904 of the working area, and a tenth more: 890.4, on the pixel grid.
    m.ok(|l| l.change_width(w(1), 0.1));
    assert_eq!(m.rect(1).w, 890.0);
    m.ok(|l| l.change_width(w(1), -0.1));
    assert_eq!(m.rect(1).w, 700.0);
}

/// niri: src/layout/scrolling.rs:4757-4865. When a window's minimum height is above its share of
/// the column, it gets its minimum and the others share what is left, again and again.
#[test]
fn windows_whose_minimum_is_above_their_share_get_it_and_the_others_share_the_rest() {
    let mut m = Model::new();
    for (n, min) in [(1, 600.0), (2, 300.0), (3, 0.0)] {
        m.open_with(
            OWNER,
            n,
            OpenOptions {
                min_size: size(0.0, min),
                ..OpenOptions::default()
            },
        );
        if n > 1 {
            m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
        }
    }
    // 1080 high, four gaps of 16: 1016 to share. A third is 338.67, below the 600 of window 1;
    // then 416 shared by two is 208, below the 300 of window 2; window 3 keeps what is left.
    let heights: Vec<f64> = (1..=3).map(|n| m.placement(n).configure.h).collect();
    assert_eq!(heights, [600.0, 300.0, 116.0]);
}

/// Two windows with no minimum share the column equally: 1080 high less three gaps of 16.
#[test]
fn windows_with_no_minimum_share_the_column_equally() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    // 1080 − 3 gaps = 1032, 516 each.
    assert_eq!(m.placement(1).configure.h, 516.0);
    assert_eq!(m.placement(2).configure.h, 516.0);
}

/// A column that is full width says so and no other does; a workspace that becomes dwindle keeps
/// the focused window maximised, as a full-width column was.
#[test]
fn full_width_is_reported_for_its_column_and_kept_for_the_focused_window_in_dwindle() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    assert!(!m.info(1).full_width && !m.info(2).full_width);
    m.ok(|l| l.set_full_width(w(2), true));
    assert!(m.info(2).full_width);
    assert!(!m.info(1).full_width);
    m.ok(|l| l.set_tiling(OWNER, Tiling::Dwindle));
    assert!(m.info(2).full_width, "it is maximised now");
    assert!(!m.info(1).full_width);
}

/// A trial of the shares as they will be rounded: a window whose minimum fits its share when the
/// shares before it are taken keeps it from the others, and one whose minimum does not takes it.
#[test]
fn a_minimum_height_is_judged_against_the_share_left_after_the_windows_above() {
    let heights_with_last_minimum = |min: f64| {
        let mut m = Model::new();
        for n in 1..=3 {
            let min_size = if n == 3 {
                size(0.0, min)
            } else {
                Size::default()
            };
            m.open_with(
                OWNER,
                n,
                OpenOptions {
                    min_size,
                    ..OpenOptions::default()
                },
            );
            if n > 1 {
                m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
            }
        }
        (1..=3)
            .map(|n| m.placement(n).configure.h)
            .collect::<Vec<f64>>()
    };
    // 1016 to share among three: 339, 339 and 338 as they round.
    assert_eq!(heights_with_last_minimum(330.0), [339.0, 339.0, 338.0]);
    // A minimum of 400 is above any third: it takes 400 and the others share 616.
    assert_eq!(heights_with_last_minimum(400.0), [308.0, 308.0, 400.0]);
}

/// Backward from a column narrower than every preset goes round to the widest one; forward from
/// one wider than every preset, to the narrowest.
#[test]
fn cycling_from_beyond_the_presets_goes_round_to_the_other_end() {
    let mut m = Model::new();
    m.open_with(
        OWNER,
        1,
        OpenOptions {
            width: Some(ColumnWidth::Fixed(300.0)),
            ..OpenOptions::default()
        },
    );
    assert_eq!(m.rect(1).w, 300.0);
    m.ok(|l| l.cycle_preset_width(w(1), false));
    assert_eq!(m.rect(1).w, 1888.0);
    let mut m = Model::new();
    m.open_with(
        OWNER,
        1,
        OpenOptions {
            width: Some(ColumnWidth::Fixed(1890.0)),
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.cycle_preset_width(w(1), true));
    assert_eq!(m.rect(1).w, 619.0);
}
