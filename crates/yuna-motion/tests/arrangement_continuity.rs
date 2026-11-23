//! More scenarios of the motion of an arrangement, for what the first ones leave open: a motion
//! retargeted while it runs keeps its speed on both axes, the special workspace's fade turns from
//! the opacity it had and ends when its time is up, a switch turned early drops the farther
//! workspace, and the end of a motion is read at the output's own scale.
//!
//! One output of 1920 × 1080 logical units, at the scale each scenario names. Times start at 10 s.

use std::time::Duration;

use yuna_layout::{Layout, LayoutError, OpenOptions, Options, OutputConfig};
use yuna_motion::{Change, Curve, Drawn, Motion, Settings, Spring};
use yuna_types::{OutputId, Point, Rect, SeatId, Size, Tiling, WindowId};

const SEAT: SeatId = SeatId(1);
const OUTPUT: OutputId = OutputId(1);

fn w(n: u64) -> WindowId {
    WindowId(n)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A layout with one seat and one output, its motion, and a clock.
struct Rig {
    layout: Layout,
    motion: Motion,
    now: Duration,
}

impl Rig {
    fn new(options: Options, settings: Settings, scale: f64) -> Rig {
        let mut layout = Layout::new(options).unwrap();
        layout.add_seat(SEAT).unwrap();
        let config = OutputConfig {
            position: Point { x: 0.0, y: 0.0 },
            physical_width: (1920.0 * scale) as u32,
            physical_height: (1080.0 * scale) as u32,
            scale,
        };
        layout.add_output(SEAT, OUTPUT, config).unwrap();
        let mut rig = Rig {
            layout,
            motion: Motion::new(settings),
            now: Duration::from_secs(10),
        };
        rig.instant();
        rig
    }

    fn scrolling(settings: Settings) -> Rig {
        Rig::new(Options::default(), settings, 1.0)
    }

    #[track_caller]
    fn op(&mut self, op: impl FnOnce(&mut Layout) -> Result<(), LayoutError>) {
        op(&mut self.layout).unwrap();
    }

    #[track_caller]
    fn open(&mut self, n: u64) {
        self.op(|l| l.open_window(SEAT, w(n), OpenOptions::default()));
    }

    #[track_caller]
    fn open_floating(&mut self, n: u64) {
        let options = OpenOptions {
            floating: true,
            committed: Some(Size { w: 400.0, h: 300.0 }),
            ..OpenOptions::default()
        };
        self.op(|l| l.open_window(SEAT, w(n), options));
    }

    fn animated(&mut self) {
        self.motion.update(&self.layout, self.now, Change::Animated);
    }

    fn instant(&mut self) {
        self.motion.update(&self.layout, self.now, Change::Instant);
    }

    fn wait(&mut self, time: Duration) {
        self.now += time;
    }

    fn find(&self, n: u64, at: Duration) -> Option<Drawn> {
        self.motion
            .frame(OUTPUT, at)
            .windows
            .into_iter()
            .find(|d| d.window == w(n))
    }

    #[track_caller]
    fn drawn(&self, n: u64, at: Duration) -> Drawn {
        self.find(n, at)
            .unwrap_or_else(|| panic!("window {n} is not drawn at {at:?}"))
    }
}

/// How fast window `n` is drawn moving along each axis, in units per second, over the 2 ms before
/// and after `at`, the time of an update: before it by the motion as it was, after it by the
/// motion the update gave.
fn speeds_around(before: &Motion, after: &Motion, n: u64, at: Duration) -> [(f64, f64); 2] {
    let drawn = |motion: &Motion, at: Duration| {
        motion
            .frame(OUTPUT, at)
            .windows
            .into_iter()
            .find(|d| d.window == w(n))
            .unwrap_or_else(|| panic!("window {n} is not drawn at {at:?}"))
            .rect
    };
    let speed = |from: Rect, to: Rect| ((to.x - from.x) / 0.002, (to.y - from.y) / 0.002);
    let (early, now, late) = (
        drawn(before, at - ms(2)),
        drawn(before, at),
        drawn(after, at + ms(2)),
    );
    assert_eq!(now, drawn(after, at), "the update moved window {n}");
    [speed(early, now), speed(now, late)]
}

/// The most a speed may change across a retarget and still be the same motion, in units per second:
/// the new pull on a spring of stiffness 800 over the 1,000 units a window may be from its target,
/// for the 2 ms a speed is measured over, plus a whole unit of rounding to the grid on each of the
/// two positions a speed is read from, over those 2 ms. A speed lost or reversed changes by the
/// speed itself, thousands of units per second.
const SPEED_CHANGE_BOUND: f64 = 800.0 * 1000.0 * 0.002 + 2.0 * 1.0 / 0.002;

/// Asserts a window goes on at the speed it had when an update retargets its motion, to within
/// [`SPEED_CHANGE_BOUND`].
#[track_caller]
fn assert_speed_kept(speeds: [(f64, f64); 2], axis: usize, at_least: f64) {
    let [(bx, by), (ax, ay)] = speeds;
    let (before, after) = if axis == 0 { (bx, ax) } else { (by, ay) };
    assert!(
        before.abs() > at_least,
        "it moved at only {before} units/s before"
    );
    assert!(
        (after - before).abs() < SPEED_CHANGE_BOUND,
        "its speed went from {before} to {after} units/s"
    );
}

/// A window whose offset is turned mid-way goes on at the speed it had, up or down: the offset on
/// the vertical axis starts with the velocity the window was drawn moving at.
#[test]
fn a_window_moved_again_mid_way_keeps_its_vertical_speed() {
    let mut rig = Rig::scrolling(Settings::default());
    rig.open_floating(1);
    rig.op(|l| l.set_floating_rect(w(1), Rect::new(100.0, 100.0, 400.0, 300.0)));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.set_floating_rect(w(1), Rect::new(100.0, 700.0, 400.0, 300.0)));
    rig.animated();
    rig.wait(ms(30));
    let before = rig.motion.clone();
    rig.op(|l| l.set_floating_rect(w(1), Rect::new(100.0, 100.0, 400.0, 300.0)));
    rig.animated();
    let speeds = speeds_around(&before, &rig.motion, 1, rig.now);
    assert_speed_kept(speeds, 1, 3000.0);
}

/// The same along the strip: columns that change places while the camera runs go on at the speed
/// they were drawn at, which is the camera's, the camera going on at its own.
#[test]
fn columns_changing_places_while_the_camera_runs_keep_their_speed() {
    let mut rig = Rig::scrolling(Settings::default());
    for n in 1..=4 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(1)));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(4)));
    rig.animated();
    rig.wait(ms(40));
    let before = rig.motion.clone();
    // The focused column, 4, trades places with 3.
    rig.op(|l| l.move_focused(SEAT, yuna_types::Direction::Left));
    rig.animated();
    for n in [3, 4] {
        assert_speed_kept(speeds_around(&before, &rig.motion, n, rig.now), 0, 5000.0);
    }
}

/// A window of the special workspace hidden in the same update as a longer motion starts is drawn
/// until its fade ends and no longer: not at no opacity until the other motion ends.
#[test]
fn a_special_workspace_that_has_faded_out_is_no_longer_drawn_while_other_motions_run() {
    let settings = Settings::default();
    let mut rig = Rig::scrolling(settings);
    for n in 1..=3 {
        rig.open(n);
    }
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.open(5);
    rig.instant();
    assert!(rig.find(5, rig.now).is_some());
    rig.wait(ms(5));
    // Focusing a window of the main workspace hides the special one, and scrolls the strip.
    rig.op(|l| l.focus_window(w(1)));
    rig.animated();
    let hidden = rig.now;
    let end = rig.motion.end(OUTPUT, hidden).expect("the strip scrolls");
    assert!(
        end > hidden + ms(300),
        "the strip scrolls for longer than the fade"
    );
    assert!(rig.find(5, hidden + ms(100)).is_some_and(|d| d.alpha > 0.0));
    let fade_ends = Curve::new(settings.special_fade, 1.0, 0.0, hidden).end();
    let after_the_fade = fade_ends + ms(20);
    assert!(rig.find(5, after_the_fade).is_none());
    assert!(rig.find(1, after_the_fade).is_some());
}

/// Hiding the special workspace while it fades in turns the opacity from where it is, not from
/// fully shown.
#[test]
fn hiding_the_special_workspace_mid_fade_goes_on_from_the_opacity_it_had() {
    let settings = Settings::default();
    let mut rig = Rig::scrolling(settings);
    rig.open(1);
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.open(5);
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.animated();
    let shown = rig.now;
    let coming_in = Curve::new(settings.special_fade, 0.0, 1.0, shown);
    rig.wait(ms(60));
    let alpha = rig.drawn(5, rig.now).alpha;
    assert_eq!(alpha, coming_in.value(rig.now));
    assert!(alpha > 0.1 && alpha < 0.9, "it is half-way: {alpha}");
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.animated();
    assert_eq!(rig.drawn(5, rig.now).alpha, alpha);
    let going_out = coming_in.retarget(rig.now, 0.0);
    let later = rig.now + ms(40);
    assert_eq!(rig.drawn(5, later).alpha, going_out.value(later));
}

/// A switch to a workspace, turned early to the one between, drops the farther workspace of the
/// two around it: the one it was sliding in, which is farther from the view than the one it left.
#[test]
fn a_switch_turned_early_to_the_workspace_between_drops_the_one_sliding_in() {
    let mut rig = Rig::scrolling(Settings::default());
    for name in ["1", "2", "3"] {
        rig.op(|l| l.add_workspace(SEAT, name, Some(OUTPUT)));
    }
    for (n, name) in [(1, "1"), (2, "2"), (3, "3")] {
        rig.op(|l| l.focus_workspace(SEAT, name));
        rig.open(n);
    }
    rig.op(|l| l.focus_workspace(SEAT, "1"));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.focus_workspace(SEAT, "3"));
    rig.animated();
    rig.wait(ms(10));
    let turn = rig.now;
    assert!(rig.find(1, turn).is_some() && rig.find(3, turn).is_some());
    let one_before = rig.drawn(1, turn).rect;
    // Workspace 2 goes into the stack between 1, still the nearer, and 3: it takes 3's place.
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.animated();
    assert_eq!(rig.drawn(1, turn).rect, one_before);
    assert!(rig.find(3, turn).is_none(), "workspace 3 left the switch");
    let end = rig.motion.end(OUTPUT, turn).unwrap();
    for at in [turn + ms(20), turn + ms(60), end - ms(1)] {
        assert!(rig.find(3, at).is_none(), "workspace 3 is drawn at {at:?}");
    }
}

/// A motion ends when its springs are within half a physical pixel of rest: on an output at scale
/// 2, a quarter of a logical unit.
#[test]
fn a_motion_ends_within_half_a_physical_pixel_of_rest_at_any_scale() {
    let settings = Settings::default();
    let options = Options {
        default_tiling: Tiling::Dwindle,
        ..Options::default()
    };
    for scale in [2.0, 1.5] {
        let mut rig = Rig::new(options.clone(), settings, scale);
        rig.open(1);
        rig.open(2);
        rig.instant();
        let before = rig.layout.arrange(OUTPUT).unwrap()[1].rect;
        rig.wait(ms(5));
        rig.op(|l| l.close_window(w(1)));
        rig.animated();
        let after = rig.layout.arrange(OUTPUT).unwrap()[0].rect;
        let tolerance = 0.5 / scale;
        let spring =
            |distance: f64| Spring::new(settings.windows, distance, 0.0, 0.0, rig.now, tolerance);
        let end = spring(before.x - after.x)
            .end()
            .max(spring(before.y - after.y).end());
        assert_eq!(
            rig.motion.end(OUTPUT, rig.now),
            Some(end),
            "at scale {scale}"
        );
    }
}

/// Workspaces "1" and "2", "2" holding windows 2 to 5 in four columns, the focus on 2; "1" shown,
/// holding window 1, at rest.
fn two_workspaces(settings: Settings) -> Rig {
    let mut rig = Rig::scrolling(settings);
    for name in ["1", "2"] {
        rig.op(|l| l.add_workspace(SEAT, name, Some(OUTPUT)));
    }
    rig.op(|l| l.focus_workspace(SEAT, "1"));
    rig.open(1);
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    for n in 2..=5 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(2)));
    rig.op(|l| l.focus_workspace(SEAT, "1"));
    rig.instant();
    rig
}

/// A switch is drawn until its slide ends, whatever the shown workspace's camera does after: the
/// camera of a workspace shown does not keep the switch alive, and once the switch is over, every
/// window of the workspace is drawn, those the strip has scrolled out of view too.
#[test]
fn a_switch_ends_with_its_slide_while_the_shown_workspaces_camera_runs_on() {
    let mut rig = two_workspaces(Settings::default());
    rig.wait(ms(5));
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.animated();
    let started = rig.now;
    rig.wait(ms(50));
    // Still sliding in, the shown workspace's strip scrolls to its last column.
    rig.op(|l| l.focus_window(w(5)));
    rig.animated();
    let at = started + ms(350);
    let end = rig
        .motion
        .end(OUTPUT, at)
        .expect("the camera is still running");
    assert!(end > at + ms(30), "it runs until {end:?}, from {started:?}");
    let drawn: Vec<u64> = rig
        .motion
        .frame(OUTPUT, at)
        .windows
        .iter()
        .map(|d| d.window.0)
        .collect();
    assert_eq!(drawn, [2, 3, 4, 5], "the columns out of view are drawn too");
}

/// A strip scrolled again while its workspace slides in goes to its new view on the camera's
/// own spring: the camera the switch kept for it is followed, not put back as it was.
#[test]
fn a_strip_scrolled_while_its_workspace_slides_in_goes_on_to_its_new_view() {
    let settings = distinct_springs();
    let mut rig = two_workspaces(settings);
    rig.wait(ms(5));
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.animated();
    rig.wait(ms(50));
    let view_before = view_x(&rig);
    rig.op(|l| l.focus_window(w(5)));
    rig.animated();
    let camera = Spring::new(
        settings.camera,
        view_before,
        view_x(&rig),
        0.0,
        rig.now,
        0.5,
    );
    let place = rig
        .layout
        .arrange(OUTPUT)
        .unwrap()
        .into_iter()
        .find(|p| p.window == w(5))
        .unwrap()
        .rect;
    for at in [rig.now + ms(100), rig.now + ms(200), rig.now + ms(300)] {
        let lag = camera.target() - camera.value(at);
        assert_eq!(
            rig.drawn(5, at).rect.x,
            (place.x + lag + 0.5).floor(),
            "at {at:?}"
        );
    }
}

/// A camera slower than the windows' springs, so that a strip that moves on the camera and one
/// that moves on its windows' offsets show apart.
fn distinct_springs() -> Settings {
    Settings {
        camera: yuna_motion::SpringParams::new(1.0, 300.0).unwrap(),
        windows: yuna_motion::SpringParams::new(1.0, 1500.0).unwrap(),
        ..Settings::default()
    }
}

/// Asserts that every window of `windows` is drawn in `with` exactly where it is in `without`, at
/// each of `times`, and that both end together: an update that touches none of them leaves their
/// motion as it was.
#[track_caller]
fn assert_same_motion(with: &Rig, without: &Rig, windows: &[u64], times: &[Duration]) {
    for &at in times {
        for &n in windows {
            let (a, b) = (with.find(n, at), without.find(n, at));
            assert_eq!(a.map(|d| d.rect), b.map(|d| d.rect), "window {n} at {at:?}");
        }
    }
    assert!(
        times
            .iter()
            .any(|&at| windows.iter().any(|&n| with.find(n, at).is_some())),
        "nothing was drawn to compare"
    );
}

/// A strip whose camera runs while its workspace slides out keeps that camera through the updates
/// that follow, as long as the switch draws the workspace: another window opening changes nothing
/// of its motion.
#[test]
fn a_workspace_sliding_out_keeps_its_cameras_motion_through_later_updates() {
    let scenario = |interrupted: bool| {
        let mut rig = Rig::scrolling(distinct_springs());
        for n in 11..=14 {
            rig.open(n);
        }
        rig.op(|l| l.focus_window(w(11)));
        rig.op(|l| l.add_workspace(SEAT, "2", Some(OUTPUT)));
        rig.op(|l| l.focus_workspace(SEAT, "2"));
        rig.open(2);
        rig.op(|l| l.focus_workspace_relative(SEAT, yuna_types::Direction::Up));
        rig.instant();
        rig.wait(ms(5));
        // The first workspace's strip scrolls to its last column; before it is done, it slides out.
        rig.op(|l| l.focus_window(w(14)));
        rig.animated();
        rig.wait(ms(20));
        rig.op(|l| l.focus_workspace(SEAT, "2"));
        rig.animated();
        rig.wait(ms(40));
        if interrupted {
            rig.open_floating(3);
            rig.animated();
        }
        rig
    };
    let (with, without) = (scenario(true), scenario(false));
    let times: Vec<Duration> = (0..=30).map(|i| with.now + ms(i * 5)).collect();
    assert_same_motion(&with, &without, &[11, 12, 13, 14], &times);
}

/// The windows of a special workspace fading out keep the camera of their strip through the updates
/// that come while they fade.
#[test]
fn a_special_workspace_fading_out_keeps_its_cameras_motion_through_later_updates() {
    let scenario = |interrupted: bool| {
        let mut rig = Rig::scrolling(distinct_springs());
        rig.open(1);
        rig.op(|l| l.toggle_special(SEAT, "s"));
        for n in 5..=8 {
            rig.open(n);
        }
        rig.op(|l| l.focus_window(w(5)));
        rig.instant();
        rig.wait(ms(5));
        rig.op(|l| l.focus_window(w(8)));
        rig.animated();
        rig.wait(ms(20));
        // The special workspace is hidden while its strip runs.
        rig.op(|l| l.toggle_special(SEAT, "s"));
        rig.animated();
        rig.wait(ms(30));
        if interrupted {
            rig.open_floating(3);
            rig.animated();
        }
        rig
    };
    let (with, without) = (scenario(true), scenario(false));
    let times: Vec<Duration> = (0..=30).map(|i| with.now + ms(i * 5)).collect();
    assert_same_motion(&with, &without, &[5, 6, 7, 8], &times);
}

/// A window of the special workspace that is hidden where it was shown, and shown on another
/// output in the same update, does not fade out where it was.
#[test]
fn a_special_workspace_shown_elsewhere_does_not_fade_out_where_it_was() {
    let side = OutputId(2);
    let mut rig = Rig::scrolling(Settings::default());
    rig.op(|l| {
        l.add_output(
            SEAT,
            side,
            OutputConfig {
                position: Point { x: 1920.0, y: 0.0 },
                physical_width: 1920,
                physical_height: 1080,
                scale: 1.0,
            },
        )
    });
    rig.open(1);
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.open(5);
    rig.instant();
    assert!(rig.find(5, rig.now).is_some());
    rig.wait(ms(5));
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.op(|l| l.focus_output(SEAT, side));
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.animated();
    assert_eq!(
        rig.layout.shown_special(side).unwrap().as_deref(),
        Some("s")
    );
    assert!(
        rig.find(5, rig.now + ms(50)).is_none(),
        "window 5 shows on the other output now"
    );
}

/// A window that keeps moving on its own spring while another window opens elsewhere keeps its
/// motion as it was: the same frames and the same end, not a motion restarted from where it is.
#[test]
fn an_update_that_leaves_a_window_alone_leaves_its_motion_as_it_was() {
    let mut rig = Rig::scrolling(Settings::default());
    rig.open_floating(1);
    rig.op(|l| l.set_floating_rect(w(1), Rect::new(100.0, 100.0, 400.0, 300.0)));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.set_floating_rect(w(1), Rect::new(900.0, 500.0, 400.0, 300.0)));
    rig.animated();
    let end = rig.motion.end(OUTPUT, rig.now).expect("it moves");
    rig.wait(ms(40));
    let times: Vec<Duration> = (0..=20).map(|i| rig.now + ms(i * 10)).collect();
    let before: Vec<Option<Rect>> = times
        .iter()
        .map(|&at| rig.find(1, at).map(|d| d.rect))
        .collect();
    rig.open_floating(2);
    rig.animated();
    assert_eq!(rig.motion.end(OUTPUT, rig.now), Some(end));
    let after: Vec<Option<Rect>> = times
        .iter()
        .map(|&at| rig.find(1, at).map(|d| d.rect))
        .collect();
    assert_eq!(after, before);
}

/// The same for columns on a strip whose camera runs: another column's window closing on the right
/// of the focus leaves the motion of the windows it does not touch as it was.
#[test]
fn a_strip_window_whose_place_and_camera_are_unchanged_keeps_its_motion() {
    let mut rig = Rig::scrolling(Settings::default());
    for n in 1..=5 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(1)));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(3)));
    rig.animated();
    rig.wait(ms(40));
    let times: Vec<Duration> = (0..=20).map(|i| rig.now + ms(i * 10)).collect();
    let before: Vec<Vec<Option<Rect>>> = (1..=4)
        .map(|n| {
            times
                .iter()
                .map(|&at| rig.find(n, at).map(|d| d.rect))
                .collect()
        })
        .collect();
    // A window far right closes: columns 1 to 4 keep their places and the camera its view.
    rig.op(|l| l.close_window(w(5)));
    rig.animated();
    let after: Vec<Vec<Option<Rect>>> = (1..=4)
        .map(|n| {
            times
                .iter()
                .map(|&at| rig.find(n, at).map(|d| d.rect))
                .collect()
        })
        .collect();
    assert_eq!(after, before);
}

/// Changing the settings ends every motion at once, and the strips' cameras go on to follow the
/// next change on their own spring, the new one: the camera is not forgotten.
#[test]
fn a_camera_follows_the_next_change_after_the_settings_changed() {
    let slow_camera = Settings {
        camera: yuna_motion::SpringParams::new(1.0, 300.0).unwrap(),
        windows: yuna_motion::SpringParams::new(1.0, 1500.0).unwrap(),
        ..Settings::default()
    };
    let mut rig = Rig::scrolling(Settings::default());
    for n in 1..=4 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(1)));
    rig.instant();
    rig.wait(ms(5));
    rig.motion.set_settings(slow_camera);
    let view_before = view_x(&rig);
    rig.op(|l| l.focus_window(w(4)));
    rig.animated();
    let camera = Spring::new(
        slow_camera.camera,
        view_before,
        view_x(&rig),
        0.0,
        rig.now,
        0.5,
    );
    assert_eq!(rig.motion.end(OUTPUT, rig.now), Some(camera.end()));
    let at = rig.now + ms(60);
    let place = rig
        .layout
        .arrange(OUTPUT)
        .unwrap()
        .into_iter()
        .find(|p| p.window == w(4))
        .unwrap()
        .rect;
    let lag = camera.target() - camera.value(at);
    assert_eq!(rig.drawn(4, at).rect.x, (place.x + lag + 0.5).floor());
}

fn view_x(rig: &Rig) -> f64 {
    rig.layout
        .shown_workspaces(OUTPUT)
        .unwrap()
        .main
        .view_x
        .unwrap()
}

/// The windows of a special workspace fading out are told visible while they fade, as they are
/// drawn, and no more once the fade is over.
#[test]
fn windows_fading_out_are_told_visible_until_their_fade_ends() {
    let settings = Settings::default();
    let mut rig = Rig::scrolling(settings);
    rig.open(1);
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.open(5);
    rig.instant();
    assert!(rig.motion.told_visible(OUTPUT, rig.now).contains(&w(5)));
    rig.wait(ms(5));
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.animated();
    let hidden = rig.now;
    let during = rig.motion.told_visible(OUTPUT, hidden + ms(100));
    assert_eq!(during, [w(1), w(5)]);
    let after = rig.motion.told_visible(OUTPUT, hidden + ms(250));
    assert_eq!(after, [w(1)]);
}

/// Two columns trade places: neither stays where it is on screen, so the strip's move under the view
/// is read from the focused column's, which went 952 units along the strip. The camera carries the
/// focused window across on its own spring; the other column, which has no anchor, moves on the
/// windows' offsets.
#[test]
fn the_camera_carries_the_focused_column_when_no_column_stays_in_place() {
    let settings = distinct_springs();
    let mut rig = Rig::scrolling(settings);
    rig.open(1);
    rig.open(2);
    rig.op(|l| l.focus_window(w(1)));
    rig.instant();
    let view = view_x(&rig);
    rig.wait(ms(5));
    rig.op(|l| l.move_focused(SEAT, yuna_types::Direction::Right));
    rig.animated();
    assert_eq!(view_x(&rig), view, "the view does not move");
    let place = rig
        .layout
        .arrange(OUTPUT)
        .unwrap()
        .into_iter()
        .find(|p| p.window == w(1))
        .unwrap()
        .rect;
    assert_eq!(place.x, 968.0, "window 1 is now the second column");
    // The strip moved 952 under the view, so the camera starts 952 units along it and goes back.
    let camera = Spring::new(settings.camera, view + 952.0, view, 0.0, rig.now, 0.5);
    for at in [rig.now + ms(20), rig.now + ms(60), rig.now + ms(120)] {
        let lag = camera.target() - camera.value(at);
        assert_eq!(
            rig.drawn(1, at).rect.x,
            (place.x + lag + 0.5).floor(),
            "at {at:?}"
        );
    }
}
