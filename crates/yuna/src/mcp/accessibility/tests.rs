//! What a call is told: names clipped on a character's boundary, plain words when a program is
//! not on its seat's bus, an application's walk cut by the wait not called complete; and a late
//! reply of a walk never taken by the next.

use serde_json::json;

use super::*;
use crate::dbus::Body;
use crate::mcp::protocol::Revision;

/// A call with no walk yet, its key `key`, on seat 2, filtering nothing.
fn tree_call(key: u64) -> TreeCall {
    TreeCall {
        key,
        id: json!(1),
        revision: Revision::Modern,
        seat: SeatId(2),
        window: None,
        pid: None,
        filter: Filter::default(),
        max_nodes: MAX_NODES,
        wait_until: None,
        walk: None,
        last_end: None,
        seen: (0, 0),
        timer: None,
    }
}

/// An application embedded, its process known.
fn app() -> registry::App {
    registry::App {
        bus_name: ":1.4".to_owned(),
        root: "/root".to_owned(),
        pid: Some(42),
        pid_asked: false,
    }
}

/// The error the bus answers a call with once its reply timeout passes.
fn no_reply() -> Message {
    let mut call = Message::method_call(
        ":1.4",
        "/root",
        "org.a11y.atspi.Accessible",
        "GetChildren",
        Body::empty(),
    );
    call.serial = 9;
    Message::error(
        &call,
        "org.freedesktop.DBus.Error.NoReply",
        "no reply in 5 s",
    )
}

/// A walk cut at its deadline leaves calls in flight; the call walks again; the bus's late
/// error for the first walk's root, 5 s after it was sent, must not fail the second walk's
/// application, which answers.
#[test]
fn a_late_reply_of_a_cut_walk_never_reaches_the_next() {
    let mut calls = vec![tree_call(1)];
    let mut first = Walk::new(vec![app()], Filter::default(), 100);
    while first.next_call(0).is_some() {}
    first.cut_at_deadline();
    calls[0].walk = Some(UnderWay {
        serial: 1,
        walk: first,
        started: Instant::now(),
    });
    calls[0].walk = Some(UnderWay {
        serial: 2,
        walk: Walk::new(vec![app()], Filter::default(), 100),
        started: Instant::now(),
    });
    assert!(!deliver(
        &mut calls,
        (1, 1),
        0,
        0,
        Query::Children,
        &no_reply()
    ));
    let second = &calls[0].walk.as_ref().expect("a walk").walk;
    assert!(
        second.apps[0].failed.is_none(),
        "{:?}",
        second.apps[0].failed
    );
    // Its own replies, and none of another call's, are taken.
    assert!(!deliver(
        &mut calls,
        (2, 2),
        0,
        0,
        Query::Children,
        &no_reply()
    ));
    assert!(deliver(
        &mut calls,
        (1, 2),
        0,
        0,
        Query::Children,
        &no_reply()
    ));
    assert!(
        calls[0].walk.as_ref().expect("a walk").walk.apps[0]
            .failed
            .is_some()
    );
}

/// A walk cut by its call's wait's end gives half a tree: its applications are said to be cut,
/// never complete.
#[test]
fn a_walk_cut_by_the_waits_end_is_not_called_complete() {
    let call = tree_call(1);
    let unfinished = Walk::new(vec![app()], Filter::default(), 100);
    let found = call_result(&call, &unfinished, &[], &[]);
    assert_eq!(
        found.value["apps"][0]["status"],
        json!("cut at the wait's end")
    );
    let mut cut = Walk::new(vec![app()], Filter::default(), 100);
    cut.cut_at_wait();
    assert!(cut.is_done());
    let found = call_result(&call, &cut, &[], &[]);
    assert_eq!(
        found.value["apps"][0]["status"],
        json!("cut at the wait's end")
    );
}

#[test]
fn a_long_name_is_clipped_on_a_characters_boundary() {
    assert_eq!(clipped("Run"), "Run");
    let long = "é".repeat(MAX_TEXT + 5);
    let cut = clipped(&long);
    assert_eq!(cut.chars().count(), MAX_TEXT + 1);
    assert!(cut.ends_with('…'));
    assert_eq!(clipped(&"a".repeat(MAX_TEXT)), "a".repeat(MAX_TEXT));
}

#[test]
fn a_program_not_on_the_bus_is_told_plainly() {
    let seat = SeatId(2);
    let without = no_bus_message(seat, false);
    assert!(
        without.contains("seat 2 has no accessibility bus"),
        "{without}"
    );
    assert!(without.contains("accessibility: true"), "{without}");
    assert!(no_bus_message(seat, true).contains("starting"));
    let process = not_on_bus_message(seat, 4242, None);
    assert!(
        process.starts_with("process 4242 is not on seat 2's accessibility bus"),
        "{process}"
    );
    let window = not_on_bus_message(seat, 4242, Some(WindowId(7)));
    assert!(
        window.starts_with("window 7's process, 4242, is not on"),
        "{window}"
    );
}
