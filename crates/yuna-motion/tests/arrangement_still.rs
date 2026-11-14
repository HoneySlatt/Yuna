//! What an operation leaves in place on screen does not move, whatever the springs of the camera
//! and of the windows; and a few extreme cases: times at the end of `Duration`, a switch with the
//! special workspace toggled, a jump across a hundred columns, and the compositor's cap of 10,000
//! windows on one strip ([ADR-0052](../../../docs/adr/0052-motion-in-a-pure-crate.md)).

use std::time::Duration;

use yuna_layout::geometry::{Point, Rect};
use yuna_layout::{
    Direction, Layout, LayoutError, OpenOptions, Options, OutputConfig, OutputId, SeatId, WindowId,
};
use yuna_motion::{Change, Frame, Motion, Settings, SpringParams};

const SEAT: SeatId = SeatId(1);
const OUTPUT: OutputId = OutputId(1);

fn w(n: u64) -> WindowId {
    WindowId(n)
}
fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn distinct() -> Settings {
    Settings {
        camera: SpringParams::new(1.0, 300.0).unwrap(),
        windows: SpringParams::new(1.0, 1500.0).unwrap(),
        ..Settings::default()
    }
}

struct Rig {
    layout: Layout,
    motion: Motion,
    now: Duration,
}

impl Rig {
    fn new(options: Options, settings: Settings) -> Rig {
        let mut layout = Layout::new(options).unwrap();
        layout.add_seat(SEAT).unwrap();
        let config = OutputConfig {
            position: Point { x: 0.0, y: 0.0 },
            physical_width: 1920,
            physical_height: 1080,
            scale: 1.0,
        };
        layout.add_output(SEAT, OUTPUT, config).unwrap();
        let mut rig = Rig {
            layout,
            motion: Motion::new(settings),
            now: Duration::from_secs(10),
        };
        rig.motion.update(&rig.layout, rig.now, Change::Instant);
        rig
    }
    fn op(&mut self, op: impl FnOnce(&mut Layout) -> Result<(), LayoutError>) {
        op(&mut self.layout).unwrap();
    }
    fn open(&mut self, n: u64) {
        self.op(|l| l.open_window(SEAT, w(n), OpenOptions::default()));
    }
    fn instant(&mut self) {
        self.motion.update(&self.layout, self.now, Change::Instant);
    }
    fn animated(&mut self) {
        self.motion.update(&self.layout, self.now, Change::Animated);
    }
    fn frame(&self, at: Duration) -> Frame {
        self.motion.frame(OUTPUT, at)
    }
    fn rect(&self, n: u64, at: Duration) -> Option<Rect> {
        self.frame(at)
            .windows
            .into_iter()
            .find(|d| d.window == w(n))
            .map(|d| d.rect)
    }
    fn placed(&self, n: u64) -> Option<Rect> {
        self.layout
            .arrange(OUTPUT)
            .unwrap()
            .into_iter()
            .find(|p| p.window == w(n))
            .map(|p| p.rect)
    }

    /// Applies `op` animated; for every window whose drawn rect at rest before equals its place
    /// after (its screen target is unchanged), returns the largest distance its drawn corner
    /// strays from that place while the motion runs.
    fn stray_of_still_windows(
        &mut self,
        op: impl FnOnce(&mut Layout) -> Result<(), LayoutError>,
        windows: &[u64],
    ) -> Vec<(u64, f64)> {
        let before: Vec<_> = windows
            .iter()
            .map(|&n| (n, self.rect(n, self.now)))
            .collect();
        self.op(op);
        self.animated();
        let end = self.motion.end(OUTPUT, self.now).unwrap_or(self.now);
        let mut out = Vec::new();
        for (n, was) in before {
            let (Some(was), Some(place)) = (was, self.placed(n)) else {
                continue;
            };
            if (was.x, was.y) != (place.x, place.y) {
                continue;
            }
            let mut worst: f64 = 0.0;
            let mut at = self.now;
            while at <= end {
                if let Some(r) = self.rect(n, at) {
                    worst = worst.max((r.x - place.x).abs()).max((r.y - place.y).abs());
                }
                at += ms(1);
            }
            out.push((n, worst));
        }
        out
    }
}

fn strip(columns: u64, settings: Settings) -> Rig {
    let mut rig = Rig::new(Options::default(), settings);
    for n in 1..=columns {
        rig.open(n);
    }
    rig.instant();
    rig
}

/// Moving the focused column one step right, the view not following (it stays in view): the
/// columns that do not move in the strip must not move on screen.
#[test]
fn moving_a_column_moves_no_other_column_on_screen() {
    let mut rig = strip(4, distinct());
    rig.op(|l| l.focus_window(w(2)));
    rig.instant();
    let view_before = rig.layout.shown_workspaces(OUTPUT).unwrap().main.view_x;
    rig.now += ms(5);
    let still = rig.stray_of_still_windows(|l| l.move_focused(SEAT, Direction::Right), &[1, 4]);
    let view_after = rig.layout.shown_workspaces(OUTPUT).unwrap().main.view_x;
    assert_eq!(
        view_before, view_after,
        "the view stays: only the column moves"
    );
    for (n, stray) in still {
        assert!(
            stray < 1.0,
            "window {n} strays {stray} units though its place is unchanged"
        );
    }
}

/// The same with equal springs: shows the stray comes from the split, not from a jump.
#[test]
fn moving_a_column_with_equal_springs() {
    let mut rig = strip(4, Settings::default());
    rig.op(|l| l.focus_window(w(2)));
    rig.instant();
    rig.now += ms(5);
    let still = rig.stray_of_still_windows(|l| l.move_focused(SEAT, Direction::Right), &[1, 4]);
    for (n, stray) in still {
        assert!(stray < 1.0, "window {n} strays {stray}");
    }
}

/// Consume into the left column: the columns left of it stay still.
#[test]
fn consume_moves_no_still_column() {
    let mut rig = strip(4, distinct());
    rig.op(|l| l.focus_window(w(3)));
    rig.instant();
    rig.now += ms(5);
    let still =
        rig.stray_of_still_windows(|l| l.consume_or_expel(SEAT, Direction::Left), &[1, 2, 4]);
    for (n, stray) in still {
        assert!(stray < 1.0, "window {n} strays {stray}");
    }
}

/// Expel to the right: the columns that keep their screen place stay still.
#[test]
fn expel_moves_no_still_column() {
    let mut rig = strip(4, distinct());
    rig.op(|l| l.focus_window(w(3)));
    rig.op(|l| l.consume_or_expel(SEAT, Direction::Left));
    rig.instant();
    rig.now += ms(5);
    let still =
        rig.stray_of_still_windows(|l| l.consume_or_expel(SEAT, Direction::Right), &[1, 2, 4]);
    for (n, stray) in still {
        assert!(stray < 1.0, "window {n} strays {stray}");
    }
}

/// Moving the focused column left, from the right end.
#[test]
fn moving_a_column_left_moves_no_other_column_on_screen() {
    let mut rig = strip(3, distinct());
    rig.op(|l| l.focus_window(w(3)));
    rig.instant();
    rig.now += ms(5);
    let still = rig.stray_of_still_windows(|l| l.move_focused(SEAT, Direction::Left), &[1]);
    for (n, stray) in still {
        assert!(stray < 1.0, "window {n} strays {stray}");
    }
}

/// Times and settings at the extremes never panic nor give a non-finite rect.
#[test]
fn extreme_times() {
    let mut rig = strip(3, Settings::default());
    rig.now = Duration::MAX - ms(1);
    rig.op(|l| l.focus_window(w(1)));
    rig.animated();
    for at in [Duration::ZERO, rig.now, Duration::MAX] {
        for d in rig.frame(at).windows {
            assert!(
                [d.rect.x, d.rect.y, d.rect.w, d.rect.h]
                    .iter()
                    .all(|v| v.is_finite())
            );
        }
        let _ = rig.motion.end(OUTPUT, at);
    }
}

/// A switch running, the special workspace toggled in the middle: nothing jumps, and every
/// window of the workspace sliding out stays told until the end.
#[test]
fn switch_with_special_toggled_and_told() {
    let mut rig = Rig::new(Options::default(), Settings::default());
    for name in ["1", "2"] {
        rig.op(|l| l.add_workspace(SEAT, name, Some(OUTPUT)));
    }
    rig.op(|l| l.focus_workspace(SEAT, "1"));
    rig.open(1);
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.open(2);
    rig.op(|l| l.focus_workspace(SEAT, "1"));
    rig.op(|l| l.move_to_special(SEAT, "s").map(|_| ()));
    // window 1 is now on the special; open another on "1"
    rig.open(3);
    rig.instant();
    rig.now += ms(5);
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.animated();
    let switch_end = rig.motion.end(OUTPUT, rig.now).unwrap();
    rig.now += ms(40);
    let told_mid = rig.motion.told_visible(OUTPUT, rig.now);
    assert!(
        told_mid.contains(&w(3)),
        "the window sliding out is told until the end"
    );
    let before = rig.frame(rig.now);
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.animated();
    let after = rig.frame(rig.now);
    for d in &after.windows {
        if let Some(b) = before.windows.iter().find(|b| b.window == d.window) {
            assert_eq!(
                (b.rect.x, b.rect.y),
                (d.rect.x, d.rect.y),
                "{:?} jumps",
                d.window
            );
        }
    }
    assert!(rig.motion.end(OUTPUT, rig.now).unwrap() >= switch_end);
    let told = rig.motion.told_visible(OUTPUT, rig.now);
    assert!(told.contains(&w(3)));
    assert!(told.contains(&w(1)));
}

/// 100 windows on one strip, focus jumping end to end: drawn values finite and the frame at the
/// end equals the arrangement. Not timed: only counts frame lengths.
#[test]
fn a_hundred_columns() {
    let mut rig = strip(100, Settings::default());
    rig.op(|l| l.focus_window(w(1)));
    rig.instant();
    rig.now += ms(5);
    rig.op(|l| l.focus_window(w(100)));
    rig.animated();
    let end = rig.motion.end(OUTPUT, rig.now).unwrap();
    let mut told_max = 0;
    let mut at = rig.now;
    while at < end {
        told_max = told_max.max(rig.motion.told_visible(OUTPUT, at).len());
        assert_eq!(rig.frame(at).windows.len(), 100);
        at += ms(4);
    }
    assert!(told_max <= 4, "fly-bys told: {told_max}");
}

/// A strip of `windows` windows, at rest: window 1 at its left end, then the others, the newest
/// first; the focus on window 1. Each window opens beside window 1 and the focus goes back to it,
/// so that each opening costs the same however long the strip: the layout places the focused
/// column from the widths of those on its left.
fn long_strip(windows: u64) -> Rig {
    let mut rig = Rig::new(Options::default(), Settings::default());
    rig.open(1);
    for n in 2..=windows {
        rig.open(n);
        rig.op(|l| l.focus(SEAT, Direction::Left));
    }
    rig.instant();
    rig
}

/// The compositor's cap on windows, 10,000 (`yuna::MAX_WINDOWS`), on one strip: a column left of
/// the focused one widens, which moves the strip under the view, then the focus jumps to the
/// other end. Each update finds the camera's anchor among every column through an index of
/// their left edges, so that it stays linear in the windows. Not timed, so that a slow runner
/// cannot fail it: the test's own time shows it.
#[test]
fn ten_thousand_columns() {
    const WINDOWS: u64 = 10_000;
    let mut rig = long_strip(WINDOWS);
    // The newest window, second on the strip, takes the focus; window 1 is on its left.
    rig.op(|l| l.focus(SEAT, Direction::Right));
    rig.instant();
    let focused = rig.placed(WINDOWS).expect("the focused window is shown");
    rig.now += ms(5);
    let view_before = rig
        .layout
        .shown_workspaces(OUTPUT)
        .expect("a known output")
        .main
        .view_x;
    rig.op(|l| l.change_width(w(1), 0.2));
    rig.animated();
    let view_after = rig
        .layout
        .shown_workspaces(OUTPUT)
        .expect("a known output")
        .main
        .view_x;
    assert_ne!(view_before, view_after, "the strip moved under the view");
    let end = rig
        .motion
        .end(OUTPUT, rig.now)
        .expect("the widened column moves");
    let mut at = rig.now;
    while at <= end {
        assert_eq!(
            rig.rect(WINDOWS, at),
            Some(focused),
            "the focused column, the anchor, stays where it is on screen at {at:?}"
        );
        at += ms(16);
    }

    // Window 2, the oldest but one, is at the strip's right end.
    rig.now = end;
    rig.op(|l| l.focus_window(w(2)));
    rig.animated();
    let end = rig
        .motion
        .end(OUTPUT, rig.now)
        .expect("the camera crosses the strip");
    let mid = rig.frame(rig.now + (end - rig.now) / 2);
    assert_eq!(mid.windows.len(), WINDOWS as usize);
    assert!(mid.windows.iter().all(|d| d.rect.x.is_finite()));
    let at_rest: Vec<(WindowId, Rect)> = rig
        .frame(end)
        .windows
        .into_iter()
        .map(|d| (d.window, d.rect))
        .collect();
    let arranged: Vec<(WindowId, Rect)> = rig
        .layout
        .arrange(OUTPUT)
        .expect("a known output")
        .into_iter()
        .map(|p| (p.window, p.rect))
        .collect();
    assert_eq!(at_rest, arranged);
}

/// Reduced motion: nothing crosses the screen, the camera and the switch show their result at
/// once; the special workspace's fade stays, over half its duration
/// ([ADR-0059](../../../docs/adr/0059-reduced-motion-nothing-slides-fades-stay.md)).
#[test]
fn reduced_motion_slides_nothing_and_keeps_fades() {
    let settings = Settings {
        reduced_motion: true,
        ..distinct()
    };
    let mut rig = strip(4, settings);
    rig.now += ms(5);
    rig.op(|l| l.focus(SEAT, Direction::Left));
    rig.animated();
    assert_eq!(rig.motion.end(OUTPUT, rig.now), None);
    for n in 1..=4 {
        assert_eq!(rig.rect(n, rig.now), rig.placed(n));
    }
    rig.op(|l| l.add_workspace(SEAT, "2", Some(OUTPUT)));
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.animated();
    assert_eq!(rig.motion.end(OUTPUT, rig.now), None);
    assert!(rig.frame(rig.now).windows.is_empty());
    rig.op(|l| l.focus_workspace_relative(SEAT, Direction::Up));
    rig.op(|l| l.move_to_special(SEAT, "s"));
    rig.instant();
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.animated();
    let end = rig.motion.end(OUTPUT, rig.now).expect("the fade runs");
    // Half of the default fade's 200 ms.
    assert_eq!(end - rig.now, ms(100));
    let mid = rig.frame(rig.now + ms(50));
    let faded = mid
        .windows
        .iter()
        .find(|d| d.alpha < 1.0)
        .expect("a window fades");
    assert!(faded.alpha > 0.0);
}
