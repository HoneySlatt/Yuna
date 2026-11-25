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

/// The `frames` tool, in `yuna mcp`, on the real clock: an agent's click, by its request's id,
/// then its client's new buffer, read through `frames` with their frames' causes and
/// presentations; a cursor past the newest frame is refused, and the GPU priority is told.
#[test]
fn an_agent_reads_its_click_and_the_frame_that_showed_its_answer() {
    use serde_json::{Value, json};
    use std::time::Instant;

    use crate::mcp::{Mcp, modern};

    let mut mcp = Mcp::new(yuna::ClockMode::Real);
    let created = mcp.call(
        "seat",
        json!({"action": "create", "width": 640, "height": 480, "rate": 60, "scale": 1}),
    );
    let seat = created["structuredContent"]["seat"]
        .as_u64()
        .expect("a seat");
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = client.create_window();
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    client.ack(window);
    let draw = |mcp: &mut Mcp, client: &mut Client, colour: u32| {
        let called_back = client.window(window).frames.len();
        client.attach(window, 64, 64, colour);
        client.request_frame(window);
        client.commit(window);
        client.send();
        let until = Instant::now() + Duration::from_secs(5);
        while client.window(window).frames.len() == called_back {
            assert!(Instant::now() < until, "the window's frame never came");
            mcp.replies();
            client.read_events();
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    draw(&mut mcp, &mut client, RED);
    client.roundtrip(mcp.server.compositor());
    client.ack(window);
    draw(&mut mcp, &mut client, RED);

    let start = mcp.call("frames", json!({"seat": seat}))["structuredContent"].clone();
    let cursor = start["next"].as_u64().expect("a cursor");
    assert_eq!(start["frames"], json!([]), "{start}");
    assert!(start["gpu_priority"].is_string(), "{start}");

    let request = 4242;
    mcp.send(
        &json!({"jsonrpc": "2.0", "id": request, "method": "tools/call",
        "params": modern(json!({"name": "pointer",
            "arguments": {"action": "click", "seat": seat, "x": 20, "y": 20}}))}),
    );
    let until = Instant::now() + Duration::from_secs(5);
    while !mcp
        .replies()
        .iter()
        .any(|reply| reply["id"] == json!(request))
    {
        assert!(Instant::now() < until, "no answer to the click");
        std::thread::sleep(Duration::from_millis(1));
    }
    client.read_events();
    draw(&mut mcp, &mut client, BLUE);

    // Read until the frame that shows the new buffer is presented: its boundary comes within a
    // period of its drawing.
    let until = Instant::now() + Duration::from_secs(5);
    let (frames, read) = loop {
        let read = mcp.call(
            "frames",
            json!({"seat": seat, "after": cursor, "limit": 16}),
        );
        let frames: Vec<Value> = read["structuredContent"]["frames"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let shown = frames.iter().any(|frame| {
            frame["commits"]
                .as_array()
                .is_some_and(|commits| commits.iter().any(|c| c["new_content"] == json!(true)))
                && !frame["presented"].is_null()
        });
        if shown {
            break (frames, read);
        }
        assert!(Instant::now() < until, "no frame showed the buffer: {read}");
        mcp.replies();
        std::thread::sleep(Duration::from_millis(2));
    };
    let structured = &read["structuredContent"];
    assert_eq!(structured["missed"], json!(0), "{structured}");
    let clicked = frames
        .iter()
        .position(|frame| {
            frame["inputs"].as_array().is_some_and(|inputs| {
                inputs.iter().any(|input| {
                    input["device"] == json!("agent")
                        && input["request"] == json!(request)
                        && input["kind"] == json!("button")
                })
            })
        })
        .unwrap_or_else(|| panic!("no frame covers the click: {structured}"));
    let shown = frames
        .iter()
        .position(|frame| {
            frame["commits"].as_array().is_some_and(|commits| {
                commits
                    .iter()
                    .any(|commit| commit["new_content"] == json!(true))
            }) && !frame["presented"].is_null()
        })
        .expect("the frame that shows the buffer");
    assert!(clicked <= shown, "{structured}");
    let frame = &frames[shown];
    assert_eq!(frame["outcome"], json!("drawn"), "{frame}");
    assert!(
        frame["causes"]
            .as_array()
            .is_some_and(|causes| causes.contains(&json!("commit"))),
        "{frame}"
    );
    let presented = frame["presented"]["at_ns"].as_u64().expect("a time");
    let ended = frame["ended_ns"].as_u64().expect("a time");
    assert!(presented >= ended, "{frame}");
    assert_eq!(frame["late"], json!(false), "{frame}");

    let newest = structured["next"].as_u64().expect("a cursor");
    // A capture names the frame it copied, the output's last drawn.
    let captured = mcp.call("capture", json!({"seat": seat}));
    let captured = &captured["structuredContent"];
    let last_drawn = frames
        .iter()
        .rev()
        .find(|frame| frame["outcome"] != json!("failed"))
        .expect("a frame");
    assert_eq!(
        captured["frame"], last_drawn["id"],
        "{captured} {last_drawn}"
    );
    assert_eq!(captured["boundary"], last_drawn["presented"]["boundary"]);
    let past = mcp.call("frames", json!({"seat": seat, "after": newest + 1}));
    assert_eq!(past["isError"], json!(true), "{past}");
    let other = mcp.call("frames", json!({"seat": seat, "output": 999}));
    assert_eq!(other["isError"], json!(true), "{other}");
    let too_many = mcp.call("frames", json!({"seat": seat, "limit": 17}));
    assert_eq!(too_many["isError"], json!(true), "{too_many}");

    // A reply's size is bounded by what it lists, whatever a frame covers: 2,000 keys typed
    // before one frame.
    let typed = mcp.call(
        "keyboard",
        json!({"action": "type", "seat": seat, "text": "a".repeat(1000)}),
    );
    assert_eq!(typed["isError"], json!(false), "{typed}");
    client.read_events();
    draw(&mut mcp, &mut client, RED);
    let read = mcp.call(
        "frames",
        json!({"seat": seat, "after": newest, "limit": 16}),
    );
    let structured = &read["structuredContent"];
    let frames = structured["frames"].as_array().expect("frames");
    let listed: usize = frames
        .iter()
        .map(|frame| {
            frame["inputs"].as_array().map_or(0, Vec::len)
                + frame["commits"].as_array().map_or(0, Vec::len)
        })
        .sum();
    let more: u64 = frames
        .iter()
        .map(|frame| frame["inputs_more"].as_u64().expect("a count"))
        .sum();
    assert!(listed <= 128, "{listed} listed");
    assert!(more >= 2000 - 128, "{more} more");
    let bytes = read.to_string().len();
    eprintln!("a full reply of frames: {bytes} bytes, {listed} listed, {more} more");
    assert!(bytes < 64 * 1024, "{bytes} bytes");
}
