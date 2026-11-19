//! The scenarios on the manual clock: the compositor in the scenario's own thread, as the stories
//! run it, driven through the stories' test clients and helpers. Each waits for every answer it
//! needs with a roundtrip, and moves the clock to the refresh boundaries the stories name, so that
//! the order of messages and every time are fixed.
//!
//! A scenario's outputs are created at a whole millisecond, its start ([`start_compositor`]):
//! times in milliseconds, which frame callbacks and input events carry, are then a whole number
//! of milliseconds from the first frame callback's, the origin of a trace's times, wherever the
//! monotonic clock stood when the process began.

use std::time::Duration;

use yuna::{BUTTON_LEFT, ClockMode, Compositor, Config, OWNER_SEAT, OutputMode};

use super::mark_client;
use super::output_client::OutputsClient;
use crate::capture_client::{CaptureClient, Format};
use crate::client::{Client, Parent, Placement};
use crate::input_client::InputClient;
use crate::outputs::full_hd;
use crate::scale::boundary_at_60_hz;
use crate::screencopy::small_output;
use crate::seats::{inside, windows_of};
use crate::selection_client::SelectionClient;
use crate::{BLUE, RED, TILE, advance_to, boundary, open_and_commit, run_armed_deadlines};

const GREEN: u32 = 0xFF_00_FF_00;
const TEXT: &str = "text/plain;charset=utf-8";

/// A compositor on the manual clock whose outputs, `outputs` on the owner's seat, are created at
/// the next whole millisecond, the scenario's start; and that start. The clock is moved there
/// before any output exists, so no refresh runs on the way. An output a scenario adds before it
/// moves the clock is created at the start too.
fn start_compositor(outputs: &[OutputMode]) -> (Compositor, Duration) {
    let mut compositor = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts");
    let past_a_millisecond = compositor.now().subsec_nanos() % 1_000_000;
    let to_a_whole_millisecond = (1_000_000 - past_a_millisecond) % 1_000_000;
    compositor
        .advance(Duration::from_nanos(u64::from(to_a_whole_millisecond)))
        .expect("the manual clock moves");
    for mode in outputs {
        compositor
            .add_output(OWNER_SEAT, *mode)
            .expect("an output is added");
    }
    let start = compositor.now();
    (compositor, start)
}

/// Acknowledges the window's last configure and commits a buffer of the size it gives, or of the
/// tile's when it leaves the size to the client, with a frame callback and presentation feedback.
fn redraw(compositor: &mut Compositor, client: &mut Client, window: usize, colour: u32) {
    let configure = client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .clone();
    let width = if configure.width > 0 {
        configure.width
    } else {
        TILE.0
    };
    let height = if configure.height > 0 {
        configure.height
    } else {
        TILE.1
    };
    client.ack(window);
    client.attach(window, width, height, colour);
    client.request_frame(window);
    client.request_feedback(window);
    client.commit(window);
    client.roundtrip(compositor);
}

/// Scenario (1): a window opens, is configured, acknowledges and commits; it maps at boundary 1,
/// where its frame callback and feedback come and it is told it is activated. A commit that
/// damages part of its buffer is drawn at boundary 2. It asks to be maximised, then to be
/// fullscreen and to leave it, each answered at the next boundary and drawn at the size told.
/// It closes.
pub fn window() {
    let (mut compositor, start) = start_compositor(&[OutputMode::default()]);
    mark_client("window");
    let mut client = Client::connect(&mut compositor);
    let w = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));

    client.ack(w);
    client.damage(w, 64, 32);
    client.request_frame(w);
    client.request_feedback(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));

    let mut k = 2;
    for change in [
        Client::set_maximized as fn(&mut Client, usize),
        |client, w| client.set_fullscreen(w, true),
        |client, w| client.set_fullscreen(w, false),
    ] {
        change(&mut client, w);
        client.roundtrip(&mut compositor);
        advance_to(&mut compositor, &mut client, boundary(start, k + 1));
        redraw(&mut compositor, &mut client, w, BLUE);
        advance_to(&mut compositor, &mut client, boundary(start, k + 2));
        k += 2;
    }

    client.close(w);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
}

/// Scenario (2): a mapped window's client takes the keyboard; a menu grabs it with the keyboard's
/// `enter` serial, is configured, drawn at boundary 2 and called back. It asks to be placed again,
/// with token 7, and is drawn there at boundary 3. A second grabbing menu ends the first's grab,
/// which is dismissed; the client destroys both, and the keyboard comes back to the window.
pub fn popups() {
    let (mut compositor, start) = start_compositor(&[OutputMode::default()]);
    mark_client("menus");
    let mut client = Client::connect(&mut compositor);
    let w = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    client.bind_keyboard(&mut compositor);

    let menu = client.create_popup(
        Parent::Window(w),
        Placement::below_right((200, 100), (10, 10, 1, 1)),
    );
    client.grab_popup(menu, client.enter_serial());
    client.commit_popup(menu);
    client.roundtrip(&mut compositor);
    client.ack_popup(menu);
    client.attach_popup(menu, 200, 100, GREEN);
    client.request_popup_frame(menu);
    client.request_popup_feedback(menu);
    client.commit_popup(menu);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));

    client.reposition_popup(menu, Placement::below_right((300, 120), (40, 30, 1, 1)), 7);
    client.roundtrip(&mut compositor);
    client.ack_popup(menu);
    client.attach_popup(menu, 300, 120, GREEN);
    client.request_popup_frame(menu);
    client.commit_popup(menu);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 3));

    let second = client.create_popup(
        Parent::Window(w),
        Placement::below_right((200, 100), (10, 10, 1, 1)),
    );
    client.grab_popup(second, client.enter_serial());
    client.commit_popup(second);
    client.roundtrip(&mut compositor);
    client.destroy_popup(menu);
    client.destroy_popup(second);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 4));
}

/// Scenario (3): an agent's seat, with an output of 1920 × 1080 at 60 Hz, and a client of it
/// with two windows, the second focused. The pointer enters the first window and moves in it; a
/// click there gives it the keyboard at the next refresh, the second window losing it; Shift and
/// A are pressed and released there; the pointer moves to the second window, leaving the first,
/// then off every window. Each step comes 5 ms after the last.
pub fn input() {
    let (mut compositor, start) = start_compositor(&[]);
    let seat = compositor.add_seat().expect("a seat");
    compositor
        .add_output(seat, full_hd())
        .expect("the seat's output");
    mark_client("input");
    let mut input = InputClient::connect(&mut compositor, seat, "agent-2");
    let first = input.client.open_window(&mut compositor);
    redraw_input(&mut compositor, &mut input, first);
    let second = input.client.open_window(&mut compositor);
    redraw_input(&mut compositor, &mut input, second);
    advance_to(
        &mut compositor,
        &mut input.client,
        boundary_at_60_hz(start, 1),
    );
    input.roundtrip(&mut compositor);
    // The windows' activated configures, acknowledged.
    for window in [first, second] {
        redraw_input(&mut compositor, &mut input, window);
    }
    advance_to(
        &mut compositor,
        &mut input.client,
        boundary_at_60_hz(start, 2),
    );
    input.roundtrip(&mut compositor);
    let windows = windows_of(&compositor, seat);

    // Every step reads what came, then lets 5 ms pass: the refresh that gives the clicked window
    // the keyboard, 3 ms before boundary 3, comes between the press and the release.
    let step = |compositor: &mut Compositor, input: &mut InputClient| {
        input.roundtrip(compositor);
        compositor
            .advance(Duration::from_millis(5))
            .expect("the manual clock moves");
    };
    let (output, x, y) = inside(&compositor, windows[0]);
    compositor
        .pointer_move(seat, Some(output), x, y)
        .expect("the pointer enters");
    step(&mut compositor, &mut input);
    compositor
        .pointer_move(seat, Some(output), x + 20.5, y + 10.25)
        .expect("the pointer moves");
    step(&mut compositor, &mut input);
    compositor
        .pointer_button(seat, BUTTON_LEFT, true)
        .expect("a press");
    step(&mut compositor, &mut input);
    compositor
        .pointer_button(seat, BUTTON_LEFT, false)
        .expect("a release");
    step(&mut compositor, &mut input);
    for (key, pressed) in [
        ("Shift_L", true),
        ("a", true),
        ("a", false),
        ("Shift_L", false),
    ] {
        compositor.key(seat, key, pressed).expect("a key");
        step(&mut compositor, &mut input);
    }
    let (output, x, y) = inside(&compositor, windows[1]);
    compositor
        .pointer_move(seat, Some(output), x, y)
        .expect("the pointer moves to the second window");
    step(&mut compositor, &mut input);
    compositor
        .pointer_move(seat, Some(output), 1919.0, 1079.0)
        .expect("the pointer moves off the windows");
    step(&mut compositor, &mut input);
}

/// Acknowledges the window's last configure, unless it did already: a configure acknowledged
/// twice is the client's protocol error.
fn ack_new_configure(client: &mut Client, window: usize) {
    let last = client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .serial;
    if client.window(window).acked != Some(last) {
        client.ack(window);
    }
}

/// Acknowledges the last configure of the input client's window if it is new, and commits a
/// buffer of the size it gives, with a frame callback: the first is the origin of the trace's
/// times.
fn redraw_input(compositor: &mut Compositor, input: &mut InputClient, window: usize) {
    let configure = input
        .client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .clone();
    ack_new_configure(&mut input.client, window);
    input
        .client
        .attach(window, configure.width, configure.height, RED);
    input.client.request_frame(window);
    input.client.commit(window);
    input.roundtrip(compositor);
}

/// Scenario (4): a mapped window gets a subsurface of 50 × 50 at (10, 10), desynchronised and
/// committed, then a second at (100, 40); the window commits with a frame callback and feedback
/// after each, drawn at the next boundary. It closes.
pub fn subsurfaces() {
    let (mut compositor, start) = start_compositor(&[OutputMode::default()]);
    mark_client("subsurfaces");
    let mut client = Client::connect(&mut compositor);
    let w = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    for (k, at) in [(2, (10, 10)), (3, (100, 40))] {
        client.add_subsurface_at(w, at);
        client.roundtrip(&mut compositor);
        ack_new_configure(&mut client, w);
        client.request_frame(w);
        client.request_feedback(w);
        client.commit(w);
        client.roundtrip(&mut compositor);
        advance_to(&mut compositor, &mut client, boundary(start, k));
    }
    client.close(w);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
}

/// Scenario (5): on the default output, 3840 × 2160 at scale 1.5, a window asks its fractional
/// scale and a viewport before its first commit: it is told 180 (1.5 × 120) and a buffer scale
/// of 2. It commits a buffer of its tile's size times 1.5, 1884 × 2112 pixels, with a viewport
/// destination of the tile, 1256 × 1408, drawn at boundary 1; then a source cut from it, drawn
/// at boundary 2.
pub fn scale() {
    let (mut compositor, start) = start_compositor(&[OutputMode::default()]);
    mark_client("scale");
    let mut client = Client::connect(&mut compositor);
    let w = client.create_window();
    client.get_fractional_scale(w);
    client.get_viewport(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    client.ack(w);
    client.attach(w, 1884, 2112, RED);
    client.set_viewport_destination(w, TILE.0, TILE.1);
    client.request_frame(w);
    client.request_feedback(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 1));

    client.ack(w);
    client.set_viewport_source(w, (0.5, 0.25), (1000.5, 1200.75));
    client.request_frame(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
}

/// Scenario (6): a client of the owner's seat binds every `wl_output` and its `xdg-output`, of
/// the default output; a second output, 1920 × 1080 at 60 Hz and scale 1, is added, and the
/// client binds it as it is announced; once the outputs' refreshes armed have run, it is removed,
/// and the client lets go of it.
pub fn outputs() {
    let (mut compositor, _) = start_compositor(&[OutputMode::default()]);
    mark_client("outputs");
    let mut outputs = OutputsClient::connect(&mut compositor);
    let added = compositor
        .add_output(OWNER_SEAT, full_hd())
        .expect("an output is added");
    // Once for the global, once for the events of the output bound.
    outputs.roundtrip(&mut compositor);
    outputs.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    compositor
        .remove_output(added)
        .expect("the output is removed");
    outputs.roundtrip(&mut compositor);
    outputs.roundtrip(&mut compositor);
}

/// Scenario (7): a window of the owner's seat on an output of 640 × 360 at 60 Hz and scale 1;
/// a capture client makes a session of that output, and captures a frame into a `wl_shm` buffer
/// of XRGB8888, which comes at once. Its next frame waits for new content: the window commits,
/// and the frame comes at the refresh that draws it.
pub fn capture() {
    let (mut compositor, start) = start_compositor(&[small_output()]);
    mark_client("window");
    let mut client = Client::connect(&mut compositor);
    let w = client.open_window(&mut compositor);
    redraw(&mut compositor, &mut client, w, RED);
    advance_to(&mut compositor, &mut client, boundary_at_60_hz(start, 1));

    mark_client("capture");
    let mut capture = CaptureClient::connect(&mut compositor, OWNER_SEAT);
    let session = capture.start_session("HEADLESS-1");
    capture.roundtrip(&mut compositor);
    let size = capture
        .session(session)
        .buffer_size
        .expect("the session's size");
    let first = capture.capture(session, Format::Xrgb8888, size.0, size.1);
    capture.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    capture.roundtrip(&mut compositor);
    capture.destroy_frame(first);
    let second = capture.capture(session, Format::Xrgb8888, size.0, size.1);
    capture.roundtrip(&mut compositor);

    mark_client("window");
    redraw(&mut compositor, &mut client, w, BLUE);
    advance_to(&mut compositor, &mut client, boundary_at_60_hz(start, 3));
    mark_client("capture");
    capture.roundtrip(&mut compositor);
    capture.destroy_frame(second);
    capture.roundtrip(&mut compositor);
}

/// Scenario (8): `selection.rs`'s story 1. Client A, which has the focus, sets the clipboard with
/// its keyboard `enter` serial; client B's window opens and takes the focus, is offered the
/// clipboard and asks for it into a pipe; A is sent the pipe, writes `Yuna` and closes it.
pub fn clipboard() {
    let (mut compositor, start) = start_compositor(&[OutputMode::default()]);
    mark_client("a");
    let mut a = SelectionClient::connect(&mut compositor);
    mark_client("b");
    let mut b = SelectionClient::connect(&mut compositor);

    mark_client("a");
    open_and_commit(&mut compositor, &mut a.client, RED);
    advance_to(&mut compositor, &mut a.client, boundary(start, 1));
    a.roundtrip(&mut compositor);
    let source = a.set_clipboard(&[TEXT], a.enter_serial());
    a.roundtrip(&mut compositor);

    mark_client("b");
    open_and_commit(&mut compositor, &mut b.client, BLUE);
    advance_to(&mut compositor, &mut b.client, boundary(start, 2));
    b.roundtrip(&mut compositor);
    let pipe = b.receive(0, TEXT);
    b.roundtrip(&mut compositor);

    mark_client("a");
    a.roundtrip(&mut compositor);
    a.answer(source, b"Yuna");
    assert_eq!(pipe.read_all(), b"Yuna");
    a.roundtrip(&mut compositor);
}
