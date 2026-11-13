//! The MCP server in the test's own process (ADR-0033): requests written to its input, its
//! replies read from its output, each through a socket pair of its own, and the compositor's
//! windows opened by test clients. `spawn` needs a process that reaps its children, so it is
//! tested on the program, in `tests/mcp.rs`.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yuna::mcp::{MAX_LINE, MAX_QUEUED_OUTPUT, MAX_WAITING_CALLS, MODERN_REVISION, Server, Stop};
use yuna::{ClockMode, Compositor, Config};

use crate::RED;
use crate::client::Client;

/// A test's end of a server: what it writes to, what it reads from.
struct Mcp {
    server: Server,
    requests: UnixStream,
    replies: UnixStream,
    read: Vec<u8>,
    next_id: u64,
}

impl Mcp {
    fn new(clock: ClockMode) -> Mcp {
        let compositor = Compositor::new(Config {
            clock,
            listen: false,
            ..Config::default()
        })
        .expect("the compositor starts");
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

    fn send(&mut self, message: &Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.requests
            .write_all(line.as_bytes())
            .expect("the request is sent");
    }

    /// Lets the server run a round, then reads the replies that came.
    fn replies(&mut self) -> Vec<Value> {
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
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.reply_to(id)
    }

    fn reply_to(&mut self, id: u64) -> Value {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if let Some(reply) = self.replies().into_iter().find(|r| r["id"] == json!(id)) {
                return reply;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("no reply to request {id}");
    }

    /// Calls a tool in 2026-07-28 and returns its result.
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let reply = self.request(
            "tools/call",
            modern(json!({"name": tool, "arguments": arguments})),
        );
        reply["result"].clone()
    }
}

/// `params` with 2026-07-28's `_meta`.
fn modern(mut params: Value) -> Value {
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

/// `outputs` creates, lists and removes outputs; bad modes and the last output are refused as
/// tool errors, which the model reads.
#[test]
fn outputs_are_created_listed_and_removed() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let created = mcp.call(
        "outputs",
        json!({"action": "create", "width": 1920, "height": 1080, "rate": 60, "scale": 1}),
    );
    assert_eq!(created["isError"], json!(false));
    let output = &created["structuredContent"]["output"];
    assert_eq!(output["id"], json!(2));
    assert_eq!(output["name"], json!("HEADLESS-2"));
    assert_eq!(output["refresh_mhz"], json!(60_000));
    assert_eq!(
        output["logical"],
        json!({"x": 2560.0, "y": 0.0, "width": 1920.0, "height": 1080.0})
    );
    let listed = mcp.call("outputs", json!({"action": "list"}));
    let ids: Vec<&Value> = listed["structuredContent"]["outputs"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|output| &output["id"])
        .collect();
    assert_eq!(ids, [&json!(1), &json!(2)]);

    let bad = mcp.call("outputs", json!({"action": "create", "rate": 5000}));
    assert_eq!(bad["isError"], json!(true));
    let unknown_field = mcp.call("outputs", json!({"action": "list", "verbose": true}));
    assert_eq!(unknown_field["isError"], json!(true));

    let removed = mcp.call("outputs", json!({"action": "remove", "output": 1}));
    assert_eq!(removed["structuredContent"], json!({"removed": 1}));
    let last = mcp.call("outputs", json!({"action": "remove", "output": 2}));
    assert_eq!(last["isError"], json!(true));
    assert_eq!(mcp.server.compositor().outputs().len(), 1);
}

/// `windows` waits for a window that matches, and is answered as soon as one maps; a waiting
/// call that is cancelled is never answered; one whose time is up is answered with no window.
#[test]
fn windows_waits_for_a_window_until_it_maps_its_time_is_up_or_it_is_cancelled() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    let empty = mcp.call("windows", json!({}));
    assert_eq!(empty["structuredContent"], json!({"windows": []}));
    let waits = [
        (
            100,
            json!({"app_id": "org.example.Test", "wait_ms": 10_000}),
        ),
        (101, json!({"wait_ms": 10_000})),
        (102, json!({"app_id": "org.example.Other", "wait_ms": 30})),
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

    let mut client = Client::connect(mcp.server.compositor());
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
    assert_eq!(windows[0]["title"], json!("Test"));
    assert_eq!(windows[0]["output"], json!(1));
    assert_eq!(
        windows[0]["rect"],
        json!({"x": 16.0, "y": 16.0, "width": 1256.0, "height": 1408.0})
    );
    // The cancelled call stays unanswered.
    std::thread::sleep(Duration::from_millis(20));
    assert!(mcp.replies().iter().all(|reply| reply["id"] != json!(101)));

    let too_long = mcp.call("windows", json!({"wait_ms": 60_000}));
    assert_eq!(too_long["isError"], json!(true));
}

/// At most 64 calls wait at once: the next is refused at once, as a tool error.
#[test]
fn at_most_64_calls_wait() {
    let mut mcp = Mcp::new(ClockMode::Manual);
    for id in 0..MAX_WAITING_CALLS {
        mcp.send(
            &json!({"jsonrpc": "2.0", "id": id + 500, "method": "tools/call",
                          "params": modern(json!({"name": "windows",
                                                  "arguments": {"wait_ms": 10_000}}))}),
        );
    }
    assert_eq!(mcp.replies(), Vec::<Value>::new());
    let refused = mcp.call("windows", json!({"wait_ms": 10_000}));
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
