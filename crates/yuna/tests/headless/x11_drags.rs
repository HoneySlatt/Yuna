//! Drag and drop between a seat's X11 programs and its Wayland programs, both ways, through
//! Yuna's X11 window manager, which speaks XDND (version 5) for the Wayland side: text, a file's
//! URI and a mebibyte through INCR, exact bytes; a copy and a move; the drop's end told to the
//! source; a target that refuses, accepts too late, never finishes, reads early, is not
//! XDND-aware, is aware at an old version or has a proxy; a source, a target or their programs
//! that go mid-drag; `XdndSelection` taken or read by another program; a drag icon under the
//! pointer; a still pointer; one motion out of a Wayland window while Yuna's window manager is
//! busy; the window over Wayland windows as a source that wants a named window finds it; a source
//! that repeats `XdndEnter`; a click after a drop; an action the Wayland source did not offer; a
//! drag within one Wayland client; a second drag in one press; an X11-to-X11 drag Yuna leaves
//! alone; a second drag before the first is finished; Xwayland killed or the seat removed
//! mid-drag; a release in the round of the move, over an X11 window hidden under a Wayland one,
//! the source held until the proxy is mapped.
//!
//! Each story runs the development shell's Xwayland for an agent's seat at 1.5
//! (`x11_windows.rs`), on the real clock but for one on the manual clock. The X11 programs are
//! the X11 test client (`x11_client.rs`) as an XDND source or target (`xdnd_client.rs`): each
//! runs on a thread of its own for the whole story, reads its events as they come, as a
//! toolkit does, and logs every message it sends and gets. The Wayland program is a selection
//! client (`selection_client.rs`) with the seat's pointer, on the story's thread, read between
//! the compositor's rounds; its drag sources write into their pipes on threads of their own.
//! The story's thread drives the seat's pointer as an agent does, through Yuna's API: it
//! presses, moves and releases between rounds, and waits for what each side logs.

use std::fs::File;
use std::io::Write;
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};
use wayland_client::protocol::wl_data_device_manager::DndAction;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{Atom, ConnectionExt as _, Window};
use yuna::yuna_layout::{OutputId, WindowId};
use yuna::{BUTTON_LEFT, ClockMode};

use crate::selection_client::{DragEvent, SelectionClient, SourceDragEvent};
use crate::x11_client::{PATIENCE, WindowSpec, X11Client, step};
use crate::x11_selections::{
    MEBIBYTE, TEXT, open_wayland_window, owner_of, paste, pattern, take_selection,
};
use crate::x11_windows::{X11Seat, at_1_5, off, open_drawn, pump, wait};
use crate::xdnd_client::{
    Answer, Finish, Logged, Peer, SourceOrder, SourceSpec, TargetOrder, TargetSpec, Xdnd,
    XdndAtoms, XdndSource, XdndTarget, named_window_at, send_xdnd, set_proxy, target_at,
    xdnd_atoms,
};

/// How long a story watches for something that must not happen: Yuna answers an X11 or a
/// Wayland message within a few rounds of its loop, milliseconds apart; a second is hundreds of
/// rounds, and keeps the stories quick.
const QUIET: Duration = Duration::from_secs(1);

/// How long Yuna waits for the other side of a drag before it gives up on it: a target's
/// status, a source's drop, a finish.
const DRAG_TIME_LIMIT: Duration = Duration::from_secs(5);

/// How late past [`DRAG_TIME_LIMIT`] a story still takes Yuna's end of a drag: a loaded
/// machine's rounds, as the clipboard's time limit allows.
const MARGIN: Duration = Duration::from_secs(3);

/// The MIME type of a list of URIs (RFC 2483).
const URI_LIST: &str = "text/uri-list";

/// A file's URI as `text/uri-list` carries it: its space escaped, its line ended by CR LF.
const URI: &[u8] = b"file:///tmp/Yuna%20drag/notes.txt\r\n";

/// A place of the seat's pointer: its output, and the point from the output's corner, in
/// logical units.
type Spot = (OutputId, f64, f64);

/// An agent's seat with an Xwayland, and the story's Wayland program with the seat's pointer.
struct DragSeat {
    seat: X11Seat,
    wl: SelectionClient,
    /// The bytes each drag source of `wl`'s writes into each pipe it is sent, by source, each
    /// on a thread of its own, as a program writes into its pipe.
    answers: Vec<(usize, Vec<u8>)>,
}

impl DragSeat {
    /// A seat of one output at 1.5, on the real clock, without motion.
    fn new(story: &str) -> DragSeat {
        DragSeat::on(X11Seat::new(story, &[at_1_5()], ClockMode::Real, off()))
    }

    fn on(mut seat: X11Seat) -> DragSeat {
        let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
        wl.take_pointer(&mut seat.compositor);
        DragSeat {
            seat,
            wl,
            answers: Vec::new(),
        }
    }

    /// One round: the compositor's loop, then the Wayland program's events, each pipe its
    /// sources were sent given a writer.
    fn round(&mut self) {
        pump(&mut self.seat.compositor);
        self.wl.read_events();
        for (source, data) in &self.answers {
            for fd in self.wl.take_sent(*source) {
                let data = data.clone();
                thread::spawn(move || {
                    // A reader that gave up closes its end: the write fails, as a program's.
                    let _ = File::from(fd).write_all(&data);
                });
            }
        }
    }

    /// Runs rounds until `done` gives something, for up to [`PATIENCE`].
    #[track_caller]
    fn until<T>(&mut self, what: &str, mut done: impl FnMut(&mut DragSeat) -> Option<T>) -> T {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(value) = done(self) {
                return value;
            }
            assert!(Instant::now() < deadline, "{what}: not within {PATIENCE:?}");
            self.round();
        }
    }

    /// Runs rounds for `how_long`: what is under way settles.
    fn settle(&mut self, how_long: Duration) {
        let end = Instant::now() + how_long;
        while Instant::now() < end {
            self.round();
        }
    }

    /// Runs rounds for `how_long`, and fails the story with `what` if `happened` ever holds.
    #[track_caller]
    fn never(
        &mut self,
        what: &str,
        how_long: Duration,
        mut happened: impl FnMut(&DragSeat) -> bool,
    ) {
        let end = Instant::now() + how_long;
        while Instant::now() < end {
            self.round();
            assert!(!happened(self), "{what}");
        }
    }

    /// The place at `fraction` of `window`'s width and height, as it is drawn now.
    fn point_in(&self, window: WindowId, (fx, fy): (f64, f64)) -> Spot {
        let compositor = &self.seat.compositor;
        let description = compositor
            .windows()
            .into_iter()
            .find(|description| description.id == window)
            .expect("the window");
        let output = description.output.expect("shown");
        let rect = description.rect.expect("drawn");
        let output_rect = self.seat.output_rect(output);
        (
            output,
            rect.x - output_rect.x + fx * rect.w,
            rect.y - output_rect.y + fy * rect.h,
        )
    }

    fn move_to(&mut self, (output, x, y): Spot) {
        let seat = self.seat.seat;
        self.seat
            .compositor
            .pointer_move(seat, Some(output), x, y)
            .expect("the pointer moves");
        self.round();
    }

    fn press(&mut self) {
        let seat = self.seat.seat;
        self.seat
            .compositor
            .pointer_button(seat, BUTTON_LEFT, true)
            .expect("pressed");
        self.round();
    }

    fn release(&mut self) {
        let seat = self.seat.seat;
        self.seat
            .compositor
            .pointer_button(seat, BUTTON_LEFT, false)
            .expect("released");
        self.round();
    }

    /// Opens a window of the Wayland program; returns its id and its index among the program's
    /// windows.
    fn open_wayland(&mut self) -> (WindowId, usize) {
        let id = open_wayland_window(&mut self.seat, &mut self.wl);
        (id, self.wl.client.state.windows.len() - 1)
    }

    /// Opens a drawn X11 window titled `title` of a new X11 program; returns the program, its
    /// window and the window's id.
    fn open_x11(&mut self, title: &str) -> (X11Client, Window, WindowId) {
        let client = self.seat.client();
        let (client, window, described, _) =
            open_drawn(&mut self.seat.compositor, client, &WindowSpec::named(title));
        (client, window, described.id)
    }

    /// The Wayland program's drag events after its first `from`.
    fn drags_since(&self, from: usize) -> &[DragEvent] {
        &self.wl.state.drags[from..]
    }

    /// The drag events of the Wayland program's source `source`, in order.
    fn source_events(&self, source: usize) -> &[SourceDragEvent] {
        &self.wl.source(source).drag
    }
}

// What an X11 drag onto a Wayland window takes, in steps.

/// A Wayland window, then an X11 window beside it, drawn, whose program drags as `spec` says,
/// of the atoms it is given. Returns the seat, the source, and the X11 and the Wayland windows.
#[track_caller]
fn x11_source_and_wayland_window(
    story: &str,
    spec: impl FnOnce(&XdndAtoms) -> SourceSpec,
) -> (DragSeat, XdndSource, WindowId, WindowId) {
    let mut drag = DragSeat::new(story);
    let (wayland, _) = drag.open_wayland();
    let (client, window, x11) = drag.open_x11("source");
    let spec = spec(&xdnd_atoms(&client).expect("the XDND atoms"));
    let source = XdndSource::start(&mut drag.seat.compositor, client, window, spec);
    (drag, source, x11, wayland)
}

/// Presses the pointer in the middle of `from`, the source's window, and waits until its drag
/// starts.
#[track_caller]
fn press_on_source(drag: &mut DragSeat, source: &XdndSource, from: WindowId) {
    let spot = drag.point_in(from, (0.5, 0.5));
    drag.move_to(spot);
    drag.press();
    drag.until("the X11 source's drag starts", |_| {
        source.pressed().then_some(())
    });
}

/// Moves the pointer into `onto` in steps, as a hand drags, to its middle.
#[track_caller]
fn move_into(drag: &mut DragSeat, onto: WindowId) {
    for fraction in [0.2, 0.35, 0.5] {
        let spot = drag.point_in(onto, (fraction, 0.5));
        drag.move_to(spot);
    }
}

/// Waits until a drag with an offer enters the Wayland program, after its first `from` drag
/// events; returns the offer and the `enter`'s serial.
#[track_caller]
fn wayland_entered(drag: &mut DragSeat, from: usize) -> (usize, u32) {
    drag.until("a drag enters the Wayland window", |drag| {
        drag.drags_since(from)
            .iter()
            .find_map(|event| match *event {
                DragEvent::Enter {
                    serial,
                    offer: Some(offer),
                    ..
                } => Some((offer, serial)),
                _ => None,
            })
    })
}

/// The Wayland program takes `mime_type` of `offer` for `action` alone.
#[track_caller]
fn wayland_accepts(
    drag: &mut DragSeat,
    (offer, serial): (usize, u32),
    mime_type: &str,
    action: DndAction,
) {
    drag.wl.accept(offer, serial, Some(mime_type));
    drag.wl.set_actions(offer, action, action);
    drag.wl.client.send();
    drag.round();
}

/// Moves the pointer within `onto`, a few pixels back and forth, until the source's last status
/// accepts the drop with an action: the target's answer reaches the source with the next
/// position, if not before.
#[track_caller]
fn move_until_accepted(drag: &mut DragSeat, source: &XdndSource, onto: WindowId) {
    let deadline = Instant::now() + PATIENCE;
    for turn in 0..u32::MAX {
        assert!(
            Instant::now() < deadline,
            "the X11 source's drag is never accepted: {:?}",
            source.got()
        );
        let spot = drag.point_in(onto, (0.5 + 0.01 * f64::from(turn % 2), 0.5));
        drag.move_to(spot);
        let settled = Instant::now() + Duration::from_millis(100);
        while Instant::now() < settled {
            if source.accepts() {
                return;
            }
            drag.round();
        }
    }
}

/// Waits until the Wayland program gets a drop, after its first `from` drag events.
#[track_caller]
fn wayland_dropped(drag: &mut DragSeat, from: usize) {
    drag.until("the Wayland window gets the drop", |drag| {
        drag.drags_since(from)
            .contains(&DragEvent::Drop)
            .then_some(())
    });
}

/// Waits until the Wayland program is left, after its first `from` drag events.
#[track_caller]
fn wayland_left(drag: &mut DragSeat, from: usize) {
    drag.until("the Wayland window is left", |drag| {
        drag.drags_since(from)
            .contains(&DragEvent::Leave)
            .then_some(())
    });
}

/// The Wayland program reads `offer` as `mime_type`, whole, through a pipe.
#[track_caller]
fn wayland_reads(drag: &mut DragSeat, offer: usize, mime_type: &str) -> Vec<u8> {
    let mut pipe = drag.wl.receive(offer, mime_type);
    drag.wl.client.send();
    let mut data = Vec::new();
    drag.until("the Wayland read ends", |_| {
        pipe.read_available(&mut data).then_some(())
    });
    data
}

/// Waits until `source` gets an `XdndFinished`; returns whether it accepted, and its action.
#[track_caller]
fn x11_finished(drag: &mut DragSeat, source: &XdndSource) -> (bool, Atom) {
    x11_got(
        drag,
        source,
        "the X11 source's XdndFinished",
        |message| match *message {
            Xdnd::Finished {
                accepted, action, ..
            } => Some((accepted, action)),
            _ => None,
        },
    )
}

/// What an X11 drag onto a Wayland window came to.
struct Dropped {
    /// The bytes the Wayland program read.
    data: Vec<u8>,
    /// The Wayland program's offer.
    offer: usize,
    /// The source's `XdndFinished`: accepted, and its action.
    finished: (bool, Atom),
}

/// An X11 drag from `source`'s window `from` onto the Wayland window `onto`, whose program
/// takes `mime_type` for `action`: pressed, moved in, accepted, released, dropped, read whole,
/// finished, through to the source's `XdndFinished`.
#[track_caller]
fn x11_drag_onto_wayland(
    drag: &mut DragSeat,
    source: &XdndSource,
    (from, onto): (WindowId, WindowId),
    mime_type: &str,
    action: DndAction,
) -> Dropped {
    let since = drag.wl.state.drags.len();
    press_on_source(drag, source, from);
    move_into(drag, onto);
    let entered = wayland_entered(drag, since);
    wayland_accepts(drag, entered, mime_type, action);
    move_until_accepted(drag, source, onto);
    drag.release();
    wayland_dropped(drag, since);
    let data = wayland_reads(drag, entered.0, mime_type);
    drag.wl.finish(entered.0);
    drag.wl.client.send();
    let finished = x11_finished(drag, source);
    Dropped {
        data,
        offer: entered.0,
        finished,
    }
}

/// Waits until `peer` got a message `pick` takes; returns what it took of the first.
#[track_caller]
fn x11_got<Order, T>(
    drag: &mut DragSeat,
    peer: &Peer<Order>,
    what: &str,
    pick: impl Fn(&Xdnd) -> Option<T>,
) -> T {
    drag.until(what, |_| peer.got().iter().find_map(&pick))
}

/// Whether `message` is an `XdndDrop`.
fn is_drop(message: &Xdnd) -> bool {
    matches!(message, Xdnd::Drop { .. })
}

/// Whether `message` is an `XdndLeave`.
fn is_leave(message: &Xdnd) -> bool {
    matches!(message, Xdnd::Leave { .. })
}

/// How many `XdndPosition`s `source` sent.
fn positions_sent(source: &XdndSource) -> usize {
    source
        .sent()
        .iter()
        .filter(|message| matches!(message, Xdnd::Position { .. }))
        .count()
}

// What a Wayland drag onto an X11 window takes, in steps.

/// An X11 window whose program takes drops as `spec` says, of the atoms it is given, then a
/// Wayland window beside it. Returns the target, the X11 window, and the Wayland window with
/// its index among the program's windows.
#[track_caller]
fn x11_target_and_wayland_window(
    drag: &mut DragSeat,
    spec: impl FnOnce(&XdndAtoms) -> TargetSpec,
) -> (XdndTarget, WindowId, (WindowId, usize)) {
    let (client, window, x11) = drag.open_x11("target");
    let spec = spec(&xdnd_atoms(&client).expect("the XDND atoms"));
    let target = XdndTarget::start(&mut drag.seat.compositor, client, window, spec);
    let wayland = drag.open_wayland();
    (target, x11, wayland)
}

/// Starts a drag of a new source of the Wayland program offering `mime_types` for `actions`,
/// from its window `from` (its `index`-th), pressed in its middle; the source writes `data`
/// into each pipe it is sent. Returns the source.
#[track_caller]
fn wayland_drag_starts(
    drag: &mut DragSeat,
    (from, index): (WindowId, usize),
    mime_types: &[&str],
    actions: DndAction,
    data: &[u8],
) -> usize {
    let spot = drag.point_in(from, (0.5, 0.5));
    drag.move_to(spot);
    let presses = drag.wl.state.presses.len();
    drag.press();
    drag.until("the Wayland program gets the press", |drag| {
        (drag.wl.state.presses.len() > presses).then_some(())
    });
    let origin = drag.wl.client.window(index).surface.clone();
    let serial = drag.wl.press_serial();
    let source = drag
        .wl
        .start_drag_for(mime_types, Some(actions), &origin, serial);
    drag.answers.push((source, data.to_vec()));
    drag.wl.client.send();
    drag.round();
    source
}

/// Waits until the Wayland source `source` is told the action its target takes, `action`: the
/// target's status reached Yuna.
#[track_caller]
fn wayland_source_told(drag: &mut DragSeat, source: usize, action: DndAction) {
    let chose = SourceDragEvent::Action(action.bits());
    drag.until("the Wayland source is told its target's action", |drag| {
        drag.source_events(source).contains(&chose).then_some(())
    });
}

/// Waits until the Wayland source `source` is finished or cancelled; returns its drag events.
#[track_caller]
fn wayland_source_ends(drag: &mut DragSeat, source: usize) -> Vec<SourceDragEvent> {
    drag.until("the Wayland source's drag ends", |drag| {
        let events = drag.source_events(source);
        events
            .iter()
            .any(|event| {
                matches!(
                    event,
                    SourceDragEvent::Finished | SourceDragEvent::Cancelled
                )
            })
            .then(|| events.to_vec())
    })
}

/// Fails the story unless `events`, a Wayland source's, tell a drop performed then finished,
/// with `action` the last chosen, and never a cancel.
#[track_caller]
fn assert_completed(events: &[SourceDragEvent], action: DndAction) {
    let performed = events
        .iter()
        .position(|event| *event == SourceDragEvent::DropPerformed);
    let finished = events
        .iter()
        .position(|event| *event == SourceDragEvent::Finished);
    assert!(
        matches!((performed, finished), (Some(performed), Some(finished)) if performed < finished),
        "performed, then finished: {events:?}"
    );
    assert!(
        !events.contains(&SourceDragEvent::Cancelled),
        "never cancelled: {events:?}"
    );
    let last_action = events.iter().rev().find_map(|event| match event {
        SourceDragEvent::Action(action) => Some(*action),
        _ => None,
    });
    assert_eq!(last_action, Some(action.bits()), "{events:?}");
}

/// Fails the story unless `events`, a Wayland source's, tell a cancel and never a drop
/// performed.
#[track_caller]
fn assert_cancelled(events: &[SourceDragEvent]) {
    assert!(
        events.contains(&SourceDragEvent::Cancelled),
        "cancelled: {events:?}"
    );
    assert!(
        !events.contains(&SourceDragEvent::DropPerformed),
        "never performed: {events:?}"
    );
}

/// A Wayland drag of `mime_types` writing `data` from the Wayland window `from` onto
/// `target`'s window `onto`, which accepts it: started, moved in, told accepted, released.
/// Returns the source.
#[track_caller]
fn wayland_drag_onto_x11(
    drag: &mut DragSeat,
    target: &XdndTarget,
    (from, onto): ((WindowId, usize), WindowId),
    mime_types: &[&str],
    data: &[u8],
) -> usize {
    let source = wayland_drag_starts(drag, from, mime_types, DndAction::Copy, data);
    move_into(drag, onto);
    x11_got(drag, target, "the X11 target's XdndEnter", |message| {
        matches!(message, Xdnd::Enter { .. }).then_some(())
    });
    wayland_source_told(drag, source, DndAction::Copy);
    drag.release();
    source
}

/// Fails the story unless every `XdndPosition` and the `XdndDrop` `target` got name one time,
/// not CurrentTime: the time Yuna took `XdndSelection` at, which its conversions are checked
/// against.
#[track_caller]
fn assert_one_time(target: &XdndTarget) {
    let times: Vec<u32> = target
        .got()
        .into_iter()
        .filter_map(|message| match message {
            Xdnd::Position { time, .. } | Xdnd::Drop { time, .. } => Some(time),
            _ => None,
        })
        .collect();
    assert!(
        !times.is_empty() && times.iter().all(|&time| time == times[0]),
        "one time for the drag: {times:?}"
    );
    assert_ne!(times[0], x11rb::CURRENT_TIME, "a time, not CurrentTime");
}

/// Waits until `target` read `XdndSelection`; returns what it got of each read.
#[track_caller]
fn x11_reads(drag: &mut DragSeat, target: &XdndTarget) -> Vec<Option<(Atom, Vec<u8>)>> {
    drag.until("the X11 target reads XdndSelection", |_| {
        let reads = target.reads();
        (!reads.is_empty()).then(|| reads.into_iter().map(|(_, _, got)| got).collect())
    })
}

// X11 to Wayland.

/// An X11 program drags UTF-8 text onto a Wayland window: the Wayland program is offered it as
/// `text/plain;charset=utf-8` alone and reads it exactly; the X11 program is told the drop was
/// performed, a copy. Each status Yuna sends asks for every position (bit 1), its rectangle
/// empty, and an accepted one carries an action; Yuna's read names the drop's time, which alone
/// the program serves then.
#[test]
fn x11_text_dropped_on_a_wayland_window_arrives_exactly() {
    let text = "Yuna, ユナ, dragged".as_bytes().to_vec();
    let offers = text.clone();
    let (mut drag, source, from, onto) = x11_source_and_wayland_window("dnd-x-text", |atoms| {
        SourceSpec::offering(vec![(atoms.UTF8_STRING, offers)], atoms.XdndActionCopy)
    });
    let dropped = x11_drag_onto_wayland(&mut drag, &source, (from, onto), TEXT, DndAction::Copy);
    assert_eq!(dropped.data, text);
    assert_eq!(drag.wl.state.offers[dropped.offer].mime_types, [TEXT]);
    assert_eq!(dropped.finished, (true, source.atoms.XdndActionCopy));
    // The source serves a request only at the time XDND gave it, as Qt's: the read after the
    // drop names the drop's time exactly.
    let dropped_at = source
        .sent()
        .into_iter()
        .find_map(|message| match message {
            Xdnd::Drop { time, .. } => Some(time),
            _ => None,
        })
        .expect("the source dropped");
    let requests = source.requests();
    assert!(
        requests.iter().all(|&(_, _, _, served)| served),
        "every read names the time XDND gave: {requests:?}"
    );
    let utf8_string = source.atoms.UTF8_STRING;
    assert!(
        requests
            .iter()
            .any(|&(_, target, time, _)| target == utf8_string && time == dropped_at),
        "the read names the drop's time, {dropped_at}: {requests:?}"
    );
    let statuses: Vec<Xdnd> = source
        .got()
        .into_iter()
        .filter(|message| matches!(message, Xdnd::Status { .. }))
        .collect();
    assert!(!statuses.is_empty());
    for status in statuses {
        let Xdnd::Status {
            accepted,
            wants_positions,
            rect,
            action,
            ..
        } = status
        else {
            unreachable!("a status");
        };
        assert!(wants_positions && rect == [0, 0], "{status:?}");
        assert!(
            !accepted || action != x11rb::NONE,
            "an accepted status carries an action: {status:?}"
        );
    }
}

/// An X11 program drags a file, offering four types, more than `XdndEnter` holds, so listed in
/// `XdndTypeList`, from the pressed window itself, which owns `XdndSelection` and names the
/// source, as some toolkits do: the Wayland program is offered `text/uri-list` among them and
/// reads the URI exactly, its escaped space kept.
#[test]
fn an_x11_files_uri_dropped_on_a_wayland_window_arrives_exactly() {
    let (mut drag, source, from, onto) = x11_source_and_wayland_window("dnd-x-uri", |atoms| {
        let path = b"/tmp/Yuna drag/notes.txt".to_vec();
        let offers = SourceSpec::offering(
            vec![
                (atoms.URI_LIST, URI.to_vec()),
                (
                    atoms.MOZ_URL,
                    b"file:///tmp/Yuna%20drag/notes.txt\nnotes.txt".to_vec(),
                ),
                (atoms.UTF8_STRING, path.clone()),
                (atoms.PLAIN_TEXT, path),
            ],
            atoms.XdndActionCopy,
        );
        SourceSpec {
            owned_by_pressed_window: true,
            ..offers
        }
    });
    let dropped =
        x11_drag_onto_wayland(&mut drag, &source, (from, onto), URI_LIST, DndAction::Copy);
    assert_eq!(dropped.data, URI);
    let offered = &drag.wl.state.offers[dropped.offer].mime_types;
    assert!(
        offered.iter().any(|offered| offered == URI_LIST),
        "{offered:?}"
    );
    assert!(
        source.sent().iter().any(|message| matches!(
            message,
            Xdnd::Enter { source: named, listed: true, .. } if *named == source.window
        )),
        "the types are listed, the pressed window named"
    );
    assert_eq!(dropped.finished, (true, source.atoms.XdndActionCopy));
}

/// A mebibyte dragged from X11 onto a Wayland window: above X11's largest request, it goes
/// through INCR, and arrives whole and in order.
#[test]
fn a_mebibyte_dragged_from_x11_arrives_whole_through_incr() {
    let data = pattern(MEBIBYTE);
    let offers = data.clone();
    let (mut drag, source, from, onto) = x11_source_and_wayland_window("dnd-x-mib", |atoms| {
        SourceSpec::offering(vec![(atoms.UTF8_STRING, offers)], atoms.XdndActionCopy)
    });
    let dropped = x11_drag_onto_wayland(&mut drag, &source, (from, onto), TEXT, DndAction::Copy);
    assert_eq!(dropped.data.len(), data.len());
    assert!(dropped.data == data, "the bytes come whole and in order");
    assert_eq!(dropped.finished, (true, source.atoms.XdndActionCopy));
}

/// An X11 program cancels its drag over a Wayland window, `XdndLeave`, as Escape does: the
/// Wayland window is left, and is not dropped on once the button is released over it.
#[test]
fn an_x11_source_that_leaves_over_a_wayland_window_never_drops() {
    let (mut drag, source, from, onto) = x11_source_and_wayland_window("dnd-x-leave", |atoms| {
        SourceSpec::offering(
            vec![(atoms.UTF8_STRING, b"left".to_vec())],
            atoms.XdndActionCopy,
        )
    });
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    let entered = wayland_entered(&mut drag, since);
    wayland_accepts(&mut drag, entered, TEXT, DndAction::Copy);
    source.order(SourceOrder::Leave);
    wayland_left(&mut drag, since);
    drag.release();
    drag.never("the Wayland window is dropped on", QUIET, |drag| {
        drag.drags_since(since).contains(&DragEvent::Drop)
    });
}

/// An X11 drag released over a Wayland window that never accepted it: the X11 program's last
/// status refused, so it leaves rather than drops, and the Wayland window is left, never
/// dropped on.
#[test]
fn an_x11_drag_released_over_a_wayland_window_that_never_accepts_is_left() {
    let (mut drag, source, from, onto) = x11_source_and_wayland_window("dnd-x-refused", |atoms| {
        SourceSpec::offering(
            vec![(atoms.UTF8_STRING, b"refused".to_vec())],
            atoms.XdndActionCopy,
        )
    });
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    wayland_entered(&mut drag, since);
    let spot = drag.point_in(onto, (0.6, 0.5));
    drag.move_to(spot);
    drag.until("the X11 source gets a status", |_| source.last_status());
    drag.release();
    wayland_left(&mut drag, since);
    drag.never("the Wayland window is dropped on", QUIET, |drag| {
        drag.drags_since(since).contains(&DragEvent::Drop)
    });
    assert_eq!(
        source.last_status().map(|(accepted, _)| accepted),
        Some(false)
    );
    assert!(source.sent().iter().any(is_leave));
    assert!(!source.sent().iter().any(is_drop));
}

/// An X11 program drops on a Wayland window that never accepted, whatever its status said: Yuna
/// answers `XdndFinished` refused, with no action, and the Wayland window is left, never
/// dropped on.
#[test]
fn an_x11_drop_on_a_wayland_window_that_never_accepted_is_refused() {
    let (mut drag, source, from, onto) =
        x11_source_and_wayland_window("dnd-x-forced", |atoms| SourceSpec {
            drops_anyway: true,
            ..SourceSpec::offering(
                vec![(atoms.UTF8_STRING, b"forced".to_vec())],
                atoms.XdndActionCopy,
            )
        });
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    wayland_entered(&mut drag, since);
    drag.until("the X11 source gets a status", |_| source.last_status());
    drag.release();
    let finished = x11_finished(&mut drag, &source);
    assert_eq!(finished, (false, x11rb::NONE));
    wayland_left(&mut drag, since);
    drag.never("the Wayland window is dropped on", QUIET, |drag| {
        drag.drags_since(since).contains(&DragEvent::Drop)
    });
}

/// The Wayland window accepts an X11 drag while the pointer is still, after the X11 program's
/// last position was answered refused: Yuna tells the program at once, with no position asked,
/// and the release drops on the Wayland window, which reads it whole and finishes.
#[test]
fn a_wayland_window_that_accepts_while_the_pointer_is_still_gets_the_drop() {
    let (mut drag, source, from, onto) =
        x11_source_and_wayland_window("dnd-x-still-yes", |atoms| {
            SourceSpec::offering(
                vec![(atoms.UTF8_STRING, b"accepted still".to_vec())],
                atoms.XdndActionCopy,
            )
        });
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    let entered = wayland_entered(&mut drag, since);
    // The motion Yuna repeats once its proxy is shown comes and is answered first.
    drag.settle(QUIET);
    drag.until("the X11 source's positions are answered", |_| {
        (source.last_status().is_some() && !source.awaits_status()).then_some(())
    });
    assert!(
        !source.accepts(),
        "refused before the Wayland window answers"
    );
    let positions = positions_sent(&source);
    wayland_accepts(&mut drag, entered, TEXT, DndAction::Copy);
    drag.until("the X11 source is told the Wayland window accepts", |_| {
        source.accepts().then_some(())
    });
    assert_eq!(positions_sent(&source), positions, "no position was asked");
    drag.release();
    wayland_dropped(&mut drag, since);
    assert_eq!(wayland_reads(&mut drag, entered.0, TEXT), b"accepted still");
    drag.wl.finish(entered.0);
    drag.wl.client.send();
    assert_eq!(
        x11_finished(&mut drag, &source),
        (true, source.atoms.XdndActionCopy)
    );
}

/// The Wayland window reads an X11 drag's data while it is dragged, before any drop, as a
/// target that looks at it first: the read gets the data whole, at once; the drop then goes
/// through and completes.
#[test]
fn a_wayland_window_reads_an_x11_drag_before_the_drop() {
    let text = b"read while dragged".to_vec();
    let offers = text.clone();
    let (mut drag, source, from, onto) = x11_source_and_wayland_window("dnd-x-early", |atoms| {
        SourceSpec::offering(vec![(atoms.UTF8_STRING, offers)], atoms.XdndActionCopy)
    });
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    let entered = wayland_entered(&mut drag, since);
    // Read once Yuna knows the source's last position, whose time the source serves.
    drag.until("the X11 source's positions are answered", |_| {
        (!source.awaits_status()).then_some(())
    });
    let early = wayland_reads(&mut drag, entered.0, TEXT);
    assert_eq!(early, text, "read before the drop");
    assert!(
        !source
            .log()
            .iter()
            .any(|logged| matches!(logged, Logged::Released { .. })),
        "the button is still held"
    );
    wayland_accepts(&mut drag, entered, TEXT, DndAction::Copy);
    move_until_accepted(&mut drag, &source, onto);
    drag.release();
    wayland_dropped(&mut drag, since);
    let late = wayland_reads(&mut drag, entered.0, TEXT);
    assert_eq!(late, text, "read at the drop");
    drag.wl.finish(entered.0);
    drag.wl.client.send();
    assert_eq!(
        x11_finished(&mut drag, &source),
        (true, source.atoms.XdndActionCopy)
    );
}

/// An X11 program asks a move, and the Wayland window takes it as a move: the program is asked
/// to `DELETE` the data after the read and before `XdndFinished`, which tells a move.
#[test]
fn an_x11_move_onto_a_wayland_window_deletes_before_it_finishes() {
    let text = b"moved".to_vec();
    let offers = text.clone();
    let (mut drag, source, from, onto) = x11_source_and_wayland_window("dnd-x-move", |atoms| {
        SourceSpec::offering(vec![(atoms.UTF8_STRING, offers)], atoms.XdndActionMove)
    });
    let dropped = x11_drag_onto_wayland(&mut drag, &source, (from, onto), TEXT, DndAction::Move);
    assert_eq!(dropped.data, text);
    let offer = &drag.wl.state.offers[dropped.offer];
    let offered = offer.source_actions.unwrap_or_default();
    assert!(
        offered & DndAction::Move.bits() != 0,
        "the offer's source asks a move: {offered:#b}"
    );
    assert_eq!(offer.action, Some(DndAction::Move.bits()));
    let atoms = source.atoms;
    assert_eq!(dropped.finished, (true, atoms.XdndActionMove));
    let log = source.log();
    let deleted = log.iter().position(|logged| {
        matches!(logged, Logged::Request { target, served: true, .. } if *target == atoms.DELETE)
    });
    let finished = log.iter().position(|logged| {
        matches!(
            logged,
            Logged::Got {
                message: Xdnd::Finished { .. },
                ..
            }
        )
    });
    assert!(
        matches!((deleted, finished), (Some(deleted), Some(finished)) if deleted < finished),
        "DELETE, then XdndFinished: {log:?}"
    );
}

/// The X11 program that drags exits mid-drag, over a Wayland window: the Wayland window is
/// left; once the button is released, another X11 program's drag drops on it as the first's
/// would have.
#[test]
fn an_x11_source_that_exits_mid_drag_leaves_the_wayland_window() {
    let (mut drag, source, from, onto) =
        x11_source_and_wayland_window("dnd-x-src-exits", |atoms| {
            SourceSpec::offering(
                vec![(atoms.UTF8_STRING, b"gone".to_vec())],
                atoms.XdndActionCopy,
            )
        });
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    let entered = wayland_entered(&mut drag, since);
    wayland_accepts(&mut drag, entered, TEXT, DndAction::Copy);
    source.exit();
    wayland_left(&mut drag, since);
    drag.release();
    drag.never("the Wayland window is dropped on", QUIET, |drag| {
        drag.drags_since(since).contains(&DragEvent::Drop)
    });

    let (client, window, again) = drag.open_x11("again");
    let atoms = xdnd_atoms(&client).expect("the XDND atoms");
    let text = b"dragged again".to_vec();
    let spec = SourceSpec::offering(
        vec![(atoms.UTF8_STRING, text.clone())],
        atoms.XdndActionCopy,
    );
    let source = XdndSource::start(&mut drag.seat.compositor, client, window, spec);
    let dropped = x11_drag_onto_wayland(&mut drag, &source, (again, onto), TEXT, DndAction::Copy);
    assert_eq!(dropped.data, text);
    assert_eq!(dropped.finished, (true, atoms.XdndActionCopy));
}

/// While the pointer is pressed on an X11 window, another X11 program takes `XdndSelection`:
/// Yuna starts no drag for it, so no XDND-aware window lies under the pointer over the Wayland
/// window, and the Wayland window is offered nothing. Then the pressed window's own program
/// drags, and its drop arrives.
#[test]
fn xdnd_selection_taken_by_another_program_than_the_pressed_ones_offers_nothing() {
    let mut drag = DragSeat::new("dnd-x-other-sel");
    let (onto, _) = drag.open_wayland();
    let (client, window, pressed) = drag.open_x11("pressed");
    let other = drag.seat.client();
    let since = drag.wl.state.drags.len();
    let spot = drag.point_in(pressed, (0.5, 0.5));
    drag.move_to(spot);
    drag.press();
    let (other, _) = step(&mut drag.seat.compositor, other, |other| {
        let atoms = xdnd_atoms(other)?;
        take_selection(other, atoms.XdndSelection)
    });
    move_into(&mut drag, onto);
    drag.never("the Wayland window is entered", QUIET, |drag| {
        !drag.drags_since(since).is_empty()
    });
    let (_other, found) = step(&mut drag.seat.compositor, other, |other| {
        let atoms = xdnd_atoms(other)?;
        let pointer = other
            .connection
            .query_pointer(other.root)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok(target_at(
            other,
            &atoms,
            (pointer.root_x, pointer.root_y),
            None,
        ))
    });
    assert_eq!(found, None, "no XDND-aware window under the pointer");
    drag.release();

    // The pressed window's program drags, its press and release left behind.
    let (client, _) = step(&mut drag.seat.compositor, client, |client| {
        client.take_events()
    });
    let atoms = xdnd_atoms(&client).expect("the XDND atoms");
    let text = b"its own".to_vec();
    let spec = SourceSpec::offering(
        vec![(atoms.UTF8_STRING, text.clone())],
        atoms.XdndActionCopy,
    );
    let source = XdndSource::start(&mut drag.seat.compositor, client, window, spec);
    let dropped = x11_drag_onto_wayland(&mut drag, &source, (pressed, onto), TEXT, DndAction::Copy);
    assert_eq!(dropped.data, text);
}

/// A Wayland window reads an X11 drop and never finishes it: the X11 program is told the drop
/// was not performed, `XdndFinished` refused, once the time limit passed with no transfer
/// moving, not before it and not long after.
#[test]
fn a_wayland_window_that_never_finishes_an_x11_drop_is_refused_in_time() {
    let text = b"never finished".to_vec();
    let offers = text.clone();
    let (mut drag, source, from, onto) =
        x11_source_and_wayland_window("dnd-x-unfinished", |atoms| {
            SourceSpec::offering(vec![(atoms.UTF8_STRING, offers)], atoms.XdndActionCopy)
        });
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    let entered = wayland_entered(&mut drag, since);
    wayland_accepts(&mut drag, entered, TEXT, DndAction::Copy);
    move_until_accepted(&mut drag, &source, onto);
    drag.release();
    let released = Instant::now();
    wayland_dropped(&mut drag, since);
    assert_eq!(wayland_reads(&mut drag, entered.0, TEXT), text);
    let finished = x11_finished(&mut drag, &source);
    let took = released.elapsed();
    assert_eq!(finished, (false, x11rb::NONE), "not performed");
    assert!(
        took >= DRAG_TIME_LIMIT,
        "not before the time limit: {took:?}"
    );
    assert!(
        took < DRAG_TIME_LIMIT + MARGIN,
        "within the time limit: {took:?}"
    );
}

/// While an X11 drag is over a Wayland window, another X11 program sends `XdndDrop` to the
/// window it finds under the pointer, naming its own window as the source: Yuna leaves it, and
/// the Wayland window is not dropped on; the real source's drop then arrives.
#[test]
fn an_xdnd_drop_naming_another_source_is_left() {
    let text = b"the real one".to_vec();
    let offers = text.clone();
    let (mut drag, source, from, onto) = x11_source_and_wayland_window("dnd-x-forged", |atoms| {
        SourceSpec::offering(vec![(atoms.UTF8_STRING, offers)], atoms.XdndActionCopy)
    });
    let mut other = drag.seat.client();
    let forger = other
        .create(&WindowSpec::named("forger"))
        .expect("a window");
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    let entered = wayland_entered(&mut drag, since);
    wayland_accepts(&mut drag, entered, TEXT, DndAction::Copy);
    move_until_accepted(&mut drag, &source, onto);
    let (_other, found) = step(&mut drag.seat.compositor, other, move |other| {
        let atoms = xdnd_atoms(other)?;
        let pointer = other
            .connection
            .query_pointer(other.root)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let Some(found) = target_at(other, &atoms, (pointer.root_x, pointer.root_y), None) else {
            return Ok(None);
        };
        let forged = Xdnd::Drop {
            source: forger,
            time: x11rb::CURRENT_TIME,
        };
        send_xdnd(other, &atoms, found.deliver_to, found.window, &forged)?;
        other.sync()?;
        Ok(Some(found))
    });
    assert!(found.is_some(), "an XDND-aware window under the pointer");
    drag.never("the Wayland window is dropped on", QUIET, |drag| {
        drag.drags_since(since).contains(&DragEvent::Drop)
    });
    drag.release();
    wayland_dropped(&mut drag, since);
    assert_eq!(wayland_reads(&mut drag, entered.0, TEXT), text);
    drag.wl.finish(entered.0);
    drag.wl.client.send();
    assert_eq!(
        x11_finished(&mut drag, &source),
        (true, source.atoms.XdndActionCopy)
    );
}

/// An X11 program keeps its drag icon under the pointer as it drags over a Wayland window: an
/// override-redirect window whose input shape it leaves whole, which Yuna draws there as it
/// draws a menu. The drop still reaches the Wayland window.
#[test]
fn an_x11_drag_icon_under_the_pointer_does_not_take_the_drop() {
    let mut drag = DragSeat::new("dnd-x-icon");
    let (onto, _) = drag.open_wayland();
    let (mut client, window, from) = drag.open_x11("source");
    let icon_spec = WindowSpec {
        background: Some(0x00_FF_80_00),
        ..WindowSpec::menu("icon", 0, 0, 48, 48)
    };
    let icon = client.create(&icon_spec).expect("the icon");
    let atoms = xdnd_atoms(&client).expect("the XDND atoms");
    let text = b"under the icon".to_vec();
    let spec = SourceSpec {
        icon: Some(icon),
        ..SourceSpec::offering(
            vec![(atoms.UTF8_STRING, text.clone())],
            atoms.XdndActionCopy,
        )
    };
    let source = XdndSource::start(&mut drag.seat.compositor, client, window, spec);
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    let output = drag.point_in(onto, (0.5, 0.5)).0;
    drag.until("the icon is drawn over the Wayland window", |drag| {
        drag.seat
            .compositor
            .drawn_x11_menus(output)
            .iter()
            .any(|drawn| drawn.window == icon)
            .then_some(())
    });
    let entered = wayland_entered(&mut drag, since);
    wayland_accepts(&mut drag, entered, TEXT, DndAction::Copy);
    move_until_accepted(&mut drag, &source, onto);
    drag.release();
    wayland_dropped(&mut drag, since);
    let data = wayland_reads(&mut drag, entered.0, TEXT);
    drag.wl.finish(entered.0);
    drag.wl.client.send();
    assert_eq!(data, text);
    assert_eq!(
        x11_finished(&mut drag, &source),
        (true, atoms.XdndActionCopy)
    );
}

/// The pointer enters a Wayland window with one motion and stays still, an X11 drag held: the
/// drag enters it all the same, though what Yuna lays over the root for the X11 program to find
/// comes after that motion. The Wayland window accepts, the program is told with no motion, and
/// the release drops on the Wayland window, which reads it whole and finishes.
#[test]
fn one_motion_into_a_wayland_window_is_enough_for_an_x11_drag_to_drop_there() {
    let (mut drag, source, from, onto) = x11_source_and_wayland_window("dnd-x-still", |atoms| {
        SourceSpec::offering(
            vec![(atoms.UTF8_STRING, b"still".to_vec())],
            atoms.XdndActionCopy,
        )
    });
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    let spot = drag.point_in(onto, (0.5, 0.5));
    drag.move_to(spot);
    let entered = wayland_entered(&mut drag, since);
    wayland_accepts(&mut drag, entered, TEXT, DndAction::Copy);
    drag.until("the X11 source is told the Wayland window accepts", |_| {
        (source.accepts() && !source.awaits_status()).then_some(())
    });
    drag.release();
    wayland_dropped(&mut drag, since);
    assert_eq!(wayland_reads(&mut drag, entered.0, TEXT), b"still");
    drag.wl.finish(entered.0);
    drag.wl.client.send();
    assert_eq!(
        x11_finished(&mut drag, &source),
        (true, source.atoms.XdndActionCopy)
    );
}

/// An X11 program drags text onto another X11 program's window beside it, the Wayland window
/// out of the drag's path on the other output: the drag goes from X11 to X11 as on X11 alone,
/// every message to the target's own window, so nothing of Yuna's comes between them; the target
/// reads it whole and the source is finished. Passes before XDND reaches Wayland: it guards
/// X11's own drags. A drag that crosses a Wayland window on its way is the next story's.
#[test]
fn an_x11_to_x11_drag_is_left_to_x11() {
    let mut drag = DragSeat::on(X11Seat::at_both("dnd-x11-x11"));
    let (at_one_and_a_half, at_one) = (drag.seat.outputs[0], drag.seat.outputs[1]);
    drag.seat.open_on(at_one);
    drag.open_wayland();
    drag.seat.open_on(at_one_and_a_half);
    let (client, window, from) = drag.open_x11("source");
    let atoms = xdnd_atoms(&client).expect("the XDND atoms");
    let text = b"X11 to X11".to_vec();
    let spec = SourceSpec::offering(
        vec![(atoms.UTF8_STRING, text.clone())],
        atoms.XdndActionCopy,
    );
    let source = XdndSource::start(&mut drag.seat.compositor, client, window, spec);
    let (target_client, target_window, onto) = drag.open_x11("target");
    let target = XdndTarget::start(
        &mut drag.seat.compositor,
        target_client,
        target_window,
        TargetSpec::copying(&atoms, atoms.UTF8_STRING),
    );
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    move_until_accepted(&mut drag, &source, onto);
    drag.release();
    let reads = x11_reads(&mut drag, &target);
    assert_eq!(reads, [Some((atoms.UTF8_STRING, text))]);
    assert_eq!(
        x11_finished(&mut drag, &source),
        (true, atoms.XdndActionCopy)
    );
    let sent_elsewhere: Vec<Logged> = source
        .log()
        .into_iter()
        .filter(|logged| matches!(logged, Logged::Sent { to, .. } if *to != target_window))
        .collect();
    assert!(sent_elsewhere.is_empty(), "{sent_elsewhere:?}");
}

/// An X11 drag crosses a Wayland window on its way to another X11 program's window, on the other
/// output: the Wayland window is entered while the pointer is over it, then left, and never
/// dropped on; past it, every message goes to the target's own window, nothing of Yuna's in
/// the way any more; the target reads the drop whole and the source is finished.
#[test]
fn an_x11_drag_across_a_wayland_window_drops_on_another_x11_window() {
    let mut drag = DragSeat::on(X11Seat::at_both("dnd-x-cross"));
    let (at_one_and_a_half, at_one) = (drag.seat.outputs[0], drag.seat.outputs[1]);
    drag.seat.open_on(at_one_and_a_half);
    let (client, window, from) = drag.open_x11("source");
    let (crossed, _) = drag.open_wayland();
    drag.seat.open_on(at_one);
    let (target_client, target_window, onto) = drag.open_x11("target");
    let atoms = xdnd_atoms(&client).expect("the XDND atoms");
    let text = b"across Wayland".to_vec();
    let spec = SourceSpec::offering(
        vec![(atoms.UTF8_STRING, text.clone())],
        atoms.XdndActionCopy,
    );
    let source = XdndSource::start(&mut drag.seat.compositor, client, window, spec);
    let target = XdndTarget::start(
        &mut drag.seat.compositor,
        target_client,
        target_window,
        TargetSpec::copying(&atoms, atoms.UTF8_STRING),
    );
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, crossed);
    wayland_entered(&mut drag, since);
    move_into(&mut drag, onto);
    wayland_left(&mut drag, since);
    move_until_accepted(&mut drag, &source, onto);
    drag.release();
    assert_eq!(
        x11_reads(&mut drag, &target),
        [Some((atoms.UTF8_STRING, text))]
    );
    assert_eq!(
        x11_finished(&mut drag, &source),
        (true, atoms.XdndActionCopy)
    );
    drag.never("the Wayland window is dropped on", QUIET, |drag| {
        drag.drags_since(since).contains(&DragEvent::Drop)
    });
    let sent: Vec<Window> = source
        .log()
        .into_iter()
        .filter_map(|logged| match logged {
            Logged::Sent { to, .. } => Some(to),
            _ => None,
        })
        .collect();
    let first_to_target = sent
        .iter()
        .position(|&to| to == target_window)
        .expect("messages to the target");
    assert!(
        first_to_target > 0
            && sent[first_to_target..]
                .iter()
                .all(|&to| to == target_window),
        "past the Wayland window, every message to the target: {sent:?}"
    );
}

/// How many times a story renames a window, keeping Yuna's window manager busy reading titles for
/// a while, as a program that updates its title fast would. In the next story, the source then
/// looks for its target before the proxy is hidden; on a machine where the manager keeps up
/// anyway, that story would pass without Yuna's repeated motion too, and guard nothing there.
/// `x11_pairing.rs` holds a window's serial back the same way.
pub(crate) const RENAMES: usize = 2_000;

/// An X11 program's drag held over a Wayland window moves in one motion onto another X11
/// program's window, while Yuna's window manager is still busy with a few thousand renames of the
/// source's window: the manager hides what Yuna lays over the root for X11 programs to find over
/// Wayland windows only once it has caught up, after the source has looked for its target for
/// that motion and found Yuna's window still. Once it is hidden, Yuna shows the source the
/// pointer again: the source looks again, speaks to the X11 window, and the release drops there.
/// An idle manager on a quiet machine mostly wins that race; a busy manager, as here, or a loaded
/// machine loses it. A release that reaches the source before its second look is not covered:
/// the source decides on its first.
#[test]
fn an_x11_drag_moved_out_of_a_wayland_window_in_one_motion_finds_the_x11_window() {
    use x11rb::protocol::xproto::{AtomEnum, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    let mut drag = DragSeat::on(X11Seat::at_both("dnd-x-out-one-motion"));
    let (at_one_and_a_half, at_one) = (drag.seat.outputs[0], drag.seat.outputs[1]);
    drag.seat.open_on(at_one_and_a_half);
    let (client, window, from) = drag.open_x11("source");
    let (crossed, _) = drag.open_wayland();
    drag.seat.open_on(at_one);
    let (target_client, target_window, onto) = drag.open_x11("target");
    let atoms = xdnd_atoms(&client).expect("the XDND atoms");
    let spec = SourceSpec::offering(
        vec![(atoms.UTF8_STRING, b"one motion".to_vec())],
        atoms.XdndActionCopy,
    );
    let source = XdndSource::start(&mut drag.seat.compositor, client, window, spec);
    let target = XdndTarget::start(
        &mut drag.seat.compositor,
        target_client,
        target_window,
        TargetSpec::copying(&atoms, atoms.UTF8_STRING),
    );
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, crossed);
    wayland_entered(&mut drag, since);
    drag.until("the X11 source's positions are answered", |_| {
        (!source.awaits_status() && source.last_status().is_some()).then_some(())
    });
    let busy = drag.seat.client();
    let (_busy, ()) = step(&mut drag.seat.compositor, busy, move |busy| {
        for n in 0..RENAMES {
            busy.connection
                .change_property8(
                    PropMode::REPLACE,
                    window,
                    AtomEnum::WM_NAME,
                    AtomEnum::STRING,
                    format!("source, renamed {n} times").as_bytes(),
                )
                .map_err(|e| e.to_string())?;
        }
        busy.sync()
    });
    let logged = source.log().len();
    let spot = drag.point_in(onto, (0.5, 0.5));
    drag.move_to(spot);
    let spoke_to_target = |source: &XdndSource| {
        source.log()[logged..].iter().any(|entry| {
            matches!(entry, Logged::Sent { to, message: Xdnd::Position { .. }, .. }
                if *to == target_window)
        })
    };
    let deadline = Instant::now() + DRAG_TIME_LIMIT + MARGIN;
    while Instant::now() < deadline && !spoke_to_target(&source) {
        drag.round();
    }
    assert!(
        spoke_to_target(&source),
        "the source never spoke to the X11 window under the pointer: {:#?}",
        &source.log()[logged..]
    );
    drag.until("the X11 window accepts", |_| {
        (source.accepts() && !source.awaits_status()).then_some(())
    });
    drag.release();
    assert_eq!(
        x11_reads(&mut drag, &target),
        [Some((atoms.UTF8_STRING, b"one motion".to_vec()))]
    );
    assert_eq!(
        x11_finished(&mut drag, &source),
        (true, atoms.XdndActionCopy)
    );
}

/// While an X11 drag is over a Wayland window, the window an X11 source finds under the pointer
/// is Yuna's window over the root, even for a source that takes only a named window, as
/// Chromium's does: it is the topmost named window there, and `XdndAware` at version 5.
#[test]
fn the_window_an_x11_drag_finds_over_a_wayland_window_is_named_and_xdnd_aware() {
    let (mut drag, source, from, onto) = x11_source_and_wayland_window("dnd-x-named", |atoms| {
        SourceSpec::offering(
            vec![(atoms.UTF8_STRING, b"named".to_vec())],
            atoms.XdndActionCopy,
        )
    });
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    wayland_entered(&mut drag, since);
    let entered = source
        .log()
        .into_iter()
        .find_map(|logged| match logged {
            Logged::Sent {
                to,
                message: Xdnd::Enter { .. },
                ..
            } => Some(to),
            _ => None,
        })
        .expect("the source entered a window");
    let atoms = source.atoms;
    let other = drag.seat.client();
    let (_other, (named, aware)) = step(&mut drag.seat.compositor, other, move |other| {
        let pointer = other
            .connection
            .query_pointer(other.root)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let named = named_window_at(other, (pointer.root_x, pointer.root_y))?;
        let aware = other.property32(entered, atoms.XdndAware)?;
        Ok((named, aware))
    });
    assert_eq!(
        named,
        Some(entered),
        "the named window under the pointer is the one the source entered"
    );
    assert_eq!(aware, Some(vec![5]), "XdndAware 5");
    drag.release();
}

/// How many `XdndEnter` and `XdndPosition` pairs the hostile sources of the next stories send.
const REPEATED_ENTERS: usize = 200;

/// How many times Yuna lets an X11 drag enter its proxy while it stays shown (`MAX_X11_ENTERS`
/// in `xwayland/drag_session.rs`).
const MOST_X11_ENTERS: usize = 2;

/// A hostile X11 program, its drag over a Wayland window, sends Yuna's window over the root
/// `XdndEnter` then `XdndPosition` [`REPEATED_ENTERS`] times, the same 64 long types listed each
/// time, faster than the Wayland program reads, then `XdndLeave`: the Wayland window is entered
/// once and given one offer, not one for each `XdndEnter`, and its program, which reads nothing
/// meanwhile, is not cut off
/// ([ADR-0131](../../../../docs/adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)).
#[test]
fn an_x11_source_repeating_xdnd_enter_makes_one_wayland_offer() {
    let (drag, since, offers) = hostile_enters("dnd-x-enter-again", false);
    let enters = drag
        .drags_since(since)
        .iter()
        .filter(|event| matches!(event, DragEvent::Enter { .. }))
        .count();
    assert_eq!(enters, 1, "entered once: {:?}", drag.drags_since(since));
    assert_eq!(drag.wl.state.offers.len() - offers, 1, "one offer");
    finish_hostile_enters(drag);
}

/// The hostile source of the story above, but leaving the proxy before each `XdndEnter`: each
/// enter is a new one, and the Wayland window is entered again for it, up to Yuna's bound of
/// enters while the proxy stays shown, then no more; its program, which reads nothing meanwhile, is not cut off
/// ([ADR-0131](../../../../docs/adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)).
#[test]
fn an_x11_source_leaving_and_entering_without_end_makes_a_bounded_number_of_offers() {
    let (drag, since, offers) = hostile_enters("dnd-x-enter-leave", true);
    let enters = drag
        .drags_since(since)
        .iter()
        .filter(|event| matches!(event, DragEvent::Enter { .. }))
        .count();
    assert_eq!(enters, MOST_X11_ENTERS, "{:?}", drag.drags_since(since));
    assert_eq!(drag.wl.state.offers.len() - offers, MOST_X11_ENTERS);
    finish_hostile_enters(drag);
}

/// A hostile X11 source over a Wayland window: it sends [`REPEATED_ENTERS`] pairs of
/// `XdndEnter` and `XdndPosition` with 64 long types listed, each pair after an `XdndLeave` if
/// `leave_between`, then `XdndLeave`, faster than the Wayland program reads. Returns the seat once
/// the program read what came, the program's drag events and offers counted before.
fn hostile_enters(story: &str, leave_between: bool) -> (DragSeat, usize, usize) {
    use x11rb::protocol::xproto::{AtomEnum, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    let mut drag = DragSeat::new(story);
    let (onto, _) = drag.open_wayland();
    let (client, window, from) = drag.open_x11("source");
    let atoms = xdnd_atoms(&client).expect("the XDND atoms");
    let spot = drag.point_in(from, (0.5, 0.5));
    drag.move_to(spot);
    drag.press();
    // Its window pressed, the program takes `XdndSelection`: its drag starts.
    let (client, ()) = step(&mut drag.seat.compositor, client, move |client| {
        client
            .connection
            .set_selection_owner(window, atoms.XdndSelection, x11rb::CURRENT_TIME)
            .map_err(|e| e.to_string())?;
        client.sync()
    });
    move_into(&mut drag, onto);
    let since = drag.wl.state.drags.len();
    let offers = drag.wl.state.offers.len();
    let (_client, ()) = step(&mut drag.seat.compositor, client, move |client| {
        let fail = |e: &dyn std::fmt::Display| e.to_string();
        let pointer = client
            .connection
            .query_pointer(client.root)
            .map_err(|e| fail(&e))?
            .reply()
            .map_err(|e| fail(&e))?;
        let root = (pointer.root_x, pointer.root_y);
        let deadline = Instant::now() + PATIENCE;
        let found = loop {
            if let Some(found) = target_at(client, &atoms, root, None) {
                break found;
            }
            if Instant::now() > deadline {
                return Err("no XDND window under the pointer".to_owned());
            }
            thread::sleep(Duration::from_millis(5));
        };
        let mut types = Vec::new();
        for n in 0..64 {
            let name = format!("application/x-yuna-hostile-{n:02}-{}", "a".repeat(200));
            let atom = client
                .connection
                .intern_atom(false, name.as_bytes())
                .map_err(|e| fail(&e))?
                .reply()
                .map_err(|e| fail(&e))?
                .atom;
            types.push(atom);
        }
        client
            .connection
            .change_property32(
                PropMode::REPLACE,
                window,
                atoms.XdndTypeList,
                AtomEnum::ATOM,
                &types,
            )
            .map_err(|e| fail(&e))?;
        let send =
            |message: &Xdnd| send_xdnd(client, &atoms, found.deliver_to, found.window, message);
        for _ in 0..REPEATED_ENTERS {
            if leave_between {
                send(&Xdnd::Leave { source: window })?;
            }
            send(&Xdnd::Enter {
                source: window,
                version: 5,
                types: types[..3].to_vec(),
                listed: true,
            })?;
            send(&Xdnd::Position {
                source: window,
                root,
                time: x11rb::CURRENT_TIME,
                action: atoms.XdndActionCopy,
            })?;
        }
        send(&Xdnd::Leave { source: window })?;
        client.sync()
    });
    // The Wayland program reads what came meanwhile, then each message is handled, the leave
    // last.
    drag.settle(QUIET);
    wayland_left(&mut drag, since);
    (drag, since, offers)
}

/// Releases the hostile source's drag; the Wayland program is still connected.
fn finish_hostile_enters(mut drag: DragSeat) {
    drag.release();
    drag.settle(QUIET);
    drag.wl
        .try_roundtrip(&mut drag.seat.compositor)
        .expect("the Wayland program is still connected");
}

// Wayland to X11.

/// A Wayland program drags UTF-8 text onto an X11 window: Yuna enters it at version 5 with the
/// text's X11 targets, four, so listed in `XdndTypeList`; every position and the drop name one
/// time, not CurrentTime; the X11 program reads `UTF8_STRING` at the drop exactly, and the
/// Wayland source is told the type accepted, then the drop performed, then finished, a copy.
#[test]
fn wayland_text_dropped_on_an_x11_window_arrives_exactly() {
    let mut drag = DragSeat::new("dnd-w-text");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let text = "Yuna, ユナ, dropped".as_bytes();
    let source = wayland_drag_onto_x11(&mut drag, &target, (from, onto), &[TEXT], text);
    let events = wayland_source_ends(&mut drag, source);
    assert_completed(&events, DndAction::Copy);
    assert!(
        events.contains(&SourceDragEvent::Target(Some(TEXT.to_owned()))),
        "the source is told the type its target accepts: {events:?}"
    );
    let atoms = target.atoms;
    assert_eq!(
        x11_reads(&mut drag, &target),
        [Some((atoms.UTF8_STRING, text.to_vec()))]
    );
    let enter = target
        .got()
        .into_iter()
        .find(|message| matches!(message, Xdnd::Enter { .. }));
    let Some(Xdnd::Enter {
        version,
        types,
        listed,
        ..
    }) = enter
    else {
        panic!("an XdndEnter");
    };
    assert_eq!(version, 5);
    assert!(listed, "four types are listed: {types:?}");
    assert!(
        types.contains(&atoms.UTF8_STRING) && types.contains(&atoms.UTF8_TEXT),
        "{types:?}"
    );
    assert_eq!(drag.wl.source(source).asked, [TEXT]);
    assert_one_time(&target);
}

/// A Wayland program drags a file's URI onto an X11 window whose program reads into the window
/// itself, as some toolkits do: it reads `text/uri-list` exactly, its escaped space kept, and the
/// drop completes.
#[test]
fn a_wayland_files_uri_dropped_on_an_x11_window_arrives_exactly() {
    let mut drag = DragSeat::new("dnd-w-uri");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| TargetSpec {
        reads_into_itself: true,
        ..TargetSpec::copying(atoms, atoms.URI_LIST)
    });
    let source = wayland_drag_onto_x11(&mut drag, &target, (from, onto), &[URI_LIST], URI);
    let events = wayland_source_ends(&mut drag, source);
    assert_completed(&events, DndAction::Copy);
    let uri_list = target.atoms.URI_LIST;
    assert_eq!(
        x11_reads(&mut drag, &target),
        [Some((uri_list, URI.to_vec()))]
    );
}

/// A mebibyte dragged from Wayland onto an X11 window: Yuna answers through INCR, and it
/// arrives whole and in order.
#[test]
fn a_mebibyte_dragged_from_wayland_arrives_whole_through_incr() {
    let mut drag = DragSeat::new("dnd-w-mib");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let data = pattern(MEBIBYTE);
    let source = wayland_drag_onto_x11(&mut drag, &target, (from, onto), &[TEXT], &data);
    let events = wayland_source_ends(&mut drag, source);
    assert_completed(&events, DndAction::Copy);
    let reads = x11_reads(&mut drag, &target);
    let [Some((_, read))] = &reads[..] else {
        panic!("one read, served: {:?}", reads.len());
    };
    assert_eq!(read.len(), data.len());
    assert!(*read == data, "the bytes come whole and in order");
}

/// A Wayland program drags for a move alone onto an X11 window that takes it as a move: Yuna
/// asks a move in each `XdndPosition`, answers the window's `DELETE` after its read, empty, and
/// the source is told the drop was performed, then finished, a move.
#[test]
fn a_wayland_move_onto_an_x11_window_deletes_and_completes_as_a_move() {
    let mut drag = DragSeat::new("dnd-w-move");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| TargetSpec {
        answer: Answer::Accept(atoms.XdndActionMove),
        deletes: true,
        ..TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let text = b"moved from Wayland";
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Move, text);
    move_into(&mut drag, onto);
    let atoms = target.atoms;
    let asked = x11_got(
        &mut drag,
        &target,
        "the X11 target's XdndPosition",
        |message| match *message {
            Xdnd::Position { action, .. } => Some(action),
            _ => None,
        },
    );
    assert_eq!(asked, atoms.XdndActionMove, "a move is asked");
    wayland_source_told(&mut drag, source, DndAction::Move);
    drag.release();
    let events = wayland_source_ends(&mut drag, source);
    assert_completed(&events, DndAction::Move);
    let reads = x11_reads(&mut drag, &target);
    assert_eq!(reads.len(), 2, "the read, then DELETE: {reads:?}");
    assert_eq!(reads[0], Some((atoms.UTF8_STRING, text.to_vec())));
    assert!(
        matches!(&reads[1], Some((_, deleted)) if deleted.is_empty()),
        "DELETE is done, empty: {:?}",
        reads[1]
    );
}

/// An X11 window reads a Wayland drag's data at its first position, before any drop, as a
/// target that looks at it first: the read is served while the button is held, and the drop
/// then completes.
#[test]
fn an_x11_window_reads_a_wayland_drag_before_the_drop() {
    let mut drag = DragSeat::new("dnd-w-early");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| TargetSpec {
        reads_early: true,
        ..TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let text = b"looked at first";
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, text);
    move_into(&mut drag, onto);
    let utf8_string = target.atoms.UTF8_STRING;
    assert_eq!(
        x11_reads(&mut drag, &target),
        [Some((utf8_string, text.to_vec()))],
        "read before the drop"
    );
    assert!(
        !target.got().iter().any(is_drop),
        "the button is still held"
    );
    wayland_source_told(&mut drag, source, DndAction::Copy);
    drag.release();
    let events = wayland_source_ends(&mut drag, source);
    assert_completed(&events, DndAction::Copy);
    assert_eq!(
        x11_reads(&mut drag, &target),
        [
            Some((utf8_string, text.to_vec())),
            Some((utf8_string, text.to_vec()))
        ],
        "read again at the drop"
    );
}

/// An X11 window accepts a Wayland drag with `XdndActionPrivate`, an action the Wayland side
/// has no name for: the source is told a copy, and the drop completes as a copy.
#[test]
fn an_x11_windows_private_action_reads_as_a_copy() {
    let mut drag = DragSeat::new("dnd-w-private");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| TargetSpec {
        answer: Answer::Accept(atoms.XdndActionPrivate),
        ..TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let text = b"private";
    let source = wayland_drag_onto_x11(&mut drag, &target, (from, onto), &[TEXT], text);
    let events = wayland_source_ends(&mut drag, source);
    assert_completed(&events, DndAction::Copy);
    let utf8_string = target.atoms.UTF8_STRING;
    assert_eq!(
        x11_reads(&mut drag, &target),
        [Some((utf8_string, text.to_vec()))]
    );
}

/// Over an X11 window, the root point each `XdndPosition` names is, to a pixel, the point
/// Xwayland itself gives the pointer at the same place, at places between X11 pixels.
#[test]
fn xdnd_positions_name_the_root_point_xwayland_gives_the_pointer() {
    let mut drag = DragSeat::new("dnd-w-points");
    let (client, window, onto) = drag.open_x11("target");
    let from = drag.open_wayland();
    // Places between pixels at 1.5: a third and two thirds of a logical unit off.
    let spots: Vec<Spot> = [(0.25, 0.3), (0.5, 0.5), (0.7, 0.8)]
        .into_iter()
        .map(|fraction| {
            let (output, x, y) = drag.point_in(onto, fraction);
            (output, x + 1.0 / 3.0, y + 2.0 / 3.0)
        })
        .collect();
    let (mut client, before) = step(&mut drag.seat.compositor, client, |client| {
        let pointer = client
            .connection
            .query_pointer(client.root)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok((pointer.root_x, pointer.root_y))
    });
    let mut xwaylands = Vec::new();
    let mut previous = before;
    for &spot in &spots {
        drag.move_to(spot);
        let (moved, point) = step(&mut drag.seat.compositor, client, move |client| {
            let deadline = Instant::now() + PATIENCE;
            loop {
                let pointer = client
                    .connection
                    .query_pointer(client.root)
                    .map_err(|e| e.to_string())?
                    .reply()
                    .map_err(|e| e.to_string())?;
                let point = (pointer.root_x, pointer.root_y);
                if pointer.child == window && point != previous {
                    return Ok(point);
                }
                if Instant::now() > deadline {
                    return Err(format!("Xwayland's pointer stays at {point:?}"));
                }
                thread::sleep(Duration::from_millis(2));
            }
        });
        client = moved;
        previous = point;
        xwaylands.push(point);
    }
    let atoms = xdnd_atoms(&client).expect("the XDND atoms");
    let target = XdndTarget::start(
        &mut drag.seat.compositor,
        client,
        window,
        TargetSpec::copying(&atoms, atoms.UTF8_STRING),
    );
    wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"points");
    for (&spot, &xwayland) in spots.iter().zip(&xwaylands) {
        drag.move_to(spot);
        let close = |(x, y): (i16, i16)| (x - xwayland.0).abs() <= 1 && (y - xwayland.1).abs() <= 1;
        let deadline = Instant::now() + PATIENCE;
        loop {
            let last = target
                .got()
                .into_iter()
                .rev()
                .find_map(|message| match message {
                    Xdnd::Position { root, .. } => Some(root),
                    _ => None,
                });
            if last.is_some_and(close) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "no XdndPosition at Xwayland's {xwayland:?}: the last at {last:?}"
            );
            drag.round();
        }
    }
    drag.release();
}

/// An X11 window refuses a Wayland drag in every status: the Wayland source is never told a
/// type is accepted; at the release Yuna sends `XdndLeave`, not `XdndDrop`, and the source is
/// cancelled, never told the drop was performed.
#[test]
fn a_wayland_drag_an_x11_window_refuses_is_cancelled_at_the_release() {
    let mut drag = DragSeat::new("dnd-w-refused");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| TargetSpec {
        answer: Answer::Refuse,
        ..TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"refused");
    move_into(&mut drag, onto);
    x11_got(
        &mut drag,
        &target,
        "the X11 target's XdndPosition",
        |message| matches!(message, Xdnd::Position { .. }).then_some(()),
    );
    drag.release();
    x11_got(
        &mut drag,
        &target,
        "the X11 target's XdndLeave",
        |message| is_leave(message).then_some(()),
    );
    let events = wayland_source_ends(&mut drag, source);
    assert_cancelled(&events);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SourceDragEvent::Target(Some(_)))),
        "never accepted: {events:?}"
    );
    assert!(!target.got().iter().any(is_drop));
}

/// A Wayland drag held over an X11 window that is XDND-aware at `aware` (none: not at all), then
/// released there: Yuna sends it nothing, and the source is cancelled, never told the drop was
/// performed.
#[track_caller]
fn a_wayland_drag_over_an_x11_window_xdnd_cannot_reach_is_cancelled(
    story: &str,
    aware: Option<u32>,
) {
    let mut drag = DragSeat::new(story);
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| TargetSpec {
        aware,
        ..TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"unaware");
    move_into(&mut drag, onto);
    drag.never("the X11 window gets an XDND message", QUIET, |_| {
        !target.got().is_empty()
    });
    drag.release();
    let events = wayland_source_ends(&mut drag, source);
    assert_cancelled(&events);
    assert!(target.got().is_empty(), "{:?}", target.got());
}

/// A Wayland drag over an X11 window with no `XdndAware`: nothing sent, cancelled.
#[test]
fn a_wayland_drag_over_an_x11_window_without_xdnd_is_cancelled() {
    a_wayland_drag_over_an_x11_window_xdnd_cannot_reach_is_cancelled("dnd-w-unaware", None);
}

/// A Wayland drag over an X11 window aware at version 2, below XDND's 3: nothing sent,
/// cancelled.
#[test]
fn a_wayland_drag_over_an_x11_window_at_xdnd_2_is_cancelled() {
    a_wayland_drag_over_an_x11_window_xdnd_cannot_reach_is_cancelled("dnd-w-v2", Some(2));
}

/// A Wayland drag onto an X11 window aware at version 3: Yuna speaks version 3, and the
/// target's `XdndFinished`, which carries no more than its window at that version, completes the
/// drop: performed, finished, a copy.
#[test]
fn a_wayland_drag_onto_an_x11_window_at_xdnd_3_completes() {
    let mut drag = DragSeat::new("dnd-w-v3");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| TargetSpec {
        aware: Some(3),
        ..TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let text = b"at version 3";
    let source = wayland_drag_onto_x11(&mut drag, &target, (from, onto), &[TEXT], text);
    let events = wayland_source_ends(&mut drag, source);
    assert_completed(&events, DndAction::Copy);
    assert!(
        target
            .got()
            .iter()
            .any(|message| matches!(message, Xdnd::Enter { version: 3, .. })),
        "{:?}",
        target.got()
    );
    let utf8_string = target.atoms.UTF8_STRING;
    assert_eq!(
        x11_reads(&mut drag, &target),
        [Some((utf8_string, text.to_vec()))]
    );
}

/// An X11 window names, in `XdndProxy`, a window of another X11 program's whose own
/// `XdndProxy` names itself, the window under the pointer having no `XdndAware` of its own:
/// Yuna sends every message to the proxy, naming the window under the pointer; the proxy's
/// program reads the drop, and the drop completes. The window's own program gets no message.
#[test]
fn a_wayland_drag_honours_an_x11_windows_xdnd_proxy() {
    let mut drag = DragSeat::new("dnd-w-proxy");
    let (shown_client, shown, onto) = drag.open_x11("shown");
    let mut proxy_client = drag.seat.client();
    let proxy = proxy_client
        .create(&WindowSpec::named("proxy"))
        .expect("the proxy");
    let atoms = xdnd_atoms(&proxy_client).expect("the XDND atoms");
    let target = XdndTarget::start(
        &mut drag.seat.compositor,
        proxy_client,
        proxy,
        TargetSpec {
            proxy_of_itself: true,
            ..TargetSpec::copying(&atoms, atoms.UTF8_STRING)
        },
    );
    let (shown_client, ()) = step(&mut drag.seat.compositor, shown_client, move |client| {
        set_proxy(client, &atoms, shown, proxy)
    });
    let from = drag.open_wayland();
    let text = b"through the proxy";
    let source = wayland_drag_onto_x11(&mut drag, &target, (from, onto), &[TEXT], text);
    let events = wayland_source_ends(&mut drag, source);
    assert_completed(&events, DndAction::Copy);
    assert_eq!(
        x11_reads(&mut drag, &target),
        [Some((atoms.UTF8_STRING, text.to_vec()))]
    );
    let named: Vec<Window> = target
        .log()
        .into_iter()
        .filter_map(|logged| match logged {
            Logged::Got { window, .. } => Some(window),
            _ => None,
        })
        .collect();
    assert!(
        !named.is_empty() && named.iter().all(|&window| window == shown),
        "each message names the window under the pointer: {named:?}"
    );
    let (_shown_client, events) = step(&mut drag.seat.compositor, shown_client, |client| {
        client.take_events()
    });
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::ClientMessage(_))),
        "the window's own program gets no message"
    );
}

/// A Wayland program destroys its drag's source over an X11 window: Yuna sends the window
/// `XdndLeave`, and no drop follows the release.
#[test]
fn a_wayland_source_destroyed_mid_drag_leaves_the_x11_window() {
    let mut drag = DragSeat::new("dnd-w-src-gone");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"destroyed");
    move_into(&mut drag, onto);
    x11_got(
        &mut drag,
        &target,
        "the X11 target's XdndEnter",
        |message| matches!(message, Xdnd::Enter { .. }).then_some(()),
    );
    drag.wl.destroy_source(source);
    drag.wl.client.send();
    x11_got(
        &mut drag,
        &target,
        "the X11 target's XdndLeave",
        |message| is_leave(message).then_some(()),
    );
    drag.release();
    drag.never("the X11 window is dropped on", QUIET, |_| {
        target.got().iter().any(is_drop)
    });
}

/// The Wayland program that drags over an X11 window is gone mid-drag: Yuna sends the window
/// `XdndLeave`, and no drop follows the release.
#[test]
fn a_wayland_source_whose_program_is_gone_mid_drag_leaves_the_x11_window() {
    let mut drag = DragSeat::new("dnd-w-wl-gone");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"gone");
    move_into(&mut drag, onto);
    x11_got(
        &mut drag,
        &target,
        "the X11 target's XdndEnter",
        |message| matches!(message, Xdnd::Enter { .. }).then_some(()),
    );
    let seat = drag.seat.seat;
    let replacement = SelectionClient::connect_to(&mut drag.seat.compositor, seat);
    drop(std::mem::replace(&mut drag.wl, replacement));
    drag.answers.clear();
    x11_got(
        &mut drag,
        &target,
        "the X11 target's XdndLeave",
        |message| is_leave(message).then_some(()),
    );
    drag.release();
    drag.never("the X11 window is dropped on", QUIET, |_| {
        target.got().iter().any(is_drop)
    });
}

/// The X11 program under a Wayland drag exits mid-drag, the button held: the drag goes on, its
/// source not cancelled while the button is held; at the release it is cancelled, never told
/// the drop was performed.
#[test]
fn an_x11_target_that_exits_mid_drag_cancels_the_wayland_source_at_the_release() {
    let mut drag = DragSeat::new("dnd-w-x11-exits");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"exits");
    move_into(&mut drag, onto);
    x11_got(
        &mut drag,
        &target,
        "the X11 target's XdndEnter",
        |message| matches!(message, Xdnd::Enter { .. }).then_some(()),
    );
    target.exit();
    drag.never(
        "the source is cancelled while the button is held",
        QUIET,
        |drag| {
            drag.source_events(source)
                .contains(&SourceDragEvent::Cancelled)
        },
    );
    drag.release();
    let events = wayland_source_ends(&mut drag, source);
    assert_cancelled(&events);
}

/// While a Wayland drag is over an X11 window, another X11 program converts `XdndSelection`:
/// Yuna refuses it, its Wayland source never asked; at the drop, the window's own program reads
/// it.
#[test]
fn xdnd_selection_read_by_another_x11_program_than_the_targets_is_refused() {
    let mut drag = DragSeat::new("dnd-w-other-read");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let mut other = drag.seat.client();
    let reader = other
        .create(&WindowSpec::named("reader"))
        .expect("a window");
    let text = b"for the target alone";
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, text);
    move_into(&mut drag, onto);
    wayland_source_told(&mut drag, source, DndAction::Copy);
    let (_other, read) = step(&mut drag.seat.compositor, other, move |other| {
        let atoms = xdnd_atoms(other)?;
        paste(other, reader, atoms.XdndSelection, atoms.UTF8_STRING)
    });
    assert_eq!(read, None, "refused");
    drag.round();
    assert!(
        drag.wl.source(source).asked.is_empty(),
        "the source is never asked"
    );
    drag.release();
    let events = wayland_source_ends(&mut drag, source);
    assert_completed(&events, DndAction::Copy);
    let utf8_string = target.atoms.UTF8_STRING;
    assert_eq!(
        x11_reads(&mut drag, &target),
        [Some((utf8_string, text.to_vec()))]
    );
}

/// Another X11 program takes `XdndSelection` while a Wayland drag is over an X11 window, the
/// button held: Yuna sends the window `XdndLeave`, and the drag goes on, its source not cancelled
/// while the button is held; at the release the source is cancelled, and the window never gets
/// `XdndDrop`.
#[test]
fn xdnd_selection_taken_mid_drag_cancels_the_wayland_drag_at_the_release() {
    let mut drag = DragSeat::new("dnd-w-stolen");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"stolen");
    move_into(&mut drag, onto);
    x11_got(
        &mut drag,
        &target,
        "the X11 target's XdndEnter",
        |message| matches!(message, Xdnd::Enter { .. }).then_some(()),
    );
    let other = drag.seat.client();
    step(&mut drag.seat.compositor, other, |other| {
        let atoms = xdnd_atoms(other)?;
        take_selection(other, atoms.XdndSelection)
    });
    x11_got(
        &mut drag,
        &target,
        "the X11 target's XdndLeave",
        |message| is_leave(message).then_some(()),
    );
    drag.never(
        "the source is cancelled while the button is held",
        QUIET,
        |drag| {
            drag.source_events(source)
                .contains(&SourceDragEvent::Cancelled)
        },
    );
    drag.release();
    let events = wayland_source_ends(&mut drag, source);
    assert_cancelled(&events);
    drag.never("the X11 window gets XdndDrop", QUIET, |_| {
        target.got().iter().any(is_drop)
    });
}

/// An X11 window reads a Wayland drop and never sends `XdndFinished`: the Wayland source is
/// cancelled once the time limit passed with no transfer moving, not before it and not long
/// after, never told the drop was performed.
#[test]
fn an_x11_target_that_never_finishes_cancels_the_wayland_source_in_time() {
    let mut drag = DragSeat::new("dnd-w-unfinished");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| TargetSpec {
        finish: Finish::Never,
        ..TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let text = b"never finished";
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, text);
    move_into(&mut drag, onto);
    wayland_source_told(&mut drag, source, DndAction::Copy);
    drag.release();
    let released = Instant::now();
    let utf8_string = target.atoms.UTF8_STRING;
    assert_eq!(
        x11_reads(&mut drag, &target),
        [Some((utf8_string, text.to_vec()))]
    );
    let events = wayland_source_ends(&mut drag, source);
    let took = released.elapsed();
    assert_cancelled(&events);
    assert!(
        took >= DRAG_TIME_LIMIT,
        "not before the time limit: {took:?}"
    );
    assert!(
        took < DRAG_TIME_LIMIT + MARGIN,
        "within the time limit: {took:?}"
    );
}

/// On the manual clock, which X11's times must never come from, a Wayland drag dropped on an
/// X11 window: its positions and its drop name one X11 time; the window's conversion, named with
/// the drop's time, is served, and the drop completes.
#[test]
fn a_wayland_drop_on_x11_is_served_on_the_manual_clock() {
    let seat = X11Seat::new("dnd-w-manual", &[at_1_5()], ClockMode::Manual, off());
    let mut drag = DragSeat::on(seat);
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let text = b"on the manual clock";
    let source = wayland_drag_onto_x11(&mut drag, &target, (from, onto), &[TEXT], text);
    let events = wayland_source_ends(&mut drag, source);
    assert_completed(&events, DndAction::Copy);
    assert_one_time(&target);
    let utf8_string = target.atoms.UTF8_STRING;
    assert_eq!(
        x11_reads(&mut drag, &target),
        [Some((utf8_string, text.to_vec()))]
    );
}

/// A Wayland program drags within itself, with no source, over an X11 window: Yuna offers the
/// X11 window nothing, and takes no `XdndSelection` for it. Passes before XDND reaches Wayland:
/// it guards that a drag with no data stays its client's.
#[test]
fn a_wayland_drag_with_no_source_offers_x11_nothing() {
    let mut drag = DragSeat::new("dnd-w-local");
    let (target, onto, (from, index)) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let spot = drag.point_in(from, (0.5, 0.5));
    drag.move_to(spot);
    let presses = drag.wl.state.presses.len();
    drag.press();
    drag.until("the Wayland program gets the press", |drag| {
        (drag.wl.state.presses.len() > presses).then_some(())
    });
    let origin = drag.wl.client.window(index).surface.clone();
    let serial = drag.wl.press_serial();
    drag.wl.start_local_drag(&origin, serial);
    drag.wl.client.send();
    move_into(&mut drag, onto);
    drag.never("the X11 window gets an XDND message", QUIET, |_| {
        !target.got().is_empty()
    });
    let other = drag.seat.client();
    let (_other, owner) = step(&mut drag.seat.compositor, other, |other| {
        let atoms = xdnd_atoms(other)?;
        owner_of(other, atoms.XdndSelection)
    });
    assert_eq!(owner, x11rb::NONE, "XdndSelection has no owner");
    drag.release();
}

/// A Wayland drag is dropped on an X11 window whose slow program reads only later; meanwhile a
/// second Wayland drag goes over another X11 window, for which Yuna takes `XdndSelection` again.
/// The first window's late read, which names its drop's time, is refused, neither source asked
/// for it; the second drop then arrives whole and completes.
#[test]
fn an_old_targets_late_read_is_refused_after_a_new_drag_took_the_selection() {
    let mut drag = DragSeat::on(X11Seat::at_both("dnd-w-late-read"));
    let (at_one_and_a_half, at_one) = (drag.seat.outputs[0], drag.seat.outputs[1]);
    drag.seat.open_on(at_one);
    let from = drag.open_wayland();
    drag.seat.open_on(at_one_and_a_half);
    let (slow_client, slow_window, slow_onto) = drag.open_x11("slow");
    let atoms = xdnd_atoms(&slow_client).expect("the XDND atoms");
    let slow = XdndTarget::start(
        &mut drag.seat.compositor,
        slow_client,
        slow_window,
        TargetSpec {
            reads_on_order: true,
            ..TargetSpec::copying(&atoms, atoms.UTF8_STRING)
        },
    );
    let (second_client, second_window, second_onto) = drag.open_x11("second");
    let second = XdndTarget::start(
        &mut drag.seat.compositor,
        second_client,
        second_window,
        TargetSpec::copying(&atoms, atoms.UTF8_STRING),
    );

    let first_source =
        wayland_drag_onto_x11(&mut drag, &slow, (from, slow_onto), &[TEXT], b"first");
    x11_got(&mut drag, &slow, "the slow target's XdndDrop", |message| {
        is_drop(message).then_some(())
    });
    let second_source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"second");
    move_into(&mut drag, second_onto);
    x11_got(
        &mut drag,
        &second,
        "the second target's XdndEnter",
        |message| matches!(message, Xdnd::Enter { .. }).then_some(()),
    );
    slow.order(TargetOrder::Read);
    assert_eq!(
        x11_reads(&mut drag, &slow),
        [None],
        "the old drop's read is refused"
    );
    assert!(
        drag.wl.source(first_source).asked.is_empty()
            && drag.wl.source(second_source).asked.is_empty(),
        "neither source is asked for the old read"
    );
    wayland_source_told(&mut drag, second_source, DndAction::Copy);
    drag.release();
    let events = wayland_source_ends(&mut drag, second_source);
    assert_completed(&events, DndAction::Copy);
    assert_eq!(
        x11_reads(&mut drag, &second),
        [Some((atoms.UTF8_STRING, b"second".to_vec()))]
    );
}

/// A Wayland drag dropped on an X11 window that takes it: once the drop is done, the seat's
/// pointer, which has not moved since, is on the X11 window, and a click there with no motion
/// between reaches the X11 program. Smithay's drag grab, which never targets an X11 window,
/// leaves the pointer on none at its end; Yuna puts it back on what is under it.
#[test]
fn after_a_wayland_drop_on_an_x11_window_a_click_there_reaches_it() {
    let mut drag = DragSeat::new("dnd-w-click-after");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let source = wayland_drag_onto_x11(&mut drag, &target, (from, onto), &[TEXT], b"then a click");
    let events = wayland_source_ends(&mut drag, source);
    assert_completed(&events, DndAction::Copy);
    let seat = drag.seat.seat;
    assert_eq!(
        drag.seat.compositor.pointer(seat).window,
        Some(onto),
        "the pointer is on the X11 window"
    );
    drag.press();
    drag.release();
    drag.until("the X11 program gets the click", |_| {
        target
            .log()
            .iter()
            .any(|logged| matches!(logged, Logged::Clicked { window } if *window == target.window))
            .then_some(())
    });
}

/// A Wayland program drags text offering Copy alone onto an X11 window that answers Move and
/// performs a move, its `DELETE` included: the Wayland source is told Copy, the one action it
/// offered, at the X11 window's answers and at the end, never Move, so that it keeps its data.
#[test]
fn a_copy_only_wayland_source_moved_by_an_x11_window_is_told_copy() {
    let mut drag = DragSeat::new("dnd-w-copy-only");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| TargetSpec {
        answer: Answer::Accept(atoms.XdndActionMove),
        deletes: true,
        ..TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let text = b"copied, not moved";
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, text);
    move_into(&mut drag, onto);
    let accepted = SourceDragEvent::Target(Some(TEXT.to_owned()));
    drag.until(
        "the Wayland source is told the X11 window accepts",
        |drag| drag.source_events(source).contains(&accepted).then_some(()),
    );
    drag.release();
    let events = wayland_source_ends(&mut drag, source);
    assert!(
        !events.contains(&SourceDragEvent::Action(DndAction::Move.bits())),
        "never told Move: {events:?}"
    );
    assert_completed(&events, DndAction::Copy);
    let atoms = target.atoms;
    let reads = x11_reads(&mut drag, &target);
    assert_eq!(
        reads.first(),
        Some(&Some((atoms.UTF8_STRING, text.to_vec())))
    );
}

/// A Wayland program starts a second drag in the same press, then the button is released over
/// its own window, which takes neither: each source is cancelled once, never dropped; then the
/// pointer, no button held, moves onto an XDND-aware X11 window, which gets no XDND message, the
/// pointer on it as on any window. A guard: Smithay ends the first drag's grab when the second
/// starts, and Yuna follows each grab's end to its own drag.
#[test]
fn a_second_wayland_drag_in_one_press_ends_both_and_leaves_x11_alone() {
    let mut drag = DragSeat::new("dnd-w-second-in-press");
    let (target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let first = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"first");
    let origin = drag.wl.client.window(from.1).surface.clone();
    let serial = drag.wl.press_serial();
    let second = drag
        .wl
        .start_drag_for(&[TEXT], Some(DndAction::Copy), &origin, serial);
    drag.answers.push((second, b"second".to_vec()));
    drag.wl.client.send();
    drag.round();
    drag.release();
    for source in [first, second] {
        let events = wayland_source_ends(&mut drag, source);
        assert_cancelled(&events);
        drag.never("a source is cancelled twice", QUIET, |drag| {
            drag.source_events(source)
                .iter()
                .filter(|event| **event == SourceDragEvent::Cancelled)
                .count()
                > 1
        });
    }
    move_into(&mut drag, onto);
    drag.never("the X11 window gets an XDND message", QUIET, |_| {
        !target.got().is_empty()
    });
    let seat = drag.seat.seat;
    assert_eq!(
        drag.seat.compositor.pointer(seat).window,
        Some(onto),
        "the pointer is on the X11 window"
    );
}

// Both ways.

/// A Wayland drag is dropped on an X11 window that holds its `XdndFinished`; a second Wayland
/// drag starts, onto another X11 window, before the first is finished: the first's late
/// `XdndFinished` completes the first source alone, and the second drop completes the second.
#[test]
fn a_second_wayland_drag_before_the_firsts_xdnd_finished_ends_both_right() {
    let mut drag = DragSeat::on(X11Seat::at_both("dnd-w-two"));
    let (at_one_and_a_half, at_one) = (drag.seat.outputs[0], drag.seat.outputs[1]);
    drag.seat.open_on(at_one);
    let from = drag.open_wayland();
    drag.seat.open_on(at_one_and_a_half);
    let (first_client, first_window, first_onto) = drag.open_x11("first");
    let atoms = xdnd_atoms(&first_client).expect("the XDND atoms");
    let first = XdndTarget::start(
        &mut drag.seat.compositor,
        first_client,
        first_window,
        TargetSpec {
            finish: Finish::OnOrder,
            ..TargetSpec::copying(&atoms, atoms.UTF8_STRING)
        },
    );
    let (second_client, second_window, second_onto) = drag.open_x11("second");
    let second = XdndTarget::start(
        &mut drag.seat.compositor,
        second_client,
        second_window,
        TargetSpec::copying(&atoms, atoms.UTF8_STRING),
    );

    let first_source =
        wayland_drag_onto_x11(&mut drag, &first, (from, first_onto), &[TEXT], b"first");
    let first_dropped = Instant::now();
    assert_eq!(
        x11_reads(&mut drag, &first),
        [Some((atoms.UTF8_STRING, b"first".to_vec()))]
    );
    let second_source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"second");
    move_into(&mut drag, second_onto);
    wayland_source_told(&mut drag, second_source, DndAction::Copy);
    assert!(
        drag.source_events(first_source)
            .iter()
            .all(|event| !matches!(
                event,
                SourceDragEvent::Finished | SourceDragEvent::Cancelled
            )),
        "the first waits for its XdndFinished: {:?}",
        drag.source_events(first_source)
    );

    first.order(TargetOrder::Finish);
    let held = first_dropped.elapsed();
    let events = wayland_source_ends(&mut drag, first_source);
    assert!(
        !events.contains(&SourceDragEvent::Cancelled),
        "the first drag was cancelled: its XdndFinished came {held:?} after its drop, against \
         the {DRAG_TIME_LIMIT:?} limit, which a loaded machine may have raced: {events:?}"
    );
    assert_completed(&events, DndAction::Copy);
    let second_events = drag.source_events(second_source);
    assert!(
        !second_events.iter().any(|event| matches!(
            event,
            SourceDragEvent::DropPerformed | SourceDragEvent::Finished | SourceDragEvent::Cancelled
        )),
        "the second goes on: {second_events:?}"
    );
    drag.release();
    let events = wayland_source_ends(&mut drag, second_source);
    assert_completed(&events, DndAction::Copy);
    assert_eq!(
        x11_reads(&mut drag, &second),
        [Some((atoms.UTF8_STRING, b"second".to_vec()))]
    );
}

/// An X11 drag is dropped on a Wayland window that holds off its finish; a second X11 program's
/// drag starts before the first is finished: the Wayland window's late finish of the first
/// offer reaches the first program alone, and the second drop completes the second.
#[test]
fn a_second_x11_drag_before_the_firsts_finish_ends_both_right() {
    let mut drag = DragSeat::on(X11Seat::at_both("dnd-x-two"));
    let (at_one_and_a_half, at_one) = (drag.seat.outputs[0], drag.seat.outputs[1]);
    drag.seat.open_on(at_one);
    let (onto, _) = drag.open_wayland();
    drag.seat.open_on(at_one_and_a_half);
    let mut sources = Vec::new();
    let mut windows = Vec::new();
    for (title, text) in [("first", b"first".to_vec()), ("second", b"second".to_vec())] {
        let (client, window, id) = drag.open_x11(title);
        let atoms = xdnd_atoms(&client).expect("the XDND atoms");
        let spec = SourceSpec::offering(vec![(atoms.UTF8_STRING, text)], atoms.XdndActionCopy);
        sources.push(XdndSource::start(
            &mut drag.seat.compositor,
            client,
            window,
            spec,
        ));
        windows.push(id);
    }
    let [first, second] = &sources[..] else {
        panic!("two sources");
    };
    let copy = first.atoms.XdndActionCopy;

    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, first, windows[0]);
    move_into(&mut drag, onto);
    let first_entered = wayland_entered(&mut drag, since);
    wayland_accepts(&mut drag, first_entered, TEXT, DndAction::Copy);
    move_until_accepted(&mut drag, first, onto);
    drag.release();
    wayland_dropped(&mut drag, since);
    let first_dropped = Instant::now();
    assert_eq!(wayland_reads(&mut drag, first_entered.0, TEXT), b"first");

    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, second, windows[1]);
    move_into(&mut drag, onto);
    let second_entered = wayland_entered(&mut drag, since);
    assert_ne!(first_entered.0, second_entered.0, "a new offer");
    wayland_accepts(&mut drag, second_entered, TEXT, DndAction::Copy);
    drag.wl.finish(first_entered.0);
    drag.wl.client.send();
    let held = first_dropped.elapsed();
    assert_eq!(
        x11_finished(&mut drag, first),
        (true, copy),
        "the Wayland window finished the first {held:?} after its drop, against the \
         {DRAG_TIME_LIMIT:?} limit, which a loaded machine may have raced"
    );
    assert!(
        !second
            .got()
            .iter()
            .any(|message| matches!(message, Xdnd::Finished { .. })),
        "the second goes on: {:?}",
        second.got()
    );

    move_until_accepted(&mut drag, second, onto);
    drag.release();
    wayland_dropped(&mut drag, since);
    assert_eq!(wayland_reads(&mut drag, second_entered.0, TEXT), b"second");
    drag.wl.finish(second_entered.0);
    drag.wl.client.send();
    assert_eq!(x11_finished(&mut drag, second), (true, copy));
    let first_finishes = first
        .got()
        .iter()
        .filter(|message| matches!(message, Xdnd::Finished { .. }))
        .count();
    assert_eq!(first_finishes, 1, "the first is finished once");
}

/// Kills the seat's Xwayland, as a crash would end it.
fn kill_xwayland(drag: &mut DragSeat) {
    let pid = drag
        .seat
        .compositor
        .x11_display(drag.seat.seat)
        .and_then(|display| display.server)
        .expect("Xwayland runs");
    kill_process(Pid::from_raw(pid).expect("a process id"), Signal::KILL)
        .expect("Xwayland is killed");
}

/// Xwayland is killed while an X11 program's drag is over a Wayland window that accepted it:
/// the Wayland window is left, and never dropped on, at the release or after. A guard of the
/// drag's end with its Xwayland.
#[test]
fn xwayland_killed_during_an_x11_drag_leaves_the_wayland_window_undropped() {
    let (mut drag, source, from, onto) = x11_source_and_wayland_window("dnd-x-killed", |atoms| {
        SourceSpec::offering(
            vec![(atoms.UTF8_STRING, b"killed".to_vec())],
            atoms.XdndActionCopy,
        )
    });
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    let entered = wayland_entered(&mut drag, since);
    wayland_accepts(&mut drag, entered, TEXT, DndAction::Copy);
    move_until_accepted(&mut drag, &source, onto);
    kill_xwayland(&mut drag);
    wayland_left(&mut drag, since);
    drag.release();
    let spot = drag.point_in(onto, (0.3, 0.3));
    drag.move_to(spot);
    drag.never("the Wayland window is dropped on", QUIET, |drag| {
        drag.drags_since(since).contains(&DragEvent::Drop)
    });
}

/// Xwayland is killed while a Wayland drag is over an X11 window that accepted it; at the
/// release, the Wayland source is cancelled, never told its drop was performed. A guard of the
/// drag's end with its Xwayland.
#[test]
fn xwayland_killed_during_a_wayland_drag_over_x11_cancels_it_at_the_release() {
    let mut drag = DragSeat::new("dnd-w-killed");
    let (_target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"killed");
    move_into(&mut drag, onto);
    wayland_source_told(&mut drag, source, DndAction::Copy);
    kill_xwayland(&mut drag);
    let seat = drag.seat.seat;
    drag.until("Xwayland is reaped", |drag| {
        drag.seat
            .compositor
            .x11_display(seat)
            .is_some_and(|display| display.server.is_none())
            .then_some(())
    });
    drag.release();
    assert_cancelled(&wayland_source_ends(&mut drag, source));
}

/// Removes the seat in the middle of a drag, then runs Yuna's loop a while: its state stays
/// sound, and no drag of the seat is left waiting on Yuna's clock.
fn seat_removed_mid_drag(mut drag: DragSeat) {
    let seat = drag.seat.seat;
    drag.seat
        .compositor
        .remove_seat(seat)
        .expect("the seat is removed");
    let compositor = &mut drag.seat.compositor;
    let end = Instant::now() + QUIET;
    while Instant::now() < end {
        pump(compositor);
    }
    compositor
        .layout()
        .check_invariants()
        .expect("the layout keeps its invariants");
    compositor
        .check_windows()
        .expect("the layout and the shells agree");
    compositor
        .check_popups_and_keyboard()
        .expect("the popups and the keyboard are sound");
    wait(compositor, "no deadline is left", |compositor| {
        compositor.next_deadline().is_none().then_some(())
    });
}

/// The seat is removed while a Wayland drag is over an X11 window that accepted it: Yuna goes
/// on, sound. A guard of the drag's end with its seat.
#[test]
fn the_seat_removed_during_a_wayland_drag_over_x11_leaves_yuna_sound() {
    let mut drag = DragSeat::new("dnd-w-seat-gone");
    let (_target, onto, from) = x11_target_and_wayland_window(&mut drag, |atoms| {
        TargetSpec::copying(atoms, atoms.UTF8_STRING)
    });
    let source = wayland_drag_starts(&mut drag, from, &[TEXT], DndAction::Copy, b"seat gone");
    move_into(&mut drag, onto);
    wayland_source_told(&mut drag, source, DndAction::Copy);
    seat_removed_mid_drag(drag);
}

/// The seat is removed while an X11 program's drag is over a Wayland window that accepted it:
/// Yuna goes on, sound. A guard of the drag's end with its seat.
#[test]
fn the_seat_removed_during_an_x11_drag_over_wayland_leaves_yuna_sound() {
    let (mut drag, source, from, onto) =
        x11_source_and_wayland_window("dnd-x-seat-gone", |atoms| {
            SourceSpec::offering(
                vec![(atoms.UTF8_STRING, b"seat gone".to_vec())],
                atoms.XdndActionCopy,
            )
        });
    let since = drag.wl.state.drags.len();
    press_on_source(&mut drag, &source, from);
    move_into(&mut drag, onto);
    let entered = wayland_entered(&mut drag, since);
    wayland_accepts(&mut drag, entered, TEXT, DndAction::Copy);
    move_until_accepted(&mut drag, &source, onto);
    seat_removed_mid_drag(drag);
}

/// Moves the seat's pointer to `spot` and releases its button in the same round of Yuna's loop,
/// as MCP's `release` given x and y does, or a hand that lets go as it arrives.
fn move_and_release_at_once(drag: &mut DragSeat, (output, x, y): Spot) {
    let seat = drag.seat.seat;
    drag.seat
        .compositor
        .pointer_move(seat, Some(output), x, y)
        .expect("the pointer moves");
    drag.seat
        .compositor
        .pointer_button(seat, BUTTON_LEFT, false)
        .expect("released");
    drag.round();
}

/// The window another X11 program finds at the root point under the seat's pointer, as an XDND
/// source finds its target there, through the window tree: none over no window aware of XDND.
fn x11_target_under_pointer(
    drag: &mut DragSeat,
    other: X11Client,
    atoms: XdndAtoms,
) -> (X11Client, Option<Window>) {
    step(&mut drag.seat.compositor, other, move |other| {
        let pointer = other
            .connection
            .query_pointer(other.root)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let found = target_at(other, &atoms, (pointer.root_x, pointer.root_y), None);
        Ok(found.map(|found| found.window))
    })
}

/// An X11 program's drag moved from its window onto a Wayland window in one motion and released
/// in the same round of Yuna's loop, as MCP's `release` given a place does, the manager idle.
/// Under the Wayland window, in X11's stacking, lies an X11 window of another workspace, hidden
/// and aware of XDND: Yuna keeps a hidden window mapped at its X11 place. The proxy Yuna shows
/// for the motion stays at the release, since the source has not looked for its target yet.
///
/// The source reads none of its events from before the motion until another X11 program finds
/// the proxy at the release's place, and still finds it there after [`QUIET`] of Yuna's rounds,
/// well within the five seconds Yuna waits for the source: it then sees its motion and its
/// release, finds the proxy, enters it, and leaves it, Yuna having no Wayland target for a drag
/// entered after its release. The hidden window gets nothing.
///
/// Not covered, and kept as debt (`docs/debt.md`): the release's round passes the manager its
/// order to map the proxy, and the source's thread may look before the manager's has carried it
/// out. A source that looks that soon finds the hidden window and drops into it. This story's
/// source, unheld, sleeps a millisecond when it has no event, and lost that race about 1 time in
/// 12 on a quiet machine (11 passes in 12 runs at a load of 1.4 to 2.6, 2026-11-18); a toolkit
/// that blocks on its socket looks sooner, and would lose it more often.
#[test]
fn an_x11_drag_released_onto_a_wayland_window_never_drops_into_a_hidden_window_idle() {
    let mut drag = DragSeat::new("dnd-hidden-idle");
    let seat = drag.seat.seat;
    let (hidden_client, hidden_window, hidden_id) = drag.open_x11("hidden-target");
    let atoms = xdnd_atoms(&hidden_client).expect("the XDND atoms");
    let hidden = XdndTarget::start(
        &mut drag.seat.compositor,
        hidden_client,
        hidden_window,
        TargetSpec::copying(&atoms, atoms.UTF8_STRING),
    );
    drag.seat
        .compositor
        .change_layout(|layout| layout.move_to_workspace(seat, "two"))
        .expect("its own workspace");
    drag.seat
        .compositor
        .change_layout(|layout| layout.focus_workspace(seat, "one"))
        .expect("the first workspace again");
    drag.until("the X11 window is hidden", |drag| {
        drag.seat
            .compositor
            .windows()
            .into_iter()
            .find(|window| window.id == hidden_id && window.output.is_none())
    });
    let (wayland, _) = drag.open_wayland();
    let (client, window, from) = drag.open_x11("source");
    let spec = SourceSpec::offering(
        vec![(atoms.UTF8_STRING, b"meant for the Wayland window".to_vec())],
        atoms.XdndActionCopy,
    );
    let source = XdndSource::start(&mut drag.seat.compositor, client, window, spec);
    press_on_source(&mut drag, &source, from);
    source.order(SourceOrder::Hold);
    drag.until("the source holds its events", |_| {
        source.held().then_some(())
    });

    let spot = drag.point_in(wayland, (0.5, 0.5));
    move_and_release_at_once(&mut drag, spot);
    let mut other = drag.seat.client();
    let deadline = Instant::now() + DRAG_TIME_LIMIT;
    let proxy = loop {
        let (back, found) = x11_target_under_pointer(&mut drag, other, atoms);
        other = back;
        if let Some(found) = found.filter(|&found| found != hidden_window) {
            break found;
        }
        assert!(
            Instant::now() < deadline,
            "no X11 program finds Yuna's proxy where the drag was released, but {found:?}: \
             it was hidden at the release"
        );
        drag.round();
    };
    drag.settle(QUIET);
    let (_other, found) = x11_target_under_pointer(&mut drag, other, atoms);
    assert_eq!(
        found,
        Some(proxy),
        "the proxy stays where the release left it"
    );

    let logged = source.log().len();
    source.order(SourceOrder::Go);
    let ended = |source: &XdndSource| {
        source.sent()[..]
            .iter()
            .any(|message| is_leave(message) || is_drop(message))
    };
    let deadline = Instant::now() + DRAG_TIME_LIMIT + MARGIN;
    while Instant::now() < deadline && !ended(&source) {
        drag.round();
    }
    let sent: Vec<Logged> = source.log()[logged..].to_vec();
    assert!(ended(&source), "the source never ended its drag: {sent:#?}");
    drag.never(
        "a hidden X11 window got the drop meant for the Wayland window over it",
        QUIET,
        |_| hidden.got().iter().any(is_drop) || !hidden.reads().is_empty(),
    );
    let entered: Vec<Window> = sent
        .iter()
        .filter_map(|logged| match logged {
            Logged::Sent {
                to,
                message: Xdnd::Enter { .. },
                ..
            } => Some(*to),
            _ => None,
        })
        .collect();
    assert_eq!(entered, [proxy], "the source entered the proxy alone");
    assert_eq!(
        hidden.got(),
        [],
        "the hidden X11 window was never spoken to"
    );
}
