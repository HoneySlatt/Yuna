//! Real clients: the size each window is asked, the size its client commits, its limits, dialogs,
//! focus when a window opens, and activation requests (M1.4, [ADR-0026](../../../../docs/adr/0026-real-clients-in-the-layout.md)).
//!
//! niri's behaviours were read at commit 7f81a7f (2026-11-10), Hyprland's at 70ec30e
//! (2026-11-11). What real clients do was read in `WAYLAND_DEBUG` traces of Alacritty 0.17,
//! GTK 4.24's `gtk4-demo` and Qt 6.12's `qml` under a headless sway, on 2026-11-12. The output is
//! 1920 × 1080 at scale 1 with gaps of 16: a column of half the width is 936 wide, a window alone
//! in a column 1048 tall.

use crate::harness::*;
use yuna_layout::geometry::Size;
use yuna_layout::{Direction, LayoutError, OpenOptions, Options, OutputConfig, Tiling};

fn size(w: f64, h: f64) -> Size {
    Size { w, h }
}

const NONE: Size = Size { w: 0.0, h: 0.0 };

fn dwindle() -> Model {
    Model::with(Options {
        default_tiling: Tiling::Dwindle,
        ..Options::default()
    })
}

/// A dialog of window `parent`, which committed `committed` before it was mapped.
fn dialog(parent: u64, committed: Size) -> OpenOptions {
    OpenOptions {
        parent: Some(w(parent)),
        committed: Some(committed),
        ..OpenOptions::default()
    }
}

/// Window `n`'s configure size.
#[track_caller]
fn configure(m: &Model, n: u64) -> Size {
    m.placement(n).configure
}

/// Windows 1, 2, 3 in one column, from top to bottom.
fn one_column_of_three() -> Model {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.open(3);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m
}

/// niri: src/handlers/xdg_shell.rs:1079-1230. The traces show why: sway's first configure is
/// 0 × 0, so every client drew one frame at its own default size before its tile's.
#[test]
fn the_first_configure_carries_the_size_the_window_will_have() {
    let mut m = Model::new();
    let before = format!("{:?}", m.layout);
    assert_eq!(
        m.layout.size_on_open(OWNER, &OpenOptions::default()),
        Ok(size(936.0, 1048.0))
    );
    assert_eq!(
        before,
        format!("{:?}", m.layout),
        "asking changed the model"
    );
    m.open(1);
    assert_eq!(configure(&m, 1), size(936.0, 1048.0));
    let fullscreen = OpenOptions {
        fullscreen: true,
        ..OpenOptions::default()
    };
    assert_eq!(
        m.layout.size_on_open(OWNER, &fullscreen),
        Ok(size(1920.0, 1080.0))
    );
    // A dialog chooses its own size: 0 × 0 tells its client so.
    let unsized_dialog = OpenOptions {
        parent: Some(w(1)),
        ..OpenOptions::default()
    };
    assert_eq!(m.layout.size_on_open(OWNER, &unsized_dialog), Ok(NONE));
    // niri asks a floating window whose height alone is fixed 0 × that height
    // (src/window/mod.rs:377-396 floats it; src/layout/floating.rs asks per axis).
    let fixed_height = OpenOptions {
        min_size: size(0.0, 300.0),
        max_size: size(0.0, 300.0),
        ..OpenOptions::default()
    };
    assert_eq!(
        m.layout.size_on_open(OWNER, &fixed_height),
        Ok(size(0.0, 300.0))
    );
    let fixed = OpenOptions {
        min_size: size(400.0, 300.0),
        max_size: size(400.0, 300.0),
        ..OpenOptions::default()
    };
    assert_eq!(m.layout.size_on_open(OWNER, &fixed), Ok(size(400.0, 300.0)));
    let mut d = dwindle();
    d.open(1);
    assert_eq!(
        d.layout.size_on_open(OWNER, &OpenOptions::default()),
        Ok(size(936.0, 1048.0)),
        "in dwindle, half of the focused window's space"
    );
}

/// niri rounds the heights it asks to whole logical units (src/layout/scrolling.rs:4838) and
/// floors the widths, so that proportional columns fit the screen exactly (src/layout/tile.rs,
/// `to_i32_floor`); both at least 1.
#[test]
fn configure_sizes_are_whole_logical_units() {
    let mut m = Model::bare(Options {
        default_width: yuna_layout::ColumnWidth::Proportion(0.45),
        ..Options::default()
    });
    m.ok(|l| l.add_seat(OWNER));
    let output = OutputConfig {
        position: yuna_layout::geometry::Point { x: 0.0, y: 0.0 },
        physical_width: 3840,
        physical_height: 2160,
        scale: 1.5,
    };
    m.ok(|l| l.add_output(OWNER, MAIN, output));
    m.open(1);
    // (2560 - 16) × 0.45 - 16 = 1128.8, on the physical grid 1128.67: asked 1128.
    assert_eq!(configure(&m, 1), size(1128.0, 1408.0));
}

/// Yuna's rule: 0 tells a client to choose its own size, so a tile never asks it.
#[test]
fn a_tile_is_never_asked_a_size_of_zero() {
    let mut m = dwindle();
    for n in 1..=24 {
        m.open(n);
    }
    for placed in m.placed(MAIN) {
        assert!(
            placed.configure.w >= 1.0 && placed.configure.h >= 1.0,
            "{placed:?}"
        );
    }
}

/// niri: src/layout/tile.rs:782-823, a tile is its window's committed size: a column follows a
/// window that commits more or less than it was asked. src/layout/scrolling.rs:1308-1343: the
/// focused column stays where it is on screen; a column on its left that changes width moves.
#[test]
fn a_column_follows_the_size_its_window_commits() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.commit_size(w(1), size(500.0, 400.0)));
    assert_rect(m.rect(1), rect(16.0 + 936.0 - 500.0, 16.0, 500.0, 400.0));
    assert_eq!(m.rect(2).x, 16.0 + 936.0 + 16.0, "the focused column stays");
    assert_eq!(
        configure(&m, 1),
        size(936.0, 1048.0),
        "the ask is unchanged"
    );
    m.ok(|l| l.commit_size(w(1), size(1000.0, 1048.0)));
    assert_eq!(m.rect(2).x - m.rect(1).right(), 16.0);
}

/// niri: src/layout/scrolling.rs:4634-4664.
#[test]
fn a_columns_width_respects_its_windows_minimums_and_maximums() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_size_limits(w(1), size(1200.0, 0.0), NONE));
    assert_eq!(configure(&m, 1).w, 1200.0);
    m.ok(|l| l.set_size_limits(w(1), NONE, size(600.0, 0.0)));
    assert_eq!(configure(&m, 1).w, 600.0);
    // In one column, a minimum wins over a smaller maximum.
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.ok(|l| l.set_size_limits(w(2), size(800.0, 0.0), NONE));
    assert_eq!(configure(&m, 1).w, 800.0);
    assert_eq!(configure(&m, 2).w, 800.0);
}

/// niri: src/layout/scrolling.rs:4795-4865, heights shared and rounded one after the other.
#[test]
fn heights_share_a_column_and_give_each_window_its_minimum() {
    let mut m = one_column_of_three();
    // 1080 - 4 × 16 = 1016 to share.
    let heights: Vec<f64> = [1, 2, 3].map(|n| configure(&m, n).h).to_vec();
    assert_eq!(heights, vec![339.0, 339.0, 338.0]);
    m.ok(|l| l.set_size_limits(w(1), size(0.0, 600.0), NONE));
    let heights: Vec<f64> = [1, 2, 3].map(|n| configure(&m, n).h).to_vec();
    assert_eq!(heights, vec![600.0, 208.0, 208.0]);
    for n in [1, 2, 3] {
        m.ok(|l| l.commit_size(w(n), configure_of(l, n)));
    }
    assert_eq!(m.rect(2).y, 16.0 + 600.0 + 16.0);
    assert_eq!(m.rect(3).y, 16.0 + 600.0 + 16.0 + 208.0 + 16.0);
}

fn configure_of(layout: &yuna_layout::Layout, n: u64) -> Size {
    let output = layout.window(w(n)).unwrap().output.unwrap();
    layout
        .arrange(output)
        .unwrap()
        .into_iter()
        .find(|p| p.window == w(n))
        .unwrap()
        .configure
}

/// niri: src/layout/scrolling.rs:4806-4826: nothing shrinks below its minimum, the column
/// overflows the working area.
#[test]
fn minimum_heights_that_do_not_fit_overflow_the_column() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    for n in [1, 2] {
        m.ok(|l| l.set_size_limits(w(n), size(0.0, 700.0), NONE));
    }
    assert_eq!(configure(&m, 1).h, 700.0);
    assert_eq!(configure(&m, 2).h, 700.0);
    for n in [1, 2] {
        m.ok(|l| l.commit_size(w(n), size(936.0, 700.0)));
    }
    assert_eq!(m.rect(2).y, 16.0 + 700.0 + 16.0);
}

/// niri: src/layout/scrolling.rs:4763-4775, a window whose minimum and maximum heights are equal
/// keeps that height; the others share the rest.
#[test]
fn a_fixed_height_window_keeps_its_height_in_a_column() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.ok(|l| l.set_size_limits(w(1), size(0.0, 300.0), size(0.0, 300.0)));
    assert_eq!(configure(&m, 1).h, 300.0);
    assert_eq!(configure(&m, 2).h, 1080.0 - 3.0 * 16.0 - 300.0);
}

/// niri: src/window/mod.rs:377-396 (a parent, or a fixed height, opens floating) and
/// src/layout/workspace.rs:696-704 (centred on the parent, at its own size).
#[test]
fn a_dialog_opens_floating_centred_on_its_parent_at_its_own_size() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(OWNER, 2, dialog(1, size(400.0, 300.0)));
    assert!(m.info(2).floating);
    assert_rect(
        m.rect(2),
        rect(
            16.0 + (936.0 - 400.0) / 2.0,
            16.0 + (1048.0 - 300.0) / 2.0,
            400.0,
            300.0,
        ),
    );
    assert_eq!(
        configure(&m, 2),
        NONE,
        "a floating window chooses its own size"
    );
    // Its client resizes it: it keeps its top-left corner.
    m.ok(|l| l.commit_size(w(2), size(500.0, 350.0)));
    assert_rect(m.rect(2), rect(284.0, 390.0, 500.0, 350.0));
    // A fixed height floats too, centred in the working area, asked its fixed size.
    let fixed = OpenOptions {
        min_size: size(400.0, 300.0),
        max_size: size(400.0, 300.0),
        committed: Some(size(400.0, 300.0)),
        ..OpenOptions::default()
    };
    m.open_with(OWNER, 3, fixed);
    assert!(m.info(3).floating);
    assert_rect(m.rect(3), rect(760.0, 390.0, 400.0, 300.0));
    assert_eq!(configure(&m, 3), size(400.0, 300.0));
}

/// niri: src/layout/workspace.rs:652,681: a dialog takes focus only from its parent.
#[test]
fn a_dialog_takes_the_focus_only_if_its_parent_has_it() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.open_with(OWNER, 3, dialog(1, size(300.0, 200.0)));
    assert_eq!(m.focused(), Some(2));
    m.open_with(OWNER, 4, dialog(2, size(300.0, 200.0)));
    assert_eq!(m.focused(), Some(4));
}

/// Yuna's rule (ADR-0020, games first): a fullscreen game's own dialog takes the focus and shows
/// over it, the game staying fullscreen; another window opening goes behind it.
#[test]
fn a_fullscreen_games_own_dialog_takes_the_focus_over_it() {
    for mut m in [Model::new(), dwindle()] {
        m.open(1);
        m.ok(|l| l.set_fullscreen(w(1), true));
        m.open(2);
        assert_eq!(m.focused(), Some(1), "a window that is not its dialog");
        m.open_with(OWNER, 3, dialog(1, size(400.0, 300.0)));
        assert_eq!(m.focused(), Some(3));
        assert!(m.info(1).fullscreen);
        let shown = m.shown(MAIN);
        assert_eq!(shown.last(), Some(&3), "the dialog is drawn over the game");
        assert!(shown.contains(&1));
        m.ok(|l| l.close_window(w(3)));
        assert_eq!(m.focused(), Some(1));
        assert!(m.info(1).fullscreen);
    }
}

/// niri: src/layout/workspace.rs:696: a dialog opens where its parent is.
#[test]
fn a_dialog_opens_on_its_parents_workspace() {
    let mut m = Model::new();
    m.ok(|l| l.add_workspace(OWNER, "a", Some(MAIN)));
    m.ok(|l| l.add_workspace(OWNER, "b", Some(MAIN)));
    m.ok(|l| l.focus_workspace(OWNER, "a"));
    m.open(1);
    m.ok(|l| l.focus_workspace(OWNER, "b"));
    m.open_with(OWNER, 2, dialog(1, size(300.0, 200.0)));
    assert_eq!(m.info(2).workspace.as_deref(), Some("a"));
    assert_eq!(m.focused(), None);
}

/// niri: src/handlers/mod.rs:738-815 focuses a window whose activation is valid; Yuna refuses it
/// while a fullscreen window holds its seat's focus, unless it is that window's dialog (games
/// first, ADR-0026). Refused, the compositor marks the window urgent.
#[test]
fn activation_focuses_unless_a_fullscreen_window_holds_the_focus() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    assert_eq!(m.layout.request_activation(w(1)), Ok(true));
    assert_eq!(m.focused(), Some(1));
    m.open(3);
    m.ok(|l| l.set_fullscreen(w(3), true));
    let before = format!("{:?}", m.layout);
    assert_eq!(m.layout.request_activation(w(1)), Ok(false));
    assert_eq!(
        before,
        format!("{:?}", m.layout),
        "a refused activation changed the model"
    );
    m.open_with(OWNER, 4, dialog(3, size(300.0, 200.0)));
    m.ok(|l| l.focus_window(w(3)));
    assert_eq!(m.layout.request_activation(w(4)), Ok(true));
    assert_eq!(m.focused(), Some(4));
    assert_eq!(
        m.layout.request_activation(w(99)),
        Err(LayoutError::UnknownWindow(w(99)))
    );
}

/// Hyprland: src/layout/algorithm/tiled/dwindle/DwindleAlgorithm.cpp:107-115: a window whose
/// maximum is smaller than the space it would split floats.
#[test]
fn in_dwindle_a_window_whose_maximum_does_not_fill_its_space_floats() {
    let mut m = dwindle();
    let small = OpenOptions {
        max_size: size(400.0, 300.0),
        committed: Some(size(400.0, 300.0)),
        ..OpenOptions::default()
    };
    m.open_with(OWNER, 1, small.clone());
    assert!(m.info(1).floating, "smaller than the empty working area");
    m.open(2);
    m.open_with(OWNER, 3, small);
    assert!(m.info(3).floating);
    assert_rect(m.rect(3), rect(760.0, 390.0, 400.0, 300.0));
}

/// Hyprland: src/desktop/WindowTarget.cpp:233-246 (`misc:size_limits_tiled` false): dwindle asks a
/// tile its space whatever its minimum. Yuna's rule: a window larger than its space is clipped to
/// it, so that it never covers its neighbours.
#[test]
fn dwindle_ignores_minimums_and_clips_a_window_to_its_space() {
    let mut m = dwindle();
    m.open(1);
    m.open(2);
    m.ok(|l| l.set_size_limits(w(2), size(1500.0, 0.0), NONE));
    assert_eq!(configure(&m, 2), size(936.0, 1048.0));
    m.ok(|l| l.commit_size(w(2), size(1500.0, 1048.0)));
    assert_rect(m.rect(2), rect(968.0, 16.0, 936.0, 1048.0));
    m.ok(|l| l.commit_size(w(2), size(500.0, 400.0)));
    assert_rect(m.rect(2), rect(968.0, 16.0, 500.0, 400.0));
}

/// niri: src/layout/tile.rs: a fullscreen window smaller than its output is centred on it.
#[test]
fn a_fullscreen_window_smaller_than_its_output_is_centred() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    assert_eq!(configure(&m, 1), size(1920.0, 1080.0));
    m.ok(|l| l.commit_size(w(1), size(1280.0, 720.0)));
    assert_rect(m.rect(1), rect(320.0, 180.0, 1280.0, 720.0));
}

/// The traces: Alacritty asks a minimum of 2 × 1, Qt none, GTK changes its own twice while it
/// starts (401 × 159, then 373 × 130); each then commits exactly the size it was asked.
#[test]
fn real_clients_take_the_size_they_are_asked() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_size_limits(w(1), size(2.0, 1.0), NONE));
    m.open(2);
    m.ok(|l| l.set_size_limits(w(2), NONE, NONE));
    m.open(3);
    m.ok(|l| l.set_size_limits(w(3), size(401.0, 159.0), NONE));
    m.ok(|l| l.set_size_limits(w(3), size(373.0, 130.0), NONE));
    let before = m.placed(MAIN);
    for n in [1, 2, 3] {
        m.ok(|l| l.commit_size(w(n), configure_of(l, n)));
    }
    assert_eq!(
        m.placed(MAIN),
        before,
        "a window that takes its size moves nothing"
    );
    for placed in before {
        assert_eq!(
            Size {
                w: placed.rect.w,
                h: placed.rect.h
            },
            placed.configure
        );
    }
}

/// Yuna's rules: bad sizes from a client are refused and change nothing.
#[test]
fn bad_sizes_and_limits_are_refused() {
    let mut m = Model::new();
    m.open(1);
    for bad in [
        size(0.0, 100.0),
        size(-1.0, 100.0),
        size(f64::NAN, 100.0),
        size(100.0, f64::INFINITY),
        size(100_001.0, 100.0),
    ] {
        m.refused(LayoutError::InvalidValue, |l| l.commit_size(w(1), bad));
    }
    m.refused(LayoutError::InvalidValue, |l| {
        l.set_size_limits(w(1), size(500.0, 0.0), size(400.0, 0.0))
    });
    m.refused(LayoutError::InvalidValue, |l| {
        l.set_size_limits(w(1), size(-1.0, 0.0), NONE)
    });
    m.refused(LayoutError::UnknownWindow(w(9)), |l| {
        l.commit_size(w(9), size(10.0, 10.0))
    });
    m.refused(LayoutError::UnknownWindow(w(9)), |l| {
        l.set_size_limits(w(9), NONE, NONE)
    });
    let unknown_parent = OpenOptions {
        parent: Some(w(9)),
        ..OpenOptions::default()
    };
    m.refused(LayoutError::UnknownWindow(w(9)), |l| {
        l.open_window(OWNER, w(2), unknown_parent)
    });
    let committed_nothing = OpenOptions {
        committed: Some(NONE),
        ..OpenOptions::default()
    };
    m.refused(LayoutError::InvalidValue, |l| {
        l.open_window(OWNER, w(2), committed_nothing)
    });
}
