//! The rest auditor through MCP's `rest`, in the test's own process (ADR-0062, point 7): at rest
//! nothing is named; a client that draws is named, with its process, its commits and the frames
//! they caused; a waiting call's timer is named, and a `rest` call's own is left out. Its
//! clients are the test's own, so no program names them: `tests/mcp.rs` names programs, and
//! `xwayland.rs` Xwayland's idle stop.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yuna::ClockMode;
use yuna_types::SeatId;

use crate::RED;
use crate::client::Client;
use crate::mcp::{Mcp, modern};

/// A seat of its own, with an output small enough for llvmpipe to draw at its rate in a debug
/// build: 640 × 480 at 60 Hz.
fn small_seat(mcp: &mut Mcp) -> u64 {
    let created = mcp.call(
        "seat",
        json!({"action": "create", "width": 640, "height": 480, "rate": 60, "scale": 1}),
    );
    assert_eq!(created["isError"], json!(false), "{created}");
    created["structuredContent"]["seat"]
        .as_u64()
        .expect("a handle")
}

/// Runs the loop until nothing is armed: the seat's first frame, and what follows it, drawn.
fn settle(mcp: &mut Mcp) {
    let until = Instant::now() + Duration::from_secs(5);
    while mcp.server.compositor().next_deadline().is_some() {
        assert!(Instant::now() < until, "the compositor never settled");
        mcp.replies();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Draws `window` with a new buffer and a frame callback, then runs the loop until it is called
/// back.
fn draw_and_wait(mcp: &mut Mcp, client: &mut Client, window: usize, colour: u32) {
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
}

/// Opens a window on `seat`, its first buffer mapping it, then draws it once more, each frame
/// called back, and runs the loop until nothing is armed.
fn open_drawn_window(mcp: &mut Mcp, seat: u64) -> (Client, usize) {
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = client.create_window();
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    client.ack(window);
    draw_and_wait(mcp, &mut client, window, RED);
    // Its first configure after mapping, which a client acknowledges before drawing for it.
    client.roundtrip(mcp.server.compositor());
    client.ack(window);
    draw_and_wait(mcp, &mut client, window, 0xFF_00_00_FF);
    settle(mcp);
    (client, window)
}

/// Collects the replies to each of `ids`, whatever their order.
fn replies_to(mcp: &mut Mcp, ids: &[u64]) -> Vec<Value> {
    let mut found: Vec<Option<Value>> = vec![None; ids.len()];
    let until = Instant::now() + Duration::from_secs(5);
    while found.iter().any(Option::is_none) {
        assert!(Instant::now() < until, "no reply to one of {ids:?}");
        for reply in mcp.replies() {
            if let Some(index) = ids.iter().position(|id| reply["id"] == json!(id)) {
                found[index] = Some(reply);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    found.into_iter().flatten().collect()
}

/// Sends a `rest` call with `arguments` as request `id`, without waiting for its answer.
fn send_rest(mcp: &mut Mcp, id: u64, arguments: Value) {
    mcp.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                     "params": modern(json!({"name": "rest", "arguments": arguments}))}));
}

/// What a result says that woke Yuna or drew: nothing, at rest.
fn named(result: &Value) -> Value {
    json!({
        "wakes": result["wakes"],
        "sources": result["sources"],
        "clients": result["clients"],
        "clients_gone": result["clients_gone"],
        "unnamed_client_wakes": result["unnamed_client_wakes"],
        "programs": result["programs"],
        "frames": result["frames"],
        "draw_causes": result["draw_causes"],
    })
}

fn nothing_named() -> Value {
    json!({
        "wakes": 0,
        "sources": [],
        "clients": [],
        "clients_gone": {"clients": 0, "wakes": 0, "commits": 0, "draws": 0},
        "unnamed_client_wakes": 0,
        "programs": [],
        "frames": 0,
        "draw_causes": [],
    })
}

/// The wakes of `source` in a result; 0 when it is not listed.
fn wakes_of(result: &Value, source: &str) -> u64 {
    result["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["source"] == json!(source))
        .map_or(0, |entry| entry["wakes"].as_u64().expect("a count"))
}

/// The frames of `cause` in a result; 0 when it is not listed.
fn frames_of(result: &Value, cause: &str) -> u64 {
    result["draw_causes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["cause"] == json!(cause))
        .map_or(0, |entry| entry["frames"].as_u64().expect("a count"))
}

/// A still window on its seat, its frame drawn and called back: one second of `rest` names
/// nothing that woke Yuna and nothing drawn, its own timer, and the wake it caused alone, left
/// out; every seat's names nothing either. Without `wait_ms`, what was counted since Yuna started:
/// the requests, the seat's first frame, the window's commits.
#[test]
fn at_rest_a_second_names_nothing_woken_and_nothing_drawn() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = small_seat(&mut mcp);
    let (_client, _window) = open_drawn_window(&mut mcp, seat);

    let began = Instant::now();
    let rested = mcp.call("rest", json!({"seat": seat, "wait_ms": 1000}));
    assert!(began.elapsed() >= Duration::from_secs(1), "{rested}");
    let rested = &rested["structuredContent"];
    assert_eq!(named(rested), nothing_named(), "{rested}");
    assert_eq!(rested["seat"], json!(seat));
    assert_eq!(rested["wait_ms"], json!(1000));
    let since = rested["since_ns"].as_u64().expect("a time");
    let now = rested["now_ns"].as_u64().expect("a time");
    assert!(now >= since + 1_000_000_000, "{rested}");

    let everyone = mcp.call("rest", json!({"wait_ms": 1000}))["structuredContent"].clone();
    assert_eq!(named(&everyone), nothing_named(), "{everyone}");
    assert_eq!(everyone["seat"], Value::Null);

    let started = mcp.call("rest", json!({}))["structuredContent"].clone();
    assert_eq!(started["wait_ms"], Value::Null);
    assert!(wakes_of(&started, "mcp_input") >= 4, "{started}");
    assert!(wakes_of(&started, "waiting_call") >= 2, "{started}");
    assert_eq!(frames_of(&started, "first_frame"), 1, "{started}");
    // The window's first buffer opened it, its client's commit, as its second is.
    assert!(frames_of(&started, "commit") >= 1, "{started}");
    let pid = std::process::id();
    let client = &started["clients"][0];
    assert_eq!(client["pid"], json!(pid), "{started}");
    assert_eq!(client["seat"], json!(seat));
    assert_eq!(client["program"], Value::Null);
    assert_eq!(client["xwayland"], json!(false));
    assert!(client["commits"].as_u64() >= Some(3), "{started}");
    assert!(client["draws"].as_u64() >= Some(1), "{started}");
}

/// A client that draws as fast as its frames are called back is named by its process and seat,
/// with its commits and the frames they caused; the frames counted are the commit's, and the
/// clients' source counts its wakes. The window's own counts are its alone: what the client did
/// before is not in them.
#[test]
fn a_client_that_animates_is_named_with_its_commits_as_the_frames_cause() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = small_seat(&mut mcp);
    let (mut client, window) = open_drawn_window(&mut mcp, seat);
    send_rest(&mut mcp, 900, json!({"seat": seat, "wait_ms": 2000}));
    // As though the last frame had just been called back: the first draws at once.
    let mut called_back = client.window(window).frames.len() - 1;
    let mut drawn = 0;
    let until = Instant::now() + Duration::from_secs(10);
    let reply = loop {
        assert!(Instant::now() < until, "no answer to rest");
        if let Some(reply) = mcp.replies().into_iter().find(|r| r["id"] == json!(900)) {
            break reply;
        }
        client.read_events();
        if client.window(window).frames.len() > called_back {
            called_back = client.window(window).frames.len();
            let colour = if drawn % 2 == 0 { 0xFF_00_00_FF } else { RED };
            client.attach(window, 64, 64, colour);
            client.request_frame(window);
            client.commit(window);
            client.send();
            drawn += 1;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let window_counts = &reply["result"]["structuredContent"];
    let clients = window_counts["clients"].as_array().expect("clients");
    assert_eq!(clients.len(), 1, "{window_counts}");
    let named = &clients[0];
    assert_eq!(named["pid"], json!(std::process::id()));
    assert_eq!(named["seat"], json!(seat));
    let commits = named["commits"].as_u64().expect("commits");
    let draws = named["draws"].as_u64().expect("draws");
    let wakes = named["wakes"].as_u64().expect("wakes");
    eprintln!("in 2 s: {drawn} frames sent, {commits} commits, {draws} draws, {wakes} wakes");
    // Its rate follows the machine's load, a frame a second under a load of 38; each commit is a
    // new buffer, drawn once, the last one's frame and the one before the window's on either side
    // of it.
    assert!(commits >= 2 && draws >= 1, "{window_counts}");
    assert!(
        commits <= draws + 1 && draws <= commits + 1 && commits <= drawn + 1,
        "{window_counts}"
    );
    assert!(wakes >= draws, "{window_counts}");
    assert_eq!(frames_of(window_counts, "commit"), draws, "{window_counts}");
    assert!(
        window_counts["frames"].as_u64() >= Some(draws),
        "{window_counts}"
    );
    assert_eq!(wakes_of(window_counts, "clients"), wakes, "{window_counts}");
    assert_eq!(window_counts["unnamed_client_wakes"], json!(0));
}

/// A `windows` call's timer, which ends its wait 300 ms in, is named in a `rest` call's second,
/// alone: the `rest` call's own timer, and the wake it caused alone, are left out.
#[test]
fn a_waiting_calls_timer_is_named_and_the_rest_calls_own_left_out() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = small_seat(&mut mcp);
    settle(&mut mcp);
    mcp.send(&json!({"jsonrpc": "2.0", "id": 800, "method": "tools/call",
                     "params": modern(json!({"name": "windows", "arguments":
                         {"seat": seat, "app_id": "none", "wait_ms": 300}}))}));
    send_rest(&mut mcp, 801, json!({"wait_ms": 1000}));
    let timed_out = mcp.reply_to(800);
    assert_eq!(
        timed_out["result"]["structuredContent"]["timed_out"],
        json!(true)
    );
    let window_counts = mcp.reply_to(801)["result"]["structuredContent"].clone();
    assert_eq!(
        named(&window_counts),
        json!({
            "wakes": 1,
            "sources": [{"source": "waiting_call", "wakes": 1}],
            "clients": [],
            "clients_gone": {"clients": 0, "wakes": 0, "commits": 0, "draws": 0},
            "unnamed_client_wakes": 0,
            "programs": [],
            "frames": 0,
            "draw_causes": [],
        }),
        "{window_counts}"
    );
}

/// A `rest` call that waits counts among the calls that wait, fails when its seat closes,
/// and is dropped unanswered when cancelled; a seat that does not exist and a wait out of range
/// are refused.
#[test]
fn a_rest_call_waits_as_the_others_do() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = small_seat(&mut mcp);
    send_rest(&mut mcp, 700, json!({"seat": seat, "wait_ms": 10_000}));
    send_rest(&mut mcp, 701, json!({"wait_ms": 10_000}));
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    mcp.send(
        &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                     "params": {"requestId": 701, "reason": "the test"}}),
    );
    mcp.send(&json!({"jsonrpc": "2.0", "id": 702, "method": "tools/call",
                     "params": modern(json!({"name": "seat",
                                             "arguments": {"action": "close", "seat": seat}}))}));
    let replies = replies_to(&mut mcp, &[700, 702]);
    let (failed, closed) = (&replies[0], &replies[1]);
    assert_eq!(closed["result"]["isError"], json!(false), "{closed}");
    assert_eq!(failed["result"]["isError"], json!(true), "{failed}");
    let text = failed["result"]["content"][0]["text"]
        .as_str()
        .expect("a reason");
    assert!(text.contains("was closed"), "{text}");
    std::thread::sleep(Duration::from_millis(20));
    assert!(mcp.replies().iter().all(|reply| reply["id"] != json!(701)));

    for bad in [
        json!({"seat": seat}),
        json!({"wait_ms": 0}),
        json!({"wait_ms": 60_000}),
        json!({"wait": 5}),
    ] {
        let refused = mcp.call("rest", bad.clone());
        assert_eq!(refused["isError"], json!(true), "{bad}: {refused}");
    }

    let seat = small_seat(&mut mcp);
    for id in 0..yuna::mcp::MAX_WAITING_CALLS as u64 {
        send_rest(
            &mut mcp,
            1000 + id,
            json!({"seat": seat, "wait_ms": 10_000}),
        );
    }
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    let refused = mcp.call("rest", json!({"wait_ms": 10_000}));
    assert_eq!(refused["isError"], json!(true), "{refused}");
    let refused = mcp.call(
        "windows",
        json!({"seat": seat, "app_id": "none", "wait_ms": 10_000}),
    );
    assert_eq!(refused["isError"], json!(true), "{refused}");
}

/// A refresh that a change of rate disarms before it runs takes its causes with it: the frame
/// the change draws is the change's, not the commit's, whose client is named for no draw.
#[test]
fn a_refresh_disarmed_by_a_change_of_rate_takes_its_causes_with_it() {
    use yuna::OutputMode;
    use yuna::rest::Cause;
    let mut compositor = crate::manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let window = client.open_window(&mut compositor);
    client.ack(window);
    client.attach(window, crate::TILE.0, crate::TILE.1, RED);
    client.commit(window);
    client.roundtrip(&mut compositor);
    crate::run_armed_deadlines(&mut compositor);
    assert_eq!(compositor.next_deadline(), None);
    let before = compositor.rest_counts();

    client.attach(window, crate::TILE.0, crate::TILE.1, crate::BLUE);
    client.commit(window);
    client.roundtrip(&mut compositor);
    assert!(
        compositor.next_deadline().is_some(),
        "the commit armed nothing"
    );
    let mode = OutputMode::new(3840, 2160, 120.0, 1.5).expect("a mode");
    compositor
        .configure_output(crate::OUTPUT, mode)
        .expect("the rate changes");
    crate::run_armed_deadlines(&mut compositor);
    let window_counts = compositor.rest_counts().since(&before);
    assert_eq!(window_counts.whole.frames, 1, "{window_counts:?}");
    assert_eq!(window_counts.whole.frames_by(Cause::OutputChange), 1);
    assert_eq!(
        window_counts.whole.frames_by(Cause::Commit),
        0,
        "{window_counts:?}"
    );
    let named = window_counts.clients.values().next().expect("the client");
    assert_eq!((named.tally.commits, named.tally.draws), (1, 0));
}

/// A window's first buffer, which opens it, draws as its client's commit, not as a change of the
/// layout (ADR-0158). A commit that brings nothing new, a frame callback alone, is answered at the
/// next refresh but is no cause of what that refresh draws: another client's new buffer is, and
/// only that client is named for the draw.
#[test]
fn a_first_buffer_is_its_clients_commit_and_a_bare_commit_causes_nothing() {
    use yuna::rest::Cause;
    let mut compositor = crate::manual_compositor();
    crate::run_armed_deadlines(&mut compositor);
    let before = compositor.rest_counts();

    let mut opener = Client::connect(&mut compositor);
    let first = opener.open_window(&mut compositor);
    opener.ack(first);
    opener.attach(first, crate::TILE.0, crate::TILE.1, RED);
    opener.commit(first);
    opener.roundtrip(&mut compositor);
    crate::run_armed_deadlines(&mut compositor);
    let opened = compositor.rest_counts().since(&before);
    assert!(opened.whole.frames >= 1, "{opened:?}");
    assert_eq!(opened.whole.frames_by(Cause::LayoutChange), 0, "{opened:?}");
    assert_eq!(
        opened.whole.frames_by(Cause::Commit),
        opened.whole.frames,
        "{opened:?}"
    );
    let named = opened.clients.values().next().expect("the opener");
    assert!(named.tally.draws >= 1, "{opened:?}");

    let mut painter = Client::connect(&mut compositor);
    let second = painter.open_window(&mut compositor);
    painter.ack(second);
    painter.attach(second, crate::TILE.0, crate::TILE.1, crate::BLUE);
    painter.commit(second);
    painter.roundtrip(&mut compositor);
    crate::run_armed_deadlines(&mut compositor);
    opener.roundtrip(&mut compositor);
    assert_eq!(compositor.next_deadline(), None);
    let settled = compositor.rest_counts();
    let called_back = opener.window(first).frames.len();

    opener.request_frame(first);
    opener.commit(first);
    opener.send();
    painter.attach(second, crate::TILE.0, crate::TILE.1, RED);
    painter.commit(second);
    painter.roundtrip(&mut compositor);
    crate::run_armed_deadlines(&mut compositor);
    opener.roundtrip(&mut compositor);
    assert_eq!(
        opener.window(first).frames.len(),
        called_back + 1,
        "the bare commit's frame callback is answered"
    );
    let drawn = compositor.rest_counts().since(&settled);
    assert_eq!(drawn.whole.frames, 1, "{drawn:?}");
    assert_eq!(drawn.whole.frames_by(Cause::Commit), 1, "{drawn:?}");
    let tallies: Vec<(u64, u64)> = drawn
        .clients
        .values()
        .map(|client| (client.tally.commits, client.tally.draws))
        .collect();
    assert_eq!(tallies, vec![(1, 0), (1, 1)], "{drawn:?}");
}

/// Commits without a buffer that still change what is drawn, a subsurface moved, are the cause of
/// the frame they draw when nothing else is: no frame is drawn without a cause, and their client
/// is named for the draw. (Smithay applies a desynchronised subsurface's new place at that
/// subsurface's own commit, not at its parent's alone: both commit here.)
#[test]
fn a_bare_commit_that_moves_a_subsurface_is_the_cause_of_its_frame() {
    use yuna::rest::Cause;
    let mut compositor = crate::manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let window = client.open_window(&mut compositor);
    client.ack(window);
    client.attach(window, crate::TILE.0, crate::TILE.1, RED);
    client.commit(window);
    let child = client.add_subsurface(window);
    // Its parent's commit maps it.
    client.attach(window, crate::TILE.0, crate::TILE.1, RED);
    client.commit(window);
    client.roundtrip(&mut compositor);
    crate::run_armed_deadlines(&mut compositor);
    client.roundtrip(&mut compositor);
    crate::run_armed_deadlines(&mut compositor);
    assert_eq!(compositor.next_deadline(), None);
    let settled = compositor.rest_counts();

    client.subsurface(child).role.set_position(40, 40);
    client.subsurface(child).surface.commit();
    client.commit(window);
    client.roundtrip(&mut compositor);
    crate::run_armed_deadlines(&mut compositor);
    let moved = compositor.rest_counts().since(&settled);
    assert_eq!(moved.whole.frames, 1, "{moved:?}");
    assert_eq!(moved.whole.frames_by(Cause::Commit), 1, "{moved:?}");
    let named = moved.clients.values().next().expect("the client");
    assert_eq!(
        (named.tally.commits, named.tally.draws),
        (2, 1),
        "{moved:?}"
    );
}

/// A window that opens on one seat is its client's commit on that seat's outputs alone: another
/// seat's output, which draws its own client's buffer at the same refresh, names that client
/// alone.
#[test]
fn a_window_opened_on_one_seat_is_no_cause_of_another_seats_frames() {
    use yuna::OutputMode;
    use yuna::rest::Cause;
    let mut compositor = crate::manual_compositor();
    let mode = OutputMode::new(640, 480, 60.0, 1.0).expect("a mode");
    let (a, b) = (
        compositor.add_seat().expect("a seat"),
        compositor.add_seat().expect("a seat"),
    );
    compositor.add_output(a, mode).expect("an output");
    compositor.add_output(b, mode).expect("an output");
    let mut painter = Client::connect_to(&mut compositor, b);
    let painted = painter.open_window(&mut compositor);
    painter.ack(painted);
    painter.attach(painted, 64, 64, RED);
    painter.commit(painted);
    painter.roundtrip(&mut compositor);
    for _ in 0..4 {
        crate::run_armed_deadlines(&mut compositor);
        painter.roundtrip(&mut compositor);
    }
    assert_eq!(compositor.next_deadline(), None);
    let mut opener = Client::connect_to(&mut compositor, a);
    let opened = opener.open_window(&mut compositor);
    opener.ack(opened);
    let settled = compositor.rest_counts();

    opener.attach(opened, 64, 64, crate::BLUE);
    opener.commit(opened);
    opener.send();
    painter.attach(painted, 64, 64, crate::BLUE);
    painter.commit(painted);
    painter.roundtrip(&mut compositor);
    opener.roundtrip(&mut compositor);
    crate::run_armed_deadlines(&mut compositor);
    let drawn = compositor.rest_counts().since(&settled);
    assert_eq!(drawn.seats[&b].frames, 1, "{drawn:?}");
    assert_eq!(
        drawn.seats[&b].frames_by(Cause::LayoutChange),
        0,
        "{drawn:?}"
    );
    assert!(drawn.seats[&a].frames >= 1, "{drawn:?}");
    let draws: Vec<(SeatId, u64)> = drawn
        .clients
        .values()
        .map(|client| (client.seat, client.tally.draws))
        .collect();
    assert_eq!(
        draws,
        vec![(b, 1), (a, drawn.seats[&a].frames)],
        "{drawn:?}"
    );
}
