//! The MCP server in the test's own process (ADR-0033): requests written to its input, its
//! replies read from its output, each through a socket pair of its own, and the compositor's
//! windows opened by test clients of a seat. Its compositor is `yuna mcp`'s: no output, no socket
//! and no `wl_seat` for the owner's seat. `spawn` and the seats' sockets need a process that
//! reaps its children, so they are tested on the program, in `tests/mcp.rs`.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yuna::colour::description::PG32UCDM;
use yuna::mcp::{
    MAX_LINE, MAX_OUTPUT_PIXELS, MAX_OUTPUT_SIDE, MAX_QUEUED_OUTPUT, MAX_WAITING_CALLS,
    MAX_WORKSPACE_NAME, MODERN_REVISION, Server, Stop,
};
use yuna::{ClockMode, Compositor, Config, OutputMode};
use yuna_types::SeatId;

use crate::RED;
use crate::client::Client;
use crate::colour_client::{ColourClient, Primaries, RenderIntent, TransferFunction};

/// A test's end of a server: what it writes to, what it reads from.
pub struct Mcp {
    pub server: Server,
    requests: UnixStream,
    replies: UnixStream,
    read: Vec<u8>,
    next_id: u64,
}

impl Mcp {
    pub fn new(clock: ClockMode) -> Mcp {
        let compositor = Compositor::new(Config {
            outputs: Vec::new(),
            clock,
            listen: false,
            announce_owner_seat: false,
            ..Config::default()
        })
        .expect("the compositor starts");
        Mcp::serving(compositor)
    }

    /// A server for `compositor`, a story's own: one with XWayland on, say.
    pub fn serving(compositor: Compositor) -> Mcp {
        let (input, requests) = UnixStream::pair().expect("a socket pair");
        let (output, replies) = UnixStream::pair().expect("a socket pair");
        replies.set_nonblocking(true).expect("non-blocking");
        let server = Server::new(
            compositor,
            OwnedFd::from(input),
            OwnedFd::from(output),
            None,
        )
        .expect("the server starts");
        Mcp {
            server,
            requests,
            replies,
            read: Vec::new(),
            next_id: 1,
        }
    }

    pub fn send(&mut self, message: &Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.requests
            .write_all(line.as_bytes())
            .expect("the request is sent");
    }

    /// Sends a request on a thread of its own: one longer than the socket's buffer, which the
    /// story's thread, the server's too, would wait on for ever. Join it once it is answered.
    pub fn send_on_thread(&mut self, message: &Value) -> std::thread::JoinHandle<()> {
        let mut line = message.to_string();
        line.push('\n');
        let mut writer = self.requests.try_clone().expect("a second handle");
        std::thread::spawn(move || {
            writer
                .write_all(line.as_bytes())
                .expect("the request is sent");
        })
    }

    /// Lets the server run a round, then reads the replies that came.
    pub fn replies(&mut self) -> Vec<Value> {
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
        let mut replies = Vec::new();
        while let Some(end) = self.read.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.read.drain(..=end).collect();
            replies.push(serde_json::from_slice(&line).expect("a reply is one JSON line"));
        }
        replies
    }

    /// Sends a request and waits, up to 5 s of real time, for its reply.
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.reply_to(id)
    }

    pub fn reply_to(&mut self, id: u64) -> Value {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if let Some(reply) = self.replies().into_iter().find(|r| r["id"] == json!(id)) {
                return reply;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("no reply to request {id}");
    }

    /// Creates a seat with `seat create` and returns its handle.
    pub fn seat(&mut self) -> u64 {
        let created = self.call("seat", json!({"action": "create"}));
        assert_eq!(created["isError"], json!(false), "{created}");
        created["structuredContent"]["seat"]
            .as_u64()
            .expect("a handle")
    }

    /// Calls a tool in 2026-07-28 and returns its result.
    pub fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let reply = self.request(
            "tools/call",
            modern(json!({"name": tool, "arguments": arguments})),
        );
        reply["result"].clone()
    }
}

/// `params` with 2026-07-28's `_meta`.
pub fn modern(mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": MODERN_REVISION,
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    params
}

/// Both revisions list the same tools; 2026-07-28 needs no earlier request, names its
/// revision and server in every result, and answers another revision with -32022; 2025-11-25
/// is served once a client opened with `initialize`.
#[test]
fn both_revisions_are_served_and_list_the_same_tools() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let modern_list = mcp.request("tools/list", modern(json!({})));
    assert_eq!(modern_list["result"]["resultType"], json!("complete"));
    assert_eq!(
        modern_list["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        json!("yuna")
    );
    let discover = mcp.request("server/discover", modern(json!({})));
    assert_eq!(
        discover["result"]["supportedVersions"],
        json!([MODERN_REVISION])
    );
    let old = mcp.request(
        "tools/list",
        json!({"_meta": {
            "io.modelcontextprotocol/protocolVersion": "2025-03-26",
            "io.modelcontextprotocol/clientCapabilities": {}
        }}),
    );
    assert_eq!(old["error"]["code"], json!(-32022));
    assert_eq!(old["error"]["data"]["supported"], json!([MODERN_REVISION]));
    let before_initialize = mcp.request("tools/list", json!({}));
    assert_eq!(before_initialize["error"]["code"], json!(-32602));

    let initialize = mcp.request(
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {},
               "clientInfo": {"name": "test", "version": "1"}}),
    );
    assert_eq!(initialize["result"]["protocolVersion"], json!("2025-11-25"));
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let legacy_list = mcp.request("tools/list", json!({}));
    assert_eq!(legacy_list["result"].get("resultType"), None);
    assert_eq!(
        legacy_list["result"]["tools"],
        modern_list["result"]["tools"]
    );
    assert_eq!(mcp.request("ping", json!({}))["result"], json!({}));
    let unknown = mcp.request("tools/call", json!({"name": "teleport"}));
    assert_eq!(unknown["error"]["code"], json!(-32602));

    // A line that is not JSON, or JSON that is no request, is answered without an id, and the
    // server goes on.
    mcp.requests
        .write_all(b"{not json\n")
        .expect("the line is sent");
    mcp.send(&json!("not a request"));
    let replies = mcp.replies();
    assert_eq!(replies[0]["error"]["code"], json!(-32700));
    assert_eq!(replies[1]["error"]["code"], json!(-32600));
    assert!(replies.iter().all(|reply| reply["id"] == Value::Null));
    assert_eq!(mcp.server.stopping(), None);
}

/// `outputs` creates, lists and removes a seat's outputs; bad modes, the seat's last output and
/// a call without its seat are refused as tool errors, which the model reads.
#[test]
fn outputs_are_created_listed_and_removed() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let no_seat = mcp.call("outputs", json!({"action": "list"}));
    assert_eq!(no_seat["isError"], json!(true));
    let unknown_seat = mcp.call("outputs", json!({"action": "list", "seat": 2}));
    assert_eq!(unknown_seat["isError"], json!(true));
    let text = unknown_seat["content"][0]["text"]
        .as_str()
        .expect("a reason");
    assert!(text.contains("seat create"), "{text}");
    let seat = mcp.seat();
    assert_eq!(seat, 2);
    let created = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "width": 1920, "height": 1080, "rate": 60,
               "scale": 1}),
    );
    assert_eq!(created["isError"], json!(false));
    let output = &created["structuredContent"]["output"];
    assert_eq!(output["id"], json!(2));
    assert_eq!(output["seat"], json!(2));
    assert_eq!(output["name"], json!("HEADLESS-2"));
    assert_eq!(output["refresh_mhz"], json!(60_000));
    assert_eq!(
        output["logical"],
        json!({"x": 2560.0, "y": 0.0, "width": 1920.0, "height": 1080.0})
    );
    let listed = mcp.call("outputs", json!({"action": "list", "seat": seat}));
    let ids: Vec<&Value> = listed["structuredContent"]["outputs"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|output| &output["id"])
        .collect();
    assert_eq!(ids, [&json!(1), &json!(2)]);

    let bad = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "rate": 5000}),
    );
    assert_eq!(bad["isError"], json!(true));
    let unknown_field = mcp.call(
        "outputs",
        json!({"action": "list", "seat": seat, "verbose": true}),
    );
    assert_eq!(unknown_field["isError"], json!(true));

    let removed = mcp.call(
        "outputs",
        json!({"action": "remove", "seat": seat, "output": 1}),
    );
    assert_eq!(removed["structuredContent"], json!({"removed": 1}));
    let last = mcp.call(
        "outputs",
        json!({"action": "remove", "seat": seat, "output": 2}),
    );
    assert_eq!(last["isError"], json!(true));
    assert_eq!(mcp.server.compositor().outputs().len(), 1);
}

/// A seat's output may run under VRR and announce the owner's monitor's HDR colours, which
/// `outputs list` tells; a range less than twice as wide as its least rate is refused. `windows`
/// gives the colours a window's surfaces declared only when asked, when it answers at once and
/// when it answers a waiting call (ADR-0049).
#[test]
fn vrr_and_hdr_outputs_are_listed_and_declared_colours_given_when_asked() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let narrow = mcp.call(
        "seat",
        json!({"action": "create", "rate": 240, "vrr_min": 200}),
    );
    assert_eq!(narrow["isError"], json!(true));
    let text = narrow["content"][0]["text"].as_str().expect("a reason");
    assert!(text.contains("VRR range of 200 to 240 Hz"), "{text}");
    let created = mcp.call(
        "seat",
        json!({"action": "create", "width": 1920, "height": 1080, "rate": 240, "scale": 1,
               "vrr_min": 48, "hdr": true}),
    );
    assert_eq!(created["isError"], json!(false), "{created}");
    let seat = created["structuredContent"]["seat"]
        .as_u64()
        .expect("a handle");
    let listed = mcp.call("outputs", json!({"action": "list", "seat": seat}));
    let output = &listed["structuredContent"]["outputs"][0];
    assert_eq!(output["refresh_mhz"], json!(240_000));
    assert_eq!(output["vrr"], json!({"min": 48.0, "max": 240.0}));
    assert_eq!(output["hdr"], json!("PG32UCDM"));

    let narrow = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "rate": 240, "vrr_min": 200}),
    );
    assert_eq!(narrow["isError"], json!(true));
    let text = narrow["content"][0]["text"].as_str().expect("a reason");
    assert!(text.contains("VRR range of 200 to 240 Hz"), "{text}");
    let plain = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "width": 640, "height": 480, "rate": 60,
               "scale": 1}),
    );
    assert_eq!(plain["structuredContent"]["output"]["vrr"], Value::Null);
    assert_eq!(plain["structuredContent"]["output"]["hdr"], Value::Null);

    // A call that waits for the window, asking for its colours.
    mcp.send(&json!({"jsonrpc": "2.0", "id": 300, "method": "tools/call",
                      "params": modern(json!({"name": "windows", "arguments": {
                          "seat": seat, "app_id": "org.example.Hdr", "colour": true,
                          "wait_ms": 10_000}}))}));
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    // A window whose first buffer comes with PQ over BT.2020, the least an HDR10 client sends.
    let mut colour = ColourClient::connect(mcp.server.compositor(), SeatId(seat), 3);
    let params = colour.params();
    params.set_tf_named(TransferFunction::St2084Pq);
    params.set_primaries_named(Primaries::Bt2020);
    let description = colour.create(params);
    colour.roundtrip(mcp.server.compositor());
    assert!(colour.events(description).ready.is_some());
    let window = colour.client.open_window(mcp.server.compositor());
    colour
        .client
        .set_app_id_and_title(window, "org.example.Hdr", "HDR");
    colour.client.ack(window);
    let surface = colour.client.window(window).surface.clone();
    colour
        .colour_surface(&surface)
        .set_image_description(colour.description(description), RenderIntent::Perceptual);
    colour.client.attach(window, 400, 300, RED);
    colour.client.commit(window);
    colour.roundtrip(mcp.server.compositor());
    crate::run_armed_deadlines(mcp.server.compositor());

    let bt2020 = json!({
        "red": [0.708, 0.292], "green": [0.17, 0.797], "blue": [0.131, 0.046],
        "white": [0.3127, 0.329],
    });
    // PQ's luminances when a client sets none: 0.005, 10,000 and 203 cd/m²; its target, the
    // same volume.
    let declared = json!([{
        "role": "window",
        "transfer_function": "st2084_pq",
        "primaries": "bt2020",
        "luminances": {"min": 0.005, "max": 10_000, "reference": 203},
        "target_primaries": bt2020,
        "target_luminance": {"min": 0.005, "max": 10_000},
        "max_cll": null,
        "max_fall": null,
        "intent": "perceptual",
    }]);
    let waited = mcp.reply_to(300);
    let windows = &waited["result"]["structuredContent"]["windows"];
    assert_eq!(windows[0]["app_id"], json!("org.example.Hdr"));
    assert_eq!(windows[0]["colours"], declared);

    let asked = mcp.call("windows", json!({"seat": seat, "colour": true}));
    let with_colours = &asked["structuredContent"]["windows"][0];
    assert_eq!(with_colours["colours"], declared);
    for arguments in [
        json!({"seat": seat}),
        json!({"seat": seat, "colour": false}),
    ] {
        let listed = mcp.call("windows", arguments);
        let window = &listed["structuredContent"]["windows"][0];
        assert_eq!(window.get("colours"), None, "{listed}");
        // Nothing else differs.
        let mut without = with_colours.clone();
        without
            .as_object_mut()
            .expect("an object")
            .remove("colours");
        assert_eq!(window, &without);
    }
    // A window that declared nothing gives an empty list.
    let mut other = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let undeclared = other.open_window(mcp.server.compositor());
    other.ack(undeclared);
    other.attach(undeclared, 400, 300, RED);
    other.commit(undeclared);
    other.roundtrip(mcp.server.compositor());
    crate::run_armed_deadlines(mcp.server.compositor());
    let asked = mcp.call("windows", json!({"seat": seat, "colour": true}));
    let windows = &asked["structuredContent"]["windows"];
    assert_eq!(windows[0]["colours"], declared);
    assert_eq!(windows[1]["colours"], json!([]));
}

/// `yuna mcp --output`'s VRR range and HDR monitor reach each new seat's output, as its size
/// does; a call's own values replace them.
#[test]
fn the_seats_default_output_gives_its_vrr_range_and_hdr_monitor() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let default = OutputMode::new(1920, 1080, 240.0, 1.0)
        .and_then(|mode| mode.with_vrr(48.0))
        .expect("a mode")
        .with_hdr(PG32UCDM);
    mcp.server.set_seat_output(default);
    let seat = mcp.seat();
    let listed = mcp.call("outputs", json!({"action": "list", "seat": seat}));
    let output = &listed["structuredContent"]["outputs"][0];
    assert_eq!(output["width"], json!(1920));
    assert_eq!(output["vrr"], json!({"min": 48.0, "max": 240.0}));
    assert_eq!(output["hdr"], json!("PG32UCDM"));

    let own = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "vrr_min": 60, "hdr": false}),
    );
    let output = &own["structuredContent"]["output"];
    assert_eq!(output["vrr"], json!({"min": 60.0, "max": 240.0}), "{own}");
    assert_eq!(output["hdr"], Value::Null);
    // A call that names its own rate gets no default range: a fixed 60 Hz, though 48 Hz is
    // more than half of 60.
    let slow = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "rate": 60}),
    );
    let output = &slow["structuredContent"]["output"];
    assert_eq!(output["vrr"], Value::Null, "{slow}");
    assert_eq!(output["refresh_mhz"], json!(60_000));
}

/// `windows` waits for a window that matches, and is answered as soon as one maps; a waiting
/// call that is cancelled is never answered; one whose time is up is answered with no window.
#[test]
fn windows_waits_for_a_window_until_it_maps_its_time_is_up_or_it_is_cancelled() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let empty = mcp.call("windows", json!({"seat": seat}));
    assert_eq!(empty["structuredContent"], json!({"windows": []}));
    let waits = [
        (
            100,
            json!({"seat": seat, "app_id": "org.example.Test", "wait_ms": 10_000}),
        ),
        (101, json!({"seat": seat, "wait_ms": 10_000})),
        (
            102,
            json!({"seat": seat, "app_id": "org.example.Other", "wait_ms": 30}),
        ),
    ];
    for (id, arguments) in &waits {
        mcp.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                          "params": modern(json!({"name": "windows", "arguments": arguments}))}));
    }
    mcp.send(
        &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                      "params": {"requestId": 101, "reason": "the test"}}),
    );
    assert_eq!(mcp.replies(), Vec::<Value>::new());

    let timed_out = mcp.reply_to(102);
    assert_eq!(
        timed_out["result"]["structuredContent"],
        json!({"windows": [], "timed_out": true})
    );

    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = client.create_window();
    client.set_app_id_and_title(window, "org.example.Test", "Test");
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    client.ack(window);
    client.attach(window, crate::TILE.0, crate::TILE.1, RED);
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    // Mapped, not yet placed: the call waits for the refresh that arranges the layout, which
    // the manual clock reaches only when moved.
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    crate::run_armed_deadlines(mcp.server.compositor());
    let found = mcp.reply_to(100);
    let windows = &found["result"]["structuredContent"]["windows"];
    assert_eq!(windows[0]["app_id"], json!("org.example.Test"));
    assert_eq!(windows[0]["seat"], json!(seat));
    assert_eq!(windows[0]["title"], json!("Test"));
    assert_eq!(windows[0]["output"], json!(1));
    assert_eq!(
        windows[0]["rect"],
        json!({"x": 16.0, "y": 16.0, "width": 1256.0, "height": 1408.0})
    );
    // The cancelled call stays unanswered.
    std::thread::sleep(Duration::from_millis(20));
    assert!(mcp.replies().iter().all(|reply| reply["id"] != json!(101)));

    let too_long = mcp.call("windows", json!({"seat": seat, "wait_ms": 60_000}));
    assert_eq!(too_long["isError"], json!(true));
}

/// At most 64 calls wait at once: the next is refused at once, as a tool error.
#[test]
fn at_most_64_calls_wait() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    for id in 0..MAX_WAITING_CALLS {
        mcp.send(
            &json!({"jsonrpc": "2.0", "id": id + 500, "method": "tools/call",
                          "params": modern(json!({"name": "windows",
                                                  "arguments": {"seat": seat, "wait_ms": 10_000}}))}),
        );
    }
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    let refused = mcp.call("windows", json!({"seat": seat, "wait_ms": 10_000}));
    assert_eq!(refused["isError"], json!(true));
}

/// A manual clock moves through `clock`, running the frames due on the way, never past real
/// time; a real clock refuses to be moved.
#[test]
fn the_clock_steps_only_a_manual_clock_and_never_past_real_time() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let now = mcp.call("clock", json!({"action": "now"}))["structuredContent"].clone();
    assert_eq!(now["mode"], json!("manual"));
    let start = now["now_ns"].as_u64().expect("a time");
    assert!(now["real_now_ns"].as_u64().expect("a time") >= start);
    let ahead = mcp.call("clock", json!({"action": "advance", "by_ms": 3_600_000}));
    assert_eq!(ahead["isError"], json!(true));
    let text = ahead["content"][0]["text"].as_str().expect("a reason");
    assert!(text.contains("never runs ahead of real time"), "{text}");
    std::thread::sleep(Duration::from_millis(5));
    let stepped = mcp.call("clock", json!({"action": "advance", "by_ns": 2_000_000}));
    assert_eq!(stepped["isError"], json!(false));
    assert_eq!(
        stepped["structuredContent"]["now_ns"].as_u64(),
        Some(start + 2_000_000)
    );
    let neither = mcp.call("clock", json!({"action": "advance"}));
    assert_eq!(neither["isError"], json!(true));

    let mut real = Mcp::new(ClockMode::Real);
    let refused = real.call("clock", json!({"action": "advance", "by_ns": 1}));
    assert_eq!(refused["isError"], json!(true));
    assert_eq!(
        real.call("clock", json!({"action": "now"}))["structuredContent"]["mode"],
        json!("real")
    );
}

/// A server that cannot reap children starts no program, and says so.
#[test]
fn spawn_is_refused_without_a_reaper() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let refused = mcp.call("spawn", json!({"action": "start", "command": ["true"]}));
    assert_eq!(refused["isError"], json!(true));
}

/// While a client does not read its replies, the server stops reading its requests once
/// 16 MiB of replies wait, and reads them again once the client has read; every reply comes,
/// in order.
#[test]
fn a_client_that_does_not_read_its_replies_stops_being_read() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let request = json!({"jsonrpc": "2.0", "id": 0, "method": "tools/list",
                         "params": modern(json!({}))});
    let reply_length = {
        let reply = mcp.request("tools/list", modern(json!({})));
        reply.to_string().len() + 1
    };
    let count = MAX_QUEUED_OUTPUT / reply_length * 2;
    let mut writer = mcp.requests.try_clone().expect("a second handle");
    let lines: String = (0..count)
        .map(|index| {
            let mut request = request.clone();
            request["id"] = json!(index + 1_000);
            request.to_string() + "\n"
        })
        .collect();
    let writing = std::thread::spawn(move || writer.write_all(lines.as_bytes()));
    let until = Instant::now() + Duration::from_secs(20);
    while !mcp.server.input_paused() {
        assert!(Instant::now() < until, "the input was never paused");
        mcp.server
            .dispatch(Some(Duration::from_millis(1)))
            .expect("the server dispatches");
    }
    // Each request is answered only while fewer than 16 MiB wait: the bound passes by one reply
    // at most.
    let queued = mcp.server.queued_output();
    assert!(queued >= MAX_QUEUED_OUTPUT, "{queued}");
    assert!(queued < MAX_QUEUED_OUTPUT + reply_length, "{queued}");
    // Paused, it reads nothing more: what waits does not grow.
    for _ in 0..10 {
        mcp.server
            .dispatch(Some(Duration::from_millis(1)))
            .expect("the server dispatches");
    }
    assert_eq!(mcp.server.queued_output(), queued);

    let mut ids = Vec::new();
    while ids.len() < count {
        assert!(Instant::now() < until, "{} replies of {count}", ids.len());
        ids.extend(
            mcp.replies()
                .iter()
                .filter_map(|reply| reply["id"].as_u64()),
        );
    }
    assert_eq!(ids, (1_000..1_000 + count as u64).collect::<Vec<_>>());
    writing
        .join()
        .expect("the writer")
        .expect("every request was written");
    assert!(!mcp.server.input_paused());
}

/// A line longer than 1 MiB ends the connection.
#[test]
fn a_line_over_a_mebibyte_ends_the_connection() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let mut writer = mcp.requests.try_clone().expect("a second handle");
    let writing = std::thread::spawn(move || {
        // The server stops reading partway: the rest of the write may fail.
        let _ = writer.write_all(&vec![b' '; MAX_LINE + 2]);
    });
    let until = Instant::now() + Duration::from_secs(10);
    while mcp.server.stopping().is_none() {
        assert!(Instant::now() < until, "the line was never refused");
        mcp.server
            .dispatch(Some(Duration::from_millis(1)))
            .expect("the server dispatches");
    }
    assert!(matches!(mcp.server.stopping(), Some(Stop::Broken(_))));
    drop(mcp);
    writing.join().expect("the writer");
}

/// A client that closes its input stops the server, which still writes the replies it owes.
#[test]
fn closing_the_input_stops_the_server_after_its_last_replies() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    mcp.send(&json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list",
                      "params": modern(json!({}))}));
    mcp.requests
        .shutdown(std::net::Shutdown::Write)
        .expect("the input closes");
    let until = Instant::now() + Duration::from_secs(5);
    let mut replies = Vec::new();
    while mcp.server.stopping().is_none() {
        assert!(Instant::now() < until, "the server never stopped");
        replies.extend(mcp.replies());
    }
    assert_eq!(mcp.server.stopping(), Some(&Stop::InputClosed));
    replies.extend(mcp.replies());
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["id"], json!(9));
    let Mcp { server, .. } = mcp;
    assert_eq!(server.finish(), Stop::InputClosed);
}

/// Two seats through MCP alone, on a real clock: each lists its own outputs and windows, an id
/// of the other's is unknown, the pointer and the keyboard reach a seat's own client, a window
/// moves to a workspace, and a closed seat's handle fails plainly. A manual-clock Yuna holds one
/// seat.
#[test]
fn seats_through_mcp_keep_to_their_own() {
    let mut manual = Mcp::new(ClockMode::Manual);
    manual.seat();
    let second = manual.call("seat", json!({"action": "create"}));
    assert_eq!(second["isError"], json!(true));

    let mut mcp = Mcp::new(ClockMode::Real);
    let a = mcp.seat();
    let created = mcp.call(
        "seat",
        json!({"action": "create", "width": 1280, "height": 720, "rate": 60, "scale": 1}),
    );
    let b = created["structuredContent"]["seat"]
        .as_u64()
        .expect("a handle");
    assert_eq!((a, b), (2, 3));
    assert_eq!(
        created["structuredContent"]["socket"],
        Value::Null,
        "no reaper, no socket"
    );
    let output_b = created["structuredContent"]["outputs"][0]["id"].clone();
    assert_eq!(
        created["structuredContent"]["outputs"][0]["logical"],
        json!({"x": 2560.0, "y": 0.0, "width": 1280.0, "height": 720.0})
    );
    let listed = mcp.call("seat", json!({"action": "list"}));
    assert_eq!(
        listed["structuredContent"]["seats"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    let outputs_a = mcp.call("outputs", json!({"action": "list", "seat": a}));
    assert_eq!(
        outputs_a["structuredContent"]["outputs"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    let theirs = mcp.call(
        "outputs",
        json!({"action": "remove", "seat": a, "output": output_b}),
    );
    assert_eq!(theirs["isError"], json!(true));
    assert!(mcp.call("pointer", json!({"action": "get", "seat": 1}))["isError"] == json!(true));

    let mut client =
        crate::input_client::InputClient::connect(mcp.server.compositor(), SeatId(a), "agent-2");
    let window = client.client.create_window();
    client.client.commit(window);
    client.roundtrip(mcp.server.compositor());
    client.client.ack(window);
    client.client.attach(window, 400, 300, RED);
    client.client.commit(window);
    client.roundtrip(mcp.server.compositor());
    let found = mcp.call("windows", json!({"seat": a, "wait_ms": 5_000}));
    let id = found["structuredContent"]["windows"][0]["id"].clone();
    assert_eq!(found["structuredContent"]["windows"][0]["seat"], json!(a));
    let none = mcp.call("windows", json!({"seat": b}));
    assert_eq!(none["structuredContent"]["windows"], json!([]));

    let clicked = mcp.call(
        "pointer",
        json!({"action": "click", "seat": a, "x": 26, "y": 26}),
    );
    let pointer = &clicked["structuredContent"]["pointer"];
    assert_eq!(pointer["window"], id, "{clicked}");
    assert_eq!(pointer["output"], json!(1));
    assert_eq!(pointer["surface_position"], json!({"x": 10.0, "y": 10.0}));
    assert_eq!(pointer["buttons"], json!([]));
    let half = mcp.call("pointer", json!({"action": "click", "seat": a, "x": 26}));
    assert_eq!(half["isError"], json!(true));
    let typed = mcp.call(
        "keyboard",
        json!({"action": "type", "seat": a, "text": "hé"}),
    );
    assert_eq!(typed["structuredContent"]["keyboard_window"], id, "{typed}");
    client.roundtrip(mcp.server.compositor());
    assert_eq!(client.state.typed, "hé");
    let on_b = mcp.call("keyboard", json!({"action": "tap", "seat": b, "keys": "a"}));
    assert_eq!(on_b["structuredContent"]["keyboard_window"], Value::Null);
    client.roundtrip(mcp.server.compositor());
    assert_eq!(client.state.typed, "hé");
    assert_eq!(typed["structuredContent"]["text_field"], json!(false));
    let refused = mcp.call(
        "keyboard",
        json!({"action": "preedit", "seat": a, "text": "にほ"}),
    );
    assert_eq!(refused["isError"], json!(true), "no field enabled");

    // Its text field enabled, text composed shows, and a character outside `us` is committed.
    client.enable_text_field(true);
    client.roundtrip(mcp.server.compositor());
    let read = mcp.call("keyboard", json!({"action": "get", "seat": a}));
    assert_eq!(
        read["structuredContent"],
        json!({"keyboard_window": id, "text_field": true, "layout": "us", "variant": "", "options": ""})
    );
    let composed = mcp.call(
        "keyboard",
        json!({"action": "preedit", "seat": a, "text": "にほ"}),
    );
    assert_eq!(composed["isError"], json!(false), "{composed}");
    client.roundtrip(mcp.server.compositor());
    assert_eq!(client.state.preedit.as_deref(), Some("にほ"));
    mcp.call(
        "keyboard",
        json!({"action": "type", "seat": a, "text": "日本"}),
    );
    client.roundtrip(mcp.server.compositor());
    assert_eq!(client.state.preedit, None);
    assert_eq!(client.state.received, "hé日本");

    let moved = mcp.call(
        "workspaces",
        json!({"action": "move_window", "seat": a, "window": id, "workspace": "web"}),
    );
    let workspaces = &moved["structuredContent"]["outputs"][0]["workspaces"];
    let web = workspaces
        .as_array()
        .expect("a list")
        .iter()
        .find(|workspace| workspace["name"] == json!("web"))
        .unwrap_or_else(|| panic!("{moved}"));
    assert_eq!(web["windows"], json!([id]));
    let not_ours = mcp.call(
        "workspaces",
        json!({"action": "move_window", "seat": b, "window": id, "workspace": "x"}),
    );
    assert_eq!(not_ours["isError"], json!(true));

    // A call waiting on the seat fails when it closes, rather than wait out its time.
    mcp.send(&json!({"jsonrpc": "2.0", "id": 900, "method": "tools/call",
                     "params": modern(json!({"name": "windows", "arguments":
                         {"seat": a, "app_id": "never", "wait_ms": 40_000}}))}));
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    mcp.send(&json!({"jsonrpc": "2.0", "id": 901, "method": "tools/call",
                     "params": modern(json!({"name": "seat", "arguments":
                         {"action": "close", "seat": a}}))}));
    let mut replies = Vec::new();
    let until = Instant::now() + Duration::from_secs(5);
    while replies.len() < 2 && Instant::now() < until {
        replies.extend(mcp.replies());
    }
    let reply = |id: u64| {
        replies
            .iter()
            .find(|reply| reply["id"] == json!(id))
            .map(|reply| reply["result"].clone())
            .expect("a reply")
    };
    let closed = reply(901);
    assert_eq!(reply(900)["isError"], json!(true), "{replies:?}");
    assert_eq!(
        closed["structuredContent"],
        json!({"closed": a, "ended": [], "left_running": []})
    );
    let stale = mcp.call("keyboard", json!({"action": "tap", "seat": a, "keys": "a"}));
    assert_eq!(stale["isError"], json!(true));
    let text = stale["content"][0]["text"].as_str().expect("a reason");
    assert!(text.contains("no seat 2"), "{text}");
    assert_eq!(mcp.server.compositor().windows(), Vec::new());
}

/// `outputs configure` gives an output another mode, each value left out its own, and answers
/// with the output as `list` gives it; the seat's output on its right moves with its right
/// edge. A rate alone keeps its VRR range, which a rate too low for it refuses. A mode refused
/// (scale 0, a rate past 1,000 Hz, a range too narrow, an unknown output, a call without its
/// output) is a tool error, and the outputs stay as they were.
#[test]
fn outputs_are_configured_whole_or_not_at_all() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let created = mcp.call(
        "seat",
        json!({"action": "create", "width": 1920, "height": 1080, "rate": 60, "scale": 1}),
    );
    let seat = created["structuredContent"]["seat"]
        .as_u64()
        .expect("a handle");
    let side = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "width": 640, "height": 480, "rate": 60,
               "scale": 1}),
    );
    assert_eq!(
        side["structuredContent"]["output"]["logical"]["x"],
        json!(1920.0)
    );
    let configure = |mcp: &mut Mcp, values: Value| {
        let mut arguments = json!({"action": "configure", "seat": seat, "output": 1});
        for (key, value) in values.as_object().expect("an object") {
            arguments[key] = value.clone();
        }
        mcp.call("outputs", arguments)
    };
    let side_x = |mcp: &mut Mcp| {
        let listed = mcp.call("outputs", json!({"action": "list", "seat": seat}));
        listed["structuredContent"]["outputs"][1]["logical"]["x"].clone()
    };

    let larger = configure(&mut mcp, json!({"width": 2560, "height": 1440}));
    assert_eq!(larger["isError"], json!(false), "{larger}");
    let output = &larger["structuredContent"]["output"];
    let listed = mcp.call("outputs", json!({"action": "list", "seat": seat}));
    assert_eq!(output, &listed["structuredContent"]["outputs"][0]);
    assert_eq!(
        (
            &output["width"],
            &output["height"],
            &output["refresh_mhz"],
            &output["scale"]
        ),
        (&json!(2560), &json!(1440), &json!(60_000), &json!(1.0))
    );
    assert_eq!(
        output["logical"],
        json!({"x": 0.0, "y": 0.0, "width": 2560.0, "height": 1440.0})
    );
    assert_eq!(side_x(&mut mcp), json!(2560.0));

    let scaled = configure(&mut mcp, json!({"scale": 2}));
    let output = &scaled["structuredContent"]["output"];
    assert_eq!(output["width"], json!(2560));
    assert_eq!(output["logical"]["width"], json!(1280.0));
    assert_eq!(side_x(&mut mcp), json!(1280.0));

    let vrr = configure(&mut mcp, json!({"rate": 120, "vrr_min": 48, "hdr": true}));
    let output = &vrr["structuredContent"]["output"];
    assert_eq!(output["vrr"], json!({"min": 48.0, "max": 120.0}), "{vrr}");
    assert_eq!(output["hdr"], json!("PG32UCDM"));
    let faster = configure(&mut mcp, json!({"rate": 144}));
    let output = &faster["structuredContent"]["output"];
    assert_eq!(
        output["vrr"],
        json!({"min": 48.0, "max": 144.0}),
        "{faster}"
    );
    assert_eq!(output["hdr"], json!("PG32UCDM"));

    let before = mcp.call("outputs", json!({"action": "list", "seat": seat}));
    for (values, reason) in [
        (json!({"scale": 0}), "scale of 0"),
        (json!({"rate": 5000}), "5000 Hz"),
        (json!({"rate": 60}), "VRR range of 48 to 60 Hz"),
        (json!({"vrr_min": 100}), "VRR range of 100 to 144 Hz"),
        (json!({"output": 9}), "no output 9"),
    ] {
        let refused = configure(&mut mcp, values.clone());
        assert_eq!(refused["isError"], json!(true), "{values}: {refused}");
        let text = refused["content"][0]["text"].as_str().expect("a reason");
        assert!(text.contains(reason), "{values}: {text}");
    }
    let without_output = mcp.call(
        "outputs",
        json!({"action": "configure", "seat": seat, "scale": 1}),
    );
    assert_eq!(without_output["isError"], json!(true));
    let after = mcp.call("outputs", json!({"action": "list", "seat": seat}));
    assert_eq!(after["structuredContent"], before["structuredContent"]);
}

/// An output configured while a `windows` call waits leaves the call waiting, unanswered until
/// its window comes; it is then answered with the window where the new mode places it: at
/// scale 2, half of 1920 units less the gaps, 936 wide.
#[test]
fn a_waiting_windows_call_outlives_its_outputs_new_mode() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    mcp.send(&json!({"jsonrpc": "2.0", "id": 400, "method": "tools/call",
                      "params": modern(json!({"name": "windows", "arguments": {
                          "seat": seat, "app_id": "org.example.Late", "wait_ms": 10_000}}))}));
    assert_eq!(mcp.replies(), Vec::<Value>::new());

    let configured = mcp.call(
        "outputs",
        json!({"action": "configure", "seat": seat, "output": 1, "scale": 2}),
    );
    assert_eq!(configured["isError"], json!(false), "{configured}");
    assert_eq!(
        configured["structuredContent"]["output"]["logical"]["width"],
        json!(1920.0)
    );
    crate::run_armed_deadlines(mcp.server.compositor());
    assert_eq!(mcp.replies(), Vec::<Value>::new());

    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = client.create_window();
    client.set_app_id_and_title(window, "org.example.Late", "Late");
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    let asked = client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .clone();
    assert_eq!((asked.width, asked.height), (936, 1048));
    client.ack(window);
    client.attach(window, asked.width, asked.height, RED);
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    crate::run_armed_deadlines(mcp.server.compositor());
    let found = mcp.reply_to(400);
    let windows = &found["result"]["structuredContent"]["windows"];
    assert_eq!(windows[0]["app_id"], json!("org.example.Late"));
    assert_eq!(
        windows[0]["rect"],
        json!({"x": 16.0, "y": 16.0, "width": 936.0, "height": 1048.0})
    );
}

/// `outputs configure` with `vrr_min: 0` runs a VRR output at a fixed rate, which then takes a
/// rate below twice its old range's least; `create` refuses 0.
#[test]
fn vrr_min_0_turns_vrr_off_in_configure_alone() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let created = mcp.call(
        "seat",
        json!({"action": "create", "width": 1920, "height": 1080, "rate": 144, "scale": 1,
               "vrr_min": 48}),
    );
    let seat = created["structuredContent"]["seat"]
        .as_u64()
        .expect("a handle");
    let low = mcp.call(
        "outputs",
        json!({"action": "configure", "seat": seat, "output": 1, "rate": 60}),
    );
    assert_eq!(low["isError"], json!(true), "{low}");

    let off = mcp.call(
        "outputs",
        json!({"action": "configure", "seat": seat, "output": 1, "vrr_min": 0}),
    );
    let output = &off["structuredContent"]["output"];
    assert_eq!(output["vrr"], Value::Null, "{off}");
    assert_eq!(output["refresh_mhz"], json!(144_000));
    let low = mcp.call(
        "outputs",
        json!({"action": "configure", "seat": seat, "output": 1, "rate": 60}),
    );
    let output = &low["structuredContent"]["output"];
    assert_eq!(output["refresh_mhz"], json!(60_000), "{low}");
    assert_eq!(output["vrr"], Value::Null);
    let on = mcp.call(
        "outputs",
        json!({"action": "configure", "seat": seat, "output": 1, "vrr_min": 24}),
    );
    assert_eq!(
        on["structuredContent"]["output"]["vrr"],
        json!({"min": 24.0, "max": 60.0}),
        "{on}"
    );

    let refused = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "vrr_min": 0}),
    );
    assert_eq!(refused["isError"], json!(true), "{refused}");
}

/// A `windows` call waiting for a window that is mapped but not yet placed is answered at the
/// arrangement that places it, after its output's change: with the place the new mode gives
/// it, at scale 2 half of 1920 units less the gaps, though the window opened at scale 1.5.
#[test]
fn a_waiting_windows_call_on_a_mapped_window_gets_its_place_in_the_new_mode() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = client.create_window();
    client.set_app_id_and_title(window, "org.example.Mapped", "Mapped");
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    client.ack(window);
    client.attach(window, crate::TILE.0, crate::TILE.1, RED);
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    // Mapped; placed only at the next refresh, which the manual clock has not reached.
    mcp.send(&json!({"jsonrpc": "2.0", "id": 410, "method": "tools/call",
                      "params": modern(json!({"name": "windows", "arguments": {
                          "seat": seat, "app_id": "org.example.Mapped", "wait_ms": 10_000}}))}));
    assert_eq!(mcp.replies(), Vec::<Value>::new());

    let configured = mcp.call(
        "outputs",
        json!({"action": "configure", "seat": seat, "output": 1, "scale": 2}),
    );
    assert_eq!(configured["isError"], json!(false), "{configured}");
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    crate::run_armed_deadlines(mcp.server.compositor());
    let found = mcp.reply_to(410);
    let windows = &found["result"]["structuredContent"]["windows"];
    assert_eq!(windows[0]["app_id"], json!("org.example.Mapped"));
    assert_eq!(
        windows[0]["place"],
        json!({"x": 16.0, "y": 16.0, "width": 936.0, "height": 1048.0})
    );
}

/// Maps a window of `client`, `app_id` and `title`, on its seat's focused output, and runs the
/// frames armed, so that the layout has placed it; returns its index in `client`.
fn open_placed_window(mcp: &mut Mcp, client: &mut Client, app_id: &str, title: &str) -> usize {
    let window = client.create_window();
    client.set_app_id_and_title(window, app_id, title);
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    client.ack(window);
    client.attach(window, crate::TILE.0, crate::TILE.1, RED);
    client.commit(window);
    client.roundtrip(mcp.server.compositor());
    crate::run_armed_deadlines(mcp.server.compositor());
    window
}

/// `events`' answer to `arguments`, which must not fail.
fn events(mcp: &mut Mcp, arguments: Value) -> Value {
    let result = mcp.call("events", arguments);
    assert_eq!(result["isError"], json!(false), "{result}");
    result["structuredContent"].clone()
}

/// The kinds of an answer's events, in order.
fn kinds(answer: &Value) -> Vec<&str> {
    answer["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|event| event["kind"].as_str())
        .collect()
}

/// The seqs of an answer's events, in order.
fn seqs(answer: &Value) -> Vec<u64> {
    answer["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|event| event["seq"].as_u64())
        .collect()
}

/// Every reply that comes until the one to request `id`, that one included, within 5 s of real
/// time.
fn replies_until(mcp: &mut Mcp, id: u64) -> Vec<Value> {
    let until = Instant::now() + Duration::from_secs(5);
    let mut replies = Vec::new();
    while !replies.iter().any(|reply: &Value| reply["id"] == json!(id)) {
        assert!(
            Instant::now() < until,
            "no reply to request {id}: {replies:?}"
        );
        replies.extend(mcp.replies());
        std::thread::sleep(Duration::from_millis(1));
    }
    replies
}

/// The newest event's number, as an `events` call without `after` gives it.
fn cursor(mcp: &mut Mcp, seat: u64) -> u64 {
    events(mcp, json!({"seat": seat}))["next"]
        .as_u64()
        .expect("a cursor")
}

/// A window that opens, then closes, is told `window_opened`, placed and focused, then
/// `window_closed` and the focus gone, in order, each event's seq above the last.
#[test]
fn events_tell_a_window_opened_then_closed_in_order() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let start = cursor(&mut mcp, seat);
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = open_placed_window(&mut mcp, &mut client, "org.example.Events", "One");
    let opened = events(&mut mcp, json!({"seat": seat, "after": start}));
    assert_eq!(
        kinds(&opened),
        ["window_opened", "window_placed", "focus"],
        "{opened}"
    );
    let told = &opened["events"][0];
    assert_eq!(told["window"], json!(1));
    assert_eq!(told["app_id"], json!("org.example.Events"));
    assert_eq!(told["title"], json!("One"));
    assert_eq!(told["shell"], json!("xdg"));
    assert_eq!(told["pid"], json!(std::process::id()));
    assert!(told["time_ns"].is_u64());
    let placed = &opened["events"][1];
    assert_eq!(placed["output"], json!(1));
    assert_eq!(
        placed["place"],
        json!({"x": 16.0, "y": 16.0, "width": 1256.0, "height": 1408.0})
    );
    assert_eq!(opened["events"][2]["window"], json!(1));
    assert_eq!(opened["missed"], json!(0));

    client.close(window);
    client.roundtrip(mcp.server.compositor());
    let closed = events(
        &mut mcp,
        json!({"seat": seat, "after": opened["next"].clone()}),
    );
    assert_eq!(kinds(&closed), ["window_closed", "focus"], "{closed}");
    assert_eq!(closed["events"][0]["window"], json!(1));
    assert_eq!(closed["events"][1]["window"], Value::Null);
    let all = events(&mut mcp, json!({"seat": seat, "after": start}));
    let numbers = seqs(&all);
    assert_eq!(numbers.len(), 5, "{all}");
    assert!(numbers.windows(2).all(|pair| pair[0] < pair[1]), "{all}");
    assert_eq!(all["next"], json!(numbers[4]));
}

/// Without `after`, `events` gives no past event: it waits from now, and is answered once a
/// window opens, with that window's events alone. `kinds` keeps only the kinds it names; a kind
/// it does not know is refused.
#[test]
fn events_wait_from_now_and_kinds_filter_them() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let mut first = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    open_placed_window(&mut mcp, &mut first, "org.example.First", "First");
    // An event of the past, which no call without `after` gives.
    let now = events(&mut mcp, json!({"seat": seat}));
    assert_eq!(now["events"], json!([]));
    assert!(now["next"].as_u64() > Some(0), "{now}");
    mcp.send(&json!({"jsonrpc": "2.0", "id": 700, "method": "tools/call",
                      "params": modern(json!({"name": "events", "arguments":
                          {"seat": seat, "wait_ms": 10_000}}))}));
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(mcp.replies(), Vec::<Value>::new());

    let mut second = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    open_placed_window(&mut mcp, &mut second, "org.example.Second", "Second");
    let waited = mcp.reply_to(700)["result"]["structuredContent"].clone();
    assert_eq!(kinds(&waited)[0], "window_opened", "{waited}");
    assert_eq!(waited["events"][0]["app_id"], json!("org.example.Second"));
    assert!(
        waited["events"]
            .as_array()
            .expect("a list")
            .iter()
            .all(|event| event["app_id"] != json!("org.example.First")),
        "{waited}"
    );
    assert_eq!(waited.get("timed_out"), None);

    let opened = events(
        &mut mcp,
        json!({"seat": seat, "after": 0, "kinds": ["window_opened"]}),
    );
    assert_eq!(
        kinds(&opened),
        ["window_opened", "window_opened"],
        "{opened}"
    );
    let focus = events(
        &mut mcp,
        json!({"seat": seat, "after": 0, "kinds": ["focus", "window_closed"]}),
    );
    assert_eq!(kinds(&focus), ["focus", "focus"], "{focus}");
    for wrong in [json!(["teleported"]), json!([]), json!("focus")] {
        let refused = mcp.call("events", json!({"seat": seat, "kinds": wrong}));
        assert_eq!(refused["isError"], json!(true), "{wrong}");
    }
    let ahead = mcp.call("events", json!({"seat": seat, "after": 1_000_000}));
    assert_eq!(ahead["isError"], json!(true));
    let too_long = mcp.call("events", json!({"seat": seat, "wait_ms": 60_000}));
    assert_eq!(too_long["isError"], json!(true));

    // A wait whose time is up says so.
    let quiet = events(
        &mut mcp,
        json!({"seat": seat, "kinds": ["client_left"], "wait_ms": 30}),
    );
    assert_eq!(quiet["events"], json!([]));
    assert_eq!(quiet["timed_out"], json!(true));
}

/// A title changed 1,500 times between two looks is one `window_changed`, with the last title.
#[test]
fn a_title_changed_1500_times_in_a_round_is_one_event() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = open_placed_window(&mut mcp, &mut client, "org.example.Titles", "title");
    let start = cursor(&mut mcp, seat);
    for n in 0..1_500 {
        client.set_app_id_and_title(window, "org.example.Titles", &format!("title {n}"));
    }
    client.roundtrip(mcp.server.compositor());
    let changed = events(
        &mut mcp,
        json!({"seat": seat, "after": start, "kinds": ["window_changed"]}),
    );
    assert_eq!(
        changed["events"].as_array().map(|events| events.len()),
        Some(1),
        "{changed}"
    );
    assert_eq!(changed["events"][0]["title"], json!("title 1499"));
    assert_eq!(changed["events"][0]["app_id"], json!("org.example.Titles"));
}

/// A seat's log keeps its last 1,024 events: a cursor older than them learns how many it
/// missed, and reads on, 256 a reply, from the oldest kept; a newer cursor missed none.
#[test]
fn a_cursor_older_than_the_log_is_told_what_it_missed() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    // Each output added, then removed: 2,000 events, besides the seat's first output.
    for _ in 0..1_000 {
        let created = mcp.call(
            "outputs",
            json!({"action": "create", "seat": seat, "width": 64, "height": 64, "rate": 60,
                   "scale": 1}),
        );
        let output = created["structuredContent"]["output"]["id"].clone();
        let removed = mcp.call(
            "outputs",
            json!({"action": "remove", "seat": seat, "output": output}),
        );
        assert_eq!(removed["isError"], json!(false), "{removed}");
    }
    let newest = cursor(&mut mcp, seat);
    assert!(newest >= 2_001, "{newest}");
    let old = events(&mut mcp, json!({"seat": seat, "after": 0}));
    let kept = yuna::mcp::MAX_EVENTS as u64;
    assert_eq!(old["missed"], json!(newest - kept), "{}", old["missed"]);
    let numbers = seqs(&old);
    assert_eq!(numbers.len(), yuna::mcp::MAX_EVENTS_PER_REPLY);
    assert_eq!(numbers[0], newest - kept + 1);
    assert_eq!(old["next"], json!(numbers[numbers.len() - 1]));
    let on = events(
        &mut mcp,
        json!({"seat": seat, "after": old["next"].clone()}),
    );
    assert_eq!(on["missed"], json!(0));
    assert_eq!(seqs(&on)[0], numbers[numbers.len() - 1] + 1);
    assert_eq!(kinds(&on)[0..2], ["output_added", "output_removed"]);
}

/// A waiting `events` call fails when its seat closes, rather than wait out its time; one that
/// `notifications/cancelled` names is dropped unanswered, even once an event comes.
#[test]
fn a_waiting_events_call_fails_with_its_seat_and_is_dropped_when_cancelled() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    for id in [800, 801] {
        mcp.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                          "params": modern(json!({"name": "events", "arguments":
                              {"seat": seat, "wait_ms": 10_000}}))}));
    }
    mcp.send(
        &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                      "params": {"requestId": 801, "reason": "the test"}}),
    );
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    open_placed_window(&mut mcp, &mut client, "org.example.Waited", "Waited");
    // Every reply up to 800's, and those after: a 801 answered with it would be among them.
    let mut replies = replies_until(&mut mcp, 800);
    std::thread::sleep(Duration::from_millis(20));
    replies.extend(mcp.replies());
    let answered = replies
        .iter()
        .find(|reply| reply["id"] == json!(800))
        .expect("800's reply");
    assert_eq!(
        kinds(&answered["result"]["structuredContent"])[0],
        "window_opened"
    );
    assert!(
        replies.iter().all(|reply| reply["id"] != json!(801)),
        "{replies:?}"
    );

    mcp.send(&json!({"jsonrpc": "2.0", "id": 802, "method": "tools/call",
                      "params": modern(json!({"name": "events", "arguments":
                          {"seat": seat, "kinds": ["output_added"], "wait_ms": 40_000}}))}));
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    mcp.send(&json!({"jsonrpc": "2.0", "id": 803, "method": "tools/call",
                      "params": modern(json!({"name": "seat", "arguments":
                          {"action": "close", "seat": seat}}))}));
    let mut replies = Vec::new();
    let until = Instant::now() + Duration::from_secs(5);
    while replies.len() < 2 && Instant::now() < until {
        replies.extend(mcp.replies());
    }
    let failed = replies
        .iter()
        .find(|reply| reply["id"] == json!(802))
        .unwrap_or_else(|| panic!("{replies:?}"));
    assert_eq!(failed["result"]["isError"], json!(true));
    let text = failed["result"]["content"][0]["text"]
        .as_str()
        .expect("a reason");
    assert!(text.contains("was closed"), "{text}");
    let stale = mcp.call("events", json!({"seat": seat}));
    assert_eq!(stale["isError"], json!(true));
}

/// An output removed is told `output_removed`, and each window it held `window_placed`: moved
/// to the seat's other output, on a workspace that output does not show (ADR-0018), so hidden,
/// with no output and no place.
#[test]
fn an_output_removed_is_told_with_each_window_it_moved() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let created = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "width": 1280, "height": 720, "rate": 60,
               "scale": 1}),
    );
    let other = created["structuredContent"]["output"]["id"].clone();
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    open_placed_window(&mut mcp, &mut client, "org.example.A", "A");
    open_placed_window(&mut mcp, &mut client, "org.example.B", "B");
    let listed = mcp.call("windows", json!({"seat": seat}));
    for window in listed["structuredContent"]["windows"]
        .as_array()
        .expect("a list")
    {
        assert_eq!(window["output"], json!(1), "{listed}");
    }
    let start = cursor(&mut mcp, seat);
    let removed = mcp.call(
        "outputs",
        json!({"action": "remove", "seat": seat, "output": 1}),
    );
    assert_eq!(removed["isError"], json!(false), "{removed}");
    crate::run_armed_deadlines(mcp.server.compositor());
    let told = events(&mut mcp, json!({"seat": seat, "after": start}));
    let list = told["events"].as_array().expect("a list");
    assert_eq!(list[0]["kind"], json!("output_removed"), "{told}");
    assert_eq!(list[0]["output"], json!(1));
    let mut moved: Vec<&Value> = list
        .iter()
        .filter(|event| event["kind"] == json!("window_placed"))
        .map(|event| &event["window"])
        .collect();
    moved.sort_by_key(|window| window.as_u64());
    assert_eq!(moved, [&json!(1), &json!(2)], "{told}");
    assert!(
        list.iter()
            .filter(|event| event["kind"] == json!("window_placed"))
            .all(|event| event["output"] == Value::Null && event["place"] == Value::Null),
        "{told}"
    );
    let workspaces = mcp.call("workspaces", json!({"action": "list", "seat": seat}));
    let outputs = &workspaces["structuredContent"]["outputs"];
    assert_eq!(outputs[0]["output"], other, "{workspaces}");
    let held: Vec<u64> = outputs[0]["workspaces"]
        .as_array()
        .expect("a list")
        .iter()
        .flat_map(|workspace| workspace["windows"].as_array().cloned().unwrap_or_default())
        .filter_map(|window| window.as_u64())
        .collect();
    assert!(held.contains(&1) && held.contains(&2), "{workspaces}");
}

/// A client ended for a protocol error, a window's surface given a popup's role besides its
/// toplevel's, is told `client_left` with the error Yuna sent it: its interface, object, code
/// and message. A client of the test's own process has the test's pid, as its window had, and
/// belongs to no program.
#[test]
fn a_protocol_error_is_told_with_its_interface_code_and_message() {
    use crate::client::Parent;
    use wayland_protocols::xdg::shell::client::xdg_wm_base;

    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = open_placed_window(&mut mcp, &mut client, "org.example.Broken", "Broken");
    let start = cursor(&mut mcp, seat);
    client.create_popup_on_window_surface(window, Parent::Window(window));
    let error = client.protocol_error(mcp.server.compositor());
    assert_eq!(error.interface, "xdg_wm_base");
    drop(client);
    let told = events(
        &mut mcp,
        json!({"seat": seat, "after": start, "kinds": ["client_left", "window_closed"]}),
    );
    assert_eq!(kinds(&told), ["window_closed", "client_left"], "{told}");
    let left = &told["events"][1];
    assert_eq!(left["reason"], json!("protocol_error"));
    assert_eq!(left["pid"], json!(std::process::id()));
    assert_eq!(left["program"], Value::Null);
    let sent = &left["error"];
    assert_eq!(sent["interface"], json!("xdg_wm_base"), "{left}");
    assert_eq!(sent["code"], json!(xdg_wm_base::Error::Role as u32));
    assert!(sent["object"].as_u64() > Some(1), "{left}");
    assert!(
        sent["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "{left}"
    );
}

/// A client that stops reading, as a frozen program does, while its seat types to it, fills its
/// connection's buffers, 1 MiB of them Yuna's (ADR-0136), a few calls of `type` of
/// [`yuna::MAX_TYPED`] characters: the display closes it, and `events` tells `client_left` with
/// reason `closed` and no error.
#[test]
fn a_client_that_reads_nothing_while_it_is_typed_to_is_told_closed() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let name = format!("agent-{seat}");
    let mut client =
        crate::input_client::InputClient::connect(mcp.server.compositor(), SeatId(seat), &name);
    let window = client.client.create_window();
    client.client.commit(window);
    client.roundtrip(mcp.server.compositor());
    client.client.ack(window);
    client.client.attach(window, 400, 300, RED);
    client.client.commit(window);
    client.roundtrip(mcp.server.compositor());
    crate::run_armed_deadlines(mcp.server.compositor());
    mcp.call(
        "pointer",
        json!({"action": "click", "seat": seat, "x": 26, "y": 26}),
    );
    crate::run_armed_deadlines(mcp.server.compositor());
    client.roundtrip(mcp.server.compositor());
    let start = cursor(&mut mcp, seat);
    // From now on the client reads nothing: each key typed waits in its socket.
    let text = "a".repeat(yuna::MAX_TYPED);
    let mut calls = 0;
    let left = loop {
        assert!(calls < 50, "the client was never closed");
        let typed = mcp.call(
            "keyboard",
            json!({"action": "type", "seat": seat, "text": text}),
        );
        assert_eq!(typed["isError"], json!(false), "{typed}");
        calls += 1;
        let told = events(
            &mut mcp,
            json!({"seat": seat, "after": start, "kinds": ["client_left"]}),
        );
        if let Some(left) = told["events"].as_array().and_then(|events| events.first()) {
            break left.clone();
        }
    };
    eprintln!(
        "the client was closed after {calls} calls of {} keys",
        yuna::MAX_TYPED
    );
    assert_eq!(left["reason"], json!("closed"), "{left}");
    assert_eq!(left["error"], Value::Null);
}

/// A seat that holds its bound of clients refuses the connections its socket takes, and tells
/// its agent `client_refused`, with the bound and how many it refused since its last such event,
/// at most once a second: a burst of refusals within a second is told once, by its first; the
/// first refusal of a burst a second later tells the rest of the first and itself. The socket is
/// the compositor's own (`listen_for_seat`), which needs no reaping of children, unlike the one
/// `seat create` makes in `yuna mcp`. It runs in a process of its own, as the other stories of
/// 257 clients do (`client_bound.rs`).
#[test]
fn refused_connections_are_told_once_a_second_at_most() {
    use yuna::MAX_CLIENTS_PER_SEAT;

    if !crate::in_its_own_process(
        "mcp::refused_connections_are_told_once_a_second_at_most",
        &[],
    ) {
        return;
    }
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let folder = crate::x11_windows::Folder::new("client-bound-mcp");
    let path = folder.0.join("seat");
    let compositor = mcp.server.compositor();
    compositor
        .listen_for_seat(SeatId(seat), path.clone())
        .expect("the seat listens");
    let _clients: Vec<UnixStream> = (0..MAX_CLIENTS_PER_SEAT)
        .map(|_| {
            compositor
                .connect_client_to(SeatId(seat))
                .expect("a client within the bound connects")
        })
        .collect();
    let start = cursor(&mut mcp, seat);
    let refuse = |mcp: &mut Mcp| {
        let mut refused = UnixStream::connect(&path).expect("the socket takes the connection");
        mcp.replies();
        let mut byte = [0_u8; 1];
        refused
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a timeout");
        assert_eq!(refused.read(&mut byte).ok(), Some(0), "closed at once");
    };
    let refused_told = |mcp: &mut Mcp| {
        events(
            mcp,
            json!({"seat": seat, "after": start, "kinds": ["client_refused"]}),
        )
    };

    let first_burst = Instant::now();
    refuse(&mut mcp);
    // Yuna told the first refusal before this instant: a second after it, the next is told.
    let first_told = Instant::now();
    for _ in 0..4 {
        refuse(&mut mcp);
    }
    assert!(
        first_burst.elapsed() < Duration::from_secs(1),
        "the first burst took {:?}: a second passed within it",
        first_burst.elapsed()
    );
    let told = refused_told(&mut mcp);
    assert_eq!(kinds(&told), ["client_refused"], "{told}");
    let event = &told["events"][0];
    assert_eq!(event["bound"], json!(MAX_CLIENTS_PER_SEAT), "{event}");
    assert_eq!(event["refused"], json!(1), "{event}");

    std::thread::sleep(Duration::from_secs(1).saturating_sub(first_told.elapsed()));
    for _ in 0..3 {
        refuse(&mut mcp);
    }
    let told = refused_told(&mut mcp);
    assert_eq!(kinds(&told), ["client_refused", "client_refused"], "{told}");
    assert_eq!(told["events"][1]["refused"], json!(4 + 1), "{told}");
}

/// The windows and outputs are described for events only when something events tell may have
/// changed: a client drawing frame after frame, as a game does, while its title, place and focus
/// stay, makes no look; a title it changes makes one.
#[test]
fn a_client_that_only_draws_makes_no_look() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    let window = open_placed_window(&mut mcp, &mut client, "org.example.Game", "Game");
    cursor(&mut mcp, seat);
    let looks = mcp.server.event_looks();
    for colour in [RED, crate::BLUE, RED, crate::BLUE] {
        client.attach(window, crate::TILE.0, crate::TILE.1, colour);
        client.damage(window, crate::TILE.0, crate::TILE.1);
        client.commit(window);
        client.roundtrip(mcp.server.compositor());
        crate::run_armed_deadlines(mcp.server.compositor());
        assert_eq!(mcp.replies(), Vec::<Value>::new());
    }
    assert_eq!(mcp.server.event_looks(), looks, "a frame made a look");
    client.set_app_id_and_title(window, "org.example.Game", "Game, paused");
    client.roundtrip(mcp.server.compositor());
    mcp.replies();
    assert_eq!(mcp.server.event_looks(), looks + 1);
}

/// A waiting call whose cursor is older than its seat's log is answered at once, with what it
/// missed, rather than wait for an event that may have been dropped.
#[test]
fn a_wait_whose_cursor_missed_events_is_answered_at_once() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let old = cursor(&mut mcp, seat);
    for _ in 0..(yuna::mcp::MAX_EVENTS / 2 + 1) {
        let created = mcp.call(
            "outputs",
            json!({"action": "create", "seat": seat, "width": 64, "height": 64, "rate": 60,
                   "scale": 1}),
        );
        let output = created["structuredContent"]["output"]["id"].clone();
        mcp.call(
            "outputs",
            json!({"action": "remove", "seat": seat, "output": output}),
        );
    }
    mcp.send(&json!({"jsonrpc": "2.0", "id": 900, "method": "tools/call",
                      "params": modern(json!({"name": "events", "arguments":
                          {"seat": seat, "after": old, "kinds": ["client_left"],
                           "wait_ms": 40_000}}))}));
    let answered = mcp.replies();
    assert_eq!(answered.len(), 1, "it waits");
    let answer = &answered[0]["result"]["structuredContent"];
    assert_eq!(answer["events"], json!([]), "{answer}");
    assert!(answer["missed"].as_u64() > Some(0), "{answer}");
    assert_eq!(answer.get("timed_out"), None);
}

/// `windows`' and `events`' waiting calls count together against `MAX_WAITING_CALLS`: once
/// that many wait, of either, the next of either is refused.
#[test]
fn windows_and_events_waits_share_one_bound() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    for id in 0..MAX_WAITING_CALLS {
        let (name, arguments) = if id % 2 == 0 {
            ("windows", json!({"seat": seat, "wait_ms": 10_000}))
        } else {
            ("events", json!({"seat": seat, "wait_ms": 10_000}))
        };
        mcp.send(
            &json!({"jsonrpc": "2.0", "id": id + 1_000, "method": "tools/call",
                          "params": modern(json!({"name": name, "arguments": arguments}))}),
        );
    }
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    let windows = mcp.call("windows", json!({"seat": seat, "wait_ms": 10_000}));
    assert_eq!(windows["isError"], json!(true), "{windows}");
    let events = mcp.call("events", json!({"seat": seat, "wait_ms": 10_000}));
    assert_eq!(events["isError"], json!(true), "{events}");
}

/// A workspace's name is bounded where `windows` repeats it (Stage 1's audit, m1.13-audit-mcp-5):
/// each window's entry of `windows` gives its workspace's name whole, in its structured content
/// and its text, so that a name bounded by a request's line alone, 600 KiB here, made the reply
/// to one call grow with the name times the windows, past the replies that may wait,
/// `MAX_QUEUED_OUTPUT`, with 30 windows already. Such a name is refused, plainly; a name at the
/// bound, repeated by 30 windows, keeps the reply under.
#[test]
fn a_workspaces_name_is_bounded_where_windows_repeats_it() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let long = "w".repeat(600 << 10);
    let id = 900;
    let sending = mcp.send_on_thread(&json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": modern(json!({"name": "workspaces",
            "arguments": {"action": "focus", "seat": seat, "name": long}}))
    }));
    let focused = mcp.reply_to(id);
    sending.join().expect("the request is sent");
    assert_eq!(
        focused["result"]["isError"],
        json!(true),
        "the long name is taken"
    );
    let at_bound = "w".repeat(MAX_WORKSPACE_NAME);
    let shown = mcp.call(
        "workspaces",
        json!({"action": "focus", "seat": seat, "name": at_bound}),
    );
    assert_eq!(shown["isError"], json!(false), "{shown}");
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    for n in 0..30 {
        open_placed_window(&mut mcp, &mut client, "org.example.Long", &n.to_string());
    }
    // The reply read here as bytes up to its newline: `Mcp::replies` scans all it holds at
    // each read, which takes long over tens of megabytes in a debug build.
    assert!(mcp.replies().is_empty());
    mcp.send(&json!({
        "jsonrpc": "2.0", "id": 901, "method": "tools/call",
        "params": modern(json!({"name": "windows", "arguments": {"seat": seat}}))
    }));
    let until = Instant::now() + Duration::from_secs(120);
    let mut bytes = 0;
    let mut buffer = vec![0; 1 << 20];
    'reading: loop {
        assert!(
            Instant::now() < until,
            "no whole reply in 120 s: {bytes} bytes"
        );
        mcp.server
            .dispatch(Some(Duration::ZERO))
            .expect("the server dispatches");
        loop {
            match mcp.replies.read(&mut buffer) {
                Ok(0) => break 'reading,
                Ok(read) => {
                    bytes += read;
                    if buffer[..read].contains(&b'\n') {
                        break 'reading;
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => panic!("the reply cannot be read: {error}"),
            }
        }
    }
    eprintln!("`windows`' reply for 30 windows takes {bytes} bytes");
    assert!(
        bytes <= MAX_QUEUED_OUTPUT,
        "one reply of {bytes} bytes, past the {MAX_QUEUED_OUTPUT} that may wait"
    );
}

/// A workspace's name is `MAX_WORKSPACE_NAME` bytes at most wherever an agent names one:
/// `create`, `remove`, `focus` and `move_window` refuse a byte more plainly, and take a name at
/// the bound.
#[test]
fn a_workspaces_name_past_its_bound_is_refused_wherever_it_is_named() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    open_placed_window(&mut mcp, &mut client, "org.example.Named", "Named");
    let listed = mcp.call("windows", json!({"seat": seat}));
    let window = listed["structuredContent"]["windows"][0]["id"].clone();
    assert!(window.is_u64(), "{listed}");
    let past = "p".repeat(MAX_WORKSPACE_NAME + 1);
    // Within the schema's `maxLength`, which counts characters, but past the bound in bytes.
    let past_in_bytes = "é".repeat(MAX_WORKSPACE_NAME / 2 + 1);
    let at = "a".repeat(MAX_WORKSPACE_NAME);
    let calls = |name: &str| {
        [
            json!({"action": "create", "seat": seat, "name": name}),
            json!({"action": "focus", "seat": seat, "name": name}),
            json!({"action": "move_window", "seat": seat, "window": window, "workspace": name}),
            json!({"action": "remove", "seat": seat, "name": name}),
        ]
    };
    for arguments in calls(&past).into_iter().chain(calls(&past_in_bytes)) {
        let refused = mcp.call("workspaces", arguments.clone());
        assert_eq!(refused["isError"], json!(true), "{arguments}: {refused}");
        let text = refused["content"][0]["text"].as_str().unwrap_or_default();
        assert!(
            text.contains(&format!("{MAX_WORKSPACE_NAME} bytes")),
            "{arguments}: {refused}"
        );
    }
    for arguments in calls(&at) {
        let taken = mcp.call("workspaces", arguments.clone());
        assert_eq!(taken["isError"], json!(false), "{arguments}: {taken}");
    }
}

/// An output's side is `MAX_OUTPUT_SIDE` pixels at most, and Yuna's outputs together hold
/// `MAX_OUTPUT_PIXELS` at most, the pixels of 16 outputs at 3840 × 2160: a mode past either,
/// made by `seat create` or `outputs create` or given by `outputs configure`, is refused plainly
/// before anything is made or changed. A side at the bound is taken, and an output configured is
/// counted with its new mode alone. The pixels' bound itself is checked to the pixel by
/// `outputs.rs`'s tests, which allocate nothing: here, one output of 8,192 × 8,192 costs 256 MiB.
#[test]
fn an_output_past_its_side_or_the_pixels_of_16_4k_outputs_is_refused() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let side = MAX_OUTPUT_SIDE;
    let refused_for = |result: &Value, figure: u64| {
        assert_eq!(result["isError"], json!(true), "{result}");
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        assert!(text.contains(&figure.to_string()), "{result}");
    };
    let wide = mcp.call(
        "seat",
        json!({"action": "create", "width": side + 1, "height": 1}),
    );
    refused_for(&wide, u64::from(side));
    assert_eq!(mcp.server.compositor().seats().len(), 1, "a seat was made");
    // At the side's bound: a square of 8,192 pixels, 67,108,864 of them.
    let created = mcp.call(
        "seat",
        json!({"action": "create", "width": side, "height": side, "rate": 60, "scale": 1}),
    );
    assert_eq!(created["isError"], json!(false), "{created}");
    let seat = created["structuredContent"]["seat"]
        .as_u64()
        .expect("a seat");
    let first = created["structuredContent"]["outputs"][0]["id"]
        .as_u64()
        .expect("its output");
    let tall = mcp.call(
        "outputs",
        json!({"action": "configure", "seat": seat, "output": first, "height": side + 1}),
    );
    refused_for(&tall, u64::from(side));
    let tall_new = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "width": 1, "height": side + 1}),
    );
    refused_for(&tall_new, u64::from(side));
    // One row of 8,192 past what is left of the pixels' bound: 8,192 × 8,009.
    let left = MAX_OUTPUT_PIXELS - u64::from(side) * u64::from(side);
    let rows = (left / u64::from(side)) as u32;
    let past = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "width": side, "height": rows + 1}),
    );
    refused_for(&past, MAX_OUTPUT_PIXELS);
    let small = mcp.call(
        "outputs",
        json!({"action": "create", "seat": seat, "width": 1, "height": 1}),
    );
    assert_eq!(small["isError"], json!(false), "{small}");
    let second = small["structuredContent"]["output"]["id"]
        .as_u64()
        .expect("an id");
    let grown = mcp.call(
        "outputs",
        json!({"action": "configure", "seat": seat, "output": second, "width": side,
               "height": rows + 1}),
    );
    refused_for(&grown, MAX_OUTPUT_PIXELS);
    let listed = mcp.call("outputs", json!({"action": "list", "seat": seat}));
    let sizes: Vec<(Value, Value)> = listed["structuredContent"]["outputs"]
        .as_array()
        .expect("outputs")
        .iter()
        .map(|output| (output["width"].clone(), output["height"].clone()))
        .collect();
    assert_eq!(sizes, [(json!(side), json!(side)), (json!(1), json!(1))]);
    // Its rate changed, the large output is counted once, with its new mode: twice, it would
    // pass the bound.
    let retimed = mcp.call(
        "outputs",
        json!({"action": "configure", "seat": seat, "output": first, "rate": 120}),
    );
    assert_eq!(retimed["isError"], json!(false), "{retimed}");
}

/// The answers of calls that waited respect the replies that may wait: while
/// `MAX_QUEUED_OUTPUT` of replies wait for a client that does not read them, the 64 `windows`
/// calls a window answers are held, none written, and come once the client reads, with every
/// other reply.
#[test]
fn the_answers_of_waiting_calls_wait_while_16_mib_of_replies_wait() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let seat = mcp.seat();
    let waiting: Vec<u64> = (500..500 + MAX_WAITING_CALLS as u64).collect();
    for id in &waiting {
        mcp.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": modern(json!({"name": "windows", "arguments":
                {"seat": seat, "app_id": "org.example.Held", "wait_ms": 40_000}}))}));
    }
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    let request = json!({"jsonrpc": "2.0", "id": 0, "method": "tools/list",
                         "params": modern(json!({}))});
    let reply_length = {
        let reply = mcp.request("tools/list", modern(json!({})));
        reply.to_string().len() + 1
    };
    let count = MAX_QUEUED_OUTPUT / reply_length * 2;
    let mut writer = mcp.requests.try_clone().expect("a second handle");
    let lines: String = (0..count)
        .map(|index| {
            let mut request = request.clone();
            request["id"] = json!(index + 1_000);
            request.to_string() + "\n"
        })
        .collect();
    let writing = std::thread::spawn(move || writer.write_all(lines.as_bytes()));
    let until = Instant::now() + Duration::from_secs(30);
    while !mcp.server.input_paused() {
        assert!(Instant::now() < until, "the input was never paused");
        mcp.server
            .dispatch(Some(Duration::from_millis(1)))
            .expect("the server dispatches");
    }
    let queued = mcp.server.queued_output();
    let mut client = Client::connect_to(mcp.server.compositor(), SeatId(seat));
    open_placed_window(&mut mcp, &mut client, "org.example.Held", "Held");
    for _ in 0..10 {
        mcp.server
            .dispatch(Some(Duration::from_millis(1)))
            .expect("the server dispatches");
    }
    assert_eq!(
        mcp.server.queued_output(),
        queued,
        "the waiting calls' answers were queued past the bound"
    );

    let mut answered = Vec::new();
    let mut listed = 0;
    while listed < count || answered.len() < waiting.len() {
        assert!(
            Instant::now() < until,
            "{listed} lists of {count}, {} answers",
            answered.len()
        );
        for reply in mcp.replies() {
            let id = reply["id"].as_u64().expect("an id");
            if id >= 1_000 {
                listed += 1;
                continue;
            }
            let windows = &reply["result"]["structuredContent"]["windows"];
            assert_eq!(windows[0]["app_id"], json!("org.example.Held"), "{reply}");
            answered.push(id);
        }
    }
    answered.sort_unstable();
    assert_eq!(answered, waiting);
    writing
        .join()
        .expect("the writer")
        .expect("every request was written");
}
