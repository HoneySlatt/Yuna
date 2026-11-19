//! The size a new window is foretold, for its first configure, is the size it is asked once
//! open, in every layout: a new column, a dwindle split, a floating window, a special workspace
//! and fullscreen ([`Layout::size_on_open`](yuna_layout::Layout::size_on_open), ADR-0026). The
//! main output is 1920 × 1080 at scale 1 with gaps of 16; the side output, 2560 × 1440 pixels at
//! scale 1.5, is 1706.67 × 960 units, where sizes fall between whole units.

use crate::harness::*;
use yuna_layout::geometry::Size;
use yuna_layout::{ColumnWidth, OpenOptions, Options, Tiling};

fn size(w: f64, h: f64) -> Size {
    Size { w, h }
}

/// Asks the size window `n` would be asked if it opened with `options` on the owner's seat, then
/// opens it: the size it is asked is the size foretold, which is returned.
#[track_caller]
fn open_foretold(m: &mut Model, n: u64, options: OpenOptions) -> Size {
    let foretold = m
        .layout
        .size_on_open(OWNER, &options)
        .expect("a size is foretold");
    m.open_with(OWNER, n, options);
    assert_eq!(
        m.placement(n).configure,
        foretold,
        "window {n} is asked another size than the one foretold"
    );
    foretold
}

/// The owner's seat with the main output and the side one, `tiling` for new workspaces.
fn two_outputs(tiling: Tiling) -> Model {
    let mut m = Model::with(Options {
        default_tiling: tiling,
        ..Options::default()
    });
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 2560, 1440, 1.5)));
    m
}

#[test]
fn a_new_column_is_foretold_its_size() {
    let mut m = two_outputs(Tiling::Scrolling);
    assert_eq!(
        open_foretold(&mut m, 1, OpenOptions::default()),
        size(936.0, 1048.0)
    );
    assert_eq!(
        open_foretold(&mut m, 2, OpenOptions::default()),
        size(936.0, 1048.0)
    );
    let fixed_width = OpenOptions {
        width: Some(ColumnWidth::Fixed(500.0)),
        ..OpenOptions::default()
    };
    assert_eq!(open_foretold(&mut m, 3, fixed_width), size(500.0, 1048.0));
    let wide_minimum = OpenOptions {
        min_size: size(1000.0, 0.0),
        ..OpenOptions::default()
    };
    assert_eq!(open_foretold(&mut m, 4, wide_minimum), size(1000.0, 1048.0));
    // (1706.67 − 16) / 2 − 16 = 829.33 wide, floored; 960 − 2 × 16 tall.
    m.ok(|l| l.focus_output(OWNER, SIDE));
    assert_eq!(
        open_foretold(&mut m, 5, OpenOptions::default()),
        size(829.0, 928.0)
    );
}

/// Each new window splits the focused one: the whole working area first, then its half side by
/// side, then a half of that one above the other.
#[test]
fn dwindle_foretells_the_half_of_the_focused_window() {
    let mut m = two_outputs(Tiling::Dwindle);
    assert_eq!(
        open_foretold(&mut m, 1, OpenOptions::default()),
        size(1888.0, 1048.0)
    );
    assert_eq!(
        open_foretold(&mut m, 2, OpenOptions::default()),
        size(936.0, 1048.0)
    );
    assert_eq!(
        open_foretold(&mut m, 3, OpenOptions::default()),
        size(936.0, 516.0)
    );
}

/// A floating window chooses its own size, 0 × 0, unless a rule gives it one: on the side
/// output, a rule's 100.4 × 50.2 units are kept on its pixel grid, 151 × 75 pixels, then asked
/// in whole units.
#[test]
fn a_floating_window_is_foretold_its_rule_s_size_or_none() {
    let mut m = two_outputs(Tiling::Scrolling);
    open_foretold(&mut m, 1, OpenOptions::default());
    let floating = OpenOptions {
        floating: true,
        ..OpenOptions::default()
    };
    assert_eq!(open_foretold(&mut m, 2, floating.clone()), size(0.0, 0.0));
    let ruled = OpenOptions {
        size: Some(size(640.0, 480.0)),
        ..floating.clone()
    };
    assert_eq!(open_foretold(&mut m, 3, ruled), size(640.0, 480.0));
    let dialog = OpenOptions {
        parent: Some(w(1)),
        committed: Some(size(300.0, 200.0)),
        ..OpenOptions::default()
    };
    assert_eq!(open_foretold(&mut m, 4, dialog), size(0.0, 0.0));
    m.ok(|l| l.focus_output(OWNER, SIDE));
    let off_the_grid = OpenOptions {
        size: Some(size(100.4, 50.2)),
        ..floating
    };
    assert_eq!(open_foretold(&mut m, 5, off_the_grid), size(101.0, 50.0));
}

#[test]
fn a_window_on_a_special_workspace_is_foretold_its_size() {
    let mut m = two_outputs(Tiling::Scrolling);
    open_foretold(&mut m, 1, OpenOptions::default());
    m.ok(|l| l.toggle_special(OWNER, "scratch"));
    assert_eq!(
        open_foretold(&mut m, 2, OpenOptions::default()),
        size(936.0, 1048.0)
    );
    assert!(m.info(2).special);
    assert_eq!(
        open_foretold(&mut m, 3, OpenOptions::default()),
        size(936.0, 1048.0)
    );
}

/// Fullscreen is the output's size, in whole units: 1707 × 960 on the side output.
#[test]
fn a_fullscreen_window_is_foretold_its_output_s_size() {
    let fullscreen = OpenOptions {
        fullscreen: true,
        ..OpenOptions::default()
    };
    for tiling in [Tiling::Scrolling, Tiling::Dwindle] {
        let mut m = two_outputs(tiling);
        open_foretold(&mut m, 1, OpenOptions::default());
        assert_eq!(
            open_foretold(&mut m, 2, fullscreen.clone()),
            size(1920.0, 1080.0)
        );
        m.ok(|l| l.focus_output(OWNER, SIDE));
        assert_eq!(
            open_foretold(&mut m, 3, fullscreen.clone()),
            size(1707.0, 960.0)
        );
    }
}
