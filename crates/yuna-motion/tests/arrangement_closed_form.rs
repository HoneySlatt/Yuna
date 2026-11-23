//! Scenarios of the motion of an arrangement whose expected frames come from the closed form of a
//! critical spring, written here, never from the crate's own [`yuna_motion::Spring`]: the camera's
//! velocity carried by a window that leaves or joins the strip, the focused column that anchors
//! the camera, the cameras of the workspaces drawn beside the shown ones, which cameras an
//! update follows, and the windows an update leaves alone.
//!
//! Each pins a behaviour that mutation testing found no scenario pinned
//! ([debt](../../../docs/debt.md), *Tests and examples*: motion's surviving mutants).
//!
//! One output of 1920 × 1080 at scale 1, so that a physical pixel is a logical unit and a drawn
//! corner is its value rounded, ties up. The defaults' gaps and widths: a gap of 16, columns of
//! half the working area, 936 wide, 952 apart along the strip. Times start at 10 s.

use std::time::Duration;

use yuna_layout::{Layer, Layout, LayoutError, OpenOptions, Options, OutputConfig, Placed};
use yuna_motion::{Change, CurveParams, Drawn, Easing, Motion, Settings, SpringParams};
use yuna_types::{Direction, OutputId, Point, Rect, SeatId, Size, WindowId, WorkspaceId};

const SEAT: SeatId = SeatId(1);
const OUTPUT: OutputId = OutputId(1);
/// The output's height, in logical units: how far a switch slides.
const HEIGHT: f64 = 1080.0;
/// Half a physical pixel at scale 1: every spring's tolerance.
const TOLERANCE: f64 = 0.5;
/// The camera's stiffness: slower than the windows', so that what moves on the camera and what
/// moves on a window's offset show apart.
const CAMERA_STIFFNESS: f64 = 300.0;
const WINDOWS_STIFFNESS: f64 = 1500.0;
const SWITCH_STIFFNESS: f64 = 1000.0;
/// How long the special workspace's fade lasts.
const FADE: Duration = Duration::from_millis(200);

fn w(n: u64) -> WindowId {
    WindowId(n)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Critical springs of the stiffnesses above, and the fade over [`FADE`].
fn settings() -> Settings {
    Settings {
        off: false,
        reduced_motion: false,
        camera: SpringParams::new(1.0, CAMERA_STIFFNESS).unwrap(),
        windows: SpringParams::new(1.0, WINDOWS_STIFFNESS).unwrap(),
        workspace_switch: SpringParams::new(1.0, SWITCH_STIFFNESS).unwrap(),
        special_fade: CurveParams::new(Easing::EaseOutCubic, FADE).unwrap(),
    }
}

/// A critical spring's offset from its target, from offset `x0` and velocity `v0` at `start`.
///
/// The oracle of every test here. With a mass of 1, ζ = 1 and ω = √stiffness, the offset x solves
/// x'' + 2ω·x' + ω²·x = 0, whose solution is
///
/// ```text
/// x(t)  = (x0 + (v0 + ω·x0)·t)·e^(−ωt)
/// x'(t) = (v0 − ω·(v0 + ω·x0)·t)·e^(−ωt)
/// ```
///
/// From its end on, the spring is exactly at its target: the offset is 0. Its end is the time
/// from which the bound `(|x0| + |v0 + ω·x0|·t)·e^(−ωt)` of |x| stays under the tolerance, the
/// only bound the crate's documentation gives for a critical spring (*When a spring ends*).
#[derive(Debug, Clone, Copy)]
struct Critical {
    omega: f64,
    x0: f64,
    v0: f64,
    start: Duration,
}

impl Critical {
    fn new(stiffness: f64, x0: f64, v0: f64, start: Duration) -> Critical {
        Critical {
            omega: stiffness.sqrt(),
            x0,
            v0,
            start,
        }
    }

    /// The weight of `t·e^(−ωt)`: v0 + ω·x0.
    fn weight(&self) -> f64 {
        self.v0 + self.omega * self.x0
    }

    fn seconds(&self, at: Duration) -> f64 {
        (at - self.start).as_secs_f64()
    }

    /// The offset from the target at `at`, a time from the start on.
    fn offset(&self, at: Duration) -> f64 {
        if at >= self.end() {
            return 0.0;
        }
        let t = self.seconds(at);
        (self.x0 + self.weight() * t) * (-self.omega * t).exp()
    }

    /// The velocity at `at`, a time from the start on.
    fn velocity(&self, at: Duration) -> f64 {
        if at >= self.end() {
            return 0.0;
        }
        let t = self.seconds(at);
        (self.v0 - self.omega * self.weight() * t) * (-self.omega * t).exp()
    }

    /// When it ends: the last time its bound is at [`TOLERANCE`], found by bisection on the
    /// bound's falling side, which starts at its peak, `1/ω − |x0|/|v0 + ω·x0|`, or at 0.
    fn end(&self) -> Duration {
        let (a, b) = (self.x0.abs(), self.weight().abs());
        let bound = |t: f64| (a + b * t) * (-self.omega * t).exp();
        let peak = if b > 0.0 {
            (1.0 / self.omega - a / b).max(0.0)
        } else {
            0.0
        };
        if bound(peak) <= TOLERANCE {
            return self.start;
        }
        let (mut low, mut high) = (peak, peak + 1.0);
        while bound(high) > TOLERANCE {
            high *= 2.0;
        }
        for _ in 0..200 {
            let middle = (low + high) / 2.0;
            if bound(middle) > TOLERANCE {
                low = middle;
            } else {
                high = middle;
            }
        }
        self.start + Duration::from_secs_f64(high)
    }

    /// Whether it passes its target: x0 and v0 + ω·x0 have opposite signs, so that x reaches 0
    /// at t = −x0/(v0 + ω·x0) > 0.
    fn passes_its_target(&self) -> bool {
        self.x0 * self.weight() < 0.0
    }
}

/// Where a frame at scale 1 draws an edge at `place`, on the grid, moved by `by`: rounded to the
/// pixel, ties up, as `Scale::snap` documents; exactly `place` when `by` is 0.
///
/// The expected `by` and the motion's agree to far less than 10⁻⁶: a value that close to a half
/// pixel, where the two could round apart, is refused rather than compared.
#[track_caller]
fn drawn_at(place: f64, by: f64) -> f64 {
    if by == 0.0 {
        return place;
    }
    let moved = place + by;
    let from_half = (moved - moved.floor() - 0.5).abs();
    assert!(
        from_half > 1e-6,
        "{moved} is too near half a pixel to compare"
    );
    (moved + 0.5).floor()
}

/// Times from `from` to `to`, every `step`, and `to` itself.
fn samples(from: Duration, to: Duration, step: Duration) -> Vec<Duration> {
    let mut times = Vec::new();
    let mut at = from;
    while at < to {
        times.push(at);
        at += step;
    }
    times.push(to);
    times
}

/// Asserts that window `n` is drawn at `x` and `y` at `at`.
#[track_caller]
fn assert_drawn_at(rig: &Rig, n: u64, at: Duration, (x, y): (f64, f64)) {
    let drawn = rig.drawn(n, at).rect;
    assert_eq!((drawn.x, drawn.y), (x, y), "window {n} at {at:?}");
}

/// A layout with one seat and one output, its motion with [`settings`], and a clock.
struct Rig {
    layout: Layout,
    motion: Motion,
    now: Duration,
}

impl Rig {
    fn new() -> Rig {
        let mut layout = Layout::new(Options::default()).unwrap();
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
            motion: Motion::new(settings()),
            now: Duration::from_secs(10),
        };
        rig.instant();
        rig
    }

    #[track_caller]
    fn op(&mut self, op: impl FnOnce(&mut Layout) -> Result<(), LayoutError>) {
        op(&mut self.layout).unwrap();
    }

    /// Opens window `n`, tiled, on the seat's focused workspace.
    #[track_caller]
    fn open(&mut self, n: u64) {
        self.op(|l| l.open_window(SEAT, w(n), OpenOptions::default()));
    }

    /// Opens window `n`, floating, 400 × 300, at `(x, y)` on the working area.
    #[track_caller]
    fn open_floating_at(&mut self, n: u64, x: f64, y: f64) {
        let options = OpenOptions {
            floating: true,
            committed: Some(Size { w: 400.0, h: 300.0 }),
            ..OpenOptions::default()
        };
        self.op(|l| l.open_window(SEAT, w(n), options));
        self.move_floating(n, x, y);
    }

    /// Moves floating window `n` to `(x, y)` on the working area, keeping its size.
    #[track_caller]
    fn move_floating(&mut self, n: u64, x: f64, y: f64) {
        self.op(|l| l.set_floating_rect(w(n), Rect::new(x, y, 400.0, 300.0)));
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

    /// Window `n`'s place, as the output shows it.
    #[track_caller]
    fn place(&self, n: u64) -> Placed {
        self.layout
            .arrange(OUTPUT)
            .unwrap()
            .into_iter()
            .find(|p| p.window == w(n))
            .unwrap_or_else(|| panic!("window {n} is not placed"))
    }

    /// Window `n`'s place on `workspace`, shown or not.
    #[track_caller]
    fn peek(&self, workspace: WorkspaceId, n: u64) -> Placed {
        self.layout
            .arrange_workspace(OUTPUT, workspace)
            .unwrap()
            .into_iter()
            .find(|p| p.window == w(n))
            .unwrap_or_else(|| panic!("window {n} is not on {workspace:?}"))
    }

    /// The view of the main workspace shown: the strip position at the working area's left edge.
    fn view_x(&self) -> f64 {
        self.layout
            .shown_workspaces(OUTPUT)
            .unwrap()
            .main
            .view_x
            .unwrap()
    }

    /// The view of the special workspace shown.
    fn special_view_x(&self) -> f64 {
        self.layout
            .shown_workspaces(OUTPUT)
            .unwrap()
            .special
            .and_then(|special| special.view_x)
            .unwrap()
    }

    fn end(&self) -> Option<Duration> {
        self.motion.end(OUTPUT, self.now)
    }
}

/// Asserts that `end`, read from the motion, is `expected`, the closed form's, to a microsecond:
/// the crate solves an end to 10⁻¹⁰ s and rounds it up to the nanosecond.
#[track_caller]
fn assert_same_end(end: Option<Duration>, expected: Duration) {
    let end = end.expect("a motion runs");
    assert!(
        end.abs_diff(expected) < Duration::from_micros(1),
        "it ends at {end:?}, not at {expected:?}"
    );
}

/// Four columns, windows 1 to 4, window `focused` focused; the motion has not read them yet.
fn four_columns(focused: u64) -> Rig {
    let mut rig = Rig::new();
    for n in 1..=4 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(focused)));
    rig
}

// ----- The camera's velocity -----

/// A window floated while its strip's camera scrolls leaves at the speed it was drawn moving at:
/// a strip window moves on screen at minus the camera's velocity, and its offset starts with that
/// velocity, from where it was drawn.
///
/// Oracle: the camera, from view -16 to 1888, and the window's offset, each a critical spring in
/// closed form. Pins `Camera::velocity` as the offset's starting velocity reads it from the
/// camera before the update (debt: *Tests and examples*, motion's surviving mutants).
#[test]
fn a_window_floated_while_the_camera_scrolls_leaves_at_the_speed_it_was_drawn_at() {
    let mut rig = four_columns(1);
    rig.instant();
    assert_eq!(rig.view_x(), -16.0);
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(4)));
    rig.animated();
    // Column 4 starts 3 × 952 along the strip; the view shows it whole, a gap from the right.
    assert_eq!(rig.view_x(), 1888.0);
    let camera = Critical::new(CAMERA_STIFFNESS, -16.0 - 1888.0, 0.0, rig.now);
    rig.wait(ms(30));
    let tiled = rig.place(4).rect;
    assert_eq!((tiled.x, tiled.y), (968.0, 16.0));
    rig.op(|l| l.set_floating(w(4), true));
    rig.animated();
    let floating = rig.place(4);
    assert_eq!(floating.layer, Layer::Floating);
    // On the strip it was drawn at its place plus the camera's lag, the view minus the camera,
    // which is minus the camera's offset; it moved at minus the camera's velocity.
    let lag = -camera.offset(rig.now);
    let speed = -camera.velocity(rig.now);
    assert!(speed < -5000.0, "the strip moves left at {speed} units/s");
    let x = Critical::new(
        WINDOWS_STIFFNESS,
        tiled.x + lag - floating.rect.x,
        speed,
        rig.now,
    );
    let y = Critical::new(WINDOWS_STIFFNESS, tiled.y - floating.rect.y, 0.0, rig.now);
    for at in samples(rig.now, rig.now + ms(60), ms(4)) {
        let expected = (
            drawn_at(floating.rect.x, x.offset(at)),
            drawn_at(floating.rect.y, y.offset(at)),
        );
        assert_drawn_at(&rig, 4, at, expected);
    }
}

/// A floating window tiled while the camera scrolls joins the strip without a jump or a change of
/// speed: it was still, and on the strip the camera carries it, so its offset starts with the
/// camera's velocity, which cancels the camera's own on screen.
///
/// Oracle: the camera, from view 1888 to -16, and the window's offset, critical springs in closed
/// form. Pins `Camera::velocity` as the offset's starting velocity reads it from the camera
/// after the update (debt: *Tests and examples*, motion's surviving mutants).
#[test]
fn a_window_tiled_while_the_camera_scrolls_starts_from_rest_on_screen() {
    let mut rig = four_columns(4);
    rig.open_floating_at(9, 500.0, 500.0);
    rig.op(|l| l.focus_window(w(4)));
    rig.instant();
    assert_eq!(rig.view_x(), 1888.0);
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(1)));
    rig.animated();
    assert_eq!(rig.view_x(), -16.0);
    let camera = Critical::new(CAMERA_STIFFNESS, 1888.0 - -16.0, 0.0, rig.now);
    rig.wait(ms(30));
    let speed = camera.velocity(rig.now);
    assert!(speed < -5000.0, "the camera moves left at {speed} units/s");
    rig.op(|l| l.set_floating(w(9), false));
    rig.animated();
    // Window 9 joins the strip right of column 1, in view: neither the view nor column 1 moved,
    // so the camera runs on as it was.
    assert_eq!(rig.view_x(), -16.0);
    let place = rig.place(9);
    assert_eq!(
        (place.layer, place.rect.x, place.rect.y),
        (Layer::Strip, 968.0, 16.0)
    );
    let lag = |at| -camera.offset(at);
    let x = Critical::new(
        WINDOWS_STIFFNESS,
        500.0 - (place.rect.x + lag(rig.now)),
        speed,
        rig.now,
    );
    let y = Critical::new(WINDOWS_STIFFNESS, 500.0 - place.rect.y, 0.0, rig.now);
    for at in samples(rig.now, rig.now + ms(60), ms(4)) {
        let expected = (
            drawn_at(place.rect.x, x.offset(at) + lag(at)),
            drawn_at(place.rect.y, y.offset(at)),
        );
        assert_drawn_at(&rig, 9, at, expected);
    }
}

// ----- The focused column, the camera's anchor -----

/// Two columns trade places: neither stays in place on screen, so the camera takes up the move of
/// the focused column, window 1's, 952 along the strip, and window 2 crosses on its offset.
///
/// Oracle: the camera, from 952 right of view -16 back to it, and window 2's offset, from 968
/// where it was drawn, critical springs in closed form. Pins `focused_column`: the focused
/// column is the column of the window with the seat's focus, not the strip's first (debt: *Tests
/// and examples*, motion's surviving mutants).
#[test]
fn columns_trading_places_move_the_focused_one_on_the_camera_and_the_other_on_its_offset() {
    let mut rig = Rig::new();
    rig.open(1);
    rig.open(2);
    rig.op(|l| l.focus_window(w(1)));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.move_focused(SEAT, Direction::Right));
    rig.animated();
    assert_eq!(rig.view_x(), -16.0);
    assert_eq!((rig.place(1).rect.x, rig.place(2).rect.x), (968.0, 16.0));
    let camera = Critical::new(CAMERA_STIFFNESS, 952.0, 0.0, rig.now);
    let lag = |at| -camera.offset(at);
    // Window 2 was drawn at 968; at its new place, 16, the camera draws it at 16 - 952: its
    // offset starts at the difference.
    let x2 = Critical::new(WINDOWS_STIFFNESS, 968.0 - (16.0 - 952.0), 0.0, rig.now);
    for at in samples(rig.now, rig.now + ms(200), ms(5)) {
        assert_drawn_at(&rig, 1, at, (drawn_at(968.0, lag(at)), 16.0));
        assert_drawn_at(&rig, 2, at, (drawn_at(16.0, x2.offset(at) + lag(at)), 16.0));
    }
}

/// A focus that moves neither the view nor the strip leaves the camera still, yet the camera
/// learns the column now focused: when the next change leaves no column in place, it carries that
/// column, window 2's, not the one focused before.
///
/// The change closes column 1 and opens window 3 right of column 2: column 2 goes from 968 to 16,
/// 952 back along the strip, and window 3 is new. Oracle: the camera from 952 left of view -16
/// back to it, in closed form, carries windows 2 and 3. Pins `Camera::follow`'s focused column
/// when the camera does not move (debt: *Tests and examples*, motion's surviving mutants).
#[test]
fn a_camera_left_still_by_a_focus_carries_the_column_focused_next() {
    let mut rig = Rig::new();
    rig.open(1);
    rig.open(2);
    rig.op(|l| l.focus_window(w(1)));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(2)));
    rig.animated();
    assert_eq!(rig.view_x(), -16.0);
    assert_eq!(rig.end(), None, "column 2 is in view: nothing moves");
    rig.wait(ms(5));
    rig.op(|l| l.close_window(w(1)));
    rig.open(3);
    rig.animated();
    assert_eq!(rig.view_x(), -16.0);
    assert_eq!((rig.place(2).rect.x, rig.place(3).rect.x), (16.0, 968.0));
    let camera = Critical::new(CAMERA_STIFFNESS, -952.0, 0.0, rig.now);
    let lag = |at| -camera.offset(at);
    for at in samples(rig.now, rig.now + ms(300), ms(5)) {
        assert_drawn_at(&rig, 2, at, (drawn_at(16.0, lag(at)), 16.0));
        assert_drawn_at(&rig, 3, at, (drawn_at(968.0, lag(at)), 16.0));
    }
}

/// While a floating window has the seat's focus, the strip remembers its focused column: when
/// the next change leaves no column in place, the camera carries that column, window 2's.
///
/// The change closes column 1 and tiles the floating window, 400 wide as it committed, right of
/// column 2: column 2 moves 952 back along the strip, and the view goes from -16 to -552 to show
/// window 9, so that no column stays in place. Oracle: the camera, moved by column 2's shift to
/// -968, goes to -552 and carries window 2 from 968 to 552; window 9 comes from its floating
/// place, (500, 500), on its offset. Critical springs in closed form. Pins `remember_focus`
/// (debt: *Tests and examples*, motion's surviving mutants).
#[test]
fn a_strip_remembers_its_focused_column_while_a_floating_window_has_the_focus() {
    let mut rig = Rig::new();
    rig.open(1);
    rig.open(2);
    rig.open_floating_at(9, 500.0, 500.0);
    rig.op(|l| l.focus_window(w(2)));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(9)));
    rig.animated();
    assert_eq!(
        rig.end(),
        None,
        "a focus on a floating window moves nothing"
    );
    rig.wait(ms(5));
    rig.op(|l| l.close_window(w(1)));
    rig.op(|l| l.set_floating(w(9), false));
    rig.animated();
    assert_eq!(rig.view_x(), -552.0);
    assert_eq!((rig.place(2).rect.x, rig.place(9).rect.x), (552.0, 1504.0));
    let camera = Critical::new(CAMERA_STIFFNESS, -968.0 - -552.0, 0.0, rig.now);
    let lag = |at| -camera.offset(at);
    // Window 9 was drawn at (500, 500); at 1504 the camera draws it 416 right of its place.
    let x9 = Critical::new(WINDOWS_STIFFNESS, 500.0 - (1504.0 + 416.0), 0.0, rig.now);
    let y9 = Critical::new(WINDOWS_STIFFNESS, 500.0 - 16.0, 0.0, rig.now);
    for at in samples(rig.now, rig.now + ms(300), ms(5)) {
        assert_drawn_at(&rig, 2, at, (drawn_at(552.0, lag(at)), 16.0));
        let expected = (
            drawn_at(1504.0, x9.offset(at) + lag(at)),
            drawn_at(16.0, y9.offset(at)),
        );
        assert_drawn_at(&rig, 9, at, expected);
    }
}

// ----- The cameras an update follows, and those it keeps -----

/// A workspace that slides out of a switch while its strip scrolls goes on scrolling on its own
/// camera, as it was, through the updates that come while it slides: the output keeps the camera
/// of each workspace its switch draws beside the shown one.
///
/// 40 ms into the switch, window 3 opens on the second workspace. Oracle: the first workspace's
/// camera, from view -16 to 1888, and the switch's slide, from 0 to the output's height, critical
/// springs in closed form; a window is drawn while its rectangle overlaps the output. Pins
/// `follow_cameras`'s cameras kept beside the shown workspaces, and `draws` for the workspaces of
/// a switch (debt: *Tests and examples*, motion's surviving mutants).
#[test]
fn a_workspace_sliding_out_scrolls_on_its_own_camera() {
    let mut rig = Rig::new();
    for n in 11..=14 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(11)));
    let first = rig.place(11).workspace;
    rig.op(|l| l.add_workspace(SEAT, "2", Some(OUTPUT)));
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.open(2);
    rig.op(|l| l.focus_window(w(11)));
    rig.instant();
    assert_eq!(rig.view_x(), -16.0);
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(14)));
    rig.animated();
    assert_eq!(rig.view_x(), 1888.0);
    let camera = Critical::new(CAMERA_STIFFNESS, -16.0 - 1888.0, 0.0, rig.now);
    rig.wait(ms(20));
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.animated();
    // The first workspace is on row 0 of the switch, the second on row 1: the view slides from
    // 0 to the output's height, and the first workspace is drawn that much up.
    let slide = Critical::new(SWITCH_STIFFNESS, -HEIGHT, 0.0, rig.now);
    rig.wait(ms(40));
    rig.open_floating_at(3, 100.0, 100.0);
    rig.animated();
    let output = Rect::new(0.0, 0.0, 1920.0, HEIGHT);
    let mut seen = 0;
    for at in samples(rig.now, rig.now + ms(260), ms(5)) {
        let position = HEIGHT + slide.offset(at);
        for n in 11..=14 {
            let place = rig.peek(first, n).rect;
            let expected = Rect {
                x: drawn_at(place.x, -camera.offset(at)),
                y: drawn_at(place.y, -position),
                ..place
            };
            if expected.overlaps(&output) {
                assert_eq!(rig.drawn(n, at).rect, expected, "window {n} at {at:?}");
                seen += 1;
            } else {
                assert!(rig.find(n, at).is_none(), "window {n} is off the output");
            }
        }
    }
    assert!(seen > 20, "the first workspace is seen sliding out");
}

/// A workspace sliding into view whose strip scrolls meanwhile goes to its new view on its own
/// camera: the camera of the workspace shown is followed, not put back as the switch kept it.
///
/// 50 ms into the switch to the second workspace, a focus on window 24 moves its view from -16 to
/// 1888. Oracle: the second workspace's camera and the switch's slide, from 0 to the output's
/// height, critical springs in closed form; while the switch slides, a window is drawn if its
/// rectangle overlaps the output. Pins `follow_cameras`: the cameras kept beside the shown
/// workspaces are those of the others alone (debt: *Tests and examples*, motion's surviving
/// mutants).
#[test]
fn a_workspace_sliding_in_scrolls_on_its_followed_camera() {
    let mut rig = Rig::new();
    rig.open(1);
    rig.op(|l| l.add_workspace(SEAT, "2", Some(OUTPUT)));
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    for n in 21..=24 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(21)));
    rig.op(|l| l.focus_window(w(1)));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.animated();
    assert_eq!(rig.view_x(), -16.0);
    // The first workspace is on row 0 of the switch, the second on row 1: the view slides from
    // 0 to the output's height, and the second workspace is drawn the rest of the way down.
    let slide = Critical::new(SWITCH_STIFFNESS, -HEIGHT, 0.0, rig.now);
    rig.wait(ms(50));
    rig.op(|l| l.focus_window(w(24)));
    rig.animated();
    assert_eq!(rig.view_x(), 1888.0);
    let camera = Critical::new(CAMERA_STIFFNESS, -16.0 - 1888.0, 0.0, rig.now);
    let output = Rect::new(0.0, 0.0, 1920.0, HEIGHT);
    let mut seen = 0;
    // From the switch's end on, the frame is the arrangement, off the output too.
    for at in samples(rig.now, slide.end() - ms(1), ms(5)) {
        let position = HEIGHT + slide.offset(at);
        for n in 21..=24 {
            let place = rig.place(n).rect;
            let expected = Rect {
                x: drawn_at(place.x, -camera.offset(at)),
                y: drawn_at(place.y, HEIGHT - position),
                ..place
            };
            if expected.overlaps(&output) {
                assert_eq!(rig.drawn(n, at).rect, expected, "window {n} at {at:?}");
                seen += 1;
            } else {
                assert!(rig.find(n, at).is_none(), "window {n} is off the output");
            }
        }
    }
    assert!(seen > 20, "the second workspace is seen sliding in");
}

/// The same for a special workspace hidden while its strip scrolls: it fades out scrolling on its
/// own camera, as it was, through the updates that come while it fades. Its windows are drawn at
/// their last places, which the layout no longer gives.
///
/// 40 ms into the fade, window 3 opens on the main workspace. Oracle: the special workspace's
/// camera, from view -16 to 1888, a critical spring in closed form, until the fade ends. Pins
/// `follow_cameras`'s cameras kept beside the shown workspaces, and `draws` for a workspace
/// fading out (debt: *Tests and examples*, motion's surviving mutants).
#[test]
fn a_special_workspace_fading_out_scrolls_on_its_own_camera() {
    let mut rig = Rig::new();
    rig.open(1);
    rig.op(|l| l.toggle_special(SEAT, "s"));
    for n in 5..=8 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(5)));
    rig.instant();
    assert_eq!(rig.special_view_x(), -16.0);
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(8)));
    rig.animated();
    assert_eq!(rig.special_view_x(), 1888.0);
    let camera = Critical::new(CAMERA_STIFFNESS, -16.0 - 1888.0, 0.0, rig.now);
    let places: Vec<(u64, Rect)> = (5..=8).map(|n| (n, rig.place(n).rect)).collect();
    rig.wait(ms(10));
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.animated();
    let hidden = rig.now;
    rig.wait(ms(40));
    rig.open_floating_at(3, 100.0, 100.0);
    rig.animated();
    for at in samples(rig.now, hidden + FADE - ms(1), ms(5)) {
        for &(n, place) in &places {
            let expected = (drawn_at(place.x, -camera.offset(at)), place.y);
            assert_drawn_at(&rig, n, at, expected);
        }
    }
}

/// A workspace shown again once the switch that took it away has ended slides in at its new
/// view, at rest: the camera the output kept for it while the switch slid it out is no longer
/// drawn, and is not followed.
///
/// The first workspace leaves at view -16 and comes back by a focus on window 14, at view 1888.
/// Oracle: its windows at their places along the strip, the switch's slide from the second
/// workspace's row to the first's a critical spring in closed form; while the switch slides, a
/// window is drawn if its rectangle overlaps the output. Pins `draws`: a camera kept for a
/// workspace the output no longer draws is not followed (debt: *Tests and examples*, motion's
/// surviving mutants).
#[test]
fn a_workspace_shown_again_after_its_switch_ended_slides_in_at_rest_at_its_new_view() {
    let mut rig = Rig::new();
    for n in 11..=14 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(11)));
    rig.op(|l| l.add_workspace(SEAT, "2", Some(OUTPUT)));
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.open(2);
    rig.op(|l| l.focus_window(w(11)));
    rig.instant();
    assert_eq!(rig.view_x(), -16.0);
    rig.wait(ms(5));
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.animated();
    let away = Critical::new(SWITCH_STIFFNESS, -HEIGHT, 0.0, rig.now);
    rig.wait(ms(600));
    assert!(away.end() < rig.now, "the switch away has ended");
    assert_eq!(rig.end(), None, "nothing runs");
    rig.op(|l| l.focus_window(w(14)));
    rig.animated();
    assert_eq!(rig.view_x(), 1888.0);
    let back = Critical::new(SWITCH_STIFFNESS, HEIGHT, 0.0, rig.now);
    let output = Rect::new(0.0, 0.0, 1920.0, HEIGHT);
    let mut seen = 0;
    // From the switch's end on, the frame is the arrangement, off the output too.
    for at in samples(rig.now, back.end() - ms(1), ms(5)) {
        for n in 11..=14 {
            let place = rig.place(n).rect;
            let expected = Rect {
                y: drawn_at(place.y, -back.offset(at)),
                ..place
            };
            if expected.overlaps(&output) {
                assert_eq!(rig.drawn(n, at).rect, expected, "window {n} at {at:?}");
                seen += 1;
            } else {
                assert!(rig.find(n, at).is_none(), "window {n} is off the output");
            }
        }
    }
    assert!(seen > 20, "the first workspace is seen sliding in");
}

/// A special workspace shown again once its fade out has ended appears at its places, even while
/// a switch still slides: the camera the output kept for it while it faded out, its strip still
/// scrolling then, is no longer drawn, and is not followed.
///
/// The special workspace's strip scrolls from view -16 to 1888 on a slow camera; 10 ms later the
/// special workspace is hidden and the output switches to its second workspace; 250 ms later,
/// past the fade's 200 ms but within the switch, it is shown again. Oracle: its windows exactly
/// at their places, by the definition of a workspace shown that the output did not draw; the
/// switch's end from the closed form of its critical spring. Pins `draws`: a switch draws its
/// own workspaces alone, not one whose fade has ended (debt: *Tests and examples*, motion's
/// surviving mutants).
#[test]
fn a_special_workspace_shown_again_after_its_fade_out_ended_appears_at_its_places() {
    let mut rig = Rig::new();
    rig.open(1);
    rig.op(|l| l.add_workspace(SEAT, "2", Some(OUTPUT)));
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.open(2);
    rig.op(|l| l.focus_window(w(1)));
    rig.op(|l| l.toggle_special(SEAT, "s"));
    for n in 5..=8 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(5)));
    rig.instant();
    assert_eq!(rig.special_view_x(), -16.0);
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(8)));
    rig.animated();
    assert_eq!(rig.special_view_x(), 1888.0);
    let scroll = Critical::new(CAMERA_STIFFNESS, -16.0 - 1888.0, 0.0, rig.now);
    rig.wait(ms(10));
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.animated();
    let switch = Critical::new(SWITCH_STIFFNESS, -HEIGHT, 0.0, rig.now);
    let hidden = rig.now;
    rig.wait(ms(250));
    assert!(hidden + FADE < rig.now, "the fade out has ended");
    assert!(rig.now < switch.end(), "the switch still slides");
    assert!(
        scroll.offset(rig.now).abs() > 50.0,
        "the special workspace's camera would still lag"
    );
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.animated();
    for at in samples(rig.now, rig.now + ms(300), ms(5)) {
        for n in 5..=8 {
            let place = rig.place(n).rect;
            assert_eq!(rig.drawn(n, at).rect, place, "window {n} at {at:?}");
        }
    }
}

// ----- The windows an update leaves alone -----

/// A window expelled into a new column where its old column was keeps its place, but not its
/// camera: the old column moved 952 along the strip, and with no column in place the camera
/// takes up that move. The window's offset takes up what the camera adds, so that it does not
/// jump.
///
/// Both windows committed their sizes, so that the expelled one's rectangle is the same. Oracle:
/// the camera from 952 right of view -16 back to it, and window 1's offset from 952, critical
/// springs in closed form. Pins `unmoved`: a window whose place stays is left alone only when its
/// camera stays too (debt: *Tests and examples*, motion's surviving mutants).
#[test]
fn a_window_expelled_where_its_column_was_takes_up_the_cameras_move() {
    let mut rig = Rig::new();
    rig.open(1);
    rig.open(2);
    // Window 2, focused, joins column 1, under window 1.
    rig.op(|l| l.consume_or_expel(SEAT, Direction::Left));
    for n in [1, 2] {
        rig.op(|l| l.commit_size(w(n), Size { w: 936.0, h: 500.0 }));
    }
    rig.op(|l| l.focus_window(w(1)));
    rig.instant();
    let before = rig.place(1);
    assert_eq!(rig.place(2).rect.y, 16.0 + 500.0 + 16.0);
    rig.wait(ms(5));
    rig.op(|l| l.consume_or_expel(SEAT, Direction::Left));
    rig.animated();
    let after = rig.place(1);
    assert_eq!(after.rect, before.rect, "window 1 keeps its place");
    assert_ne!(after.column, before.column, "in a new column");
    assert_eq!(rig.view_x(), -16.0);
    assert_eq!((rig.place(2).rect.x, rig.place(2).rect.y), (968.0, 16.0));
    let camera = Critical::new(CAMERA_STIFFNESS, 952.0, 0.0, rig.now);
    let lag = |at| -camera.offset(at);
    let x1 = Critical::new(WINDOWS_STIFFNESS, 952.0, 0.0, rig.now);
    // Window 2 was drawn at (16, 532): at 968, the camera draws it at 16 again.
    let y2 = Critical::new(WINDOWS_STIFFNESS, 532.0 - 16.0, 0.0, rig.now);
    for at in samples(rig.now, rig.now + ms(300), ms(5)) {
        assert_drawn_at(&rig, 1, at, (drawn_at(16.0, x1.offset(at) + lag(at)), 16.0));
        let expected = (drawn_at(968.0, lag(at)), drawn_at(16.0, y2.offset(at)));
        assert_drawn_at(&rig, 2, at, expected);
    }
}

/// A floating window whose motion passes its target keeps the end that motion was given when an
/// update leaves it alone: its offset runs on as it was, not solved again from where it is, which
/// would end it earlier.
///
/// The window slides right from x = 0 to 1500 and, 50 ms later, while it passes x = 865 at 16,000
/// units/s, is sent to 1060: it passes 1060. 20 ms later, window 10 opens. Oracle: both motions
/// critical springs in closed form, the end the last time the second's bound is at the
/// tolerance. Pins `unmoved` for a window without a camera (debt: *Tests and examples*, motion's
/// surviving mutants).
#[test]
fn a_floating_window_left_alone_keeps_the_end_of_its_motion() {
    let mut rig = Rig::new();
    rig.open_floating_at(9, 0.0, 100.0);
    rig.instant();
    rig.wait(ms(5));
    rig.move_floating(9, 1500.0, 100.0);
    rig.animated();
    let first = Critical::new(WINDOWS_STIFFNESS, -1500.0, 0.0, rig.now);
    rig.wait(ms(50));
    rig.move_floating(9, 1060.0, 100.0);
    rig.animated();
    let second = Critical::new(
        WINDOWS_STIFFNESS,
        1500.0 + first.offset(rig.now) - 1060.0,
        first.velocity(rig.now),
        rig.now,
    );
    assert!(second.passes_its_target());
    assert_same_end(rig.end(), second.end());
    rig.wait(ms(20));
    // Solved again from here, the motion would end over a millisecond earlier.
    let again = Critical::new(
        WINDOWS_STIFFNESS,
        second.offset(rig.now),
        second.velocity(rig.now),
        rig.now,
    );
    assert!(again.end() + ms(1) < second.end());
    rig.open_floating_at(10, 1200.0, 600.0);
    rig.animated();
    assert_same_end(rig.end(), second.end());
    for at in samples(rig.now, rig.now + ms(100), ms(5)) {
        assert_drawn_at(&rig, 9, at, (drawn_at(1060.0, second.offset(at)), 100.0));
    }
}

/// The same for a strip window: window 9, sliding right as a floating window, is tiled right of
/// column 1, at 968, as it passes x = 865 at 16,000 units/s; it passes 968. 20 ms later, window
/// 10 opens, floating: column 1 and window 9 keep their places and the camera stays still.
///
/// Oracle: both motions critical springs in closed form, the end the last time the second's bound
/// is at the tolerance. Pins `unmoved` for a window whose camera stays (debt: *Tests and
/// examples*, motion's surviving mutants).
#[test]
fn a_strip_window_left_alone_keeps_the_end_of_its_motion() {
    let mut rig = Rig::new();
    rig.open(1);
    rig.open_floating_at(9, 0.0, 16.0);
    rig.instant();
    rig.wait(ms(5));
    rig.move_floating(9, 1500.0, 16.0);
    rig.animated();
    let first = Critical::new(WINDOWS_STIFFNESS, -1500.0, 0.0, rig.now);
    rig.wait(ms(50));
    rig.op(|l| l.set_floating(w(9), false));
    rig.animated();
    assert_eq!(rig.view_x(), -16.0);
    let place = rig.place(9);
    assert_eq!(
        (place.layer, place.rect.x, place.rect.y),
        (Layer::Strip, 968.0, 16.0)
    );
    let second = Critical::new(
        WINDOWS_STIFFNESS,
        1500.0 + first.offset(rig.now) - 968.0,
        first.velocity(rig.now),
        rig.now,
    );
    assert!(second.passes_its_target());
    assert_same_end(rig.end(), second.end());
    rig.wait(ms(20));
    let again = Critical::new(
        WINDOWS_STIFFNESS,
        second.offset(rig.now),
        second.velocity(rig.now),
        rig.now,
    );
    assert!(again.end() + ms(1) < second.end());
    rig.open_floating_at(10, 1200.0, 600.0);
    rig.animated();
    assert_eq!(rig.place(9).rect, place.rect);
    assert_same_end(rig.end(), second.end());
    for at in samples(rig.now, rig.now + ms(100), ms(5)) {
        assert_drawn_at(&rig, 9, at, (drawn_at(968.0, second.offset(at)), 16.0));
    }
}
