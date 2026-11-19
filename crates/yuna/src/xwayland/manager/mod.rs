//! Yuna's X11 window manager, one per Xwayland, on a thread of its own
//! ([ADR-0066](../../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md)): the
//! thread waits on X11's replies, so that Yuna's loop never does.
//!
//! It connects through Xwayland's `-wm` socket once `-displayfd` has said Xwayland is ready, then
//! takes charge in an order that lets no X11 client read a setting before it exists
//! ([ADR-0067](../../../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)):
//!
//! 1. XFixes' disconnection mode, *terminate*, for its own connection: Xwayland's `-terminate`
//!    fires only once every client left has asked it, the manager included.
//! 2. `RESOURCE_MANAGER` on the root, `Xft.dpi` at 96 × S, S being the scale Xwayland's client
//!    was given ([ADR-0068](../../../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)).
//! 3. XSETTINGS: `Xft/DPI` with it, and a window scale of 1, since X11 programs draw in X11
//!    pixels.
//! 4. The drags' proxy, unmapped (`xdnd.rs`), and XFixes' selection events on the manager's
//!    window, for `CLIPBOARD`, `PRIMARY` and `XdndSelection`: no X11 client's selection or drag
//!    goes unseen (`bridge.rs`).
//! 5. Composite's manual redirection of the root's windows, and SubstructureRedirect on the root:
//!    Yuna draws and places X11 windows.
//! 6. `_NET_SUPPORTED`, `_NET_SUPPORTING_WM_CHECK`, and the root's empty `_NET_CLIENT_LIST`,
//!    `_NET_CLIENT_LIST_STACKING` and `_NET_ACTIVE_WINDOW`.
//! 7. `WM_S0`, last: Xwayland listens on its sockets only from then on, so the first client
//!    finds everything above in place.
//!
//! Then it serves. It follows each top-level window, a child of the root: it reads the
//! properties Yuna needs when the window appears and each time one changes, bounded
//! (`properties.rs`), and posts them with what the window does to Yuna's loop, through a calloop
//! channel, [`ManagerEvent`]: its creation, its asking to be mapped or configured, its mapping,
//! unmapping and destruction, an override-redirect window's moves and place in X11's stacking,
//! which the manager follows from the root's events, the serial that pairs it with its Wayland
//! surface (`WL_SURFACE_SERIAL`, a client message Xwayland sends the manager; a client's copy is
//! ignored), its sync counter's changes, and the client messages it sends: `_NET_WM_STATE`,
//! `_NET_ACTIVE_WINDOW`, a ping's answer. It reads the properties of
//! [`MAX_PROPERTIES_PER_CLIENT`] windows of one X11 client at most: a window made past them is
//! followed without them, and Yuna is never told of it. The manager maps it when it asks, as
//! Yuna maps a window past its client's limit in the layout, and it never shows: like that
//! window, it lies below every window Yuna stacks, where it takes no click from one Yuna shows,
//! and it is not in `_NET_CLIENT_LIST`. An override-redirect window past them maps itself, never
//! asking, and is not lowered: it may lie above windows Yuna shows. Yuna decides, and its [`Command`]s come back through
//! what Yuna owes the manager (`owed.rs`), bounded, which the thread takes when its wake-up
//! socket says so and between X11's events: map, configure with or without a sync request, let
//! commits go, set the states, focus, close, ping, restack. When Yuna's watchdog waits on them,
//! the thread pings Yuna's loop once it takes them. A manager that stops taking them is ended
//! with its Xwayland (`xwayland/mod.rs`). What follows from a decision by ICCCM and EWMH alone,
//! the manager does itself: `WM_STATE`, `_NET_FRAME_EXTENTS`, `_NET_CLIENT_LIST`, set once per
//! batch of events and commands, the synthetic `ConfigureNotify` after a configure (ICCCM
//! 4.1.5), the focus model (ICCCM 4.1.7), whether a window is asked to close or its client is
//! ended, and at a withdrawal the deletion of `_NET_WM_STATE`, and of `_XWAYLAND_ALLOW_COMMITS`
//! if a sync request held it. `_NET_WM_MOVERESIZE` and `WM_CHANGE_STATE` are taken and logged:
//! Yuna has no interactive move, and minimises nothing.
//!
//! It also carries the seat's clipboard and primary selection between X11 and Wayland, both ways
//! (`bridge.rs`): it posts what X11 clients copy and ask, and takes Yuna's word of what Wayland
//! clients copy, the pipes of their pastes, and the data of their selections. And it speaks XDND
//! for drags between them, both ways (`xdnd.rs`): it posts what X11 sources and targets say, and
//! answers them at once from what Yuna last told it.
//!
//! **No reparenting.** The manager never puts a window inside a frame of its own: Yuna draws no
//! title bars ([ADR-0024](../../../../../docs/adr/0024-no-title-bars.md)), so a frame would only
//! cost a window and a round trip each. Its known consequence: Java's AWT, which waits for a
//! reparenting manager unless told otherwise, needs `_JAVA_AWT_WM_NONREPARENTING=1`
//! ([debt](../../../../../docs/debt.md#xwayland)).
//!
//! Its connection reads Xwayland's socket through a stream of its own (`stream.rs`), which keeps a
//! made-up `KeymapNotify` from making x11rb lose its place among the replies.
//!
//! The thread ends when Xwayland closes the connection, when Yuna lets the commands go, or on an
//! error; its channel's end tells Yuna, which then ends that Xwayland, even when the thread
//! panicked. Nothing an X11 client sends can make it panic: every value it reads is checked. A
//! debug build panics on demand, for the test of that path (`YUNA_PANIC_ON_X11_CLOSE`,
//! `commands.rs`).
//!
//! Its parts, in `manager/`:
//!
//! - this file: the thread's start and end, taking charge in the order above with the settings
//!   it writes, and what the parts share: the atoms, and the manager's state, [`Manager`], with
//!   each window it follows, [`Tracked`];
//! - `messages.rs`: what the thread posts, [`ManagerEvent`], and the commands it takes,
//!   [`Command`];
//! - `owed.rs`: the commands Yuna sent that the thread has not taken yet, bounded;
//! - `stream.rs`: the connection's stream, which follows X11's framing as it reads;
//! - `serve.rs`: its loop, where it sleeps, and the root's lists of windows it writes once a
//!   batch;
//! - `x11_events.rs`: X11's events;
//! - `commands.rs`: Yuna's commands;
//! - `reading.rs`: the properties it reads of a window, and its client's process.

mod commands;
mod messages;
mod owed;
mod reading;
mod serve;
mod stream;
mod x11_events;

use std::collections::HashMap;
use std::io;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use rustix::net::SendFlags;
use smithay::reexports::calloop::channel::SyncSender;
use smithay::reexports::calloop::ping::Ping;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::composite::{ConnectionExt as _, Redirect};
use x11rb::protocol::res;
use x11rb::protocol::sync::{self, Alarm, Counter};
use x11rb::protocol::xfixes::{ClientDisconnectFlags, ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _,
    CreateWindowAux, EventMask, PropMode, Window, WindowClass,
};
use x11rb::rust_connection::{DefaultStream, RustConnection};
use x11rb::wrapper::ConnectionExt as _;
use yuna_layout::SeatId;

use super::bridge::Bridge;
use super::properties::{Properties, X11Rect};
use super::xdnd::{self, Xdnd};
#[cfg(debug_assertions)]
pub use commands::PANIC_ON_X11_CLOSE;
pub(crate) use messages::{Command, ManagerEvent, NetState, StateAction, StateChange, Stop};
pub(crate) use owed::OverBound;
use owed::Owed;
#[cfg(debug_assertions)]
pub use owed::X11_OWED_BOUND;
pub(crate) use stream::{ManagerConnection, ManagerStream};

/// How many of the manager's events wait for Yuna's loop before the thread waits in turn: X11's
/// own buffers then hold what comes, rather than Yuna's memory.
pub(crate) const EVENT_QUEUE: usize = 256;

/// The most windows of one X11 client whose properties the manager reads and Yuna is told of:
/// ADR-0131's figure for one client's windows in the layout (`MAX_WINDOWS_PER_CLIENT`), which no
/// program that shows its windows passes. Past it, a window costs the manager its record without
/// text, and Yuna nothing, where one with its properties costs each up to three texts of
/// `MAX_KEPT_TEXT_BYTES` (`properties.rs`).
pub const MAX_PROPERTIES_PER_CLIENT: usize = 1_000;

/// The DPI of an X11 program at scale 1, as X11 and its toolkits count it.
const BASE_DPI: f64 = 96.0;

/// ICCCM's `WM_STATE` values (4.1.3.1).
const WITHDRAWN_STATE: u32 = 0;
const NORMAL_STATE: u32 = 1;
const ICONIC_STATE: u32 = 3;

/// Yuna's side of a manager: what Yuna owes it, and the socket that wakes its thread for it. When
/// it drops, the thread ends.
#[derive(Debug)]
pub(crate) struct Commands {
    owed: Arc<Mutex<Owed>>,
    wake: UnixStream,
}

impl Commands {
    /// Adds `command` to what Yuna owes the manager's thread, which [`Commands::wake`] wakes for
    /// it. Never waits. Refused past its bound, [`MAX_OWED_BYTES`](owed::MAX_OWED_BYTES): the
    /// manager is that far behind, stalled. A thread that has ended never takes it, and Yuna
    /// learns of its end through the events' channel.
    pub(crate) fn send(&self, command: Command) -> Result<(), OverBound> {
        lock(&self.owed).push(command, Instant::now())
    }

    /// Wakes the manager's thread for the commands sent. Never waits: a full socket wakes the
    /// thread all the same; NOSIGNAL, so that a thread gone is no SIGPIPE.
    pub(crate) fn wake(&self) {
        let _ = rustix::net::send(&self.wake, &[0], SendFlags::DONTWAIT | SendFlags::NOSIGNAL);
    }

    /// When the oldest command the manager's thread has not taken was sent; none when it owes
    /// none. While it owes some, Yuna's watchdog waits on them, and the thread pings Yuna's loop
    /// when it takes them ([`start`]'s `taken`).
    pub(crate) fn watch(&self) -> Option<Instant> {
        lock(&self.owed).watch()
    }
}

impl Drop for Commands {
    /// Lets the manager go, and frees what it was owed now: its thread ends at its next wake-up,
    /// which the wake-up socket's closing gives, or its next command taken.
    fn drop(&mut self) {
        lock(&self.owed).release();
    }
}

/// What Yuna owes a manager, locked. Each holder leaves it whole between its own steps, so that
/// a panic elsewhere, the manager's thread's, leaves nothing half done: its poisoning is ignored.
fn lock(owed: &Mutex<Owed>) -> MutexGuard<'_, Owed> {
    owed.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Why the manager stopped: an X11 error, an I/O error, or what it found wrong.
pub(super) type Failure = Box<dyn std::error::Error + Send + Sync>;

x11rb::atom_manager! {
    /// The atoms the manager names.
    pub(crate) Atoms: AtomsCookie {
        WM_S0,
        MANAGER,
        UTF8_STRING,
        COMPOUND_TEXT,
        WL_SURFACE_SERIAL,
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        WM_TAKE_FOCUS,
        WM_STATE,
        WM_CHANGE_STATE,
        _NET_SUPPORTED,
        _NET_SUPPORTING_WM_CHECK,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_PING,
        _NET_WM_SYNC_REQUEST,
        _NET_WM_SYNC_REQUEST_COUNTER,
        _NET_STARTUP_ID,
        _NET_WM_STATE,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_HIDDEN,
        _NET_ACTIVE_WINDOW,
        _NET_WM_MOVERESIZE,
        _NET_CLIENT_LIST,
        _NET_CLIENT_LIST_STACKING,
        _NET_FRAME_EXTENTS,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_WINDOW_TYPE_DIALOG,
        _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WM_WINDOW_TYPE_TOOLBAR,
        _NET_WM_WINDOW_TYPE_SPLASH,
        _NET_WM_WINDOW_TYPE_MENU,
        _NET_WM_WINDOW_TYPE_DROPDOWN_MENU,
        _NET_WM_WINDOW_TYPE_POPUP_MENU,
        _NET_WM_WINDOW_TYPE_TOOLTIP,
        _NET_WM_WINDOW_TYPE_NOTIFICATION,
        _NET_WM_WINDOW_TYPE_COMBO,
        _NET_WM_WINDOW_TYPE_DND,
        _NET_WM_WINDOW_TYPE_DOCK,
        _NET_WM_WINDOW_TYPE_DESKTOP,
        _MOTIF_WM_HINTS,
        _GTK_FRAME_EXTENTS,
        _XWAYLAND_RANDR_EMU_MONITOR_RECTS,
        _XWAYLAND_ALLOW_COMMITS,
        _XSETTINGS_S0,
        _XSETTINGS_SETTINGS,
        CLIPBOARD,
        TARGETS,
        TIMESTAMP,
        INCR,
        TEXT,
        DELETE,
        NULL,
        _YUNA_SELECTION,
        XdndAware,
        XdndProxy,
        XdndSelection,
        XdndTypeList,
        XdndActionList,
        XdndActionDescription,
        XdndEnter,
        XdndPosition,
        XdndStatus,
        XdndLeave,
        XdndDrop,
        XdndFinished,
        XdndActionCopy,
        XdndActionMove,
        XdndActionLink,
        XdndActionAsk,
        XdndActionPrivate,
    }
}

/// Starts the manager of seat `seat`'s Xwayland on a thread of its own, on `wm`, Xwayland's
/// `-wm` socket, for a scale of `scale`. Its events go to `events`; `taken` pings Yuna's loop
/// when it takes commands a watchdog waited on ([`Commands::watch`]).
pub(crate) fn start(
    seat: SeatId,
    wm: UnixStream,
    scale: f64,
    events: SyncSender<ManagerEvent>,
    taken: Ping,
) -> io::Result<Commands> {
    let owed = Arc::new(Mutex::new(Owed::bounded_at(owed::owed_bound())));
    let (wake, woken) = UnixStream::pair()?;
    wake.set_nonblocking(true)?;
    woken.set_nonblocking(true)?;
    let shared = Arc::clone(&owed);
    std::thread::Builder::new()
        .name(format!("yuna-x11-{}", seat.0))
        .spawn(move || {
            let stop = match run(wm, scale, &events, &shared, taken, &woken) {
                Ok(()) => Stop::Released,
                Err(failure) if is_closed(&*failure) => Stop::Closed,
                Err(failure) => Stop::Failed(failure.to_string()),
            };
            let _ = events.send(ManagerEvent::Stopped(stop));
        })?;
    Ok(Commands { owed, wake })
}

/// Whether `failure` is Xwayland closing the connection.
fn is_closed(failure: &(dyn std::error::Error + 'static)) -> bool {
    let connection = failure.downcast_ref::<ConnectionError>().or_else(|| {
        match failure.downcast_ref::<ReplyError>() {
            Some(ReplyError::ConnectionError(error)) => Some(error),
            _ => None,
        }
    });
    matches!(connection, Some(ConnectionError::IoError(error))
        if matches!(error.kind(), io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe))
}

/// The thread's work: connects, takes charge, says so, then serves until the end.
fn run(
    wm: UnixStream,
    scale: f64,
    events: &SyncSender<ManagerEvent>,
    owed: &Mutex<Owed>,
    taken: Ping,
    woken: &UnixStream,
) -> Result<(), Failure> {
    let (stream, _) = DefaultStream::from_unix_stream(wm)?;
    // Xwayland has a single screen.
    let connection = RustConnection::connect_to_stream(ManagerStream::new(stream), 0)?;
    let (atoms, own, proxy) = take_charge(&connection, scale)?;
    if events.send(ManagerEvent::Ready).is_err() {
        return Ok(());
    }
    let mut manager = Manager::new(&connection, atoms, own, proxy, events, owed, taken)?;
    manager.serve(woken)
}

/// Takes charge of Xwayland's root, in the order the module's documentation gives, and checks
/// that `WM_S0` is the manager's. Returns the atoms, the manager's own window and the drags'
/// proxy.
fn take_charge(
    connection: &ManagerConnection,
    scale: f64,
) -> Result<(Atoms, Window, Window), Failure> {
    let atoms = Atoms::new(connection)?.reply()?;
    let root = connection
        .setup()
        .roots
        .first()
        .ok_or("Xwayland's setup lists no screen")?
        .root;

    // 6 is the version of `SetClientDisconnectMode`; XFixes refuses a request above the version
    // its client asked.
    let xfixes = connection.xfixes_query_version(6, 0)?.reply()?;
    if xfixes.major_version < 6 {
        return Err(format!(
            "Xwayland's XFixes is version {}: `-terminate` needs 6",
            xfixes.major_version
        )
        .into());
    }
    connection
        .xfixes_set_client_disconnect_mode(ClientDisconnectFlags::TERMINATE)?
        .check()?;

    connection.change_property8(
        PropMode::REPLACE,
        root,
        AtomEnum::RESOURCE_MANAGER,
        AtomEnum::STRING,
        resources(scale).as_bytes(),
    )?;

    // One window owns the manager's selections and is its check window; it is never mapped.
    let window = connection.generate_id()?;
    connection.create_window(
        0,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new(),
    )?;
    connection.change_property8(
        PropMode::REPLACE,
        window,
        atoms._XSETTINGS_SETTINGS,
        atoms._XSETTINGS_SETTINGS,
        &xsettings(0, &settings(scale)),
    )?;
    take_selection(connection, root, window, atoms._XSETTINGS_S0, &atoms)?;

    let proxy = xdnd::create_proxy(connection, root, &atoms)?;
    let owners = SelectionEventMask::SET_SELECTION_OWNER
        | SelectionEventMask::SELECTION_WINDOW_DESTROY
        | SelectionEventMask::SELECTION_CLIENT_CLOSE;
    for selection in [
        atoms.CLIPBOARD,
        AtomEnum::PRIMARY.into(),
        atoms.XdndSelection,
    ] {
        connection.xfixes_select_selection_input(window, selection, owners)?;
    }

    connection.composite_query_version(0, 4)?.reply()?;
    connection
        .composite_redirect_subwindows(root, Redirect::MANUAL)?
        .check()?;
    connection
        .change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY),
        )?
        .check()?;

    connection.change_property32(
        PropMode::REPLACE,
        root,
        atoms._NET_SUPPORTED,
        AtomEnum::ATOM,
        &supported(&atoms),
    )?;
    for on in [root, window] {
        connection.change_property32(
            PropMode::REPLACE,
            on,
            atoms._NET_SUPPORTING_WM_CHECK,
            AtomEnum::WINDOW,
            &[window],
        )?;
    }
    connection.change_property8(
        PropMode::REPLACE,
        window,
        atoms._NET_WM_NAME,
        atoms.UTF8_STRING,
        b"Yuna",
    )?;
    for list in [atoms._NET_CLIENT_LIST, atoms._NET_CLIENT_LIST_STACKING] {
        connection.change_property32(PropMode::REPLACE, root, list, AtomEnum::WINDOW, &[])?;
    }
    connection.change_property32(
        PropMode::REPLACE,
        root,
        atoms._NET_ACTIVE_WINDOW,
        AtomEnum::WINDOW,
        &[x11rb::NONE],
    )?;

    take_selection(connection, root, window, atoms.WM_S0, &atoms)?;
    // A reply after every request above: each was handled, and the selection is the manager's.
    let owner = connection.get_selection_owner(atoms.WM_S0)?.reply()?.owner;
    if owner != window {
        return Err(format!("WM_S0 belongs to window {owner}, not the manager's").into());
    }
    Ok((atoms, window, proxy))
}

/// What the manager supports, for `_NET_SUPPORTED`: the hints it reads or sets and the messages
/// it takes.
fn supported(atoms: &Atoms) -> Vec<Atom> {
    vec![
        atoms._NET_SUPPORTING_WM_CHECK,
        atoms._NET_WM_NAME,
        atoms._NET_WM_PID,
        atoms._NET_WM_PING,
        atoms._NET_WM_SYNC_REQUEST,
        atoms._NET_WM_SYNC_REQUEST_COUNTER,
        atoms._NET_WM_STATE,
        atoms._NET_WM_STATE_FULLSCREEN,
        atoms._NET_WM_STATE_MAXIMIZED_HORZ,
        atoms._NET_WM_STATE_MAXIMIZED_VERT,
        atoms._NET_WM_STATE_HIDDEN,
        atoms._NET_ACTIVE_WINDOW,
        atoms._NET_CLIENT_LIST,
        atoms._NET_CLIENT_LIST_STACKING,
        atoms._NET_FRAME_EXTENTS,
        atoms._NET_WM_WINDOW_TYPE,
        atoms._NET_WM_WINDOW_TYPE_NORMAL,
        atoms._NET_WM_WINDOW_TYPE_DIALOG,
        atoms._NET_WM_WINDOW_TYPE_DOCK,
        atoms._NET_WM_WINDOW_TYPE_DESKTOP,
        atoms._GTK_FRAME_EXTENTS,
    ]
}

/// Makes `window` the owner of `selection` and tells the root's listeners, as ICCCM asks of a
/// manager (its section 2.8).
fn take_selection(
    connection: &ManagerConnection,
    root: Window,
    window: Window,
    selection: Atom,
    atoms: &Atoms,
) -> Result<(), Failure> {
    connection.set_selection_owner(window, selection, x11rb::CURRENT_TIME)?;
    let announcement = ClientMessageEvent::new(
        32,
        root,
        atoms.MANAGER,
        [x11rb::CURRENT_TIME, selection, window, 0, 0],
    );
    connection.send_event(false, root, EventMask::STRUCTURE_NOTIFY, announcement)?;
    Ok(())
}

/// What the manager knows of a top-level window.
#[derive(Debug)]
struct Tracked {
    properties: Properties,
    /// Where it is, as its last `ConfigureNotify` said.
    geometry: X11Rect,
    override_redirect: bool,
    /// Whether it is mapped, as the last map or unmap notification said.
    mapped: bool,
    /// Whether the manager mapped it after it asked, as Yuna said or, past its client's limit, on
    /// its own: a managed window; in `_NET_CLIENT_LIST` unless past the limit.
    managed: bool,
    /// Whether a malformed property of it, or a title read as Latin-1 from `COMPOUND_TEXT`, was
    /// logged: once per window.
    logged: bool,
    /// The alarm that reports its sync counter's changes, made at its first sync request, and
    /// the counter it watches.
    alarm: Option<(Alarm, Counter)>,
    /// Whether its `_XWAYLAND_ALLOW_COMMITS` is 0: a sync request waits.
    commits_held: bool,
    /// Whether it was made past its client's [`MAX_PROPERTIES_PER_CLIENT`]: its properties not
    /// read, nothing of it posted to Yuna, and mapped by the manager when it asks, below every
    /// window Yuna stacks, out of `_NET_CLIENT_LIST`.
    past_limit: bool,
}

/// The manager serving: its connection, what it follows, and where it posts.
struct Manager<'a> {
    connection: &'a ManagerConnection,
    atoms: Atoms,
    root: Window,
    /// The manager's own window, which owns its selections; never followed.
    own: Window,
    /// The bits of a resource id that name the resource within its client: the rest name the
    /// client.
    resource_mask: u32,
    /// Whether Xwayland has XRes, which gives each client's process.
    xres: bool,
    /// Whether Xwayland has XSync, through which a window's sync counter is watched.
    sync: bool,
    /// The window each sync alarm watches.
    alarms: HashMap<Alarm, Window>,
    windows: HashMap<Window, Tracked>,
    /// How many windows each X11 client has whose properties are read, by the bits of a
    /// window's id that name its client: [`MAX_PROPERTIES_PER_CLIENT`] at most.
    with_properties: HashMap<u32, usize>,
    /// The managed windows mapped, in the order they were: `_NET_CLIENT_LIST`.
    clients: Vec<Window>,
    /// The same windows, bottom to top as Yuna stacked them: `_NET_CLIENT_LIST_STACKING`.
    stacking: Vec<Window>,
    /// The order Yuna last stacked windows in, bottom to top.
    restacked: Vec<Window>,
    /// X11's stacking of the root's children, bottom to top, as its events say: the manager's
    /// own window, the drags' proxy and every window it follows.
    order: Vec<Window>,
    /// Whether `clients` or `stacking` changed since the root's lists were last set.
    lists_changed: bool,
    events: &'a SyncSender<ManagerEvent>,
    /// What Yuna owes it.
    owed: &'a Mutex<Owed>,
    /// Pings Yuna's loop when it takes commands Yuna's watchdog waited on, which Yuna then
    /// disarms. Apart from `events`, whose kinds Yuna's state reads.
    taken: Ping,
    /// The seat's selections on X11's side.
    bridge: Bridge<'a>,
    /// The seat's drags on X11's side.
    xdnd: Xdnd<'a>,
}

/// Whether the manager goes on: false once Yuna's end of the events' channel has gone.
pub(super) type GoOn = bool;

impl<'a> Manager<'a> {
    fn new(
        connection: &'a ManagerConnection,
        atoms: Atoms,
        own: Window,
        proxy: Window,
        events: &'a SyncSender<ManagerEvent>,
        owed: &'a Mutex<Owed>,
        taken: Ping,
    ) -> Result<Manager<'a>, Failure> {
        let setup = connection.setup();
        let root = setup
            .roots
            .first()
            .ok_or("Xwayland's setup lists no screen")?
            .root;
        let xres = connection
            .extension_information(res::X11_EXTENSION_NAME)?
            .is_some();
        if !xres {
            tracing::debug!("Xwayland has no XRes: X11 windows' processes come from _NET_WM_PID");
        }
        let sync = connection
            .extension_information(sync::X11_EXTENSION_NAME)?
            .is_some();
        if !sync {
            tracing::info!("Xwayland has no XSync: X11 windows are resized without sync requests");
        }
        Ok(Manager {
            connection,
            atoms,
            root,
            own,
            resource_mask: setup.resource_id_mask,
            xres,
            sync,
            alarms: HashMap::new(),
            windows: HashMap::new(),
            with_properties: HashMap::new(),
            clients: Vec::new(),
            stacking: Vec::new(),
            restacked: Vec::new(),
            // Both made before any client's window, the proxy after the manager's own.
            order: vec![own, proxy],
            lists_changed: false,
            events,
            owed,
            taken,
            bridge: Bridge::new(connection, events, atoms, root, own),
            xdnd: Xdnd::new(connection, events, atoms, own, proxy),
        })
    }

    /// Posts `event` to Yuna's loop; false once it is gone.
    fn post(&self, event: ManagerEvent) -> GoOn {
        self.events.send(event).is_ok()
    }

    /// Whether Yuna is told of `window`: a window the manager follows, and not past its client's
    /// [`MAX_PROPERTIES_PER_CLIENT`].
    fn told(&self, window: Window) -> bool {
        self.windows
            .get(&window)
            .is_some_and(|tracked| !tracked.past_limit)
    }
}

/// X11's DPI at scale `scale`, rounded.
fn dpi(scale: f64) -> i32 {
    (BASE_DPI * scale).round() as i32
}

/// The root's `RESOURCE_MANAGER` at scale `scale`: `Xft.dpi`, which Xlib and Xft programs read
/// for their fonts and Qt for its scale.
fn resources(scale: f64) -> String {
    format!("Xft.dpi:\t{}\n", dpi(scale))
}

/// The XSETTINGS at scale `scale`: `Xft/DPI` in 1024ths of a DPI, as `RESOURCE_MANAGER` gives
/// it; a window scale of 1, since an X11 program draws in X11 pixels, which S already makes the
/// right size; and the DPI at scale 1, which GTK divides by its window scale when one is forced.
fn settings(scale: f64) -> [(&'static str, i32); 3] {
    [
        ("Xft/DPI", (BASE_DPI * scale * 1024.0).round() as i32),
        ("Gdk/WindowScalingFactor", 1),
        ("Gdk/UnscaledDPI", (BASE_DPI * 1024.0) as i32),
    ]
}

/// The XSETTINGS property holding integer `settings`, in the encoding of the XSETTINGS
/// specification, 0.5: little-endian, its byte order first; then the serial and the count; then
/// each setting's type (0, an integer), its name padded to four bytes, the serial it last changed
/// at, 0, and its value.
fn xsettings(serial: u32, settings: &[(&str, i32)]) -> Vec<u8> {
    const LITTLE_ENDIAN: u8 = 0;
    const INTEGER: u8 = 0;
    let mut blob = vec![LITTLE_ENDIAN, 0, 0, 0];
    blob.extend_from_slice(&serial.to_le_bytes());
    // A few settings, named by Yuna: far below either bound.
    let count = u32::try_from(settings.len()).unwrap_or(u32::MAX);
    blob.extend_from_slice(&count.to_le_bytes());
    for (name, value) in settings {
        blob.extend_from_slice(&[INTEGER, 0]);
        let length = u16::try_from(name.len()).unwrap_or(u16::MAX);
        blob.extend_from_slice(&length.to_le_bytes());
        blob.extend_from_slice(name.as_bytes());
        blob.resize(blob.len().next_multiple_of(4), 0);
        blob.extend_from_slice(&0_u32.to_le_bytes());
        blob.extend_from_slice(&value.to_le_bytes());
    }
    blob
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_followed_with_their_properties_are_the_layouts_figure() {
        assert_eq!(MAX_PROPERTIES_PER_CLIENT, crate::MAX_WINDOWS_PER_CLIENT);
    }

    #[test]
    fn the_xsettings_blob_is_little_endian_with_each_name_padded() {
        let blob = xsettings(7, &[("Xft/DPI", 147_456), ("Gdk/WindowScalingFactor", 1)]);
        let mut expected = vec![0, 0, 0, 0, 7, 0, 0, 0, 2, 0, 0, 0];
        // "Xft/DPI": 7 bytes, padded with one.
        expected.extend_from_slice(&[0, 0, 7, 0]);
        expected.extend_from_slice(b"Xft/DPI\0");
        expected.extend_from_slice(&[0, 0, 0, 0]);
        expected.extend_from_slice(&147_456_i32.to_le_bytes());
        // "Gdk/WindowScalingFactor": 23 bytes, padded with one.
        expected.extend_from_slice(&[0, 0, 23, 0]);
        expected.extend_from_slice(b"Gdk/WindowScalingFactor\0");
        expected.extend_from_slice(&[0, 0, 0, 0]);
        expected.extend_from_slice(&[1, 0, 0, 0]);
        assert_eq!(blob, expected);
    }

    #[test]
    fn a_name_of_four_bytes_takes_no_padding() {
        let blob = xsettings(0, &[("Net/", -1)]);
        assert_eq!(&blob[12..16], &[0, 0, 4, 0]);
        assert_eq!(&blob[16..20], b"Net/");
        assert_eq!(&blob[20..24], &[0, 0, 0, 0]);
        assert_eq!(&blob[24..], &[0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn the_dpi_follows_the_scale() {
        assert_eq!(resources(1.5), "Xft.dpi:\t144\n");
        assert_eq!(resources(1.0), "Xft.dpi:\t96\n");
        // 96 × 1.75 = 168; 96 × 1.1 = 105.6, rounded.
        assert_eq!(resources(1.75), "Xft.dpi:\t168\n");
        assert_eq!(resources(1.1), "Xft.dpi:\t106\n");
        assert_eq!(
            settings(1.5),
            [
                ("Xft/DPI", 147_456),
                ("Gdk/WindowScalingFactor", 1),
                ("Gdk/UnscaledDPI", 98_304)
            ]
        );
        // 96 × 1.1 × 1024 = 108,134.4.
        assert_eq!(settings(1.1)[0], ("Xft/DPI", 108_134));
    }
}
