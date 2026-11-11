//! The two cases the owner found broken in Umbriel, on the owner's 4K monitor at scale 1.5
//! (2560 × 1440 logical): fullscreen on a workspace with a single column, and fullscreen in
//! dwindle.

use crate::harness::*;
use yuna_layout::{ColumnWidth, OpenOptions, Options, Tiling};

const OUTPUT: yuna_layout::geometry::Rect =
    yuna_layout::geometry::Rect::new(0.0, 0.0, 2560.0, 1440.0);

fn four_k(tiling: Tiling) -> Model {
    let mut m = Model::bare(Options {
        default_tiling: tiling,
        ..Options::default()
    });
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 3840, 2160, 1.5)));
    m
}

/// A window alone on its workspace goes fullscreen and back, at the default width and at the
/// whole width (the owner's rule for a window alone): it covers the output exactly, then finds
/// exactly its rectangle again.
#[test]
fn a_lone_column_goes_fullscreen_and_comes_back_exactly() {
    for width in [None, Some(ColumnWidth::Proportion(1.0))] {
        let mut m = four_k(Tiling::Scrolling);
        m.open_with(
            OWNER,
            1,
            OpenOptions {
                width,
                ..OpenOptions::default()
            },
        );
        let before = m.placement(1);
        m.ok(|l| l.set_fullscreen(w(1), true));
        assert_rect(m.rect(1), OUTPUT);
        assert!(m.placement(1).fullscreen);
        assert_eq!(m.shown(MAIN), vec![1]);
        m.ok(|l| l.set_fullscreen(w(1), false));
        assert_eq!(m.placement(1), before);
        // Twice more, quickly: still exact.
        m.ok(|l| l.set_fullscreen(w(1), true));
        m.ok(|l| l.set_fullscreen(w(1), true));
        m.ok(|l| l.set_fullscreen(w(1), false));
        assert_eq!(m.placement(1), before);
    }
}

/// In dwindle, the fullscreen window covers the output and is the only one drawn; the others
/// neither show over it nor through it, and the tree comes back unchanged.
#[test]
fn fullscreen_in_dwindle_covers_the_output_alone_and_the_tree_comes_back() {
    let mut m = four_k(Tiling::Dwindle);
    for n in 1..=3 {
        m.open(n);
    }
    m.open_with(
        OWNER,
        4,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.focus_window(w(2)));
    let before = m.placed(MAIN);
    m.ok(|l| l.set_fullscreen(w(2), true));
    let placed = m.placed(MAIN);
    assert_eq!(placed.len(), 1);
    assert_eq!(placed[0].window, w(2));
    assert_rect(placed[0].rect, OUTPUT);
    m.ok(|l| l.set_fullscreen(w(2), false));
    let after = m.placed(MAIN);
    for b in &before {
        let a = after.iter().find(|a| a.window == b.window).unwrap();
        assert_eq!(a.rect, b.rect, "window {:?} moved", b.window);
    }
}
