//! The headless compositor in the test's own process: the manual clock, test clients written with
//! `wayland-client`, and exact figures (ADR-0023, ADR-0025, ADR-0026).
//!
//! Every story runs on the default output: 3840 × 2160 at 240 Hz and scale 1.5, so 2560 × 1440
//! logical units, with gaps of 16. A new column is half the width: (2560 − 3 × 16) / 2 = 1256
//! wide, 1440 − 2 × 16 = 1408 tall. Refresh boundary `k` comes `k / 240` s after the output was
//! created, which is the clock's start: the manual clock does not move until a test moves it.
//! `scale.rs` and `dmabuf.rs` hold the stories of viewporter, fractional scale and linux-dmabuf.

mod client;
mod dmabuf;
mod scale;
mod selection;
mod selection_client;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use client::{Client, Feedback, Kind, Mode, State};
use smithay::utils::Rectangle;
use yuna::yuna_layout::{OpenOptions, OutputId, Tiling, WindowId};
use yuna::{ClockMode, Compositor, Config, Counters, Error, OWNER_SEAT, OutputMode};

const TILE: (i32, i32) = (1256, 1408);
const RED: u32 = 0xFF_FF_00_00;
const BLUE: u32 = 0xFF_00_00_FF;
/// 1 s / 240, to the nanosecond below.
const PERIOD: Duration = Duration::from_nanos(4_166_666);
const OUTPUT: OutputId = OutputId(1);

fn manual_compositor() -> Compositor {
    Compositor::new(Config {
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts")
}

/// Refresh boundary `k` of an output created at `start`, at 240 Hz.
fn boundary(start: Duration, k: u64) -> Duration {
    start + Duration::from_nanos(k * 1_000_000_000 / 240)
}

/// A frame callback's time: milliseconds, in 32 bits.
fn millis(time: Duration) -> u32 {
    time.as_millis() as u32
}

fn tiled() -> Vec<State> {
    sorted(vec![
        State::TiledLeft,
        State::TiledRight,
        State::TiledTop,
        State::TiledBottom,
    ])
}

fn tiled_and(more: &[State]) -> Vec<State> {
    let mut states = tiled();
    states.extend_from_slice(more);
    sorted(states)
}

fn sorted(mut states: Vec<State>) -> Vec<State> {
    states.sort_by_key(|state| *state as u32);
    states
}

/// Moves the clock to `to`, then lets the client read what came.
fn advance_to(compositor: &mut Compositor, client: &mut Client, to: Duration) {
    let by = to.saturating_sub(compositor.now());
    compositor.advance(by).expect("the manual clock moves");
    client.roundtrip(compositor);
}

/// A story's end: once everything is drawn and answered, 10 s of clock draw nothing, send no
/// frame callback and no configure, wake nothing, and leave no deadline armed.
fn assert_rests(compositor: &mut Compositor) {
    assert_eq!(compositor.next_deadline(), None);
    let before = compositor.counters();
    compositor
        .advance(Duration::from_secs(10))
        .expect("the manual clock moves");
    let after = compositor.counters();
    assert_eq!(activity(before, after), (0, 0, 0, 0));
    assert_eq!(after.wakeups, before.wakeups);
    assert_eq!(compositor.next_deadline(), None);
}

/// Opens a window, acknowledges its first configure and commits a buffer of the tile's size
/// with a frame callback and presentation feedback; nothing is drawn yet.
fn open_and_commit(compositor: &mut Compositor, client: &mut Client, colour: u32) -> usize {
    let window = client.open_window(compositor);
    client.ack(window);
    client.attach(window, TILE.0, TILE.1, colour);
    client.request_frame(window);
    client.request_feedback(window);
    client.commit(window);
    client.roundtrip(compositor);
    window
}

/// What changed in the counters between two readings, wake-ups aside.
fn activity(before: Counters, after: Counters) -> (u64, u64, u64, u64) {
    (
        after.frames_drawn - before.frames_drawn,
        after.frame_callbacks - before.frame_callbacks,
        after.configures - before.configures,
        after.commits - before.commits,
    )
}

/// Story 1: a toplevel's first configure is its tile, 1256 × 1408, tiled on four edges, with
/// server-side decorations; once it is mapped, it is told it is activated.
#[test]
fn the_first_configure_is_the_tile_and_mapping_activates() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = client.open_window(&mut compositor);

    let window = client.window(w);
    assert_eq!(window.configures.len(), 1);
    let first = &window.configures[0];
    assert_eq!((first.width, first.height), TILE);
    assert_eq!(first.states, tiled());
    assert_eq!(window.decoration_mode, Some(Mode::ServerSide));

    client.ack(w);
    client.attach(w, TILE.0, TILE.1, RED);
    client.commit(w);
    client.roundtrip(&mut compositor);
    // The layout is arranged at the boundary, not at the commit.
    assert_eq!(client.window(w).configures.len(), 1);
    advance_to(&mut compositor, &mut client, boundary(start, 1));

    let window = client.window(w);
    assert_eq!(window.configures.len(), 2);
    let mapped = &window.configures[1];
    assert_eq!((mapped.width, mapped.height), TILE);
    assert_eq!(mapped.states, tiled_and(&[State::Activated]));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
}

/// Story 2: a buffer of the tile's size is drawn in exactly one frame, at the next boundary; its
/// frame callback comes at that boundary; its feedback says presented there, sequence 1, with
/// the output's period, VSYNC alone.
#[test]
fn one_frame_at_the_next_boundary_with_its_callback_and_feedback() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = open_and_commit(&mut compositor, &mut client, RED);
    assert_eq!(compositor.counters().frames_drawn, 0);

    advance_to(
        &mut compositor,
        &mut client,
        boundary(start, 1) - Duration::from_nanos(1),
    );
    assert_eq!(compositor.counters().frames_drawn, 0);
    assert!(client.window(w).frames.is_empty());
    assert!(client.window(w).feedback.is_empty());

    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert_eq!(compositor.counters().frames_drawn, 1);
    let window = client.window(w);
    assert_eq!(window.frames, vec![millis(boundary(start, 1))]);
    assert_eq!(
        window.feedback,
        vec![Feedback::Presented {
            time: boundary(start, 1),
            refresh: PERIOD,
            seq: 1,
            flags: Kind::Vsync,
        }]
    );

    // Nothing else is drawn until something changes.
    advance_to(&mut compositor, &mut client, boundary(start, 10));
    assert_eq!(compositor.counters().frames_drawn, 1);
}

/// A frame callback asked without new content still comes at the next boundary, with nothing
/// drawn; its feedback says presented at that boundary.
#[test]
fn a_callback_without_damage_comes_at_the_next_boundary() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert_eq!(compositor.counters().frames_drawn, 1);

    client.request_frame(w);
    client.request_feedback(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(compositor.counters().frames_drawn, 1);
    let window = client.window(w);
    assert_eq!(window.frames.last(), Some(&millis(boundary(start, 2))));
    assert_eq!(
        window.feedback.last(),
        Some(&Feedback::Presented {
            time: boundary(start, 2),
            refresh: PERIOD,
            seq: 2,
            flags: Kind::Vsync,
        })
    );
}

/// Story 3: a frame whose drawing ends after the boundary it started at is presented at the
/// following one; a frame is never presented at the boundary of the frame before it. The
/// drawing is made late with the draw hook, which passes manual time as drawing would take it.
#[test]
fn a_late_frame_is_presented_at_the_following_boundary() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    // The first frame takes 3 ms to draw, the others no time.
    let first_frame = Rc::new(Cell::new(true));
    let hook_first_frame = Rc::clone(&first_frame);
    compositor.set_draw_hook(move |clock| {
        if hook_first_frame.replace(false) {
            clock.pass(Duration::from_millis(3));
        }
    });
    let w = open_and_commit(&mut compositor, &mut client, RED);

    // Drawn at boundary 1 (4.17 ms), done at 7.17 ms: presented at boundary 2 (8.33 ms).
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert!(!first_frame.get());
    assert_eq!(
        compositor.now(),
        boundary(start, 1) + Duration::from_millis(3)
    );
    assert_eq!(
        client.window(w).feedback,
        vec![Feedback::Presented {
            time: boundary(start, 2),
            refresh: PERIOD,
            seq: 2,
            flags: Kind::Vsync,
        }]
    );
    // The callback goes out when the frame is done.
    assert_eq!(
        client.window(w).frames,
        vec![millis(boundary(start, 1) + Duration::from_millis(3))]
    );

    // The next frame, committed at 7.17 ms, is drawn at boundary 2 and done at once; boundary 2
    // already shows the frame before it, so it is presented at boundary 3.
    client.ack(w);
    client.attach(w, TILE.0, TILE.1, BLUE);
    client.request_feedback(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(compositor.counters().frames_drawn, 2);
    assert_eq!(
        client.window(w).feedback.last(),
        Some(&Feedback::Presented {
            time: boundary(start, 3),
            refresh: PERIOD,
            seq: 3,
            flags: Kind::Vsync,
        })
    );
}

/// Story 4: at rest, once a window is mapped and has drawn, 10 s of clock draw nothing, send no
/// callback and no configure, wake nothing, and leave no deadline armed.
#[test]
fn at_rest_nothing_runs() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    // As a real client: it acknowledges the activated configure and draws again.
    client.ack(w);
    client.attach(w, TILE.0, TILE.1, RED);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(compositor.counters().frames_drawn, 2);
    assert_eq!(compositor.next_deadline(), None);

    let before = compositor.counters();
    compositor
        .advance(Duration::from_secs(10))
        .expect("the manual clock moves");
    let after = compositor.counters();
    assert_eq!(activity(before, after), (0, 0, 0, 0));
    assert_eq!(after.wakeups, before.wakeups);
    assert_eq!(compositor.next_deadline(), None);
    client.roundtrip(&mut compositor);
    assert_eq!(client.window(w).configures.len(), 2);
}

/// Story 5: a second toplevel opens a new column: the first keeps its size and is no longer
/// activated, in one configure; closing the second gives the first its activated state back.
#[test]
fn a_second_window_takes_the_activation_and_gives_it_back() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let a = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert_eq!(client.window(a).configures.len(), 2);

    let b = open_and_commit(&mut compositor, &mut client, BLUE);
    let first_of_b = &client.window(b).configures[0];
    assert_eq!((first_of_b.width, first_of_b.height), TILE);
    advance_to(&mut compositor, &mut client, boundary(start, 2));

    let window_a = client.window(a);
    assert_eq!(window_a.configures.len(), 3);
    let a_now = &window_a.configures[2];
    assert_eq!((a_now.width, a_now.height), TILE);
    assert_eq!(a_now.states, tiled());
    let b_now = client.window(b).configures.last().expect("a configure");
    assert_eq!((b_now.width, b_now.height), TILE);
    assert_eq!(b_now.states, tiled_and(&[State::Activated]));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));

    client.close(b);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 3));
    let window_a = client.window(a);
    assert_eq!(window_a.configures.len(), 4);
    let a_back = &window_a.configures[3];
    assert_eq!((a_back.width, a_back.height), TILE);
    assert_eq!(a_back.states, tiled_and(&[State::Activated]));

    // Window ids are never reused: the next window is the third.
    open_and_commit(&mut compositor, &mut client, BLUE);
    advance_to(&mut compositor, &mut client, boundary(start, 4));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(3)));
}

/// Story 6: a window moved to a workspace not shown is told it is suspended; while it keeps
/// asking, it gets one frame callback a second of clock, and none is drawn for it; when it stops
/// asking, nothing stays armed; shown again, it is no longer suspended.
#[test]
fn a_hidden_window_is_suspended_and_called_once_a_second() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert_eq!(client.window(w).frames, vec![millis(boundary(start, 1))]);
    // It asks again at once, as a game does.
    client.request_frame(w);
    client.commit(w);
    client.roundtrip(&mut compositor);

    compositor.change_layout(|layout| {
        layout
            .move_to_special(OWNER_SEAT, "scratch")
            .expect("the window moves");
    });
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    let hidden = client.window(w).configures.last().expect("a configure");
    assert_eq!((hidden.width, hidden.height), TILE);
    assert_eq!(hidden.states, tiled_and(&[State::Suspended]));
    let drawn_when_hidden = compositor.counters().frames_drawn;

    // Five seconds in steps of 10 ms; the window asks again after each callback.
    let hidden_at = compositor.now();
    let mut answered = client.window(w).frames.len();
    while compositor.now() < hidden_at + Duration::from_secs(5) {
        let step = compositor.now() + Duration::from_millis(10);
        advance_to(&mut compositor, &mut client, step);
        if client.window(w).frames.len() > answered {
            answered = client.window(w).frames.len();
            client.request_frame(w);
            client.commit(w);
            client.roundtrip(&mut compositor);
        }
    }
    let since_hidden = &client.window(w).frames[1..];
    let expected: Vec<u32> = (1..=5)
        .map(|second| millis(boundary(start, 1) + Duration::from_secs(second)))
        .collect();
    assert_eq!(since_hidden, expected.as_slice());
    assert_eq!(compositor.counters().frames_drawn, drawn_when_hidden);

    // It stops asking once its last request is answered: nothing stays armed.
    advance_to(
        &mut compositor,
        &mut client,
        hidden_at + Duration::from_secs(7),
    );
    assert_eq!(client.window(w).frames.len(), 7);
    assert_eq!(compositor.next_deadline(), None);

    compositor.change_layout(|layout| {
        layout
            .toggle_special(OWNER_SEAT, "scratch")
            .expect("the special workspace shows");
    });
    // Past the next boundary: two boundaries are 4,166,666 or 4,166,667 ns apart.
    let past_next_boundary = compositor.now() + Duration::from_millis(5);
    advance_to(&mut compositor, &mut client, past_next_boundary);
    let shown = client.window(w).configures.last().expect("a configure");
    assert!(!shown.states.contains(&State::Suspended));
    assert!(shown.states.contains(&State::Activated));
}

/// Story 7a: a client that commits a size other than the one asked is drawn at its own size
/// (ADR-0026): 600 × 400 logical units are 900 × 600 pixels at scale 1.5, and the background
/// starts at the next pixel.
#[test]
fn a_window_is_drawn_at_the_size_its_client_committed() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = client.open_window(&mut compositor);
    client.ack(w);
    client.attach(w, 600, 400, RED);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 1));

    let placed = compositor
        .layout()
        .arrange(OUTPUT)
        .expect("the output exists");
    assert_eq!(placed.len(), 1);
    let rect = placed[0].rect;
    assert_eq!((rect.w, rect.h), (600.0, 400.0));
    // Asked the tile still: a client may draw smaller (ADR-0026).
    assert_eq!(
        (placed[0].configure.w, placed[0].configure.h),
        (1256.0, 1408.0)
    );

    let (x, y) = ((rect.x * 1.5).round() as i32, (rect.y * 1.5).round() as i32);
    let mut pixel = |px: i32, py: i32| -> Vec<u8> {
        compositor
            .read_pixels(OUTPUT, Rectangle::new((px, py).into(), (1, 1).into()))
            .expect("the pixel reads")
    };
    let red = vec![255, 0, 0, 255];
    let background = pixel(3839, 2159);
    assert_ne!(background, red);
    assert_eq!(pixel(x, y), red);
    assert_eq!(pixel(x + 899, y + 599), red);
    assert_eq!(pixel(x + 900, y + 300), background);
    assert_eq!(pixel(x + 450, y + 600), background);
    assert_eq!(pixel(x - 1, y), background);
}

/// Story 7b: a client that leaves in the middle of a frame, its commit taken and its boundary
/// armed, takes nothing down: the frame is drawn, the others keep running.
#[test]
fn a_client_leaving_mid_frame_leaves_the_others_running() {
    let mut compositor = manual_compositor();
    let mut staying = Client::connect(&mut compositor);
    let mut leaving = Client::connect(&mut compositor);
    let start = compositor.now();
    let a = open_and_commit(&mut compositor, &mut staying, RED);
    let b = open_and_commit(&mut compositor, &mut leaving, BLUE);
    advance_to(&mut compositor, &mut staying, boundary(start, 1));
    leaving.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));

    // The leaving client commits a new frame, then is gone before the boundary.
    leaving.ack(b);
    leaving.attach(b, TILE.0, TILE.1, RED);
    leaving.request_frame(b);
    leaving.request_feedback(b);
    leaving.commit(b);
    leaving.roundtrip(&mut compositor);
    assert!(compositor.next_deadline().is_some());
    drop(leaving);
    compositor
        .advance(boundary(start, 2) - compositor.now())
        .expect("the manual clock moves");
    assert_eq!(compositor.counters().frames_drawn, 2);

    // The display sees it gone; its window closes and the other takes the activation back.
    staying.roundtrip(&mut compositor);
    assert_eq!(
        compositor
            .layout()
            .arrange(OUTPUT)
            .expect("the output")
            .len(),
        1
    );
    advance_to(&mut compositor, &mut staying, boundary(start, 3));
    let last = staying.window(a).configures.last().expect("a configure");
    assert!(last.states.contains(&State::Activated));

    // And it keeps drawing.
    staying.ack(a);
    staying.attach(a, TILE.0, TILE.1, BLUE);
    staying.request_frame(a);
    staying.commit(a);
    staying.roundtrip(&mut compositor);
    let frames_before = staying.window(a).frames.len();
    advance_to(&mut compositor, &mut staying, boundary(start, 4));
    assert_eq!(staying.window(a).frames.len(), frames_before + 1);
}

/// An output larger than the renderer can draw is refused when it is created.
#[test]
fn an_output_beyond_the_renderers_limit_is_refused() {
    let too_wide = OutputMode::new(1_000_000, 1_000, 60.0, 1.0).expect("valid as a mode");
    let started = Compositor::new(Config {
        outputs: vec![too_wide],
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    });
    assert!(matches!(started, Err(Error::InvalidOutput(_))));
}

/// Outputs are numbered from 1 in the order given, and laid out left to right in logical units.
#[test]
fn outputs_are_numbered_and_laid_out_left_to_right() {
    let compositor = Compositor::new(Config {
        outputs: vec![
            OutputMode::default(),
            OutputMode::new(1920, 1080, 60.0, 1.0).expect("a valid mode"),
        ],
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts");
    assert_eq!(compositor.outputs(), vec![OutputId(1), OutputId(2)]);
    let second = compositor
        .layout()
        .output_rect(OutputId(2))
        .expect("the second output");
    assert_eq!(
        (second.x, second.y, second.w, second.h),
        (2560.0, 0.0, 1920.0, 1080.0)
    );
}

/// A client asking a minimum beyond what the layout takes still opens: its limits are clamped,
/// never a reason to leave it unmapped.
#[test]
fn a_window_asking_an_absurd_minimum_still_opens() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = client.create_window();
    client.set_min_size(w, 2_000_000, 2_000_000);
    client.commit(w);
    client.roundtrip(&mut compositor);
    assert_eq!(client.window(w).configures.len(), 1);
    client.ack(w);
    client.attach(w, TILE.0, TILE.1, RED);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    assert_eq!(compositor.counters().frames_drawn, 1);
}

/// A client that destroys a window's surface before its toplevel closes the window: it leaves
/// the layout, and the other window gets the activation back.
#[test]
fn a_window_whose_surface_goes_first_closes() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let a = open_and_commit(&mut compositor, &mut client, RED);
    let b = open_and_commit(&mut compositor, &mut client, BLUE);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));

    client.destroy_surface(b);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    let placed = compositor
        .layout()
        .arrange(OUTPUT)
        .expect("the output exists");
    assert_eq!(placed.len(), 1);
    assert_eq!(placed[0].window, WindowId(1));
    let last = client.window(a).configures.last().expect("a configure");
    assert_eq!(last.states, tiled_and(&[State::Activated]));
}

/// A subsurface added to a shown window, in the frame where another window opens, gets
/// `wl_surface.enter`: the arrangement keeps its window on its output and sends none.
#[test]
fn a_new_subsurface_enters_its_output_while_the_layout_waits() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let a = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));

    open_and_commit(&mut compositor, &mut client, BLUE);
    let sub = client.add_subsurface(a);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(client.subsurface(sub).enters, 1);
}

/// A compositor without an output is refused.
#[test]
fn a_compositor_without_an_output_is_refused() {
    let started = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    });
    assert!(matches!(started, Err(Error::Refused(_))));
}

/// A window the layout refuses to open is not shown, but its client gets its frame callbacks at
/// the hidden windows' rate, once a second, rather than waiting for ever. The layout refuses it
/// here because its id is taken: Yuna's own checks keep clients from reaching a refusal.
#[test]
fn a_window_the_layout_refuses_still_gets_frame_callbacks() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    compositor
        .change_layout(|layout| layout.open_window(OWNER_SEAT, WindowId(1), OpenOptions::default()))
        .expect("the id is taken");
    let w = open_and_commit(&mut compositor, &mut client, RED);
    let committed_at = compositor.now();
    advance_to(&mut compositor, &mut client, committed_at);
    assert_eq!(client.window(w).frames, vec![millis(committed_at)]);
    assert_eq!(client.window(w).feedback, vec![Feedback::Discarded]);

    client.request_frame(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    let one_second = committed_at + Duration::from_secs(1);
    advance_to(
        &mut compositor,
        &mut client,
        one_second - Duration::from_millis(1),
    );
    assert_eq!(client.window(w).frames.len(), 1);
    advance_to(&mut compositor, &mut client, one_second);
    assert_eq!(
        client.window(w).frames,
        vec![millis(committed_at), millis(one_second)]
    );
    assert_eq!(compositor.next_deadline(), None);
}

/// A window whose geometry starts inside its surface, as a toolkit's shadow makes it, at scale
/// 1.5: 13 and 9 logical units are 19.5 and 13.5 pixels. Its content's first pixel is at its
/// rectangle's corner, whole, and nothing of its margin shows around it.
#[test]
fn a_geometry_inside_its_surface_starts_at_the_tile_corner() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = client.open_window(&mut compositor);
    client.ack(w);
    client.attach_with_margin(w, (600, 400), (13, 9), RED, BLUE);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 1));

    let placed = compositor
        .layout()
        .arrange(OUTPUT)
        .expect("the output exists");
    let rect = placed[0].rect;
    assert_eq!((rect.w, rect.h), (600.0, 400.0));
    let (x, y) = ((rect.x * 1.5).round() as i32, (rect.y * 1.5).round() as i32);
    let mut pixel = |px: i32, py: i32| -> Vec<u8> {
        compositor
            .read_pixels(OUTPUT, Rectangle::new((px, py).into(), (1, 1).into()))
            .expect("the pixel reads")
    };
    let red = vec![255, 0, 0, 255];
    let background = pixel(3839, 2159);
    assert_eq!(pixel(x, y), red);
    assert_eq!(pixel(x + 1, y + 1), red);
    assert_eq!(pixel(x - 1, y), background);
    assert_eq!(pixel(x, y - 1), background);
    assert_eq!(pixel(x - 1, y - 1), background);
}

/// A request the layout takes but that changes nothing still gets a configure, as xdg-shell
/// asks: here, leaving a maximised state the window is not in.
#[test]
fn a_request_that_changes_nothing_is_answered() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    let before = client.window(w).configures.clone();

    client.unset_maximized(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    let after = &client.window(w).configures;
    assert_eq!(after.len(), before.len() + 1);
    let (last, previous) = (&after[after.len() - 1], &before[before.len() - 1]);
    assert_eq!(
        (last.width, last.height, &last.states),
        (previous.width, previous.height, &previous.states)
    );
}

/// A request the layout refuses gets a configure at once, of the state the window has: in
/// dwindle, a window behind a fullscreen one cannot take the screen.
#[test]
fn a_refused_request_is_answered_at_once() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    compositor
        .change_layout(|layout| layout.set_tiling(OWNER_SEAT, Tiling::Dwindle))
        .expect("the workspace tiles in dwindle");
    let game = open_and_commit(&mut compositor, &mut client, RED);
    client.set_fullscreen(game, true);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    let behind = open_and_commit(&mut compositor, &mut client, BLUE);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    let before = client.window(behind).configures.clone();

    client.set_fullscreen(behind, true);
    client.roundtrip(&mut compositor);
    let after = &client.window(behind).configures;
    assert_eq!(after.len(), before.len() + 1);
    let last = &after[after.len() - 1];
    assert!(!last.states.contains(&State::Fullscreen));
    assert_eq!(
        compositor
            .layout()
            .window(WindowId(2))
            .map(|info| info.fullscreen),
        Some(false)
    );
}

/// Fullscreen asked after the first configure, before the first buffer, gets a new configure:
/// the size and state the window will open with.
#[test]
fn a_request_before_mapping_is_answered_with_the_opening_size() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let w = client.open_window(&mut compositor);
    assert_eq!(client.window(w).configures.len(), 1);

    client.set_fullscreen(w, true);
    client.roundtrip(&mut compositor);
    let window = client.window(w);
    assert_eq!(window.configures.len(), 2);
    let second = &window.configures[1];
    assert_eq!((second.width, second.height), (2560, 1440));
    assert_eq!(second.states, tiled_and(&[State::Fullscreen]));
}
