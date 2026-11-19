//! The clipboard and the primary selection: `wl_data_device`, the primary selection
//! (`zwp_primary_selection_device_manager_v1`) and `ext-data-control-v1`, on Smithay's handlers.
//! Each seat has its own, which follow its keyboard focus: a client is offered them just before
//! its `enter` ([`State::give_selections_to`]), and only the client with the focus may set them.
//! A seat's focus is only on its own clients, so a selection never crosses seats. Between Wayland
//! clients, Yuna never touches the data they pass each other: a reader's pipe goes to the
//! source's client, and a selection lasts as long as its source. `ext-data-control-v1` reaches
//! every seat's selections, so only the owner's seat's clients see it.
//!
//! Yuna also holds a seat's selection itself ([`HeldSelection`]), for two kinds of owner. For an
//! X11 client of the seat ([`X11Offer`]), it offers the owner's MIME types as the seat's
//! selection, and each read of it goes, with the reader's pipe, to the seat's X11 window manager,
//! which writes the data on its own thread (`xwayland/selections.rs`). For an agent, through MCP's
//! `clipboard set`, it offers text as [`AGENT_MIME_TYPES`] and writes it into each reader's pipe
//! itself, on the loop, at the reader's pace, never waiting: [`MAX_CLIPBOARD_WRITES`] at once on
//! a seat, each ended once [`CLIPBOARD_TIMEOUT`] passes without progress. And it reads a seat's
//! selection for MCP's `clipboard get`, whoever holds it, as a client's paste would
//! ([`SelectionRead`]). A seat removed leaves none of it behind, though Smithay keeps its record.
//!
//! It learns when a Wayland client sets a selection, clears it, or its source goes, and when an
//! agent sets or clears one: it records the MIME types, which X11 is offered. Yuna's fork of
//! Smithay tells of a source that goes (`docs/smithay-patches.md`, patch 6). The flow between
//! X11 and Wayland is `xwayland/bridge.rs`'s, step by step; what falls short is in
//! `docs/debt.md`.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::io::Errno;
use rustix::pipe::{PipeFlags, pipe_with};
use smithay::input::Seat;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use smithay::wayland::selection::data_device::{
    self, DataDeviceHandler, DataDeviceState, clear_data_device_selection,
    current_data_device_selection_userdata, request_data_device_client_selection,
    set_data_device_focus, set_data_device_selection,
};
use smithay::wayland::selection::ext_data_control::{DataControlHandler, DataControlState};
use smithay::wayland::selection::primary_selection::{
    self, PrimarySelectionHandler, PrimarySelectionState, clear_primary_selection,
    current_primary_selection_userdata, request_primary_client_selection, set_primary_focus,
    set_primary_selection,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};

use yuna_layout::SeatId;

use crate::rest::{Source, Wake, insert_source};
use crate::seats::{seat_id, seat_of_client};
use crate::state::{OWNER_SEAT, State};
use crate::xwayland::{MAX_MIME_TYPE_BYTES, MAX_MIME_TYPES, Selection};

/// The MIME types an agent's text is offered as, UTF-8 under each: what Wayland programs paste
/// text as; X11 is offered `UTF8_STRING`, `STRING` and `TEXT` from them.
pub(crate) const AGENT_MIME_TYPES: [&str; 3] =
    ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];

/// How long one of Yuna's own transfers of a selection may wait on its peer. MCP's `clipboard
/// get` waits this long in all for the program that copied, then answers with what came: an
/// agent waits for that answer. Yuna's write of an agent's text into one reader's pipe ends once
/// this long passes with no byte taken, as an X11 transfer does
/// ([`X11_TRANSFER_TIMEOUT`](crate::X11_TRANSFER_TIMEOUT), ADR-0091): a reader that drains a long
/// text slowly is never cut. A program answers a paste in milliseconds.
pub const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(5);

/// How many of Yuna's writes of an agent's text run at once on a seat, one per read: a paste asks
/// for a type or two, a clipboard manager for a few more. A read past them is refused, its pipe
/// closed at once: the reader sees it end, empty.
pub const MAX_CLIPBOARD_WRITES: usize = 8;

/// The selection protocols' globals and Smithay's state for each, what Wayland's side last set
/// as each seat's selections, for X11, and Yuna's writes of agents' selections.
pub(crate) struct Selections {
    data_device: DataDeviceState,
    primary: PrimarySelectionState,
    data_control: DataControlState,
    set_on_wayland: HashMap<(SeatId, Selection), WaylandSelection>,
    writes: HashMap<u64, ClipboardWrite>,
    /// The next write's key: keys increase and are never reused.
    next_write: u64,
    /// The writes asked during the round, each with its seat, its reader's pipe, non-blocking,
    /// and the text: started at the round's end, where the loop's handle is.
    unstarted: Vec<(SeatId, OwnedFd, Arc<[u8]>)>,
    /// The writes whose pipe may take more, and those whose time is up or whose seat went, by
    /// key: their sources only note them.
    writable: Vec<u64>,
    ended: Vec<u64>,
}

/// A seat's selection as Wayland's side last set it, for X11: a serial counting each change,
/// and the MIME types of the selection a client or an agent set, none once it was cleared,
/// replaced by an X11 owner's, or its source is gone.
#[derive(Debug, Clone, Default)]
pub(crate) struct WaylandSelection {
    pub serial: u64,
    pub mime_types: Option<Vec<String>>,
}

/// A seat's selection Yuna holds itself, and for whom: Smithay's `SelectionUserData`, of which
/// each offer a client is given keeps a copy.
#[derive(Debug, Clone)]
pub(crate) enum HeldSelection {
    /// An X11 client's of the seat, offering `mime_types`: a read goes to its manager.
    X11 {
        offer: X11Offer,
        mime_types: Vec<String>,
    },
    /// An agent's text, through MCP's `clipboard set`, offered as [`AGENT_MIME_TYPES`]: Yuna
    /// writes it to each reader.
    Agent(Arc<[u8]>),
}

/// A seat's selection Yuna holds for an X11 owner: X11's `selection` of Xwayland `server`, whose
/// manager saw its owner `owner`-th. A Wayland client's read of it goes to that manager, which
/// reads that owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct X11Offer {
    pub server: u64,
    pub selection: Selection,
    pub owner: u64,
}

/// What reading a seat's selection gives MCP's `clipboard get`.
#[derive(Debug)]
pub(crate) enum SelectionRead {
    /// Nothing: the selection turned out empty, its source gone, or it lacks the type.
    Empty,
    /// An agent's text, which Yuna holds whole.
    Held(Arc<[u8]>),
    /// The read end of a pipe, non-blocking, that the selection's source writes into, then
    /// closes.
    Pipe(OwnedFd),
}

/// Yuna writing an agent's text into one reader's pipe, at the reader's pace.
#[derive(Debug)]
struct ClipboardWrite {
    seat: SeatId,
    /// The reader's pipe, non-blocking.
    pipe: OwnedFd,
    text: Arc<[u8]>,
    written: usize,
    /// When it last wrote a byte.
    progress: Instant,
    /// The source that wakes the loop when the pipe may take more, on a copy of the pipe.
    source: RegistrationToken,
    /// The timer that ends it once [`CLIPBOARD_TIMEOUT`] passes without progress.
    timer: RegistrationToken,
}

/// How far a write into a pipe went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Written {
    /// The whole text.
    All,
    /// Until the pipe was full.
    Partly,
    /// The pipe failed: its reader closed it, most often.
    Failed(Errno),
}

/// Writes what is left of `text` past `written` into `pipe`, non-blocking, until it is all
/// written or the pipe is full.
fn write_some(pipe: &OwnedFd, text: &[u8], written: &mut usize) -> Written {
    while *written < text.len() {
        match rustix::io::write(pipe, &text[*written..]) {
            // A pipe never takes nothing while writable; were it to, the timer ends the write.
            Ok(0) => return Written::Partly,
            Ok(count) => *written += count,
            Err(Errno::AGAIN) => return Written::Partly,
            Err(Errno::INTR) => {}
            // EPIPE most often, not SIGPIPE: Rust's runtime ignores that signal.
            Err(error) => return Written::Failed(error),
        }
    }
    Written::All
}

impl Selections {
    /// Creates the three globals. Data control reaches the primary selection too. Every client
    /// of the owner's seat sees it; no agent's client does, since it would read and set any
    /// seat's selections, another agent's among them.
    pub(crate) fn new(display_handle: &DisplayHandle) -> Selections {
        let data_device = DataDeviceState::new::<State>(display_handle);
        let primary = PrimarySelectionState::new::<State>(display_handle);
        let data_control =
            DataControlState::new::<State, _>(display_handle, Some(&primary), |client| {
                seat_of_client(client) == OWNER_SEAT
            });
        Selections {
            data_device,
            primary,
            data_control,
            set_on_wayland: HashMap::new(),
            writes: HashMap::new(),
            next_write: 1,
            unstarted: Vec::new(),
            writable: Vec::new(),
            ended: Vec::new(),
        }
    }

    /// `seat`'s `selection` as Wayland's side last set it, if it ever did.
    pub(crate) fn set_on_wayland(
        &self,
        seat: SeatId,
        selection: Selection,
    ) -> Option<&WaylandSelection> {
        self.set_on_wayland.get(&(seat, selection))
    }

    /// Forgets what Wayland's side set on `seat`, removed, and ends Yuna's writes to its readers
    /// at the round's end.
    fn forget_seat(&mut self, seat: SeatId) {
        self.set_on_wayland.retain(|&(of, _), _| of != seat);
        self.unstarted.retain(|(of, _, _)| *of != seat);
        let of_seat = self
            .writes
            .iter()
            .filter(|(_, write)| write.seat == seat)
            .map(|(&key, _)| key);
        self.ended.extend(of_seat);
    }

    /// How many writes of `seat` run or are about to start.
    fn writes_of(&self, seat: SeatId) -> usize {
        let running = self.writes.values().filter(|write| write.seat == seat);
        let unstarted = self.unstarted.iter().filter(|(of, _, _)| *of == seat);
        running.count() + unstarted.count()
    }

    /// Starts writing `text` into `pipe`, a reader's of an agent's selection of `seat`: as much
    /// as the pipe takes now, then, while some is left, as it takes more. The pipe closes once
    /// the text is written, [`CLIPBOARD_TIMEOUT`] passes without progress, or it fails.
    fn start_write(
        &mut self,
        seat: SeatId,
        pipe: OwnedFd,
        text: Arc<[u8]>,
        handle: &LoopHandle<'static, State>,
    ) {
        let mut written = 0;
        match write_some(&pipe, &text, &mut written) {
            Written::All => return,
            Written::Failed(error) => {
                tracing::debug!(%error, seat = seat.0, "a reader of an agent's selection left");
                return;
            }
            Written::Partly => {}
        }
        let key = self.next_write;
        self.next_write += 1;
        let watched = match pipe.try_clone() {
            Ok(watched) => watched,
            Err(error) => {
                tracing::warn!(%error, "an agent's selection is not written: no descriptor");
                return;
            }
        };
        let source = insert_source(
            handle,
            Wake::of_seat(Source::SelectionWrite, seat),
            Generic::new(watched, Interest::WRITE, Mode::Level),
            move |_, _, state| {
                if !state.selections.writable.contains(&key) {
                    state.selections.writable.push(key);
                }
                Ok(PostAction::Continue)
            },
        );
        let timer = insert_source(
            handle,
            Wake::of_seat(Source::SelectionWriteTimeout, seat),
            Timer::from_duration(CLIPBOARD_TIMEOUT),
            move |_, _, state| {
                let writes = &state.selections.writes;
                match writes.get(&key).map(|write| write.progress.elapsed()) {
                    // It wrote since: its limit runs from then.
                    Some(idle) if idle < CLIPBOARD_TIMEOUT => {
                        TimeoutAction::ToDuration(CLIPBOARD_TIMEOUT - idle)
                    }
                    Some(_) => {
                        state.selections.ended.push(key);
                        TimeoutAction::Drop
                    }
                    None => TimeoutAction::Drop,
                }
            },
        );
        match (source, timer) {
            (Ok(source), Ok(timer)) => {
                self.writes.insert(
                    key,
                    ClipboardWrite {
                        seat,
                        pipe,
                        text,
                        written,
                        progress: Instant::now(),
                        source,
                        timer,
                    },
                );
            }
            (source, timer) => {
                if let Ok(source) = source {
                    handle.remove(source);
                }
                if let Ok(timer) = timer {
                    handle.remove(timer);
                }
                tracing::warn!("an agent's selection is not written whole: the loop refused it");
            }
        }
    }

    /// Ends write `key`, if it runs: its pipe closes, and its reader sees it end.
    fn end_write(&mut self, key: u64, handle: &LoopHandle<'static, State>) -> Option<usize> {
        let write = self.writes.remove(&key)?;
        handle.remove(write.source);
        // A timer that fired has dropped itself: its token is then stale, and nothing happens.
        handle.remove(write.timer);
        Some(write.written)
    }
}

/// Which of a seat's selections Smithay's `target` is.
fn selection_of(target: SelectionTarget) -> Selection {
    match target {
        SelectionTarget::Clipboard => Selection::Clipboard,
        SelectionTarget::Primary => Selection::Primary,
    }
}

/// The MIME types of a source X11 is offered: the first [`MAX_MIME_TYPES`], those no longer than
/// [`MAX_MIME_TYPE_BYTES`].
pub(crate) fn for_x11(mime_types: Vec<String>) -> Vec<String> {
    mime_types
        .into_iter()
        .filter(|mime_type| mime_type.len() <= MAX_MIME_TYPE_BYTES)
        .take(MAX_MIME_TYPES)
        .collect()
}

/// A new pipe for a read of a selection: its read end, non-blocking, and its write end, for the
/// source, blocking, as a client writes it. None when no pipe can be made.
pub(crate) fn reader_pipe() -> Option<(OwnedFd, OwnedFd)> {
    let made = pipe_with(PipeFlags::CLOEXEC)
        .and_then(|(read, write)| fcntl_setfl(&read, OFlags::NONBLOCK).map(|()| (read, write)));
    match made {
        Ok(ends) => Some(ends),
        Err(error) => {
            tracing::warn!(%error, "no pipe for a read of a selection: it is refused");
            None
        }
    }
}

impl State {
    /// Offers `seat`'s clipboard and primary selection to the client of `focused`, the surface
    /// about to get the seat's keyboard focus; with none, to no client. A client that gets them
    /// is told what they hold. Who may set them is the keyboard focus's business, in Smithay:
    /// each seat has its own, and its focus is only ever on its own clients.
    pub(crate) fn give_selections_to(&self, seat: SeatId, focused: Option<&WlSurface>) {
        let Some(seat) = self.seat(seat) else {
            return;
        };
        let client = focused.and_then(Resource::client);
        set_data_device_focus(&self.display_handle, &seat.seat, client.clone());
        set_primary_focus(&self.display_handle, &seat.seat, client);
    }

    /// Makes `mime_types` `seat`'s `selection`, held by Yuna for an X11 owner: a client that
    /// reads it is answered through [`SelectionHandler::send_selection`]. A client's selection it
    /// replaces is cancelled, as when another client sets one. X11 already knows.
    pub(crate) fn hold_x11_selection(
        &mut self,
        seat: SeatId,
        selection: Selection,
        mime_types: Vec<String>,
        offer: X11Offer,
    ) {
        let held = HeldSelection::X11 {
            offer,
            mime_types: mime_types.clone(),
        };
        self.replace_selection(seat, selection, Some((mime_types, held)));
        self.wayland_selection_replaced(seat, selection);
    }

    /// Empties `seat`'s `selection`, whoever holds it, for X11's side: its owner went, or offers
    /// no MIME type. A client's selection is cancelled. X11 already knows.
    pub(crate) fn clear_selection_from_x11(&mut self, seat: SeatId, selection: Selection) {
        self.replace_selection(seat, selection, None);
        self.wayland_selection_replaced(seat, selection);
    }

    /// Makes `text` `seat`'s `selection`, held by Yuna for an agent, offered as
    /// [`AGENT_MIME_TYPES`], until a client or X11 sets another, or an agent clears it. A
    /// client's selection it replaces is cancelled; X11 is offered it at the round's end.
    pub(crate) fn hold_agent_selection(
        &mut self,
        seat: SeatId,
        selection: Selection,
        text: Arc<[u8]>,
    ) {
        let mime_types: Vec<String> = AGENT_MIME_TYPES.map(str::to_owned).into();
        let held = HeldSelection::Agent(text);
        self.replace_selection(seat, selection, Some((mime_types.clone(), held)));
        self.wayland_selection_set(seat, selection, Some(mime_types));
    }

    /// Empties `seat`'s `selection` for an agent, whoever holds it: a client's is cancelled, and
    /// X11 is told at the round's end, the manager releasing the X11 selection it holds for
    /// Wayland. An X11 owner's stays its own in X11 (`docs/debt.md`).
    pub(crate) fn clear_selection_for_agent(&mut self, seat: SeatId, selection: Selection) {
        self.replace_selection(seat, selection, None);
        self.wayland_selection_set(seat, selection, None);
    }

    /// Sets `seat`'s `selection` in Smithay to one Yuna holds, its MIME types and for whom, or
    /// to none: the client with the focus and the data-control clients are told.
    fn replace_selection(
        &mut self,
        seat: SeatId,
        selection: Selection,
        held: Option<(Vec<String>, HeldSelection)>,
    ) {
        let Some(seat_data) = self.seat(seat) else {
            return;
        };
        let (handle, seat) = (&self.display_handle, &seat_data.seat);
        match (selection, held) {
            (Selection::Clipboard, Some((mime_types, held))) => {
                set_data_device_selection(handle, seat, mime_types, held);
            }
            (Selection::Primary, Some((mime_types, held))) => {
                set_primary_selection(handle, seat, mime_types, held);
            }
            (Selection::Clipboard, None) => clear_data_device_selection(handle, seat),
            (Selection::Primary, None) => clear_primary_selection(handle, seat),
        }
    }

    /// The selection Yuna holds as `seat`'s `selection`, if it holds one.
    pub(crate) fn held_selection(
        &self,
        seat: SeatId,
        selection: Selection,
    ) -> Option<HeldSelection> {
        let seat = &self.seat(seat)?.seat;
        match selection {
            Selection::Clipboard => current_data_device_selection_userdata(seat).map(|h| h.clone()),
            Selection::Primary => current_primary_selection_userdata(seat).map(|h| h.clone()),
        }
    }

    /// The MIME types `seat`'s `selection` offers, none when it is empty: those of the
    /// selection Yuna holds, else a client's first [`MAX_MIME_TYPES`] of
    /// [`MAX_MIME_TYPE_BYTES`] at most, as X11 is offered them.
    pub(crate) fn selection_mime_types(&self, seat: SeatId, selection: Selection) -> Vec<String> {
        match self.held_selection(seat, selection) {
            Some(HeldSelection::X11 { mime_types, .. }) => mime_types,
            Some(HeldSelection::Agent(_)) => AGENT_MIME_TYPES.map(str::to_owned).into(),
            None => self
                .selections
                .set_on_wayland(seat, selection)
                .and_then(|set| set.mime_types.clone())
                .unwrap_or_default(),
        }
    }

    /// Reads `seat`'s `selection` as `mime_type`, one of its types, as a Wayland client's paste
    /// would, for MCP: an agent's text whole; an X11 owner's through its manager, and a client's
    /// from the client, each into a new pipe whose read end it gives. Refused when no pipe can be
    /// made.
    pub(crate) fn read_selection(
        &mut self,
        seat: SeatId,
        selection: Selection,
        mime_type: String,
    ) -> Result<SelectionRead, crate::Error> {
        let no_pipe = || crate::Error::Refused("no pipe can be made to read the selection".into());
        match self.held_selection(seat, selection) {
            Some(HeldSelection::Agent(text)) => Ok(SelectionRead::Held(text)),
            Some(HeldSelection::X11 { offer, .. }) => {
                let (read, write) = reader_pipe().ok_or_else(no_pipe)?;
                self.x11_send_selection(offer, mime_type, write);
                Ok(SelectionRead::Pipe(read))
            }
            None => {
                let (read, write) = reader_pipe().ok_or_else(no_pipe)?;
                if self.request_client_selection(seat, selection, mime_type, write) {
                    Ok(SelectionRead::Pipe(read))
                } else {
                    Ok(SelectionRead::Empty)
                }
            }
        }
    }

    /// Reads `seat`'s `selection`, as Wayland's side set it, as `mime_type`, for X11: the read
    /// end, non-blocking, of a new pipe its source writes into, a client or Yuna for an agent;
    /// none when no such selection is there, it lacks the type, Yuna writes
    /// [`MAX_CLIPBOARD_WRITES`] of it already, or no pipe can be made.
    pub(crate) fn read_wayland_selection(
        &mut self,
        seat: SeatId,
        selection: Selection,
        mime_type: String,
    ) -> Option<OwnedFd> {
        match self.held_selection(seat, selection) {
            Some(HeldSelection::Agent(text)) => {
                if !AGENT_MIME_TYPES.contains(&mime_type.as_str()) {
                    return None;
                }
                let (read, write) = reader_pipe()?;
                self.write_agent_selection(seat, write, text)
                    .then_some(read)
            }
            Some(HeldSelection::X11 { .. }) => None,
            None => {
                let (read, write) = reader_pipe()?;
                self.request_client_selection(seat, selection, mime_type, write)
                    .then_some(read)
            }
        }
    }

    /// Asks the client whose selection is `seat`'s `selection` to write it as `mime_type` into
    /// `pipe`; false when no client's selection is there, or it lacks the type, and `pipe` is
    /// dropped.
    fn request_client_selection(
        &mut self,
        seat: SeatId,
        selection: Selection,
        mime_type: String,
        pipe: OwnedFd,
    ) -> bool {
        let Some(seat_data) = self.seat(seat) else {
            return false;
        };
        let gone = match selection {
            Selection::Clipboard => {
                match request_data_device_client_selection(&seat_data.seat, mime_type, pipe) {
                    Ok(()) => return true,
                    Err(error) => matches!(error, data_device::SelectionRequestError::NoSelection),
                }
            }
            Selection::Primary => {
                match request_primary_client_selection(&seat_data.seat, mime_type, pipe) {
                    Ok(()) => return true,
                    Err(error) => {
                        matches!(error, primary_selection::SelectionRequestError::NoSelection)
                    }
                }
            }
        };
        if gone {
            self.client_selection_gone(seat, selection);
        }
        false
    }

    /// Writes an agent's `text` into `pipe`, a reader's of `seat`, from the round's end;
    /// refused, the pipe closed, past [`MAX_CLIPBOARD_WRITES`] or when it cannot be made
    /// non-blocking.
    fn write_agent_selection(&mut self, seat: SeatId, pipe: OwnedFd, text: Arc<[u8]>) -> bool {
        if self.selections.writes_of(seat) >= MAX_CLIPBOARD_WRITES {
            tracing::debug!(
                seat = seat.0,
                "a read of an agent's selection is refused: too many writes run"
            );
            return false;
        }
        let non_blocking =
            fcntl_getfl(&pipe).and_then(|flags| fcntl_setfl(&pipe, flags | OFlags::NONBLOCK));
        if let Err(error) = non_blocking {
            tracing::debug!(%error, "a read of an agent's selection is refused: its pipe");
            return false;
        }
        self.selections.unstarted.push((seat, pipe, text));
        true
    }

    /// Yuna's writes of agents' selections, at a round's end: ends those that made no progress
    /// for [`CLIPBOARD_TIMEOUT`] or whose seat went, goes on with those whose pipe takes more,
    /// and starts those asked. Each writes until its pipe is full, and never waits.
    pub(crate) fn run_clipboard_writes(&mut self, handle: &LoopHandle<'static, State>) {
        let selections = &mut self.selections;
        for key in std::mem::take(&mut selections.ended) {
            if let Some(written) = selections.end_write(key, handle) {
                tracing::debug!(
                    written,
                    "a reader of an agent's selection stopped reading, or its seat went: its pipe \
                     closes"
                );
            }
        }
        for key in std::mem::take(&mut selections.writable) {
            let Some(write) = selections.writes.get_mut(&key) else {
                continue;
            };
            let before = write.written;
            let written = write_some(&write.pipe, &write.text, &mut write.written);
            if write.written > before {
                write.progress = Instant::now();
            }
            match written {
                Written::Partly => {}
                Written::All => {
                    selections.end_write(key, handle);
                }
                Written::Failed(error) => {
                    tracing::debug!(%error, "a reader of an agent's selection left");
                    selections.end_write(key, handle);
                }
            }
        }
        for (seat, pipe, text) in std::mem::take(&mut selections.unstarted) {
            selections.start_write(seat, pipe, text, handle);
        }
    }

    /// Empties `seat`'s selections, which is being removed, and forgets them: Smithay keeps the
    /// record of a removed seat, and would keep an agent's text on it until Yuna stops. Yuna's
    /// writes to its readers end at the round's end. Its Xwayland is gone: X11 is not told.
    pub(crate) fn forget_seat_selections(&mut self, seat: SeatId) {
        for selection in Selection::BOTH {
            self.replace_selection(seat, selection, None);
        }
        self.selections.forget_seat(seat);
    }

    /// Wayland's side set `seat`'s `selection`, a client or an agent, offering `mime_types`, or
    /// cleared it: recorded with a new serial, and X11 told at the round's end.
    fn wayland_selection_set(
        &mut self,
        seat: SeatId,
        selection: Selection,
        mime_types: Option<Vec<String>>,
    ) {
        let set = self
            .selections
            .set_on_wayland
            .entry((seat, selection))
            .or_default();
        set.serial += 1;
        set.mime_types = mime_types;
        self.x11_wayland_selection_changed(seat, selection);
    }

    /// What Wayland's side set as `seat`'s `selection` is replaced by an X11 owner's, or by
    /// none: Smithay has cancelled a client's source. X11 already knows: what replaced it came
    /// from X11.
    fn wayland_selection_replaced(&mut self, seat: SeatId, selection: Selection) {
        if let Some(set) = self.selections.set_on_wayland.get_mut(&(seat, selection)) {
            set.mime_types = None;
        }
    }

    /// A client's selection of `seat` turns out gone though Smithay did not say, a data-control
    /// source destroyed: X11 is told at the round's end.
    fn client_selection_gone(&mut self, seat: SeatId, selection: Selection) {
        let Some(set) = self.selections.set_on_wayland.get_mut(&(seat, selection)) else {
            return;
        };
        if set.mime_types.take().is_some() {
            set.serial += 1;
            self.x11_wayland_selection_changed(seat, selection);
        }
    }
}

/// Yuna holds a seat's selection for an X11 owner or an agent ([`HeldSelection`]): a read of it
/// goes to that owner's manager, or Yuna writes the agent's text. A client that sets a selection
/// is recorded, for X11.
impl SelectionHandler for State {
    type SelectionUserData = HeldSelection;

    /// A client sets or clears `seat`'s selection, called before Smithay sets it: it is not the
    /// seat's yet. Through Yuna's fork, also with none once a client's source that was the
    /// selection is destroyed, by its client or with it, Smithay having cleared it.
    fn new_selection(
        &mut self,
        target: SelectionTarget,
        source: Option<SelectionSource>,
        seat: Seat<State>,
    ) {
        let Some(seat) = seat_id(&seat) else {
            return;
        };
        // The source lives: Smithay calls this as its client sets it.
        let mime_types = source.map(|source| for_x11(source.mime_types()));
        self.wayland_selection_set(seat, selection_of(target), mime_types);
    }

    /// A client reads a selection Yuna holds, as one of its types, which Smithay checked: for an
    /// X11 owner, its pipe goes to the owner's manager, and is never read nor written here; for
    /// an agent, Yuna writes the text into it.
    fn send_selection(
        &mut self,
        _target: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        seat: Seat<State>,
        held: &HeldSelection,
    ) {
        match held {
            HeldSelection::X11 { offer, .. } => self.x11_send_selection(*offer, mime_type, fd),
            HeldSelection::Agent(text) => {
                if let Some(seat) = seat_id(&seat) {
                    self.write_agent_selection(seat, fd, text.clone());
                }
            }
        }
    }
}

impl DataDeviceHandler for State {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.selections.data_device
    }
}

impl PrimarySelectionHandler for State {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.selections.primary
    }
}

impl DataControlHandler for State {
    fn data_control_state(&mut self) -> &mut DataControlState {
        &mut self.selections.data_control
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::xwayland::Selection;
    use crate::{Compositor, Config};

    /// A seat removed while it holds an agent's text keeps none of it: Smithay keeps the record
    /// of every seat it made, and would keep the selections on it until Yuna stops.
    #[test]
    fn a_removed_seat_keeps_none_of_an_agents_text() {
        let mut compositor = Compositor::new(Config {
            outputs: Vec::new(),
            listen: false,
            announce_owner_seat: false,
            ..Config::default()
        })
        .expect("the compositor starts");
        let seat = compositor.add_seat().expect("a seat");
        let text: Arc<[u8]> = Arc::from(&b"Yuna"[..]);
        for selection in Selection::BOTH {
            compositor.hold_agent_selection(seat, selection, text.clone());
        }
        assert_eq!(Arc::strong_count(&text), 3, "held as both selections");
        compositor.remove_seat(seat).expect("the seat is removed");
        assert_eq!(Arc::strong_count(&text), 1, "the test's own alone");
    }
}
