//! Outputs coming, going and given another mode: workspaces move and come back, windows are
//! never lost (niri), and are placed again in an output's new rectangle.

use crate::harness::*;
use yuna_layout::{LayoutError, OpenOptions, Options};
use yuna_types::Size;

/// `MAIN` with workspaces "a" and "b", "b" shown with window 1; `SIDE` with "c".
fn two_outputs() -> Model {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1280, 800, 1.0)));
    m.ok(|l| l.add_workspace(OWNER, "a", Some(MAIN)));
    m.ok(|l| l.add_workspace(OWNER, "b", Some(MAIN)));
    m.ok(|l| l.add_workspace(OWNER, "c", Some(SIDE)));
    m.ok(|l| l.focus_workspace(OWNER, "c"));
    m.ok(|l| l.focus_workspace(OWNER, "b"));
    m.open(1);
    m
}

/// niri: src/layout/tests.rs:2681, src/layout/tests.rs:1991.
#[test]
fn a_removed_output_hands_its_workspaces_over_and_the_other_keeps_what_it_shows() {
    let mut m = two_outputs();
    m.ok(|l| l.remove_output(MAIN));
    assert_eq!(
        m.workspaces(SIDE),
        (vec!["c".into(), "a".into(), "b".into()], 0)
    );
    assert_eq!(m.info(1).output, Some(SIDE));
    assert_eq!(m.layout.active_output(OWNER), Some(SIDE));
}

/// niri: docs/wiki/Workspaces.md:25-26, src/layout/tests.rs:2681, src/layout/tests.rs:2062.
#[test]
fn a_returning_output_takes_back_its_workspaces_and_shows_the_one_it_showed() {
    let mut m = two_outputs();
    m.ok(|l| l.remove_output(MAIN));
    // A window opening on "b" while it is away does not change a named workspace's home.
    m.ok(|l| l.focus_workspace(OWNER, "b"));
    m.open(2);
    m.ok(|l| l.focus_workspace(OWNER, "c"));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    assert_eq!(m.workspaces(MAIN), (vec!["a".into(), "b".into()], 1));
    assert_eq!(m.workspaces(SIDE), (vec!["c".into()], 0));
    assert_eq!(m.info(2).output, Some(MAIN));
}

/// niri: src/layout/tests.rs:2041, docs/wiki/Development:-Design-Principles.md (invisible state).
#[test]
fn an_unnamed_workspace_used_elsewhere_stays_there() {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    m.ok(|l| l.add_output(OWNER, SIDE, output(1920.0, 1920, 1080, 1.0)));
    m.ok(|l| l.focus_output(OWNER, MAIN));
    m.open(1);
    m.ok(|l| l.remove_output(MAIN));
    // Window 2 opens on the unnamed workspace that came from MAIN: it now belongs to SIDE.
    m.ok(|l| l.focus_window(w(1)));
    m.open(2);
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    assert_eq!(m.info(1).output, Some(SIDE));
}

/// niri: src/layout/tests.rs:2361, src/layout/mod.rs:2458.
#[test]
fn without_any_output_the_workspaces_wait_with_their_windows() {
    let mut m = two_outputs();
    m.ok(|l| l.remove_output(SIDE));
    m.ok(|l| l.remove_output(MAIN));
    assert_eq!(m.info(1).output, None);
    assert_eq!(m.focused(), None);
    // A window opening now waits with them.
    m.open(2);
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    let names = m.workspaces(MAIN).0;
    for name in ["a", "b", "c"] {
        assert!(names.iter().any(|n| n == name), "{name} came back");
    }
    assert_eq!(m.info(1).output, Some(MAIN));
    assert_eq!(m.info(2).output, Some(MAIN));
}

#[test]
fn a_pinned_window_stays_in_view_when_its_output_goes() {
    let mut m = two_outputs();
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.set_pinned(w(2), true));
    m.ok(|l| l.remove_output(MAIN));
    assert_eq!(m.info(2).output, Some(SIDE));
    assert!(m.info(2).pinned);
    assert!(m.shown(SIDE).contains(&2));
}

#[test]
fn a_pinned_window_comes_back_into_view_when_a_seats_only_output_is_replaced() {
    let mut m = Model::new();
    m.ok(|l| l.focus_workspace(OWNER, "1"));
    m.ok(|l| l.focus_workspace(OWNER, "2"));
    m.open_with(
        OWNER,
        1,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.set_pinned(w(1), true));
    m.ok(|l| l.remove_output(MAIN));
    m.ok(|l| l.add_output(OWNER, SIDE, output(0.0, 1920, 1080, 1.0)));
    assert!(m.shown(SIDE).contains(&1));
    assert!(m.info(1).pinned);
}

/// Window `n`'s rectangle and the size it is asked, as `(x, y, w, h)` and `(w, h)`.
fn place(m: &Model, n: u64) -> ((f64, f64, f64, f64), (f64, f64)) {
    let p = m.placement(n);
    (
        (p.rect.x, p.rect.y, p.rect.w, p.rect.h),
        (p.configure.w, p.configure.h),
    )
}

/// An output given a smaller mode keeps its windows, their order and the focus, and places them
/// again inside it: two columns of half its width, 616 wide where they were 936.
#[test]
fn an_output_made_smaller_places_its_windows_again_inside_it() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    assert_eq!(place(&m, 1), ((16.0, 16.0, 936.0, 1048.0), (936.0, 1048.0)));
    assert_eq!(
        place(&m, 2),
        ((968.0, 16.0, 936.0, 1048.0), (936.0, 1048.0))
    );
    m.ok(|l| l.configure_output(MAIN, 1280, 720, 1.0));
    assert_rect(
        m.layout.output_rect(MAIN).expect("the output exists"),
        rect(0.0, 0.0, 1280.0, 720.0),
    );
    assert_eq!(place(&m, 1), ((16.0, 16.0, 616.0, 688.0), (616.0, 688.0)));
    assert_eq!(place(&m, 2), ((648.0, 16.0, 616.0, 688.0), (616.0, 688.0)));
    assert_eq!(m.shown(MAIN), vec![1, 2]);
    assert_eq!(m.focused(), Some(2));
}

/// An output given a larger mode places its windows again in all of it.
#[test]
fn an_output_made_larger_places_its_windows_again_in_all_of_it() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.configure_output(MAIN, 2560, 1440, 1.0));
    assert_rect(
        m.layout.output_rect(MAIN).expect("the output exists"),
        rect(0.0, 0.0, 2560.0, 1440.0),
    );
    assert_eq!(
        place(&m, 1),
        ((16.0, 16.0, 1256.0, 1408.0), (1256.0, 1408.0))
    );
    assert_eq!(
        place(&m, 2),
        ((1288.0, 16.0, 1256.0, 1408.0), (1256.0, 1408.0))
    );
    assert_eq!(m.focused(), Some(2));
}

/// A scale raised, then lowered, changes the output's logical size with its pixels kept: 3840 ×
/// 2160 pixels are 2560 × 1440 units at 1.5, 1920 × 1080 at 2, 3072 × 1728 at 1.25. Every
/// rectangle stays on the new pixel grid (the model's invariants check it after each step).
#[test]
fn a_scale_raised_then_lowered_places_the_windows_again_on_its_grid() {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 3840, 2160, 1.5)));
    m.open(1);
    m.open(2);
    assert_eq!(
        place(&m, 2),
        ((1288.0, 16.0, 1256.0, 1408.0), (1256.0, 1408.0))
    );
    m.ok(|l| l.configure_output(MAIN, 3840, 2160, 2.0));
    assert_eq!(
        m.layout
            .output_scale(MAIN)
            .expect("the output exists")
            .get(),
        2.0
    );
    assert_rect(
        m.layout.output_rect(MAIN).expect("the output exists"),
        rect(0.0, 0.0, 1920.0, 1080.0),
    );
    assert_eq!(place(&m, 1), ((16.0, 16.0, 936.0, 1048.0), (936.0, 1048.0)));
    assert_eq!(
        place(&m, 2),
        ((968.0, 16.0, 936.0, 1048.0), (936.0, 1048.0))
    );
    m.ok(|l| l.configure_output(MAIN, 3840, 2160, 1.25));
    assert_rect(
        m.layout.output_rect(MAIN).expect("the output exists"),
        rect(0.0, 0.0, 3072.0, 1728.0),
    );
    assert_eq!(
        place(&m, 2),
        ((1544.0, 16.0, 1512.0, 1696.0), (1512.0, 1696.0))
    );
    assert_eq!(m.shown(MAIN), vec![1, 2]);
    assert_eq!(m.focused(), Some(2));
}

/// A fullscreen game that draws 1280 × 720 whatever it is asked is centred on 1920 × 1080, at
/// (320, 180). The output given 2560 × 1440, the game is placed at the size it is now asked, the
/// whole output, and the layout says it awaits a commit; one that never commits again stays so.
/// Its next commit, the same 1280 × 720, centres it again, at (640, 360). A window whose asked
/// size the change leaves awaits nothing.
#[test]
fn a_window_that_keeps_its_size_waits_at_its_asked_size_for_its_next_commit() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.ok(|l| {
        l.commit_size(
            w(1),
            Size {
                w: 1280.0,
                h: 720.0,
            },
        )
    });
    m.ok(|l| l.focus_workspace(OWNER, "floating"));
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.commit_size(w(2), Size { w: 300.0, h: 200.0 }));
    m.ok(|l| l.focus_window(w(1)));
    assert_rect(m.rect(1), rect(320.0, 180.0, 1280.0, 720.0));
    assert!(!m.layout.awaits_commit(w(1)));

    m.ok(|l| l.configure_output(MAIN, 2560, 1440, 1.0));
    assert!(m.layout.awaits_commit(w(1)));
    assert!(!m.layout.awaits_commit(w(2)));
    let p = m.placement(1);
    assert_rect(p.rect, rect(0.0, 0.0, 2560.0, 1440.0));
    assert_eq!((p.configure.w, p.configure.h), (2560.0, 1440.0));

    m.ok(|l| {
        l.commit_size(
            w(1),
            Size {
                w: 1280.0,
                h: 720.0,
            },
        )
    });
    assert!(!m.layout.awaits_commit(w(1)));
    assert_rect(m.rect(1), rect(640.0, 360.0, 1280.0, 720.0));
    assert!(!m.layout.awaits_commit(w(9)));
}

/// Windows whose clients committed the sizes they were asked are placed at once for the sizes
/// the new mode asks, not by their old commits: smaller, both columns fit at 616 wide; larger
/// again, at 936. Placed by the old commits, the view would keep the focused column where the
/// old widths needed it, and once the clients commit, the column on its left would be cut off
/// the screen though both fit. Their next commits change nothing. A window whose asked size the
/// change leaves, floating at a size of its own, keeps its commit.
#[test]
fn windows_are_placed_for_the_sizes_the_new_mode_asks_not_their_old_commits() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| {
        l.commit_size(
            w(1),
            Size {
                w: 936.0,
                h: 1048.0,
            },
        )
    });
    m.ok(|l| {
        l.commit_size(
            w(2),
            Size {
                w: 936.0,
                h: 1048.0,
            },
        )
    });
    m.ok(|l| l.focus_workspace(OWNER, "floating"));
    m.open_with(
        OWNER,
        3,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.commit_size(w(3), Size { w: 300.0, h: 200.0 }));
    m.ok(|l| l.focus_window(w(2)));

    m.ok(|l| l.configure_output(MAIN, 1280, 720, 1.0));
    assert_eq!(place(&m, 1), ((16.0, 16.0, 616.0, 688.0), (616.0, 688.0)));
    assert_eq!(place(&m, 2), ((648.0, 16.0, 616.0, 688.0), (616.0, 688.0)));
    for n in [1, 2] {
        m.ok(|l| l.commit_size(w(n), Size { w: 616.0, h: 688.0 }));
    }
    assert_eq!(place(&m, 1), ((16.0, 16.0, 616.0, 688.0), (616.0, 688.0)));
    assert_eq!(place(&m, 2), ((648.0, 16.0, 616.0, 688.0), (616.0, 688.0)));

    m.ok(|l| l.configure_output(MAIN, 1920, 1080, 1.0));
    assert_eq!(place(&m, 1), ((16.0, 16.0, 936.0, 1048.0), (936.0, 1048.0)));
    assert_eq!(
        place(&m, 2),
        ((968.0, 16.0, 936.0, 1048.0), (936.0, 1048.0))
    );
    m.ok(|l| l.focus_window(w(3)));
    assert_eq!((m.rect(3).w, m.rect(3).h), (300.0, 200.0));
}

/// A fullscreen window takes the output's new size; a floating window keeps its place relative
/// to the working area where it still fits, and one left past the new edges is moved only as
/// far as Mutter's rule needs: a quarter of it, 50 × 37.5 units here, stays on.
#[test]
fn fullscreen_windows_take_the_new_size_and_floating_ones_stay_within_reach() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.ok(|l| l.focus_workspace(OWNER, "floating"));
    let floating = OpenOptions {
        floating: true,
        ..OpenOptions::default()
    };
    m.open_with(OWNER, 2, floating.clone());
    m.open_with(OWNER, 3, floating);
    m.ok(|l| l.set_floating_rect(w(2), rect(100.0, 100.0, 400.0, 300.0)));
    m.ok(|l| l.set_floating_rect(w(3), rect(1700.0, 900.0, 200.0, 150.0)));

    m.ok(|l| l.configure_output(MAIN, 1280, 720, 1.0));
    assert_rect(m.rect(2), rect(100.0, 100.0, 400.0, 300.0));
    assert_rect(m.rect(3), rect(1230.0, 683.0, 200.0, 150.0));
    m.ok(|l| l.focus_window(w(1)));
    let p = m.placement(1);
    assert_rect(p.rect, rect(0.0, 0.0, 1280.0, 720.0));
    assert_eq!((p.configure.w, p.configure.h), (1280.0, 720.0));
    assert!(p.fullscreen);
}

/// A mode that leaves the output's logical rectangle as it was, twice the pixels at twice the
/// scale, leaves every window where it was; the same mode again changes nothing at all.
#[test]
fn a_mode_that_keeps_the_rectangle_keeps_the_windows_where_they_were() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    let before = m.placed(MAIN);
    m.ok(|l| l.configure_output(MAIN, 3840, 2160, 2.0));
    assert_rect(
        m.layout.output_rect(MAIN).expect("the output exists"),
        rect(0.0, 0.0, 1920.0, 1080.0),
    );
    assert_eq!(
        m.layout
            .output_scale(MAIN)
            .expect("the output exists")
            .get(),
        2.0
    );
    assert_eq!(m.placed(MAIN), before);
    let layout = format!("{:?}", m.layout);
    m.ok(|l| l.configure_output(MAIN, 3840, 2160, 2.0));
    assert_eq!(format!("{:?}", m.layout), layout);
}

/// The seat's outputs right of the one changed move with its right edge, their windows with
/// them; another seat's output never moves, though it lies there too.
#[test]
fn the_outputs_on_its_right_move_with_its_right_edge_and_no_other_seats() {
    const AGENTS_OUTPUT: yuna_types::OutputId = yuna_types::OutputId(3);
    let mut m = two_outputs();
    m.ok(|l| l.focus_workspace(OWNER, "c"));
    m.open(2);
    m.ok(|l| l.add_seat(AGENT));
    m.ok(|l| l.add_output(AGENT, AGENTS_OUTPUT, output(1920.0, 1920, 1080, 1.0)));
    assert_eq!(place(&m, 2), ((1936.0, 16.0, 616.0, 768.0), (616.0, 768.0)));

    m.ok(|l| l.configure_output(MAIN, 2560, 1440, 1.0));
    assert_rect(
        m.layout.output_rect(SIDE).expect("the output exists"),
        rect(2560.0, 0.0, 1280.0, 800.0),
    );
    assert_eq!(place(&m, 2), ((2576.0, 16.0, 616.0, 768.0), (616.0, 768.0)));
    assert_rect(
        m.layout
            .output_rect(AGENTS_OUTPUT)
            .expect("the output exists"),
        rect(1920.0, 0.0, 1920.0, 1080.0),
    );

    // Smaller again: it comes back, keeping its distance.
    m.ok(|l| l.configure_output(MAIN, 1280, 720, 1.0));
    assert_rect(
        m.layout.output_rect(SIDE).expect("the output exists"),
        rect(1280.0, 0.0, 1280.0, 800.0),
    );
    assert_eq!(place(&m, 2), ((1296.0, 16.0, 616.0, 768.0), (616.0, 768.0)));

    // The output on the right changed: nothing on its left moves.
    m.ok(|l| l.configure_output(SIDE, 2560, 1600, 1.0));
    assert_rect(
        m.layout.output_rect(MAIN).expect("the output exists"),
        rect(0.0, 0.0, 1280.0, 720.0),
    );
    assert_rect(
        m.layout.output_rect(SIDE).expect("the output exists"),
        rect(1280.0, 0.0, 2560.0, 1600.0),
    );
}

/// A mode the model cannot take is refused, and nothing changes: an unknown output, a scale or
/// a size out of range, and a move that would take an output on its right past the
/// coordinates' range.
#[test]
fn a_mode_out_of_range_is_refused_and_changes_nothing() {
    let mut m = two_outputs();
    let unknown = yuna_types::OutputId(9);
    m.refused(LayoutError::UnknownOutput(unknown), |l| {
        l.configure_output(unknown, 1280, 720, 1.0)
    });
    for scale in [0.0, -1.0, 0.1, 11.0, f64::NAN, f64::INFINITY] {
        m.refused(LayoutError::InvalidValue, |l| {
            l.configure_output(MAIN, 1920, 1080, scale)
        });
    }
    m.refused(LayoutError::InvalidValue, |l| {
        l.configure_output(MAIN, 0, 1080, 1.0)
    });
    // 30,000 pixels at 0.25 are 120,000 units, past the widest size.
    m.refused(LayoutError::InvalidValue, |l| {
        l.configure_output(MAIN, 30_000, 1080, 0.25)
    });

    let mut far = Model::bare(Options::default());
    far.ok(|l| l.add_seat(OWNER));
    far.ok(|l| l.add_output(OWNER, MAIN, output(0.0, 1920, 1080, 1.0)));
    far.ok(|l| l.add_output(OWNER, SIDE, output(999_000.0, 1920, 1080, 1.0)));
    far.refused(LayoutError::InvalidValue, |l| {
        l.configure_output(MAIN, 3840, 1080, 1.0)
    });
}
