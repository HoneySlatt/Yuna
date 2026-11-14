//! Motion in the compositor ([ADR-0054](../../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md),
//! [ADR-0055](../../../../docs/adr/0055-what-moves-first-and-animations-off-headless.md)): with
//! animations on, each frame draws what `yuna-motion` gives at the boundary the frame aims at;
//! every refresh while a motion runs arms the next, and the last frame is the arrangement; clients
//! are told from their places, not from what is drawn; the pointer, clicks, captures and MCP's
//! `windows` follow what is drawn.
//!
//! Every story runs on one output of 1920 × 1080 at 240 Hz and scale 1, on the manual clock, with
//! critical springs of stiffness 800 for the camera and the windows, unless it says otherwise. A
//! column is (1920 − 3 × 16) / 2 = 936 wide, or 1920 − 2 × 16 = 1888 at full width. The frames a
//! story expects come from a `Motion` of its own, fed the same layout at the same times as Yuna's.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use smithay::utils::Rectangle;
use yuna::mcp::Server;
use yuna::yuna_layout::geometry::Rect;
use yuna::yuna_layout::{Direction, Layout, OutputId, SeatId, WindowId};
use yuna::yuna_motion::{Change, Drawn, Motion, Settings, SpringParams};
use yuna::{
    BUTTON_LEFT, CaptureSize, CaptureTarget, ClockMode, Compositor, Config, OWNER_SEAT, OutputMode,
};

use crate::client::{Client, Feedback, Parent, Placement, State};
use crate::input_client::{InputClient, InputEvent};
use crate::{
    BLUE, LEAD, OUTPUT, RED, advance_to, assert_rests, boundary, first_boundary_at_or_after,
};

pub(crate) const GREEN: u32 = 0xFF_00_FF_00;

/// Animations on: critical springs of stiffness 800 for the camera and the windows, the other
/// motions at their defaults.
pub(crate) fn animations() -> Settings {
    let spring = SpringParams::new(1.0, 800.0).expect("a valid spring");
    Settings {
        off: false,
        camera: spring,
        windows: spring,
        ..Settings::default()
    }
}

pub(crate) fn full_hd() -> OutputMode {
    OutputMode::new(1920, 1080, 240.0, 1.0).expect("a valid mode")
}

/// A manual-clock compositor of one 1920 × 1080 output on the owner's seat, its animations as
/// `settings` say.
fn compositor_with(settings: Settings) -> Compositor {
    Compositor::new(Config {
        outputs: vec![full_hd()],
        clock: ClockMode::Manual,
        listen: false,
        animations: settings,
        ..Config::default()
    })
    .expect("the compositor starts")
}

/// A manual-clock compositor with animations on, no output of the owner's, and an agent seat
/// with one 1920 × 1080 output, for input and captures.
fn agent_compositor() -> (Compositor, SeatId, OutputId) {
    let mut compositor = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Manual,
        listen: false,
        animations: animations(),
        ..Config::default()
    })
    .expect("the compositor starts");
    let seat = compositor.add_seat().expect("a seat");
    let output = compositor.add_output(seat, full_hd()).expect("an output");
    (compositor, seat, output)
}

/// Runs every deadline until none is left, `read` letting the clients read after each: the
/// motions end and the compositor rests. Fails if it does not within 10 s of clock.
pub(crate) fn settle(compositor: &mut Compositor, mut read: impl FnMut(&mut Compositor)) {
    let until = compositor.now() + Duration::from_secs(10);
    while let Some(at) = compositor.next_deadline() {
        assert!(at <= until, "the compositor never rests");
        compositor
            .advance(at.saturating_sub(compositor.now()))
            .expect("the manual clock moves");
        read(compositor);
    }
}

/// As [`settle`], then a period more, nothing armed: the last frame of a motion is drawn ahead of
/// the boundary it shows at, while the motion still runs on the clock; from that boundary, its
/// end, the motion is over at any time asked.
fn settle_past_the_end(compositor: &mut Compositor, read: impl FnMut(&mut Compositor)) {
    settle(compositor, read);
    compositor
        .advance(crate::PERIOD)
        .expect("the manual clock moves");
}

/// Acknowledges the window's last configure, and commits a buffer of `colour` at the size it
/// asks.
pub(crate) fn commit_as_asked(
    compositor: &mut Compositor,
    client: &mut Client,
    window: usize,
    colour: u32,
) {
    let configure = client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.ack(window);
    client.attach(window, configure.width, configure.height, colour);
    client.commit(window);
    client.roundtrip(compositor);
}

/// Opens a window of `colour` at the size its first configure asks, and lets it settle.
pub(crate) fn open(compositor: &mut Compositor, client: &mut Client, colour: u32) -> usize {
    let window = client.open_window(compositor);
    commit_as_asked(compositor, client, window, colour);
    settle(compositor, |compositor| client.roundtrip(compositor));
    window
}

/// Opens a column of each colour, in order, then makes each as wide as the working area and
/// commits it at that size, so that every focus move scrolls: the client's windows 0, 1, 2…,
/// whose ids are 1, 2, 3…, the last focused.
pub(crate) fn open_full_width_columns(
    compositor: &mut Compositor,
    client: &mut Client,
    colours: &[u32],
) -> Vec<usize> {
    let windows: Vec<usize> = colours
        .iter()
        .map(|&colour| open(compositor, client, colour))
        .collect();
    for &window in &windows {
        compositor
            .change_layout(|layout| layout.set_full_width(window_id(window), true))
            .expect("full width");
    }
    settle(compositor, |compositor| client.roundtrip(compositor));
    for (&window, &colour) in windows.iter().zip(colours) {
        commit_as_asked(compositor, client, window, colour);
    }
    settle(compositor, |compositor| client.roundtrip(compositor));
    windows
}

/// The layout's id of a story's window: its client opened its windows in order, one client
/// alone.
pub(crate) fn window_id(window: usize) -> WindowId {
    WindowId(window as u64 + 1)
}

/// The windows of `drawn` that show on an output of rectangle `output`, with their rectangles,
/// bottom to top: what a frame puts on screen.
fn visible(drawn: &[Drawn], output: Rect) -> Vec<(WindowId, Rect)> {
    drawn
        .iter()
        .filter(|drawn| drawn.rect.overlaps(&output))
        .map(|drawn| (drawn.window, drawn.rect))
        .collect()
}

fn output_rect(compositor: &Compositor, output: OutputId) -> Rect {
    compositor
        .output_descriptions()
        .into_iter()
        .find(|description| description.id == output)
        .expect("the output")
        .rect
}

/// The places the layout gives `output`'s windows, with alpha 1: the frame of an output at rest.
pub(crate) fn arrangement(compositor: &Compositor, output: OutputId) -> Vec<(WindowId, Rect, f64)> {
    compositor
        .layout()
        .arrange(output)
        .expect("the output is arranged")
        .iter()
        .map(|placed| (placed.window, placed.rect, 1.0))
        .collect()
}

pub(crate) fn as_drawn(drawn: &[Drawn]) -> Vec<(WindowId, Rect, f64)> {
    drawn
        .iter()
        .map(|drawn| (drawn.window, drawn.rect, drawn.alpha))
        .collect()
}

/// Three full-width columns, the focus moved left twice, the second 10 boundaries into the first
/// motion: each boundary from the operation to the end has one refresh, whose frame is the
/// reference's at that boundary, drawn when what shows on the output changed, presented there in
/// sequence; the last frame is the arrangement, and then nothing more runs.
#[test]
fn each_boundary_draws_the_motion_sampled_there_until_the_arrangement() {
    let settings = animations();
    let mut compositor = compositor_with(settings);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    open_full_width_columns(&mut compositor, &mut client, &[RED, GREEN, BLUE]);
    let output = output_rect(&compositor, OUTPUT);
    let mut reference = Motion::new(settings);
    reference.update(compositor.layout(), compositor.now(), Change::Instant);
    assert_eq!(
        compositor.drawn(OUTPUT),
        reference.frame(OUTPUT, compositor.now()).windows
    );
    let mut previous = visible(compositor.drawn(OUTPUT), output);

    // The first move, 0.1 ms after boundary k on the idle output: arranged and drawn at once, its
    // frame aimed at boundary k + 1, which its drawing reaches.
    let mut k = first_boundary_at_or_after(start, compositor.now()) + 1;
    let operation = boundary(start, k) + Duration::from_micros(100);
    advance_to(&mut compositor, &mut client, operation);
    compositor
        .change_layout(|layout| layout.focus(OWNER_SEAT, Direction::Left))
        .expect("the focus moves left");
    reference.update(compositor.layout(), operation, Change::Animated);
    let second_move = k + 10;
    let mut refresh_at = operation;
    let mut boundaries = 0;
    loop {
        k += 1;
        let sample = boundary(start, k);
        let expected = reference.frame(OUTPUT, sample).windows;
        let shown = visible(&expected, output);
        // The frame's feedback, asked through a window it shows.
        let (asking, _) = *shown.first().expect("a window shows");
        let asking = asking.0 as usize - 1;
        let drawn_before = compositor.counters().frames_drawn;
        client.request_feedback(asking);
        client.commit(asking);
        // The round that reads the commit runs the refreshes due now: the first, armed at once.
        client.roundtrip(&mut compositor);
        if refresh_at > compositor.now() {
            assert_eq!(compositor.next_deadline(), Some(refresh_at), "boundary {k}");
            advance_to(&mut compositor, &mut client, refresh_at);
        }
        assert_eq!(
            compositor.drawn(OUTPUT),
            expected.as_slice(),
            "boundary {k}"
        );
        let drew = compositor.counters().frames_drawn - drawn_before;
        assert_eq!(drew, u64::from(shown != previous), "boundary {k}");
        previous = shown;
        advance_to(&mut compositor, &mut client, sample);
        assert_eq!(
            client.window(asking).feedback.last(),
            Some(&Feedback::Presented {
                time: sample,
                refresh: crate::PERIOD,
                seq: k,
                flags: crate::Kind::Vsync,
            }),
            "boundary {k}"
        );
        boundaries += 1;
        if reference.end(OUTPUT, sample).is_none() {
            break;
        }
        refresh_at = boundary(start, k + 1) - LEAD;
        if k == second_move {
            compositor
                .change_layout(|layout| layout.focus(OWNER_SEAT, Direction::Left))
                .expect("the focus moves left again");
            // Arranged at the motion's next refresh.
            reference.update(compositor.layout(), refresh_at, Change::Animated);
        }
    }
    // Two moves of 1,904 units on critical springs: about half a second.
    assert!((60..240).contains(&boundaries), "{boundaries} boundaries");
    assert_eq!(
        as_drawn(compositor.drawn(OUTPUT)),
        arrangement(&compositor, OUTPUT)
    );
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    assert_rests(&mut compositor);
}

/// Moves the focus left 2 ms after a boundary k that comes after the output's last refresh, so
/// that the output is idle, and has the second window ask presentation feedback in the round
/// that reads the operation, so that the frame drawn at once tells where it was presented. The
/// reference takes the layout at the operation, as Yuna arranges it then. Returns k.
fn move_left_2_ms_after_a_boundary(
    compositor: &mut Compositor,
    client: &mut Client,
    reference: &mut Motion,
    start: Duration,
) -> u64 {
    let k = first_boundary_at_or_after(start, compositor.now()) + 1;
    let operation = boundary(start, k) + Duration::from_millis(2);
    advance_to(compositor, client, operation);
    compositor
        .change_layout(|layout| layout.focus(OWNER_SEAT, Direction::Left))
        .expect("the focus moves left");
    reference.update(compositor.layout(), operation, Change::Animated);
    assert_eq!(compositor.next_deadline(), Some(operation));
    // Read in the round that runs the refresh due at once: the frame takes its feedback.
    client.request_feedback(1);
    client.commit(1);
    client.roundtrip(compositor);
    k
}

/// The feedback the second window got last: presented at boundary `k`.
#[track_caller]
fn assert_presented_at(client: &Client, start: Duration, k: u64) {
    assert_eq!(
        client.window(1).feedback.last(),
        Some(&Feedback::Presented {
            time: boundary(start, k),
            refresh: crate::PERIOD,
            seq: k,
            flags: crate::Kind::Vsync,
        })
    );
}

/// A move on an idle output 2 ms after boundary k is drawn at once. Its drawing is predicted to
/// take as long as the output's longest of the last half second, without the lead's floor and
/// margin: on the manual clock, with no draw hook, no time. So it samples boundary k + 1, where
/// it is presented, and the next refresh, due a lead before boundary k + 2, samples k + 2.
#[test]
fn a_frame_drawn_at_once_samples_the_boundary_its_drawing_reaches() {
    let settings = animations();
    let mut compositor = compositor_with(settings);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    open_full_width_columns(&mut compositor, &mut client, &[RED, GREEN]);
    let mut reference = Motion::new(settings);
    reference.update(compositor.layout(), compositor.now(), Change::Instant);

    let k = move_left_2_ms_after_a_boundary(&mut compositor, &mut client, &mut reference, start);
    let reached = reference.frame(OUTPUT, boundary(start, k + 1)).windows;
    assert_eq!(compositor.drawn(OUTPUT), reached.as_slice());
    assert_ne!(
        reached,
        reference.frame(OUTPUT, boundary(start, k + 2)).windows
    );
    advance_to(&mut compositor, &mut client, boundary(start, k + 1));
    assert_presented_at(&client, start, k + 1);
    let next = boundary(start, k + 2) - LEAD;
    assert_eq!(compositor.next_deadline(), Some(next));
    advance_to(&mut compositor, &mut client, next);
    assert_eq!(
        compositor.drawn(OUTPUT),
        reference.frame(OUTPUT, boundary(start, k + 2)).windows
    );
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
}

/// Every drawing takes 3 ms, by the draw hook, and one was drawn less than half a second ago. A
/// move on the idle output 2 ms after boundary k is drawn at once, predicted to end 3 ms later,
/// past boundary k + 1: it samples boundary k + 2, and its drawing, ending there indeed, is
/// presented there. The next refresh, due the drawing and the margin before boundary k + 3,
/// samples k + 3.
#[test]
fn a_frame_drawn_at_once_across_a_boundary_samples_the_next() {
    const DRAWING: Duration = Duration::from_millis(3);
    let settings = animations();
    let mut compositor = compositor_with(settings);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    open_full_width_columns(&mut compositor, &mut client, &[RED, GREEN]);
    compositor.set_draw_hook(|clock| clock.pass(DRAWING));
    // A new buffer for the second window, its last configure acknowledged already: one drawing.
    let drawn_before = compositor.counters().frames_drawn;
    let configure = client
        .window(1)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(1, configure.width, configure.height, GREEN);
    client.commit(1);
    client.roundtrip(&mut compositor);
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    assert_eq!(compositor.counters().frames_drawn, drawn_before + 1);
    let mut reference = Motion::new(settings);
    reference.update(compositor.layout(), compositor.now(), Change::Instant);

    let k = move_left_2_ms_after_a_boundary(&mut compositor, &mut client, &mut reference, start);
    assert!(
        compositor.now() > boundary(start, k + 1),
        "the drawing ends past k + 1"
    );
    let reached = reference.frame(OUTPUT, boundary(start, k + 2)).windows;
    assert_eq!(compositor.drawn(OUTPUT), reached.as_slice());
    assert_ne!(
        reached,
        reference.frame(OUTPUT, boundary(start, k + 1)).windows
    );
    advance_to(&mut compositor, &mut client, boundary(start, k + 2));
    assert_presented_at(&client, start, k + 2);
    // The drawing and the margin, 1 ms.
    let next = boundary(start, k + 3) - (DRAWING + Duration::from_millis(1));
    assert_eq!(compositor.next_deadline(), Some(next));
    advance_to(&mut compositor, &mut client, next);
    assert_eq!(
        compositor.drawn(OUTPUT),
        reference.frame(OUTPUT, boundary(start, k + 3)).windows
    );
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
}

/// A move on an idle output 2 ms after boundary k, on the manual clock where drawing takes no
/// time: its frame, drawn at once, shows at boundary k + 1, and each boundary from k + 1 to
/// k + 4 then shows a frame of its own, drawn before it: none holds a frame for two boundaries.
#[test]
fn an_at_once_frame_does_not_hold_the_next_boundary() {
    let settings = animations();
    let mut compositor = compositor_with(settings);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    open_full_width_columns(&mut compositor, &mut client, &[RED, GREEN]);
    let k = first_boundary_at_or_after(start, compositor.now()) + 1;
    let operation = boundary(start, k) + Duration::from_millis(2);
    advance_to(&mut compositor, &mut client, operation);
    compositor
        .change_layout(|layout| layout.focus(OWNER_SEAT, Direction::Left))
        .expect("the focus moves left");
    let mut drawn_before_each = Vec::new();
    for j in 1..=4 {
        let drawn = compositor.counters().frames_drawn;
        while let Some(at) = compositor
            .next_deadline()
            .filter(|&at| at < boundary(start, k + j))
        {
            advance_to(&mut compositor, &mut client, at);
        }
        drawn_before_each.push((k + j, compositor.counters().frames_drawn - drawn));
        advance_to(&mut compositor, &mut client, boundary(start, k + j));
    }
    assert!(
        drawn_before_each.iter().all(|&(_, frames)| frames == 1),
        "frames drawn before each boundary: {drawn_before_each:?}"
    );
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
}

/// A drawing of 20 ms, then `rest`, every other drawing taking no time; then a scroll on the idle
/// output 1 ms after a boundary k, its first frame drawn at once. Returns the boundaries the
/// scroll's first two frames sample, counted from k: for each, the first of k to k + 40 whose
/// reference frame it draws. The scroll then ends at the arrangement, and nothing runs.
fn first_two_samples_after_a_slow_drawing(rest: Duration) -> (Option<u64>, Option<u64>) {
    let settings = animations();
    let mut compositor = compositor_with(settings);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    open_full_width_columns(&mut compositor, &mut client, &[RED, BLUE]);
    let mut slow = true;
    compositor.set_draw_hook(move |clock| {
        if slow {
            slow = false;
            clock.pass(Duration::from_millis(20));
        }
    });
    // A new buffer for the second window, its last configure acknowledged already: one drawing.
    let drawn_before = compositor.counters().frames_drawn;
    let configure = client
        .window(1)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(1, configure.width, configure.height, RED);
    client.commit(1);
    client.roundtrip(&mut compositor);
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    assert_eq!(compositor.counters().frames_drawn, drawn_before + 1);

    let rested = compositor.now() + rest;
    advance_to(&mut compositor, &mut client, rested);
    let mut reference = Motion::new(settings);
    reference.update(compositor.layout(), compositor.now(), Change::Instant);
    let k = first_boundary_at_or_after(start, compositor.now()) + 1;
    let operation = boundary(start, k) + Duration::from_millis(1);
    advance_to(&mut compositor, &mut client, operation);
    compositor
        .change_layout(|layout| layout.focus(OWNER_SEAT, Direction::Left))
        .expect("the focus moves left");
    reference.update(compositor.layout(), operation, Change::Animated);
    assert_eq!(compositor.next_deadline(), Some(operation), "due at once");
    advance_to(&mut compositor, &mut client, operation);
    let sampled = |compositor: &Compositor| {
        let drawn = compositor.drawn(OUTPUT);
        (k..=k + 40)
            .find(|&j| reference.frame(OUTPUT, boundary(start, j)).windows == drawn)
            .map(|j| j - k)
    };
    let first = sampled(&compositor);
    let next = compositor.next_deadline().expect("the motion runs");
    advance_to(&mut compositor, &mut client, next);
    let second = sampled(&compositor);
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    assert_eq!(
        as_drawn(compositor.drawn(OUTPUT)),
        arrangement(&compositor, OUTPUT)
    );
    assert_rests(&mut compositor);
    (first, second)
}

/// Half a second after a drawing of 20 ms, which no longer counts in the lead, a scroll on the
/// idle output 1 ms after boundary k is drawn at once. Its drawing is predicted to take as long as
/// that last one, at most one period: its frame samples k + 2, which 1 ms and a period reach. Its
/// drawing takes no time, so that frame shows from k + 1. The motion's next refresh samples a
/// later boundary, k + 3, the one after: never one before the frame it follows.
#[test]
fn after_rest_the_frame_after_one_drawn_at_once_samples_a_later_boundary() {
    let (first, second) = first_two_samples_after_a_slow_drawing(Duration::from_millis(600));
    assert!(
        first < second,
        "the first frame samples k + {first:?}, the second k + {second:?}: no later boundary"
    );
    assert_eq!((first, second), (Some(2), Some(3)));
}

/// As above, the drawing of 20 ms only 100 ms before the scroll, so that it still counts in the
/// lead, which is then one period: the first frame samples k + 2, and the next k + 3.
#[test]
fn soon_after_a_slow_drawing_the_frame_after_one_drawn_at_once_samples_a_later_boundary() {
    let (first, second) = first_two_samples_after_a_slow_drawing(Duration::from_millis(100));
    assert!(
        first < second,
        "the first frame samples k + {first:?}, the second k + {second:?}: no later boundary"
    );
    assert_eq!((first, second), (Some(2), Some(3)));
}

/// Two named workspaces, a window each; switching back from the second to the first slides both:
/// mid-way both windows are drawn, at the end only the first. The window shown is told it is
/// visible once, at the switch; the one left is told `suspended` once, at the refresh whose frame
/// no longer draws it, never while one does.
#[test]
fn a_switch_slides_both_workspaces_and_hides_the_one_left_once_it_ends() {
    let mut compositor = compositor_with(animations());
    let mut client = Client::connect(&mut compositor);
    let output = output_rect(&compositor, OUTPUT);
    compositor
        .change_layout(|layout| layout.focus_workspace(OWNER_SEAT, "one"))
        .expect("workspace one");
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    let shown = open(&mut compositor, &mut client, RED);
    compositor
        .change_layout(|layout| layout.focus_workspace(OWNER_SEAT, "two"))
        .expect("workspace two");
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    let left = open(&mut compositor, &mut client, BLUE);
    let suspended = |configures: &[crate::client::Configure]| {
        configures
            .iter()
            .filter(|configure| configure.states.contains(&State::Suspended))
            .count()
    };
    assert_eq!(
        client
            .window(shown)
            .configures
            .last()
            .map(|c| c.states.contains(&State::Suspended)),
        Some(true)
    );
    let shown_before = client.window(shown).configures.len();
    let left_before = client.window(left).configures.len();

    compositor
        .change_layout(|layout| layout.focus_workspace(OWNER_SEAT, "one"))
        .expect("back to workspace one");
    let mut first_refresh = true;
    let mut both_drawn = false;
    let mut left_drawn_last = true;
    while let Some(at) = compositor.next_deadline() {
        advance_to(&mut compositor, &mut client, at);
        let windows: Vec<WindowId> = visible(compositor.drawn(OUTPUT), output)
            .into_iter()
            .map(|(window, _)| window)
            .collect();
        if first_refresh {
            // Told at the operation, once: visible, focused.
            let told = &client.window(shown).configures[shown_before..];
            assert_eq!(told.len(), 1);
            assert!(!told[0].states.contains(&State::Suspended));
            assert!(told[0].states.contains(&State::Activated));
            first_refresh = false;
        }
        both_drawn |= windows.contains(&window_id(shown)) && windows.contains(&window_id(left));
        left_drawn_last = windows.contains(&window_id(left));
        let left_suspended = suspended(&client.window(left).configures[left_before..]);
        if left_drawn_last {
            assert_eq!(left_suspended, 0, "told hidden while still drawn");
        }
    }
    assert!(both_drawn, "the slide never showed both workspaces");
    assert!(!left_drawn_last);
    assert_eq!(
        visible(compositor.drawn(OUTPUT), output),
        vec![(
            window_id(shown),
            compositor.windows()[0].place.expect("a place")
        )]
    );
    assert_eq!(client.window(shown).configures.len(), shown_before + 1);
    let left_told = &client.window(left).configures[left_before..];
    assert_eq!(suspended(left_told), 1);
    assert!(
        left_told
            .last()
            .expect("a configure")
            .states
            .contains(&State::Suspended)
    );
    assert_rests(&mut compositor);
}

/// Six full-width columns, the focus moved left five times 30 ms apart while the strip moves,
/// each move a new column entering the view: each window gets at most one configure from each
/// move and one when the motion ends, none from the frames between.
#[test]
fn moves_during_a_motion_configure_each_window_at_most_once_each() {
    let mut compositor = compositor_with(animations());
    let mut client = Client::connect(&mut compositor);
    let windows = open_full_width_columns(
        &mut compositor,
        &mut client,
        &[RED, GREEN, BLUE, RED, GREEN, BLUE],
    );
    let counts = |client: &Client| -> Vec<usize> {
        windows
            .iter()
            .map(|&window| client.window(window).configures.len())
            .collect()
    };
    let configures_before = compositor.counters().configures;
    let operation = compositor.now() + Duration::from_millis(1);
    let moves = 5;
    for step in 0..moves {
        advance_to(
            &mut compositor,
            &mut client,
            operation + Duration::from_millis(30) * step,
        );
        let before = counts(&client);
        let configures = compositor.counters().configures;
        compositor
            .change_layout(|layout| layout.focus(OWNER_SEAT, Direction::Left))
            .expect("the focus moves left");
        let arranged_at = compositor.next_deadline().expect("a refresh is armed");
        advance_to(&mut compositor, &mut client, arranged_at);
        let after = counts(&client);
        for (window, (before, after)) in before.iter().zip(&after).enumerate() {
            assert!(
                after - before <= 1,
                "move {step}: window {window} configured {} times",
                after - before
            );
        }
        // The frames until the next move tell nothing.
        let told = compositor.counters().configures;
        assert!(told - configures <= windows.len() as u64);
        advance_to(
            &mut compositor,
            &mut client,
            operation + Duration::from_millis(30) * (step + 1) - Duration::from_nanos(1),
        );
        assert_eq!(compositor.counters().configures, told, "move {step}");
        assert!(compositor.next_deadline().is_some(), "the motion runs");
    }
    let before = counts(&client);
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    let after = counts(&client);
    for (window, (before, after)) in before.iter().zip(&after).enumerate() {
        assert!(
            after - before <= 1,
            "the end: window {window} configured {} times",
            after - before
        );
    }
    let bound = windows.len() as u64 * (u64::from(moves) + 1);
    assert!(compositor.counters().configures - configures_before <= bound);
    assert_rests(&mut compositor);
}

/// Mid-scroll, the pointer moved to a point inside a window's drawn rectangle but outside its
/// place, inside another window's place, enters the window drawn there, at the point's place in
/// it as drawn; a click there, later in the motion, is first moved to the window's new place,
/// then pressed there.
#[test]
fn the_pointer_and_a_click_reach_the_window_drawn_under_them() {
    let (mut compositor, seat, output) = agent_compositor();
    let mut input = InputClient::connect(&mut compositor, seat, "agent-2");
    for colour in [RED, GREEN, BLUE] {
        open(&mut compositor, &mut input.client, colour);
    }
    // The view shows the second and third columns; the pointer rests on the third.
    compositor
        .pointer_move(seat, Some(output), 1500.0, 500.0)
        .expect("the pointer moves");
    settle(&mut compositor, |compositor| input.roundtrip(compositor));
    let (first, second) = (WindowId(1), WindowId(2));
    compositor
        .change_layout(|layout| layout.focus_window(first))
        .expect("the first window is focused");
    let rect_of = |compositor: &Compositor, window: WindowId| {
        let description = compositor
            .windows()
            .into_iter()
            .find(|description| description.id == window)
            .expect("the window");
        (
            description.rect.expect("drawn"),
            description.place.expect("placed"),
        )
    };
    // Boundary by boundary until the second window, sliding right, is drawn 200 units in.
    loop {
        let at = compositor.next_deadline().expect("the motion runs");
        compositor
            .advance(at.saturating_sub(compositor.now()))
            .expect("the manual clock moves");
        let (drawn, _) = rect_of(&compositor, second);
        if drawn.x >= 200.0 {
            break;
        }
    }
    input.roundtrip(&mut compositor);
    input.take_events();
    let (drawn, place) = rect_of(&compositor, second);
    let (x, y) = (drawn.x + 100.0, 500.0);
    let point = Rect::new(x, y, 1.0, 1.0);
    assert!(
        !place.overlaps(&point),
        "the point is in the second window's place"
    );
    let (_, first_place) = rect_of(&compositor, first);
    assert!(
        first_place.overlaps(&point),
        "the point is not in the first window's place"
    );

    compositor
        .pointer_move(seat, Some(output), x, y)
        .expect("the pointer moves");
    input.roundtrip(&mut compositor);
    let second_surface = input.client.window(1).surface.clone();
    let entered: Vec<InputEvent> = input
        .take_events()
        .into_iter()
        .filter(|event| matches!(event, InputEvent::PointerEnter { .. }))
        .collect();
    assert_eq!(entered.len(), 1);
    let InputEvent::PointerEnter {
        surface,
        x: local_x,
        y: local_y,
        ..
    } = &entered[0]
    else {
        unreachable!()
    };
    assert_eq!(*surface, second_surface);
    assert_eq!((*local_x, *local_y), (x - drawn.x, y - drawn.y));
    assert_eq!(compositor.pointer(seat).window, Some(second));

    // Two boundaries on, the window has moved under the still pointer; the click reaches it
    // where it is drawn now.
    for _ in 0..2 {
        let at = compositor.next_deadline().expect("the motion runs");
        compositor
            .advance(at.saturating_sub(compositor.now()))
            .expect("the manual clock moves");
    }
    let (drawn_now, _) = rect_of(&compositor, second);
    assert!(drawn_now.x > drawn.x);
    compositor
        .pointer_button(seat, BUTTON_LEFT, true)
        .expect("the button is pressed");
    compositor
        .pointer_button(seat, BUTTON_LEFT, false)
        .expect("the button is released");
    input.roundtrip(&mut compositor);
    let events = input.take_events();
    let pressed = events
        .iter()
        .position(|event| matches!(event, InputEvent::Button { pressed: true, .. }))
        .expect("the press reaches a client");
    assert!(
        !events[..pressed].iter().any(|event| matches!(
            event,
            InputEvent::PointerEnter { .. } | InputEvent::PointerLeave { .. }
        )),
        "{events:?}"
    );
    let moved = events[..pressed]
        .iter()
        .rev()
        .find_map(|event| match event {
            InputEvent::Motion { x, y } => Some((*x, *y)),
            _ => None,
        })
        .expect("a motion before the press");
    assert_eq!(moved, (x - drawn_now.x, y - drawn_now.y));
    assert_eq!(compositor.pointer(seat).window, Some(second));
    settle(&mut compositor, |compositor| input.roundtrip(compositor));
}

/// An agent's scroll run until nothing is armed: the last refresh drew the motion's end, ahead of
/// the boundary it shows at, which the clock has not reached. From then on, what follows what is
/// drawn reads the output at rest: `windows` gives each window its place as its rectangle and no
/// end, and the pointer moved half a unit inside the second window's place, by its right edge,
/// reaches that window.
#[test]
fn after_the_last_refresh_the_pointer_and_windows_see_the_rest() {
    let (mut compositor, seat, output) = agent_compositor();
    let mut input = InputClient::connect(&mut compositor, seat, "agent-2");
    for colour in [RED, GREEN, BLUE] {
        open(&mut compositor, &mut input.client, colour);
    }
    let mut reference = Motion::new(animations());
    reference.update(compositor.layout(), compositor.now(), Change::Instant);
    compositor
        .change_layout(|layout| layout.focus_window(WindowId(1)))
        .expect("the first window is focused");
    let arranged_at = compositor.next_deadline().expect("a refresh is armed");
    reference.update(compositor.layout(), arranged_at, Change::Animated);
    settle(&mut compositor, |compositor| input.roundtrip(compositor));
    assert_eq!(compositor.next_deadline(), None);
    assert!(
        reference.end(output, compositor.now()).is_some(),
        "the clock is past the motion's end: nothing to check"
    );

    let windows = compositor.windows();
    assert_eq!(windows.len(), 3);
    for window in &windows {
        assert_eq!(window.rect, window.place, "window {}", window.id.0);
        assert_eq!(window.motion_ends, None, "window {}", window.id.0);
    }
    let place = windows[1].place.expect("the second window is placed");
    let origin = output_rect(&compositor, output);
    compositor
        .pointer_move(
            seat,
            Some(output),
            place.right() - 0.5 - origin.x,
            place.y + 100.0 - origin.y,
        )
        .expect("the pointer moves");
    input.roundtrip(&mut compositor);
    assert_eq!(compositor.pointer(seat).window, Some(WindowId(2)));
    assert_rests(&mut compositor);
}

/// Mid-scroll, a capture of a window cuts its frame where that frame drew it, not at its place:
/// the pixels of the output there, the window's colour alone.
#[test]
fn a_windows_capture_mid_scroll_is_where_its_frame_drew_it() {
    let (mut compositor, seat, output) = agent_compositor();
    let mut client = Client::connect_to(&mut compositor, seat);
    for colour in [RED, GREEN, BLUE] {
        open(&mut compositor, &mut client, colour);
    }
    let second = WindowId(2);
    compositor
        .change_layout(|layout| layout.focus_window(WindowId(1)))
        .expect("the first window is focused");
    for _ in 0..20 {
        let at = compositor.next_deadline().expect("the motion runs");
        advance_to(&mut compositor, &mut client, at);
    }
    let ticket = compositor
        .capture(seat, CaptureTarget::Window(second), CaptureSize::Physical)
        .expect("the capture is asked");
    // Copied at the next refresh, after its frame is drawn.
    let mut captured = Vec::new();
    while captured.is_empty() {
        let at = compositor.next_deadline().expect("the motion runs");
        advance_to(&mut compositor, &mut client, at);
        captured = compositor.take_captures();
    }
    let (got, frame) = captured.pop().expect("a capture");
    assert_eq!(got, ticket);
    let frame = frame.expect("the capture is copied");
    let drawn = compositor
        .drawn(output)
        .iter()
        .find(|drawn| drawn.window == second)
        .expect("the window is drawn")
        .rect;
    let place = compositor
        .layout()
        .arrange(output)
        .expect("arranged")
        .into_iter()
        .find(|placed| placed.window == second)
        .expect("placed")
        .rect;
    assert_ne!(drawn, place);
    let (left, top) = (drawn.x.max(0.0), drawn.y.max(0.0));
    let (right, bottom) = (drawn.right().min(1920.0), drawn.bottom().min(1080.0));
    assert_eq!((frame.mapping.x, frame.mapping.y), (left, top));
    assert_eq!(
        (
            f64::from(frame.mapping.width),
            f64::from(frame.mapping.height)
        ),
        (right - left, bottom - top)
    );
    let region = Rectangle::new(
        (left as i32, top as i32).into(),
        ((right - left) as i32, (bottom - top) as i32).into(),
    );
    let read = compositor
        .read_pixels(output, region)
        .expect("the pixels are read");
    let on_output: Vec<u8> = read
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect();
    let image = frame.image();
    assert_eq!(image, on_output);
    assert!(
        image
            .as_chunks::<3>()
            .0
            .iter()
            .all(|pixel| *pixel == [0, 255, 0])
    );
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
}

/// The pixel at (100, 100), R, G, B, A: where the special workspace's window lies over the main
/// workspace's.
fn pixel(compositor: &mut Compositor) -> [u8; 4] {
    let read = compositor
        .read_pixels(OUTPUT, Rectangle::new((100, 100).into(), (1, 1).into()))
        .expect("the pixel is read");
    [read[0], read[1], read[2], read[3]]
}

/// The special workspace shown fades in over the main one: mid-fade its window is drawn at an
/// alpha between 0 and 1, red over blue; at the end, opaque. Hidden, it fades out, then is drawn
/// no more.
#[test]
fn the_special_workspace_fades_in_and_out() {
    let mut compositor = compositor_with(animations());
    let mut client = Client::connect(&mut compositor);
    open(&mut compositor, &mut client, BLUE);
    let special = open(&mut compositor, &mut client, RED);
    compositor
        .change_layout(|layout| layout.move_to_special(OWNER_SEAT, "scratch"))
        .expect("the window goes to the special workspace");
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    assert_eq!(pixel(&mut compositor), [0, 0, 255, 255]);
    let alpha_of = |compositor: &Compositor| {
        compositor
            .drawn(OUTPUT)
            .iter()
            .find(|drawn| drawn.window == window_id(special))
            .map(|drawn| drawn.alpha)
    };

    for shown in [true, false] {
        compositor
            .change_layout(|layout| layout.toggle_special(OWNER_SEAT, "scratch"))
            .expect("the special workspace toggles");
        let started = compositor.now();
        let mut mid_fade = None;
        while let Some(at) = compositor.next_deadline() {
            advance_to(&mut compositor, &mut client, at);
            if mid_fade.is_none() && compositor.now() >= started + Duration::from_millis(60) {
                mid_fade = Some((alpha_of(&compositor), pixel(&mut compositor)));
            }
        }
        let (alpha, [red, green, blue, opaque]) = mid_fade.expect("the fade lasts 200 ms");
        let alpha = alpha.expect("the special window is drawn mid-fade");
        assert!(alpha > 0.0 && alpha < 1.0, "alpha {alpha}");
        assert!(
            red > 0 && red < 255 && blue > 0 && blue < 255,
            "{red} {blue}"
        );
        assert_eq!((green, opaque), (0, 255));
        // Red over blue, at the window's alpha, to a step of the 8-bit channels.
        assert!(
            (f64::from(red) - 255.0 * alpha).abs() <= 2.0,
            "{red} at {alpha}"
        );
        if shown {
            assert_eq!(alpha_of(&compositor), Some(1.0));
            assert_eq!(pixel(&mut compositor), [255, 0, 0, 255]);
        } else {
            assert_eq!(alpha_of(&compositor), None);
            assert_eq!(pixel(&mut compositor), [0, 0, 255, 255]);
        }
        client.roundtrip(&mut compositor);
    }
    assert_rests(&mut compositor);
}

/// With animations off, the same scroll draws one frame, the arrangement, and rests, as before
/// motion.
#[test]
fn with_animations_off_a_scroll_draws_one_frame_and_rests() {
    let mut compositor = compositor_with(Settings {
        off: true,
        ..animations()
    });
    let mut client = Client::connect(&mut compositor);
    for colour in [RED, GREEN, BLUE] {
        open(&mut compositor, &mut client, colour);
    }
    let frames = compositor.counters().frames_drawn;
    compositor
        .change_layout(|layout| layout.focus_window(WindowId(1)))
        .expect("the first window is focused");
    let at = compositor.next_deadline().expect("a refresh is armed");
    advance_to(&mut compositor, &mut client, at);
    assert_eq!(compositor.counters().frames_drawn, frames + 1);
    assert_eq!(
        as_drawn(compositor.drawn(OUTPUT)),
        arrangement(&compositor, OUTPUT)
    );
    assert!(
        compositor
            .windows()
            .iter()
            .all(|w| w.rect == w.place && w.motion_ends.is_none())
    );
    assert_rests(&mut compositor);
}

/// An MCP server of its own, its compositor's animations on, driven through a socket pair as
/// `mcp.rs` drives one.
struct AnimatedMcp {
    server: Server,
    requests: UnixStream,
    replies: UnixStream,
    read: Vec<u8>,
    next_id: u64,
}

impl AnimatedMcp {
    fn new() -> AnimatedMcp {
        let compositor = Compositor::new(Config {
            outputs: Vec::new(),
            clock: ClockMode::Manual,
            listen: false,
            announce_owner_seat: false,
            animations: animations(),
            ..Config::default()
        })
        .expect("the compositor starts");
        let (input, requests) = UnixStream::pair().expect("a socket pair");
        let (output, replies) = UnixStream::pair().expect("a socket pair");
        replies.set_nonblocking(true).expect("non-blocking");
        let mut server = Server::new(
            compositor,
            OwnedFd::from(input),
            OwnedFd::from(output),
            None,
        )
        .expect("the server starts");
        server.set_seat_output(full_hd());
        AnimatedMcp {
            server,
            requests,
            replies,
            read: Vec::new(),
            next_id: 1,
        }
    }

    /// Calls a tool in 2026-07-28 and waits, up to 5 s of real time, for its result.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let params = crate::mcp::modern(json!({"name": tool, "arguments": arguments}));
        let mut line =
            json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": params})
                .to_string();
        line.push('\n');
        self.requests
            .write_all(line.as_bytes())
            .expect("the request is sent");
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            self.server
                .dispatch(Some(Duration::ZERO))
                .expect("the server dispatches");
            let mut buffer = [0; 65536];
            loop {
                match self.replies.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => self.read.extend_from_slice(&buffer[..read]),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) => panic!("the replies cannot be read: {error}"),
                }
            }
            while let Some(end) = self.read.iter().position(|&byte| byte == b'\n') {
                let line: Vec<u8> = self.read.drain(..=end).collect();
                let reply: Value = serde_json::from_slice(&line).expect("a reply is one JSON line");
                if reply["id"] == json!(id) {
                    return reply["result"].clone();
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("no reply to {tool}");
    }
}

/// `windows` mid-motion gives a window's drawn rectangle apart from its place, and when the
/// motion ends; at rest, the two are equal and the end null.
#[test]
fn mcp_windows_gives_the_drawn_rectangle_the_place_and_the_motions_end() {
    let mut mcp = AnimatedMcp::new();
    let created = mcp.call("seat", json!({"action": "create"}));
    let seat = created["structuredContent"]["seat"]
        .as_u64()
        .expect("a handle");
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    for colour in [RED, GREEN, BLUE] {
        open(mcp.server.compositor(), &mut client, colour);
    }
    settle_past_the_end(mcp.server.compositor(), |compositor| {
        client.roundtrip(compositor)
    });
    let second = |result: &Value| result["structuredContent"]["windows"][1].clone();
    let at_rest = second(&mcp.call("windows", json!({"seat": seat})));
    assert_eq!(at_rest["rect"], at_rest["place"]);
    assert_eq!(at_rest["motion_ends_ns"], Value::Null);

    let compositor = mcp.server.compositor();
    compositor
        .change_layout(|layout| layout.focus_window(WindowId(1)))
        .expect("the first window is focused");
    for _ in 0..10 {
        let at = compositor.next_deadline().expect("the motion runs");
        compositor
            .advance(at.saturating_sub(compositor.now()))
            .expect("the manual clock moves");
    }
    let now = compositor.now();
    let moving = second(&mcp.call("windows", json!({"seat": seat})));
    assert_ne!(moving["rect"], moving["place"]);
    assert_ne!(moving["rect"], at_rest["rect"]);
    let ends = moving["motion_ends_ns"].as_u64().expect("an end");
    assert!(u128::from(ends) > now.as_nanos());

    let compositor = mcp.server.compositor();
    settle_past_the_end(compositor, |compositor| client.roundtrip(compositor));
    let rested = second(&mcp.call("windows", json!({"seat": seat})));
    assert_eq!(rested["rect"], rested["place"]);
    assert_eq!(rested["place"], moving["place"]);
    assert_eq!(rested["motion_ends_ns"], Value::Null);
}

/// Makes `operation` 0.1 ms after a boundary that comes after the output's last refresh, so
/// that the output is idle: Yuna arranges the layout at once, and its first frame samples the
/// next boundary, which a drawing of a lead reaches. The reference takes the layout then, as
/// Yuna does. Returns the operation's time.
pub(crate) fn operate_on_idle(
    compositor: &mut Compositor,
    client: &mut Client,
    reference: &mut Motion,
    start: Duration,
    operation: impl FnOnce(&mut Layout),
) -> Duration {
    let k = first_boundary_at_or_after(start, compositor.now()) + 1;
    let at = boundary(start, k) + Duration::from_micros(100);
    advance_to(compositor, client, at);
    compositor.change_layout(operation);
    reference.update(compositor.layout(), at, Change::Animated);
    at
}

/// Steps the motion an operation on the idle output just began ([`operate_on_idle`]), refresh
/// by refresh until it ends, on the manual clock where drawing takes no time, and checks each
/// frame against `reference`. The first refresh is due at once and samples the boundary its
/// drawing reaches; each later one is due [`LEAD`] before the next boundary and samples it. Its
/// frame, [`Compositor::drawn`], must equal the reference's there. After each frame but the
/// last, the clock moves to its boundary and `between` runs, given the frame's index from 0; it
/// says whether it changed the layout, and the reference then takes the layout at the next
/// refresh, where Yuna arranges it. The last frame is the one whose boundary the reference's
/// motion has ended at. Returns each frame: the boundary it sampled and what it drew.
fn step_frames(
    compositor: &mut Compositor,
    client: &mut Client,
    reference: &mut Motion,
    start: Duration,
    mut between: impl FnMut(usize, &mut Compositor, &mut Client) -> bool,
) -> Vec<(u64, Vec<Drawn>)> {
    let mut frames = Vec::new();
    let mut due = compositor.now();
    let mut k = first_boundary_at_or_after(start, due + LEAD);
    loop {
        assert_eq!(
            compositor.next_deadline(),
            Some(due),
            "the refresh of boundary {k}"
        );
        advance_to(compositor, client, due);
        let sample = boundary(start, k);
        assert_eq!(
            compositor.drawn(OUTPUT),
            reference.frame(OUTPUT, sample).windows.as_slice(),
            "boundary {k}"
        );
        frames.push((k, compositor.drawn(OUTPUT).to_vec()));
        if reference.end(OUTPUT, sample).is_none() {
            return frames;
        }
        assert!(frames.len() < 1_000, "the motion never ends");
        advance_to(compositor, client, sample);
        due = boundary(start, k + 1) - LEAD;
        if between(frames.len() - 1, compositor, client) {
            reference.update(compositor.layout(), due, Change::Animated);
        }
        k += 1;
    }
}

/// The windows of a frame that show on the output, by id.
fn shown_ids(drawn: &[Drawn], output: Rect) -> Vec<WindowId> {
    visible(drawn, output)
        .into_iter()
        .map(|(window, _)| window)
        .collect()
}

/// A window on each of two named workspaces, "one" (red) and "two" (blue), "two" shown; the
/// switch to "one" is turned back to "two" ten boundaries in, while both slide. Every frame, the
/// windows of both workspaces at their drawn rectangles, equals the reference's at the boundary
/// it samples, the retarget sliding on as the reference retargets it; the frames
/// around the retarget show both workspaces; the last is the arrangement, "two" alone; then
/// nothing runs.
#[test]
fn a_switch_turned_back_mid_way_is_the_references_frame_by_frame() {
    let settings = animations();
    let mut compositor = compositor_with(settings);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let output = output_rect(&compositor, OUTPUT);
    for (name, colour) in [("one", RED), ("two", BLUE)] {
        compositor
            .change_layout(|layout| layout.focus_workspace(OWNER_SEAT, name))
            .expect("the workspace is shown");
        settle(&mut compositor, |compositor| client.roundtrip(compositor));
        open(&mut compositor, &mut client, colour);
    }
    let (one, two) = (WindowId(1), WindowId(2));
    let mut reference = Motion::new(settings);
    reference.update(compositor.layout(), compositor.now(), Change::Instant);
    assert_eq!(shown_ids(compositor.drawn(OUTPUT), output), vec![two]);

    operate_on_idle(
        &mut compositor,
        &mut client,
        &mut reference,
        start,
        |layout| {
            layout
                .focus_workspace(OWNER_SEAT, "one")
                .expect("back to workspace one");
        },
    );
    let frames = step_frames(
        &mut compositor,
        &mut client,
        &mut reference,
        start,
        |frame, compositor, _| {
            if frame != 10 {
                return false;
            }
            compositor
                .change_layout(|layout| layout.focus_workspace(OWNER_SEAT, "two"))
                .expect("workspace two again");
            true
        },
    );
    let both = |frame: &(u64, Vec<Drawn>)| shown_ids(&frame.1, output) == vec![one, two];
    assert!(
        frames[5..=15].iter().all(both),
        "a frame around the retarget showed one workspace alone"
    );
    // Both slides on a critical spring of stiffness 1,000 over 1,080 units: a quarter of a
    // second or more.
    assert!(frames.len() > 60, "{} frames", frames.len());
    assert_eq!(
        as_drawn(compositor.drawn(OUTPUT)),
        arrangement(&compositor, OUTPUT)
    );
    assert_eq!(shown_ids(compositor.drawn(OUTPUT), output), vec![two]);
    client.roundtrip(&mut compositor);
    assert_rests(&mut compositor);
}

/// The alpha of `window` in a frame; none when the frame does not draw it.
fn alpha_in(drawn: &[Drawn], window: WindowId) -> Option<f64> {
    drawn
        .iter()
        .find(|drawn| drawn.window == window)
        .map(|drawn| drawn.alpha)
}

/// A blue window on the main workspace, a red one on the hidden special workspace "scratch".
/// Returns the compositor, its client, the output's creation and the special window's id.
fn special_workspace_hidden(settings: Settings) -> (Compositor, Client, Duration, WindowId) {
    let mut compositor = compositor_with(settings);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    open(&mut compositor, &mut client, BLUE);
    let special = open(&mut compositor, &mut client, RED);
    compositor
        .change_layout(|layout| layout.move_to_special(OWNER_SEAT, "scratch"))
        .expect("the window goes to the special workspace");
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    (compositor, client, start, window_id(special))
}

/// The special workspace shown fades in, and ten boundaries in, toggled out, fades out as the
/// reference retargets its fade. Every frame's windows and
/// alphas equal the reference's at the boundary it samples; mid-fade the special window's alpha
/// is between 0 and 1, rising then falling; the last frame is the main workspace alone; then
/// nothing runs.
#[test]
fn the_special_fade_toggled_mid_way_is_the_references_frame_by_frame() {
    let settings = animations();
    let (mut compositor, mut client, start, special) = special_workspace_hidden(settings);
    let mut reference = Motion::new(settings);
    reference.update(compositor.layout(), compositor.now(), Change::Instant);
    assert_eq!(alpha_in(compositor.drawn(OUTPUT), special), None);

    operate_on_idle(
        &mut compositor,
        &mut client,
        &mut reference,
        start,
        |layout| {
            layout
                .toggle_special(OWNER_SEAT, "scratch")
                .expect("the special workspace is shown");
        },
    );
    let frames = step_frames(
        &mut compositor,
        &mut client,
        &mut reference,
        start,
        |frame, compositor, _| {
            if frame != 10 {
                return false;
            }
            compositor
                .change_layout(|layout| layout.toggle_special(OWNER_SEAT, "scratch"))
                .expect("the special workspace is hidden");
            true
        },
    );
    let alphas: Vec<f64> = frames
        .iter()
        .filter_map(|(_, drawn)| alpha_in(drawn, special))
        .collect();
    assert!(
        alphas.iter().all(|&alpha| alpha > 0.0 && alpha < 1.0),
        "{alphas:?}"
    );
    // Rising until the toggle, after frame 10; falling from the refresh that arranged it.
    assert!(
        alphas[..=10].windows(2).all(|pair| pair[0] < pair[1]),
        "{alphas:?}"
    );
    assert!(
        alphas[11..].windows(2).all(|pair| pair[0] > pair[1]),
        "{alphas:?}"
    );
    assert_eq!(alpha_in(compositor.drawn(OUTPUT), special), None);
    assert_eq!(
        as_drawn(compositor.drawn(OUTPUT)),
        arrangement(&compositor, OUTPUT)
    );
    client.roundtrip(&mut compositor);
    assert_rests(&mut compositor);
}

/// Reduced motion: nothing slides, fades stay over half their duration
/// ([ADR-0059](../../../../docs/adr/0059-reduced-motion-nothing-slides-fades-stay.md)).
fn reduced_motion() -> Settings {
    Settings {
        reduced_motion: true,
        ..animations()
    }
}

/// Under reduced motion, a scroll shows its result at once: one frame, drawn at once on the idle
/// output, the arrangement; then nothing runs.
#[test]
fn under_reduced_motion_a_scroll_draws_one_frame_at_the_arrangement() {
    let settings = reduced_motion();
    let mut compositor = compositor_with(settings);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    open_full_width_columns(&mut compositor, &mut client, &[RED, GREEN, BLUE]);
    let mut reference = Motion::new(settings);
    reference.update(compositor.layout(), compositor.now(), Change::Instant);
    let drawn_before = compositor.counters().frames_drawn;
    let placed_before = arrangement(&compositor, OUTPUT);

    operate_on_idle(
        &mut compositor,
        &mut client,
        &mut reference,
        start,
        |layout| {
            layout
                .focus(OWNER_SEAT, Direction::Left)
                .expect("the focus moves left");
        },
    );
    let frames = step_frames(
        &mut compositor,
        &mut client,
        &mut reference,
        start,
        |_, _, _| false,
    );
    assert_eq!(frames.len(), 1);
    assert_eq!(compositor.counters().frames_drawn, drawn_before + 1);
    let placed = arrangement(&compositor, OUTPUT);
    assert_ne!(placed, placed_before, "the scroll moved nothing");
    assert_eq!(as_drawn(compositor.drawn(OUTPUT)), placed);
    client.roundtrip(&mut compositor);
    assert_rests(&mut compositor);
}

/// Under reduced motion, the special workspace still fades in, over half its duration, 100 ms:
/// frame by frame as the reference, its window at its place in every frame, its alpha rising
/// below 1 until the first boundary at or after the 100 ms, where it is 1; then nothing runs.
#[test]
fn under_reduced_motion_the_special_fade_runs_over_half_its_duration() {
    let settings = reduced_motion();
    let (mut compositor, mut client, start, special) = special_workspace_hidden(settings);
    let mut reference = Motion::new(settings);
    reference.update(compositor.layout(), compositor.now(), Change::Instant);

    let operation = operate_on_idle(
        &mut compositor,
        &mut client,
        &mut reference,
        start,
        |layout| {
            layout
                .toggle_special(OWNER_SEAT, "scratch")
                .expect("the special workspace is shown");
        },
    );
    let half = Duration::from_millis(100);
    assert_eq!(reference.end(OUTPUT, operation), Some(operation + half));
    let frames = step_frames(
        &mut compositor,
        &mut client,
        &mut reference,
        start,
        |_, _, _| false,
    );
    let (last, _) = *frames.last().expect("a frame");
    assert_eq!(last, first_boundary_at_or_after(start, operation + half));
    let place = compositor
        .layout()
        .arrange(OUTPUT)
        .expect("arranged")
        .into_iter()
        .find(|placed| placed.window == special)
        .expect("the special window is placed")
        .rect;
    let mut alphas = Vec::new();
    for (k, drawn) in &frames {
        let window = drawn
            .iter()
            .find(|drawn| drawn.window == special)
            .unwrap_or_else(|| panic!("boundary {k} does not draw the special window"));
        assert_eq!(window.rect, place, "boundary {k}");
        alphas.push(window.alpha);
    }
    assert!(
        alphas.windows(2).all(|pair| pair[0] < pair[1]),
        "{alphas:?}"
    );
    assert!(alphas[..alphas.len() - 1].iter().all(|&alpha| alpha < 1.0));
    assert_eq!(alphas.last(), Some(&1.0));
    client.roundtrip(&mut compositor);
    assert_rests(&mut compositor);
}

/// Two full-width columns, red then blue, blue focused with a green popup of 200 × 100 under a
/// button at (10, 10) of it, placed at (11, 11) of its window; focusing red scrolls blue out to
/// the right. Mid-scroll, with blue drawn away from its place, the popup's pixels are at blue's
/// drawn rectangle moved by (11, 11): green from there over 200 × 100, blue around it.
#[test]
fn a_popup_mid_scroll_is_drawn_at_its_windows_drawn_rectangle() {
    let mut compositor = compositor_with(animations());
    let mut client = Client::connect(&mut compositor);
    let windows = open_full_width_columns(&mut compositor, &mut client, &[RED, BLUE]);
    let blue = window_id(windows[1]);
    let menu = Placement::below_right((200, 100), (10, 10, 1, 1));
    client.map_popup(&mut compositor, Parent::Window(windows[1]), menu, GREEN);
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    let pixel = |compositor: &mut Compositor, x: f64, y: f64| {
        let region = Rectangle::new((x as i32, y as i32).into(), (1, 1).into());
        let read = compositor
            .read_pixels(OUTPUT, region)
            .expect("the pixel is read");
        [read[0], read[1], read[2], read[3]]
    };
    let (green, blue_pixel) = ([0, 255, 0, 255], [0, 0, 255, 255]);
    // At rest, blue at (16, 16): the popup from (27, 27).
    assert_eq!(pixel(&mut compositor, 27.0, 27.0), green);
    assert_eq!(pixel(&mut compositor, 26.0, 27.0), blue_pixel);

    compositor
        .change_layout(|layout| layout.focus_window(window_id(windows[0])))
        .expect("red is focused");
    let drawn_rect = |compositor: &Compositor| {
        compositor
            .drawn(OUTPUT)
            .iter()
            .find(|drawn| drawn.window == blue)
            .map(|drawn| drawn.rect)
    };
    let rect = loop {
        let at = compositor.next_deadline().expect("the scroll runs");
        advance_to(&mut compositor, &mut client, at);
        let rect = drawn_rect(&compositor).expect("blue is drawn");
        if rect.x >= 600.0 {
            break rect;
        }
    };
    let place = compositor
        .layout()
        .arrange(OUTPUT)
        .expect("arranged")
        .into_iter()
        .find(|placed| placed.window == blue)
        .expect("blue is placed")
        .rect;
    assert_ne!(rect.x, place.x, "blue is at its place");
    assert!(rect.x + 211.0 < 1920.0, "the popup leaves the output");
    let (x, y) = (rect.x + 11.0, rect.y + 11.0);
    for (dx, dy) in [(0.0, 0.0), (199.0, 0.0), (0.0, 99.0), (199.0, 99.0)] {
        assert_eq!(pixel(&mut compositor, x + dx, y + dy), green, "{dx} {dy}");
    }
    for (dx, dy) in [(-1.0, 0.0), (0.0, -1.0), (200.0, 0.0), (0.0, 100.0)] {
        assert_eq!(
            pixel(&mut compositor, x + dx, y + dy),
            blue_pixel,
            "{dx} {dy}"
        );
    }
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    assert_rests(&mut compositor);
}

/// Three full-width columns, the third focused; focusing the first scrolls two columns left.
/// Eight boundaries in, the second window, crossing the view, closes; eight boundaries later a
/// new window opens and takes the focus. Yuna does not panic; every frame
/// equals the reference's, the closed window in none from the refresh after its close; the last
/// frame is the arrangement; then nothing runs.
#[test]
fn a_window_closed_and_one_opened_mid_scroll_end_at_the_arrangement() {
    let settings = animations();
    let mut compositor = compositor_with(settings);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let windows = open_full_width_columns(&mut compositor, &mut client, &[RED, GREEN, BLUE]);
    let closed = window_id(windows[1]);
    let output = output_rect(&compositor, OUTPUT);
    let mut reference = Motion::new(settings);
    reference.update(compositor.layout(), compositor.now(), Change::Instant);

    operate_on_idle(
        &mut compositor,
        &mut client,
        &mut reference,
        start,
        |layout| {
            layout
                .focus_window(window_id(windows[0]))
                .expect("the first window is focused");
        },
    );
    let mut opened = None;
    let frames = step_frames(
        &mut compositor,
        &mut client,
        &mut reference,
        start,
        |frame, compositor, client| match frame {
            8 => {
                client.close(windows[1]);
                client.roundtrip(compositor);
                true
            }
            16 => {
                let window = client.open_window(compositor);
                commit_as_asked(compositor, client, window, RED);
                opened = Some(window_id(window));
                true
            }
            _ => false,
        },
    );
    let opened = opened.expect("a window opened mid-scroll");
    assert!(frames.len() > 17, "{} frames", frames.len());
    assert!(shown_ids(&frames[8].1, output).contains(&closed));
    assert!(
        frames[..=8]
            .iter()
            .all(|(_, drawn)| drawn.iter().any(|drawn| drawn.window == closed))
    );
    for (k, drawn) in &frames[9..] {
        assert!(
            drawn.iter().all(|drawn| drawn.window != closed),
            "boundary {k} draws the closed window"
        );
    }
    assert!(
        frames[17..]
            .iter()
            .all(|(_, drawn)| drawn.iter().any(|drawn| drawn.window == opened))
    );
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(opened));
    assert_eq!(
        as_drawn(compositor.drawn(OUTPUT)),
        arrangement(&compositor, OUTPUT)
    );
    client.roundtrip(&mut compositor);
    assert_rests(&mut compositor);
}

/// Runs the deadlines armed up to a period from now, `owner` and `agent` reading after each: at
/// least one refresh of each output.
fn run_a_period(compositor: &mut Compositor, owner: &mut Client, mut agent: Option<&mut Client>) {
    let until = compositor.now() + crate::PERIOD;
    while let Some(at) = compositor.next_deadline().filter(|&at| at <= until) {
        advance_to(compositor, owner, at);
        if let Some(agent) = agent.as_deref_mut() {
            agent.roundtrip(compositor);
        }
    }
}

/// Whether `output`'s last frame is its arrangement: its motion shows its end.
fn shows_the_arrangement(compositor: &Compositor, output: OutputId) -> bool {
    as_drawn(compositor.drawn(output)) == arrangement(compositor, output)
}

/// The owner scrolls three full-width columns; six boundaries in, the focus moved left, an agent's
/// new seat gets an output. The owner's scroll goes on: at its output's next refresh, its frame
/// is not the arrangement yet, and its motion then runs to it. A new output starts at rest, and
/// makes nothing else instant
/// ([ADR-0015](../../../../docs/adr/0015-agents-workspaces.md),
/// [ADR-0052](../../../../docs/adr/0052-motion-in-a-pure-crate.md)).
#[test]
fn an_agents_new_output_does_not_snap_the_owners_motion() {
    let mut compositor = compositor_with(animations());
    let mut client = Client::connect(&mut compositor);
    open_full_width_columns(&mut compositor, &mut client, &[RED, GREEN, BLUE]);
    compositor
        .change_layout(|layout| layout.focus(OWNER_SEAT, Direction::Left))
        .expect("the focus moves left");
    for _ in 0..6 {
        let at = compositor.next_deadline().expect("the scroll runs");
        advance_to(&mut compositor, &mut client, at);
    }
    let seat = compositor.add_seat().expect("a seat");
    compositor.add_output(seat, full_hd()).expect("an output");
    run_a_period(&mut compositor, &mut client, None);
    assert!(
        !shows_the_arrangement(&compositor, OUTPUT),
        "the owner's scroll jumped to its end when an agent added an output"
    );
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    assert!(shows_the_arrangement(&compositor, OUTPUT));
    assert_rests(&mut compositor);
}

/// The owner scrolls, and so does an agent on the first of its two outputs; six boundaries in,
/// the agent removes its second output. That is instant for the agent's seat alone: at the next
/// refreshes, the agent's first output shows its arrangement, while the owner's scroll goes on,
/// then runs to its end.
#[test]
fn an_agents_output_removed_ends_its_own_seats_motion_alone() {
    let mut compositor = compositor_with(animations());
    let mut owner = Client::connect(&mut compositor);
    open_full_width_columns(&mut compositor, &mut owner, &[RED, GREEN, BLUE]);
    let seat = compositor.add_seat().expect("a seat");
    let first = compositor.add_output(seat, full_hd()).expect("an output");
    let mut agent = Client::connect_to(&mut compositor, seat);
    for colour in [RED, GREEN, BLUE] {
        open(&mut compositor, &mut agent, colour);
    }
    let second = compositor.add_output(seat, full_hd()).expect("an output");
    settle(&mut compositor, |compositor| {
        owner.roundtrip(compositor);
        agent.roundtrip(compositor);
    });
    // The owner's windows are 1 to 3, the agent's 4 to 6, its last focused.
    compositor
        .change_layout(|layout| {
            layout.focus(OWNER_SEAT, Direction::Left)?;
            layout.focus_window(WindowId(4))
        })
        .expect("both seats' focus moves");
    for _ in 0..6 {
        let at = compositor.next_deadline().expect("the scrolls run");
        advance_to(&mut compositor, &mut owner, at);
        agent.roundtrip(&mut compositor);
    }
    assert!(!shows_the_arrangement(&compositor, OUTPUT));
    assert!(!shows_the_arrangement(&compositor, first));

    compositor
        .remove_output(second)
        .expect("the agent's second output is removed");
    run_a_period(&mut compositor, &mut owner, Some(&mut agent));
    assert!(
        shows_the_arrangement(&compositor, first),
        "the agent's motion went on after its seat lost an output"
    );
    assert!(
        !shows_the_arrangement(&compositor, OUTPUT),
        "the owner's scroll jumped to its end when an agent removed an output"
    );
    settle(&mut compositor, |compositor| {
        owner.roundtrip(compositor);
        agent.roundtrip(compositor);
    });
    assert!(shows_the_arrangement(&compositor, OUTPUT));
    assert_rests(&mut compositor);
}

/// The owner scrolls; six boundaries in, an agent's seat, with an output, is removed. The owner's
/// scroll goes on, then runs to its end.
#[test]
fn an_agents_seat_removed_does_not_snap_the_owners_motion() {
    let mut compositor = compositor_with(animations());
    let mut client = Client::connect(&mut compositor);
    open_full_width_columns(&mut compositor, &mut client, &[RED, GREEN, BLUE]);
    let seat = compositor.add_seat().expect("a seat");
    compositor.add_output(seat, full_hd()).expect("an output");
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    compositor
        .change_layout(|layout| layout.focus(OWNER_SEAT, Direction::Left))
        .expect("the focus moves left");
    for _ in 0..6 {
        let at = compositor.next_deadline().expect("the scroll runs");
        advance_to(&mut compositor, &mut client, at);
    }
    compositor
        .remove_seat(seat)
        .expect("the agent's seat is removed");
    run_a_period(&mut compositor, &mut client, None);
    assert!(
        !shows_the_arrangement(&compositor, OUTPUT),
        "the owner's scroll jumped to its end when an agent's seat was removed"
    );
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
    assert!(shows_the_arrangement(&compositor, OUTPUT));
    assert_rests(&mut compositor);
}

/// The sequence number of the last frame window 1's feedback says it was presented at.
fn presented_seq(client: &Client) -> Option<u64> {
    match client.window(1).feedback.last() {
        Some(Feedback::Presented { seq, .. }) => Some(*seq),
        _ => None,
    }
}

/// As `a_frame_drawn_at_once_across_a_boundary_samples_the_next`, but the move comes after more
/// than half a second of rest, so the output has no recent drawing: the common case of a frame
/// drawn at once, the first operation after a pause. Every drawing takes 3 ms. Its drawing is
/// expected
/// to take as long as the last one, however old: it samples the boundary it is presented at.
#[test]
fn after_rest_a_frame_drawn_at_once_across_a_boundary_samples_where_it_shows() {
    const DRAWING: Duration = Duration::from_millis(3);
    let settings = animations();
    let mut compositor = compositor_with(settings);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    compositor.set_draw_hook(|clock| clock.pass(DRAWING));
    open_full_width_columns(&mut compositor, &mut client, &[RED, GREEN]);
    // Half a second and more of rest: the output's drawings are forgotten.
    let rest = compositor.now() + Duration::from_millis(600);
    advance_to(&mut compositor, &mut client, rest);
    let mut reference = Motion::new(settings);
    reference.update(compositor.layout(), compositor.now(), Change::Instant);

    let k = first_boundary_at_or_after(start, compositor.now()) + 1;
    let operation = boundary(start, k) + Duration::from_millis(2);
    advance_to(&mut compositor, &mut client, operation);
    compositor
        .change_layout(|layout| layout.focus(OWNER_SEAT, Direction::Left))
        .expect("the focus moves left");
    reference.update(compositor.layout(), operation, Change::Animated);
    assert_eq!(compositor.next_deadline(), Some(operation));
    client.request_feedback(1);
    client.commit(1);
    client.roundtrip(&mut compositor);
    assert!(
        compositor.now() > boundary(start, k + 1),
        "the drawing ends past k + 1"
    );
    let drawn = compositor.drawn(OUTPUT).to_vec();
    let sampled = (k + 1..=k + 3)
        .find(|&j| reference.frame(OUTPUT, boundary(start, j)).windows == drawn)
        .map(|j| j - k);
    advance_to(&mut compositor, &mut client, boundary(start, k + 2));
    let shown = presented_seq(&client).map(|seq| seq - k);
    // The next refresh.
    let next = compositor.next_deadline().expect("the motion runs");
    advance_to(&mut compositor, &mut client, next);
    let drawn = compositor.drawn(OUTPUT).to_vec();
    let next_sampled = (k + 1..=k + 4)
        .find(|&j| reference.frame(OUTPUT, boundary(start, j)).windows == drawn)
        .map(|j| j - k);
    assert_eq!(
        sampled, shown,
        "the frame drawn at once samples the boundary it is presented at"
    );
    assert_eq!(
        next_sampled,
        shown.map(|j| j + 1),
        "the next refresh samples the boundary after it"
    );
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
}
