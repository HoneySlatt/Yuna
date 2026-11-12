//! Fullscreen, fake fullscreen, and how they meet columns and floating windows (niri).

use crate::harness::*;
use yuna_layout::{ColumnWidth, Direction, OpenOptions, Options};

const OUTPUT: yuna_layout::geometry::Rect =
    yuna_layout::geometry::Rect::new(0.0, 0.0, 1920.0, 1080.0);

/// niri: docs/wiki/Fullscreen-and-Maximize.md:32-47.
#[test]
fn a_fullscreen_window_covers_the_whole_output_and_is_told_so() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    let p = m.placement(1);
    assert_rect(p.rect, OUTPUT);
    assert!(p.fullscreen);
    assert!(m.info(1).fullscreen);
}

/// niri: src/layout/tests/fullscreen.rs:464 (the view goes -16, 116, -16).
#[test]
fn leaving_fullscreen_puts_the_view_back_where_it_was() {
    let mut m = Model::with(Options {
        default_width: ColumnWidth::Fixed(100.0),
        ..Options::default()
    });
    m.open(1);
    m.open(2);
    assert_eq!((m.rect(1).x, m.rect(2).x), (16.0, 132.0));
    m.ok(|l| l.set_fullscreen(w(2), true));
    // The view sits on window 2: window 1 is 100 + 16 to its left.
    assert_eq!((m.rect(1).x, m.rect(2).x), (-116.0, 0.0));
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert_eq!((m.rect(1).x, m.rect(2).x), (16.0, 132.0));
}

/// niri: src/layout/scrolling.rs:2841.
#[test]
fn a_window_sharing_its_column_leaves_it_to_go_fullscreen_and_keeps_focus() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    m.ok(|l| l.set_fullscreen(w(2), true));
    assert_eq!(m.focused(), Some(2));
    assert_rect(m.rect(2), OUTPUT);
    let list = &m.layout.workspaces(MAIN).unwrap()[0].windows;
    assert_eq!(list, &[w(1), w(2)]);
    // Window 1 has its column to itself again, out of view on the left.
    assert_eq!(m.rect(1).h, 1048.0);
    assert!(m.rect(1).right() <= 0.0);
}

/// niri: docs/wiki/Fullscreen-and-Maximize.md:49-57, docs/wiki/Development:-Design-Principles.md:65-73.
#[test]
fn a_fullscreen_column_scrolls_away_like_any_other_and_stays_fullscreen() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.open_with(
        OWNER,
        9,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.set_fullscreen(w(2), true));
    // Focused fullscreen: the floating window hides.
    assert_eq!(m.shown(MAIN), vec![1, 2]);
    m.ok(|l| l.focus(OWNER, Direction::Left));
    assert_eq!(m.focused(), Some(1));
    assert!(m.info(2).fullscreen);
    assert_rect(m.rect(1), rect(16.0, 16.0, 936.0, 1048.0));
    // Focus left the fullscreen column: the floating window shows again.
    assert_eq!(m.shown(MAIN), vec![1, 2, 9]);
}

/// niri: src/layout/workspace.rs:616 (code).
#[test]
fn a_new_window_does_not_take_the_focus_from_a_fullscreen_one() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.open(2);
    assert_eq!(m.focused(), Some(1));
    assert_rect(m.rect(1), OUTPUT);
}

/// niri: docs/wiki/Fullscreen-and-Maximize.md:67-77, src/layout/tests/fullscreen.rs:205.
#[test]
fn fake_fullscreen_tells_the_window_without_moving_it() {
    let mut m = Model::new();
    m.open(1);
    let before = m.rect(1);
    m.ok(|l| l.set_fake_fullscreen(w(1), true));
    assert!(m.placement(1).fullscreen);
    assert!(!m.info(1).fullscreen);
    assert_rect(m.rect(1), before);
    m.ok(|l| l.set_fake_fullscreen(w(1), false));
    assert!(!m.placement(1).fullscreen);
}

/// niri: src/layout/mod.rs:3497 and 3543 (code), src/layout/tests/fullscreen.rs:205 and 220.
#[test]
fn fullscreen_and_fake_fullscreen_exclude_each_other() {
    let mut m = Model::new();
    m.open(1);
    let before = m.rect(1);
    // Fake fullscreen on a fullscreen window: it leaves real fullscreen.
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.ok(|l| l.set_fake_fullscreen(w(1), true));
    assert!(!m.info(1).fullscreen && m.info(1).fake_fullscreen);
    assert_rect(m.rect(1), before);
    // Real fullscreen on a fake one: it stops being fake.
    m.ok(|l| l.set_fullscreen(w(1), true));
    assert!(m.info(1).fullscreen && !m.info(1).fake_fullscreen);
    // A client leaving fullscreen while fake: it is told it is not fullscreen any more.
    m.ok(|l| l.set_fake_fullscreen(w(1), true));
    m.ok(|l| l.set_fullscreen(w(1), false));
    assert!(!m.placement(1).fullscreen);
}

/// niri: src/layout/scrolling.rs:4305-4308 (code).
#[test]
fn a_window_joining_a_fullscreen_column_ends_its_fullscreen() {
    let mut m = Model::new();
    m.open(1);
    m.open(2);
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.ok(|l| l.consume_or_expel(OWNER, Direction::Left));
    assert!(!m.info(1).fullscreen);
    assert_eq!(m.rect(1).x, m.rect(2).x);
}

/// niri: src/layout/floating.rs:1354, docs/wiki/Fullscreen-and-Maximize.md:51-53,
/// src/layout/tests.rs:3525.
#[test]
fn a_floating_window_goes_fullscreen_through_the_tiles_and_floats_back_where_it_was() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.set_floating_rect(w(2), rect(100.0, 50.0, 640.0, 480.0)));
    m.ok(|l| l.set_fullscreen(w(2), true));
    assert!(!m.info(2).floating);
    assert_rect(m.rect(2), OUTPUT);
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert!(m.info(2).floating);
    assert_rect(m.rect(2), rect(100.0, 50.0, 640.0, 480.0));
    assert_eq!(m.focused(), Some(2));
}

/// Yuna's rule (ADR-0019): a window floats back only when it leaves fullscreen itself; fullscreen
/// ended another way leaves it tiled.
#[test]
fn a_floating_window_floats_back_only_when_it_leaves_fullscreen_itself() {
    let mut m = Model::with(Options {
        default_tiling: yuna_layout::Tiling::Dwindle,
        ..Options::default()
    });
    m.open(1);
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.set_fullscreen(w(2), true));
    m.ok(|l| l.focus_window(w(1)));
    assert!(!m.info(2).fullscreen && !m.info(2).floating);
    // Now tiled, a fullscreen round trip keeps it tiled (niri: workspace.rs:1303-1310).
    m.ok(|l| l.set_fullscreen(w(2), true));
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert!(!m.info(2).floating);
}

/// niri: src/layout/workspace.rs:611 (a window opened floating and fullscreen floats after).
#[test]
fn a_window_opened_floating_and_fullscreen_floats_on_leaving_fullscreen() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            floating: true,
            fullscreen: true,
            ..OpenOptions::default()
        },
    );
    assert!(m.info(2).fullscreen);
    m.ok(|l| l.set_fullscreen(w(2), false));
    assert!(m.info(2).floating);
}

/// niri: src/layout/scrolling.rs:48-53 (the view kept relative to its column).
#[test]
fn leaving_fullscreen_after_a_column_on_the_left_closed_shows_the_columns_as_before() {
    let mut m = Model::new();
    for n in 1..=3 {
        m.open(n);
    }
    m.ok(|l| l.set_fullscreen(w(3), true));
    m.ok(|l| l.close_window(w(1)));
    m.ok(|l| l.set_fullscreen(w(3), false));
    assert_eq!((m.rect(2).x, m.rect(3).x), (16.0, 968.0));
}

/// Found by the property tests: on an output at scale 1.25, a fullscreen window that committed
/// 1 × 1 is centred on a half pixel; snapping both its edges rounded its width away. It keeps one
/// physical pixel, 0.8 units, its corner snapped alone.
#[test]
fn a_fullscreen_window_of_one_pixel_centred_on_a_half_pixel_keeps_its_pixel() {
    let mut m = Model::bare(Options::default());
    m.ok(|l| l.add_seat(OWNER));
    m.ok(|l| l.add_output(OWNER, MAIN, output(60.0, 3840, 2160, 1.25)));
    let tiny = OpenOptions {
        fullscreen: true,
        committed: Some(yuna_layout::geometry::Size { w: 1.0, h: 1.0 }),
        ..OpenOptions::default()
    };
    m.open_with(OWNER, 1, tiny);
    // 60 + (3072 − 0.8) / 2 = 1595.6, 1994.5 pixels: snapped to 1995, 1596 units.
    assert_rect(m.rect(1), rect(1596.0, 864.0, 0.8, 0.8));
}

/// A dialog of window `parent`.
fn dialog(parent: u64) -> OpenOptions {
    OpenOptions {
        parent: Some(w(parent)),
        ..OpenOptions::default()
    }
}

/// Yuna's rule (games first, ADR-0026): a fullscreen game's dialog closing gives the focus back
/// to the game, though another floating window remains; niri gives it to the topmost floating
/// window (src/layout/floating.rs:524-529), which would show over the game.
#[test]
fn a_fullscreen_games_dialog_closing_gives_the_focus_back_to_the_game() {
    let mut m = Model::new();
    m.open(1);
    m.open_with(
        OWNER,
        2,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.focus_window(w(1)));
    m.ok(|l| l.set_fullscreen(w(1), true));
    m.open_with(OWNER, 3, dialog(1));
    assert_eq!(m.focused(), Some(3));
    m.ok(|l| l.close_window(w(3)));
    assert_eq!(m.focused(), Some(1));
    assert_eq!(
        m.shown(MAIN),
        vec![1],
        "the other floating window hides again"
    );
}

/// Yuna's rule (games first, ADR-0026): while a fullscreen game holds the focus, its own
/// activation and its dialogs' are granted. xdg-shell: a closed dialog's dialogs become its
/// parent's, so they stay the game's.
#[test]
fn activation_over_a_fullscreen_game_is_granted_to_the_game_and_its_dialogs() {
    let mut m = Model::new();
    m.open(1);
    m.ok(|l| l.set_fullscreen(w(1), true));
    assert_eq!(m.layout.request_activation(w(1)), Ok(true));
    m.open_with(OWNER, 2, dialog(1));
    m.open_with(OWNER, 3, dialog(2));
    assert_eq!(m.focused(), Some(3), "its parent had the focus");
    m.ok(|l| l.close_window(w(2)));
    m.ok(|l| l.focus_window(w(1)));
    assert_eq!(m.layout.request_activation(w(3)), Ok(true));
    assert_eq!(m.focused(), Some(3));
}

/// ADR-0019's departure, games first: in dwindle, while a fullscreen game's own dialog, or a
/// floating window over it, has the seat's focus, the game still holds the screen: no other
/// window goes fullscreen or maximised there, and one opening fullscreen opens behind it.
#[test]
fn a_fullscreen_game_holds_the_screen_while_a_window_over_it_has_the_focus() {
    let mut m = Model::with(Options {
        default_tiling: yuna_layout::Tiling::Dwindle,
        ..Options::default()
    });
    m.open(1);
    m.open(2);
    m.open_with(
        OWNER,
        3,
        OpenOptions {
            floating: true,
            ..OpenOptions::default()
        },
    );
    m.ok(|l| l.focus_window(w(1)));
    m.ok(|l| l.set_fullscreen(w(1), true));
    let held = yuna_layout::LayoutError::Refused("another window's fullscreen holds the screen");
    // Its own dialog has the focus.
    m.open_with(OWNER, 4, dialog(1));
    assert_eq!(m.focused(), Some(4));
    m.refused(held.clone(), |l| l.set_fullscreen(w(2), true));
    m.refused(held.clone(), |l| l.set_full_width(w(2), true));
    m.refused(held.clone(), |l| l.set_fullscreen(w(3), true));
    m.open_with(
        OWNER,
        5,
        OpenOptions {
            fullscreen: true,
            ..OpenOptions::default()
        },
    );
    assert!(!m.info(5).fullscreen);
    m.ok(|l| l.close_window(w(4)));
    // Another floating window over it has the focus.
    m.ok(|l| l.switch_focus_floating(OWNER));
    assert_eq!(m.focused(), Some(3));
    m.refused(held.clone(), |l| l.set_fullscreen(w(2), true));
    m.refused(held, |l| l.set_full_width(w(2), true));
    assert!(m.info(1).fullscreen);
}

/// ADR-0026, games first: while a fullscreen game's dialog has the focus, the game still holds
/// its screen: another window's activation request is refused, in both tilings, while the game's
/// own and its dialog's are granted.
#[test]
fn activation_is_refused_while_a_fullscreen_games_dialog_has_the_focus() {
    for tiling in [yuna_layout::Tiling::Scrolling, yuna_layout::Tiling::Dwindle] {
        let mut m = Model::with(Options {
            default_tiling: tiling,
            ..Options::default()
        });
        m.open(1);
        m.open(2);
        m.ok(|l| l.set_fullscreen(w(2), true));
        m.open_with(OWNER, 3, dialog(2));
        assert_eq!(m.focused(), Some(3));
        let before = format!("{:?}", m.layout);
        assert_eq!(m.layout.request_activation(w(1)), Ok(false), "{tiling:?}");
        assert_eq!(before, format!("{:?}", m.layout));
        assert_eq!(m.layout.request_activation(w(3)), Ok(true));
        // A dialog of its dialog too: a file chooser opened from a game's settings.
        m.open_with(OWNER, 4, dialog(3));
        m.ok(|l| l.focus_window(w(2)));
        assert_eq!(m.layout.request_activation(w(4)), Ok(true), "{tiling:?}");
        assert_eq!(m.layout.request_activation(w(2)), Ok(true));
        assert!(m.info(2).fullscreen);
        assert_eq!(m.focused(), Some(2));
    }
}
