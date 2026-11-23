//! The test seams WLCS's shim drives Yuna through (ADR-0149): a waker another thread wakes the
//! loop with, a client's key that finds its windows by their surfaces, a seat whose windows open
//! floating, and the placement of a floating window.
//!
//! The stories run on the manual clock, the owner's default output first: 2560 logical units
//! wide, so that an agent's output added after it starts at x = 2560.

use std::time::{Duration, Instant};

use wayland_client::Proxy;
use yuna::rest::Source;
use yuna::{Compositor, OutputMode, Waker};
use yuna_ipc::Refusal;
use yuna_types::{Rect, SeatId, WindowId};

use crate::client::Client;
use crate::{BLUE, RED, assert_rests, manual_compositor, run_armed_deadlines};

/// Where an agent's output starts: right of the owner's, 3840 pixels at scale 1.5.
const AGENT_OUTPUT_X: f64 = 2560.0;

/// A compositor with an agent's seat and its output, 1920 × 1080 at 60 Hz and scale 1, as the
/// shim runs, every deadline its start armed run.
fn with_agent_seat() -> (Compositor, SeatId) {
    let mut compositor = manual_compositor();
    let seat = compositor.add_seat().expect("a seat");
    let mode = OutputMode::new(1920, 1080, 60.0, 1.0).expect("a mode");
    compositor.add_output(seat, mode).expect("an output");
    run_armed_deadlines(&mut compositor);
    (compositor, seat)
}

/// Maps `window` at `width` × `height`, its configure answered, and runs the refresh it arms.
fn map(compositor: &mut Compositor, client: &mut Client, window: usize, size: (i32, i32)) {
    client.ack(window);
    client.attach(window, size.0, size.1, RED);
    client.commit(window);
    client.roundtrip(compositor);
    run_armed_deadlines(compositor);
    client.roundtrip(compositor);
}

/// The place of `window`, as last arranged.
fn place(compositor: &Compositor, window: WindowId) -> Rect {
    compositor
        .windows()
        .into_iter()
        .find(|description| description.id == window)
        .and_then(|description| description.place)
        .expect("the window has a place")
}

fn sendable_and_cloneable<T: Send + Clone + 'static>(_: &T) {}

/// A waker sent to another thread wakes a round waiting there, which returns at once rather
/// than at its timeout, and the rest auditor names it; wakes given before a round count as one
/// round, whichever handle gave them, and the loop then rests.
#[test]
fn a_waker_on_another_thread_ends_a_waiting_round_and_is_named() {
    let mut compositor = manual_compositor();
    run_armed_deadlines(&mut compositor);
    let waker = compositor.waker().expect("a waker");
    sendable_and_cloneable(&waker);
    let before = compositor.rest_counts();
    let sent: Waker = waker.clone();
    let started = Instant::now();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        sent.wake();
    });
    // A long timeout rather than none, so that a waker that wakes nothing fails the story
    // rather than hang it.
    compositor
        .dispatch(Some(Duration::from_secs(30)))
        .expect("a round");
    let waited = started.elapsed();
    thread.join().expect("the waking thread ends");
    assert!(
        waited >= Duration::from_millis(100) && waited < Duration::from_secs(10),
        "the round waited {waited:?}"
    );
    let woke = compositor.rest_counts().since(&before);
    assert_eq!(woke.woke, 1);
    assert_eq!(woke.whole.wakes_by(Source::Waker), 1);
    assert_eq!(Source::Waker.name(), "waker");
    assert!(!Source::Waker.serves_a_seat());

    let second = compositor.waker().expect("a second handle");
    let before = compositor.rest_counts();
    waker.wake();
    second.wake();
    compositor.dispatch(Some(Duration::ZERO)).expect("a round");
    compositor.dispatch(Some(Duration::ZERO)).expect("a round");
    let woke = compositor.rest_counts().since(&before);
    assert_eq!((woke.woke, woke.whole.wakes_by(Source::Waker)), (1, 1));
    assert_rests(&mut compositor);
}

/// A client's key finds the window of its toplevel's surface once it is mapped, and nothing
/// for another client, an unknown id, an object that is no surface, or a client gone.
#[test]
fn a_clients_key_finds_its_window_by_its_surface_and_nothing_else() {
    let (mut compositor, seat) = with_agent_seat();
    let (key, mut client) = Client::connect_keyed(&mut compositor, seat);
    let (other_key, mut other) = Client::connect_keyed(&mut compositor, seat);
    let w = client.open_window(&mut compositor);
    let surface = client.window(w).surface.id().protocol_id();
    assert_eq!(
        compositor.window_of_surface(&key, surface),
        None,
        "not mapped yet"
    );

    map(&mut compositor, &mut client, w, (300, 200));
    let window = compositor.windows()[0].id;
    assert_eq!(compositor.window_of_surface(&key, surface), Some(window));
    assert_eq!(compositor.window_of_surface(&other_key, surface), None);
    assert_eq!(compositor.window_of_surface(&key, 100_000), None);
    // Object 1 is the client's `wl_display`.
    assert_eq!(compositor.window_of_surface(&key, 1), None);

    drop(client);
    other.roundtrip(&mut compositor);
    assert_eq!(compositor.windows(), Vec::new());
    assert_eq!(compositor.window_of_surface(&key, surface), None);
}

/// A seat set to open windows floating opens its clients' new windows floating, asked 0 × 0,
/// centred on its output at the size their client commits, which they follow at each commit,
/// their corner kept. Set back, the seat's new windows tile. An unknown seat is refused.
#[test]
fn a_floating_seats_windows_open_floating_at_their_clients_size() {
    let (mut compositor, seat) = with_agent_seat();
    compositor
        .set_seat_opens_floating(seat, true)
        .expect("the seat opens windows floating");
    let mut client = Client::connect_to(&mut compositor, seat);
    let w = client.open_window(&mut compositor);
    map(&mut compositor, &mut client, w, (300, 200));
    let description = compositor.windows()[0].clone();
    assert!(description.floating);
    let x = AGENT_OUTPUT_X + (1920.0 - 300.0) / 2.0;
    let y = (1080.0 - 200.0) / 2.0;
    assert_eq!(description.place, Some(Rect::new(x, y, 300.0, 200.0)));

    client.attach(w, 400, 250, BLUE);
    client.commit(w);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    assert_eq!(
        place(&compositor, description.id),
        Rect::new(x, y, 400.0, 250.0)
    );
    let sizes: Vec<(i32, i32)> = client
        .window(w)
        .configures
        .iter()
        .map(|configure| (configure.width, configure.height))
        .collect();
    assert!(!sizes.is_empty());
    assert!(
        sizes.iter().all(|&size| size == (0, 0)),
        "a floating window chooses its size: {sizes:?}"
    );

    compositor
        .set_seat_opens_floating(seat, false)
        .expect("the seat tiles again");
    let tiled = client.open_window(&mut compositor);
    map(&mut compositor, &mut client, tiled, (300, 200));
    let windows = compositor.windows();
    assert_eq!(windows.len(), 2);
    assert!(windows[0].floating, "a window open stays as it is");
    assert!(!windows[1].floating);

    let unknown = SeatId(99);
    assert_eq!(
        compositor.set_seat_opens_floating(unknown, true),
        Err(Refusal::UnknownSeat(unknown))
    );
}

/// Placing a floating window moves it there on its output, its size kept, with no configure;
/// a place far off keeps a quarter of it on the output, as for any floating window. A tiled
/// window, an unknown one and a place that is no number are refused, nothing changed.
#[test]
fn a_floating_window_placed_moves_with_no_configure() {
    let (mut compositor, seat) = with_agent_seat();
    compositor
        .set_seat_opens_floating(seat, true)
        .expect("the seat opens windows floating");
    let mut client = Client::connect_to(&mut compositor, seat);
    let w = client.open_window(&mut compositor);
    map(&mut compositor, &mut client, w, (300, 200));
    let window = compositor.windows()[0].id;
    let configures = compositor.counters().configures;
    let told = client.window(w).configures.len();

    compositor
        .place_window(window, 100.0, 50.0)
        .expect("the window is placed");
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    let placed = Rect::new(AGENT_OUTPUT_X + 100.0, 50.0, 300.0, 200.0);
    assert_eq!(place(&compositor, window), placed);
    let drawn = compositor.windows()[0].rect;
    assert_eq!(
        drawn,
        Some(placed),
        "drawn where it was placed, without motion"
    );
    assert_eq!(compositor.counters().configures, configures);
    assert_eq!(client.window(w).configures.len(), told);

    // A quarter of 300, 75 units, stays on the output.
    compositor
        .place_window(window, -10_000.0, 0.0)
        .expect("the window is placed");
    run_armed_deadlines(&mut compositor);
    let fitted = Rect::new(AGENT_OUTPUT_X - 225.0, 0.0, 300.0, 200.0);
    assert_eq!(place(&compositor, window), fitted);

    let mut owner = Client::connect(&mut compositor);
    let t = owner.open_window(&mut compositor);
    map(&mut compositor, &mut owner, t, (300, 200));
    let tiled = compositor.windows()[1].id;
    assert!(!compositor.windows()[1].floating);
    let tiled_place = place(&compositor, tiled);
    assert_eq!(
        compositor.place_window(tiled, 10.0, 10.0),
        Err(Refusal::WindowTiled)
    );
    let unknown = WindowId(99);
    assert_eq!(
        compositor.place_window(unknown, 10.0, 10.0),
        Err(Refusal::UnknownWindow(unknown))
    );
    assert_eq!(
        compositor.place_window(window, f64::NAN, 10.0),
        Err(Refusal::InvalidValue)
    );
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    owner.roundtrip(&mut compositor);
    assert_eq!(place(&compositor, window), fitted);
    assert_eq!(place(&compositor, tiled), tiled_place);
    assert_rests(&mut compositor);
}
