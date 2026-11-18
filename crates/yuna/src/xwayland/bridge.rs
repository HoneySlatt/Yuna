//! The clipboard and the primary selection between a seat's X11 programs and its Wayland
//! programs, on X11's side: a part of the seat's X11 window manager, on its thread
//! (`manager/`), which alone speaks X11 and touches the transfers' pipes. Yuna's loop decides
//! what the seat's Wayland selections are (`selections.rs`); it never reads nor writes a byte of
//! the data, and never waits.
//!
//! The manager's window owns the X11 selections it holds for Wayland, and XFixes tells it of
//! every change of `CLIPBOARD`'s and `PRIMARY`'s owner, selected before `WM_S0` is taken, so that
//! no X11 client's selection goes unseen.
//!
//! **X11 to Wayland.** When an X11 client takes a selection, the manager converts its `TARGETS`
//! onto a window of its own, maps them to MIME types ([`offer_of_targets`]), and posts them;
//! Yuna offers them as the seat's Wayland selection, held by itself. A Wayland client that reads
//! it sends its pipe, which comes back as a command: the manager converts the selection to the
//! target of that type onto a new window, child of its own, reads the property in pieces, or
//! INCR's chunks as the owner appends them, and writes them into the pipe as it drains. An owner
//! gone, its selection set to none, its window destroyed or its client gone, empties the Wayland
//! selection; a client gone also ends the transfers it was serving.
//!
//! **Wayland to X11.** When a Wayland client of the seat sets a selection, Yuna's loop sends its
//! MIME types; the manager takes the X11 selection for them, and answers each `SelectionRequest`:
//! `TARGETS`, `TIMESTAMP`, and each data target ([`targets_of_offer`]) by asking Yuna's loop for
//! the read end of a pipe the Wayland source writes into. What comes is set on the requestor's
//! property whole, or, above the core protocol's maximum request, in INCR chunks, each once the
//! requestor deleted the last. An unknown target, or a failure before the answer, is refused
//! with the property none, as is a request named with a time before Yuna took the selection
//! (ICCCM 2.2), and a request for the manager's other selections, `WM_S0` and `_XSETTINGS_S0`,
//! which serve no data. When the Wayland selection ends, the X11 selection is released.
//!
//! **Answers made up.** Any X11 client can send any window a `SelectionNotify`: an answer that
//! names another selection or target than the conversion waiting on its window is left. One that
//! names them all cannot be told from the owner's, and ends that conversion alone.
//!
//! **Who reads.** The seat's Wayland selections are read from X11 only while one of the seat's
//! X11 windows has its keyboard: as a Wayland client reads them only with the focus. Otherwise
//! every request for them is refused, `TARGETS` included. An X11 client's selection always goes
//! to the seat's Wayland selection, which Wayland's own rule then gives to the client with the
//! focus.
//!
//! **Bounds.** A transfer that makes no progress for [`X11_TRANSFER_TIMEOUT`] ends, as does one
//! past [`MAX_X11_TRANSFER_BYTES`]; [`MAX_X11_TRANSFERS`] run at once each way, and a request
//! past them is refused. A transfer holds a piece of a property, or a chunk, at a time: its
//! reader's pace sets the next. A reader or a pipe that closes, a requestor or an owner gone, a
//! reply of the wrong type or too short, each ends its transfer alone: the Wayland reader sees
//! its pipe end, the X11 requestor a refusal, or, mid-INCR, no next chunk.
//!
//! **Drags.** A drag's data between X11 and Wayland moves through the same transfers, within the
//! same bounds and count (`xdnd.rs`): an X11 program's `XdndSelection` read for a Wayland target,
//! named with the time of the drag's latest position, and asked again, [`MAX_DRAG_ASKS`] times at
//! most, when the source refused it for a newer position; and a Wayland client's drag served to
//! the X11 target's requests, which `xdnd.rs` checks first. A drag's transfers that ended keep
//! when they last moved, from which its time limits run.

use std::collections::HashMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::time::{Duration, Instant};

use smithay::reexports::calloop::channel::SyncSender;
use smithay::reexports::rustix;
use smithay::reexports::rustix::buffer::spare_capacity;
use smithay::reexports::rustix::event::PollFlags;
use smithay::reexports::rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use smithay::reexports::rustix::io::Errno;
use x11rb::connection::Connection;
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::xfixes::{self, SelectionEvent};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, PropertyNotifyEvent, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent,
    SelectionRequestEvent, Timestamp, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::manager::{Atoms, Failure, GoOn, ManagerEvent};

/// How long a transfer of a selection between X11 and Wayland may go without progress, no byte
/// read or written and no step of its X11 peer, before it ends: as long as a ping waits
/// ([`X11_PING_TIMEOUT`](crate::X11_PING_TIMEOUT)), as KWin waits 5 to 10 s. A transfer that
/// moves is never ended by it, however long it takes.
pub const X11_TRANSFER_TIMEOUT: Duration = Duration::from_secs(5);

/// How many transfers run at once on a seat, each way: a paste asks for a type or two, a
/// clipboard manager for a few more. One past them is refused.
pub const MAX_X11_TRANSFERS: usize = 8;

/// The most bytes one transfer carries: an uncompressed 8K image is 133 MB. Past it, the
/// transfer ends. Its bytes go through in pieces, never held whole.
pub const MAX_X11_TRANSFER_BYTES: u64 = 256 * 1024 * 1024;

/// The most MIME types of a Wayland selection X11 is offered, and the longest, in bytes: a
/// program offers a dozen, of a few dozen bytes each. The others are left.
pub(crate) const MAX_MIME_TYPES: usize = 64;
pub(crate) const MAX_MIME_TYPE_BYTES: usize = 255;

/// The most targets read of an X11 owner's `TARGETS`; the others are left.
const MAX_TARGETS: u32 = 256;

/// How much of a property a transfer to Wayland reads at once, in bytes: its memory.
const PIECE_BYTES: u32 = 256 * 1024;

/// How many times a read of an X11 drag is asked, at most: once, then again each time its source
/// refused it for a newer position that came meanwhile, as Qt refuses every time but its last
/// position's.
const MAX_DRAG_ASKS: u8 = 4;

/// The bytes of a `ChangeProperty` request before its data.
const CHANGE_PROPERTY_HEADER: usize = 24;

/// How many atoms' names the manager keeps; past it, it forgets them all and asks again.
const MAX_NAMES: usize = 4096;

/// The MIME types of text, as the rest of the desktop names them.
const UTF8_TEXT: &str = "text/plain;charset=utf-8";
const PLAIN_TEXT: &str = "text/plain";

/// One of a seat's two selections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Selection {
    /// `CLIPBOARD`, `wl_data_device`'s selection.
    Clipboard,
    /// `PRIMARY`, the primary selection's.
    Primary,
}

impl Selection {
    pub(crate) const BOTH: [Selection; 2] = [Selection::Clipboard, Selection::Primary];

    /// Its atom in X11.
    fn atom(self, atoms: &Atoms) -> Atom {
        match self {
            Selection::Clipboard => atoms.CLIPBOARD,
            Selection::Primary => AtomEnum::PRIMARY.into(),
        }
    }
}

/// Whether `name` reads as a MIME type, which a Wayland client may take as it is: a type and a
/// subtype around a slash, of printable ASCII, not too long.
fn is_mime_type(name: &str) -> bool {
    name.len() <= MAX_MIME_TYPE_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
        && name
            .split_once('/')
            .is_some_and(|(kind, subtype)| !kind.is_empty() && !subtype.is_empty())
}

/// What an X11 owner's targets, with their names, offer Wayland: each MIME type with the target
/// it is converted from. A target named as a MIME type is offered as it is; then `UTF8_STRING`
/// is `text/plain;charset=utf-8`, and `STRING`, else `TEXT`, is `text/plain`, unless the owner
/// named those types itself. The others, `TARGETS`, `TIMESTAMP`, `COMPOUND_TEXT` among them, have
/// no MIME type.
pub(super) fn offer_of_targets(targets: &[(Atom, String)]) -> Vec<(String, Atom)> {
    let mut offer: Vec<(String, Atom)> = Vec::new();
    for (atom, name) in targets {
        if is_mime_type(name) && !offer.iter().any(|(offered, _)| offered == name) {
            offer.push((name.clone(), *atom));
        }
    }
    for (alias, mime_type) in [
        ("UTF8_STRING", UTF8_TEXT),
        ("STRING", PLAIN_TEXT),
        ("TEXT", PLAIN_TEXT),
    ] {
        let target = targets.iter().find(|(_, name)| name == alias);
        if let Some((atom, _)) = target
            && !offer.iter().any(|(offered, _)| offered == mime_type)
        {
            offer.push((mime_type.to_owned(), *atom));
        }
    }
    offer
}

/// The targets X11 is offered for a Wayland selection's MIME types, by name, each with the MIME
/// type it is read as: each MIME type under its own name; `UTF8_STRING` from
/// `text/plain;charset=utf-8`, else `text/plain`; `STRING` and `TEXT` from `text/plain`, else
/// `text/plain;charset=utf-8`. The bytes go as the source writes them: text is never encoded
/// again. `TARGETS` and `TIMESTAMP` are the manager's own.
pub(super) fn targets_of_offer(mime_types: &[String]) -> Vec<(String, String)> {
    let mut targets: Vec<(String, String)> = mime_types
        .iter()
        .filter(|mime_type| is_mime_type(mime_type))
        .map(|mime_type| (mime_type.clone(), mime_type.clone()))
        .collect();
    let offered = |wanted: &&str| mime_types.iter().any(|mime_type| mime_type == wanted);
    let utf8_first = [UTF8_TEXT, PLAIN_TEXT].into_iter().find(offered);
    let plain_first = [PLAIN_TEXT, UTF8_TEXT].into_iter().find(offered);
    for (name, from) in [
        ("UTF8_STRING", utf8_first),
        ("STRING", plain_first),
        ("TEXT", plain_first),
    ] {
        if let Some(from) = from
            && !targets.iter().any(|(target, _)| target == name)
        {
            targets.push((name.to_owned(), from.to_owned()));
        }
    }
    targets
}

/// What the manager knows of one of the seat's selections.
#[derive(Debug, Default)]
struct Held {
    /// The seat's Wayland selection, while Yuna owns the X11 selection for it: each target with
    /// the MIME type it is read as.
    wayland: Option<Vec<(Atom, String)>>,
    /// The serial of the last Wayland selection Yuna's loop told of.
    wayland_serial: u64,
    /// How many of Yuna's takings of the X11 selection XFixes has not reported yet.
    taking: u32,
    /// When Yuna last took it, as XFixes reported, once no taking is unreported: `TIMESTAMP`'s
    /// answer, and the time its release names, so that it never releases another's.
    owned_at: Option<Timestamp>,
    /// The X11 client that owns it, when one does.
    x11: Option<X11Owner>,
}

/// An X11 client that owns a selection.
#[derive(Debug)]
struct X11Owner {
    /// Its number among the owners the manager saw: a paste names the owner it was offered.
    number: u64,
    /// What it offers, once its `TARGETS` are read: each MIME type with its target.
    offer: Option<Vec<(String, Atom)>>,
    /// The window its `TARGETS` are converted onto, and when they were asked, until they come.
    asking: Option<(Window, Instant)>,
}

/// What a transfer to Wayland reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadFrom {
    /// One of the seat's selections, from the X11 owner of this number.
    Selection { selection: Selection, owner: u64 },
    /// An X11 program's drag, the manager's `drag`-th (`xdnd.rs`).
    Drag(u64),
}

/// A transfer from an X11 owner into a Wayland reader's pipe, on a window of the manager's own.
#[derive(Debug)]
struct ToWayland {
    from: ReadFrom,
    /// The X11 selection converted: `CLIPBOARD`, `PRIMARY` or `XdndSelection`.
    selection: Atom,
    /// The target it is converted to.
    target: Atom,
    /// The time its conversion named, and how many times it was asked.
    time: Timestamp,
    asks: u8,
    /// The reader's pipe, non-blocking.
    pipe: OwnedFd,
    stage: Fetch,
    /// What was read and not written yet, from `written` on.
    pending: Vec<u8>,
    written: usize,
    /// The bytes read so far, for [`MAX_X11_TRANSFER_BYTES`].
    total: u64,
    /// The last time it moved.
    progress: Instant,
}

/// Where a transfer to Wayland stands on X11's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fetch {
    /// The owner has not answered the conversion yet.
    Converting,
    /// The property is read in pieces, the next from `offset`, in 4-byte units; `incr` when it
    /// is a chunk of INCR, after which the owner appends the next.
    Reading { offset: u32, incr: bool },
    /// INCR: the owner's next chunk, which has `arrived` once its property is set.
    AwaitingChunk { arrived: bool },
    /// Everything read: what is pending goes, then the pipe closes.
    Done,
}

/// An X11 requestor's request for the seat's Wayland selection, as the answer names it.
#[derive(Debug, Clone, Copy)]
struct Request {
    requestor: Window,
    selection: Atom,
    target: Atom,
    /// The property the answer goes into: the target's, for a requestor that named none.
    property: Atom,
    time: Timestamp,
}

impl Request {
    fn of(asked: &SelectionRequestEvent) -> Request {
        Request {
            requestor: asked.requestor,
            selection: asked.selection,
            target: asked.target,
            // ICCCM 2.2: a requestor that names no property is an old one, and the target
            // names it.
            property: if asked.property == x11rb::NONE {
                asked.target
            } else {
                asked.property
            },
            time: asked.time,
        }
    }
}

/// What an X11 request for Wayland's data is served from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Served {
    /// One of the seat's Wayland selections.
    Selection(Selection),
    /// A Wayland client's drag, Yuna's `drag`-th (`xdnd.rs`).
    Drag(u64),
}

/// A transfer from a Wayland source's pipe into an X11 requestor's property.
#[derive(Debug)]
struct ToX11 {
    request: Request,
    served: Served,
    /// The type of the property set: the target, `UTF8_STRING` for `TEXT`.
    type_: Atom,
    /// The read end of the source's pipe, non-blocking, once Yuna's loop gives it.
    source: Option<OwnedFd>,
    /// Whether the source closed its end: all its bytes are read.
    ended: bool,
    /// What was read and not set on the property yet.
    buffer: Vec<u8>,
    total: u64,
    stage: Answer,
    progress: Instant,
}

/// Where a transfer to X11 stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// Yuna's loop has not given the source's pipe yet.
    AwaitingSource,
    /// The source is read; the requestor knows nothing yet. Past one chunk, INCR starts.
    Collecting,
    /// INCR: the requestor has `deleted` the last property set, so that the next can go.
    Incr { deleted: bool },
}

/// Where a transfer stands once it moved as far as it could.
enum Step<T> {
    /// It goes on: it waits for a pipe or for X11.
    Going(T),
    /// It ended whole.
    Ended,
    /// It ends short, for this reason.
    Failed(T, &'static str),
}

/// The selections on X11's side of a seat's manager: what it holds for Wayland, the X11 owners it
/// reads for Wayland, and the transfers each way.
pub(super) struct Bridge<'a> {
    connection: &'a RustConnection,
    events: &'a SyncSender<ManagerEvent>,
    atoms: Atoms,
    root: Window,
    /// The manager's own window: it owns the selections held for Wayland, and the transfers'
    /// windows are its children.
    own: Window,
    /// The most bytes one property takes: the core protocol's maximum request, less its header.
    /// Above it, INCR.
    chunk: usize,
    clipboard: Held,
    primary: Held,
    /// The seat's X11 window that has its keyboard, if one does: X11 reads Wayland's selections
    /// only then.
    focused: Option<Window>,
    /// The transfers to Wayland, by their window.
    to_wayland: HashMap<Window, ToWayland>,
    /// The transfers to X11, by their number.
    to_x11: HashMap<u64, ToX11>,
    next_transfer: u64,
    next_owner: u64,
    /// The names of the atoms met in owners' targets; none for an atom X11 does not know.
    names: HashMap<Atom, Option<String>>,
    /// When the transfers of the latest drags last moved, once they ended, each way, by drag:
    /// a drag's time limit runs from its last move, ended transfers included.
    drags_to_wayland_moved: Vec<(u64, Instant)>,
    drags_to_x11_moved: Vec<(u64, Instant)>,
}

/// Records that drag `drag`'s transfers last moved `at`, in `moves`, one entry a drag: the
/// manager forgets the drags it no longer follows at each turn ([`Bridge::keep_drag_moves`]).
fn note_move(moves: &mut Vec<(u64, Instant)>, drag: u64, at: Instant) {
    match moves.iter_mut().find(|(of, _)| *of == drag) {
        Some((_, last)) => *last = (*last).max(at),
        None => moves.push((drag, at)),
    }
}

impl<'a> Bridge<'a> {
    pub(super) fn new(
        connection: &'a RustConnection,
        events: &'a SyncSender<ManagerEvent>,
        atoms: Atoms,
        root: Window,
        own: Window,
    ) -> Bridge<'a> {
        let maximum_request = usize::from(connection.setup().maximum_request_length) * 4;
        Bridge {
            connection,
            events,
            atoms,
            root,
            own,
            chunk: maximum_request
                .saturating_sub(CHANGE_PROPERTY_HEADER)
                .max(4096),
            clipboard: Held::default(),
            primary: Held::default(),
            focused: None,
            to_wayland: HashMap::new(),
            to_x11: HashMap::new(),
            next_transfer: 1,
            next_owner: 1,
            names: HashMap::new(),
            drags_to_wayland_moved: Vec::new(),
            drags_to_x11_moved: Vec::new(),
        }
    }

    fn post(&self, event: ManagerEvent) -> GoOn {
        self.events.send(event).is_ok()
    }

    fn atom_of(&self, selection: Selection) -> Atom {
        selection.atom(&self.atoms)
    }

    fn selection_of(&self, atom: Atom) -> Option<Selection> {
        Selection::BOTH
            .into_iter()
            .find(|&selection| self.atom_of(selection) == atom)
    }

    fn held(&mut self, selection: Selection) -> &mut Held {
        match selection {
            Selection::Clipboard => &mut self.clipboard,
            Selection::Primary => &mut self.primary,
        }
    }

    /// The seat's keyboard went to `window`, one of its X11 windows, or to none of them.
    pub(super) fn focus_moved(&mut self, window: Option<Window>) {
        self.focused = window;
    }

    /// `window` was unmapped, destroyed or reparented away: it no longer has the keyboard.
    pub(super) fn window_left(&mut self, window: Window) {
        if self.focused == Some(window) {
            self.focused = None;
        }
    }

    /// `window` is destroyed: the transfers it asked end, with no one left to tell.
    pub(super) fn window_destroyed(&mut self, window: Window) {
        self.window_left(window);
        self.to_x11
            .retain(|_, transfer| transfer.request.requestor != window);
    }

    /// An X11 request of the manager failed with `BadWindow` on `window`: a requestor destroyed
    /// before the manager learnt of it, whose transfers end.
    pub(super) fn window_failed(&mut self, window: Window) -> Result<GoOn, Failure> {
        self.to_x11
            .retain(|_, transfer| transfer.request.requestor != window);
        if self.to_wayland.contains_key(&window) {
            return self.end_to_wayland(window, "its window failed");
        }
        Ok(true)
    }

    /// Yuna's loop says what the seat's Wayland `selection` is, `serial` its number: a Wayland
    /// client's, of these MIME types, for which the manager takes the X11 selection; or none,
    /// and the manager releases it if it holds it.
    pub(super) fn own(
        &mut self,
        selection: Selection,
        serial: u64,
        mime_types: Option<Vec<String>>,
    ) -> Result<(), Failure> {
        self.held(selection).wayland_serial = serial;
        let Some(mime_types) = mime_types else {
            return self.release(selection);
        };
        let targets = self.intern(&targets_of_offer(&mime_types))?;
        if let Some((window, _)) = self.held(selection).x11.take().and_then(|x11| x11.asking) {
            self.connection.destroy_window(window)?;
        }
        let atom = self.atom_of(selection);
        let held = self.held(selection);
        held.wayland = Some(targets);
        held.taking += 1;
        held.owned_at = None;
        self.connection
            .set_selection_owner(self.own, atom, x11rb::CURRENT_TIME)?;
        Ok(())
    }

    /// Releases `selection` if Yuna holds it: at the time it took it, so that an X11 client
    /// that took it since keeps it; when that time is not known yet, only if Yuna still owns it.
    fn release(&mut self, selection: Selection) -> Result<(), Failure> {
        let atom = self.atom_of(selection);
        let held = self.held(selection);
        if held.wayland.take().is_none() {
            return Ok(());
        }
        let time = match held.owned_at.take() {
            Some(time) => time,
            None => {
                let owner = self.connection.get_selection_owner(atom)?.reply()?.owner;
                if owner != self.own {
                    return Ok(());
                }
                x11rb::CURRENT_TIME
            }
        };
        self.connection
            .set_selection_owner(x11rb::NONE, atom, time)?;
        Ok(())
    }

    /// XFixes says `selection`'s owner changed: Yuna's own taking, reported; or an X11 client's,
    /// whose targets are asked; or none. A change reported while one of Yuna's takings is not
    /// came before it, in X11's order: that taking ends it, and it is left.
    pub(super) fn owner_changed(
        &mut self,
        changed: &xfixes::SelectionNotifyEvent,
    ) -> Result<GoOn, Failure> {
        let Some(selection) = self.selection_of(changed.selection) else {
            return Ok(true);
        };
        let own = self.own;
        let held = self.held(selection);
        if changed.owner == own {
            held.taking = held.taking.saturating_sub(1);
            if held.taking == 0 && held.wayland.is_some() {
                held.owned_at = Some(changed.selection_timestamp);
            }
            return Ok(true);
        }
        if held.taking > 0 {
            return Ok(true);
        }
        held.wayland = None;
        held.owned_at = None;
        let previous = held.x11.take();
        if let Some(previous) = previous {
            if let Some((window, _)) = previous.asking {
                self.connection.destroy_window(window)?;
            }
            // Its client gone, the transfers it served get nothing more.
            if changed.subtype == SelectionEvent::SELECTION_CLIENT_CLOSE {
                let served: Vec<Window> = self
                    .to_wayland
                    .iter()
                    .filter(|(_, transfer)| {
                        transfer.from
                            == ReadFrom::Selection {
                                selection,
                                owner: previous.number,
                            }
                    })
                    .map(|(&window, _)| window)
                    .collect();
                for window in served {
                    if !self.end_to_wayland(window, "its owner's client left")? {
                        return Ok(false);
                    }
                }
            }
        }
        if changed.owner == x11rb::NONE {
            return Ok(self.post(ManagerEvent::SelectionDropped(selection)));
        }
        let number = self.next_owner;
        self.next_owner += 1;
        let window = self.transfer_window()?;
        self.connection.convert_selection(
            window,
            changed.selection,
            self.atoms.TARGETS,
            self.atoms._YUNA_SELECTION,
            x11rb::CURRENT_TIME,
        )?;
        self.held(selection).x11 = Some(X11Owner {
            number,
            offer: None,
            asking: Some((window, Instant::now())),
        });
        Ok(true)
    }

    /// The window `selection`'s X11 owner's `TARGETS` are converted onto, until they come.
    fn asks_on(&self, selection: Selection) -> Option<Window> {
        let held = match selection {
            Selection::Clipboard => &self.clipboard,
            Selection::Primary => &self.primary,
        };
        held.x11.as_ref()?.asking.map(|(window, _)| window)
    }

    /// A window for one conversion: a child of the manager's own, so that the root's
    /// notifications never name it, listening to its properties' changes.
    pub(super) fn transfer_window(&self) -> Result<Window, Failure> {
        let window = self.connection.generate_id()?;
        self.connection.create_window(
            0,
            window,
            self.own,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        Ok(window)
    }

    /// An owner answered a conversion of the manager's: its `TARGETS`, or a transfer's data. Any
    /// X11 client can send any window such an answer (`SendEvent`), the owner's own as well: one
    /// that names another window, selection or target than a conversion waiting, or comes after
    /// the first, is left. One that names them all cannot be told from the owner's, and ends that
    /// conversion alone. A drag's read refused is asked again when its source moved on since, a
    /// newer time in `drag_time`, the drag's ([`ask_again`]).
    pub(super) fn converted(
        &mut self,
        answer: &SelectionNotifyEvent,
        drag_time: impl Fn(u64) -> Option<Timestamp>,
    ) -> Result<GoOn, Failure> {
        let window = answer.requestor;
        let targets = self.atoms.TARGETS;
        let asking = Selection::BOTH.into_iter().find(|&selection| {
            self.asks_on(selection) == Some(window)
                && answer.selection == self.atom_of(selection)
                && answer.target == targets
        });
        if let Some(selection) = asking {
            let answered = answer.property != x11rb::NONE;
            return self.targets_answered(selection, window, answered);
        }
        let atoms = self.atoms;
        let Some(transfer) = self.to_wayland.get_mut(&window) else {
            return Ok(true);
        };
        let asked = transfer.stage == Fetch::Converting
            && answer.selection == transfer.selection
            && answer.target == transfer.target;
        if !asked {
            return Ok(true);
        }
        // Refused, or answered in a property the manager did not name.
        if answer.property != atoms._YUNA_SELECTION {
            let newest = match transfer.from {
                ReadFrom::Drag(drag) => drag_time(drag),
                ReadFrom::Selection { .. } => None,
            };
            let Some(time) = ask_again(transfer.time, newest, transfer.asks) else {
                return self.end_to_wayland(window, "its owner refused it");
            };
            transfer.time = time;
            transfer.asks += 1;
            transfer.progress = Instant::now();
            let (selection, target) = (transfer.selection, transfer.target);
            self.connection.convert_selection(
                window,
                selection,
                target,
                atoms._YUNA_SELECTION,
                time,
            )?;
            return Ok(true);
        }
        transfer.stage = Fetch::Reading {
            offset: 0,
            incr: false,
        };
        transfer.progress = Instant::now();
        self.pump_to_wayland(window)
    }

    /// An X11 owner's `TARGETS` came on `window`, or did not: what it offers is read, mapped to
    /// MIME types and posted, nothing when it refused or sent no list of atoms.
    fn targets_answered(
        &mut self,
        selection: Selection,
        window: Window,
        answered: bool,
    ) -> Result<GoOn, Failure> {
        let atoms = if answered {
            self.read_atoms(window)?
        } else {
            Vec::new()
        };
        self.connection.destroy_window(window)?;
        let named = self.names_of(&atoms)?;
        let offer = offer_of_targets(&named);
        let held = self.held(selection);
        let wayland_serial = held.wayland_serial;
        let Some(owner) = held.x11.as_mut() else {
            return Ok(true);
        };
        owner.asking = None;
        let event = ManagerEvent::SelectionTaken {
            selection,
            owner: owner.number,
            mime_types: offer
                .iter()
                .map(|(mime_type, _)| mime_type.clone())
                .collect(),
            wayland_serial,
        };
        owner.offer = Some(offer);
        Ok(self.post(event))
    }

    /// The atoms of the list on `window`'s conversion property, at most [`MAX_TARGETS`]: none
    /// when it holds no list of 32-bit values.
    fn read_atoms(&self, window: Window) -> Result<Vec<Atom>, Failure> {
        let reply = self
            .connection
            .get_property(
                true,
                window,
                self.atoms._YUNA_SELECTION,
                AtomEnum::ANY,
                0,
                MAX_TARGETS,
            )?
            .reply();
        let Some(reply) = refused_is_none(reply)? else {
            return Ok(Vec::new());
        };
        Ok(reply
            .value32()
            .map(|atoms| atoms.collect())
            .unwrap_or_default())
    }

    /// The names of `atoms`, those X11 knows, asked at once for those not known yet.
    pub(super) fn names_of(&mut self, atoms: &[Atom]) -> Result<Vec<(Atom, String)>, Failure> {
        if self.names.len() > MAX_NAMES {
            self.names.clear();
        }
        let mut unknown: Vec<Atom> = atoms
            .iter()
            .copied()
            .filter(|atom| !self.names.contains_key(atom))
            .collect();
        unknown.sort_unstable();
        unknown.dedup();
        let cookies = unknown
            .into_iter()
            .map(|atom| Ok((atom, self.connection.get_atom_name(atom)?)))
            .collect::<Result<Vec<_>, ConnectionError>>()?;
        for (atom, cookie) in cookies {
            let name = refused_is_none(cookie.reply())?
                .and_then(|reply| String::from_utf8(reply.name).ok());
            self.names.insert(atom, name);
        }
        Ok(atoms
            .iter()
            .filter_map(|atom| {
                let name = self.names.get(atom)?.clone()?;
                Some((*atom, name))
            })
            .collect())
    }

    /// The atoms of `targets`' names, asked at once, each with its MIME type.
    pub(super) fn intern(
        &mut self,
        targets: &[(String, String)],
    ) -> Result<Vec<(Atom, String)>, Failure> {
        let cookies = targets
            .iter()
            .map(|(name, mime_type)| {
                Ok((
                    name,
                    mime_type,
                    self.connection.intern_atom(false, name.as_bytes())?,
                ))
            })
            .collect::<Result<Vec<_>, ConnectionError>>()?;
        let mut interned: Vec<(Atom, String)> = Vec::with_capacity(cookies.len());
        for (name, mime_type, cookie) in cookies {
            let Some(reply) = refused_is_none(cookie.reply())? else {
                continue;
            };
            if !interned.iter().any(|(atom, _)| *atom == reply.atom) {
                interned.push((reply.atom, mime_type.clone()));
            }
            if self.names.len() <= MAX_NAMES {
                self.names.insert(reply.atom, Some(name.clone()));
            }
        }
        Ok(interned)
    }

    /// A Wayland client reads `selection`, offered by X11 owner `owner`, as `mime_type`, into
    /// `pipe`: converted onto a new window, or refused, the pipe closed, when that owner no
    /// longer holds it, its offer lacks the type, or [`MAX_X11_TRANSFERS`] run.
    pub(super) fn send_to_wayland(
        &mut self,
        selection: Selection,
        owner: u64,
        mime_type: &str,
        pipe: OwnedFd,
    ) -> Result<GoOn, Failure> {
        let target = self
            .held(selection)
            .x11
            .as_ref()
            .filter(|x11| x11.number == owner)
            .and_then(|x11| x11.offer.as_ref())
            .and_then(|offer| offer.iter().find(|(offered, _)| offered == mime_type))
            .map(|&(_, target)| target);
        let from = ReadFrom::Selection { selection, owner };
        let atom = self.atom_of(selection);
        self.start_to_wayland(from, atom, target, x11rb::CURRENT_TIME, pipe)
    }

    /// A Wayland target reads X11 drag `drag`, its `XdndSelection`, as `target`, the target of
    /// one of its MIME types, none when it offers no such type, into `pipe`: converted with the
    /// drag's `time`, as XDND asks, or refused as [`Bridge::send_to_wayland`] refuses.
    pub(super) fn send_drag_to_wayland(
        &mut self,
        drag: u64,
        target: Option<Atom>,
        time: Timestamp,
        pipe: OwnedFd,
    ) -> Result<GoOn, Failure> {
        let atom = self.atoms.XdndSelection;
        self.start_to_wayland(ReadFrom::Drag(drag), atom, target, time, pipe)
    }

    /// Starts a transfer to Wayland: `selection` converted to `target` at `time` onto a new
    /// window, its answer written into `pipe`. Refused, the pipe closed, without a target, with
    /// a pipe that cannot be made non-blocking, or when [`MAX_X11_TRANSFERS`] run. Its end, a
    /// refusal included, is posted.
    fn start_to_wayland(
        &mut self,
        from: ReadFrom,
        selection: Atom,
        target: Option<Atom>,
        time: Timestamp,
        pipe: OwnedFd,
    ) -> Result<GoOn, Failure> {
        let non_blocking =
            fcntl_getfl(&pipe).and_then(|flags| fcntl_setfl(&pipe, flags | OFlags::NONBLOCK));
        let refusal = match (target, non_blocking) {
            (None, _) => Some("its owner or type is gone"),
            (_, Err(_)) => Some("its pipe cannot be made non-blocking"),
            _ if self.to_wayland.len() >= MAX_X11_TRANSFERS => Some("too many transfers run"),
            _ => None,
        };
        let Some(target) = target.filter(|_| refusal.is_none()) else {
            tracing::debug!(reason = refusal, "X11's data is not sent to Wayland");
            drop(pipe);
            return Ok(self.post(ManagerEvent::TransferToWaylandEnded));
        };
        let window = self.transfer_window()?;
        self.connection.convert_selection(
            window,
            selection,
            target,
            self.atoms._YUNA_SELECTION,
            time,
        )?;
        self.to_wayland.insert(
            window,
            ToWayland {
                from,
                selection,
                target,
                time,
                asks: 1,
                pipe,
                stage: Fetch::Converting,
                pending: Vec::new(),
                written: 0,
                total: 0,
                progress: Instant::now(),
            },
        );
        Ok(true)
    }

    /// Writes what transfer `window` read into its pipe, reads what comes next, and so on, until
    /// the pipe is full, X11 has nothing yet, or the transfer ends.
    fn pump_to_wayland(&mut self, window: Window) -> Result<GoOn, Failure> {
        let Some(transfer) = self.to_wayland.remove(&window) else {
            return Ok(true);
        };
        let from = transfer.from;
        match self.fetch_and_write(window, transfer)? {
            Step::Going(transfer) => {
                self.to_wayland.insert(window, transfer);
                Ok(true)
            }
            Step::Ended => {
                self.ended_to_wayland(from, Instant::now());
                self.closed_to_wayland(window, None)
            }
            // Dropped, the transfer closes its pipe.
            Step::Failed(transfer, reason) => {
                self.ended_to_wayland(from, transfer.progress);
                self.closed_to_wayland(window, Some(reason))
            }
        }
    }

    /// A transfer to Wayland that last moved `at` ended: for a drag's, its time limit runs from
    /// then.
    fn ended_to_wayland(&mut self, from: ReadFrom, at: Instant) {
        if let ReadFrom::Drag(drag) = from {
            note_move(&mut self.drags_to_wayland_moved, drag, at);
        }
    }

    /// [`Bridge::pump_to_wayland`]'s work on `transfer`.
    fn fetch_and_write(
        &self,
        window: Window,
        mut transfer: ToWayland,
    ) -> Result<Step<ToWayland>, Failure> {
        let property = self.atoms._YUNA_SELECTION;
        loop {
            while transfer.written < transfer.pending.len() {
                match rustix::io::write(&transfer.pipe, &transfer.pending[transfer.written..]) {
                    Ok(written) => {
                        transfer.written += written;
                        transfer.progress = Instant::now();
                    }
                    Err(Errno::AGAIN) => return Ok(Step::Going(transfer)),
                    Err(Errno::INTR) => {}
                    // EPIPE most often, not SIGPIPE: Rust's runtime ignores that signal.
                    Err(_) => return Ok(Step::Failed(transfer, "its reader closed its pipe")),
                }
            }
            transfer.pending.clear();
            transfer.written = 0;
            let (offset, incr) = match transfer.stage {
                Fetch::Converting | Fetch::AwaitingChunk { arrived: false } => {
                    return Ok(Step::Going(transfer));
                }
                Fetch::Done => return Ok(Step::Ended),
                Fetch::AwaitingChunk { arrived: true } => (0, true),
                Fetch::Reading { offset, incr } => (offset, incr),
            };
            let reply = self
                .connection
                .get_property(
                    false,
                    window,
                    property,
                    AtomEnum::ANY,
                    offset,
                    PIECE_BYTES / 4,
                )?
                .reply();
            let Some(piece) = refused_is_none(reply)? else {
                return Ok(Step::Failed(transfer, "its window is gone"));
            };
            if piece.type_ == x11rb::NONE {
                return Ok(Step::Failed(transfer, "its owner set no property"));
            }
            transfer.progress = Instant::now();
            if offset == 0 && !incr && piece.type_ == self.atoms.INCR {
                // ICCCM 2.7.2: deleting the property asks for the first chunk.
                self.connection.delete_property(window, property)?;
                transfer.stage = Fetch::AwaitingChunk { arrived: false };
                continue;
            }
            let length = piece.value.len();
            transfer.total += length as u64;
            if transfer.total > MAX_X11_TRANSFER_BYTES {
                return Ok(Step::Failed(
                    transfer,
                    "it is larger than a transfer may be",
                ));
            }
            transfer.stage = if piece.bytes_after > 0 {
                // A piece of a property longer than one is whole 4-byte units.
                let read = u32::try_from(length / 4).unwrap_or(u32::MAX);
                if read == 0 {
                    return Ok(Step::Failed(transfer, "its property reads short"));
                }
                Fetch::Reading {
                    offset: offset.saturating_add(read),
                    incr,
                }
            } else {
                self.connection.delete_property(window, property)?;
                // INCR's last chunk is an empty one.
                let last_chunk = offset == 0 && length == 0;
                if incr && !last_chunk {
                    Fetch::AwaitingChunk { arrived: false }
                } else {
                    Fetch::Done
                }
            };
            transfer.pending = piece.value;
        }
    }

    /// Ends transfer `window` to Wayland, taken from the transfers already: its window goes,
    /// its pipe closes, and Yuna's loop counts it ended.
    fn closed_to_wayland(&mut self, window: Window, failed: Option<&str>) -> Result<GoOn, Failure> {
        if let Some(reason) = failed {
            tracing::debug!(
                reason,
                "a transfer of an X11 selection to Wayland ends short"
            );
        }
        self.connection.destroy_window(window)?;
        Ok(self.post(ManagerEvent::TransferToWaylandEnded))
    }

    /// Ends transfer `window` to Wayland short, for `reason`.
    fn end_to_wayland(&mut self, window: Window, reason: &str) -> Result<GoOn, Failure> {
        let Some(transfer) = self.to_wayland.remove(&window) else {
            return Ok(true);
        };
        self.ended_to_wayland(transfer.from, transfer.progress);
        self.closed_to_wayland(window, Some(reason))
    }

    /// An X11 client asks for one of the seat's selections that Yuna holds: served
    /// ([`Bridge::serve`]) while one of the seat's X11 windows has the keyboard, refused
    /// otherwise.
    pub(super) fn requested(&mut self, asked: &SelectionRequestEvent) -> Result<GoOn, Failure> {
        let Some(selection) = self.selection_of(asked.selection) else {
            return self.refuse(&Request::of(asked));
        };
        if self.focused.is_none() {
            tracing::debug!(
                ?selection,
                "an X11 client asks for a Wayland selection without the keyboard: refused"
            );
            return self.refuse(&Request::of(asked));
        }
        let held = self.held(selection);
        let (offered, owned_at) = (held.wayland.clone(), held.owned_at);
        self.serve(asked, offered, owned_at, Served::Selection(selection))
    }

    /// Answers an X11 client's request for Wayland's data, which Yuna took at `owned_at` and
    /// offers as `offered`, each target with its MIME type, none when it offers nothing: at once
    /// for `TARGETS` and `TIMESTAMP`, and for a drag's `DELETE`, which a target asks after a move
    /// and Wayland's source carries out itself; through a transfer for a data target. Refused
    /// otherwise: an unknown target, a request named with a time before Yuna took the selection,
    /// one of the manager's own windows as the requestor, or past [`MAX_X11_TRANSFERS`].
    pub(super) fn serve(
        &mut self,
        asked: &SelectionRequestEvent,
        offered: Option<Vec<(Atom, String)>>,
        owned_at: Option<Timestamp>,
        served: Served,
    ) -> Result<GoOn, Failure> {
        let request = Request::of(asked);
        let property = request.property;
        // The manager's own windows keep their events; INCR would change their mask.
        let own_window = request.requestor == self.root
            || request.requestor == self.own
            || self.to_wayland.contains_key(&request.requestor)
            || Selection::BOTH
                .into_iter()
                .any(|selection| self.asks_on(selection) == Some(request.requestor));
        let atoms = self.atoms;
        // ICCCM 2.2: a request made before Yuna took the selection asks for an older one.
        let stale = owned_at.is_some_and(|taken| earlier(request.time, taken));
        let offered = match offered {
            Some(targets) if !own_window && !stale => targets,
            _ => return self.refuse(&request),
        };
        if request.target == atoms.TARGETS {
            let mut list = vec![atoms.TARGETS, atoms.TIMESTAMP];
            list.extend(offered.iter().map(|&(target, _)| target));
            self.connection.change_property32(
                PropMode::REPLACE,
                request.requestor,
                property,
                AtomEnum::ATOM,
                &list,
            )?;
            return self.notify(&request, property);
        }
        if request.target == atoms.TIMESTAMP {
            self.connection.change_property32(
                PropMode::REPLACE,
                request.requestor,
                property,
                AtomEnum::INTEGER,
                &[owned_at.unwrap_or(x11rb::CURRENT_TIME)],
            )?;
            return self.notify(&request, property);
        }
        if request.target == atoms.DELETE && matches!(served, Served::Drag(_)) {
            // ICCCM 2.6.3.1: done, with an empty property of type NULL.
            self.connection.change_property8(
                PropMode::REPLACE,
                request.requestor,
                property,
                atoms.NULL,
                &[],
            )?;
            return self.notify(&request, property);
        }
        let mime_type = offered
            .iter()
            .find(|&&(target, _)| target == request.target)
            .map(|(_, mime_type)| mime_type.clone());
        let Some(mime_type) = mime_type else {
            return self.refuse(&request);
        };
        if self.to_x11.len() >= MAX_X11_TRANSFERS {
            tracing::debug!("an X11 client's request is refused: too many transfers run");
            return self.refuse(&request);
        }
        let transfer = self.next_transfer;
        self.next_transfer += 1;
        let type_ = if request.target == atoms.TEXT {
            atoms.UTF8_STRING
        } else {
            request.target
        };
        self.to_x11.insert(
            transfer,
            ToX11 {
                request,
                served,
                type_,
                source: None,
                ended: false,
                buffer: Vec::new(),
                total: 0,
                stage: Answer::AwaitingSource,
                progress: Instant::now(),
            },
        );
        Ok(self.post(match served {
            Served::Selection(selection) => ManagerEvent::ReadWayland {
                selection,
                mime_type,
                transfer,
            },
            Served::Drag(drag) => ManagerEvent::ReadWaylandDrag {
                drag,
                mime_type,
                transfer,
            },
        }))
    }

    /// Refuses `asked`, a request none may be served for.
    pub(super) fn refuse_request(&self, asked: &SelectionRequestEvent) -> Result<GoOn, Failure> {
        self.refuse(&Request::of(asked))
    }

    /// Tells `request`'s requestor its answer is in `property`; none is a refusal.
    fn notify(&self, request: &Request, property: Atom) -> Result<GoOn, Failure> {
        let answer = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property,
        };
        self.connection
            .send_event(false, request.requestor, EventMask::NO_EVENT, answer)?;
        Ok(true)
    }

    fn refuse(&self, request: &Request) -> Result<GoOn, Failure> {
        self.notify(request, x11rb::NONE)
    }

    /// Yuna's loop gives transfer `transfer` the read end of the Wayland source's pipe, or
    /// none: the source cannot be read, and the request is refused.
    pub(super) fn wayland_data(
        &mut self,
        transfer: u64,
        source: Option<OwnedFd>,
    ) -> Result<GoOn, Failure> {
        let Some(sending) = self.to_x11.get_mut(&transfer) else {
            // Ended meanwhile, out of time: the pipe closes.
            return Ok(true);
        };
        let Some(source) = source else {
            return self.end_to_x11(transfer, "its source cannot be read");
        };
        sending.source = Some(source);
        sending.stage = Answer::Collecting;
        sending.progress = Instant::now();
        self.pump_to_x11(transfer)
    }

    /// Reads transfer `transfer`'s source, and sets what came on the requestor's property, whole
    /// or chunk by chunk, until the source has nothing yet, the requestor has not deleted the
    /// last chunk, or the transfer ends.
    fn pump_to_x11(&mut self, transfer: u64) -> Result<GoOn, Failure> {
        let Some(sending) = self.to_x11.remove(&transfer) else {
            return Ok(true);
        };
        let served = sending.served;
        match self.read_and_answer(sending)? {
            Step::Going(sending) => {
                self.to_x11.insert(transfer, sending);
                Ok(true)
            }
            Step::Ended => {
                self.moved_to_x11(served, Instant::now());
                Ok(true)
            }
            Step::Failed(sending, reason) => self.ended_to_x11(&sending, reason),
        }
    }

    /// A transfer to X11 that last moved `at` ended: for a drag's, its time limit runs from
    /// then.
    fn moved_to_x11(&mut self, served: Served, at: Instant) {
        if let Served::Drag(drag) = served {
            note_move(&mut self.drags_to_x11_moved, drag, at);
        }
    }

    /// [`Bridge::pump_to_x11`]'s work on `sending`.
    fn read_and_answer(&self, mut sending: ToX11) -> Result<Step<ToX11>, Failure> {
        let request = sending.request;
        loop {
            let mut moved = false;
            // One chunk more than a property takes, at most: past it, INCR. Read into the
            // buffer's spare room, which nothing fills first.
            if let Some(source) = &sending.source
                && !sending.ended
                && sending.buffer.len() <= self.chunk
            {
                let room = self.chunk + 1 - sending.buffer.len();
                sending.buffer.reserve_exact(room);
                match rustix::io::read(source, spare_capacity(&mut sending.buffer)) {
                    Ok(0) => {
                        sending.ended = true;
                        moved = true;
                    }
                    Ok(read) => {
                        sending.total += read as u64;
                        sending.progress = Instant::now();
                        moved = true;
                        if sending.total > MAX_X11_TRANSFER_BYTES {
                            return Ok(Step::Failed(
                                sending,
                                "it is larger than a transfer may be",
                            ));
                        }
                    }
                    Err(Errno::AGAIN) => {}
                    Err(Errno::INTR) => continue,
                    Err(_) => return Ok(Step::Failed(sending, "its source's pipe failed")),
                }
            }
            match sending.stage {
                Answer::AwaitingSource => return Ok(Step::Going(sending)),
                Answer::Collecting if sending.buffer.len() > self.chunk => {
                    // ICCCM 2.7.2: INCR, the property's value a lower bound of the size. The
                    // requestor deleting it asks for each chunk; a window the manager does not
                    // follow is listened to for it.
                    self.connection.change_window_attributes(
                        request.requestor,
                        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                    )?;
                    let size = u32::try_from(sending.buffer.len()).unwrap_or(u32::MAX);
                    self.connection.change_property32(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        self.atoms.INCR,
                        &[size],
                    )?;
                    self.notify(&request, request.property)?;
                    sending.stage = Answer::Incr { deleted: false };
                }
                Answer::Collecting if sending.ended => {
                    self.connection.change_property8(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        sending.type_,
                        &sending.buffer,
                    )?;
                    self.notify(&request, request.property)?;
                    return Ok(Step::Ended);
                }
                Answer::Incr { deleted: true }
                    if sending.buffer.len() >= self.chunk
                        || (sending.ended && !sending.buffer.is_empty()) =>
                {
                    let length = sending.buffer.len().min(self.chunk);
                    self.connection.change_property8(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        sending.type_,
                        &sending.buffer[..length],
                    )?;
                    sending.buffer.drain(..length);
                    sending.stage = Answer::Incr { deleted: false };
                    sending.progress = Instant::now();
                }
                Answer::Incr { deleted: true } if sending.ended => {
                    // The last chunk, an empty one.
                    self.connection.change_property8(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        sending.type_,
                        &[],
                    )?;
                    return Ok(Step::Ended);
                }
                Answer::Collecting | Answer::Incr { .. } if !moved => {
                    return Ok(Step::Going(sending));
                }
                Answer::Collecting | Answer::Incr { .. } => {}
            }
        }
    }

    /// Ends transfer `transfer` to X11 short, for `reason`: refused while its requestor was told
    /// nothing; mid-INCR, no next chunk comes, as ICCCM has no other way to say it.
    fn end_to_x11(&mut self, transfer: u64, reason: &str) -> Result<GoOn, Failure> {
        let Some(sending) = self.to_x11.remove(&transfer) else {
            return Ok(true);
        };
        self.ended_to_x11(&sending, reason)
    }

    /// [`Bridge::end_to_x11`] for `sending`, taken from the transfers already.
    fn ended_to_x11(&mut self, sending: &ToX11, reason: &str) -> Result<GoOn, Failure> {
        self.moved_to_x11(sending.served, sending.progress);
        tracing::debug!(
            reason,
            "a transfer of a Wayland selection to X11 ends short"
        );
        match sending.stage {
            Answer::AwaitingSource | Answer::Collecting => self.refuse(&sending.request),
            Answer::Incr { .. } => Ok(true),
        }
    }

    /// A property changed: an INCR chunk set on a transfer's window, or the property a
    /// requestor deleted to ask for the next chunk. Others are not the bridge's.
    pub(super) fn property(&mut self, changed: &PropertyNotifyEvent) -> Result<GoOn, Failure> {
        if changed.state == Property::NEW_VALUE && changed.atom == self.atoms._YUNA_SELECTION {
            let Some(transfer) = self.to_wayland.get_mut(&changed.window) else {
                return Ok(true);
            };
            if transfer.stage != (Fetch::AwaitingChunk { arrived: false }) {
                return Ok(true);
            }
            transfer.stage = Fetch::AwaitingChunk { arrived: true };
            transfer.progress = Instant::now();
            return self.pump_to_wayland(changed.window);
        }
        if changed.state != Property::DELETE {
            return Ok(true);
        }
        let deleted = self.to_x11.iter().find_map(|(&transfer, sending)| {
            let asked = sending.request.requestor == changed.window
                && sending.request.property == changed.atom
                && sending.stage == (Answer::Incr { deleted: false });
            asked.then_some(transfer)
        });
        let Some(transfer) = deleted else {
            return Ok(true);
        };
        if let Some(sending) = self.to_x11.get_mut(&transfer) {
            sending.stage = Answer::Incr { deleted: true };
            sending.progress = Instant::now();
        }
        self.pump_to_x11(transfer)
    }

    /// Moves every transfer as far as it goes without waiting, and ends those that made no
    /// progress for [`X11_TRANSFER_TIMEOUT`], an owner's `TARGETS` that never came among them.
    pub(super) fn run(&mut self) -> Result<GoOn, Failure> {
        let now = Instant::now();
        let overdue = |progress: Instant| now.duration_since(progress) >= X11_TRANSFER_TIMEOUT;
        for selection in Selection::BOTH {
            let asking = self.held(selection).x11.as_ref().and_then(|x11| x11.asking);
            if let Some((window, asked)) = asking
                && overdue(asked)
                && !self.targets_answered(selection, window, false)?
            {
                return Ok(false);
            }
        }
        let windows: Vec<Window> = self.to_wayland.keys().copied().collect();
        for window in windows {
            let late = self
                .to_wayland
                .get(&window)
                .is_some_and(|transfer| overdue(transfer.progress));
            let go_on = if late {
                self.end_to_wayland(window, "it made no progress in time")?
            } else {
                self.pump_to_wayland(window)?
            };
            if !go_on {
                return Ok(false);
            }
        }
        let transfers: Vec<u64> = self.to_x11.keys().copied().collect();
        for transfer in transfers {
            let late = self
                .to_x11
                .get(&transfer)
                .is_some_and(|sending| overdue(sending.progress));
            let go_on = if late {
                self.end_to_x11(transfer, "it made no progress in time")?
            } else {
                self.pump_to_x11(transfer)?
            };
            if !go_on {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The pipes the manager's sleep watches, and for what: those a transfer waits to write
    /// into or to read from.
    pub(super) fn waits(&self) -> Vec<(BorrowedFd<'_>, PollFlags)> {
        let writes = self
            .to_wayland
            .values()
            .filter(|transfer| transfer.written < transfer.pending.len())
            .map(|transfer| (transfer.pipe.as_fd(), PollFlags::OUT));
        let reads = self.to_x11.values().filter_map(|sending| {
            let source = sending.source.as_ref()?;
            let room = !sending.ended && sending.buffer.len() <= self.chunk;
            room.then(|| (source.as_fd(), PollFlags::IN))
        });
        writes.chain(reads).collect()
    }

    /// When a transfer of X11 drag `drag` to Wayland last moved, running or ended; none if none
    /// ran.
    pub(super) fn drag_to_wayland_progress(&self, drag: u64) -> Option<Instant> {
        let running = self
            .to_wayland
            .values()
            .filter(|transfer| transfer.from == ReadFrom::Drag(drag))
            .map(|transfer| transfer.progress);
        let ended = self
            .drags_to_wayland_moved
            .iter()
            .filter(|(of, _)| *of == drag)
            .map(|&(_, at)| at);
        running.chain(ended).max()
    }

    /// When a transfer of Wayland drag `drag` to X11 last moved, running or ended; none if none
    /// ran.
    pub(super) fn drag_to_x11_progress(&self, drag: u64) -> Option<Instant> {
        let running = self
            .to_x11
            .values()
            .filter(|sending| sending.served == Served::Drag(drag))
            .map(|sending| sending.progress);
        let ended = self
            .drags_to_x11_moved
            .iter()
            .filter(|(of, _)| *of == drag)
            .map(|&(_, at)| at);
        running.chain(ended).max()
    }

    /// Keeps the last moves of the drags the manager still follows, X11 programs' to Wayland and
    /// Wayland clients' to X11, and forgets the others': at most the drags it follows, and those
    /// whose transfers ended in one turn.
    pub(super) fn keep_drag_moves(
        &mut self,
        follows_x11: impl Fn(u64) -> bool,
        follows_wayland: impl Fn(u64) -> bool,
    ) {
        self.drags_to_wayland_moved
            .retain(|&(drag, _)| follows_x11(drag));
        self.drags_to_x11_moved
            .retain(|&(drag, _)| follows_wayland(drag));
    }

    /// X11 drag `drag`'s source left: the transfers reading it end.
    pub(super) fn end_drag_to_wayland(&mut self, drag: u64) -> Result<GoOn, Failure> {
        let reading: Vec<Window> = self
            .to_wayland
            .iter()
            .filter(|(_, transfer)| transfer.from == ReadFrom::Drag(drag))
            .map(|(&window, _)| window)
            .collect();
        for window in reading {
            if !self.end_to_wayland(window, "its drag's source left")? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// When the first transfer, or the first owner's `TARGETS`, runs out of time, if one runs.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        let asked = [&self.clipboard, &self.primary]
            .into_iter()
            .filter_map(|held| held.x11.as_ref()?.asking.map(|(_, asked)| asked));
        let moved = self
            .to_wayland
            .values()
            .map(|transfer| transfer.progress)
            .chain(self.to_x11.values().map(|sending| sending.progress));
        asked
            .chain(moved)
            .min()
            .map(|earliest| earliest + X11_TRANSFER_TIMEOUT)
    }
}

/// The time a refused read of a drag is asked again with: the drag's `newest`, once its source
/// moved on since the read named `asked_with`, while it was asked fewer than [`MAX_DRAG_ASKS`]
/// times; none otherwise, and the read ends.
fn ask_again(asked_with: Timestamp, newest: Option<Timestamp>, asks: u8) -> Option<Timestamp> {
    newest.filter(|&newest| newest != asked_with && asks < MAX_DRAG_ASKS)
}

/// Whether X11 time `time` is before `than`: `CurrentTime` never is. X11's times are
/// milliseconds that wrap around in 49.7 days, compared as X11's server compares them: `time`
/// is earlier when `than` lies less than half the range after it.
fn earlier(time: Timestamp, than: Timestamp) -> bool {
    time != x11rb::CURRENT_TIME && (1..1 << 31).contains(&than.wrapping_sub(time))
}

/// A reply, or none when X11 refused the request, for a window or an atom that does not exist; a
/// lost connection fails.
pub(super) fn refused_is_none<R>(reply: Result<R, ReplyError>) -> Result<Option<R>, Failure> {
    match reply {
        Ok(reply) => Ok(Some(reply)),
        Err(ReplyError::X11Error(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(names: &[(Atom, &str)]) -> Vec<(Atom, String)> {
        names
            .iter()
            .map(|&(atom, name)| (atom, name.to_owned()))
            .collect()
    }

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    /// A drag's read refused for an older position is asked again with the newest, a bounded
    /// number of times; a read refused at the newest, or a selection's, ends.
    #[test]
    fn a_drags_read_refused_for_an_older_position_is_asked_again() {
        assert_eq!(ask_again(10, Some(12), 1), Some(12));
        assert_eq!(ask_again(12, Some(12), 1), None, "refused at the newest");
        assert_eq!(ask_again(10, None, 1), None, "no drag");
        assert_eq!(ask_again(10, Some(12), MAX_DRAG_ASKS), None, "asked enough");
    }

    /// A drag's transfers that ended keep their last move, so that its time limit runs from it:
    /// each drag at its latest move, until the manager forgets the drag.
    #[test]
    fn the_last_move_of_a_drags_ended_transfers_is_kept() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut moves = Vec::new();
        note_move(&mut moves, 1, at(10));
        note_move(&mut moves, 1, at(5));
        assert_eq!(moves, [(1, at(10))], "the latest move stays");
        for drag in 2..=5 {
            note_move(&mut moves, drag, at(drag));
        }
        assert_eq!(moves.len(), 5, "no drag is forgotten for a newer one");
    }

    #[test]
    fn a_time_is_earlier_within_half_the_range_and_current_time_never_is() {
        assert!(earlier(10, 11));
        assert!(!earlier(11, 11));
        assert!(!earlier(12, 11));
        // Across the wrap: u32::MAX is a millisecond before 0, 1 before 2^31.
        assert!(earlier(u32::MAX, 0));
        assert!(earlier(1, 1 << 31));
        assert!(!earlier(0, 1 << 31));
        assert!(!earlier(x11rb::CURRENT_TIME, 5));
    }

    #[test]
    fn x11_text_targets_become_text_mime_types_and_mime_named_targets_stay() {
        let targets = named(&[
            (1, "TARGETS"),
            (2, "TIMESTAMP"),
            (3, "UTF8_STRING"),
            (4, "COMPOUND_TEXT"),
            (5, "TEXT"),
            (6, "STRING"),
            (7, "text/html"),
            (8, "image/png"),
        ]);
        let offer = offer_of_targets(&targets);
        assert_eq!(
            offer,
            [
                ("text/html".to_owned(), 7),
                ("image/png".to_owned(), 8),
                (UTF8_TEXT.to_owned(), 3),
                // STRING before TEXT, whatever the owner's order.
                (PLAIN_TEXT.to_owned(), 6),
            ]
        );
    }

    #[test]
    fn an_owner_naming_a_text_type_itself_is_read_as_named() {
        let targets = named(&[(3, "UTF8_STRING"), (9, UTF8_TEXT), (5, "TEXT")]);
        assert_eq!(
            offer_of_targets(&targets),
            [(UTF8_TEXT.to_owned(), 9), (PLAIN_TEXT.to_owned(), 5)]
        );
    }

    #[test]
    fn names_that_are_no_mime_type_are_left() {
        for name in [
            "",
            "/",
            "text/",
            "/plain",
            "text/pl\u{e9}in",
            "SAVE_TARGETS",
        ] {
            assert!(!is_mime_type(name), "{name:?}");
        }
        assert!(is_mime_type("text/plain; charset=utf-8"));
        // 255 bytes, the bound, then one more.
        assert!(is_mime_type(&format!("text/{}", "x".repeat(250))));
        assert!(!is_mime_type(&format!("text/{}", "x".repeat(251))));
        assert!(offer_of_targets(&named(&[(1, "PIXMAP"), (2, "BITMAP")])).is_empty());
    }

    #[test]
    fn wayland_text_is_offered_under_x11s_text_targets_too() {
        let targets = targets_of_offer(&strings(&[UTF8_TEXT, PLAIN_TEXT, "text/html"]));
        let pairs: Vec<(&str, &str)> = targets
            .iter()
            .map(|(target, mime_type)| (target.as_str(), mime_type.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                (UTF8_TEXT, UTF8_TEXT),
                (PLAIN_TEXT, PLAIN_TEXT),
                ("text/html", "text/html"),
                ("UTF8_STRING", UTF8_TEXT),
                ("STRING", PLAIN_TEXT),
                ("TEXT", PLAIN_TEXT),
            ]
        );
    }

    #[test]
    fn one_text_type_serves_every_text_target() {
        let only_plain = targets_of_offer(&strings(&[PLAIN_TEXT]));
        assert!(only_plain.contains(&("UTF8_STRING".to_owned(), PLAIN_TEXT.to_owned())));
        let only_utf8 = targets_of_offer(&strings(&[UTF8_TEXT]));
        assert!(only_utf8.contains(&("STRING".to_owned(), UTF8_TEXT.to_owned())));
        assert!(only_utf8.contains(&("TEXT".to_owned(), UTF8_TEXT.to_owned())));
    }

    #[test]
    fn a_source_naming_x11s_text_targets_itself_keeps_them_once() {
        // wl-copy offers these beside the MIME types.
        let targets = targets_of_offer(&strings(&[UTF8_TEXT, "UTF8_STRING", "STRING", "TEXT"]));
        let names: Vec<&str> = targets.iter().map(|(target, _)| target.as_str()).collect();
        assert_eq!(names, [UTF8_TEXT, "UTF8_STRING", "STRING", "TEXT"]);
    }

    #[test]
    fn a_selection_of_no_text_offers_no_text_target() {
        let targets = targets_of_offer(&strings(&["image/png"]));
        assert_eq!(targets, [("image/png".to_owned(), "image/png".to_owned())]);
        assert!(targets_of_offer(&[]).is_empty());
    }
}
