//! The frame record (ADR-0158): each seat's inputs, commits and frames, numbered, read through
//! `Compositor::frames`.

use std::time::Duration;

use yuna::rest::Cause;
use yuna::{Compositor, Frame, FrameOutcome, InputDevice, InputKind, OutputMode};
use yuna_types::{OutputId, SeatId};

use crate::client::Client;
use crate::{BLUE, RED, run_armed_deadlines};

/// A seat of its own, with one 640 × 480 output at 60 Hz, on the manual clock.
fn seat_with_output() -> (Compositor, SeatId, OutputId) {
    let mut compositor = crate::manual_compositor();
    let seat = compositor.add_seat().expect("a seat");
    let mode = OutputMode::new(640, 480, 60.0, 1.0).expect("a mode");
    let output = compositor.add_output(seat, mode).expect("an output");
    run_armed_deadlines(&mut compositor);
    (compositor, seat, output)
}

/// A window of `client`, drawn and settled.
fn drawn_window(compositor: &mut Compositor, client: &mut Client) -> usize {
    let window = client.open_window(compositor);
    client.ack(window);
    client.attach(window, 64, 64, RED);
    client.commit(window);
    client.roundtrip(compositor);
    for _ in 0..4 {
        run_armed_deadlines(compositor);
        client.roundtrip(compositor);
    }
    assert_eq!(compositor.next_deadline(), None);
    window
}

/// The frames of `seat` after `after`, every output's.
fn frames_after(compositor: &Compositor, seat: SeatId, after: u64) -> Vec<Frame> {
    compositor
        .frames(seat, None, after, 64)
        .expect("the seat's frames")
        .frames
        .iter()
        .map(|(_, frame)| **frame)
        .collect()
}

/// A pointer's motion, then a client's new buffer: the frame that shows it covers both, in
/// order, the motion by the interface's device and the commit by its client, with its window and
/// its output; its cause is the commit; it is presented at the boundary it aimed at, which the
/// clock then reaches.
#[test]
fn a_frame_covers_the_inputs_and_commits_before_it_and_tells_its_presentation() {
    let (mut compositor, seat, output) = seat_with_output();
    let mut client = Client::connect_to(&mut compositor, seat);
    let window = drawn_window(&mut compositor, &mut client);
    let before = compositor.last_frame(seat);

    compositor
        .pointer_move(seat, None, 20.0, 20.0)
        .expect("the pointer moves");
    client.attach(window, 64, 64, BLUE);
    client.commit(window);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);

    let reading = compositor
        .frames(seat, Some(output), before, 16)
        .expect("the seat's frames");
    assert_eq!(reading.missed, 0);
    let drawn: Vec<&Frame> = reading
        .frames
        .iter()
        .map(|(_, frame)| *frame)
        .filter(|frame| matches!(frame.outcome, FrameOutcome::Drawn { .. }))
        .collect();
    assert_eq!(drawn.len(), 1, "{:?}", reading.frames);
    let frame = drawn[0];
    assert_eq!(frame.output, output);
    assert!(frame.causes.contains(Cause::Commit), "{frame:?}");
    assert!(!frame.causes.contains(Cause::LayoutChange), "{frame:?}");
    let inputs: Vec<_> = reading.inputs(frame).map(|(_, input)| *input).collect();
    assert!(
        inputs
            .iter()
            .any(|input| input.kind == InputKind::Motion && input.device == InputDevice::Interface),
        "{inputs:?}"
    );
    let commits: Vec<_> = reading.commits(frame).map(|(_, commit)| *commit).collect();
    let window_id = compositor.windows()[0].id;
    assert!(
        commits.iter().any(|commit| commit.new_content
            && commit.window == Some(window_id)
            && commit.output == Some(output)),
        "{commits:?}"
    );
    let presented = frame.presented.expect("a presentation");
    assert_eq!(presented, frame.aimed, "{frame:?}");
    assert!(!frame.late());
    assert!(frame.armed_at <= frame.due_at && frame.due_at <= frame.ran_at);
    assert!(presented.at >= frame.ended_at, "{frame:?}");
}

/// Frames are numbered per seat and each has an id of its own; a cursor past the ring's oldest
/// is told exactly how many frames it missed. A motion's frames each arm the next: no two share
/// an id.
#[test]
fn a_seats_frames_are_numbered_and_each_has_its_own_id() {
    let (mut compositor, seat, _output) = seat_with_output();
    let mut client = Client::connect_to(&mut compositor, seat);
    let window = drawn_window(&mut compositor, &mut client);
    let before = compositor.last_frame(seat);
    let count = yuna::MAX_FRAMES as u64 + 10;
    for k in 0..count {
        client.attach(window, 64, 64, if k % 2 == 0 { BLUE } else { RED });
        client.commit(window);
        client.roundtrip(&mut compositor);
        compositor
            .advance(Duration::from_millis(17))
            .expect("the clock moves");
    }
    run_armed_deadlines(&mut compositor);
    let newest = compositor.last_frame(seat);
    assert!(newest >= before + count, "{newest} after {before}");
    let reading = compositor
        .frames(seat, None, 0, usize::MAX)
        .expect("frames");
    assert_eq!(reading.missed, newest - yuna::MAX_FRAMES as u64);
    assert_eq!(reading.frames.len(), yuna::MAX_FRAMES);
    let numbers: Vec<u64> = reading.frames.iter().map(|(n, _)| *n).collect();
    let expected: Vec<u64> = (newest - yuna::MAX_FRAMES as u64 + 1..=newest).collect();
    assert_eq!(numbers, expected);
    let mut ids: Vec<u64> = reading.frames.iter().map(|(_, f)| f.id.get()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), yuna::MAX_FRAMES, "two frames share an id");
    let paged = compositor
        .frames(seat, None, newest - 3, 2)
        .expect("frames");
    assert_eq!(paged.frames.len(), 2);
    assert_eq!(paged.next, newest - 1);
    assert_eq!(paged.missed, 0);
}

/// A run of an agent's pointer motions between two frames is one entry, counted; the next
/// frame's motions start another. A frame whose output was removed before its boundary is told
/// discarded, never presented.
#[test]
fn motions_between_frames_merge_and_a_removed_outputs_frame_is_discarded() {
    let (mut compositor, seat, output) = seat_with_output();
    let mode = OutputMode::new(640, 480, 60.0, 1.0).expect("a mode");
    let second = compositor.add_output(seat, mode).expect("an output");
    run_armed_deadlines(&mut compositor);
    let mut client = Client::connect_to(&mut compositor, seat);
    let window = drawn_window(&mut compositor, &mut client);
    let before = compositor.last_frame(seat);
    for x in 0..5 {
        compositor
            .pointer_move(seat, Some(output), 10.0 + f64::from(x), 10.0)
            .expect("the pointer moves");
    }
    client.attach(window, 64, 64, BLUE);
    client.commit(window);
    client.roundtrip(&mut compositor);
    run_armed_deadlines(&mut compositor);
    let frames = frames_after(&compositor, seat, before);
    let reading = compositor.frames(seat, None, before, 64).expect("frames");
    let covering = frames
        .iter()
        .find(|frame| frame.inputs.count() > 0)
        .expect("a frame covers the motions");
    let motions: Vec<_> = reading.inputs(covering).map(|(_, input)| *input).collect();
    assert_eq!(motions.len(), 1, "{motions:?}");
    assert_eq!(motions[0].count, 5);
    assert!(motions[0].last >= motions[0].time);

    // A refresh drawn and not yet presented when its output goes.
    let before = compositor.last_frame(seat);
    client.attach(window, 64, 64, RED);
    client.commit(window);
    client.roundtrip(&mut compositor);
    let due = compositor.next_deadline().expect("a refresh");
    compositor
        .advance(due.saturating_sub(compositor.now()))
        .expect("the clock moves");
    let drawn = frames_after(&compositor, seat, before);
    let pending = drawn
        .iter()
        .find(|frame| frame.presented.is_some_and(|at| at.at > compositor.now()))
        .copied();
    // Drawn at its deadline, a lead before its boundary, on the manual clock.
    let pending = pending.unwrap_or_else(|| panic!("no frame waits for its boundary: {drawn:?}"));
    compositor
        .remove_output(pending.output)
        .expect("the output goes");
    let after = frames_after(&compositor, seat, before);
    let discarded = after
        .iter()
        .find(|frame| frame.id == pending.id)
        .expect("the frame is kept");
    assert!(
        discarded.discarded && discarded.presented.is_none(),
        "{discarded:?}"
    );
    assert!(second != output);
}
