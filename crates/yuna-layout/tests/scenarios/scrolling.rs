//! Scrolling columns, niri's layout. The output is 1920 × 1080 at scale 1 with gaps of 16, so a
//! column of half the width is (1920 − 16) × 0.5 − 16 = 936 wide and 1080 − 2 × 16 = 1048 tall.

use crate::harness::*;
use yuna_layout::{ColumnWidth, Direction, OpenOptions, Options};

const HALF: f64 = 936.0;
const TALL: f64 = 1048.0;

/// Opens windows 1, 2 and 3 in a row; the view scrolls to show 2 and 3.
fn three_columns() -> Model {
    let mut m = Model::new();
    for n in 1..=3 {
        m.open(n);
    }
    m
}

/// niri: src/layout/scrolling.rs:963-978 (code).
#[test]
fn a_new_window_opens_right_of_the_focused_one_and_takes_focus() {
    let mut m = Model::new();
    m.open(1);
    assert_rect(m.rect(1), rect(16.0, 16.0, HALF, TALL));
    m.open(2);
    assert_rect(m.rect(2), rect(16.0 + HALF + 16.0, 16.0, HALF, TALL));
    assert_eq!(m.focused(), Some(2));
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.open(3);
    let (_, shown) = m.workspaces(MAIN);
    let order = &m.layout.workspaces(MAIN).unwrap()[shown].windows;
    assert_eq!(order, &[w(1), w(3), w(2)]);
    assert_eq!(m.focused(), Some(3));
}

/// niri: docs/wiki/Development:-Design-Principles.md:6-14.
#[test]
fn opening_a_window_resizes_no_other() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.open(3);
    for n in 1..=3 {
        assert_eq!(m.rect(n).w, HALF);
        assert_eq!(m.rect(n).h, TALL);
    }
}

/// niri: docs/wiki/Configuration:-Layout.md:123, src/layout/scrolling.rs:5457.
#[test]
fn the_view_scrolls_the_least_needed_to_show_the_focused_column() {
    let m = three_columns();
    // The third column came in from the right: its right edge sits a gap from the output's.
    assert_rect(m.rect(3), rect(1920.0 - 16.0 - HALF, 16.0, HALF, TALL));
    assert_rect(m.rect(2), rect(16.0, 16.0, HALF, TALL));
    assert_rect(m.rect(1), rect(16.0 - 16.0 - HALF, 16.0, HALF, TALL));
}

/// niri: docs/wiki/Configuration:-Layout.md:123.
#[test]
fn focusing_a_column_already_in_view_does_not_move_the_view() {
    let mut m = three_columns();
    let before = m.placed(MAIN);
    m.ok(|l| l.focus(OWNER, Direction::Left));
    assert_eq!(m.focused(), Some(2));
    let after = m.placed(MAIN);
    for (b, a) in before.iter().zip(&after) {
        assert_eq!(b.rect, a.rect);
    }
    // Further left, column 1 comes in from the left, a gap from the edge.
    m.ok(|l| l.focus(OWNER, Direction::Left));
    assert_rect(m.rect(1), rect(16.0, 16.0, HALF, TALL));
}

/// niri: docs/wiki/Development:-Design-Principles.md:16-21.
#[test]
fn a_column_closing_left_of_the_focused_one_does_not_move_it() {
    let mut m = three_columns();
    let focused = m.rect(3);
    m.ok(|l| l.close_window(w(1)));
    assert_rect(m.rect(3), focused);
    // Nor does a column left of it changing width.
    m.ok(|l| l.cycle_preset_width(w(2), false));
    assert_rect(m.rect(3), focused);
}

/// niri: src/layout/scrolling.rs:1162 (code; no niri test asserts the target).
#[test]
fn closing_the_focused_column_focuses_the_one_that_takes_its_place() {
    let mut m = three_columns();
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.ok(|l| l.close_window(w(2)));
    assert_eq!(m.focused(), Some(3));
    // The last column closing gives focus to the new last one.
    m.ok(|l| l.close_window(w(3)));
    assert_eq!(m.focused(), Some(1));
}

/// niri: src/layout/scrolling.rs:769 (code).
#[test]
fn a_window_closed_right_after_it_opened_gives_back_focus_and_view() {
    let mut m = three_columns();
    m.ok(|l| l.focus(OWNER, Direction::Left));
    let before = m.placed(MAIN);
    m.open(4);
    assert_eq!(m.focused(), Some(4));
    m.ok(|l| l.close_window(w(4)));
    assert_eq!(m.focused(), Some(2));
    assert_eq!(m.placed(MAIN), before);
}

/// niri: src/layout/tests.rs:3317.
#[test]
fn closing_a_window_above_the_focused_one_keeps_the_focus() {
    let mut m = Model::new();
    m.open(0);
    m.open(1);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    // One column: 0, 1, 2 from top to bottom; focus on 2, then on 1.
    m.ok(|l| l.focus(OWNER, Direction::Up));
    assert_eq!(m.focused(), Some(1));
    m.ok(|l| l.close_window(w(0)));
    assert_eq!(m.focused(), Some(1));
}

/// niri: resources/default-config.kdl:542-546.
#[test]
fn consume_or_expel_moves_a_window_into_and_out_of_a_column() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    // Alone in its column, window 2 joins the bottom of the column on its left.
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    let half_height = (TALL - 16.0) / 2.0;
    assert_rect(m.rect(1), rect(16.0, 16.0, HALF, half_height));
    assert_rect(
        m.rect(2),
        rect(16.0, 16.0 + half_height + 16.0, HALF, half_height),
    );
    assert_eq!(m.focused(), Some(2));
    // Sharing a column, it leaves it for a new column on the left.
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    assert_rect(m.rect(2), rect(16.0, 16.0, HALF, TALL));
    assert_rect(m.rect(1), rect(16.0 + HALF + 16.0, 16.0, HALF, TALL));
    assert_eq!(m.focused(), Some(2));
}

/// niri: resources/default-config.kdl:542-546.
#[test]
fn consume_or_expel_right_mirrors_left() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Right));
    assert_eq!(m.rect(1).x, m.rect(2).x);
    assert!(
        m.rect(2).y < m.rect(1).y,
        "window 1 joined the bottom of 2's column"
    );
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Right));
    assert!(
        m.rect(1).x > m.rect(2).x,
        "window 1 left for a column on the right"
    );
}

#[test]
fn focus_moves_up_and_down_inside_a_column_and_stops_at_its_ends() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.ok(|l| l.focus(OWNER, Direction::Up));
    assert_eq!(m.focused(), Some(1));
    m.ok(|l| l.focus(OWNER, Direction::Up));
    assert_eq!(m.focused(), Some(1));
    m.ok(|l| l.move_focused(OWNER, Direction::Down));
    assert!(m.rect(1).y > m.rect(2).y, "window 1 moved below window 2");
    assert_eq!(m.focused(), Some(1));
}

#[test]
fn moving_a_column_swaps_it_with_its_neighbour_and_focus_follows() {
    let mut m = three_columns();
    m.ok(|l| l.move_focused(OWNER, Direction::Left));
    let list = &m.layout.workspaces(MAIN).unwrap()[0].windows;
    assert_eq!(list, &[w(1), w(3), w(2)]);
    assert_eq!(m.focused(), Some(3));
}

/// niri: docs/wiki/Configuration:-Layout.md:182-183.
#[test]
fn proportions_count_the_gaps_so_two_halves_fill_the_output() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    assert_eq!(m.rect(1).x, 16.0);
    assert_eq!(m.rect(2).x - m.rect(1).right(), 16.0);
    assert_eq!(1920.0 - m.rect(2).right(), 16.0);
}

/// niri: src/layout/tests.rs:3389, src/layout/scrolling.rs:4796-4842 (code).
#[test]
fn preset_widths_cycle_and_a_manual_change_restarts_the_cycle() {
    let mut m = Model::new();
    m.open(1);
    let width = |m: &Model| m.rect(1).w;
    assert_eq!(width(&m), HALF);
    // Forward from a width that is no preset: the first preset wider, two thirds.
    m.ok(|l| l.cycle_preset_width(w(1), true));
    assert_eq!(width(&m), (1904.0_f64 * 2.0 / 3.0 - 16.0).round());
    m.ok(|l| l.cycle_preset_width(w(1), true));
    assert_eq!(width(&m), 1888.0);
    m.ok(|l| l.cycle_preset_width(w(1), true));
    assert_eq!(width(&m), (1904.0_f64 / 3.0 - 16.0).round());
    // A manual change forgets the preset: forward goes to the first preset wider, a half.
    m.ok(|l| l.change_width(w(1), 0.1));
    assert_eq!(width(&m), (1904.0_f64 * (1.0 / 3.0 + 0.1) - 16.0).round());
    m.ok(|l| l.cycle_preset_width(w(1), true));
    assert_eq!(width(&m), HALF);
    // Backward from a half: a third.
    m.ok(|l| l.cycle_preset_width(w(1), false));
    assert_eq!(width(&m), (1904.0_f64 / 3.0 - 16.0).round());
}

/// niri: docs/wiki/Fullscreen-and-Maximize.md:4-13.
#[test]
fn a_full_width_column_spans_the_working_area_and_goes_back_to_its_width() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.set_full_width(w(2), true));
    assert_rect(m.rect(2), rect(16.0, 16.0, 1920.0 - 32.0, TALL));
    assert!(m.info(2).full_width);
    m.ok(|l| l.set_full_width(w(2), false));
    assert_eq!(m.rect(2).w, HALF);
}

/// niri: src/layout/scrolling.rs:5464-5466 (code).
#[test]
fn a_column_wider_than_the_view_shows_from_its_left_edge() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            width: Some(ColumnWidth::Fixed(3000.0)),
            ..OpenOptions::default()
        },
    );
    assert_rect(m.rect(2), rect(0.0, 16.0, 3000.0, TALL));
}

/// niri: src/layout/scrolling.rs:5468-5470 (the padding shrinks to fit).
#[test]
fn a_column_almost_as_wide_as_the_view_shows_whole() {
    let mut m = Model::new();
    m.open_with(
        OWNER,
        1,
        OpenOptions {
            width: Some(ColumnWidth::Fixed(1910.0)),
            ..OpenOptions::default()
        },
    );
    assert_rect(m.rect(1), rect(5.0, 16.0, 1910.0, TALL));
}

#[test]
fn a_new_column_takes_the_configured_default_width() {
    let mut m = Model::with(Options {
        default_width: ColumnWidth::Proportion(1.0 / 3.0),
        ..Options::default()
    });
    m.open(1);
    assert_eq!(m.rect(1).w, (1904.0_f64 / 3.0 - 16.0).round());
}

/// niri: src/layout/tests.rs:3701, docs/wiki/Development:-Fractional-Layout.md:21-35.
#[test]
fn at_scale_one_and_a_half_every_edge_lands_on_a_physical_pixel() {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 3840, 2160, 1.5)));
    assert_rect(
        m.layout.output_rect(MAIN).unwrap(),
        rect(0.0, 0.0, 2560.0, 1440.0),
    );
    for n in 1..=4 {
        m.open(n);
        m.ok(|l| l.change_width(w(n), 0.0137 * n as f64));
    }
    for p in m.placed(MAIN) {
        for edge in [p.rect.x, p.rect.y, p.rect.right(), p.rect.bottom()] {
            let physical = edge * 1.5;
            assert!(
                (physical - physical.round()).abs() < 1e-6,
                "{edge} is between pixels"
            );
        }
    }
}

/// niri: src/layout/scrolling.rs:1848-1855 (an expelled window keeps its column's widths).
#[test]
fn an_expelled_window_keeps_its_columns_full_width() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.ok(|l| l.set_full_width(w(2), true));
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    assert!(m.info(2).full_width);
    assert!(m.info(1).full_width);
}

/// niri: src/layout/scrolling.rs:1210-1214 (forgotten only when a column left of the focus goes).
#[test]
fn a_window_closed_right_after_it_opened_gives_focus_back_though_another_closed_meanwhile() {
    let mut m = Model::new();
    for n in [1, 2, 4] {
        m.open(n);
    }
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.open(3);
    m.ok(|l| l.close_window(w(4)));
    m.ok(|l| l.close_window(w(3)));
    assert_eq!(m.focused(), Some(1));
}

/// niri: src/layout/scrolling.rs:790-797 and 3774-3779 (the view kept for leaving fullscreen
/// goes when another column takes the focus).
#[test]
fn leaving_fullscreen_does_not_bring_back_a_view_kept_for_another_column() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.ok(|l| l.focus(OWNER, Direction::Right));
    m.ok(|l| l.set_fullscreen(w(1), false));
    m.open(3);
    m.ok(|l| l.set_fullscreen(w(2), true));
    m.ok(|l| l.focus(OWNER, Direction::Left));
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert_eq!(m.rect(2).x, 16.0);
}

/// A column holding more windows than its height fits gives the last ones no height, inside
/// the working area, rather than pushing them below the output.
#[test]
fn a_column_too_full_for_its_height_stays_inside_the_working_area() {
    let mut m = Model::with(Options {
        gap: 300.0,
        ..Options::default()
    });
    m.open(0);
    for n in 1..4 {
        m.open(n);
        m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    }
    assert!(m.rect(3).bottom() <= 1080.0);
    let mut m = Model::new();
    m.open(0);
    for n in 1..68 {
        m.open(n);
        m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    }
    assert!(m.rect(67).bottom() <= 1080.0);
}
