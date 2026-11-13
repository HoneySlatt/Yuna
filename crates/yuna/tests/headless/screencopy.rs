//! `ext-image-copy-capture-v1` through `wayland-client`
//! ([ADR-0038](../../../../docs/adr/0038-captures-coordinates-windows-and-wayland.md)): a client
//! captures its own seat's output into its `wl_shm` buffer, in both formats offered; a later frame
//! waits for new content, and costs nothing at rest; another seat's output, a buffer that does
//! not fit and a removed output fail as the protocol says.
//!
//! The output here is 640 × 360 pixels at 60 Hz and scale 1, so that a window's buffer pixel is
//! an output's pixel: its column is (640 − 3 × 16) / 2 = 296 wide, at (16, 16).

use std::time::Duration;

use smithay::utils::Rectangle;
use yuna::yuna_layout::OutputId;
use yuna::{
    ClockMode, Compositor, Config, MAX_CAPTURE_SESSIONS_PER_CLIENT, OWNER_SEAT, OutputMode,
};

use crate::capture_client::{CaptureClient, FailureReason, Format};
use crate::client::Client;
use crate::{BLUE, RED, assert_rests, run_armed_deadlines};

fn small_output() -> OutputMode {
    OutputMode::new(640, 360, 60.0, 1.0).expect("a mode")
}

/// A manual-clock compositor of one small output on the owner's seat, with a window of `colour`
/// drawn on it.
fn compositor_with_a_window(colour: u32) -> (Compositor, Client) {
    let mut compositor = Compositor::new(Config {
        outputs: vec![small_output()],
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts");
    let mut client = Client::connect(&mut compositor);
    let window = client.open_window(&mut compositor);
    client.ack(window);
    let configure = client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(window, configure.width, configure.height, colour);
    client.commit(window);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    (compositor, client)
}

/// Pixel (`x`, `y`) of output 1 as the renderer holds it: R, G, B, A.
fn output_pixel(compositor: &mut Compositor, x: i32, y: i32) -> [u8; 4] {
    let pixel = compositor
        .read_pixels(OutputId(1), Rectangle::new((x, y).into(), (1, 1).into()))
        .expect("a pixel");
    [pixel[0], pixel[1], pixel[2], pixel[3]]
}

/// A session is told the output's physical size and `wl_shm`'s two formats; its first frame is the frame
/// the output holds, ready at once with full damage and the time of its boundary, in the
/// buffer's own order of bytes.
#[test]
fn a_client_captures_its_seats_output_into_its_buffer() {
    let (mut compositor, _client) = compositor_with_a_window(RED);
    let background = output_pixel(&mut compositor, 0, 0);
    let mut capture = CaptureClient::connect(&mut compositor, OWNER_SEAT);
    let session = capture.start_session("HEADLESS-1");
    capture.roundtrip(&mut compositor);
    let events = capture.session(session).clone();
    assert_eq!(events.buffer_size, Some((640, 360)));
    assert_eq!(events.formats, [Format::Argb8888, Format::Xrgb8888]);
    assert_eq!((events.done, events.stopped), (1, false));

    // A session's first frame comes at once; each format gets a session of its own.
    for format in [Format::Argb8888, Format::Xrgb8888] {
        let session = capture.start_session("HEADLESS-1");
        let frame = capture.capture(session, format, 640, 360);
        capture.roundtrip(&mut compositor);
        let events = capture.frame(frame).clone();
        assert!(events.ready, "{format:?}: {events:?}");
        assert_eq!(events.failed, None);
        assert_eq!(events.damage, [(0, 0, 640, 360)]);
        let presented = events.presented.expect("a presentation time");
        assert!(presented <= compositor.now() + Duration::from_millis(17));
        let in_memory = |[r, g, b, a]: [u8; 4]| [b, g, r, a];
        assert_eq!(
            capture.pixel(frame, 100, 100),
            in_memory([255, 0, 0, 255]),
            "{format:?}"
        );
        assert_eq!(
            capture.pixel(frame, 15, 100),
            in_memory(background),
            "{format:?}"
        );
        assert_eq!(
            capture.pixel(frame, 16, 100),
            in_memory([255, 0, 0, 255]),
            "{format:?}"
        );
    }
}

/// After its first frame, a session's next waits for a refresh that draws something: at rest
/// nothing is drawn for it and nothing wakes; a commit that changes the window brings it.
#[test]
fn a_later_frame_waits_for_new_content_and_costs_nothing_at_rest() {
    let (mut compositor, mut client) = compositor_with_a_window(RED);
    let mut capture = CaptureClient::connect(&mut compositor, OWNER_SEAT);
    let session = capture.start_session("HEADLESS-1");
    let first = capture.capture(session, Format::Xrgb8888, 640, 360);
    capture.roundtrip(&mut compositor);
    assert!(capture.frame(first).ready);
    capture.destroy_frame(first);

    let second = capture.capture(session, Format::Xrgb8888, 640, 360);
    capture.roundtrip(&mut compositor);
    assert_eq!(capture.frame(second).clone(), Default::default());
    assert_rests(&mut compositor);
    capture.roundtrip(&mut compositor);
    assert!(!capture.frame(second).ready);

    let configure = client
        .window(0)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(0, configure.width, configure.height, BLUE);
    client.commit(0);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    capture.roundtrip(&mut compositor);
    let events = capture.frame(second).clone();
    assert!(events.ready, "{events:?}");
    assert_eq!(capture.pixel(second, 100, 100), [255, 0, 0, 255]);
}

/// A session's first frame asked while a frame is on its way is that frame.
#[test]
fn a_first_frame_is_the_one_on_its_way() {
    let (mut compositor, mut client) = compositor_with_a_window(RED);
    let configure = client
        .window(0)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(0, configure.width, configure.height, BLUE);
    client.commit(0);
    client.roundtrip(&mut compositor);
    let refresh = compositor.next_deadline().expect("a refresh armed");
    assert!(refresh > compositor.now());
    let mut capture = CaptureClient::connect(&mut compositor, OWNER_SEAT);
    let session = capture.start_session("HEADLESS-1");
    let frame = capture.capture(session, Format::Argb8888, 640, 360);
    capture.roundtrip(&mut compositor);
    assert!(!capture.frame(frame).ready);
    compositor
        .advance(refresh - compositor.now())
        .expect("the clock moves");
    capture.roundtrip(&mut compositor);
    assert!(capture.frame(frame).ready);
    assert_eq!(capture.pixel(frame, 100, 100), [255, 0, 0, 255]);
}

/// A client captures only its own seat's outputs: a session of another seat's is stopped, the
/// owner's client's of an agent's output and an agent's client's of the owner's alike.
#[test]
fn a_session_of_another_seats_output_is_stopped() {
    let (mut compositor, _client) = compositor_with_a_window(RED);
    let agent = compositor.add_seat().expect("a seat");
    let agent_output = compositor
        .add_output(agent, small_output())
        .expect("an output");
    run_armed_deadlines(&mut compositor);
    let agent_name = format!("HEADLESS-{}", agent_output.0);

    let mut owner = CaptureClient::connect(&mut compositor, OWNER_SEAT);
    let stopped = owner.start_session(&agent_name);
    let kept = owner.start_session("HEADLESS-1");
    owner.roundtrip(&mut compositor);
    assert!(owner.session(stopped).stopped);
    assert!(!owner.session(kept).stopped);

    let mut theirs = CaptureClient::connect(&mut compositor, agent);
    let stopped = theirs.start_session("HEADLESS-1");
    let own = theirs.start_session(&agent_name);
    theirs.roundtrip(&mut compositor);
    assert!(theirs.session(stopped).stopped);
    assert!(!theirs.session(own).stopped);
    let frame = theirs.capture(own, Format::Argb8888, 640, 360);
    theirs.roundtrip(&mut compositor);
    assert!(theirs.frame(frame).ready);
}

/// A buffer narrower or shorter than the output fails its frame as not fitting the constraints;
/// the session goes on. `wl_shm` itself refuses a buffer of a format it does not offer.
#[test]
fn a_buffer_that_does_not_fit_fails_its_frame() {
    let (mut compositor, _client) = compositor_with_a_window(RED);
    let mut capture = CaptureClient::connect(&mut compositor, OWNER_SEAT);
    let session = capture.start_session("HEADLESS-1");
    let small = capture.capture(session, Format::Argb8888, 320, 360);
    capture.roundtrip(&mut compositor);
    assert_eq!(
        capture.frame(small).failed,
        Some(FailureReason::BufferConstraints)
    );
    capture.destroy_frame(small);
    let short = capture.capture(session, Format::Argb8888, 640, 180);
    capture.roundtrip(&mut compositor);
    assert_eq!(
        capture.frame(short).failed,
        Some(FailureReason::BufferConstraints)
    );
    capture.destroy_frame(short);
    let fits = capture.capture(session, Format::Argb8888, 640, 360);
    capture.roundtrip(&mut compositor);
    assert!(capture.frame(fits).ready);
}

/// A removed output stops its sessions, and fails the frame that waited on it.
#[test]
fn a_removed_output_stops_its_sessions() {
    let mut compositor = Compositor::new(Config {
        outputs: vec![small_output(), small_output()],
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts");
    run_armed_deadlines(&mut compositor);
    let mut capture = CaptureClient::connect(&mut compositor, OWNER_SEAT);
    let session = capture.start_session("HEADLESS-2");
    let first = capture.capture(session, Format::Argb8888, 640, 360);
    capture.roundtrip(&mut compositor);
    assert!(capture.frame(first).ready);
    capture.destroy_frame(first);
    let waiting = capture.capture(session, Format::Argb8888, 640, 360);
    capture.roundtrip(&mut compositor);
    compositor.remove_output(OutputId(2)).expect("removed");
    capture.roundtrip(&mut compositor);
    assert!(capture.session(session).stopped);
    assert_eq!(capture.frame(waiting).failed, Some(FailureReason::Stopped));
}

/// A client keeps at most 16 sessions; the next is stopped at once.
#[test]
fn a_client_keeps_at_most_16_sessions() {
    let (mut compositor, _client) = compositor_with_a_window(RED);
    let mut capture = CaptureClient::connect(&mut compositor, OWNER_SEAT);
    let sessions: Vec<usize> = (0..=MAX_CAPTURE_SESSIONS_PER_CLIENT)
        .map(|_| capture.start_session("HEADLESS-1"))
        .collect();
    capture.roundtrip(&mut compositor);
    let stopped: Vec<bool> = sessions
        .iter()
        .map(|&s| capture.session(s).stopped)
        .collect();
    assert_eq!(stopped.iter().filter(|&&s| s).count(), 1);
    assert!(stopped[MAX_CAPTURE_SESSIONS_PER_CLIENT]);
}

/// A client that leaves with a frame waiting takes nothing with it: Yuna draws the next change
/// and rests.
#[test]
fn a_client_that_leaves_with_a_frame_waiting_leaves_nothing() {
    let (mut compositor, mut client) = compositor_with_a_window(RED);
    let mut capture = CaptureClient::connect(&mut compositor, OWNER_SEAT);
    let session = capture.start_session("HEADLESS-1");
    let first = capture.capture(session, Format::Argb8888, 640, 360);
    capture.roundtrip(&mut compositor);
    capture.destroy_frame(first);
    capture.capture(session, Format::Argb8888, 640, 360);
    capture.roundtrip(&mut compositor);
    drop(capture);
    compositor
        .dispatch(Some(Duration::ZERO))
        .expect("the compositor dispatches");
    let configure = client
        .window(0)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(0, configure.width, configure.height, BLUE);
    client.commit(0);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    assert_eq!(output_pixel(&mut compositor, 100, 100), [0, 0, 255, 255]);
    assert_rests(&mut compositor);
}
