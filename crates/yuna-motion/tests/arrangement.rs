//! Scenarios of the motion of an arrangement: a layout driven by its public operations, read by
//! a [`Motion`] after each, and its frames compared with the springs and curves they must
//! follow, computed here from the crate's own [`Spring`] and [`Curve`].
//!
//! One output of 1920 × 1080 at scale 1, so that a physical pixel is a logical unit and a drawn
//! corner is its value rounded, ties up. Times start at 10 s, away from the clock's origin.

use std::time::Duration;

use yuna_layout::geometry::{Point, Rect, Size};
use yuna_layout::{
    Layer, Layout, LayoutError, OpenOptions, Options, OutputConfig, OutputId, Placed, SeatId,
    Tiling, WindowId, WorkspaceId,
};
use yuna_motion::{
    Change, Curve, CurveParams, Drawn, Easing, Frame, Motion, Settings, Spring, SpringParams,
};

const SEAT: SeatId = SeatId(1);
const OUTPUT: OutputId = OutputId(1);
/// The output's height, in logical units: how far a switch slides.
const HEIGHT: f64 = 1080.0;
/// Half a physical pixel at scale 1: every spring's tolerance.
const TOLERANCE: f64 = 0.5;
/// A frame at 240 Hz, rounded up to the nanosecond.
const FRAME_AT_240_HZ: Duration = Duration::from_nanos(4_166_667);

fn w(n: u64) -> WindowId {
    WindowId(n)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A camera and windows on springs of different stiffness, so that a camera move and window
/// offsets that cancel only when they are equal show apart.
fn distinct_springs() -> Settings {
    Settings {
        camera: SpringParams::new(1.0, 300.0).unwrap(),
        windows: SpringParams::new(1.0, 1500.0).unwrap(),
        ..Settings::default()
    }
}

/// `value` moved by `by` as a frame draws it at scale 1: rounded, ties up; unmoved when `by` is
/// 0.
fn moved(value: f64, by: f64) -> f64 {
    if by == 0.0 {
        value
    } else {
        (value + by + 0.5).floor()
    }
}

/// Times from `from` to `to`, every 3 ms, and `to` itself.
fn samples(from: Duration, to: Duration) -> Vec<Duration> {
    let mut times = Vec::new();
    let mut at = from;
    while at < to {
        times.push(at);
        at += ms(3);
    }
    times.push(to);
    times
}

/// The layout's arrangement as a frame at rest draws it.
fn arrangement(layout: &Layout, output: OutputId) -> Frame {
    let windows = layout
        .arrange(output)
        .unwrap()
        .into_iter()
        .map(|placed| Drawn {
            window: placed.window,
            rect: placed.rect,
            alpha: 1.0,
            workspace: placed.workspace,
            placed,
        })
        .collect();
    Frame { windows }
}

/// A layout with one seat and one output, its motion, and a clock.
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
        rig.instant();
        rig
    }

    /// Scrolling columns, the defaults' gaps and widths.
    fn scrolling(settings: Settings) -> Rig {
        Rig::new(Options::default(), settings)
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

    /// Opens window `n`, floating, 400 × 300.
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

    fn frame(&self, at: Duration) -> Frame {
        self.motion.frame(OUTPUT, at)
    }

    fn find(&self, n: u64, at: Duration) -> Option<Drawn> {
        self.frame(at)
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

    fn workspace(&self, name: &str) -> WorkspaceId {
        self.layout
            .workspaces(OUTPUT)
            .unwrap()
            .into_iter()
            .find(|ws| ws.name.as_deref() == Some(name))
            .unwrap()
            .id
    }

    fn view_x(&self) -> f64 {
        self.layout
            .shown_workspaces(OUTPUT)
            .unwrap()
            .main
            .view_x
            .unwrap()
    }

    fn end(&self) -> Option<Duration> {
        self.motion.end(OUTPUT, self.now)
    }

    fn told(&self, at: Duration) -> Vec<u64> {
        self.motion
            .told_visible(OUTPUT, at)
            .into_iter()
            .map(|window| window.0)
            .collect()
    }

    /// The frame at `at` is the arrangement, and nothing runs.
    #[track_caller]
    fn assert_at_rest(&self, at: Duration) {
        assert_eq!(self.frame(at), arrangement(&self.layout, OUTPUT));
        assert_eq!(self.motion.end(OUTPUT, at), None);
    }
}

/// Three named workspaces, "1", "2" and "3", in that order on the output, holding windows 1, 2
/// and 3; "1" shown, at rest.
fn three_workspaces(settings: Settings) -> Rig {
    let mut rig = Rig::scrolling(settings);
    for name in ["1", "2", "3"] {
        rig.op(|l| l.add_workspace(SEAT, name, Some(OUTPUT)));
    }
    for (n, name) in [(1, "1"), (2, "2"), (3, "3")] {
        rig.op(|l| l.focus_workspace(SEAT, name));
        rig.open(n);
    }
    rig.op(|l| l.focus_workspace(SEAT, "1"));
    rig.instant();
    let order: Vec<_> = rig.layout.workspaces(OUTPUT).unwrap();
    let names: Vec<_> = order.iter().map(|ws| ws.name.as_deref()).collect();
    assert_eq!(names, [Some("1"), Some("2"), Some("3")]);
    rig
}

// ----- 1. Offsets -----

#[test]
fn a_window_whose_place_moves_slides_from_where_it_was_drawn() {
    let settings = distinct_springs();
    let options = Options {
        default_tiling: Tiling::Dwindle,
        ..Options::default()
    };
    let mut rig = Rig::new(options, settings);
    rig.open(1);
    rig.open(2);
    rig.instant();
    let before = rig.place(2).rect;
    rig.wait(ms(5));
    // Window 2 takes the whole output: its corner moves left.
    rig.op(|l| l.close_window(w(1)));
    rig.animated();
    let after = rig.place(2).rect;
    assert!(after.x < before.x);
    assert!(rig.find(1, rig.now).is_none(), "a closed window is dropped");
    let x = Spring::new(
        settings.windows,
        before.x - after.x,
        0.0,
        0.0,
        rig.now,
        TOLERANCE,
    );
    let y = Spring::new(
        settings.windows,
        before.y - after.y,
        0.0,
        0.0,
        rig.now,
        TOLERANCE,
    );
    let end = x.end().max(y.end());
    assert_eq!(rig.end(), Some(end));
    assert_eq!(rig.drawn(2, rig.now).rect.x, before.x);
    for at in samples(rig.now, end) {
        let expected = Rect {
            x: moved(after.x, x.value(at)),
            y: moved(after.y, y.value(at)),
            ..after
        };
        assert_eq!(rig.drawn(2, at).rect, expected, "at {at:?}");
    }
    rig.assert_at_rest(end);
}

#[test]
fn a_running_offset_is_retargeted_with_its_velocity() {
    let settings = Settings::default();
    let mut rig = Rig::scrolling(settings);
    rig.open_floating(1);
    rig.op(|l| l.set_floating_rect(w(1), Rect::new(100.0, 100.0, 400.0, 300.0)));
    rig.instant();
    let start = rig.place(1).rect;
    rig.wait(ms(5));
    rig.op(|l| l.set_floating_rect(w(1), Rect::new(1100.0, 100.0, 400.0, 300.0)));
    rig.animated();
    let first = rig.place(1).rect;
    let going = Spring::new(
        settings.windows,
        start.x - first.x,
        0.0,
        0.0,
        rig.now,
        TOLERANCE,
    );
    // A change of mind, mid-way.
    rig.wait(ms(30));
    let turn = rig.now;
    let drawn_at_turn = first.x + going.value(turn);
    let velocity = going.velocity(turn);
    assert!(velocity > 1000.0, "it moves right at {velocity} units/s");
    rig.op(|l| l.set_floating_rect(w(1), Rect::new(100.0, 100.0, 400.0, 300.0)));
    rig.animated();
    let second = rig.place(1).rect;
    let back = Spring::new(
        settings.windows,
        drawn_at_turn - second.x,
        0.0,
        velocity,
        turn,
        TOLERANCE,
    );
    assert_eq!(rig.end(), Some(back.end()));
    for at in samples(turn, back.end()) {
        assert_eq!(rig.drawn(1, at).rect.x, moved(second.x, back.value(at)));
        assert_eq!(rig.drawn(1, at).rect.y, second.y);
    }
    // It goes on to the right before it turns: the velocity is kept.
    assert!(rig.drawn(1, turn + ms(4)).rect.x > rig.drawn(1, turn).rect.x);
    rig.assert_at_rest(back.end());
}

#[test]
fn a_window_going_floating_or_tiled_keeps_where_it_is_drawn() {
    let mut rig = Rig::scrolling(distinct_springs());
    rig.open(1);
    rig.open_floating(2);
    rig.instant();
    let floating = rig.place(2).rect;
    rig.wait(ms(5));
    rig.op(|l| l.set_floating(w(2), false));
    rig.animated();
    let tiled = rig.place(2);
    assert_eq!(tiled.layer, Layer::Strip);
    assert_ne!((tiled.rect.x, tiled.rect.y), (floating.x, floating.y));
    let drawn = rig.drawn(2, rig.now).rect;
    assert_eq!((drawn.x, drawn.y), (floating.x, floating.y));
    // Back to floating mid-way: it goes on from where it is drawn.
    rig.wait(ms(40));
    let drawn = rig.drawn(2, rig.now).rect;
    assert_ne!((drawn.x, drawn.y), (tiled.rect.x, tiled.rect.y));
    rig.op(|l| l.set_floating(w(2), true));
    rig.animated();
    assert_eq!(rig.place(2).layer, Layer::Floating);
    let after = rig.drawn(2, rig.now).rect;
    assert_eq!((after.x, after.y), (drawn.x, drawn.y));
    let end = rig.end().unwrap();
    rig.assert_at_rest(end);
}

#[test]
fn a_window_changing_layer_or_workspace_or_new_moves_at_once() {
    let mut rig = Rig::scrolling(Settings::default());
    rig.open(1);
    rig.open_floating(2);
    rig.op(|l| l.set_floating_rect(w(2), Rect::new(100.0, 100.0, 400.0, 300.0)));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.set_floating_rect(w(2), Rect::new(900.0, 100.0, 400.0, 300.0)));
    rig.animated();
    rig.wait(ms(20));
    assert_ne!(rig.drawn(2, rig.now).rect, rig.place(2).rect);
    // Pinned: another layer, drawn at its place at once.
    rig.op(|l| l.set_pinned(w(2), true));
    rig.animated();
    assert_eq!(rig.drawn(2, rig.now).rect, rig.place(2).rect);
    rig.assert_at_rest(rig.now);

    // A new window is drawn at its place at once.
    rig.wait(ms(5));
    rig.open_floating(3);
    rig.animated();
    assert_eq!(rig.drawn(3, rig.now).rect, rig.place(3).rect);

    // Moved to another workspace, the seat following it: it slides in with that workspace, at
    // its place there, while the workspace left keeps window 1.
    rig.op(|l| l.set_pinned(w(2), false));
    rig.op(|l| l.focus_window(w(3)));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.set_floating_rect(w(3), Rect::new(1000.0, 500.0, 400.0, 300.0)));
    rig.animated();
    rig.wait(ms(20));
    assert_ne!(rig.drawn(3, rig.now).rect, rig.place(3).rect, "it moves");
    let left_behind = rig.layout.shown_workspaces(OUTPUT).unwrap().main.id;
    rig.op(|l| l.move_to_workspace(SEAT, "2"));
    rig.animated();
    let place = rig.place(3);
    assert_ne!(place.workspace, left_behind);
    let switch = Spring::new(
        Settings::default().workspace_switch,
        0.0,
        HEIGHT,
        0.0,
        rig.now,
        TOLERANCE,
    );
    let mut seen = 0;
    for at in samples(rig.now, switch.end()) {
        if let Some(drawn) = rig.find(3, at) {
            assert_eq!(drawn.rect.x, place.rect.x);
            assert_eq!(drawn.rect.y, moved(place.rect.y, HEIGHT - switch.value(at)));
            seen += 1;
        }
    }
    assert!(seen > 10, "window 3 slides in with its workspace");
    assert!(
        rig.find(1, rig.now).is_some(),
        "window 1 stays where it was"
    );
}

// ----- 2. The camera -----

#[test]
fn focusing_right_scrolls_only_the_camera() {
    let settings = distinct_springs();
    let mut rig = Rig::scrolling(settings);
    for n in 1..=4 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(1)));
    rig.instant();
    let view_before = rig.view_x();
    let before: Vec<Placed> = (1..=4).map(|n| rig.place(n)).collect();
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(4)));
    rig.animated();
    let view_after = rig.view_x();
    assert!(view_after > view_before + 1000.0);
    let camera = Spring::new(
        settings.camera,
        view_before,
        view_after,
        0.0,
        rig.now,
        TOLERANCE,
    );
    assert_eq!(rig.end(), Some(camera.end()));
    for at in samples(rig.now, camera.end()) {
        for (n, was) in (1..=4).zip(&before) {
            let place = rig.place(n).rect;
            // The strip does not move along itself: only the view does.
            assert_eq!(place.x + view_after, was.rect.x + view_before);
            let drawn = rig.drawn(n, at).rect;
            let lag = camera.target() - camera.value(at);
            assert_eq!(drawn.x, moved(place.x, lag), "window {n} at {at:?}");
            assert_eq!(drawn.y, place.y);
        }
    }
    rig.assert_at_rest(camera.end());
}

#[test]
fn a_column_left_of_the_focused_one_widening_or_closing_moves_nothing_on_screen() {
    let mut rig = Rig::scrolling(distinct_springs());
    for n in 1..=4 {
        rig.open(n);
    }
    rig.instant();
    let still: Vec<Rect> = (2..=4).map(|n| rig.place(n).rect).collect();
    let view = rig.view_x();
    rig.wait(ms(5));
    rig.op(|l| l.change_width(w(1), 0.2));
    rig.animated();
    assert!(rig.view_x() > view, "the strip moved under the view");
    let change = rig.now;
    rig.wait(ms(30));
    rig.op(|l| l.close_window(w(1)));
    rig.animated();
    let close = rig.now;
    let end = rig.end().unwrap_or(close).max(close + ms(300));
    for at in samples(change, end) {
        for (n, place) in (2..=4).zip(&still) {
            assert_eq!(rig.drawn(n, at).rect, *place, "window {n} at {at:?}");
        }
    }
}

// ----- 3. Fullscreen -----

#[test]
fn fullscreen_in_or_out_is_instant_for_its_workspace() {
    let mut rig = Rig::scrolling(Settings::default());
    for n in 1..=3 {
        rig.open(n);
    }
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(1)));
    rig.animated();
    rig.wait(ms(20));
    assert!(rig.end().is_some());
    rig.op(|l| l.set_fullscreen(w(1), true));
    rig.animated();
    rig.assert_at_rest(rig.now);
    // Out of fullscreen while the camera moves on to another column.
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(3)));
    rig.animated();
    rig.wait(ms(20));
    assert!(rig.end().is_some());
    rig.op(|l| l.set_fullscreen(w(1), false));
    rig.animated();
    rig.assert_at_rest(rig.now);
}

// ----- 4. The workspace switch -----

#[test]
fn a_switch_slides_the_workspace_left_out_and_the_one_shown_in_and_nothing_between() {
    let settings = Settings::default();
    let mut rig = three_workspaces(settings);
    let one = rig.workspace("1");
    let leaving = rig.peek(one, 1).rect;
    rig.wait(ms(5));
    rig.op(|l| l.focus_workspace(SEAT, "3"));
    rig.animated();
    let entering = rig.place(3).rect;
    // The stack is [1, 3]: workspace 2 between them is left out, 3 comes in from below.
    let position = Spring::new(
        settings.workspace_switch,
        0.0,
        HEIGHT,
        0.0,
        rig.now,
        TOLERANCE,
    );
    assert_eq!(rig.end(), Some(position.end()));
    let output = rig.layout.output_rect(OUTPUT).unwrap();
    let mut both = false;
    for at in samples(rig.now, position.end()) {
        let frame = rig.frame(at);
        assert!(frame.windows.iter().all(|d| d.window != w(2)));
        let p = position.value(at);
        // A window of a workspace sliding is listed exactly while it shows on the output.
        for (n, place, slide) in [(1, leaving, -p), (3, entering, HEIGHT - p)] {
            let expected = Rect {
                y: moved(place.y, slide),
                ..place
            };
            let drawn = rig.find(n, at).map(|d| d.rect);
            let shown = expected.overlaps(&output).then_some(expected);
            assert_eq!(drawn, shown, "window {n} at {at:?}");
        }
        both |= rig.find(1, at).is_some() && rig.find(3, at).is_some();
    }
    assert!(both, "both workspaces are drawn on the way");
    rig.assert_at_rest(position.end());

    // Back up to workspace 1: it comes in from above.
    rig.now = position.end() + ms(5);
    rig.op(|l| l.focus_workspace(SEAT, "1"));
    rig.animated();
    let back = Spring::new(
        settings.workspace_switch,
        HEIGHT,
        0.0,
        0.0,
        rig.now,
        TOLERANCE,
    );
    let place = rig.place(1).rect;
    let mid = rig.now + ms(40);
    assert_eq!(rig.drawn(1, mid).rect.y, moved(place.y, -back.value(mid)));
    assert!(rig.drawn(1, mid).rect.y < place.y);
}

#[test]
fn a_switch_retargeted_between_keeps_the_nearer_workspace_where_it_is() {
    let settings = Settings::default();
    let mut rig = three_workspaces(settings);
    let (two, three) = (rig.workspace("2"), rig.workspace("3"));
    rig.wait(ms(5));
    rig.op(|l| l.focus_workspace(SEAT, "3"));
    rig.animated();
    let first = Spring::new(
        settings.workspace_switch,
        0.0,
        HEIGHT,
        0.0,
        rig.now,
        TOLERANCE,
    );
    rig.wait(ms(80));
    let turn = rig.now;
    assert!(
        first.value(turn) > HEIGHT / 2.0,
        "workspace 3 is the nearer"
    );
    let three_before = rig.drawn(3, turn).rect;
    assert!(rig.find(1, turn).is_some());
    // Workspace 2 goes into the stack between 1 and 3: [1, 2, 3].
    rig.op(|l| l.focus_workspace(SEAT, "2"));
    rig.animated();
    assert_eq!(rig.drawn(3, turn).rect, three_before);
    assert!(
        rig.find(1, turn).is_none(),
        "workspace 1 is now two heights away"
    );
    let position = first.shifted(HEIGHT).retarget(turn, HEIGHT);
    assert_eq!(rig.end(), Some(position.end()));
    let (place2, place3) = (rig.place(2).rect, rig.peek(three, 3).rect);
    assert_eq!(rig.place(2).workspace, two);
    for at in samples(turn, position.end()) {
        let p = position.value(at);
        if let Some(drawn) = rig.find(2, at) {
            assert_eq!(drawn.rect.y, moved(place2.y, HEIGHT - p), "at {at:?}");
        }
        if let Some(drawn) = rig.find(3, at) {
            assert_eq!(drawn.rect.y, moved(place3.y, 2.0 * HEIGHT - p), "at {at:?}");
        }
    }
    // Workspace 2 comes in from above, where it lies.
    assert!(rig.drawn(2, turn).rect.y < place2.y);
    // The velocity is kept: workspace 3 goes on up a little before it turns back down.
    assert!(rig.drawn(3, turn + ms(4)).rect.y < three_before.y);
    rig.assert_at_rest(position.end());
}

#[test]
fn a_workspace_gone_mid_switch_leaves_its_stack_and_nothing_shown_jumps() {
    let settings = Settings::default();
    let mut rig = Rig::scrolling(settings);
    for name in ["a", "b", "c"] {
        rig.op(|l| l.add_workspace(SEAT, name, Some(OUTPUT)));
    }
    for (n, name) in [(1, "b"), (2, "c")] {
        rig.op(|l| l.focus_workspace(SEAT, name));
        rig.open(n);
    }
    rig.op(|l| l.focus_workspace(SEAT, "a"));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.focus_workspace(SEAT, "b"));
    rig.animated();
    rig.wait(ms(30));
    rig.op(|l| l.focus_workspace(SEAT, "c"));
    rig.animated();
    // The stack is [a, b, c], the view between b and c.
    rig.wait(ms(70));
    let at = rig.now;
    let (one, two) = (rig.drawn(1, at).rect, rig.drawn(2, at).rect);
    // Workspace a, empty and unnamed, goes: the stack becomes [b, c], and the view moves up a
    // height with it.
    rig.op(|l| l.remove_workspace(SEAT, "a"));
    rig.animated();
    assert_eq!(rig.layout.workspaces(OUTPUT).unwrap().len(), 2);
    assert_eq!(rig.drawn(1, at).rect, one);
    assert_eq!(rig.drawn(2, at).rect, two);
    let end = rig.end().unwrap();
    let mut previous = two.y;
    for at in samples(at, end) {
        let y = rig.drawn(2, at).rect.y;
        assert!(
            (y - previous).abs() < 60.0,
            "a step of {} at {at:?}",
            y - previous
        );
        previous = y;
    }
    rig.assert_at_rest(end);
}

#[test]
fn pinned_windows_stay_above_and_out_of_the_switch() {
    let mut rig = three_workspaces(Settings::default());
    rig.open_floating(9);
    rig.op(|l| l.set_pinned(w(9), true));
    rig.instant();
    let pinned = rig.place(9).rect;
    rig.wait(ms(5));
    rig.op(|l| l.focus_workspace(SEAT, "3"));
    rig.animated();
    let end = rig.end().unwrap();
    for at in samples(rig.now, end) {
        let frame = rig.frame(at);
        let top = frame.windows.last().unwrap();
        assert_eq!((top.window, top.rect), (w(9), pinned), "at {at:?}");
    }
}

#[test]
fn a_workspace_removed_elsewhere_starts_no_switch() {
    let mut rig = Rig::scrolling(Settings::default());
    for name in ["a", "b"] {
        rig.op(|l| l.add_workspace(SEAT, name, Some(OUTPUT)));
    }
    rig.op(|l| l.focus_workspace(SEAT, "b"));
    rig.open(1);
    rig.instant();
    let shown = rig.layout.shown_workspaces(OUTPUT).unwrap().main.id;
    let index = |rig: &Rig| {
        rig.layout
            .workspaces(OUTPUT)
            .unwrap()
            .iter()
            .position(|ws| ws.shown)
    };
    assert_eq!(index(&rig), Some(1));
    rig.wait(ms(5));
    // "a", empty, loses its name and goes: "b" is now first, still shown.
    rig.op(|l| l.remove_workspace(SEAT, "a"));
    rig.animated();
    assert_eq!(index(&rig), Some(0));
    assert_eq!(rig.layout.shown_workspaces(OUTPUT).unwrap().main.id, shown);
    rig.assert_at_rest(rig.now);
}

// ----- 5. The special workspace -----

#[test]
fn the_special_workspace_fades_in_and_out_between_the_main_one_and_pinned_windows() {
    let settings = Settings::default();
    let fade = settings.special_fade;
    let mut rig = Rig::scrolling(settings);
    rig.open(1);
    rig.open_floating(9);
    rig.op(|l| l.set_pinned(w(9), true));
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.open(5);
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.instant();
    assert!(rig.find(5, rig.now).is_none());

    // In.
    rig.wait(ms(5));
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.animated();
    let shown = rig.now;
    let fade_in = Curve::new(fade, 0.0, 1.0, shown);
    assert_eq!(rig.end(), Some(shown + ms(200)));
    let frame = rig.frame(shown + ms(100));
    let order: Vec<WindowId> = frame.windows.iter().map(|d| d.window).collect();
    assert_eq!(order, [w(1), w(5), w(9)]);
    let alphas: Vec<f64> = frame.windows.iter().map(|d| d.alpha).collect();
    assert_eq!(alphas, [1.0, fade_in.value(shown + ms(100)), 1.0]);
    assert_eq!(alphas[1], 1.0 - 0.5_f64.powi(3));
    rig.assert_at_rest(shown + ms(200));

    // Out: its window is drawn at its last place while it fades.
    rig.now = shown + ms(300);
    let last = rig.place(5);
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.animated();
    let hidden = rig.now;
    assert!(
        rig.layout
            .arrange(OUTPUT)
            .unwrap()
            .iter()
            .all(|p| p.window != w(5))
    );
    let fade_out = Curve::new(fade, 1.0, 0.0, hidden);
    let halfway = rig.drawn(5, hidden + ms(100));
    assert_eq!(halfway.rect, last.rect);
    assert_eq!(halfway.placed, last);
    assert_eq!(halfway.alpha, fade_out.value(hidden + ms(100)));
    let order: Vec<WindowId> = rig
        .frame(hidden + ms(100))
        .windows
        .iter()
        .map(|d| d.window)
        .collect();
    assert_eq!(order, [w(1), w(5), w(9)]);

    // Toggled back mid-fade: the curve turns from where it is.
    rig.now = hidden + ms(100);
    let alpha = rig.drawn(5, rig.now).alpha;
    rig.op(|l| l.toggle_special(SEAT, "s"));
    rig.animated();
    assert_eq!(rig.drawn(5, rig.now).alpha, alpha);
    let back = fade_out.retarget(rig.now, 1.0);
    assert_eq!(rig.end(), Some(rig.now + ms(175)));
    assert_eq!(back.end(), rig.now + ms(175));
    let later = rig.now + ms(50);
    assert_eq!(rig.drawn(5, later).alpha, back.value(later));
    rig.assert_at_rest(back.end());
}

// ----- 6. Instant -----

#[test]
fn an_instant_update_or_animations_off_ends_every_motion() {
    let start_a_motion = |settings: Settings| {
        let mut rig = Rig::scrolling(settings);
        for n in 1..=3 {
            rig.open(n);
        }
        rig.instant();
        rig.wait(ms(5));
        rig.op(|l| l.focus_window(w(1)));
        rig.animated();
        rig
    };
    let mut rig = start_a_motion(Settings::default());
    rig.wait(ms(20));
    assert!(rig.end().is_some());
    rig.instant();
    rig.assert_at_rest(rig.now);

    let rig = start_a_motion(Settings {
        off: true,
        ..Settings::default()
    });
    rig.assert_at_rest(rig.now);

    let mut rig = start_a_motion(Settings::default());
    rig.wait(ms(20));
    rig.motion.set_settings(Settings::default());
    rig.assert_at_rest(rig.now);
}

// ----- 7. The end -----

/// The last three frames at 240 Hz before the end move no corner more than a pixel each, and
/// the end draws the arrangement.
#[track_caller]
fn assert_ends_softly(rig: &Rig) {
    let end = rig.end().expect("a motion runs");
    let frames: Vec<Frame> = (0..=3)
        .rev()
        .map(|k| rig.frame(end - FRAME_AT_240_HZ * k))
        .collect();
    for pair in frames.windows(2) {
        assert_eq!(pair[0].windows.len(), pair[1].windows.len());
        for (a, b) in pair[0].windows.iter().zip(&pair[1].windows) {
            assert_eq!(a.window, b.window);
            let step = (a.rect.x - b.rect.x).abs().max((a.rect.y - b.rect.y).abs());
            assert!(step <= 1.0, "{:?} moves {step} in a frame", a.window);
        }
    }
    assert_eq!(
        rig.motion.end(OUTPUT, end - Duration::from_nanos(1)),
        Some(end)
    );
    rig.assert_at_rest(end);
}

#[test]
fn a_long_camera_move_ends_without_a_jump() {
    let mut rig = Rig::scrolling(Settings::default());
    for n in 1..=100 {
        rig.open(n);
    }
    rig.instant();
    let view = rig.view_x();
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(1)));
    rig.animated();
    assert!(view - rig.view_x() > 90_000.0, "99 columns away");
    assert_ends_softly(&rig);
}

#[test]
fn short_moves_end_without_a_jump() {
    let mut rig = Rig::scrolling(Settings::default());
    for n in 1..=3 {
        rig.open(n);
    }
    rig.op(|l| l.focus_window(w(1)));
    rig.instant();
    let view = rig.view_x();
    rig.wait(ms(5));
    // One column to the right.
    rig.op(|l| l.focus_window(w(3)));
    rig.animated();
    assert!(rig.view_x() - view < 1000.0);
    assert_ends_softly(&rig);

    rig.open_floating(4);
    rig.op(|l| l.set_floating_rect(w(4), Rect::new(100.0, 100.0, 400.0, 300.0)));
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.set_floating_rect(w(4), Rect::new(103.0, 100.0, 400.0, 300.0)));
    rig.animated();
    assert_ends_softly(&rig);
}

// ----- 8. What is told -----

#[test]
fn a_window_is_told_visible_by_its_place_and_stays_told_while_the_motion_runs() {
    let mut rig = Rig::scrolling(Settings::default());
    for n in 1..=4 {
        rig.open(n);
    }
    rig.instant();
    assert_eq!(rig.told(rig.now), [3, 4]);
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(1)));
    rig.animated();
    assert_eq!(rig.told(rig.now), [1, 2, 3, 4]);
    let end = rig.end().unwrap();
    assert_eq!(rig.told(end - ms(1)), [1, 2, 3, 4]);
    assert_eq!(rig.told(end), [1, 2]);
}

#[test]
fn a_window_only_passing_through_the_view_is_never_told() {
    let mut rig = Rig::scrolling(Settings::default());
    for n in 1..=100 {
        rig.open(n);
    }
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(1)));
    rig.animated();
    let output = rig.layout.output_rect(OUTPUT).unwrap();
    let end = rig.end().unwrap();
    let mut passed = false;
    let mut at = rig.now;
    while at < end {
        let drawn = rig.drawn(50, at);
        if drawn.rect.overlaps(&output) {
            passed = true;
            assert!(!rig.told(at).contains(&50), "told at {at:?}");
        }
        at += ms(1);
    }
    assert!(passed, "window 50 crosses the view");
}

// ----- 9. Robustness -----

#[test]
fn times_out_of_order_count_as_the_last_update() {
    let mut rig = Rig::scrolling(Settings::default());
    for n in 1..=3 {
        rig.open(n);
    }
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(1)));
    rig.animated();
    let now = rig.now;
    assert_eq!(rig.frame(Duration::ZERO), rig.frame(now));
    assert_eq!(rig.motion.end(OUTPUT, Duration::ZERO), rig.end());
    assert_eq!(rig.told(Duration::ZERO), rig.told(now));
    // An update dated before the last one counts at its time.
    let drawn = rig.drawn(1, now).rect;
    rig.op(|l| l.focus_window(w(3)));
    rig.motion
        .update(&rig.layout, Duration::ZERO, Change::Animated);
    assert_eq!(rig.drawn(1, Duration::ZERO).rect, drawn);
    assert_eq!(rig.drawn(1, now).rect, drawn);
}

#[test]
fn outputs_and_seats_coming_and_going_never_break_it() {
    let mut rig = Rig::scrolling(Settings::default());
    for n in 1..=3 {
        rig.open(n);
    }
    rig.instant();
    rig.wait(ms(5));
    rig.op(|l| l.focus_window(w(1)));
    rig.animated();
    // An output the layout no longer knows is forgotten.
    rig.op(|l| l.remove_output(OUTPUT));
    rig.animated();
    assert!(rig.frame(rig.now).windows.is_empty());
    assert_eq!(rig.end(), None);
    assert!(rig.told(rig.now).is_empty());
    // Back, at another scale: at rest at once.
    let config = OutputConfig {
        position: Point { x: 0.0, y: 0.0 },
        physical_width: 2560,
        physical_height: 1440,
        scale: 1.5,
    };
    rig.op(|l| l.add_output(SEAT, OUTPUT, config));
    rig.animated();
    rig.assert_at_rest(rig.now);
    // Moving on and off: nothing breaks, nothing is drawn off its numbers.
    for _ in 0..5 {
        rig.op(|l| l.focus_window(w(3)));
        rig.wait(ms(7));
        rig.animated();
        for at in samples(rig.now, rig.now + ms(60)) {
            for drawn in rig.frame(at).windows {
                let r = drawn.rect;
                assert!(
                    [r.x, r.y, r.w, r.h, drawn.alpha]
                        .iter()
                        .all(|v| v.is_finite())
                );
            }
        }
        rig.op(|l| l.focus_window(w(1)));
    }
    assert!(rig.motion.frame(OutputId(77), rig.now).windows.is_empty());
    assert_eq!(rig.motion.end(OutputId(77), rig.now), None);
    // The seat goes, its outputs with it, once its windows are closed.
    for n in 1..=3 {
        rig.op(|l| l.close_window(w(n)));
    }
    rig.animated();
    rig.op(|l| l.remove_seat(SEAT));
    rig.animated();
    assert!(rig.frame(rig.now).windows.is_empty());
}

#[test]
fn the_default_settings_are_valid_parameters() {
    let settings = Settings::default();
    assert!(!settings.off);
    assert_eq!(settings.camera, SpringParams::new(1.0, 800.0).unwrap());
    assert_eq!(settings.windows, SpringParams::new(1.0, 800.0).unwrap());
    assert_eq!(
        settings.workspace_switch,
        SpringParams::new(1.0, 1000.0).unwrap()
    );
    assert_eq!(
        settings.special_fade,
        CurveParams::new(Easing::EaseOutCubic, ms(200)).unwrap()
    );
}
