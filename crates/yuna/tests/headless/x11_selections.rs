//! The clipboard and the primary selection between a seat's X11 programs and its Wayland
//! programs, both ways, through Yuna's X11 window manager: exact bytes, INCR both ways, an owner
//! that leaves mid-way, a source that never writes, a selection older than the seat's Xwayland,
//! a source that goes, made-up and malformed answers, stale requests, the bound on transfers, and
//! the keyboard focus that lets X11 read Wayland's selection.
//!
//! Each story runs the development shell's Xwayland for an agent's seat, on the real clock: the
//! X11 test client (`x11_client.rs`) on a thread of its own, as an X11 program that copies or
//! pastes, and a Wayland selection client (`selection_client.rs`) of the same seat on the
//! story's thread, between the compositor's rounds. A Wayland source answers on threads of its
//! own, as a program writes into its pipe.

use std::os::fd::OwnedFd;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, PropMode, Property,
    SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Timestamp, Window,
};
use x11rb::wrapper::ConnectionExt as _;
use yuna::yuna_layout::WindowId;
use yuna::{ClockMode, MAX_X11_TRANSFERS, WindowShell, X11_TRANSFER_TIMEOUT};

use crate::selection_client::{Pipe, SelectionClient, SelectionEvent};
use crate::x11_client::{PATIENCE, WindowSpec, X11Client, step, step_pumping};
use crate::x11_windows::{X11Seat, at_1_5, off, open_drawn, pump, wait};

const TEXT: &str = "text/plain;charset=utf-8";

/// The INCR chunks of the X11 test client that copies: smaller than any X11 server's maximum
/// request, as an X11 program's.
const CHUNK: usize = 64 * 1024;

/// One mebibyte: above the maximum request of X11's core protocol, 256 KiB, so INCR both ways.
const MEBIBYTE: usize = 1024 * 1024;

x11rb::atom_manager! {
    /// The atoms the stories name.
    pub(crate) SelectionAtoms: SelectionAtomsCookie {
        CLIPBOARD,
        TARGETS,
        INCR,
        TIMESTAMP,
        UTF8_STRING,
        YUNA_TEST_PASTE,
        YUNA_TEST_TIME,
    }
}

pub(crate) fn atoms_of(client: &X11Client) -> SelectionAtoms {
    SelectionAtoms::new(&client.connection)
        .expect("the atoms are asked")
        .reply()
        .expect("the atoms")
}

/// `length` bytes with no period a chunk could hide in: a chunk lost, doubled or swapped changes
/// them.
fn pattern(length: usize) -> Vec<u8> {
    (0..length as u64)
        .map(|index| {
            let mut x = index + 1;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// Opens a window for `wl` on the seat and waits until it has the seat's keyboard, its client
/// told so.
fn open_wayland_window(seat: &mut X11Seat, wl: &mut SelectionClient) -> WindowId {
    let window = wl.client.open_window(&mut seat.compositor);
    wl.client.ack(window);
    wl.client.attach(window, 400, 300, 0xFF_20_40_60);
    wl.client.commit(window);
    wl.roundtrip(&mut seat.compositor);
    let seat_id = seat.seat;
    let id = wait(
        &mut seat.compositor,
        "the Wayland window has the keyboard",
        |compositor| {
            compositor
                .windows()
                .into_iter()
                .find(|window| window.shell == WindowShell::Xdg && window.focused)
                .map(|window| window.id)
                .filter(|&id| compositor.keyboard_window(seat_id) == Some(id))
        },
    );
    let deadline = Instant::now() + PATIENCE;
    while wl.state.keyboard_on.is_none() {
        assert!(
            Instant::now() < deadline,
            "the Wayland client is never entered"
        );
        wl.roundtrip(&mut seat.compositor);
    }
    id
}

/// Gives the seat's keyboard to `window` and waits until it has it.
fn focus(seat: &mut X11Seat, window: WindowId) {
    seat.compositor
        .change_layout(|layout| layout.focus_window(window))
        .expect("the window takes the focus");
    let seat_id = seat.seat;
    wait(&mut seat.compositor, "the keyboard moves", |compositor| {
        (compositor.keyboard_window(seat_id) == Some(window)).then_some(())
    });
}

/// Opens an X11 window, mapped and drawn, and waits until it has the seat's keyboard.
fn open_x11_window(seat: &mut X11Seat, client: X11Client, title: &str) -> (X11Client, Window) {
    let (client, window, described, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named(title));
    focus(seat, described.id);
    (client, window)
}

/// Makes a window of `client`'s own, never mapped, the owner of `selection`, as an X11 program
/// that copies does; returns the window.
pub(crate) fn take_selection(client: &mut X11Client, selection: Atom) -> Result<Window, String> {
    let owner = client.create(&WindowSpec::named("owner"))?;
    let fail = |error: &dyn std::fmt::Display| error.to_string();
    client
        .connection
        .set_selection_owner(owner, selection, x11rb::CURRENT_TIME)
        .map_err(|e| fail(&e))?;
    let now = client
        .connection
        .get_selection_owner(selection)
        .map_err(|e| fail(&e))?
        .reply()
        .map_err(|e| fail(&e))?
        .owner;
    if now != owner {
        return Err(format!("the selection belongs to {now}, not {owner}"));
    }
    Ok(owner)
}

/// Tells `request`'s requestor its answer is in `property`, or, with none, refused.
fn notify(
    client: &X11Client,
    request: &SelectionRequestEvent,
    property: Atom,
) -> Result<(), String> {
    let event = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: request.time,
        requestor: request.requestor,
        selection: request.selection,
        target: request.target,
        property,
    };
    client
        .connection
        .send_event(false, request.requestor, EventMask::NO_EVENT, event)
        .map_err(|e| e.to_string())?;
    client.connection.flush().map_err(|e| e.to_string())
}

/// Serves `owner`'s selection to the requests that come, as an X11 program that copied does:
/// `TARGETS` lists `UTF8_STRING`, and `UTF8_STRING` is `data`, in INCR chunks of [`CHUNK`] bytes
/// when it is longer (ICCCM 2.7.2); other targets are refused. Returns once `data` went whole,
/// or, with `leave_after_first_chunk`, once its first chunk did.
pub(crate) fn serve(
    client: &mut X11Client,
    owner: Window,
    data: &[u8],
    leave_after_first_chunk: bool,
) -> Result<(), String> {
    let atoms = atoms_of(client);
    let fail = |error: &dyn std::fmt::Display| error.to_string();
    loop {
        let event = client.wait_for(
            "a selection request",
            |event| matches!(event, Event::SelectionRequest(request) if request.owner == owner),
        )?;
        let Event::SelectionRequest(request) = event else {
            unreachable!("a selection request");
        };
        let property = if request.property == x11rb::NONE {
            request.target
        } else {
            request.property
        };
        let requestor = request.requestor;
        if request.target == atoms.TARGETS {
            client
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    requestor,
                    property,
                    AtomEnum::ATOM,
                    &[atoms.TARGETS, atoms.UTF8_STRING],
                )
                .map_err(|e| fail(&e))?;
            notify(client, &request, property)?;
            continue;
        }
        if request.target != atoms.UTF8_STRING {
            notify(client, &request, x11rb::NONE)?;
            continue;
        }
        if data.len() <= CHUNK {
            client
                .connection
                .change_property8(
                    PropMode::REPLACE,
                    requestor,
                    property,
                    atoms.UTF8_STRING,
                    data,
                )
                .map_err(|e| fail(&e))?;
            return notify(client, &request, property);
        }
        client
            .connection
            .change_window_attributes(
                requestor,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .map_err(|e| fail(&e))?;
        let length = u32::try_from(data.len()).map_err(|e| fail(&e))?;
        client
            .connection
            .change_property32(
                PropMode::REPLACE,
                requestor,
                property,
                atoms.INCR,
                &[length],
            )
            .map_err(|e| fail(&e))?;
        notify(client, &request, property)?;
        // Each chunk once the requestor deleted the property, then an empty one, the end.
        for piece in data.chunks(CHUNK).chain([&[][..]]) {
            client.wait_for("the requestor's deletion", |event| {
                matches!(event, Event::PropertyNotify(deleted)
                    if deleted.window == requestor
                        && deleted.atom == property
                        && deleted.state == Property::DELETE)
            })?;
            client
                .connection
                .change_property8(
                    PropMode::REPLACE,
                    requestor,
                    property,
                    atoms.UTF8_STRING,
                    piece,
                )
                .map_err(|e| fail(&e))?;
            client.connection.flush().map_err(|e| fail(&e))?;
            if leave_after_first_chunk {
                return Ok(());
            }
        }
        return Ok(());
    }
}

/// What a paste got: the type its owner gave and the bytes, or nothing when it was refused.
pub(crate) type Pasted = Option<(Atom, Vec<u8>)>;

/// Converts `selection` to `target` onto `window`, as an X11 program that pastes does, and reads
/// the answer whole, INCR included.
pub(crate) fn paste(
    client: &mut X11Client,
    window: Window,
    selection: Atom,
    target: Atom,
) -> Result<Pasted, String> {
    paste_at(client, window, selection, target, x11rb::CURRENT_TIME)
}

/// [`paste`], the request naming `time`, as ICCCM asks of a program: the time of the event that
/// made it paste.
fn paste_at(
    client: &mut X11Client,
    window: Window,
    selection: Atom,
    target: Atom,
    time: Timestamp,
) -> Result<Pasted, String> {
    let atoms = atoms_of(client);
    let property = atoms.YUNA_TEST_PASTE;
    let fail = |error: &dyn std::fmt::Display| error.to_string();
    client
        .connection
        .convert_selection(window, selection, target, property, time)
        .map_err(|e| fail(&e))?;
    client.connection.flush().map_err(|e| fail(&e))?;
    let answer = client.wait_for(
        "the selection's answer",
        |event| matches!(event, Event::SelectionNotify(answer) if answer.requestor == window),
    )?;
    let Event::SelectionNotify(answer) = answer else {
        unreachable!("a selection notification");
    };
    if answer.property == x11rb::NONE {
        return Ok(None);
    }
    let read = |client: &X11Client| {
        client
            .connection
            .get_property(true, window, property, AtomEnum::ANY, 0, u32::MAX / 4)
            .map_err(|e| fail(&e))?
            .reply()
            .map_err(|e| fail(&e))
    };
    let first = read(client)?;
    if first.type_ != atoms.INCR {
        return Ok(Some((first.type_, first.value)));
    }
    let mut data = Vec::new();
    loop {
        client.wait_for("the next chunk", |event| {
            matches!(event, Event::PropertyNotify(changed)
                if changed.window == window
                    && changed.atom == property
                    && changed.state == Property::NEW_VALUE)
        })?;
        let chunk = read(client)?;
        // A notification of the INCR property itself, read and deleted already.
        if chunk.type_ == x11rb::NONE {
            continue;
        }
        if chunk.value.is_empty() {
            return Ok(Some((chunk.type_, data)));
        }
        data.extend_from_slice(&chunk.value);
    }
}

/// A Wayland paste in progress: the pipe given with `receive`, and what came through it.
#[derive(Default)]
struct WaylandPaste {
    pipe: Option<Pipe>,
    data: Vec<u8>,
    /// When the pipe ended: every writer closed it.
    ended: Option<Instant>,
}

impl WaylandPaste {
    /// Reads `wl`'s events; once the last selection event of the kind `of` picks offers
    /// `mime_type`, asks for it; reads what came.
    fn advance(
        &mut self,
        wl: &mut SelectionClient,
        mime_type: &str,
        of: impl Fn(&SelectionEvent) -> Option<Option<usize>>,
    ) {
        wl.read_events();
        if self.pipe.is_none()
            && let Some(offer) = last_offer(wl, &of)
            && wl.state.offers[offer]
                .mime_types
                .iter()
                .any(|offered| offered == mime_type)
        {
            self.pipe = Some(wl.receive(offer, mime_type));
            wl.client.send();
        }
        if self.ended.is_none()
            && let Some(pipe) = &mut self.pipe
            && pipe.read_available(&mut self.data)
        {
            self.ended = Some(Instant::now());
        }
    }
}

/// What a clipboard event offers, an offer or nothing; none for another event.
fn clipboard(event: &SelectionEvent) -> Option<Option<usize>> {
    match *event {
        SelectionEvent::Clipboard(offer) => Some(offer),
        _ => None,
    }
}

/// What a primary selection event offers, an offer or nothing; none for another event.
fn primary(event: &SelectionEvent) -> Option<Option<usize>> {
    match *event {
        SelectionEvent::Primary(offer) => Some(offer),
        _ => None,
    }
}

/// The offer of `wl`'s last selection event of the kind `of` picks: none when it offered
/// nothing, an older offer being replaced.
fn last_offer(
    wl: &SelectionClient,
    of: impl Fn(&SelectionEvent) -> Option<Option<usize>>,
) -> Option<usize> {
    wl.state.events.iter().rev().find_map(of).flatten()
}

/// An X11 program copies `data` into `selection`; the Wayland client, which has the keyboard,
/// is offered it as text and pastes it whole. Returns what it read.
fn x11_copies_wayland_pastes(
    seat: &mut X11Seat,
    wl: &mut SelectionClient,
    client: X11Client,
    selection: impl Fn(&SelectionAtoms) -> Atom + Send + 'static,
    of: impl Fn(&SelectionEvent) -> Option<Option<usize>> + Copy,
    data: Vec<u8>,
) -> (X11Client, Vec<u8>) {
    let mut paste = WaylandPaste::default();
    let (client, ()) = step_pumping(
        &mut seat.compositor,
        client,
        |compositor| {
            pump(compositor);
            paste.advance(wl, TEXT, of);
        },
        move |client| {
            let selection = selection(&atoms_of(client));
            let owner = take_selection(client, selection)?;
            serve(client, owner, &data, false)
        },
    );
    let compositor = &mut seat.compositor;
    wait(compositor, "the Wayland paste ends", |compositor| {
        pump(compositor);
        paste.advance(wl, TEXT, of);
        paste.ended.map(|_| ())
    });
    (client, paste.data)
}

/// The X11 window `window` pastes `selection` as `UTF8_STRING`, while the Wayland client's
/// source `source` writes `data` into each pipe it is sent, on threads of their own.
fn x11_pastes(
    seat: &mut X11Seat,
    wl: &mut SelectionClient,
    client: X11Client,
    window: Window,
    selection: impl Fn(&SelectionAtoms) -> Atom + Send + 'static,
    source: usize,
    data: &[u8],
) -> (X11Client, Pasted) {
    let paste = X11Paste {
        window,
        time: x11rb::CURRENT_TIME,
    };
    x11_pastes_at(seat, wl, client, paste, selection, source, data)
}

/// Where and when an X11 program pastes: onto `window`, its request naming `time`.
#[derive(Debug, Clone, Copy)]
struct X11Paste {
    window: Window,
    time: Timestamp,
}

/// [`x11_pastes`], at `paste`'s time.
fn x11_pastes_at(
    seat: &mut X11Seat,
    wl: &mut SelectionClient,
    client: X11Client,
    paste: X11Paste,
    selection: impl Fn(&SelectionAtoms) -> Atom + Send + 'static,
    source: usize,
    data: &[u8],
) -> (X11Client, Pasted) {
    let mut writers: Vec<JoinHandle<()>> = Vec::new();
    let answer = |fd: OwnedFd, data: Vec<u8>| {
        thread::spawn(move || {
            use std::io::Write;
            // A reader that gave up closes its end: the source's write fails, as a program's.
            let _ = std::fs::File::from(fd).write_all(&data);
        })
    };
    let result = step_pumping(
        &mut seat.compositor,
        client,
        |compositor| {
            pump(compositor);
            wl.read_events();
            for fd in wl.take_sent(source) {
                writers.push(answer(fd, data.to_vec()));
            }
        },
        move |client| {
            let atoms = atoms_of(client);
            let (window, time) = (paste.window, paste.time);
            paste_at(client, window, selection(&atoms), atoms.UTF8_STRING, time)
        },
    );
    for writer in writers {
        writer.join().expect("the source's writer ends");
    }
    result
}

#[test]
fn an_x11_copy_is_pasted_by_a_wayland_client_as_utf8_text() {
    let mut seat = X11Seat::new("sel-x11-to-wl", &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let client = seat.client();
    let text = "Yuna, ユナ".as_bytes().to_vec();
    let (_client, pasted) = x11_copies_wayland_pastes(
        &mut seat,
        &mut wl,
        client,
        |atoms| atoms.CLIPBOARD,
        clipboard,
        text.clone(),
    );
    assert_eq!(pasted, text);
    let offer = last_offer(&wl, clipboard);
    let offered = &wl.state.offers[offer.expect("an offer")].mime_types;
    assert_eq!(
        offered,
        &[TEXT],
        "UTF8_STRING is offered as UTF-8 text alone"
    );
}

#[test]
fn a_wayland_copy_is_pasted_by_an_x11_window_as_utf8_string() {
    let mut seat = X11Seat::new("sel-wl-to-x11", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, window) = open_x11_window(&mut seat, client, "paster");
    let x11_id = seat
        .compositor
        .keyboard_window(seat.seat)
        .expect("the X11 window");
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(&mut seat.compositor);
    focus(&mut seat, x11_id);

    let text = "Yuna, ユナ".as_bytes();
    let (client, pasted) = x11_pastes(
        &mut seat,
        &mut wl,
        client,
        window,
        |atoms| atoms.CLIPBOARD,
        source,
        text,
    );
    let utf8_string = atoms_of(&client).UTF8_STRING;
    assert_eq!(pasted, Some((utf8_string, text.to_vec())));
    assert_eq!(wl.source(source).asked, [TEXT]);
}

#[test]
fn a_mebibyte_goes_from_x11_to_wayland_through_incr() {
    let mut seat = X11Seat::new("sel-x11-to-wl-mib", &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let client = seat.client();
    let data = pattern(MEBIBYTE);
    let (_client, pasted) = x11_copies_wayland_pastes(
        &mut seat,
        &mut wl,
        client,
        |atoms| atoms.CLIPBOARD,
        clipboard,
        data.clone(),
    );
    assert_eq!(pasted.len(), data.len());
    assert!(pasted == data, "the bytes come whole and in order");
}

#[test]
fn a_mebibyte_goes_from_wayland_to_x11_through_incr() {
    let mut seat = X11Seat::new("sel-wl-to-x11-mib", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, window) = open_x11_window(&mut seat, client, "paster");
    let x11_id = seat
        .compositor
        .keyboard_window(seat.seat)
        .expect("the X11 window");
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(&mut seat.compositor);
    focus(&mut seat, x11_id);

    let data = pattern(MEBIBYTE);
    let (_client, pasted) = x11_pastes(
        &mut seat,
        &mut wl,
        client,
        window,
        |atoms| atoms.CLIPBOARD,
        source,
        &data,
    );
    let (_, pasted) = pasted.expect("the paste is answered");
    assert_eq!(pasted.len(), data.len());
    assert!(pasted == data, "the bytes come whole and in order");
}

#[test]
fn the_primary_selection_goes_from_x11_to_wayland() {
    let mut seat = X11Seat::new("sel-primary", &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let client = seat.client();
    let text = b"selected, not copied".to_vec();
    let (_client, pasted) = x11_copies_wayland_pastes(
        &mut seat,
        &mut wl,
        client,
        |_| AtomEnum::PRIMARY.into(),
        primary,
        text.clone(),
    );
    assert_eq!(pasted, text);
    assert!(
        !wl.state
            .events
            .iter()
            .any(|event| matches!(clipboard(event), Some(Some(_)))),
        "the clipboard is not touched"
    );
}

/// An X11 owner sends the first chunk of a mebibyte, then its program exits: the Wayland paste
/// ends at once, well before the transfers' time limit, with the bytes that came; then another
/// X11 program copies, and the Wayland client pastes it whole.
#[test]
fn an_x11_owner_that_exits_mid_transfer_ends_the_wayland_paste() {
    let mut seat = X11Seat::new("sel-owner-exits", &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let client = seat.client();
    let data = pattern(MEBIBYTE);
    let mut paste = WaylandPaste::default();
    let serving = data.clone();
    let (client, ()) = step_pumping(
        &mut seat.compositor,
        client,
        |compositor| {
            pump(compositor);
            paste.advance(&mut wl, TEXT, clipboard);
        },
        move |client| {
            let selection = atoms_of(client).CLIPBOARD;
            let owner = take_selection(client, selection)?;
            serve(client, owner, &serving, true)
        },
    );
    drop(client);
    let left = Instant::now();
    wait(
        &mut seat.compositor,
        "the Wayland paste ends",
        |compositor| {
            pump(compositor);
            paste.advance(&mut wl, TEXT, clipboard);
            paste.ended.map(|_| ())
        },
    );
    let took = paste.ended.expect("ended").duration_since(left);
    assert!(
        took < X11_TRANSFER_TIMEOUT,
        "the owner's exit ends the paste, not the time limit: {took:?}"
    );
    assert!(paste.data.len() < data.len(), "the paste is cut short");
    assert!(
        data.starts_with(&paste.data),
        "what came is the start of the data"
    );

    let again = seat.client();
    let text = b"again".to_vec();
    let (_again, pasted) = x11_copies_wayland_pastes(
        &mut seat,
        &mut wl,
        again,
        |atoms| atoms.CLIPBOARD,
        clipboard,
        text.clone(),
    );
    assert_eq!(pasted, text);
}

/// A Wayland source is sent the pipe and never writes into it, nor closes it: the X11 paste is
/// refused once the transfer made no progress for the time limit, not before, and not long
/// after.
#[test]
fn a_wayland_source_that_never_writes_is_refused_within_the_time_limit() {
    let mut seat = X11Seat::new("sel-never-writes", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, window) = open_x11_window(&mut seat, client, "paster");
    let x11_id = seat
        .compositor
        .keyboard_window(seat.seat)
        .expect("the X11 window");
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(&mut seat.compositor);
    focus(&mut seat, x11_id);

    let mut held: Vec<OwnedFd> = Vec::new();
    let asked = Instant::now();
    let (_client, (pasted, took)) = step_pumping(
        &mut seat.compositor,
        client,
        |compositor| {
            pump(compositor);
            wl.read_events();
            held.extend(wl.take_sent(source));
        },
        move |client| {
            let atoms = atoms_of(client);
            let pasted = paste(client, window, atoms.CLIPBOARD, atoms.UTF8_STRING)?;
            Ok((pasted, asked.elapsed()))
        },
    );
    assert_eq!(held.len(), 1, "the source was sent the pipe, and holds it");
    assert_eq!(pasted, None, "refused");
    assert!(
        took >= X11_TRANSFER_TIMEOUT,
        "not before the time limit: {took:?}"
    );
    assert!(
        took < X11_TRANSFER_TIMEOUT + Duration::from_secs(3),
        "within the time limit: {took:?}"
    );
}

/// A Wayland client copies while the seat has no Xwayland; the first X11 program starts one,
/// opens a window, and pastes the copy.
#[test]
fn a_wayland_copy_older_than_the_seats_xwayland_is_pasted_in_x11() {
    let mut seat = X11Seat::new("sel-before-x11", &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(&mut seat.compositor);
    let display = seat
        .compositor
        .x11_display(seat.seat)
        .expect("the seat's display");
    assert_eq!(display.server, None, "no Xwayland yet");

    let client = seat.client();
    let (client, window) = open_x11_window(&mut seat, client, "late");
    let text = b"copied before X11".as_slice();
    let (client, pasted) = x11_pastes(
        &mut seat,
        &mut wl,
        client,
        window,
        |atoms| atoms.CLIPBOARD,
        source,
        text,
    );
    let utf8_string = atoms_of(&client).UTF8_STRING;
    assert_eq!(pasted, Some((utf8_string, text.to_vec())));
}

/// While a Wayland window has the seat's keyboard, X11 reads nothing of the Wayland client's
/// selection, its targets included; once one of the seat's X11 windows has it, it reads it. An
/// X11 copy reaches the Wayland client with the keyboard whatever has it (the first story).
#[test]
fn x11_reads_the_wayland_selection_only_while_an_x11_window_has_the_keyboard() {
    let mut seat = X11Seat::new("sel-focus-rule", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, window) = open_x11_window(&mut seat, client, "paster");
    let x11_id = seat
        .compositor
        .keyboard_window(seat.seat)
        .expect("the X11 window");
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(&mut seat.compositor);

    let text = b"for the focused alone".as_slice();
    let (client, refused) = step(&mut seat.compositor, client, move |client| {
        let atoms = atoms_of(client);
        let targets = paste(client, window, atoms.CLIPBOARD, atoms.TARGETS)?;
        let data = paste(client, window, atoms.CLIPBOARD, atoms.UTF8_STRING)?;
        Ok((targets, data))
    });
    assert_eq!(
        refused,
        (None, None),
        "nothing while Wayland has the keyboard"
    );
    wl.read_events();
    assert!(
        wl.source(source).asked.is_empty(),
        "the source is never asked"
    );

    focus(&mut seat, x11_id);
    let (client, pasted) = x11_pastes(
        &mut seat,
        &mut wl,
        client,
        window,
        |atoms| atoms.CLIPBOARD,
        source,
        text,
    );
    let utf8_string = atoms_of(&client).UTF8_STRING;
    assert_eq!(pasted, Some((utf8_string, text.to_vec())));
}

/// Waits for the next request for `owner`'s selection, among the events that came and come.
fn next_request(client: &mut X11Client, owner: Window) -> Result<SelectionRequestEvent, String> {
    match client.wait_for(
        "a selection request",
        |event| matches!(event, Event::SelectionRequest(request) if request.owner == owner),
    )? {
        Event::SelectionRequest(request) => Ok(request),
        _ => unreachable!("a selection request"),
    }
}

/// An X11 owner answers wrongly: its `TARGETS` in bytes, not atoms, then, taken again, a paste
/// with a property it never set. Each ends alone: the Wayland client is offered nothing for the
/// first, its paste of the second ends empty, and the owner's right answer that follows is
/// pasted whole.
#[test]
fn an_x11_owners_malformed_answers_end_their_transfers_alone() {
    let mut seat = X11Seat::new("sel-malformed", &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let client = seat.client();
    let offers_before = wl.state.offers.len();
    let (client, owner) = step(&mut seat.compositor, client, |client| {
        let atoms = atoms_of(client);
        let owner = take_selection(client, atoms.CLIPBOARD)?;
        let request = next_request(client, owner)?;
        if request.target != atoms.TARGETS {
            return Err("TARGETS is asked first".to_owned());
        }
        client
            .connection
            .change_property8(
                PropMode::REPLACE,
                request.requestor,
                request.property,
                AtomEnum::STRING,
                b"no atoms here",
            )
            .map_err(|e| e.to_string())?;
        notify(client, &request, request.property)?;
        Ok(owner)
    });
    // Its targets read as none: the selection empties, and nothing is offered.
    let settled = Instant::now() + Duration::from_millis(300);
    while Instant::now() < settled {
        pump(&mut seat.compositor);
        wl.read_events();
    }
    assert_eq!(wl.state.offers.len(), offers_before, "nothing offered");
    assert_eq!(last_offer(&wl, clipboard), None);

    let text = b"right at last".to_vec();
    let mut first = WaylandPaste::default();
    let (client, ()) = step_pumping(
        &mut seat.compositor,
        client,
        |compositor| {
            pump(compositor);
            first.advance(&mut wl, TEXT, clipboard);
        },
        move |client| {
            let atoms = atoms_of(client);
            client
                .connection
                .set_selection_owner(owner, atoms.CLIPBOARD, x11rb::CURRENT_TIME)
                .map_err(|e| e.to_string())?;
            client.connection.flush().map_err(|e| e.to_string())?;
            let targets = next_request(client, owner)?;
            client
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    targets.requestor,
                    targets.property,
                    AtomEnum::ATOM,
                    &[atoms.TARGETS, atoms.UTF8_STRING],
                )
                .map_err(|e| e.to_string())?;
            notify(client, &targets, targets.property)?;
            // Told the answer is in a property never set.
            let data = next_request(client, owner)?;
            notify(client, &data, data.property)
        },
    );
    wait(&mut seat.compositor, "the first paste ends", |compositor| {
        pump(compositor);
        first.advance(&mut wl, TEXT, clipboard);
        first.ended.map(|_| ())
    });
    assert_eq!(
        first.data, b"",
        "the malformed answer ends its paste, empty"
    );

    // The same offer, read again: the owner answers right.
    let mut second = WaylandPaste::default();
    let (_client, ()) = step_pumping(
        &mut seat.compositor,
        client,
        |compositor| {
            pump(compositor);
            second.advance(&mut wl, TEXT, clipboard);
        },
        move |client| serve(client, owner, &text, false),
    );
    wait(
        &mut seat.compositor,
        "the second paste ends",
        |compositor| {
            pump(compositor);
            second.advance(&mut wl, TEXT, clipboard);
            second.ended.map(|_| ())
        },
    );
    assert_eq!(second.data, b"right at last");
}

/// A Wayland client asks an X11 selection nine times at once, its owner answering none: the
/// ninth pipe ends at once, refused past the eight transfers that run at once; the eight end at
/// the time limit, and Yuna serves the next paste.
#[test]
fn reads_past_the_transfers_at_once_are_refused_and_the_others_end_in_time() {
    let mut seat = X11Seat::new("sel-transfer-cap", &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let client = seat.client();
    let (client, owner) = step_pumping(
        &mut seat.compositor,
        client,
        |compositor| {
            pump(compositor);
        },
        |client| {
            let atoms = atoms_of(client);
            let owner = take_selection(client, atoms.CLIPBOARD)?;
            let targets = next_request(client, owner)?;
            client
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    targets.requestor,
                    targets.property,
                    AtomEnum::ATOM,
                    &[atoms.TARGETS, atoms.UTF8_STRING],
                )
                .map_err(|e| e.to_string())?;
            notify(client, &targets, targets.property)?;
            Ok(owner)
        },
    );
    let offer = wait(
        &mut seat.compositor,
        "the X11 selection is offered",
        |compositor| {
            pump(compositor);
            wl.read_events();
            last_offer(&wl, clipboard)
        },
    );
    assert_eq!(MAX_X11_TRANSFERS, 8);
    let mut pipes: Vec<Pipe> = (0..=MAX_X11_TRANSFERS)
        .map(|_| wl.receive(offer, TEXT))
        .collect();
    wl.client.send();
    let asked = Instant::now();
    let mut data = Vec::new();
    let mut ended: Vec<Option<Duration>> = vec![None; pipes.len()];
    let deadline = asked + X11_TRANSFER_TIMEOUT + Duration::from_secs(3);
    while ended.iter().any(Option::is_none) {
        assert!(Instant::now() < deadline, "a read never ended: {ended:?}");
        pump(&mut seat.compositor);
        for (pipe, end) in pipes.iter_mut().zip(&mut ended) {
            if end.is_none() && pipe.read_available(&mut data) {
                *end = Some(asked.elapsed());
            }
        }
    }
    assert!(data.is_empty(), "no read got data");
    let ninth = ended[MAX_X11_TRANSFERS].expect("ended");
    assert!(
        ninth < Duration::from_secs(1),
        "the ninth is refused at once: {ninth:?}"
    );
    for end in &ended[..MAX_X11_TRANSFERS] {
        let end = end.expect("ended");
        assert!(
            end >= X11_TRANSFER_TIMEOUT,
            "not before the time limit: {end:?}"
        );
    }

    // The eight requests it never answered are left, as a program that gave up on them.
    let (client, _) = step(&mut seat.compositor, client, |client| client.take_events());
    let text = b"served again".to_vec();
    let mut paste = WaylandPaste::default();
    let (_client, ()) = step_pumping(
        &mut seat.compositor,
        client,
        |compositor| {
            pump(compositor);
            paste.advance(&mut wl, TEXT, clipboard);
        },
        move |client| serve(client, owner, &text, false),
    );
    wait(&mut seat.compositor, "the next paste ends", |compositor| {
        pump(compositor);
        paste.advance(&mut wl, TEXT, clipboard);
        paste.ended.map(|_| ())
    });
    assert_eq!(paste.data, b"served again");
}

/// The owner of `selection` now, as X11 says: none is `NONE`.
fn owner_of(client: &X11Client, selection: Atom) -> Result<Window, String> {
    Ok(client
        .connection
        .get_selection_owner(selection)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .owner)
}

/// Waits until `selection` is owned, or not, as `owned` says.
fn owner_becomes(client: &X11Client, selection: Atom, owned: bool) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    while (owner_of(client, selection)? != x11rb::NONE) != owned {
        if Instant::now() > deadline {
            return Err(format!(
                "the selection's owner: owned {owned} not within {PATIENCE:?}"
            ));
        }
        thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

/// A Wayland copy's source destroyed by its client, then a Wayland client that leaves holding
/// both selections: each time the X11 selections Yuna held for them are released, and X11
/// programs see no owner.
#[test]
fn a_wayland_source_destroyed_or_its_client_gone_releases_the_x11_selection() {
    let mut seat = X11Seat::new("sel-source-gone", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let copied = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(&mut seat.compositor);
    let (client, ()) = step(&mut seat.compositor, client, |client| {
        owner_becomes(client, atoms_of(client).CLIPBOARD, true)
    });

    wl.destroy_source(copied);
    wl.roundtrip(&mut seat.compositor);
    let (client, ()) = step(&mut seat.compositor, client, |client| {
        owner_becomes(client, atoms_of(client).CLIPBOARD, false)
    });

    wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.set_primary(&[TEXT], wl.enter_serial());
    wl.roundtrip(&mut seat.compositor);
    let (client, ()) = step(&mut seat.compositor, client, |client| {
        owner_becomes(client, atoms_of(client).CLIPBOARD, true)?;
        owner_becomes(client, AtomEnum::PRIMARY.into(), true)
    });
    drop(wl);
    let (_client, ()) = step(&mut seat.compositor, client, |client| {
        owner_becomes(client, atoms_of(client).CLIPBOARD, false)?;
        owner_becomes(client, AtomEnum::PRIMARY.into(), false)
    });
}

/// A time of X11's server now, as ICCCM has a program get one: a property of `window` appended
/// nothing, its notification's time.
fn server_time(client: &mut X11Client, window: Window) -> Result<Timestamp, String> {
    let atom = atoms_of(client).YUNA_TEST_TIME;
    client
        .connection
        .change_property8(PropMode::APPEND, window, atom, AtomEnum::STRING, &[])
        .map_err(|e| e.to_string())?;
    client.connection.flush().map_err(|e| e.to_string())?;
    match client.wait_for("the property's notification", |event| {
        matches!(event, Event::PropertyNotify(changed)
            if changed.window == window && changed.atom == atom)
    })? {
        Event::PropertyNotify(changed) => Ok(changed.time),
        _ => unreachable!("a property notification"),
    }
}

/// ICCCM 2.2: a request named with a time before Yuna took the selection is refused, the source
/// never asked; one at `CurrentTime`, or at a later time, is answered.
#[test]
fn a_request_older_than_yunas_taking_of_the_selection_is_refused() {
    let mut seat = X11Seat::new("sel-stale-time", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, window) = open_x11_window(&mut seat, client, "paster");
    let x11_id = seat
        .compositor
        .keyboard_window(seat.seat)
        .expect("the X11 window");
    let (client, before) = step(&mut seat.compositor, client, move |client| {
        server_time(client, window)
    });
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(&mut seat.compositor);
    focus(&mut seat, x11_id);
    // Yuna's taking known: its TIMESTAMP is a time, not CurrentTime.
    let (client, taken) = step(&mut seat.compositor, client, move |client| {
        let atoms = atoms_of(client);
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some((_, bytes)) = paste(client, window, atoms.CLIPBOARD, atoms.TIMESTAMP)?
                && let Ok(bytes) = <[u8; 4]>::try_from(bytes.as_slice())
                && u32::from_ne_bytes(bytes) != x11rb::CURRENT_TIME
            {
                return Ok(u32::from_ne_bytes(bytes));
            }
            if Instant::now() > deadline {
                return Err("Yuna's TIMESTAMP never names a time".to_owned());
            }
            thread::sleep(Duration::from_millis(2));
        }
    });
    assert!(
        before < taken,
        "the old time is before the taking: {before} {taken}"
    );

    let text = b"pasted at its time".as_slice();
    let stale = X11Paste {
        window,
        time: before,
    };
    let (client, refused) = x11_pastes_at(
        &mut seat,
        &mut wl,
        client,
        stale,
        |a| a.CLIPBOARD,
        source,
        text,
    );
    assert_eq!(refused, None, "a request older than the taking is refused");
    assert!(
        wl.source(source).asked.is_empty(),
        "the source is never asked"
    );

    let (mut client, pasted) = x11_pastes(
        &mut seat,
        &mut wl,
        client,
        window,
        |a| a.CLIPBOARD,
        source,
        text,
    );
    let utf8_string = atoms_of(&client).UTF8_STRING;
    assert_eq!(pasted, Some((utf8_string, text.to_vec())), "CurrentTime");
    let later = server_time(&mut client, window).expect("a time");
    let fresh = X11Paste {
        window,
        time: later,
    };
    let (_client, pasted) = x11_pastes_at(
        &mut seat,
        &mut wl,
        client,
        fresh,
        |a| a.CLIPBOARD,
        source,
        text,
    );
    assert_eq!(
        pasted,
        Some((utf8_string, text.to_vec())),
        "a time after the taking"
    );
}

/// An X11 client makes up answers to the transfer window Yuna's paste waits on: a refusal
/// naming another selection, and one naming another target. Both are left; the owner's own
/// answer is pasted whole.
#[test]
fn made_up_answers_naming_another_selection_or_target_are_left() {
    let mut seat = X11Seat::new("sel-made-up", &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let client = seat.client();
    let text = b"the owner's own".to_vec();
    let mut paste = WaylandPaste::default();
    let served = text.clone();
    let (_client, ()) = step_pumping(
        &mut seat.compositor,
        client,
        |compositor| {
            pump(compositor);
            paste.advance(&mut wl, TEXT, clipboard);
        },
        move |client| {
            let atoms = atoms_of(client);
            let owner = take_selection(client, atoms.CLIPBOARD)?;
            let targets = next_request(client, owner)?;
            client
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    targets.requestor,
                    targets.property,
                    AtomEnum::ATOM,
                    &[atoms.TARGETS, atoms.UTF8_STRING],
                )
                .map_err(|e| e.to_string())?;
            notify(client, &targets, targets.property)?;
            let data = next_request(client, owner)?;
            let other_selection = SelectionRequestEvent {
                selection: AtomEnum::PRIMARY.into(),
                ..data
            };
            notify(client, &other_selection, x11rb::NONE)?;
            let other_target = SelectionRequestEvent {
                target: atoms.TARGETS,
                ..data
            };
            notify(client, &other_target, x11rb::NONE)?;
            client
                .connection
                .change_property8(
                    PropMode::REPLACE,
                    data.requestor,
                    data.property,
                    atoms.UTF8_STRING,
                    &served,
                )
                .map_err(|e| e.to_string())?;
            notify(client, &data, data.property)
        },
    );
    wait(&mut seat.compositor, "the paste ends", |compositor| {
        pump(compositor);
        paste.advance(&mut wl, TEXT, clipboard);
        paste.ended.map(|_| ())
    });
    assert_eq!(paste.data, text);
}

/// An X11 window reads the Wayland selection while it has the keyboard; once the keyboard is
/// back on the Wayland window, X11 reads nothing more, the source asked once.
#[test]
fn x11_reads_nothing_once_the_keyboard_is_back_on_wayland() {
    let mut seat = X11Seat::new("sel-focus-back", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, window) = open_x11_window(&mut seat, client, "paster");
    let x11_id = seat
        .compositor
        .keyboard_window(seat.seat)
        .expect("the X11 window");
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    let wayland_id = open_wayland_window(&mut seat, &mut wl);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(&mut seat.compositor);
    focus(&mut seat, x11_id);
    let text = b"while it had the keyboard".as_slice();
    let (client, pasted) = x11_pastes(
        &mut seat,
        &mut wl,
        client,
        window,
        |a| a.CLIPBOARD,
        source,
        text,
    );
    assert!(pasted.is_some(), "read with the keyboard");

    focus(&mut seat, wayland_id);
    let (_client, refused) = x11_pastes(
        &mut seat,
        &mut wl,
        client,
        window,
        |a| a.CLIPBOARD,
        source,
        text,
    );
    assert_eq!(refused, None, "nothing once Wayland has the keyboard");
    assert_eq!(wl.source(source).asked, [TEXT], "the source asked once");
}

/// The X11 window that has the keyboard is unmapped, and later another destroyed: each time the
/// keyboard goes to the Wayland window, no focus command following, and X11 reads nothing more,
/// whichever of its windows asks.
#[test]
fn an_x11_window_with_the_keyboard_unmapped_or_destroyed_reads_nothing_more() {
    let mut seat = X11Seat::new("sel-focus-gone", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (mut client, first) = open_x11_window(&mut seat, client, "first");
    let first_id = seat
        .compositor
        .keyboard_window(seat.seat)
        .expect("the X11 window");
    // A window of the program's own that never maps, which asks once the others are gone.
    let asker = client
        .create(&WindowSpec::named("asker"))
        .expect("a window");
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    let wayland_id = open_wayland_window(&mut seat, &mut wl);
    let source = wl.set_clipboard(&[TEXT], wl.enter_serial());
    wl.roundtrip(&mut seat.compositor);
    let text = b"for the window with the keyboard".as_slice();
    let seat_id = seat.seat;
    let keyboard_back_on_wayland = |seat: &mut X11Seat| {
        wait(
            &mut seat.compositor,
            "the keyboard is back on Wayland",
            |compositor| (compositor.keyboard_window(seat_id) == Some(wayland_id)).then_some(()),
        );
    };

    focus(&mut seat, first_id);
    let (client, pasted) = x11_pastes(
        &mut seat,
        &mut wl,
        client,
        first,
        |a| a.CLIPBOARD,
        source,
        text,
    );
    assert!(pasted.is_some(), "read with the keyboard");
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        client.withdraw(first)
    });
    keyboard_back_on_wayland(&mut seat);
    let (client, refused) = x11_pastes(
        &mut seat,
        &mut wl,
        client,
        asker,
        |a| a.CLIPBOARD,
        source,
        text,
    );
    assert_eq!(refused, None, "nothing once the window is unmapped");
    assert_eq!(wl.source(source).asked.len(), 1);

    let (client, second) = open_x11_window(&mut seat, client, "second");
    let (client, pasted) = x11_pastes(
        &mut seat,
        &mut wl,
        client,
        second,
        |a| a.CLIPBOARD,
        source,
        text,
    );
    assert!(
        pasted.is_some(),
        "the second window reads with the keyboard"
    );
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        client
            .connection
            .destroy_window(second)
            .map_err(|e| e.to_string())?;
        client.sync()
    });
    keyboard_back_on_wayland(&mut seat);
    let (_client, refused) = x11_pastes(
        &mut seat,
        &mut wl,
        client,
        asker,
        |a| a.CLIPBOARD,
        source,
        text,
    );
    assert_eq!(refused, None, "nothing once the window is destroyed");
    assert_eq!(wl.source(source).asked.len(), 2);
}
