//! XDND, version 5, for the X11 test client: an X11 program that drags ([`XdndSource`]) and one
//! that takes drops ([`XdndTarget`]), each as a toolkit's does. The protocol is freedesktop.org's
//! *XDND* specification: client messages between a source window and a target window, the data
//! read through the `XdndSelection` selection. As Qt's and GTK 4's, by default the source owns
//! `XdndSelection` and names itself by a hidden window of its program, not the pressed one, and
//! answers a request only at the time XDND gave it; the target reads into a hidden window of its
//! program, as Qt's does.
//!
//! Each runs on a thread of its own, with its own connection, for as long as the story keeps it:
//! its loop reads its events as they come, and the orders the story sends it between them. The
//! story's thread meanwhile runs the compositor and drives the seat's pointer. Each records
//! every XDND message it sends and gets, every request for `XdndSelection` it answers, every
//! conversion it makes, and the target every click on its window, in order, in a log the story
//! reads ([`Peer::log`]). A failure of its own, such as an X11 request that fails, ends its
//! thread and is the log's last entry; the story's next read of the log panics with it.
//!
//! [`named_window_at`] finds the window under a point as Chromium's drag does, among named
//! windows alone.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux,
    ConnectionExt as _, CreateWindowAux, EventMask, GrabMode, GrabStatus, MapState, PropMode,
    Property, PropertyNotifyEvent, SelectionRequestEvent, Timestamp, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;
use yuna::Compositor;

use crate::x11_client::{X11Client, step};
use crate::x11_selections::{CHUNK, Pasted, notify, paste_at};

/// The version of XDND the source and the target speak.
const VERSION: u32 = 5;

/// How long the source waits at the release for the status of its last `XdndPosition` before it
/// leaves: XDND asks a source to give up on a target that does not answer in a reasonable time.
const STATUS_PATIENCE: Duration = Duration::from_secs(5);

/// Where the drag icon's corner is from the pointer: the pointer 8 pixels into it, as a toolkit
/// puts an icon's hot spot inside it.
const ICON_HOT_SPOT: i16 = 8;

x11rb::atom_manager! {
    /// The atoms XDND names, and the targets the stories drag.
    pub(crate) XdndAtoms: XdndAtomsCookie {
        XdndAware,
        XdndProxy,
        XdndSelection,
        XdndTypeList,
        XdndEnter,
        XdndPosition,
        XdndStatus,
        XdndLeave,
        XdndDrop,
        XdndFinished,
        XdndActionCopy,
        XdndActionMove,
        XdndActionPrivate,
        TARGETS,
        DELETE,
        INCR,
        NULL,
        UTF8_STRING,
        URI_LIST: b"text/uri-list",
        MOZ_URL: b"text/x-moz-url",
        PLAIN_TEXT: b"text/plain",
        UTF8_TEXT: b"text/plain;charset=utf-8",
    }
}

/// The XDND atoms, asked of `client`'s server.
pub(crate) fn xdnd_atoms(client: &X11Client) -> Result<XdndAtoms, String> {
    XdndAtoms::new(&client.connection)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())
}

/// An XDND client message, as sent or got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Xdnd {
    /// `XdndEnter`: the source's window, the version it speaks, and its types: inline, or, with
    /// `listed` (bit 0: more than three), read from its window's `XdndTypeList`.
    Enter {
        source: Window,
        version: u32,
        types: Vec<Atom>,
        listed: bool,
    },
    /// `XdndPosition`: the pointer's point on the root, the time to convert with, the action
    /// asked.
    Position {
        source: Window,
        root: (i16, i16),
        time: Timestamp,
        action: Atom,
    },
    /// `XdndStatus`: whether the target accepts the drop (bit 0), whether it wants positions
    /// within its rectangle (bit 1), the rectangle's two words, `(x, y)` and `(w, h)`, and the
    /// action it takes.
    Status {
        target: Window,
        accepted: bool,
        wants_positions: bool,
        rect: [u32; 2],
        action: Atom,
    },
    /// `XdndLeave`: the source left the target, or gave up the drag.
    Leave { source: Window },
    /// `XdndDrop`, with the time to convert with.
    Drop { source: Window, time: Timestamp },
    /// `XdndFinished`: whether the target performed the drop (bit 0, from version 5), and the
    /// action it performed.
    Finished {
        target: Window,
        accepted: bool,
        action: Atom,
    },
}

/// A point on the root as XDND packs it into a word: x in the high half, y in the low.
fn pack((x, y): (i16, i16)) -> u32 {
    (u32::from(x as u16) << 16) | u32::from(y as u16)
}

fn unpack(word: u32) -> (i16, i16) {
    ((word >> 16) as u16 as i16, word as u16 as i16)
}

impl Xdnd {
    /// Its message type and its five words.
    fn encode(&self, atoms: &XdndAtoms) -> (Atom, [u32; 5]) {
        match self {
            Xdnd::Enter {
                source,
                version,
                types,
                listed,
            } => {
                let mut data = [*source, (version << 24) | u32::from(*listed), 0, 0, 0];
                for (slot, atom) in data[2..].iter_mut().zip(types) {
                    *slot = *atom;
                }
                (atoms.XdndEnter, data)
            }
            Xdnd::Position {
                source,
                root,
                time,
                action,
            } => (
                atoms.XdndPosition,
                [*source, 0, pack(*root), *time, *action],
            ),
            Xdnd::Status {
                target,
                accepted,
                wants_positions,
                rect,
                action,
            } => {
                let flags = u32::from(*accepted) | (u32::from(*wants_positions) << 1);
                (
                    atoms.XdndStatus,
                    [*target, flags, rect[0], rect[1], *action],
                )
            }
            Xdnd::Leave { source } => (atoms.XdndLeave, [*source, 0, 0, 0, 0]),
            Xdnd::Drop { source, time } => (atoms.XdndDrop, [*source, 0, *time, 0, 0]),
            Xdnd::Finished {
                target,
                accepted,
                action,
            } => (
                atoms.XdndFinished,
                [*target, u32::from(*accepted), *action, 0, 0],
            ),
        }
    }
}

/// One thing a peer did or got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Logged {
    /// An XDND message got; `window` is the window the event names: the window under the
    /// pointer, even when the message came through that window's proxy.
    Got { window: Window, message: Xdnd },
    /// An XDND message sent to the client of `to`, the event naming `window`.
    Sent {
        to: Window,
        window: Window,
        message: Xdnd,
    },
    /// The source's window was pressed: it took `XdndSelection` and grabbed the pointer with
    /// the press's time.
    Pressed { time: Timestamp },
    /// The pointer's button was released, at `time`.
    Released { time: Timestamp },
    /// The source took [`SourceOrder::Hold`]: it reads no event until [`SourceOrder::Go`].
    Held,
    /// A button was pressed on the target's window, which takes clicks as any program's.
    Clicked { window: Window },
    /// A request for `XdndSelection` from `requestor`, for `target`, naming `time`; whether it
    /// was served (`DELETE` done) rather than refused.
    Request {
        requestor: Window,
        target: Atom,
        time: Timestamp,
        served: bool,
    },
    /// A conversion of `XdndSelection` to `target` the target made, naming `time`, and what it
    /// got.
    Read {
        target: Atom,
        time: Timestamp,
        got: Pasted,
    },
    /// An X11 error the server sent for a request that waits for no answer, such as a message
    /// sent to a window gone since: a toolkit carries on.
    XError(String),
    /// The peer's thread failed: the story fails with it.
    Failed(String),
}

/// A peer's thread, its log, and the orders the story sends it.
pub(crate) struct Peer<Order> {
    /// The window it drags from, or takes drops on.
    pub window: Window,
    pub atoms: XdndAtoms,
    log: Arc<Mutex<Vec<Logged>>>,
    orders: Sender<Order>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

/// An X11 program that drags from its window ([`SourceSpec`]).
pub(crate) type XdndSource = Peer<SourceOrder>;

/// An X11 program that takes drops on its window ([`TargetSpec`]).
pub(crate) type XdndTarget = Peer<TargetOrder>;

impl<Order: Send + 'static> Peer<Order> {
    /// Runs `role` for `window` on a thread of its own, with `client`'s connection.
    fn spawn(
        client: X11Client,
        window: Window,
        atoms: XdndAtoms,
        role: impl Role<Order> + Send + 'static,
    ) -> Peer<Order> {
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (orders, received) = mpsc::channel();
        let side = Side {
            client,
            atoms,
            log: Arc::clone(&log),
        };
        let stopped = Arc::clone(&stop);
        let worker = thread::spawn(move || run(side, role, &received, &stopped));
        Peer {
            window,
            atoms,
            log,
            orders,
            stop,
            worker: Some(worker),
        }
    }

    /// Sends the peer an order, which its loop takes between two events.
    pub(crate) fn order(&self, order: Order) {
        self.orders.send(order).expect("the peer takes orders");
    }

    /// Ends the peer's thread and waits for it: its connection closes, as its program exits,
    /// and its windows go.
    pub(crate) fn exit(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take()
            && let Err(panic) = worker.join()
        {
            std::panic::resume_unwind(panic);
        }
    }
}

impl<Order> Peer<Order> {
    /// What the peer did and got so far, in order. Panics with the peer's failure, if it failed.
    pub(crate) fn log(&self) -> Vec<Logged> {
        let log = self.log.lock().expect("the peer's log").clone();
        if let Some(Logged::Failed(error)) = log.last() {
            panic!("the X11 peer of window {} failed: {error}", self.window);
        }
        log
    }

    /// The XDND messages the peer got, in order.
    pub(crate) fn got(&self) -> Vec<Xdnd> {
        self.log()
            .into_iter()
            .filter_map(|logged| match logged {
                Logged::Got { message, .. } => Some(message),
                _ => None,
            })
            .collect()
    }

    /// The XDND messages the peer sent, in order.
    pub(crate) fn sent(&self) -> Vec<Xdnd> {
        self.log()
            .into_iter()
            .filter_map(|logged| match logged {
                Logged::Sent { message, .. } => Some(message),
                _ => None,
            })
            .collect()
    }

    /// The requests for `XdndSelection` the peer was sent: requestor, target, time, served.
    pub(crate) fn requests(&self) -> Vec<(Window, Atom, Timestamp, bool)> {
        self.log()
            .into_iter()
            .filter_map(|logged| match logged {
                Logged::Request {
                    requestor,
                    target,
                    time,
                    served,
                } => Some((requestor, target, time, served)),
                _ => None,
            })
            .collect()
    }

    /// The conversions the peer made: target, time, and what each got.
    pub(crate) fn reads(&self) -> Vec<(Atom, Timestamp, Pasted)> {
        self.log()
            .into_iter()
            .filter_map(|logged| match logged {
                Logged::Read { target, time, got } => Some((target, time, got)),
                _ => None,
            })
            .collect()
    }
}

impl<Order> Drop for Peer<Order> {
    /// Stops the peer's thread without waiting for it: a story that fails must not wait on a
    /// peer blocked in a read.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// What a peer does with each event and each order, and while nothing comes.
trait Role<Order> {
    fn event(&mut self, side: &mut Side, event: Event) -> Result<(), String>;
    fn order(&mut self, side: &mut Side, order: Order) -> Result<(), String>;
    /// Runs when no event came: for what waits on a deadline.
    fn idle(&mut self, _side: &mut Side) -> Result<(), String> {
        Ok(())
    }
    /// Whether it reads no event for now, as a program busy elsewhere: what X11 sends it waits
    /// in its connection, and only its orders are taken.
    fn holds(&self) -> bool {
        false
    }
}

/// A peer's connection, its atoms and its log.
struct Side {
    client: X11Client,
    atoms: XdndAtoms,
    log: Arc<Mutex<Vec<Logged>>>,
}

impl Side {
    fn record(&self, what: Logged) {
        self.log.lock().expect("the peer's log").push(what);
    }

    /// Sends `message` to `to`'s client, the event naming `window`, and records it.
    fn send(&self, to: Window, window: Window, message: Xdnd) -> Result<(), String> {
        send_xdnd(&self.client, &self.atoms, to, window, &message)?;
        self.record(Logged::Sent {
            to,
            window,
            message,
        });
        Ok(())
    }

    /// The XDND message `event` is, recorded; none for another client message.
    fn got(&self, event: &ClientMessageEvent) -> Result<Option<Xdnd>, String> {
        if event.format != 32 {
            return Ok(None);
        }
        let atoms = &self.atoms;
        let [l0, l1, l2, l3, l4] = event.data.as_data32();
        let message = if event.type_ == atoms.XdndEnter {
            let listed = l1 & 1 == 1;
            let types = if listed {
                self.client
                    .property32(l0, atoms.XdndTypeList)?
                    .unwrap_or_default()
            } else {
                [l2, l3, l4]
                    .into_iter()
                    .filter(|&atom| atom != x11rb::NONE)
                    .collect()
            };
            Xdnd::Enter {
                source: l0,
                version: l1 >> 24,
                types,
                listed,
            }
        } else if event.type_ == atoms.XdndPosition {
            Xdnd::Position {
                source: l0,
                root: unpack(l2),
                time: l3,
                action: l4,
            }
        } else if event.type_ == atoms.XdndStatus {
            Xdnd::Status {
                target: l0,
                accepted: l1 & 1 == 1,
                wants_positions: l1 & 2 == 2,
                rect: [l2, l3],
                action: l4,
            }
        } else if event.type_ == atoms.XdndLeave {
            Xdnd::Leave { source: l0 }
        } else if event.type_ == atoms.XdndDrop {
            Xdnd::Drop {
                source: l0,
                time: l2,
            }
        } else if event.type_ == atoms.XdndFinished {
            Xdnd::Finished {
                target: l0,
                accepted: l1 & 1 == 1,
                action: l2,
            }
        } else {
            return Ok(None);
        };
        self.record(Logged::Got {
            window: event.window,
            message: message.clone(),
        });
        Ok(Some(message))
    }
}

/// A peer's loop: its orders, then its next event, or a short sleep when none came or it holds,
/// until the story stops it or it fails.
fn run<Order>(
    mut side: Side,
    mut role: impl Role<Order>,
    orders: &Receiver<Order>,
    stop: &AtomicBool,
) {
    let mut turn = || -> Result<(), String> {
        while let Ok(order) = orders.try_recv() {
            role.order(&mut side, order)?;
        }
        if role.holds() {
            thread::sleep(Duration::from_millis(1));
            return Ok(());
        }
        match side.client.next_event()? {
            Some(Event::Error(error)) => side.record(Logged::XError(format!("{error:?}"))),
            Some(event) => role.event(&mut side, event)?,
            None => {
                role.idle(&mut side)?;
                thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(())
    };
    while !stop.load(Ordering::Relaxed) {
        if let Err(error) = turn() {
            side.record(Logged::Failed(error));
            return;
        }
    }
}

/// What a source offers, and how it drags.
#[derive(Debug, Clone)]
pub(crate) struct SourceSpec {
    /// Each target it offers, with the bytes it answers, typed as the target.
    pub offers: Vec<(Atom, Vec<u8>)>,
    /// The action each `XdndPosition` asks.
    pub action: Atom,
    /// An override-redirect window it maps and keeps under the pointer while it drags, its
    /// input shape left whole, as a toolkit's drag icon. Its own search for a target skips it,
    /// as a toolkit's does.
    pub icon: Option<Window>,
    /// Whether it drops at the release whatever its last `XdndStatus` said, as a source that
    /// ignores the status.
    pub drops_anyway: bool,
    /// Whether the pressed window itself owns `XdndSelection` and names the source in each
    /// message; else a hidden window of the program's does, as Qt's and GTK 4's.
    pub owned_by_pressed_window: bool,
}

impl SourceSpec {
    /// A source of `offers`, which asks `action`, from a hidden window of its program, with no
    /// icon.
    pub(crate) fn offering(offers: Vec<(Atom, Vec<u8>)>, action: Atom) -> SourceSpec {
        SourceSpec {
            offers,
            action,
            icon: None,
            drops_anyway: false,
            owned_by_pressed_window: false,
        }
    }
}

/// What the story may tell a source to do.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SourceOrder {
    /// Send `XdndLeave` to the target and end the drag, as a source whose user pressed Escape.
    Leave,
    /// Read no event until [`SourceOrder::Go`], logged [`Logged::Held`] once taken: the story
    /// decides when the source sees what X11 sent it meanwhile, as a source whose thread is
    /// late, and finds the window under the pointer as X11 has it then.
    Hold,
    /// Read the events again, those that waited first.
    Go,
}

/// A window of `client`'s program that it never maps, 3 × 3 at the root's corner, which
/// selects `events`: what Qt makes to own a selection or to read one.
fn hidden_window(client: &X11Client, events: EventMask) -> Result<Window, String> {
    let connection = &client.connection;
    let window = connection.generate_id().map_err(|e| e.to_string())?;
    connection
        .create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            client.root,
            0,
            0,
            3,
            3,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(events),
        )
        .map_err(|e| e.to_string())?;
    Ok(window)
}

impl Peer<SourceOrder> {
    /// Makes `window`, mapped and drawn, a source of `spec`'s, and starts it: once its window is
    /// pressed, it drags. It takes the window's button releases and motion, makes its hidden
    /// owner unless the pressed window owns, and lists its types in the owner's `XdndTypeList`
    /// when it has more than three.
    pub(crate) fn start(
        compositor: &mut Compositor,
        client: X11Client,
        window: Window,
        spec: SourceSpec,
    ) -> XdndSource {
        let targets: Vec<Atom> = spec.offers.iter().map(|(target, _)| *target).collect();
        let owned_by_pressed_window = spec.owned_by_pressed_window;
        let (client, (atoms, owner)) = step(compositor, client, move |client| {
            let atoms = xdnd_atoms(client)?;
            let fail = |e: x11rb::errors::ConnectionError| e.to_string();
            let events = EventMask::STRUCTURE_NOTIFY
                | EventMask::KEY_PRESS
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::POINTER_MOTION
                | EventMask::EXPOSURE
                | EventMask::FOCUS_CHANGE
                | EventMask::PROPERTY_CHANGE;
            client
                .connection
                .change_window_attributes(
                    window,
                    &ChangeWindowAttributesAux::new().event_mask(events),
                )
                .map_err(fail)?;
            let owner = if owned_by_pressed_window {
                window
            } else {
                hidden_window(client, EventMask::NO_EVENT)?
            };
            if targets.len() > 3 {
                client
                    .connection
                    .change_property32(
                        PropMode::REPLACE,
                        owner,
                        atoms.XdndTypeList,
                        x11rb::protocol::xproto::AtomEnum::ATOM,
                        &targets,
                    )
                    .map_err(fail)?;
            }
            client.sync()?;
            Ok((atoms, owner))
        });
        let source = Source {
            window,
            owner,
            spec,
            drag: None,
            dropped: None,
            sending: Vec::new(),
            held: false,
        };
        Peer::spawn(client, window, atoms, source)
    }

    /// The last `XdndStatus` the source got: whether it accepted, and its action.
    pub(crate) fn last_status(&self) -> Option<(bool, Atom)> {
        self.got()
            .into_iter()
            .rev()
            .find_map(|message| match message {
                Xdnd::Status {
                    accepted, action, ..
                } => Some((accepted, action)),
                _ => None,
            })
    }

    /// Whether the last `XdndStatus` accepted the drop with an action other than None: what the
    /// source drops on, as GTK 4's.
    pub(crate) fn accepts(&self) -> bool {
        self.last_status()
            .is_some_and(|(accepted, action)| accepted && action != x11rb::NONE)
    }

    /// Whether an `XdndPosition` the source sent waits for its `XdndStatus`: none came since.
    pub(crate) fn awaits_status(&self) -> bool {
        let log = self.log();
        let last_position = log.iter().rposition(|logged| {
            matches!(
                logged,
                Logged::Sent {
                    message: Xdnd::Position { .. },
                    ..
                }
            )
        });
        let last_status = log.iter().rposition(|logged| {
            matches!(
                logged,
                Logged::Got {
                    message: Xdnd::Status { .. },
                    ..
                }
            )
        });
        match (last_position, last_status) {
            (Some(position), Some(status)) => position > status,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// Whether the source's window was pressed and its drag started.
    pub(crate) fn pressed(&self) -> bool {
        self.log()
            .iter()
            .any(|logged| matches!(logged, Logged::Pressed { .. }))
    }

    /// Whether the source took [`SourceOrder::Hold`]: it reads no event since.
    pub(crate) fn held(&self) -> bool {
        self.log().contains(&Logged::Held)
    }
}

/// An XDND source's state.
struct Source {
    /// The window pressed to drag.
    window: Window,
    /// The window that owns `XdndSelection` and names the source in each message.
    owner: Window,
    spec: SourceSpec,
    /// The drag under way, from the press to the end of the release.
    drag: Option<Drag>,
    /// The time of its last drop, until the target's `XdndFinished`.
    dropped: Option<Timestamp>,
    /// The answers it sends in INCR chunks, each until its last.
    sending: Vec<IncrAnswer>,
    /// Whether it reads no event, from [`SourceOrder::Hold`] to [`SourceOrder::Go`].
    held: bool,
}

/// A drag under way.
#[derive(Debug, Default)]
struct Drag {
    /// The target under the pointer, once entered.
    target: Option<Found>,
    /// Whether an `XdndPosition` waits for its status: XDND has one in flight at most.
    waiting: bool,
    /// The last place the pointer moved to while one waited, with its time: sent once the
    /// status comes.
    kept: Option<((i16, i16), Timestamp)>,
    /// The time of the last `XdndPosition` sent: the one a request must name during the drag.
    position_time: Option<Timestamp>,
    /// Whether the last status accepted the drop with an action other than None.
    accepted: bool,
    /// The release, once it came while a status was awaited: its time, and until when the
    /// source waits.
    released: Option<(Timestamp, Instant)>,
}

/// An XDND target found under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Found {
    /// The top-level window the pointer is in, which messages name.
    pub window: Window,
    /// Where messages go: its proxy, or the window itself.
    pub deliver_to: Window,
    /// The version spoken: the target's, at most [`VERSION`].
    pub version: u32,
}

/// An answer sent in INCR chunks (ICCCM 2.7.2): the next once the requestor deleted the last.
struct IncrAnswer {
    requestor: Window,
    property: Atom,
    type_: Atom,
    data: Vec<u8>,
    /// How much was sent.
    sent: usize,
}

impl Role<SourceOrder> for Source {
    fn event(&mut self, side: &mut Side, event: Event) -> Result<(), String> {
        match event {
            Event::ButtonPress(press)
                if press.event == self.window && press.detail == 1 && self.drag.is_none() =>
            {
                self.press(side, press.time)
            }
            Event::MotionNotify(motion) if self.drag.is_some() => {
                self.motion(side, (motion.root_x, motion.root_y), motion.time)
            }
            Event::ButtonRelease(release) if release.detail == 1 && self.drag.is_some() => {
                self.release(side, release.time)
            }
            Event::ClientMessage(message) => {
                match side.got(&message)? {
                    Some(Xdnd::Status {
                        target,
                        accepted,
                        action,
                        ..
                    }) => self.status(side, target, accepted, action)?,
                    Some(Xdnd::Finished { .. }) => self.dropped = None,
                    _ => {}
                }
                Ok(())
            }
            Event::SelectionRequest(request) => self.answer(side, &request),
            Event::PropertyNotify(deleted) if deleted.state == Property::DELETE => {
                self.send_next_chunk(side, &deleted)
            }
            _ => Ok(()),
        }
    }

    fn order(&mut self, side: &mut Side, order: SourceOrder) -> Result<(), String> {
        match order {
            SourceOrder::Leave => {
                let Some(drag) = self.drag.take() else {
                    return Ok(());
                };
                if let Some(target) = drag.target {
                    let leave = Xdnd::Leave { source: self.owner };
                    side.send(target.deliver_to, target.window, leave)?;
                }
                self.let_go(side, x11rb::CURRENT_TIME)
            }
            SourceOrder::Hold => {
                self.held = true;
                side.record(Logged::Held);
                Ok(())
            }
            SourceOrder::Go => {
                self.held = false;
                Ok(())
            }
        }
    }

    fn holds(&self) -> bool {
        self.held
    }

    fn idle(&mut self, side: &mut Side) -> Result<(), String> {
        let gave_up = self
            .drag
            .as_ref()
            .and_then(|drag| drag.released)
            .filter(|&(_, until)| Instant::now() > until);
        if let Some((time, _)) = gave_up {
            if let Some(drag) = &mut self.drag {
                drag.accepted = false;
            }
            self.end(side, time)?;
        }
        Ok(())
    }
}

impl Source {
    /// The press on its window: its owner takes `XdndSelection` and the window grabs the
    /// pointer, with the press's time; its icon maps.
    fn press(&mut self, side: &mut Side, time: Timestamp) -> Result<(), String> {
        let connection = &side.client.connection;
        let fail = |error: &dyn std::fmt::Display| error.to_string();
        connection
            .set_selection_owner(self.owner, side.atoms.XdndSelection, time)
            .map_err(|e| fail(&e))?;
        if let Some(icon) = self.spec.icon {
            connection.map_window(icon).map_err(|e| fail(&e))?;
        }
        let grab = connection
            .grab_pointer(
                false,
                self.window,
                EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                x11rb::NONE,
                x11rb::NONE,
                time,
            )
            .map_err(|e| fail(&e))?
            .reply()
            .map_err(|e| fail(&e))?;
        if grab.status != GrabStatus::SUCCESS {
            return Err(format!("the source's pointer grab: {:?}", grab.status));
        }
        let owner = connection
            .get_selection_owner(side.atoms.XdndSelection)
            .map_err(|e| fail(&e))?
            .reply()
            .map_err(|e| fail(&e))?
            .owner;
        if owner != self.owner {
            return Err(format!("XdndSelection is {owner}'s, not the source's"));
        }
        side.record(Logged::Pressed { time });
        self.drag = Some(Drag::default());
        Ok(())
    }

    /// The pointer moved to `root`: the icon follows; the target under it is entered, left for
    /// another, and told the place, once its last status came.
    fn motion(&mut self, side: &mut Side, root: (i16, i16), time: Timestamp) -> Result<(), String> {
        if let Some(icon) = self.spec.icon {
            let place = ConfigureWindowAux::new()
                .x(i32::from(root.0 - ICON_HOT_SPOT))
                .y(i32::from(root.1 - ICON_HOT_SPOT));
            side.client
                .connection
                .configure_window(icon, &place)
                .map_err(|e| e.to_string())?;
        }
        let found = target_at(&side.client, &side.atoms, root, self.spec.icon);
        let types: Vec<Atom> = self.spec.offers.iter().map(|(target, _)| *target).collect();
        let owner = self.owner;
        let Some(drag) = &mut self.drag else {
            return Ok(());
        };
        if drag.released.is_some() {
            return Ok(());
        }
        if found != drag.target {
            if let Some(left) = drag.target {
                side.send(left.deliver_to, left.window, Xdnd::Leave { source: owner })?;
            }
            *drag = Drag {
                target: found,
                ..Drag::default()
            };
            if let Some(entered) = found {
                let enter = Xdnd::Enter {
                    source: owner,
                    version: entered.version,
                    listed: types.len() > 3,
                    types,
                };
                side.send(entered.deliver_to, entered.window, enter)?;
            }
        }
        let Some(target) = drag.target else {
            return Ok(());
        };
        if drag.waiting {
            drag.kept = Some((root, time));
            return Ok(());
        }
        let position = Xdnd::Position {
            source: owner,
            root,
            time,
            action: self.spec.action,
        };
        drag.position_time = Some(time);
        side.send(target.deliver_to, target.window, position)?;
        drag.waiting = true;
        Ok(())
    }

    /// The button's release: the source drops, or leaves, once the last status came.
    fn release(&mut self, side: &mut Side, time: Timestamp) -> Result<(), String> {
        side.record(Logged::Released { time });
        side.client
            .connection
            .ungrab_pointer(time)
            .map_err(|e| e.to_string())?;
        let Some(drag) = &mut self.drag else {
            return Ok(());
        };
        if drag.waiting && drag.target.is_some() {
            drag.released = Some((time, Instant::now() + STATUS_PATIENCE));
            return Ok(());
        }
        self.end(side, time)
    }

    /// A status from `target`, solicited or not: whether it accepts, with which action; then the
    /// next place kept goes, or the drag ends if it was released.
    fn status(
        &mut self,
        side: &mut Side,
        target: Window,
        accepted: bool,
        action: Atom,
    ) -> Result<(), String> {
        let owner = self.owner;
        let asked = self.spec.action;
        let Some(drag) = &mut self.drag else {
            return Ok(());
        };
        let Some(current) = drag.target.filter(|current| current.window == target) else {
            return Ok(());
        };
        drag.waiting = false;
        drag.accepted = accepted && action != x11rb::NONE;
        if let Some((time, _)) = drag.released {
            return self.end(side, time);
        }
        if let Some((root, time)) = drag.kept.take() {
            let position = Xdnd::Position {
                source: owner,
                root,
                time,
                action: asked,
            };
            drag.position_time = Some(time);
            side.send(current.deliver_to, current.window, position)?;
            drag.waiting = true;
        }
        Ok(())
    }

    /// The drag's end at the release: `XdndDrop` if the last status accepted it, with an action,
    /// else `XdndLeave`.
    fn end(&mut self, side: &mut Side, time: Timestamp) -> Result<(), String> {
        let Some(drag) = self.drag.take() else {
            return Ok(());
        };
        if let Some(target) = drag.target {
            let message = if drag.accepted || self.spec.drops_anyway {
                self.dropped = Some(time);
                Xdnd::Drop {
                    source: self.owner,
                    time,
                }
            } else {
                Xdnd::Leave { source: self.owner }
            };
            side.send(target.deliver_to, target.window, message)?;
        }
        self.let_go(side, time)
    }

    /// Lets go of the pointer and hides the icon.
    fn let_go(&mut self, side: &mut Side, time: Timestamp) -> Result<(), String> {
        let connection = &side.client.connection;
        connection.ungrab_pointer(time).map_err(|e| e.to_string())?;
        if let Some(icon) = self.spec.icon {
            connection.unmap_window(icon).map_err(|e| e.to_string())?;
        }
        connection.flush().map_err(|e| e.to_string())
    }

    /// Whether a request naming `time` is for this source's data, as Qt's decides: during the
    /// drag it names the last `XdndPosition`'s time; after a drop, the drop's, until its
    /// `XdndFinished`.
    fn in_time(&self, time: Timestamp) -> bool {
        let at_position = self
            .drag
            .as_ref()
            .is_some_and(|drag| drag.position_time == Some(time));
        at_position || self.dropped == Some(time)
    }

    /// Answers a request for `XdndSelection` that names its time ([`Source::in_time`]):
    /// `TARGETS`, `DELETE` (done, an empty `NULL`), or an offered target, in INCR chunks when
    /// longer than one; refuses others.
    fn answer(&mut self, side: &mut Side, request: &SelectionRequestEvent) -> Result<(), String> {
        let atoms = side.atoms;
        let connection = &side.client.connection;
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let property = if request.property == x11rb::NONE {
            request.target
        } else {
            request.property
        };
        let requestor = request.requestor;
        let offered = self
            .spec
            .offers
            .iter()
            .find(|(target, _)| *target == request.target);
        let served = if request.selection != atoms.XdndSelection || !self.in_time(request.time) {
            false
        } else if request.target == atoms.TARGETS {
            let mut targets = vec![atoms.TARGETS];
            targets.extend(self.spec.offers.iter().map(|(target, _)| *target));
            connection
                .change_property32(
                    PropMode::REPLACE,
                    requestor,
                    property,
                    x11rb::protocol::xproto::AtomEnum::ATOM,
                    &targets,
                )
                .map_err(fail)?;
            true
        } else if request.target == atoms.DELETE {
            connection
                .change_property8(PropMode::REPLACE, requestor, property, atoms.NULL, &[])
                .map_err(fail)?;
            true
        } else if let Some((target, data)) = offered {
            if data.len() <= CHUNK {
                connection
                    .change_property8(PropMode::REPLACE, requestor, property, *target, data)
                    .map_err(fail)?;
            } else {
                connection
                    .change_window_attributes(
                        requestor,
                        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                    )
                    .map_err(fail)?;
                let length = u32::try_from(data.len()).map_err(|e| e.to_string())?;
                connection
                    .change_property32(
                        PropMode::REPLACE,
                        requestor,
                        property,
                        atoms.INCR,
                        &[length],
                    )
                    .map_err(fail)?;
                self.sending.push(IncrAnswer {
                    requestor,
                    property,
                    type_: *target,
                    data: data.clone(),
                    sent: 0,
                });
            }
            true
        } else {
            false
        };
        notify(
            &side.client,
            request,
            if served { property } else { x11rb::NONE },
        )?;
        side.record(Logged::Request {
            requestor,
            target: request.target,
            time: request.time,
            served,
        });
        Ok(())
    }

    /// The requestor deleted an INCR answer's property: the next chunk goes, the last one empty.
    fn send_next_chunk(
        &mut self,
        side: &mut Side,
        deleted: &PropertyNotifyEvent,
    ) -> Result<(), String> {
        let Some(index) = self.sending.iter().position(|answer| {
            answer.requestor == deleted.window && answer.property == deleted.atom
        }) else {
            return Ok(());
        };
        let answer = &mut self.sending[index];
        let end = (answer.sent + CHUNK).min(answer.data.len());
        let piece = &answer.data[answer.sent..end];
        let connection = &side.client.connection;
        connection
            .change_property8(
                PropMode::REPLACE,
                answer.requestor,
                answer.property,
                answer.type_,
                piece,
            )
            .map_err(|e| e.to_string())?;
        connection.flush().map_err(|e| e.to_string())?;
        if piece.is_empty() {
            self.sending.remove(index);
        } else {
            answer.sent = end;
        }
        Ok(())
    }
}

/// The XDND target under `root`, as a source finds it: the top-level child of the root there
/// (skipping `skipped`, the source's own icon), its `XdndProxy` honoured when the proxy names
/// itself, its `XdndAware` version at least 3. None over no window, a window gone since, or a
/// window that is not aware.
pub(crate) fn target_at(
    client: &X11Client,
    atoms: &XdndAtoms,
    root: (i16, i16),
    skipped: Option<Window>,
) -> Option<Found> {
    let child = match skipped {
        None => {
            let screen_root = client.root;
            client
                .connection
                .translate_coordinates(screen_root, screen_root, root.0, root.1)
                .ok()?
                .reply()
                .ok()?
                .child
        }
        Some(skipped) => top_child_at(client, root, skipped)?,
    };
    if child == x11rb::NONE {
        return None;
    }
    let named_proxy = client
        .property32(child, atoms.XdndProxy)
        .ok()
        .flatten()
        .and_then(|values| values.first().copied());
    let deliver_to = named_proxy
        .filter(|&proxy| {
            client.property32(proxy, atoms.XdndProxy).ok().flatten() == Some(vec![proxy])
        })
        .unwrap_or(child);
    let version = *client
        .property32(deliver_to, atoms.XdndAware)
        .ok()
        .flatten()?
        .first()?;
    (version >= 3).then_some(Found {
        window: child,
        deliver_to,
        version: version.min(VERSION),
    })
}

/// The window under `root` as Chromium's drag finds its target (`ui/gfx/x/window_cache.cc`,
/// `GetWindowAtPoint`): the topmost mapped window whose rectangle holds the point, its children
/// first, that has a `WM_NAME`; an unnamed window is looked through. None when no named window
/// holds the point.
pub(crate) fn named_window_at(
    client: &X11Client,
    root: (i16, i16),
) -> Result<Option<Window>, String> {
    named_window_in(client, client.root, (i32::from(root.0), i32::from(root.1)))
}

/// [`named_window_at`] within `window`, `point` in its parent's coordinates (the root's own
/// for the root).
fn named_window_in(
    client: &X11Client,
    window: Window,
    point: (i32, i32),
) -> Result<Option<Window>, String> {
    let connection = &client.connection;
    let fail = |e: &dyn std::fmt::Display| e.to_string();
    let attributes = connection
        .get_window_attributes(window)
        .map_err(|e| fail(&e))?
        .reply()
        .map_err(|e| fail(&e))?;
    if attributes.map_state == MapState::UNMAPPED {
        return Ok(None);
    }
    let point = if window == client.root {
        point
    } else {
        let place = client.geometry(window)?;
        let inside = (place.x..place.x + place.width as i32).contains(&point.0)
            && (place.y..place.y + place.height as i32).contains(&point.1);
        if !inside {
            return Ok(None);
        }
        (point.0 - place.x, point.1 - place.y)
    };
    let children = connection
        .query_tree(window)
        .map_err(|e| fail(&e))?
        .reply()
        .map_err(|e| fail(&e))?
        .children;
    for child in children.into_iter().rev() {
        if let Some(named) = named_window_in(client, child, point)? {
            return Ok(Some(named));
        }
    }
    let name = connection
        .get_property(false, window, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 1)
        .map_err(|e| fail(&e))?
        .reply()
        .map_err(|e| fail(&e))?;
    Ok((name.format != 0).then_some(window))
}

/// The topmost mapped child of the root whose rectangle holds `root`, `skipped` left out.
fn top_child_at(client: &X11Client, root: (i16, i16), skipped: Window) -> Option<Window> {
    let connection = &client.connection;
    let children = connection
        .query_tree(client.root)
        .ok()?
        .reply()
        .ok()?
        .children;
    let (x, y) = (i32::from(root.0), i32::from(root.1));
    children.into_iter().rev().find(|&child| {
        if child == skipped {
            return false;
        }
        let mapped = connection
            .get_window_attributes(child)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|attributes| attributes.map_state == MapState::VIEWABLE);
        mapped
            && client.geometry(child).is_ok_and(|place| {
                (place.x..place.x + place.width as i32).contains(&x)
                    && (place.y..place.y + place.height as i32).contains(&y)
            })
    })
}

/// How a target answers each `XdndPosition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// It accepts the drop, for this action.
    Accept(Atom),
    /// It refuses it.
    Refuse,
}

/// When a target sends `XdndFinished`, once it took the drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Finish {
    /// Once it read the drop.
    AtOnce,
    /// When the story orders it ([`TargetOrder::Finish`]).
    OnOrder,
    /// Never.
    Never,
}

/// What a target is, and how it takes a drop.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TargetSpec {
    /// Its `XdndAware` version; none, it has none.
    pub aware: Option<u32>,
    /// Whether its `XdndProxy` names itself: a proxy of another window.
    pub proxy_of_itself: bool,
    pub answer: Answer,
    /// The target it converts `XdndSelection` to at the drop, naming the drop's time; none, it
    /// reads nothing.
    pub reads: Option<Atom>,
    /// Whether it converts at the first `XdndPosition` too, naming that position's time, as a
    /// target that looks at the data before it answers.
    pub reads_early: bool,
    /// Whether it reads the drop only when the story orders it ([`TargetOrder::Read`]), still
    /// naming the drop's time, as a slow program.
    pub reads_on_order: bool,
    /// Whether it converts into its own window, as some toolkits do; else into a hidden window
    /// of its program's, as Qt's.
    pub reads_into_itself: bool,
    /// Whether it converts `DELETE` after it read, before `XdndFinished`, as a move's target.
    pub deletes: bool,
    pub finish: Finish,
}

impl TargetSpec {
    /// A target aware at version 5 that accepts a copy, reads `target` at the drop into a hidden
    /// window of its program, and finishes at once.
    pub(crate) fn copying(atoms: &XdndAtoms, target: Atom) -> TargetSpec {
        TargetSpec {
            aware: Some(VERSION),
            proxy_of_itself: false,
            answer: Answer::Accept(atoms.XdndActionCopy),
            reads: Some(target),
            reads_early: false,
            reads_on_order: false,
            reads_into_itself: false,
            deletes: false,
            finish: Finish::AtOnce,
        }
    }
}

/// What the story may tell a target to do.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TargetOrder {
    /// Read the drop it holds ([`TargetSpec::reads_on_order`]), then finish as its spec says.
    Read,
    /// Send the `XdndFinished` it holds ([`Finish::OnOrder`]).
    Finish,
}

impl Peer<TargetOrder> {
    /// Makes `window` a target of `spec`'s, its properties set and its hidden requestor made
    /// unless it reads into itself, and starts it.
    pub(crate) fn start(
        compositor: &mut Compositor,
        client: X11Client,
        window: Window,
        spec: TargetSpec,
    ) -> XdndTarget {
        let (client, (atoms, requestor)) = step(compositor, client, move |client| {
            let atoms = xdnd_atoms(client)?;
            let fail = |e: x11rb::errors::ConnectionError| e.to_string();
            if let Some(version) = spec.aware {
                client
                    .connection
                    .change_property32(
                        PropMode::REPLACE,
                        window,
                        atoms.XdndAware,
                        x11rb::protocol::xproto::AtomEnum::ATOM,
                        &[version],
                    )
                    .map_err(fail)?;
            }
            if spec.proxy_of_itself {
                set_proxy(client, &atoms, window, window)?;
            }
            let requestor = if spec.reads_into_itself {
                window
            } else {
                hidden_window(client, EventMask::PROPERTY_CHANGE)?
            };
            client.sync()?;
            Ok((atoms, requestor))
        });
        let target = Target {
            requestor,
            spec,
            current: None,
            unread: None,
            held: None,
        };
        Peer::spawn(client, window, atoms, target)
    }
}

/// Sends `message` to `to`'s client, the event naming `window`.
pub(crate) fn send_xdnd(
    client: &X11Client,
    atoms: &XdndAtoms,
    to: Window,
    window: Window,
    message: &Xdnd,
) -> Result<(), String> {
    let (type_, data) = message.encode(atoms);
    let event = ClientMessageEvent::new(32, window, type_, data);
    let connection = &client.connection;
    connection
        .send_event(false, to, EventMask::NO_EVENT, event)
        .map_err(|e| e.to_string())?;
    connection.flush().map_err(|e| e.to_string())
}

/// Sets `window`'s `XdndProxy` to `proxy`.
pub(crate) fn set_proxy(
    client: &X11Client,
    atoms: &XdndAtoms,
    window: Window,
    proxy: Window,
) -> Result<(), String> {
    client
        .connection
        .change_property32(
            PropMode::REPLACE,
            window,
            atoms.XdndProxy,
            x11rb::protocol::xproto::AtomEnum::WINDOW,
            &[proxy],
        )
        .map_err(|e| e.to_string())?;
    client.sync()
}

/// An XDND target's state.
struct Target {
    /// The window it converts into: a hidden one of its program's, or its own.
    requestor: Window,
    spec: TargetSpec,
    /// The source that entered, while it has not left or dropped.
    current: Option<Entered>,
    /// The drop taken and not read yet, held for the story's order.
    unread: Option<Taken>,
    /// The `XdndFinished` held for the story's order: the source's window, and the message.
    held: Option<(Window, Xdnd)>,
}

/// A source that entered a target.
#[derive(Debug, Clone, Copy)]
struct Entered {
    version: u32,
    /// Whether it read at a position already.
    read: bool,
}

/// A drop a target accepted: from whom, the window it named, its time, the version spoken and
/// the action accepted.
#[derive(Debug, Clone, Copy)]
struct Taken {
    source: Window,
    named: Window,
    time: Timestamp,
    version: u32,
    action: Atom,
}

impl Role<TargetOrder> for Target {
    fn event(&mut self, side: &mut Side, event: Event) -> Result<(), String> {
        if let Event::ButtonPress(press) = event {
            side.record(Logged::Clicked {
                window: press.event,
            });
            return Ok(());
        }
        let Event::ClientMessage(message) = event else {
            return Ok(());
        };
        let Some(xdnd) = side.got(&message)? else {
            return Ok(());
        };
        // The window under the pointer, which the target's answers name: the event's, even
        // through a proxy.
        let named = message.window;
        match xdnd {
            Xdnd::Enter { version, .. } => {
                self.current = Some(Entered {
                    version,
                    read: false,
                });
            }
            Xdnd::Position { source, time, .. } => self.position(side, source, named, time)?,
            Xdnd::Leave { .. } => self.current = None,
            Xdnd::Drop { source, time } => self.drop(side, source, named, time)?,
            _ => {}
        }
        Ok(())
    }

    fn order(&mut self, side: &mut Side, order: TargetOrder) -> Result<(), String> {
        match order {
            TargetOrder::Read => match self.unread.take() {
                Some(taken) => self.take(side, taken),
                None => Ok(()),
            },
            TargetOrder::Finish => match self.held.take() {
                Some((source, finished)) => side.send(source, source, finished),
                None => Ok(()),
            },
        }
    }
}

impl Target {
    /// A position: read if it reads early, then answer as the spec says.
    fn position(
        &mut self,
        side: &mut Side,
        source: Window,
        named: Window,
        time: Timestamp,
    ) -> Result<(), String> {
        if self.spec.reads_early
            && let Some(current) = &mut self.current
            && !current.read
            && let Some(target) = self.spec.reads
        {
            current.read = true;
            self.read(side, target, time)?;
        }
        let (accepted, action) = match self.spec.answer {
            Answer::Accept(action) => (true, action),
            Answer::Refuse => (false, x11rb::NONE),
        };
        let status = Xdnd::Status {
            target: named,
            accepted,
            wants_positions: true,
            rect: [0, 0],
            action,
        };
        side.send(source, source, status)
    }

    /// A drop: an accepting target takes it, now or on the story's order; a refusing one
    /// finishes refused.
    fn drop(
        &mut self,
        side: &mut Side,
        source: Window,
        named: Window,
        time: Timestamp,
    ) -> Result<(), String> {
        let version = self
            .current
            .take()
            .map_or(VERSION, |current| current.version);
        let Answer::Accept(action) = self.spec.answer else {
            let refused = Xdnd::Finished {
                target: named,
                accepted: false,
                action: x11rb::NONE,
            };
            return self.finish(side, source, refused);
        };
        let taken = Taken {
            source,
            named,
            time,
            version,
            action,
        };
        if self.spec.reads_on_order {
            self.unread = Some(taken);
            return Ok(());
        }
        self.take(side, taken)
    }

    /// Reads an accepted drop, deletes for a move, and finishes: performed if every read got
    /// data.
    fn take(&mut self, side: &mut Side, taken: Taken) -> Result<(), String> {
        let mut performed = true;
        if let Some(target) = self.spec.reads {
            performed &= self.read(side, target, taken.time)?.is_some();
        }
        if self.spec.deletes {
            performed &= self.read(side, side.atoms.DELETE, taken.time)?.is_some();
        }
        // Before version 5, XdndFinished carries no more than the target's window.
        let told = taken.version >= 5 && performed;
        let finished = Xdnd::Finished {
            target: taken.named,
            accepted: told,
            action: if told { taken.action } else { x11rb::NONE },
        };
        self.finish(side, taken.source, finished)
    }

    /// Sends `finished` to `source` as the spec says: at once, on the story's order, or never.
    fn finish(&mut self, side: &mut Side, source: Window, finished: Xdnd) -> Result<(), String> {
        match self.spec.finish {
            Finish::AtOnce => side.send(source, source, finished),
            Finish::OnOrder => {
                self.held = Some((source, finished));
                Ok(())
            }
            Finish::Never => Ok(()),
        }
    }

    /// Converts `XdndSelection` to `target` into its requestor, naming `time`, reads the answer
    /// whole, and records it; returns what it got.
    fn read(&mut self, side: &mut Side, target: Atom, time: Timestamp) -> Result<Pasted, String> {
        let selection = side.atoms.XdndSelection;
        let got = paste_at(&mut side.client, self.requestor, selection, target, time)?;
        side.record(Logged::Read {
            target,
            time,
            got: got.clone(),
        });
        Ok(got)
    }
}
