//! Dwindle: which window takes the focus when the focused one closes, how a slot splits, and when
//! a window whose maximum size is small floats instead of splitting a slot. The output is 1920 ×
//! 1080 at scale 1 with gaps of 16, so a tree of one window fills 1888 × 1048.

use crate::harness::*;
use yuna_layout::geometry::Size;
use yuna_layout::{OpenOptions, Options};
use yuna_types::Tiling;

fn dwindle() -> Model {
    Model::with(Options {
        default_tiling: Tiling::Dwindle,
        ..Options::default()
    })
}

fn with_maximum(w: f64, h: f64) -> OpenOptions {
    OpenOptions {
        max_size: Size { w, h },
        ..OpenOptions::default()
    }
}

/// Whether a window opened with this maximum floats, on a workspace holding `before` windows.
fn floats_with(before: u64, maximum: Size) -> bool {
    let mut m = dwindle();
    for n in 1..=before {
        m.open(n);
    }
    m.open_with(OWNER, 99, with_maximum(maximum.w, maximum.h));
    m.info(99).floating
}

/// Hyprland: src/layout/algorithm/tiled/dwindle/DwindleAlgorithm.cpp:107-115. A window whose
/// maximum is smaller than the slot it would split floats; on an empty workspace the slot is the
/// working area.
#[test]
fn a_window_whose_maximum_is_below_the_slot_it_would_split_floats() {
    let size = |w, h| Size { w, h };
    // The empty workspace: the working area, 1920 × 1080.
    assert!(floats_with(0, size(500.0, 0.0)), "narrower than the area");
    assert!(floats_with(0, size(0.0, 500.0)), "lower than the area");
    assert!(
        floats_with(0, size(1900.0, 0.0)),
        "narrower than the area by little"
    );
    assert!(
        floats_with(0, size(0.0, 1070.0)),
        "lower than the area by little"
    );
    assert!(!floats_with(0, size(1920.0, 0.0)), "as wide as the area");
    assert!(!floats_with(0, size(0.0, 1080.0)), "as high as the area");
    assert!(
        !floats_with(0, size(2000.0, 1100.0)),
        "larger than the area"
    );
    assert!(!floats_with(0, size(0.0, 0.0)), "no maximum");
    // One window: the slot is its slot, 1888 × 1048, inside the gaps.
    assert!(!floats_with(1, size(1900.0, 0.0)), "wider than the slot");
    assert!(!floats_with(1, size(0.0, 1060.0)), "higher than the slot");
    assert!(
        !floats_with(1, size(1888.0, 1048.0)),
        "as large as the slot"
    );
    assert!(floats_with(1, size(1887.0, 0.0)), "narrower than the slot");
    assert!(floats_with(1, size(0.0, 1047.0)), "lower than the slot");
    assert!(
        floats_with(1, size(500.0, 2000.0)),
        "narrower on one axis only"
    );
    assert!(
        floats_with(1, size(2000.0, 500.0)),
        "lower on one axis only"
    );
    assert!(!floats_with(1, size(2000.0, 2000.0)), "larger on both axes");
}

/// The slot is the one the focused window holds, the first or the second part of its split: on an
/// output whose slots split into parts of 937 and 936 units, a maximum of 936 floats in the larger
/// slot and tiles in the smaller.
#[test]
fn the_slot_a_new_window_would_split_is_the_focused_windows_whichever_part_it_is() {
    let mut m = Model::bare(Options {
        default_tiling: Tiling::Dwindle,
        ..Options::default()
    });
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1921, 1080, 1.0)));
    m.open(1);
    m.open(2);
    let (first, second) = (m.rect(1).w, m.rect(2).w);
    assert_ne!(first, second, "the split of 1889 units is uneven");
    let smaller = first.min(second);
    for (focus, window_is_wider) in [(1, first > second), (2, second > first)] {
        m.ok(|l| l.focus_window(w(focus)));
        m.open_with(OWNER, 3, with_maximum(smaller, 0.0));
        assert_eq!(m.info(3).floating, window_is_wider, "focus on {focus}");
        m.ok(|l| l.close_window(w(3)));
    }
}

/// Yuna's rule (the dwindle module's documentation): a slot splits side by side when wider than tall,
/// one above the other otherwise, so a square one splits one above the other.
#[test]
fn a_square_slot_splits_one_above_the_other() {
    let mut m = Model::bare(Options {
        default_tiling: Tiling::Dwindle,
        ..Options::default()
    });
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1080, 1080, 1.0)));
    m.open(1);
    assert_rect(m.rect(1), rect(16.0, 16.0, 1048.0, 1048.0));
    m.open(2);
    assert_rect(m.rect(1), rect(16.0, 16.0, 1048.0, 516.0));
    assert_rect(m.rect(2), rect(16.0, 548.0, 1048.0, 516.0));
}

/// Hyprland: input:focus_on_close 0. Closing the focused window gives the focus to the window now
/// nearest where it was: the one that covers its centre once the tree is laid out again, and, when
/// the centre lies between two, the first in reading order, not the sibling's last leaf.
#[test]
fn closing_the_focused_window_focuses_the_one_nearest_where_it_was() {
    let mut m = dwindle();
    m.open(1);
    m.open(2);
    m.ok(|l| l.focus_window(w(1)));
    m.open(3);
    m.open(4);
    // 2 on the right, all the height; on the left 1 above 3 and 4, which are side by side.
    assert_rect(m.rect(2), rect(968.0, 16.0, 936.0, 1048.0));
    assert_rect(m.rect(1), rect(16.0, 16.0, 936.0, 516.0));
    assert_rect(m.rect(3), rect(16.0, 548.0, 460.0, 516.0));
    m.ok(|l| l.focus_window(w(2)));
    m.ok(|l| l.close_window(w(2)));
    // The left part now fills the output: 1 on the left, 3 above 4 on the right. The centre of 2,
    // (1436, 540), lies in the gap between 3 and 4, at 8 units from each.
    assert_rect(m.rect(3), rect(968.0, 16.0, 936.0, 516.0));
    assert_eq!(m.focused(), Some(3));
}

/// Without a tie: the window that covers the closed one's centre once the tree is laid out again
/// takes the focus, whichever is first in reading order.
#[test]
fn closing_the_focused_window_focuses_the_one_that_now_covers_where_it_was() {
    let mut m = dwindle();
    m.open(1);
    m.open(2);
    m.ok(|l| l.focus_window(w(1)));
    m.open(3);
    // 1 above 3 on the left, 2 on the right, all the height.
    m.ok(|l| l.focus_window(w(2)));
    m.ok(|l| l.close_window(w(2)));
    // 1 and 3 now fill the output side by side; the centre of 2, (1436, 540), is in 3's slot.
    assert_rect(m.rect(3), rect(968.0, 16.0, 936.0, 1048.0));
    assert_eq!(m.focused(), Some(3));
}

/// The same where the closed window lay low on the right, and the windows that remain lie above
/// and beside the place it held: the focus goes to the nearest, measured by how far its rectangle
/// is from the closed window's centre on each axis.
#[test]
fn closing_a_window_low_on_the_right_focuses_the_nearest_of_those_around_its_centre() {
    let mut m = dwindle();
    m.open(1);
    for (focus, n) in [(1, 2), (2, 3), (2, 4), (4, 5)] {
        m.ok(|l| l.focus_window(w(focus)));
        m.open(n);
    }
    // 1 on the left; 2 above 3 on the right, 3 the lower, 936 × 516; 4 above 5 beside it.
    assert_rect(m.rect(3), rect(968.0, 548.0, 936.0, 516.0));
    assert_rect(m.rect(2), rect(968.0, 16.0, 460.0, 516.0));
    m.ok(|l| l.focus_window(w(3)));
    m.ok(|l| l.close_window(w(3)));
    // 2 now fills the upper right, 4 and 5 are side by side below it, 460 wide each; the centre of
    // 3, (1436, 806), lies in the gap between 4 and 5, 8 units from each.
    assert_rect(m.rect(4), rect(968.0, 548.0, 460.0, 516.0));
    assert_rect(m.rect(5), rect(1444.0, 548.0, 460.0, 516.0));
    assert_eq!(m.focused(), Some(4));
}

/// ADR-0110, departure 1 (Hyprland's default ends the fullscreen instead): a window keeps its own
/// maximised state under its fullscreen. Maximised while fullscreen, it holds both; leaving maximised
/// ends that state alone.
#[test]
fn a_fullscreen_window_can_be_maximised_too_and_leaves_maximised_alone() {
    let mut m = dwindle();
    m.open(1);
    m.open(2);
    m.ok(|l| l.set_fullscreen(w(2), true));
    m.ok(|l| l.set_full_width(w(2), true));
    assert!(
        m.info(2).fullscreen,
        "maximising does not end its fullscreen"
    );
    assert!(m.info(2).full_width);
    m.ok(|l| l.set_full_width(w(2), false));
    assert!(m.info(2).fullscreen);
    assert!(!m.info(2).full_width);

    let mut m = dwindle();
    m.open(1);
    m.open(2);
    m.ok(|l| l.set_full_width(w(2), true));
    m.ok(|l| l.set_full_width(w(2), false));
    assert!(!m.info(2).full_width);
    assert_rect(m.rect(2), rect(968.0, 16.0, 936.0, 1048.0));
}

/// A window beneath a fullscreen one is still placed, and a new mode for its output asks it
/// another size, which it has to answer: it awaits a commit like any other window.
#[test]
fn a_window_hidden_behind_a_fullscreen_one_awaits_a_commit_after_its_output_changes_mode() {
    let mut m = dwindle();
    m.open(1);
    m.open(2);
    for n in [1, 2] {
        let asked = m.placement(n).configure;
        m.ok(|l| l.commit_size(w(n), asked));
    }
    m.ok(|l| l.set_fullscreen(w(2), true));
    assert!(!m.layout.awaits_commit(w(1)));
    m.ok(|l| l.configure_output(MAIN, 1280, 720, 1.0));
    assert!(
        m.layout.awaits_commit(w(1)),
        "the slot it is hidden in has another size"
    );
}
