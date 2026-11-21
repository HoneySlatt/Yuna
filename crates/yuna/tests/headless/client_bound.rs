//! The clients one seat connects: [`MAX_CLIENTS_PER_SEAT`] at most, its Xwayland apart
//! ([ADR-0147](../../../../docs/adr/0147-a-seat-connects-256-clients-at-most.md)). Past the
//! bound, an in-process connection is an error, and one through the seat's socket file is closed
//! at once while the socket listens on and the seat's other clients are served. A client that
//! left frees its place once Yuna has freed it, at the end of the round in which the display read
//! its end.
//!
//! Each story opens a seat's 257 clients, about three descriptors each, and runs alone, in a
//! process of its own ([`in_its_own_process`](crate::in_its_own_process)): the stories that run beside it in one process
//! share its limit of open files, which the test binary does not raise, and would mix their
//! clients with its own. The clients that fill a seat are raw connections that send nothing: the
//! display serves them as any other.

use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use yuna::{ClockMode, Compositor, Config, Error, MAX_CLIENTS_PER_SEAT};
use yuna_types::SeatId;

use crate::in_its_own_process;
use crate::x11_windows::{Folder, X11Seat, at_1, off};

/// A compositor on the manual clock, without output or socket, and an agent's seat in it.
fn compositor_with_a_seat() -> (Compositor, SeatId) {
    let mut compositor = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts");
    let seat = compositor.add_seat().expect("a seat");
    (compositor, seat)
}

/// One round of the compositor's loop, without waiting.
fn round(compositor: &mut Compositor) {
    compositor
        .dispatch(Some(Duration::ZERO))
        .expect("the compositor dispatches");
}

/// `count` in-process clients of `seat`, which must all connect.
fn fill(compositor: &mut Compositor, seat: SeatId, count: usize) -> Vec<UnixStream> {
    (0..count)
        .map(|n| {
            compositor
                .connect_client_to(seat)
                .unwrap_or_else(|error| panic!("client {n} of {count} connects: {error}"))
        })
        .collect()
}

/// Ends a client's connection, so that the display sees it gone at its next round, even while
/// a process another story forked holds a copy of the descriptor.
fn leave(client: UnixStream) {
    client
        .shutdown(Shutdown::Both)
        .expect("the client's connection ends");
}

/// A connection to the socket file at `path`, accepted, or closed, by the round that follows.
fn connect_through(compositor: &mut Compositor, path: &Path) -> UnixStream {
    let stream = UnixStream::connect(path).expect("the seat's socket takes the connection");
    round(compositor);
    stream.set_nonblocking(true).expect("non-blocking");
    stream
}

/// Whether the peer of `stream` has closed it: a read gives its end of file, not "would block".
fn closed_by_yuna(stream: &mut UnixStream) -> bool {
    let mut byte = [0_u8; 1];
    match stream.read(&mut byte) {
        Ok(0) => true,
        Ok(_) => panic!("Yuna wrote to a connection that asked nothing"),
        Err(error) if error.kind() == ErrorKind::WouldBlock => false,
        Err(error) => panic!("the connection cannot be read: {error}"),
    }
}

/// Sends `wl_display.sync` on a raw connection, its callback's id `callback` the next free id
/// of the connection's, then runs a round and reads what came: whether the answer did, the
/// `wl_callback.done` and `wl_display.delete_id` of 12 bytes each.
fn raw_sync(compositor: &mut Compositor, stream: &mut UnixStream, callback: u32) -> bool {
    let mut request = Vec::with_capacity(12);
    request.extend_from_slice(&1_u32.to_ne_bytes());
    request.extend_from_slice(&(12_u32 << 16).to_ne_bytes());
    request.extend_from_slice(&callback.to_ne_bytes());
    stream.write_all(&request).expect("the request is sent");
    round(compositor);
    let mut answer = [0_u8; 24];
    match stream.read(&mut answer) {
        Ok(24) => answer[..4] == callback.to_ne_bytes(),
        Ok(_) => false,
        Err(error) if error.kind() == ErrorKind::WouldBlock => false,
        Err(error) => panic!("the answer cannot be read: {error}"),
    }
}

/// An in-process client past the bound is refused with an error that names the seat and the
/// bound, through either way to connect one, and nothing is inserted; another seat is not
/// bound by it. Once a client has left and the display has freed it, one more connects.
#[test]
fn a_seats_257th_client_in_process_is_refused_until_one_leaves() {
    if !in_its_own_process(
        "client_bound::a_seats_257th_client_in_process_is_refused_until_one_leaves",
        &[],
    ) {
        return;
    }
    let (mut compositor, seat) = compositor_with_a_seat();
    let mut clients = fill(&mut compositor, seat, MAX_CLIENTS_PER_SEAT);
    let refused = compositor
        .connect_client_to(seat)
        .expect_err("the 257th client is refused");
    let Error::Refused(reason) = &refused else {
        panic!("refused for another reason: {refused}");
    };
    assert!(
        reason.contains(&format!("seat {}", seat.0))
            && reason.contains(&MAX_CLIENTS_PER_SEAT.to_string()),
        "{reason}"
    );
    assert!(matches!(
        compositor.connect_client_seeing_every_seat(seat),
        Err(Error::Refused(_))
    ));
    round(&mut compositor);
    assert_eq!(compositor.client_pids(seat).len(), MAX_CLIENTS_PER_SEAT);
    let _owners = compositor
        .connect_client()
        .expect("the owner's seat has its own bound");

    let gone = clients.pop().expect("a client");
    leave(gone);
    assert!(
        compositor.connect_client_to(seat).is_err(),
        "a place is free before the display read the client's end"
    );
    round(&mut compositor);
    assert_eq!(compositor.client_pids(seat).len(), MAX_CLIENTS_PER_SEAT - 1);
    clients.push(
        compositor
            .connect_client_to(seat)
            .expect("the client that left freed its place"),
    );
    assert!(compositor.connect_client_to(seat).is_err());
}

/// Past the bound, a connection through the seat's socket file is accepted and closed at once:
/// it reads its end of file after one round. The socket listens on: the next is closed too, the
/// seat's first client still gets its answers, and once a client has left, a connection is
/// served again.
#[test]
fn a_seats_socket_closes_its_257th_connection_and_serves_the_others() {
    if !in_its_own_process(
        "client_bound::a_seats_socket_closes_its_257th_connection_and_serves_the_others",
        &[],
    ) {
        return;
    }
    let (mut compositor, seat) = compositor_with_a_seat();
    let folder = Folder::new("client-bound-socket");
    let path = folder.0.join("seat");
    compositor
        .listen_for_seat(seat, path.clone())
        .expect("the seat listens");
    let mut clients: Vec<UnixStream> = (0..MAX_CLIENTS_PER_SEAT)
        .map(|_| connect_through(&mut compositor, &path))
        .collect();
    assert_eq!(compositor.client_pids(seat).len(), MAX_CLIENTS_PER_SEAT);
    assert!(
        !closed_by_yuna(&mut clients[0]),
        "a client within the bound was closed"
    );
    for _ in 0..2 {
        let mut refused = connect_through(&mut compositor, &path);
        assert!(
            closed_by_yuna(&mut refused),
            "a connection past the bound is served"
        );
    }
    assert!(
        raw_sync(&mut compositor, &mut clients[0], 2),
        "the first client is answered"
    );
    assert_eq!(compositor.client_pids(seat).len(), MAX_CLIENTS_PER_SEAT);

    leave(clients.pop().expect("a client"));
    round(&mut compositor);
    let mut served = connect_through(&mut compositor, &path);
    assert!(!closed_by_yuna(&mut served));
    assert!(
        raw_sync(&mut compositor, &mut served, 2),
        "the new client is answered"
    );
}

/// A seat's own Xwayland is never refused, and never counted: on a seat that holds the bound
/// of clients, Xwayland starts for an X11 client and serves it; the seat refuses its next Wayland
/// client still; and once one of them has left, one more connects, the seat then serving its
/// Xwayland beside 256 clients.
#[test]
fn a_seats_xwayland_is_neither_counted_nor_refused() {
    if !in_its_own_process(
        "client_bound::a_seats_xwayland_is_neither_counted_nor_refused",
        &[],
    ) {
        return;
    }
    let mut seat = X11Seat::new("client-bound", &[at_1()], ClockMode::Real, off());
    let id = seat.seat;
    let mut clients = fill(&mut seat.compositor, id, MAX_CLIENTS_PER_SEAT);
    let _x11 = seat.client();
    assert!(
        seat.compositor
            .x11_display(id)
            .and_then(|display| display.server)
            .is_some(),
        "Xwayland runs"
    );
    assert!(seat.compositor.connect_client_to(id).is_err());

    leave(clients.pop().expect("a client"));
    round(&mut seat.compositor);
    clients.push(
        seat.compositor
            .connect_client_to(id)
            .expect("Xwayland does not take a place"),
    );
    assert!(seat.compositor.connect_client_to(id).is_err());
}
