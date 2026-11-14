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
use yuna::yuna_layout::{Direction, OutputId, SeatId, WindowId};
use yuna::yuna_motion::{Change, Drawn, Motion, Settings, SpringParams};
use yuna::{
    BUTTON_LEFT, CaptureSize, CaptureTarget, ClockMode, Compositor, Config, OWNER_SEAT, OutputMode,
};

use crate::client::{Client, Feedback, State};
use crate::input_client::{InputClient, InputEvent};
use crate::{
    BLUE, LEAD, OUTPUT, RED, advance_to, assert_rests, boundary, first_boundary_at_or_after,
};

const GREEN: u32 = 0xFF_00_FF_00;

/// Animations on: critical springs of stiffness 800 for the camera and the windows, the other
/// motions at their defaults.
fn animations() -> Settings {
    let spring = SpringParams::new(1.0, 800.0).expect("a valid spring");
    Settings {
        off: false,
        camera: spring,
        windows: spring,
        ..Settings::default()
    }
}

fn full_hd() -> OutputMode {
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
fn settle(compositor: &mut Compositor, mut read: impl FnMut(&mut Compositor)) {
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
fn commit_as_asked(compositor: &mut Compositor, client: &mut Client, window: usize, colour: u32) {
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
fn open(compositor: &mut Compositor, client: &mut Client, colour: u32) -> usize {
    let window = client.open_window(compositor);
    commit_as_asked(compositor, client, window, colour);
    settle(compositor, |compositor| client.roundtrip(compositor));
    window
}

/// Opens a column of each colour, in order, then makes each as wide as the working area and
/// commits it at that size, so that every focus move scrolls: the client's windows 0, 1, 2…,
/// whose ids are 1, 2, 3…, the last focused.
fn open_full_width_columns(
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
fn window_id(window: usize) -> WindowId {
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
fn arrangement(compositor: &Compositor, output: OutputId) -> Vec<(WindowId, Rect, f64)> {
    compositor
        .layout()
        .arrange(output)
        .expect("the output is arranged")
        .iter()
        .map(|placed| (placed.window, placed.rect, 1.0))
        .collect()
}

fn as_drawn(drawn: &[Drawn]) -> Vec<(WindowId, Rect, f64)> {
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

/// A move on an idle output 2 ms after boundary k is drawn at once, aimed at boundary k + 1, but
/// its drawing, 3 ms by the lead, would end after it: its motion is sampled at boundary k + 2.
#[test]
fn a_frame_drawn_at_once_samples_the_boundary_its_drawing_reaches() {
    let settings = animations();
    let mut compositor = compositor_with(settings);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    open_full_width_columns(&mut compositor, &mut client, &[RED, GREEN]);
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
    advance_to(&mut compositor, &mut client, operation);
    let reached = reference.frame(OUTPUT, boundary(start, k + 2)).windows;
    assert_eq!(compositor.drawn(OUTPUT), reached.as_slice());
    assert_ne!(
        reached,
        reference.frame(OUTPUT, boundary(start, k + 1)).windows
    );
    settle(&mut compositor, |compositor| client.roundtrip(compositor));
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
