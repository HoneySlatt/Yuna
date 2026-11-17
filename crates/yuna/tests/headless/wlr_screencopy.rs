//! `wlr-screencopy-unstable-v1` through `wayland-client`, as uwui's `wayland.py` reads frames
//! ([ADR-0089](../../../../docs/adr/0089-yuna-for-the-owners-other-projects.md)): a client copies
//! its own seat's output, whole or a region, into its `wl_shm` buffer; a copy with damage waits
//! for something new and costs nothing at rest; another seat's output, a region off the output
//! and the bounds fail its frame; a wrong buffer and a second copy are protocol errors.
//!
//! The compositor is `screencopy.rs`'s: one output of 640 × 360 pixels at 60 Hz and scale 1 on
//! the owner's seat, a red window in its first column, 296 wide at (16, 16).

use std::time::Duration;

use wayland_client::WEnum;

use yuna::yuna_layout::OutputId;
use yuna::{
    ClockMode, Compositor, Config, MAX_CAPTURE_SESSIONS, MAX_CAPTURE_SESSIONS_PER_CLIENT,
    OWNER_SEAT,
};

use crate::capture_client::CaptureClient;
use crate::screencopy::{compositor_with_a_window, output_pixel, small_output};
use crate::wlr_capture_client::{Format, WlrCaptureClient};
use crate::{BLUE, RED, assert_rests, run_armed_deadlines};

const WHOLE: (u32, u32) = (640, 360);
/// Red, as a buffer of either format holds it: B, G, R, A.
const RED_IN_MEMORY: [u8; 4] = [0, 0, 255, 255];

fn repaint(compositor: &mut Compositor, client: &mut crate::client::Client, colour: u32) {
    let configure = client
        .window(0)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.attach(0, configure.width, configure.height, colour);
    client.commit(0);
    client.roundtrip(compositor);
}

/// A frame is told its buffer, `XRGB8888` at the output's physical size, then that the list is
/// done; a copy comes at the output's next refresh, with no flag, no damage and the time of its
/// boundary, in the buffer's own order of bytes, either format taken.
#[test]
fn a_client_copies_its_seats_output_into_its_buffer() {
    let (mut compositor, _client) = compositor_with_a_window(RED);
    let background = output_pixel(&mut compositor, 0, 0);
    let mut capture = WlrCaptureClient::connect(&mut compositor, OWNER_SEAT);
    for format in [Format::Xrgb8888, Format::Argb8888] {
        let frame = capture.frame("HEADLESS-1", None);
        capture.roundtrip(&mut compositor);
        let events = capture.events(frame).clone();
        assert_eq!(
            events.buffer,
            Some((WEnum::Value(Format::Xrgb8888), 640, 360, 640 * 4))
        );
        assert!(events.buffer_done);
        capture.copy(frame, format, WHOLE, false);
        capture.roundtrip(&mut compositor);
        run_armed_deadlines(&mut compositor);
        capture.roundtrip(&mut compositor);
        let events = capture.events(frame).clone();
        assert!(events.ready(), "{format:?}: {events:?}");
        assert_eq!((events.flags, events.failed), (Some(0), false));
        assert_eq!(events.damage, []);
        let presented = events.presented.expect("a presentation time");
        assert!(presented <= compositor.now() + Duration::from_millis(17));
        assert_eq!(capture.pixel(frame, 100, 100), RED_IN_MEMORY);
        let [r, g, b, a] = background;
        assert_eq!(capture.pixel(frame, 15, 100), [b, g, r, a]);
        assert_eq!(capture.pixel(frame, 16, 100), RED_IN_MEMORY);
        capture.destroy(frame);
    }
}

/// A region in the output's logical units is copied at its physical size; one partly off the
/// output is cut to it; one off it, or empty, fails at once.
#[test]
fn a_region_is_cut_to_its_output() {
    let (mut compositor, _client) = compositor_with_a_window(RED);
    let background = output_pixel(&mut compositor, 639, 359);
    let mut capture = WlrCaptureClient::connect(&mut compositor, OWNER_SEAT);
    let inside = capture.frame("HEADLESS-1", Some((100, 50, 20, 10)));
    let cut = capture.frame("HEADLESS-1", Some((630, 350, 50, 50)));
    let off = capture.frame("HEADLESS-1", Some((700, 0, 10, 10)));
    let empty = capture.frame("HEADLESS-1", Some((10, 10, 0, 10)));
    capture.roundtrip(&mut compositor);
    assert_eq!(
        capture.events(inside).buffer,
        Some((WEnum::Value(Format::Xrgb8888), 20, 10, 80))
    );
    assert_eq!(
        capture.events(cut).buffer,
        Some((WEnum::Value(Format::Xrgb8888), 10, 10, 40))
    );
    for failed in [off, empty] {
        assert!(capture.events(failed).failed);
        assert_eq!(capture.events(failed).buffer, None);
    }
    capture.copy(inside, Format::Xrgb8888, (20, 10), false);
    capture.copy(cut, Format::Xrgb8888, (10, 10), false);
    capture.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    capture.roundtrip(&mut compositor);
    assert_eq!(capture.pixel(inside, 0, 0), RED_IN_MEMORY);
    assert_eq!(capture.pixel(inside, 19, 9), RED_IN_MEMORY);
    let [r, g, b, a] = background;
    assert_eq!(capture.pixel(cut, 9, 9), [b, g, r, a]);
}

/// A copy with damage takes the frame the output holds the first time, with its whole damage;
/// the next waits for a refresh that draws something: at rest nothing is drawn for it and
/// nothing wakes; a commit that changes the window brings it.
#[test]
fn a_copy_with_damage_waits_for_new_content_and_costs_nothing_at_rest() {
    let (mut compositor, mut client) = compositor_with_a_window(RED);
    let mut capture = WlrCaptureClient::connect(&mut compositor, OWNER_SEAT);
    let first = capture.frame("HEADLESS-1", None);
    capture.copy(first, Format::Xrgb8888, WHOLE, true);
    capture.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    capture.roundtrip(&mut compositor);
    let events = capture.events(first).clone();
    assert!(events.ready(), "{events:?}");
    assert_eq!(events.damage, [(0, 0, 640, 360)]);
    capture.destroy(first);

    let second = capture.frame("HEADLESS-1", None);
    capture.copy(second, Format::Xrgb8888, WHOLE, true);
    capture.roundtrip(&mut compositor);
    assert_rests(&mut compositor);
    capture.roundtrip(&mut compositor);
    assert!(!capture.events(second).ready());

    repaint(&mut compositor, &mut client, BLUE);
    run_armed_deadlines(&mut compositor);
    capture.roundtrip(&mut compositor);
    let events = capture.events(second).clone();
    assert!(events.ready(), "{events:?}");
    assert_eq!(events.damage, [(0, 0, 640, 360)]);
    assert_eq!(capture.pixel(second, 100, 100), [255, 0, 0, 255]);
}

/// Content drawn between two copies with damage is the next copy's, at once: a manager
/// remembers what it copied, as `wayland.py`'s loop of frames needs.
#[test]
fn content_drawn_between_two_copies_is_the_next_copy() {
    let (mut compositor, mut client) = compositor_with_a_window(RED);
    let mut capture = WlrCaptureClient::connect(&mut compositor, OWNER_SEAT);
    let first = capture.frame("HEADLESS-1", None);
    capture.copy(first, Format::Xrgb8888, WHOLE, true);
    capture.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    capture.roundtrip(&mut compositor);
    assert!(capture.events(first).ready());
    repaint(&mut compositor, &mut client, BLUE);
    run_armed_deadlines(&mut compositor);

    let second = capture.frame("HEADLESS-1", None);
    capture.copy(second, Format::Xrgb8888, WHOLE, true);
    capture.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    capture.roundtrip(&mut compositor);
    assert!(capture.events(second).ready());
    assert_eq!(capture.pixel(second, 100, 100), [255, 0, 0, 255]);
}

/// A client copies only its own seat's outputs: a frame of another seat's fails at once. Such a
/// client is never shown another seat's `wl_output` (ADR-0042): these are spies.
#[test]
fn a_frame_of_another_seats_output_fails() {
    let (mut compositor, _client) = compositor_with_a_window(RED);
    let agent = compositor.add_seat().expect("a seat");
    let agent_output = compositor
        .add_output(agent, small_output())
        .expect("an output");
    run_armed_deadlines(&mut compositor);
    let agent_name = format!("HEADLESS-{}", agent_output.0);

    let mut owner = WlrCaptureClient::connect_spy(&mut compositor, OWNER_SEAT);
    let failed = owner.frame(&agent_name, None);
    owner.roundtrip(&mut compositor);
    assert!(owner.events(failed).failed);

    let mut theirs = WlrCaptureClient::connect_spy(&mut compositor, agent);
    let failed = theirs.frame("HEADLESS-1", None);
    let own = theirs.frame(&agent_name, None);
    theirs.roundtrip(&mut compositor);
    assert!(theirs.events(failed).failed);
    theirs.copy(own, Format::Xrgb8888, WHOLE, false);
    theirs.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    theirs.roundtrip(&mut compositor);
    assert!(theirs.events(own).ready());
}

/// A buffer of another size is the client's protocol error, as the protocol says; so is a
/// second copy of one frame.
#[test]
fn a_wrong_buffer_and_a_second_copy_are_protocol_errors() {
    let (mut compositor, _client) = compositor_with_a_window(RED);
    let mut wrong = WlrCaptureClient::connect(&mut compositor, OWNER_SEAT);
    let frame = wrong.frame("HEADLESS-1", None);
    wrong.roundtrip(&mut compositor);
    wrong.copy(frame, Format::Xrgb8888, (320, 360), false);
    assert!(wrong.client.try_roundtrip(&mut compositor).is_err());

    let mut twice = WlrCaptureClient::connect(&mut compositor, OWNER_SEAT);
    let frame = twice.frame("HEADLESS-1", None);
    twice.roundtrip(&mut compositor);
    twice.copy(frame, Format::Xrgb8888, WHOLE, false);
    twice.copy(frame, Format::Xrgb8888, WHOLE, false);
    assert!(twice.client.try_roundtrip(&mut compositor).is_err());

    // Yuna serves the others.
    let mut fine = WlrCaptureClient::connect(&mut compositor, OWNER_SEAT);
    let frame = fine.frame("HEADLESS-1", None);
    fine.copy(frame, Format::Xrgb8888, WHOLE, false);
    fine.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    fine.roundtrip(&mut compositor);
    assert!(fine.events(frame).ready());
}

/// A client keeps at most 4 frames waiting; one past fails at once. A frame destroyed while it
/// waits leaves nothing: Yuna rests.
#[test]
fn frames_waiting_are_bounded_and_a_destroyed_one_leaves_nothing() {
    let (mut compositor, _client) = compositor_with_a_window(RED);
    let mut capture = WlrCaptureClient::connect(&mut compositor, OWNER_SEAT);
    let first = capture.frame("HEADLESS-1", None);
    capture.copy(first, Format::Xrgb8888, WHOLE, true);
    capture.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    capture.roundtrip(&mut compositor);
    assert!(capture.events(first).ready());

    let waiting: Vec<usize> = (0..=MAX_CAPTURE_SESSIONS_PER_CLIENT)
        .map(|_| {
            let frame = capture.frame("HEADLESS-1", None);
            capture.copy(frame, Format::Xrgb8888, WHOLE, true);
            frame
        })
        .collect();
    capture.roundtrip(&mut compositor);
    let failed: Vec<bool> = waiting
        .iter()
        .map(|&frame| capture.events(frame).failed)
        .collect();
    assert_eq!(failed.iter().filter(|&&f| f).count(), 1);
    assert!(failed[MAX_CAPTURE_SESSIONS_PER_CLIENT]);

    for &frame in &waiting {
        capture.destroy(frame);
    }
    capture.roundtrip(&mut compositor);
    assert_eq!(compositor.pending_captures(), 0);
    assert_rests(&mut compositor);
}

/// A frame waiting on an output that is removed fails.
#[test]
fn a_removed_output_fails_its_waiting_frame() {
    let mut compositor = Compositor::new(Config {
        outputs: vec![small_output(), small_output()],
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts");
    run_armed_deadlines(&mut compositor);
    let mut capture = WlrCaptureClient::connect(&mut compositor, OWNER_SEAT);
    let first = capture.frame("HEADLESS-2", None);
    capture.copy(first, Format::Xrgb8888, WHOLE, true);
    capture.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    capture.roundtrip(&mut compositor);
    assert!(capture.events(first).ready());
    let waiting = capture.frame("HEADLESS-2", None);
    capture.copy(waiting, Format::Xrgb8888, WHOLE, true);
    capture.roundtrip(&mut compositor);
    compositor.remove_output(OutputId(2)).expect("removed");
    capture.roundtrip(&mut compositor);
    assert!(capture.events(waiting).failed);
}

/// The bounds hold across both capture protocols: a client keeps 4 sessions and frames waiting
/// together, all clients 16. Four clients holding 4 wlr frames each leave no room for a fifth's
/// `ext-image-copy-capture-v1` session, nor four holding 4 sessions each for a wlr frame.
#[test]
fn the_capture_bounds_count_both_protocols_together() {
    let (mut compositor, _client) = compositor_with_a_window(RED);
    let mut wlr_clients = Vec::new();
    for _ in 0..MAX_CAPTURE_SESSIONS / MAX_CAPTURE_SESSIONS_PER_CLIENT {
        let mut capture = WlrCaptureClient::connect(&mut compositor, OWNER_SEAT);
        let first = capture.frame("HEADLESS-1", None);
        capture.copy(first, Format::Xrgb8888, WHOLE, true);
        capture.roundtrip(&mut compositor);
        run_armed_deadlines(&mut compositor);
        capture.roundtrip(&mut compositor);
        for _ in 0..MAX_CAPTURE_SESSIONS_PER_CLIENT {
            let frame = capture.frame("HEADLESS-1", None);
            capture.copy(frame, Format::Xrgb8888, WHOLE, true);
        }
        capture.roundtrip(&mut compositor);
        wlr_clients.push(capture);
    }
    let mut ext = CaptureClient::connect(&mut compositor, OWNER_SEAT);
    let session = ext.start_session("HEADLESS-1");
    ext.roundtrip(&mut compositor);
    assert!(
        ext.session(session).stopped,
        "a 17th capture past the bound of all clients"
    );

    // One client: 3 sessions and 1 wlr frame waiting fill its 4; a second frame fails.
    let (mut compositor, _client) = compositor_with_a_window(RED);
    let mut ext = CaptureClient::connect(&mut compositor, OWNER_SEAT);
    for _ in 0..MAX_CAPTURE_SESSIONS_PER_CLIENT - 1 {
        ext.start_session("HEADLESS-1");
    }
    ext.roundtrip(&mut compositor);
    let mut same = WlrCaptureClient::bind_to(ext.client, &mut compositor);
    let first = same.frame("HEADLESS-1", None);
    same.copy(first, Format::Xrgb8888, WHOLE, true);
    same.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    same.roundtrip(&mut compositor);
    assert!(same.events(first).ready());
    let kept = same.frame("HEADLESS-1", None);
    same.copy(kept, Format::Xrgb8888, WHOLE, true);
    let past = same.frame("HEADLESS-1", None);
    same.copy(past, Format::Xrgb8888, WHOLE, true);
    same.roundtrip(&mut compositor);
    assert!(!same.events(kept).failed);
    assert!(same.events(past).failed, "a 5th capture of one client");
}
