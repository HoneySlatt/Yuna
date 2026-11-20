//! MCP's `clipboard` tool on a seat's clipboard and primary selection, the server in the test's
//! own process (`mcp.rs`): what a Wayland or an X11 program copied, read exactly with its types;
//! text an agent sets, pasted exactly by a Wayland or an X11 program, from either selection, or
//! empty; a program that never writes; data past `max_bytes`; data that is not text; a text past
//! `set`'s bound; clearing, an X11 owner's too; a seat closed, or a call cancelled, while a read
//! waits; a seat closed while writes run; the bound on waiting calls; readers that never read the
//! agent's text, and one that reads it slowly.
//!
//! The stories run on the real clock: the time limits are real time. A Wayland program is a
//! selection client (`selection_client.rs`) of the seat whose window has the keyboard; one that
//! copied writes its data on a thread of its own, as a program writes into its pipe. An X11
//! program is the X11 test client (`x11_client.rs`) on the seat's X display, on a thread of its
//! own.

use std::fs::File;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, Window};
use yuna::mcp::{
    DEFAULT_CLIPBOARD_BYTES, MAX_CLIPBOARD_BYTES, MAX_CLIPBOARD_TEXT_BYTES, MAX_WAITING_CALLS,
};
use yuna::{
    CLIPBOARD_TIMEOUT, ClockMode, Compositor, Config, MAX_CLIPBOARD_WRITES, X11Display,
    XwaylandConfig,
};
use yuna_types::SeatId;

use crate::mcp::{Mcp, modern};
use crate::selection_client::{Pipe, SelectionClient, SelectionEvent};
use crate::x11_client::{PATIENCE, WindowSpec, X11Client, connect};
use crate::x11_selections::{Pasted, atoms_of, paste, serve, take_selection};
use crate::x11_windows::{Folder, open_drawn, pump};

const TEXT: &str = "text/plain;charset=utf-8";

/// The types an agent's text is offered as.
const AGENT_TYPES: [&str; 3] = [TEXT, "text/plain", "UTF8_STRING"];

/// The next id of a request a story sends itself, far from those `Mcp::request` numbers.
fn fresh_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1_000_000);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Creates a seat whose output is light to draw, 1920 × 1080 at 60 Hz and scale 1; its handle.
fn light_seat(mcp: &mut Mcp) -> u64 {
    let created = mcp.call(
        "seat",
        json!({"action": "create", "width": 1920, "height": 1080, "rate": 60, "scale": 1}),
    );
    assert_eq!(created["isError"], json!(false), "{created}");
    created["structuredContent"]["seat"]
        .as_u64()
        .expect("a handle")
}

/// A Wayland program of `seat` whose window has the seat's keyboard, its client told so.
fn focused_client(mcp: &mut Mcp, seat: u64) -> SelectionClient {
    let compositor = mcp.server.compositor();
    let mut wl = SelectionClient::connect_to(compositor, SeatId(seat));
    let window = wl.client.open_window(compositor);
    wl.client.ack(window);
    wl.client.attach(window, 400, 300, 0xFF_20_40_60);
    wl.client.commit(window);
    wl.roundtrip(compositor);
    let deadline = Instant::now() + PATIENCE;
    while wl.state.keyboard_on.is_none() {
        assert!(
            Instant::now() < deadline,
            "the client never gets the keyboard"
        );
        pump(mcp.server.compositor());
        wl.roundtrip(mcp.server.compositor());
    }
    wl
}

/// Sends a `clipboard` call without waiting for its answer; returns its id.
fn send_clipboard(mcp: &mut Mcp, arguments: Value) -> u64 {
    let id = fresh_id();
    mcp.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                     "params": modern(json!({"name": "clipboard", "arguments": arguments}))}));
    id
}

/// Runs the server, and reads `wl`'s events, until the answer to request `id` comes, for
/// [`PATIENCE`] at most; each pipe `wl`'s source `source` is sent meanwhile goes to `answer`.
/// Returns the answer's result.
fn answer_while(
    mcp: &mut Mcp,
    wl: &mut SelectionClient,
    id: u64,
    source: usize,
    mut answer: impl FnMut(OwnedFd),
) -> Value {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(reply) = mcp.replies().into_iter().find(|r| r["id"] == json!(id)) {
            return reply["result"].clone();
        }
        wl.read_events();
        for pipe in wl.take_sent(source) {
            answer(pipe);
        }
        assert!(Instant::now() < deadline, "no answer to request {id}");
        thread::sleep(Duration::from_millis(1));
    }
}

/// Sets `seat`'s clipboard to `text`, longer than the requests' socket holds, through a request
/// sent on a thread of its own; returns the call's result.
fn set_long_text(mcp: &mut Mcp, seat: u64, text: &str) -> Value {
    let id = fresh_id();
    let sending = mcp.send_on_thread(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": modern(json!({"name": "clipboard",
                                "arguments": {"action": "set", "seat": seat, "text": text}}))}));
    let reply = mcp.reply_to(id);
    sending.join().expect("the request is sent");
    reply["result"].clone()
}

/// Writes `data` into `pipe` on a thread of its own, then closes it, as a program that copied
/// does; a reader that closed its end ends it.
fn write_on_thread(pipe: OwnedFd, data: Vec<u8>) -> JoinHandle<()> {
    thread::spawn(move || {
        let _ = File::from(pipe).write_all(&data);
    })
}

/// Calls `clipboard` with `arguments` while `wl`'s source `source` writes `data` into each pipe
/// it is sent; returns the call's result.
fn get_while_writing(
    mcp: &mut Mcp,
    wl: &mut SelectionClient,
    source: usize,
    data: &[u8],
    arguments: Value,
) -> Value {
    let id = send_clipboard(mcp, arguments);
    let mut writers = Vec::new();
    let result = answer_while(mcp, wl, id, source, |pipe| {
        writers.push(write_on_thread(pipe, data.to_vec()));
    });
    for writer in writers {
        writer.join().expect("the program's writer ends");
    }
    result
}

/// The offer of the first selection event of `wl`'s past its `since`-th that offers something,
/// of the primary selection or the clipboard, waited for while the server runs.
fn new_offer(mcp: &mut Mcp, wl: &mut SelectionClient, since: usize, primary: bool) -> usize {
    let deadline = Instant::now() + PATIENCE;
    loop {
        wl.read_events();
        let offered = wl.state.events[since.min(wl.state.events.len())..]
            .iter()
            .find_map(|event| match (*event, primary) {
                (SelectionEvent::Clipboard(Some(offer)), false)
                | (SelectionEvent::Primary(Some(offer)), true) => Some(offer),
                _ => None,
            });
        if let Some(offer) = offered {
            return offer;
        }
        assert!(Instant::now() < deadline, "the client is never offered it");
        mcp.replies();
        thread::sleep(Duration::from_millis(1));
    }
}

/// Reads `pipe` while the server runs, until it ends, for [`PATIENCE`] at most.
fn read_to_end(mcp: &mut Mcp, mut pipe: Pipe) -> Vec<u8> {
    let deadline = Instant::now() + PATIENCE;
    let mut data = Vec::new();
    while !pipe.read_available(&mut data) {
        assert!(Instant::now() < deadline, "the pipe never ends");
        mcp.replies();
        thread::sleep(Duration::from_millis(1));
    }
    data
}

/// `length` bytes of printable ASCII text, with no period a lost or doubled piece could hide in.
fn long_text(length: usize) -> Vec<u8> {
    (0..length as u64)
        .map(|index| {
            let mut x = index + 1;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            b'!' + (x >> 24) as u8 % 94
        })
        .collect()
}

/// `get`'s result for an empty selection.
fn empty() -> Value {
    json!({"mime_types": [], "mime": null, "data": null, "base64": false, "bytes": 0,
           "truncated": false, "timed_out": false})
}

/// `get`'s result for `text`, read whole as `mime` from a selection offering `mime_types`.
fn text_read(mime_types: &[&str], mime: &str, text: &str) -> Value {
    json!({"mime_types": mime_types, "mime": mime, "data": text, "base64": false,
           "bytes": text.len(), "truncated": false, "timed_out": false})
}

/// Waits for the compositor's deadlines to pass, then checks that 300 ms of the server's loop
/// wake nothing: no write or read left polling.
fn assert_quiet(mcp: &mut Mcp) {
    let deadline = Instant::now() + PATIENCE;
    while mcp.server.compositor().next_deadline().is_some() {
        assert!(
            Instant::now() < deadline,
            "a deadline is armed again and again"
        );
        mcp.replies();
        thread::sleep(Duration::from_millis(1));
    }
    let before = mcp.server.compositor().counters().wakeups;
    mcp.server
        .dispatch(Some(Duration::from_millis(300)))
        .expect("the server dispatches");
    assert_eq!(
        mcp.server.compositor().counters().wakeups,
        before,
        "Yuna rests"
    );
}

/// Runs the server until `until`.
fn pump_until(mcp: &mut Mcp, until: Instant) {
    while Instant::now() < until {
        mcp.replies();
        thread::sleep(Duration::from_millis(5));
    }
}

/// An empty selection answers at once, without an error; a copy is read exactly, as the type
/// asked or the first text type it offers, with every type it offers; a type it does not offer
/// is refused; the agent's own text, as long as `set` takes, replaces it, is offered to the client
/// with the keyboard and pasted exactly, its writes made as the reader drains them, until the
/// client copies again. Yuna then rests.
#[test]
fn a_wayland_copy_is_read_and_an_agents_text_pasted_exactly() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    assert_eq!(
        mcp.call("clipboard", json!({"action": "get", "seat": seat}))["structuredContent"],
        empty()
    );
    let mut wl = focused_client(&mut mcp, seat);

    let copied = "Yuna, ユナ, copied";
    let source = wl.set_clipboard(&["text/html", "text/plain", TEXT], wl.enter_serial());
    wl.roundtrip(mcp.server.compositor());
    let types = ["text/html", "text/plain", TEXT];
    let got = get_while_writing(
        &mut mcp,
        &mut wl,
        source,
        copied.as_bytes(),
        json!({"action": "get", "seat": seat}),
    );
    assert_eq!(got["structuredContent"], text_read(&types, TEXT, copied));
    let html = get_while_writing(
        &mut mcp,
        &mut wl,
        source,
        b"<b>Yuna</b>",
        json!({"action": "get", "seat": seat, "mime": "text/html"}),
    );
    assert_eq!(
        html["structuredContent"],
        text_read(&types, "text/html", "<b>Yuna</b>")
    );
    assert_eq!(wl.source(source).asked, [TEXT, "text/html"]);
    let refused = mcp.call(
        "clipboard",
        json!({"action": "get", "seat": seat, "mime": "image/png"}),
    );
    assert_eq!(refused["isError"], json!(true), "{refused}");

    let text = String::from_utf8(long_text(MAX_CLIPBOARD_TEXT_BYTES)).expect("ASCII");
    let since = wl.state.events.len();
    let set = set_long_text(&mut mcp, seat, &text);
    assert_eq!(
        set["structuredContent"],
        json!({"mime_types": AGENT_TYPES, "bytes": text.len()})
    );
    let offer = new_offer(&mut mcp, &mut wl, since, false);
    assert_eq!(wl.state.offers[offer].mime_types, AGENT_TYPES);
    assert!(wl.source(source).cancelled, "the client's copy is replaced");
    let pipe = wl.receive(offer, "text/plain");
    wl.client.send();
    assert!(
        read_to_end(&mut mcp, pipe) == text.as_bytes(),
        "the text comes whole and in order"
    );
    assert_eq!(
        mcp.call("clipboard", json!({"action": "get", "seat": seat}))["structuredContent"],
        json!({"mime_types": AGENT_TYPES, "mime": TEXT,
               "data": &text[..DEFAULT_CLIPBOARD_BYTES], "base64": false,
               "bytes": DEFAULT_CLIPBOARD_BYTES, "truncated": true, "timed_out": false}),
        "the agent's own text, read at once, to max_bytes"
    );

    // A client that copies again holds the selection.
    let again = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(mcp.server.compositor());
    let got = get_while_writing(
        &mut mcp,
        &mut wl,
        again,
        b"mine again",
        json!({"action": "get", "seat": seat}),
    );
    assert_eq!(
        got["structuredContent"],
        text_read(&[TEXT], TEXT, "mine again")
    );
    assert_quiet(&mut mcp);
}

/// The primary selection is apart from the clipboard: a client's primary selection is read with
/// `selection: primary` alone, and the agent's text set there is offered and pasted as the
/// primary selection, the clipboard left empty.
#[test]
fn the_primary_selection_is_read_and_set_apart_from_the_clipboard() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let mut wl = focused_client(&mut mcp, seat);
    let source = wl.set_primary(&[TEXT], wl.enter_serial());
    wl.roundtrip(mcp.server.compositor());
    let selected = get_while_writing(
        &mut mcp,
        &mut wl,
        source,
        b"selected",
        json!({"action": "get", "seat": seat, "selection": "primary"}),
    );
    assert_eq!(
        selected["structuredContent"],
        text_read(&[TEXT], TEXT, "selected")
    );
    let clipboard = mcp.call("clipboard", json!({"action": "get", "seat": seat}));
    assert_eq!(clipboard["structuredContent"], empty());

    let since = wl.state.events.len();
    mcp.call(
        "clipboard",
        json!({"action": "set", "seat": seat, "selection": "primary", "text": "chosen"}),
    );
    let offer = new_offer(&mut mcp, &mut wl, since, true);
    let pipe = wl.receive(offer, TEXT);
    wl.client.send();
    assert_eq!(read_to_end(&mut mcp, pipe), b"chosen");
    let clipboard = mcp.call("clipboard", json!({"action": "get", "seat": seat}));
    assert_eq!(clipboard["structuredContent"], empty());
}

/// A program that writes a little, then never closes its pipe: `get` answers with what came and
/// `timed_out`, once [`CLIPBOARD_TIMEOUT`] is up and soon after, and Yuna goes on answering.
#[test]
fn a_program_that_never_finishes_writing_times_the_read_out() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let mut wl = focused_client(&mut mcp, seat);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(mcp.server.compositor());

    let asked = Instant::now();
    let id = send_clipboard(&mut mcp, json!({"action": "get", "seat": seat}));
    let mut held = Vec::new();
    let got = answer_while(&mut mcp, &mut wl, id, source, |pipe| {
        let mut pipe = File::from(pipe);
        pipe.write_all(b"partial").expect("the pipe takes it");
        held.push(pipe);
    });
    let took = asked.elapsed();
    assert_eq!(held.len(), 1, "the program was asked, and holds its pipe");
    assert_eq!(
        got["structuredContent"],
        json!({"mime_types": [TEXT], "mime": TEXT, "data": "partial", "base64": false,
               "bytes": 7, "truncated": false, "timed_out": true})
    );
    assert!(took >= CLIPBOARD_TIMEOUT, "not before the limit: {took:?}");
    assert!(
        took < CLIPBOARD_TIMEOUT + Duration::from_secs(3),
        "within the limit: {took:?}"
    );
    let listed = mcp.call("seat", json!({"action": "list"}));
    assert_eq!(listed["isError"], json!(false));
    assert_eq!(mcp.server.stopping(), None);
}

/// 3 MiB copied: `get` gives `max_bytes` of it, its first bytes, and `truncated`, 64 KiB by
/// default; the program that wrote sees its pipe closed. `max_bytes` past 1 MiB, or 0, is
/// refused.
#[test]
fn a_large_copy_is_read_to_max_bytes_and_said_truncated() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let mut wl = focused_client(&mut mcp, seat);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(mcp.server.compositor());
    let data = long_text(3 * 1024 * 1024);
    let text = std::str::from_utf8(&data).expect("ASCII");

    for (arguments, length) in [
        (
            json!({"action": "get", "seat": seat, "max_bytes": MAX_CLIPBOARD_BYTES}),
            MAX_CLIPBOARD_BYTES,
        ),
        (
            json!({"action": "get", "seat": seat}),
            DEFAULT_CLIPBOARD_BYTES,
        ),
    ] {
        let got = get_while_writing(&mut mcp, &mut wl, source, &data, arguments);
        let read = &got["structuredContent"];
        assert_eq!(read["bytes"], json!(length));
        assert_eq!(read["truncated"], json!(true));
        assert_eq!(read["timed_out"], json!(false));
        assert_eq!(read["base64"], json!(false));
        assert!(read["data"] == json!(&text[..length]), "its first bytes");
    }
    for max_bytes in [0, MAX_CLIPBOARD_BYTES + 1] {
        let refused = mcp.call(
            "clipboard",
            json!({"action": "get", "seat": seat, "max_bytes": max_bytes}),
        );
        assert_eq!(refused["isError"], json!(true), "{refused}");
    }
    assert_quiet(&mut mcp);
}

/// Data that is not UTF-8 comes in base64: an image, or bytes offered as text; a selection
/// offering no text type gives its types, and no data unless a type is named.
#[test]
fn data_that_is_not_text_comes_in_base64() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let mut wl = focused_client(&mut mcp, seat);
    let image = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0xFF];
    let source = wl.set_clipboard(&["image/png"], wl.enter_serial());
    wl.roundtrip(mcp.server.compositor());
    let types = mcp.call("clipboard", json!({"action": "get", "seat": seat}));
    assert_eq!(
        types["structuredContent"],
        json!({"mime_types": ["image/png"], "mime": null, "data": null, "base64": false,
               "bytes": 0, "truncated": false, "timed_out": false})
    );
    let got = get_while_writing(
        &mut mcp,
        &mut wl,
        source,
        &image,
        json!({"action": "get", "seat": seat, "mime": "image/png"}),
    );
    assert_eq!(
        got["structuredContent"],
        json!({"mime_types": ["image/png"], "mime": "image/png", "data": "iVBORw0KGgoA/w==",
               "base64": true, "bytes": 10, "truncated": false, "timed_out": false})
    );

    let bad_text = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(mcp.server.compositor());
    let got = get_while_writing(
        &mut mcp,
        &mut wl,
        bad_text,
        &[b'Y', 0xC3, b'('],
        json!({"action": "get", "seat": seat}),
    );
    assert_eq!(got["structuredContent"]["data"], json!("WcMo"));
    assert_eq!(got["structuredContent"]["base64"], json!(true));
}

/// Text cut by `max_bytes` inside a character is given as text up to that character.
#[test]
fn text_cut_inside_a_character_stays_text() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    mcp.call(
        "clipboard",
        json!({"action": "set", "seat": seat, "text": "ユナ"}),
    );
    let got = mcp.call(
        "clipboard",
        json!({"action": "get", "seat": seat, "max_bytes": 4}),
    );
    assert_eq!(
        got["structuredContent"],
        json!({"mime_types": AGENT_TYPES, "mime": TEXT, "data": "ユ", "base64": false,
               "bytes": 3, "truncated": true, "timed_out": false})
    );
}

/// `clear` empties the agent's selection and a client's alike: the client with the keyboard is
/// told the clipboard holds nothing, a client's copy is cancelled, and `get` finds it empty.
#[test]
fn clear_empties_the_agents_selection_and_a_clients() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let mut wl = focused_client(&mut mcp, seat);
    let since = wl.state.events.len();
    mcp.call(
        "clipboard",
        json!({"action": "set", "seat": seat, "text": "short-lived"}),
    );
    new_offer(&mut mcp, &mut wl, since, false);
    let cleared = mcp.call("clipboard", json!({"action": "clear", "seat": seat}));
    assert_eq!(cleared["structuredContent"], json!({"mime_types": []}));
    mcp.replies();
    wl.roundtrip(mcp.server.compositor());
    assert_eq!(
        wl.state.events.last(),
        Some(&SelectionEvent::Clipboard(None))
    );
    assert_eq!(
        mcp.call("clipboard", json!({"action": "get", "seat": seat}))["structuredContent"],
        empty()
    );

    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(mcp.server.compositor());
    mcp.call("clipboard", json!({"action": "clear", "seat": seat}));
    mcp.replies();
    wl.roundtrip(mcp.server.compositor());
    assert!(wl.source(source).cancelled);
    assert_eq!(
        mcp.call("clipboard", json!({"action": "get", "seat": seat}))["structuredContent"],
        empty()
    );
}

/// A seat closed while a `get` waits for its program fails the call, which the closing
/// answers alongside.
#[test]
fn closing_the_seat_fails_a_waiting_get() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let mut wl = focused_client(&mut mcp, seat);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(mcp.server.compositor());
    let get = send_clipboard(&mut mcp, json!({"action": "get", "seat": seat}));
    let deadline = Instant::now() + PATIENCE;
    let mut held = Vec::new();
    while held.is_empty() {
        assert!(Instant::now() < deadline, "the program is never asked");
        assert_eq!(mcp.replies(), Vec::<Value>::new(), "the get waits");
        wl.read_events();
        held.extend(wl.take_sent(source));
    }
    let close = fresh_id();
    mcp.send(
        &json!({"jsonrpc": "2.0", "id": close, "method": "tools/call",
                     "params": modern(json!({"name": "seat",
                                             "arguments": {"action": "close", "seat": seat}}))}),
    );
    let mut replies = Vec::new();
    while replies.len() < 2 {
        assert!(
            Instant::now() < deadline,
            "the calls are not answered: {replies:?}"
        );
        replies.extend(mcp.replies());
    }
    let failed = replies
        .iter()
        .find(|r| r["id"] == json!(get))
        .expect("the get");
    assert_eq!(failed["result"]["isError"], json!(true));
    assert_eq!(
        failed["result"]["content"][0]["text"],
        json!(format!("seat {seat} was closed"))
    );
    let closed = replies
        .iter()
        .find(|r| r["id"] == json!(close))
        .expect("the close");
    assert_eq!(closed["result"]["structuredContent"]["closed"], json!(seat));
}

/// A `get` that waits counts among the 64 calls that may wait: with 63 `windows` calls, the
/// next call that would wait is refused; once the `get` is cancelled, it is never answered, and
/// one more call may wait.
#[test]
fn a_waiting_get_counts_among_the_waiting_calls_and_is_cancelled() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let mut wl = focused_client(&mut mcp, seat);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(mcp.server.compositor());
    let windows = |id: u64| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
               "params": modern(json!({"name": "windows",
                                       "arguments": {"seat": seat, "app_id": "none",
                                                     "wait_ms": 30_000}}))})
    };
    for _ in 1..MAX_WAITING_CALLS {
        mcp.send(&windows(fresh_id()));
    }
    let get = send_clipboard(&mut mcp, json!({"action": "get", "seat": seat}));
    let deadline = Instant::now() + PATIENCE;
    let mut held = Vec::new();
    while held.is_empty() {
        assert!(Instant::now() < deadline, "the program is never asked");
        assert_eq!(mcp.replies(), Vec::<Value>::new(), "all wait");
        wl.read_events();
        held.extend(wl.take_sent(source));
    }
    let refused = mcp.call(
        "windows",
        json!({"seat": seat, "app_id": "none", "wait_ms": 1}),
    );
    let full = json!(format!("{MAX_WAITING_CALLS} calls wait already"));
    assert_eq!(refused["isError"], json!(true), "{refused}");
    assert_eq!(refused["content"][0]["text"], full);
    let another_get = mcp.call("clipboard", json!({"action": "get", "seat": seat}));
    assert_eq!(another_get["isError"], json!(true), "{another_get}");
    assert_eq!(another_get["content"][0]["text"], full);

    mcp.send(
        &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                     "params": {"requestId": get, "reason": "the test"}}),
    );
    let waits = fresh_id();
    mcp.send(&windows(waits));
    let until = Instant::now() + Duration::from_millis(200);
    while Instant::now() < until {
        let replies = mcp.replies();
        assert!(
            replies
                .iter()
                .all(|r| r["id"] != json!(get) && r["id"] != json!(waits)),
            "the cancelled get is never answered, and the new call waits: {replies:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

/// Readers that never read the agent's text: at most 8 are written at once, the 9th's pipe
/// ending at once, empty; the others hold what their pipe takes, their pipe still open just
/// before [`CLIPBOARD_TIMEOUT`] without progress, and closed soon after. A reader after them gets
/// the whole text, and Yuna then rests.
#[test]
fn readers_that_never_read_are_bounded_and_closed_in_time() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let mut wl = focused_client(&mut mcp, seat);
    let text = long_text(256 * 1024);
    let since = wl.state.events.len();
    let set = set_long_text(
        &mut mcp,
        seat,
        &String::from_utf8(text.clone()).expect("ASCII"),
    );
    assert_eq!(set["isError"], json!(false), "{set}");
    let offer = new_offer(&mut mcp, &mut wl, since, false);
    let asked = Instant::now();
    let mut pipes: Vec<Pipe> = (0..=MAX_CLIPBOARD_WRITES)
        .map(|_| wl.receive(offer, TEXT))
        .collect();
    wl.client.send();
    let refused = pipes.pop().expect("the last reader");
    assert_eq!(
        read_to_end(&mut mcp, refused),
        b"",
        "past the bound, refused"
    );
    assert!(asked.elapsed() < Duration::from_secs(1), "at once");
    pump_until(
        &mut mcp,
        asked + CLIPBOARD_TIMEOUT - Duration::from_millis(500),
    );
    assert!(
        pipes.iter().all(|pipe| !pipe.writer_closed()),
        "every write still open before the limit"
    );
    let deadline = asked + CLIPBOARD_TIMEOUT + Duration::from_secs(2);
    while !pipes.iter().all(Pipe::writer_closed) {
        assert!(
            Instant::now() < deadline,
            "a write still open past the limit"
        );
        mcp.replies();
        thread::sleep(Duration::from_millis(5));
    }
    for pipe in pipes {
        let held = read_to_end(&mut mcp, pipe);
        assert!(
            !held.is_empty() && held.len() < text.len(),
            "what its pipe took: {} bytes",
            held.len()
        );
        assert!(text.starts_with(&held));
    }
    let pipe = wl.receive(offer, TEXT);
    wl.client.send();
    assert!(
        read_to_end(&mut mcp, pipe) == text,
        "whole, once the others ended"
    );
    assert_quiet(&mut mcp);
}

/// A text one byte past what `set` takes is refused, the server answering on; the selection
/// stays as it was.
#[test]
fn a_text_past_sets_bound_is_refused_and_the_server_answers_on() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let too_long = String::from_utf8(long_text(MAX_CLIPBOARD_TEXT_BYTES + 1)).expect("ASCII");
    let refused = set_long_text(&mut mcp, seat, &too_long);
    assert_eq!(refused["isError"], json!(true), "{refused}");
    assert_eq!(
        refused["content"][0]["text"],
        json!(format!(
            "the text takes {} bytes, over the {MAX_CLIPBOARD_TEXT_BYTES} set takes",
            MAX_CLIPBOARD_TEXT_BYTES + 1
        ))
    );
    assert_eq!(mcp.server.stopping(), None);
    let listed = mcp.call("seat", json!({"action": "list"}));
    assert_eq!(listed["isError"], json!(false), "{listed}");
    assert_eq!(
        mcp.call("clipboard", json!({"action": "get", "seat": seat}))["structuredContent"],
        empty()
    );
}

/// An empty text is a selection all the same: offered as text, read empty, and pasted empty.
#[test]
fn an_empty_text_is_offered_and_pasted_empty() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let mut wl = focused_client(&mut mcp, seat);
    let since = wl.state.events.len();
    let set = mcp.call(
        "clipboard",
        json!({"action": "set", "seat": seat, "text": ""}),
    );
    assert_eq!(
        set["structuredContent"],
        json!({"mime_types": AGENT_TYPES, "bytes": 0})
    );
    assert_eq!(
        mcp.call("clipboard", json!({"action": "get", "seat": seat}))["structuredContent"],
        text_read(&AGENT_TYPES, TEXT, "")
    );
    let offer = new_offer(&mut mcp, &mut wl, since, false);
    let pipe = wl.receive(offer, TEXT);
    wl.client.send();
    assert_eq!(read_to_end(&mut mcp, pipe), b"");
    assert_quiet(&mut mcp);
}

/// A reader that drains the agent's text slowly, 32 KiB a second, is written to past
/// [`CLIPBOARD_TIMEOUT`], since the write never waits that long without progress, and gets the
/// whole text.
#[test]
fn a_reader_that_drains_slowly_gets_the_whole_text() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let mut wl = focused_client(&mut mcp, seat);
    let text = long_text(MAX_CLIPBOARD_TEXT_BYTES);
    let since = wl.state.events.len();
    let set = set_long_text(
        &mut mcp,
        seat,
        &String::from_utf8(text.clone()).expect("ASCII"),
    );
    assert_eq!(set["isError"], json!(false), "{set}");
    let offer = new_offer(&mut mcp, &mut wl, since, false);
    let mut pipe = wl.receive(offer, TEXT);
    wl.client.send();
    let started = Instant::now();
    let mut data = Vec::new();
    let mut written_past_the_limit = false;
    while !pipe.read_at_most(&mut data, 32 * 1024) {
        assert!(started.elapsed() < PATIENCE, "the text never comes whole");
        if started.elapsed() > CLIPBOARD_TIMEOUT + Duration::from_millis(500)
            && !pipe.writer_closed()
        {
            written_past_the_limit = true;
        }
        pump_until(&mut mcp, Instant::now() + Duration::from_secs(1));
    }
    assert!(
        written_past_the_limit,
        "the write ran past the limit, which this story is about"
    );
    assert!(data == text, "the text comes whole and in order");
}

/// A seat closed while Yuna writes its agent's text to a reader that does not read: the write
/// ends with the seat, its pipe closed at once rather than at the time limit.
#[test]
fn closing_the_seat_ends_its_writes() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let seat = light_seat(&mut mcp);
    let mut wl = focused_client(&mut mcp, seat);
    let since = wl.state.events.len();
    let text = String::from_utf8(long_text(MAX_CLIPBOARD_TEXT_BYTES)).expect("ASCII");
    let set = set_long_text(&mut mcp, seat, &text);
    assert_eq!(set["isError"], json!(false), "{set}");
    let offer = new_offer(&mut mcp, &mut wl, since, false);
    let pipe = wl.receive(offer, TEXT);
    wl.client.send();
    pump_until(&mut mcp, Instant::now() + Duration::from_millis(300));
    assert!(!pipe.writer_closed(), "the write runs");
    let closed = mcp.call("seat", json!({"action": "close", "seat": seat}));
    assert_eq!(closed["isError"], json!(false), "{closed}");
    let deadline = Instant::now() + Duration::from_secs(1);
    while !pipe.writer_closed() {
        assert!(Instant::now() < deadline, "the write outlives its seat");
        mcp.replies();
        thread::sleep(Duration::from_millis(5));
    }
    assert_quiet(&mut mcp);
}

/// A server whose seats get an X display, and its seat, which listens in a folder of the
/// story's own.
struct X11Mcp {
    mcp: Mcp,
    seat: u64,
    display: X11Display,
    _folder: Folder,
}

impl X11Mcp {
    fn new(story: &str) -> X11Mcp {
        let folder = Folder::new(story);
        let compositor = Compositor::new(Config {
            outputs: Vec::new(),
            clock: ClockMode::Real,
            listen: false,
            announce_owner_seat: false,
            xwayland: Some(XwaylandConfig::default()),
            ..Config::default()
        })
        .expect("the compositor starts");
        let mut mcp = Mcp::serving(compositor);
        let seat = light_seat(&mut mcp);
        let compositor = mcp.server.compositor();
        compositor
            .listen_for_seat(SeatId(seat), folder.0.join("seat"))
            .expect("the seat listens");
        let display = compositor
            .x11_display(SeatId(seat))
            .expect("the seat's display");
        X11Mcp {
            mcp,
            seat,
            display,
            _folder: folder,
        }
    }

    fn client(&mut self) -> X11Client {
        connect(self.mcp.server.compositor(), &self.display)
    }

    /// Runs `work` with `client` on a thread of its own while the server runs, [`PATIENCE`] at
    /// most.
    fn step<T: Send + 'static>(
        &mut self,
        mut client: X11Client,
        work: impl FnOnce(&mut X11Client) -> Result<T, String> + Send + 'static,
    ) -> (X11Client, T) {
        let worker = thread::spawn(move || {
            let result = work(&mut client);
            (client, result)
        });
        let deadline = Instant::now() + PATIENCE + Duration::from_secs(5);
        while !worker.is_finished() {
            assert!(Instant::now() < deadline, "an X11 step never ended");
            self.mcp
                .server
                .dispatch(Some(Duration::from_millis(2)))
                .expect("the server dispatches");
        }
        match worker.join() {
            Ok((client, Ok(value))) => (client, value),
            Ok((_, Err(error))) => panic!("an X11 step failed: {error}"),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}

/// The owner of X11's `selection` now, as `client` asks X11.
fn owner_of(client: &X11Client, selection: Atom) -> Result<Window, String> {
    Ok(client
        .connection
        .get_selection_owner(selection)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .owner)
}

/// Once X11's `selection` has an owner, Yuna's manager for the agent's text, pastes it onto
/// `window` as `UTF8_STRING`.
fn paste_once_owned(
    client: &mut X11Client,
    window: Window,
    selection: Atom,
) -> Result<Pasted, String> {
    let deadline = Instant::now() + PATIENCE;
    while owner_of(client, selection)? == x11rb::NONE {
        if Instant::now() > deadline {
            return Err("Yuna never owns X11's selection".to_owned());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let utf8_string = atoms_of(client).UTF8_STRING;
    paste(client, window, selection, utf8_string)
}

/// An X11 program copies text: `get` reads it through Yuna's X11 manager, as a Wayland paste
/// does, offered as UTF-8 text alone. `clear` then empties it on Wayland's side alone: the X11
/// program still owns X11's clipboard, as `docs/debt.md` says.
#[test]
fn an_x11_copy_is_read_through_the_bridge_and_cleared_on_waylands_side_alone() {
    let mut x = X11Mcp::new("clip-x11-copy");
    let seat = x.seat;
    let mut client = x.client();
    let text = "Yuna, ユナ, from X11";
    let data = text.as_bytes().to_vec();
    let owner = thread::spawn(move || {
        let atoms = atoms_of(&client);
        let owner = take_selection(&mut client, atoms.CLIPBOARD)?;
        serve(&mut client, owner, &data, false)?;
        Ok::<_, String>((client, owner))
    });
    let deadline = Instant::now() + PATIENCE;
    let got = loop {
        let got = x
            .mcp
            .call("clipboard", json!({"action": "get", "seat": seat}));
        if got["structuredContent"]["mime_types"] != json!([]) {
            break got;
        }
        assert!(
            Instant::now() < deadline,
            "the X11 copy never reaches the seat"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(got["structuredContent"], text_read(&[TEXT], TEXT, text));
    let (client, owner) = owner
        .join()
        .expect("the X11 owner ends")
        .expect("it served its copy");

    let cleared = x
        .mcp
        .call("clipboard", json!({"action": "clear", "seat": seat}));
    assert_eq!(cleared["isError"], json!(false), "{cleared}");
    pump_until(&mut x.mcp, Instant::now() + Duration::from_millis(300));
    assert_eq!(
        x.mcp
            .call("clipboard", json!({"action": "get", "seat": seat}))["structuredContent"],
        empty()
    );
    let (_client, still) = x.step(client, |client| {
        let clipboard = atoms_of(client).CLIPBOARD;
        owner_of(client, clipboard)
    });
    assert_eq!(still, owner, "the X11 program keeps X11's clipboard");
}

/// The agent sets text while an X11 window has the seat's keyboard: an X11 program pastes it
/// exactly as `UTF8_STRING`, from X11's `CLIPBOARD` and, set as the primary selection, from
/// `PRIMARY`, once Yuna's manager owns each for it.
#[test]
fn an_agents_text_is_pasted_by_an_x11_window_from_either_selection() {
    let mut x = X11Mcp::new("clip-set-x11");
    let seat = x.seat;
    let client = x.client();
    let (client, window, described, _) = open_drawn(
        x.mcp.server.compositor(),
        client,
        &WindowSpec::named("paster"),
    );
    let compositor = x.mcp.server.compositor();
    compositor
        .change_layout(|layout| layout.focus_window(described.id))
        .expect("the window takes the focus");
    let deadline = Instant::now() + PATIENCE;
    while compositor.keyboard_window(SeatId(seat)) != Some(described.id) {
        assert!(Instant::now() < deadline, "the keyboard never moves");
        pump(compositor);
    }
    let utf8_string = atoms_of(&client).UTF8_STRING;

    let text = "Yuna, ユナ, for X11";
    let set = x.mcp.call(
        "clipboard",
        json!({"action": "set", "seat": seat, "text": text}),
    );
    assert_eq!(set["isError"], json!(false), "{set}");
    let (client, pasted) = x.step(client, move |client| {
        let clipboard = atoms_of(client).CLIPBOARD;
        paste_once_owned(client, window, clipboard)
    });
    assert_eq!(pasted, Some((utf8_string, text.as_bytes().to_vec())));

    let selected = "chosen for PRIMARY";
    let set = x.mcp.call(
        "clipboard",
        json!({"action": "set", "seat": seat, "selection": "primary", "text": selected}),
    );
    assert_eq!(set["isError"], json!(false), "{set}");
    let (_client, pasted) = x.step(client, move |client| {
        paste_once_owned(client, window, AtomEnum::PRIMARY.into())
    });
    assert_eq!(pasted, Some((utf8_string, selected.as_bytes().to_vec())));
}
