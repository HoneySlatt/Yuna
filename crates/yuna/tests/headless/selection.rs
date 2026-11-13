//! The clipboard, the primary selection and `ext-data-control-v1` between clients: stories with
//! exact figures, on the manual clock, each ending at rest.
//!
//! A client is told what the selections hold when it gets the keyboard focus: the clipboard
//! first, then the primary selection. Each client numbers the offers it is given from 0, in
//! order; a selection event names an offer by that number.

use std::time::Duration;

use yuna::Compositor;

use crate::selection_client::SelectionClient;
use crate::selection_client::SelectionEvent::{
    Clipboard, ControlClipboard, ControlPrimary, Primary,
};
use crate::{
    BLUE, LEAD, OUTPUT, RED, advance_to, assert_rests, boundary, manual_compositor, millis,
    open_and_commit, run_armed_deadlines,
};

const TEXT: &str = "text/plain;charset=utf-8";
/// The seat's serials count up from 1 for the whole process: no test gets near this one.
const NEVER_SENT: u32 = u32::MAX;

/// Runs the deadlines still armed, the last frame of a story and its presentation, then checks
/// that Yuna rests.
fn settle_and_rest(compositor: &mut Compositor) {
    run_armed_deadlines(compositor);
    assert_rests(compositor);
}

/// Opens a window for `client` and moves the clock to `at`, a refresh boundary, where the window
/// maps and takes the focus; then the client reads what came.
fn open_with_focus(
    compositor: &mut Compositor,
    client: &mut SelectionClient,
    colour: u32,
    at: Duration,
) {
    open_and_commit(compositor, &mut client.client, colour);
    advance_to(compositor, &mut client.client, at);
    client.roundtrip(compositor);
}

/// Story 1: client A, which has the focus, sets the clipboard with the serial of its keyboard
/// `enter`. Client B's window opens and takes the focus: B is offered the clipboard and asks for
/// it into a pipe; A is sent that pipe, writes `Yuna` and closes it; B reads exactly `Yuna`.
#[test]
fn the_clipboard_follows_the_focus_from_one_client_to_another() {
    let mut compositor = manual_compositor();
    let mut a = SelectionClient::connect(&mut compositor);
    let mut b = SelectionClient::connect(&mut compositor);
    let start = compositor.now();
    open_with_focus(&mut compositor, &mut a, RED, boundary(start, 1));
    assert_eq!(a.state.enters.len(), 1);
    assert_eq!(a.state.events, [Clipboard(None), Primary(None)]);

    let source = a.set_clipboard(&[TEXT], a.enter_serial());
    a.roundtrip(&mut compositor);
    // The client with the focus is offered its own selection.
    assert_eq!(a.state.events[2..], [Clipboard(Some(0))]);

    open_with_focus(&mut compositor, &mut b, BLUE, boundary(start, 2));
    assert_eq!(b.state.enters.len(), 1);
    assert_eq!(b.state.events, [Clipboard(Some(0)), Primary(None)]);
    assert_eq!(b.state.offers.len(), 1);
    assert_eq!(b.state.offers[0].mime_types, [TEXT]);

    let pipe = b.receive(0, TEXT);
    b.roundtrip(&mut compositor);
    a.roundtrip(&mut compositor);
    assert_eq!(a.source(source).asked, [TEXT]);
    a.answer(source, b"Yuna");
    assert_eq!(pipe.read_all(), b"Yuna");
    // A, without the focus now, is told nothing more; its source still holds the clipboard.
    assert_eq!(a.state.events.len(), 3);
    assert!(!a.source(source).cancelled);
    settle_and_rest(&mut compositor);
}

/// Story 2: story 1 with the primary selection.
#[test]
fn the_primary_selection_follows_the_focus_from_one_client_to_another() {
    let mut compositor = manual_compositor();
    let mut a = SelectionClient::connect(&mut compositor);
    let mut b = SelectionClient::connect(&mut compositor);
    let start = compositor.now();
    open_with_focus(&mut compositor, &mut a, RED, boundary(start, 1));

    let source = a.set_primary(&[TEXT], a.enter_serial());
    a.roundtrip(&mut compositor);
    assert_eq!(
        a.state.events,
        [Clipboard(None), Primary(None), Primary(Some(0))]
    );

    open_with_focus(&mut compositor, &mut b, BLUE, boundary(start, 2));
    assert_eq!(b.state.enters.len(), 1);
    assert_eq!(b.state.events, [Clipboard(None), Primary(Some(0))]);
    assert_eq!(b.state.offers[0].mime_types, [TEXT]);

    let pipe = b.receive(0, TEXT);
    b.roundtrip(&mut compositor);
    a.roundtrip(&mut compositor);
    assert_eq!(a.source(source).asked, [TEXT]);
    a.answer(source, b"Yuna");
    assert_eq!(pipe.read_all(), b"Yuna");
    assert_eq!(a.state.events.len(), 3);
    settle_and_rest(&mut compositor);
}

/// Story 3: a client without the focus sets the clipboard and the primary selection, with the
/// serial of the `enter` it got when it had the focus, then with a serial the seat never sent.
/// Smithay takes a selection only from the client with the keyboard focus: each request is
/// ignored, the client with the focus and the data-control client get no new event, and the
/// sources are never used.
#[test]
fn a_selection_from_a_client_without_the_focus_is_ignored() {
    let mut compositor = manual_compositor();
    let mut focused = SelectionClient::connect(&mut compositor);
    let mut other = SelectionClient::connect(&mut compositor);
    let mut manager = SelectionClient::connect_data_control(&mut compositor);
    let start = compositor.now();
    // The other client's window has the focus first; the focused client's then takes it.
    open_with_focus(&mut compositor, &mut other, BLUE, boundary(start, 1));
    open_with_focus(&mut compositor, &mut focused, RED, boundary(start, 2));
    other.roundtrip(&mut compositor);
    manager.roundtrip(&mut compositor);
    assert_eq!(other.state.enters.len(), 1);
    let serial_once_sent = other.enter_serial();

    let sources = [
        other.set_clipboard(&[TEXT], serial_once_sent),
        other.set_primary(&[TEXT], serial_once_sent),
        other.set_clipboard(&[TEXT], NEVER_SENT),
        other.set_primary(&[TEXT], NEVER_SENT),
    ];
    other.roundtrip(&mut compositor);
    focused.roundtrip(&mut compositor);
    manager.roundtrip(&mut compositor);
    assert_eq!(focused.state.events, [Clipboard(None), Primary(None)]);
    assert!(focused.state.offers.is_empty());
    assert_eq!(
        manager.state.events,
        [ControlClipboard(None), ControlPrimary(None)]
    );
    assert_eq!(other.state.events, [Clipboard(None), Primary(None)]);
    for source in sources {
        let source = other.source(source);
        assert!(source.asked.is_empty());
        assert!(!source.cancelled);
    }
    settle_and_rest(&mut compositor);
}

/// Smithay checks the focus, not the serial (`docs/debt.md`): the client with the focus sets the
/// clipboard naming a serial the seat never sent, and the selection is taken. This pins the gap
/// down until Yuna checks serials.
#[test]
fn the_serial_of_the_client_with_the_focus_is_not_checked() {
    let mut compositor = manual_compositor();
    let mut focused = SelectionClient::connect(&mut compositor);
    let mut manager = SelectionClient::connect_data_control(&mut compositor);
    let start = compositor.now();
    open_with_focus(&mut compositor, &mut focused, RED, boundary(start, 1));

    focused.set_clipboard(&[TEXT], NEVER_SENT);
    focused.roundtrip(&mut compositor);
    manager.roundtrip(&mut compositor);
    assert_eq!(
        focused.state.events,
        [Clipboard(None), Primary(None), Clipboard(Some(0))]
    );
    assert_eq!(
        manager.state.events,
        [
            ControlClipboard(None),
            ControlPrimary(None),
            ControlClipboard(Some(0))
        ]
    );
    settle_and_rest(&mut compositor);
}

/// Story 4: a data-control client with no window is offered the current clipboard as soon as
/// it binds its device, then each new one; it receives the data, and sets the clipboard itself,
/// which the client with the focus is then offered and can receive.
#[test]
fn a_data_control_client_reads_and_sets_the_clipboard_without_a_window() {
    let mut compositor = manual_compositor();
    let mut focused = SelectionClient::connect(&mut compositor);
    let start = compositor.now();
    open_with_focus(&mut compositor, &mut focused, RED, boundary(start, 1));
    let first = focused.set_clipboard(&[TEXT], focused.enter_serial());
    focused.roundtrip(&mut compositor);

    let mut manager = SelectionClient::connect_data_control(&mut compositor);
    assert_eq!(
        manager.state.events,
        [ControlClipboard(Some(0)), ControlPrimary(None)]
    );
    assert_eq!(manager.state.offers[0].mime_types, [TEXT]);

    let second = focused.set_clipboard(&[TEXT, "text/plain"], focused.enter_serial());
    focused.roundtrip(&mut compositor);
    manager.roundtrip(&mut compositor);
    assert!(focused.source(first).cancelled);
    assert_eq!(manager.state.events[2..], [ControlClipboard(Some(1))]);
    assert_eq!(manager.state.offers[1].mime_types, [TEXT, "text/plain"]);

    let pipe = manager.receive(1, "text/plain");
    manager.roundtrip(&mut compositor);
    focused.roundtrip(&mut compositor);
    assert_eq!(focused.source(second).asked, ["text/plain"]);
    focused.answer(second, b"Yuna");
    assert_eq!(pipe.read_all(), b"Yuna");

    let own = manager.control_set_clipboard(&["text/uri-list"]);
    manager.roundtrip(&mut compositor);
    focused.roundtrip(&mut compositor);
    assert!(focused.source(second).cancelled);
    assert_eq!(manager.state.events[3..], [ControlClipboard(Some(2))]);
    assert_eq!(
        focused.state.events,
        [
            Clipboard(None),
            Primary(None),
            Clipboard(Some(0)),
            Clipboard(Some(1)),
            Clipboard(Some(2))
        ]
    );
    assert_eq!(focused.state.offers[2].mime_types, ["text/uri-list"]);

    let pipe = focused.receive(2, "text/uri-list");
    focused.roundtrip(&mut compositor);
    manager.roundtrip(&mut compositor);
    assert_eq!(manager.source(own).asked, ["text/uri-list"]);
    manager.answer(own, b"Yuna");
    assert_eq!(pipe.read_all(), b"Yuna");
    settle_and_rest(&mut compositor);
}

/// Story 5: the client whose sources hold the clipboard and the primary selection leaves while
/// another client waits on a transfer from it. The client with the focus and the data-control
/// client are told both selections are empty, and the waiting pipe ends: Yuna kept no copy of it.
#[test]
fn the_selections_empty_when_their_client_leaves() {
    let mut compositor = manual_compositor();
    let mut leaving = SelectionClient::connect(&mut compositor);
    let mut focused = SelectionClient::connect(&mut compositor);
    let mut manager = SelectionClient::connect_data_control(&mut compositor);
    let start = compositor.now();
    open_with_focus(&mut compositor, &mut leaving, RED, boundary(start, 1));
    let serial = leaving.enter_serial();
    let clipboard = leaving.set_clipboard(&[TEXT], serial);
    leaving.set_primary(&[TEXT], serial);
    leaving.roundtrip(&mut compositor);

    open_with_focus(&mut compositor, &mut focused, BLUE, boundary(start, 2));
    manager.roundtrip(&mut compositor);
    assert_eq!(focused.state.events, [Clipboard(Some(0)), Primary(Some(1))]);
    assert_eq!(
        manager.state.events,
        [
            ControlClipboard(None),
            ControlPrimary(None),
            ControlClipboard(Some(0)),
            ControlPrimary(Some(1))
        ]
    );

    // A transfer starts: the leaving client holds the pipe and has not written yet.
    let pipe = focused.receive(0, TEXT);
    focused.roundtrip(&mut compositor);
    leaving.roundtrip(&mut compositor);
    assert_eq!(leaving.source(clipboard).asked, [TEXT]);
    drop(leaving);

    focused.roundtrip(&mut compositor);
    manager.roundtrip(&mut compositor);
    let mut told_focused = focused.state.events[2..].to_vec();
    told_focused.sort();
    assert_eq!(told_focused, [Clipboard(None), Primary(None)]);
    let mut told_manager = manager.state.events[4..].to_vec();
    told_manager.sort();
    assert_eq!(told_manager, [ControlClipboard(None), ControlPrimary(None)]);
    assert_eq!(pipe.read_all(), b"");
    settle_and_rest(&mut compositor);
}

/// Story 5, misbehaving requests: a source destroyed as soon as it is set, a receive from a
/// selection whose source is gone, a MIME type never offered, an offer destroyed while its
/// transfer runs, and a drag with no pointer to start it. Each pipe that cannot be served ends
/// at once; the drag is refused, never half-started; and another client keeps its frame
/// callbacks.
#[test]
fn misbehaving_selection_requests_leave_the_others_running() {
    let mut compositor = manual_compositor();
    let mut other = SelectionClient::connect(&mut compositor);
    let mut client = SelectionClient::connect(&mut compositor);
    let start = compositor.now();
    open_with_focus(&mut compositor, &mut other, RED, boundary(start, 1));
    open_with_focus(&mut compositor, &mut client, BLUE, boundary(start, 2));
    let serial = client.enter_serial();
    assert_eq!(client.state.events, [Clipboard(None), Primary(None)]);

    // A source destroyed at once: the clipboard empties, and its offer serves nothing.
    let gone = client.set_clipboard(&[TEXT], serial);
    client.destroy_source(gone);
    client.roundtrip(&mut compositor);
    assert_eq!(
        client.state.events[2..],
        [Clipboard(Some(0)), Clipboard(None)]
    );
    let pipe = client.receive(0, TEXT);
    client.roundtrip(&mut compositor);
    assert_eq!(pipe.read_all(), b"");

    // A MIME type never offered: refused, and the source is not asked.
    let source = client.set_clipboard(&[TEXT], serial);
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.events[4..], [Clipboard(Some(1))]);
    let pipe = client.receive(1, "image/png");
    client.roundtrip(&mut compositor);
    assert_eq!(pipe.read_all(), b"");
    assert!(client.source(source).asked.is_empty());

    // An offer destroyed while its transfer runs: the pipe still goes to the source.
    let pipe = client.receive(1, TEXT);
    client.destroy_offer(1);
    client.roundtrip(&mut compositor);
    assert_eq!(client.source(source).asked, [TEXT]);
    client.answer(source, b"Yuna");
    assert_eq!(pipe.read_all(), b"Yuna");

    // A drag with no pointer grab: refused, with no event to the device or the source.
    let surface = client.client.window(0).surface.clone();
    let dragged = client.start_drag(&[TEXT], &surface, serial);
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.events.len(), 5);
    let dragged = client.source(dragged);
    assert_eq!(dragged.drag_events, 0);
    assert!(dragged.asked.is_empty() && !dragged.cancelled);

    // The other client keeps its frame callbacks.
    let window = 0;
    other.client.request_frame(window);
    other.client.commit(window);
    other.roundtrip(&mut compositor);
    let frames_before = other.client.window(window).frames.len();
    advance_to(&mut compositor, &mut other.client, boundary(start, 3));
    assert_eq!(
        other.client.window(window).frames[frames_before..],
        [millis(boundary(start, 3) - LEAD)]
    );
    settle_and_rest(&mut compositor);
}

/// A data-control client that sets the clipboard twice to one source breaks the protocol: it is
/// disconnected alone, its selection empties, and the client with the focus keeps running.
#[test]
fn a_data_control_client_that_breaks_the_protocol_leaves_alone() {
    let mut compositor = manual_compositor();
    let mut focused = SelectionClient::connect(&mut compositor);
    let mut manager = SelectionClient::connect_data_control(&mut compositor);
    let start = compositor.now();
    open_with_focus(&mut compositor, &mut focused, RED, boundary(start, 1));

    let source = manager.control_set_clipboard(&[TEXT]);
    manager.control_set_clipboard_source(source);
    // Its requests go out; it is not read again, for its connection ends in a protocol error.
    manager
        .client
        .connection()
        .flush()
        .expect("the requests go out");
    focused.roundtrip(&mut compositor);
    assert_eq!(
        focused.state.events,
        [
            Clipboard(None),
            Primary(None),
            Clipboard(Some(0)),
            Clipboard(None)
        ]
    );
    // The manager reads what Yuna sent last: `used_source` (1), on its device.
    if let Some(guard) = manager.client.connection().prepare_read() {
        assert!(guard.read().is_err(), "the manager's connection ends");
    }
    let error = manager
        .client
        .connection()
        .protocol_error()
        .expect("the manager got a protocol error");
    assert_eq!(
        (error.object_interface.as_str(), error.code),
        ("ext_data_control_device_v1", 1)
    );

    let window = 0;
    focused.client.request_frame(window);
    focused.client.commit(window);
    focused.roundtrip(&mut compositor);
    let frames_before = focused.client.window(window).frames.len();
    advance_to(&mut compositor, &mut focused.client, boundary(start, 2));
    assert_eq!(
        focused.client.window(window).frames[frames_before..],
        [millis(boundary(start, 2) - LEAD)]
    );
    settle_and_rest(&mut compositor);
}

/// When the last window closes, no client has the focus, so none may set the selections or is
/// offered them: the client whose window closed is told nothing of a new clipboard, and its own
/// request is ignored. Its next window takes the focus, and is offered the clipboard.
#[test]
fn a_client_whose_window_closed_is_offered_nothing() {
    let mut compositor = manual_compositor();
    let mut client = SelectionClient::connect(&mut compositor);
    let mut manager = SelectionClient::connect_data_control(&mut compositor);
    let start = compositor.now();
    open_with_focus(&mut compositor, &mut client, RED, boundary(start, 1));
    let serial = client.enter_serial();
    assert_eq!(client.state.events, [Clipboard(None), Primary(None)]);

    client.client.close(0);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client.client, boundary(start, 2));
    assert!(
        compositor
            .layout()
            .arrange(OUTPUT)
            .expect("the output")
            .is_empty()
    );

    manager.control_set_clipboard(&[TEXT]);
    manager.roundtrip(&mut compositor);
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.events, [Clipboard(None), Primary(None)]);
    assert!(client.state.offers.is_empty());

    client.set_clipboard(&["text/plain"], serial);
    client.roundtrip(&mut compositor);
    manager.roundtrip(&mut compositor);
    assert_eq!(
        manager.state.events,
        [
            ControlClipboard(None),
            ControlPrimary(None),
            ControlClipboard(Some(0))
        ]
    );

    open_with_focus(&mut compositor, &mut client, BLUE, boundary(start, 3));
    assert_eq!(client.state.enters.len(), 2);
    assert_eq!(
        client.state.events[2..],
        [Clipboard(Some(0)), Primary(None)]
    );
    assert_eq!(client.state.offers[0].mime_types, [TEXT]);
    settle_and_rest(&mut compositor);
}

/// The client with the focus, whose sources hold both selections, leaves. Its window closes, so
/// the focus goes to the other client's window at the next boundary: that client is told both
/// selections are empty, as is the data-control client, and Yuna rests.
#[test]
fn the_selections_empty_when_the_focused_client_leaves() {
    let mut compositor = manual_compositor();
    let mut other = SelectionClient::connect(&mut compositor);
    let mut focused = SelectionClient::connect(&mut compositor);
    let mut manager = SelectionClient::connect_data_control(&mut compositor);
    let start = compositor.now();
    open_with_focus(&mut compositor, &mut other, BLUE, boundary(start, 1));
    open_with_focus(&mut compositor, &mut focused, RED, boundary(start, 2));
    focused.set_clipboard(&[TEXT], focused.enter_serial());
    focused.set_primary(&[TEXT], focused.enter_serial());
    focused.roundtrip(&mut compositor);
    manager.roundtrip(&mut compositor);
    assert_eq!(
        manager.state.events,
        [
            ControlClipboard(None),
            ControlPrimary(None),
            ControlClipboard(Some(0)),
            ControlPrimary(Some(1))
        ]
    );

    drop(focused);
    other.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut other.client, boundary(start, 3));
    other.roundtrip(&mut compositor);
    manager.roundtrip(&mut compositor);
    assert_eq!(other.state.enters.len(), 2);
    // Told at its first focus, then again when the focus came back.
    assert_eq!(
        other.state.events,
        [
            Clipboard(None),
            Primary(None),
            Clipboard(None),
            Primary(None)
        ]
    );
    assert_eq!(
        manager.state.events[4..],
        [ControlClipboard(None), ControlPrimary(None)]
    );
    settle_and_rest(&mut compositor);
}
