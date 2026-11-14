//! The headless compositor in the test's own process: the manual clock, test clients written with
//! `wayland-client`, and exact figures (ADR-0023, ADR-0025, ADR-0026).
//!
//! Every story runs on the default output: 3840 × 2160 at 240 Hz and scale 1.5, so 2560 × 1440
//! logical units, with gaps of 16. A new column is half the width: (2560 − 3 × 16) / 2 = 1256
//! wide, 1440 − 2 × 16 = 1408 tall. Refresh boundary `k` comes `k / 240` s after the output was
//! created, which is the clock's start: the manual clock does not move until a test moves it.
//! `scale.rs` and `dmabuf.rs` hold the stories of viewporter, fractional scale and linux-dmabuf.

mod activation;
mod capture_client;
mod captures;
mod client;
mod colour;
mod colour_client;
mod dialogs;
mod dmabuf;
mod input_client;
mod mcp;
mod misbehaving;
mod motion;
mod motion_pacing;
mod outputs;
mod popups;
mod random_requests;
mod scale;
mod screencopy;
mod seats;
mod selection;
mod selection_client;
mod text_input;
mod vrr;
mod window_cap;

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
/// The lead of an output whose drawings take no time, as on the manual clock: the floor, 2 ms,
/// plus the margin, 1 ms (ADR-0031).
const LEAD: Duration = Duration::from_millis(3);

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

/// The first refresh boundary at or after `time`, at 240 Hz from `start`.
fn first_boundary_at_or_after(start: Duration, time: Duration) -> u64 {
    let mut k = 0;
    while boundary(start, k) < time {
        k += 1;
    }
    k
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

/// Runs the deadlines armed now, a refresh then its presentation, and any they arm in turn: at
/// most four, so that a deadline armed again and again fails the story's rest that follows.
fn run_armed_deadlines(compositor: &mut Compositor) {
    for _ in 0..4 {
        let Some(at) = compositor.next_deadline() else {
            return;
        };
        compositor
            .advance(at.saturating_sub(compositor.now()))
            .expect("the manual clock moves");
    }
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

/// Story 2: a buffer of the tile's size, committed as the output is created, is drawn in exactly
/// one frame, at boundary 1 minus the lead; its frame callback comes then; its feedback is held
/// until boundary 1 and says presented there, sequence 1, with the output's period, VSYNC alone.
#[test]
fn one_frame_ahead_of_the_next_boundary_with_its_callback_and_feedback() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = open_and_commit(&mut compositor, &mut client, RED);
    assert_eq!(compositor.counters().frames_drawn, 0);

    let due = boundary(start, 1) - LEAD;
    advance_to(&mut compositor, &mut client, due - Duration::from_nanos(1));
    assert_eq!(compositor.counters().frames_drawn, 0);
    assert!(client.window(w).frames.is_empty());

    advance_to(&mut compositor, &mut client, due);
    assert_eq!(compositor.counters().frames_drawn, 1);
    assert_eq!(client.window(w).frames, vec![millis(due)]);
    advance_to(
        &mut compositor,
        &mut client,
        boundary(start, 1) - Duration::from_nanos(1),
    );
    assert!(client.window(w).feedback.is_empty());

    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert_eq!(
        client.window(w).feedback,
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
    assert_rests(&mut compositor);
}

/// A frame callback asked without new content, while the output is not idle, comes at the next
/// boundary minus the lead, with nothing drawn; its feedback says presented at that boundary.
#[test]
fn a_callback_without_damage_comes_ahead_of_the_next_boundary() {
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
    assert_eq!(
        window.frames.last(),
        Some(&millis(boundary(start, 2) - LEAD))
    );
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

/// Story 3: a frame whose drawing ends after the boundary it aimed at is presented at the
/// following one, and its feedback held until then. The slow drawing lengthens the lead, at most
/// to one period, so the next frame is drawn a period ahead; half a second later the lead is
/// back to the floor and the margin. The drawing is made late with the draw hook, which passes
/// manual time as drawing would take it.
#[test]
fn a_late_frame_is_presented_at_the_following_boundary() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    // The first frame takes 4 ms to draw, longer than the lead; the others no time.
    let first_frame = Rc::new(Cell::new(true));
    let hook_first_frame = Rc::clone(&first_frame);
    compositor.set_draw_hook(move |clock| {
        if hook_first_frame.replace(false) {
            clock.pass(Duration::from_millis(4));
        }
    });
    let w = open_and_commit(&mut compositor, &mut client, RED);

    // Drawn from boundary 1 minus 3 ms, done 1 ms after boundary 1: presented at boundary 2.
    let done = boundary(start, 1) + Duration::from_millis(1);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert!(!first_frame.get());
    assert_eq!(compositor.now(), done);
    // The callback goes out when the frame is done; the feedback waits for its boundary.
    assert_eq!(client.window(w).frames, vec![millis(done)]);
    assert!(client.window(w).feedback.is_empty());

    // The next frame, committed now, while the first waits: the lead is one period, 4 ms + 1 ms
    // capped, so it is drawn one period ahead of boundary 3, the first whose deadline is not
    // past.
    client.ack(w);
    client.attach(w, TILE.0, TILE.1, BLUE);
    client.request_feedback(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    let ahead_of_3 = boundary(start, 3) - PERIOD;
    advance_to(
        &mut compositor,
        &mut client,
        ahead_of_3 - Duration::from_nanos(1),
    );
    assert_eq!(compositor.counters().frames_drawn, 1);
    advance_to(&mut compositor, &mut client, ahead_of_3);
    assert_eq!(compositor.counters().frames_drawn, 2);
    assert_eq!(
        client.window(w).feedback,
        vec![Feedback::Presented {
            time: boundary(start, 2),
            refresh: PERIOD,
            seq: 2,
            flags: Kind::Vsync,
        }]
    );
    advance_to(&mut compositor, &mut client, boundary(start, 3));
    assert_eq!(
        client.window(w).feedback.last(),
        Some(&Feedback::Presented {
            time: boundary(start, 3),
            refresh: PERIOD,
            seq: 3,
            flags: Kind::Vsync,
        })
    );

    // Half a second after the slow drawing ended, it no longer counts: the lead is 3 ms again.
    let later = done + Duration::from_millis(500);
    advance_to(
        &mut compositor,
        &mut client,
        later - Duration::from_millis(1),
    );
    client.attach(w, TILE.0, TILE.1, RED);
    client.commit(w);
    client.roundtrip(&mut compositor);
    // Idle: drawn at once, with no time taken, so it counts the floor.
    assert_eq!(compositor.counters().frames_drawn, 3);
    advance_to(&mut compositor, &mut client, later);
    client.attach(w, TILE.0, TILE.1, BLUE);
    client.commit(w);
    client.roundtrip(&mut compositor);
    let k = first_boundary_at_or_after(start, later + LEAD);
    assert_eq!(compositor.next_deadline(), Some(boundary(start, k) - LEAD));
}

/// An output idle long enough draws a commit at once, in the round that reads it: its callback
/// comes then, and its feedback at the first boundary at or after the drawing. A second commit
/// right after waits for the next boundary's deadline, which no other frame takes.
#[test]
fn an_idle_output_draws_at_once() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    // Configured activated at boundary 1: acknowledged and drawn at boundary 2.
    client.ack(w);
    client.attach(w, TILE.0, TILE.1, RED);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 3));
    let drawn = compositor.counters().frames_drawn;
    assert_eq!(compositor.next_deadline(), None);

    // Between boundaries 10 and 11.
    let now = boundary(start, 10) + Duration::from_millis(1);
    advance_to(&mut compositor, &mut client, now);
    client.attach(w, TILE.0, TILE.1, BLUE);
    client.request_frame(w);
    client.request_feedback(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.counters().frames_drawn, drawn + 1);
    assert_eq!(client.window(w).frames.last(), Some(&millis(now)));
    assert_eq!(compositor.next_deadline(), Some(boundary(start, 11)));

    client.attach(w, TILE.0, TILE.1, RED);
    client.request_feedback(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.counters().frames_drawn, drawn + 1);
    advance_to(&mut compositor, &mut client, boundary(start, 12));
    assert_eq!(compositor.counters().frames_drawn, drawn + 2);
    let presented: Vec<u64> = client
        .window(w)
        .feedback
        .iter()
        .filter_map(|feedback| match feedback {
            Feedback::Presented { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(presented[presented.len() - 2..], [11, 12]);
    assert_rests(&mut compositor);
}

/// A commit at its boundary's deadline exactly, while the output is not idle, is drawn at once
/// and presented at that boundary; one a nanosecond later, at the next.
#[test]
fn a_commit_at_the_deadline_makes_its_boundary_and_one_after_it_the_next() {
    for (late, expected) in [(Duration::ZERO, 2), (Duration::from_nanos(1), 3)] {
        let mut compositor = manual_compositor();
        let mut client = Client::connect(&mut compositor);
        let start = compositor.now();
        let w = open_and_commit(&mut compositor, &mut client, RED);
        // Drawn ahead of boundary 1: the output is not idle until boundary 2.
        advance_to(&mut compositor, &mut client, boundary(start, 1));
        advance_to(
            &mut compositor,
            &mut client,
            boundary(start, 2) - LEAD + late,
        );
        client.attach(w, TILE.0, TILE.1, BLUE);
        client.request_feedback(w);
        client.commit(w);
        client.roundtrip(&mut compositor);
        advance_to(&mut compositor, &mut client, boundary(start, 4));
        assert_eq!(compositor.counters().frames_drawn, 2);
        assert_eq!(
            client.window(w).feedback.last(),
            Some(&Feedback::Presented {
                time: boundary(start, expected),
                refresh: PERIOD,
                seq: expected,
                flags: Kind::Vsync,
            }),
            "a commit {late:?} after the deadline"
        );
    }
}

/// A client that answers each frame callback by asking another, without new content, gets one a
/// period, never in a loop: the refresh that drew nothing counts, so the output is not idle
/// again before the boundary after it. Nothing is drawn.
#[test]
fn callbacks_asked_without_damage_come_once_a_period() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    client.ack(w);
    client.attach(w, TILE.0, TILE.1, RED);
    client.commit(w);
    client.roundtrip(&mut compositor);
    let idle_at = boundary(start, 10) + Duration::from_millis(1);
    advance_to(&mut compositor, &mut client, idle_at);
    let drawn = compositor.counters().frames_drawn;

    client.request_frame(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    assert_eq!(client.window(w).frames.last(), Some(&millis(idle_at)));
    for k in 12..22 {
        let answered = client.window(w).frames.len();
        client.request_frame(w);
        client.commit(w);
        client.roundtrip(&mut compositor);
        assert_eq!(client.window(w).frames.len(), answered);
        let due = boundary(start, k) - LEAD;
        assert_eq!(compositor.next_deadline(), Some(due));
        advance_to(&mut compositor, &mut client, due);
        assert_eq!(client.window(w).frames[answered..], [millis(due)]);
    }
    assert_eq!(compositor.counters().frames_drawn, drawn);
    run_armed_deadlines(&mut compositor);
    assert_rests(&mut compositor);
}

/// A client that commits once every ten boundaries, 2 ms before one, as a film may: alone, it
/// finds the output idle, is drawn at once and makes that boundary. Beside a client that draws
/// every frame, the boundary's deadline, 3 ms before it, has passed: it makes the next one
/// (ADR-0031's two latch points).
#[test]
fn a_steady_client_makes_its_boundary_alone_and_the_next_beside_an_animating_one() {
    for (animating, late) in [(false, 0), (true, 1)] {
        let mut compositor = manual_compositor();
        let mut steady = Client::connect(&mut compositor);
        let mut animator = Client::connect(&mut compositor);
        let start = compositor.now();
        let s = open_and_commit(&mut compositor, &mut steady, RED);
        let a = open_and_commit(&mut compositor, &mut animator, BLUE);
        advance_to(&mut compositor, &mut steady, boundary(start, 1));
        animator.roundtrip(&mut compositor);
        if animating {
            animator.attach(a, TILE.0, TILE.1, RED);
            animator.request_frame(a);
            animator.commit(a);
            animator.roundtrip(&mut compositor);
        }
        // The animator answers each callback with a new frame, however it came.
        let mut answered = animator.window(a).frames.len();
        for frame in 1..=4_u64 {
            let k = 10 * frame;
            let commit_at = boundary(start, k) - Duration::from_millis(2);
            while compositor.now() < commit_at {
                let next = compositor
                    .next_deadline()
                    .map_or(commit_at, |at| at.min(commit_at));
                advance_to(&mut compositor, &mut animator, next);
                if animating && animator.window(a).frames.len() > answered {
                    answered = animator.window(a).frames.len();
                    animator.attach(a, TILE.0, TILE.1, BLUE);
                    animator.request_frame(a);
                    animator.commit(a);
                    animator.roundtrip(&mut compositor);
                }
            }
            steady.attach(s, TILE.0, TILE.1, BLUE);
            steady.request_feedback(s);
            steady.commit(s);
            steady.roundtrip(&mut compositor);
            let expected = k + late;
            while compositor.now() < boundary(start, expected) {
                let next = compositor
                    .next_deadline()
                    .map_or(boundary(start, expected), |at| {
                        at.min(boundary(start, expected))
                    });
                advance_to(&mut compositor, &mut steady, next);
                animator.roundtrip(&mut compositor);
                if animating && animator.window(a).frames.len() > answered {
                    answered = animator.window(a).frames.len();
                    animator.attach(a, TILE.0, TILE.1, BLUE);
                    animator.request_frame(a);
                    animator.commit(a);
                    animator.roundtrip(&mut compositor);
                }
            }
            assert_eq!(
                steady.window(s).feedback.last(),
                Some(&Feedback::Presented {
                    time: boundary(start, expected),
                    refresh: PERIOD,
                    seq: expected,
                    flags: Kind::Vsync,
                }),
                "frame {frame}, beside an animating client: {animating}"
            );
        }
    }
}

/// Drawings longer than a period, as llvmpipe's at 4K: the lead is one period, so each frame is
/// drawn one period before the boundary it aims at, and presented late, at the first boundary
/// after its drawing ends; the feedback says so.
#[test]
fn drawings_longer_than_a_period_are_drawn_a_period_ahead_and_presented_late() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    compositor.set_draw_hook(|clock| clock.pass(Duration::from_millis(7)));
    let w = open_and_commit(&mut compositor, &mut client, RED);
    // The first drawing, from boundary 1 minus 3 ms, ends at 8.17 ms: presented at boundary 2.
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    client.ack(w);
    client.attach(w, TILE.0, TILE.1, BLUE);
    client.request_feedback(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    // Committed at 8.17 ms, the second aims at boundary 3, the first after boundary 2 whose
    // deadline, one period before it, is not past: drawn from just after boundary 2 to 7 ms
    // later, 15.33 ms, it is presented at boundary 4.
    assert_eq!(compositor.next_deadline(), Some(boundary(start, 2)));
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(
        compositor.next_deadline(),
        Some(boundary(start, 3) - PERIOD)
    );
    advance_to(&mut compositor, &mut client, boundary(start, 4));
    let presented: Vec<(u64, Duration)> = client
        .window(w)
        .feedback
        .iter()
        .filter_map(|feedback| match feedback {
            Feedback::Presented { seq, time, .. } => Some((*seq, *time)),
            _ => None,
        })
        .collect();
    assert_eq!(
        presented,
        vec![(2, boundary(start, 2)), (4, boundary(start, 4))]
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
    assert_eq!(
        client.window(w).frames,
        vec![millis(boundary(start, 1) - LEAD)]
    );
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
        .map(|second| millis(boundary(start, 1) - LEAD + Duration::from_secs(second)))
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

/// A compositor started on one thread while another thread drops the last one starts. Smithay
/// shares one EGL display per device among compositors, and terminated it when the last one
/// went, even while another thread was initialising it again: tests run compositors on several
/// threads at once. Each thread in turn drops its compositor while the other starts one; a
/// compositor that fails to start, or a panic in Smithay, which unwraps in that window
/// (`EGLDisplay::new`), is recorded, so that the other thread is never left waiting. Before the
/// fix, the race showed in 9 runs of 20 on two cores, as on CI's runners, and in none of 20 on
/// sixteen.
#[test]
fn a_compositor_starts_while_another_thread_drops_the_last_one() {
    let mode = OutputMode::new(64, 64, 60.0, 1.0).expect("a valid mode");
    let start = || {
        Compositor::new(Config {
            outputs: vec![mode],
            clock: ClockMode::Manual,
            listen: false,
            ..Config::default()
        })
    };
    let rounds = 100;
    let barrier = std::sync::Barrier::new(2);
    let failures = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for thread in 0..2 {
            let (barrier, failures) = (&barrier, &failures);
            scope.spawn(move || {
                let mut held = None;
                for round in 0..rounds {
                    barrier.wait();
                    // One thread drops its compositor as the other starts one, then the roles
                    // swap at the next round.
                    if round % 2 == thread {
                        drop(held.take());
                    } else {
                        let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(start));
                        let failure = match started {
                            Ok(Ok(compositor)) => {
                                held = Some(compositor);
                                continue;
                            }
                            Ok(Err(error)) => error.to_string(),
                            Err(_) => "a panic while starting".to_owned(),
                        };
                        failures
                            .lock()
                            .expect("the failures")
                            .push(format!("round {round}: {failure}"));
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().expect("the failures");
    assert!(
        failures.is_empty(),
        "{} failed: {failures:#?}",
        failures.len()
    );
}

/// A compositor may start without an output, as `yuna mcp` does, whose outputs are its seats':
/// it then has nothing to draw, and rests. A window of the owner's seat opens on no output,
/// hidden.
#[test]
fn a_compositor_without_an_output_starts_and_rests() {
    let mut compositor = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts");
    assert_eq!(compositor.outputs(), Vec::new());
    assert_eq!(compositor.next_deadline(), None);
    let mut client = Client::connect(&mut compositor);
    open_and_commit(&mut compositor, &mut client, RED);
    run_armed_deadlines(&mut compositor);
    let windows = compositor.windows();
    assert_eq!(windows.len(), 1, "{windows:?}");
    assert_eq!((windows[0].output, windows[0].rect), (None, None));
    assert_eq!(compositor.counters().frames_drawn, 0);
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
