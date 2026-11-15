//! Yuna's X11 window manager, one per Xwayland, on a thread of its own
//! ([ADR-0066](../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md)): the thread
//! waits on X11's replies, so that Yuna's loop never does.
//!
//! It connects through Xwayland's `-wm` socket once `-displayfd` has said Xwayland is ready, then
//! takes charge in an order that lets no X11 client read a setting before it exists
//! ([ADR-0067](../../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)):
//!
//! 1. XFixes' disconnection mode, *terminate*, for its own connection: Xwayland's `-terminate`
//!    fires only once every client left has asked it, the manager included.
//! 2. `RESOURCE_MANAGER` on the root, `Xft.dpi` at 96 × S, S being the scale Xwayland's client
//!    was given ([ADR-0068](../../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)).
//! 3. XSETTINGS: `Xft/DPI` with it, and a window scale of 1, since X11 programs draw in X11
//!    pixels.
//! 4. Composite's manual redirection of the root's windows, and SubstructureRedirect on the root:
//!    Yuna draws and places X11 windows.
//! 5. `_NET_SUPPORTED`, `_NET_SUPPORTING_WM_CHECK`, and the root's empty `_NET_CLIENT_LIST`,
//!    `_NET_CLIENT_LIST_STACKING` and `_NET_ACTIVE_WINDOW`.
//! 6. `WM_S0`, last: Xwayland listens on its sockets only from then on, so the first client
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
//! `_NET_ACTIVE_WINDOW`, a ping's answer. Yuna decides, and its [`Command`]s come back through a
//! channel the thread reads when its wake-up socket says so: map, configure with or without a
//! sync request, let commits go, set the states, focus, close, ping, restack. What follows from a
//! decision by ICCCM and EWMH alone, the manager does itself: `WM_STATE`, `_NET_FRAME_EXTENTS`,
//! `_NET_CLIENT_LIST`, set once per batch of events and commands, the synthetic `ConfigureNotify`
//! after a configure (ICCCM 4.1.5), the focus model (ICCCM 4.1.7), whether a window is asked to
//! close or its client is ended, and at a withdrawal the deletion of `_NET_WM_STATE`, and of
//! `_XWAYLAND_ALLOW_COMMITS` if a sync request held it. `_NET_WM_MOVERESIZE` and
//! `WM_CHANGE_STATE` are taken and logged: Yuna has no interactive move, and minimises nothing.
//!
//! **No reparenting.** The manager never puts a window inside a frame of its own: Yuna draws no
//! title bars ([ADR-0024](../../../../docs/adr/0024-no-title-bars.md)), so a frame would only
//! cost a window and a round trip each. Its known consequence: Java's AWT, which waits for a
//! reparenting manager unless told otherwise, needs `_JAVA_AWT_WM_NONREPARENTING=1`
//! ([debt](../../../../docs/debt.md#xwayland)).
//!
//! The thread ends when Xwayland closes the connection, when Yuna lets the commands go, or on an
//! error; its channel's end tells Yuna, which then ends that Xwayland, even when the thread
//! panicked. Nothing an X11 client sends can make it panic: every value it reads is checked.

use std::collections::HashMap;
use std::io;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;

use smithay::reexports::calloop::channel::SyncSender;
use smithay::reexports::rustix;
use smithay::reexports::rustix::event::{PollFd, PollFlags};
use smithay::reexports::rustix::io::Errno;
use smithay::reexports::rustix::net::{RecvFlags, SendFlags};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::Event;
use x11rb::protocol::composite::{ConnectionExt as _, Redirect};
use x11rb::protocol::res::{self, ClientIdMask, ClientIdSpec, ConnectionExt as _};
use x11rb::protocol::sync::{
    self, Alarm, ConnectionExt as _, Counter, CreateAlarmAux, Int64, TESTTYPE, VALUETYPE,
};
use x11rb::protocol::xfixes::{ClientDisconnectFlags, ConnectionExt as _};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConfigWindow,
    ConfigureNotifyEvent, ConfigureRequestEvent, ConfigureWindowAux, ConnectionExt as _,
    CreateWindowAux, EventMask, InputFocus, Place, PropMode, StackMode, Window, WindowClass,
};
use x11rb::rust_connection::{DefaultStream, RustConnection};
use x11rb::wrapper::ConnectionExt as _;
use yuna_layout::SeatId;

use super::properties::{
    self, MAX_EMULATED, MAX_LIST, MAX_STARTUP_ID_BYTES, MAX_TEXT_BYTES, Malformed, Properties,
    Protocols, Raw, TextTypes, WindowType, Wishes, X11Rect,
};

/// How many of the manager's events wait for Yuna's loop before the thread waits in turn: X11's
/// own buffers then hold what comes, rather than Yuna's memory.
pub(crate) const EVENT_QUEUE: usize = 256;

/// The DPI of an X11 program at scale 1, as X11 and its toolkits count it.
const BASE_DPI: f64 = 96.0;

/// ICCCM's `WM_STATE` values (4.1.3.1).
const WITHDRAWN_STATE: u32 = 0;
const NORMAL_STATE: u32 = 1;
const ICONIC_STATE: u32 = 3;

/// The bit of an event's type that says a client sent it with `SendEvent` (X11's protocol,
/// section 10).
const SENT_EVENT: u8 = 0x80;

/// `_NET_WM_STATE`'s actions (EWMH, `_NET_WM_STATE`).
const STATE_REMOVE: u32 = 0;
const STATE_ADD: u32 = 1;
const STATE_TOGGLE: u32 = 2;

/// What the manager's thread posts to Yuna's loop.
#[derive(Debug)]
pub(crate) enum ManagerEvent {
    /// It took charge: `WM_S0` is its, and X11 clients are let in.
    Ready,
    /// A top-level window appeared: created on the root, or reparented to it.
    Created {
        window: Window,
        /// The X11 client that made it: the bits of its id that name its client.
        client: u32,
        /// Whether it is override-redirect: placed by its client, never managed.
        override_redirect: bool,
        /// Its rectangle, as its client made it.
        geometry: X11Rect,
        /// Its process, as X11's own record of its client's connection gives it (XRes): never
        /// Xwayland's, which is Yuna's own through the socket pair.
        pid: Option<i32>,
        properties: Box<Properties>,
    },
    /// A property Yuna reads changed: all of them, as they are now.
    Properties {
        window: Window,
        properties: Box<Properties>,
    },
    /// It asks to be mapped: Yuna maps it, or not, with [`Command::Map`].
    MapRequest(Window),
    /// It is mapped; override-redirect or not, as it is at that moment.
    Mapped {
        window: Window,
        override_redirect: bool,
    },
    /// An override-redirect window moved or changed size: its rectangle now.
    Configured { window: Window, rect: X11Rect },
    /// A mapped override-redirect window's place in X11's stacking: just above `above`, the
    /// mapped override-redirect window below it, or below every other one with none. Posted when
    /// it is mapped and each time its client restacks it.
    Restacked {
        window: Window,
        above: Option<Window>,
    },
    /// It is unmapped: withdrawn by its client, or about to be destroyed.
    Unmapped(Window),
    /// It is gone, destroyed, or reparented away from the root.
    Destroyed(Window),
    /// It asks to be moved or resized to `asked`, its current place on the axes it does not
    /// name: answered with a [`Command::Configure`].
    ConfigureRequest { window: Window, asked: X11Rect },
    /// Xwayland gave its Wayland surface this serial (xwayland-shell-v1).
    SurfaceSerial { window: Window, serial: u64 },
    /// Its client asks to change its states (`_NET_WM_STATE`).
    StateRequest { window: Window, change: StateChange },
    /// Its client asks for it to be activated (`_NET_ACTIVE_WINDOW`).
    ActivateRequest(Window),
    /// Its sync counter reached `value` (`_NET_WM_SYNC_REQUEST`): it drew the size the sync
    /// request of that value came with, or a later one. Each change of the counter is posted.
    SyncCounter { window: Window, value: u64 },
    /// Its client answered a ping (`_NET_WM_PING`).
    PingAnswered(Window),
    /// The thread ends, for this reason.
    Stopped(Stop),
}

/// A change of states a client asks, as Yuna takes it: each state it names, added, removed or
/// toggled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StateChange {
    pub action: StateAction,
    pub fullscreen: bool,
    /// `_NET_WM_STATE_MAXIMIZED_HORZ`: full width in Yuna's layout.
    pub maximized: bool,
}

/// What a `_NET_WM_STATE` message does to the states it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateAction {
    Remove,
    Add,
    Toggle,
}

/// Why the manager's thread ended.
#[derive(Debug)]
pub(crate) enum Stop {
    /// Xwayland closed the connection: it is ending.
    Closed,
    /// Yuna let it go.
    Released,
    /// Something failed; its reason.
    Failed(String),
}

/// The states a window is told, `_NET_WM_STATE`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NetState {
    pub fullscreen: bool,
    /// Full width in the layout: maximised, on both axes, as X11 says it.
    pub maximized: bool,
    /// Hidden: on no output, X11's way of saying `suspended`.
    pub hidden: bool,
}

/// What Yuna's loop asks of the manager.
#[derive(Debug)]
pub(crate) enum Command {
    /// Maps a window that asked to be, its `WM_STATE` Normal.
    Map(Window),
    /// Moves and sizes a window, and tells it so with a synthetic `ConfigureNotify`. With
    /// `sync`, a `_NET_WM_SYNC_REQUEST` of that value goes first, and Xwayland holds the window's
    /// commits (`_XWAYLAND_ALLOW_COMMITS` 0) until [`Command::AllowCommits`].
    Configure {
        window: Window,
        rect: X11Rect,
        sync: Option<u64>,
    },
    /// Lets Xwayland commit a window's surface again, after a sync request.
    AllowCommits(Window),
    /// Pings a window (`_NET_WM_PING`); its answer comes as [`ManagerEvent::PingAnswered`].
    Ping(Window),
    /// Sets the states a window is told.
    State { window: Window, state: NetState },
    /// Gives the X input focus to a window, as its model asks (ICCCM 4.1.7), and names it in
    /// `_NET_ACTIVE_WINDOW`; or to none.
    Focus(Option<Window>),
    /// Asks a window to close, through `WM_DELETE_WINDOW` when it takes it, else ends its client.
    Close(Window),
    /// Stacks these windows in this order, bottom to top: Xwayland finds the window under the
    /// pointer through X11's stacking, so the windows Yuna shows lie above those it hides.
    Restack(Vec<Window>),
}

/// Yuna's side of a manager: its commands, and the socket that wakes its thread for them. When it
/// drops, the thread ends.
#[derive(Debug)]
pub(crate) struct Commands {
    sender: mpsc::Sender<Command>,
    wake: UnixStream,
}

impl Commands {
    /// Passes `command` to the manager's thread and wakes it. Never waits: a thread that has
    /// ended loses it, and Yuna learns of its end through the events' channel.
    pub(crate) fn send(&self, command: Command) {
        if self.sender.send(command).is_ok() {
            // A full socket wakes the thread all the same; NOSIGNAL, so that a thread gone is
            // no SIGPIPE.
            let _ = rustix::net::send(&self.wake, &[0], SendFlags::DONTWAIT | SendFlags::NOSIGNAL);
        }
    }
}

/// Why the manager stopped: an X11 error, an I/O error, or what it found wrong.
type Failure = Box<dyn std::error::Error + Send + Sync>;

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
    }
}

/// Starts the manager of seat `seat`'s Xwayland on a thread of its own, on `wm`, Xwayland's
/// `-wm` socket, for a scale of `scale`. Its events go to `events`.
pub(crate) fn start(
    seat: SeatId,
    wm: UnixStream,
    scale: f64,
    events: SyncSender<ManagerEvent>,
) -> io::Result<Commands> {
    let (sender, commands) = mpsc::channel();
    let (wake, woken) = UnixStream::pair()?;
    wake.set_nonblocking(true)?;
    woken.set_nonblocking(true)?;
    std::thread::Builder::new()
        .name(format!("yuna-x11-{}", seat.0))
        .spawn(move || {
            let stop = match run(wm, scale, &events, &commands, &woken) {
                Ok(()) => Stop::Released,
                Err(failure) if is_closed(&*failure) => Stop::Closed,
                Err(failure) => Stop::Failed(failure.to_string()),
            };
            let _ = events.send(ManagerEvent::Stopped(stop));
        })?;
    Ok(Commands { sender, wake })
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
    commands: &mpsc::Receiver<Command>,
    woken: &UnixStream,
) -> Result<(), Failure> {
    let (stream, _) = DefaultStream::from_unix_stream(wm)?;
    // Xwayland has a single screen.
    let connection = RustConnection::connect_to_stream(stream, 0)?;
    let (atoms, own) = take_charge(&connection, scale)?;
    if events.send(ManagerEvent::Ready).is_err() {
        return Ok(());
    }
    let mut manager = Manager::new(&connection, atoms, own, events)?;
    manager.serve(commands, woken)
}

/// Takes charge of Xwayland's root, in the order the module's documentation gives, and checks
/// that `WM_S0` is the manager's. Returns the atoms and the manager's own window.
fn take_charge(connection: &RustConnection, scale: f64) -> Result<(Atoms, Window), Failure> {
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
    Ok((atoms, window))
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
    connection: &RustConnection,
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

/// The properties of a window the manager reads, each with its bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    NetWmName,
    WmName,
    Class,
    NormalHints,
    Hints,
    Protocols,
    TransientFor,
    WindowType,
    Motif,
    FrameExtents,
    Pid,
    State,
    Emulated,
    SyncCounter,
    StartupId,
}

impl Read {
    const ALL: [Read; 15] = [
        Read::NetWmName,
        Read::WmName,
        Read::Class,
        Read::NormalHints,
        Read::Hints,
        Read::Protocols,
        Read::TransientFor,
        Read::WindowType,
        Read::Motif,
        Read::FrameExtents,
        Read::Pid,
        Read::State,
        Read::Emulated,
        Read::SyncCounter,
        Read::StartupId,
    ];

    fn atom(self, atoms: &Atoms) -> Atom {
        match self {
            Read::NetWmName => atoms._NET_WM_NAME,
            Read::WmName => AtomEnum::WM_NAME.into(),
            Read::Class => AtomEnum::WM_CLASS.into(),
            Read::NormalHints => AtomEnum::WM_NORMAL_HINTS.into(),
            Read::Hints => AtomEnum::WM_HINTS.into(),
            Read::Protocols => atoms.WM_PROTOCOLS,
            Read::TransientFor => AtomEnum::WM_TRANSIENT_FOR.into(),
            Read::WindowType => atoms._NET_WM_WINDOW_TYPE,
            Read::Motif => atoms._MOTIF_WM_HINTS,
            Read::FrameExtents => atoms._GTK_FRAME_EXTENTS,
            Read::Pid => atoms._NET_WM_PID,
            Read::State => atoms._NET_WM_STATE,
            Read::Emulated => atoms._XWAYLAND_RANDR_EMU_MONITOR_RECTS,
            Read::SyncCounter => atoms._NET_WM_SYNC_REQUEST_COUNTER,
            Read::StartupId => atoms._NET_STARTUP_ID,
        }
    }

    /// The most read of it, in 32-bit units, as `GetProperty` counts: each a property's whole
    /// size, or its bound (`properties.rs`).
    fn bound(self) -> u32 {
        match self {
            Read::NetWmName | Read::WmName | Read::Class => MAX_TEXT_BYTES / 4,
            Read::Protocols | Read::WindowType | Read::State => MAX_LIST,
            Read::NormalHints => 18,
            Read::Hints => 9,
            Read::Motif => 5,
            Read::FrameExtents => 4,
            Read::TransientFor | Read::Pid => 1,
            Read::Emulated => MAX_EMULATED * 4,
            Read::SyncCounter => 2,
            Read::StartupId => MAX_STARTUP_ID_BYTES / 4,
        }
    }
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
    /// Whether Yuna mapped it after it asked: a managed window, in `_NET_CLIENT_LIST`.
    managed: bool,
    /// Whether a malformed property of it, or a title read as Latin-1 from `COMPOUND_TEXT`, was
    /// logged: once per window.
    logged: bool,
    /// The alarm that reports its sync counter's changes, made at its first sync request, and
    /// the counter it watches.
    alarm: Option<(Alarm, Counter)>,
    /// Whether its `_XWAYLAND_ALLOW_COMMITS` is 0: a sync request waits.
    commits_held: bool,
}

/// The manager serving: its connection, what it follows, and where it posts.
struct Manager<'a> {
    connection: &'a RustConnection,
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
    /// The managed windows mapped, in the order they were: `_NET_CLIENT_LIST`.
    clients: Vec<Window>,
    /// The same windows, bottom to top as Yuna stacked them: `_NET_CLIENT_LIST_STACKING`.
    stacking: Vec<Window>,
    /// The order Yuna last stacked windows in, bottom to top.
    restacked: Vec<Window>,
    /// X11's stacking of the root's children, bottom to top, as its events say: the manager's
    /// own window and every window it follows.
    order: Vec<Window>,
    /// Whether `clients` or `stacking` changed since the root's lists were last set.
    lists_changed: bool,
    events: &'a SyncSender<ManagerEvent>,
}

/// Whether the manager goes on: false once Yuna's end of the events' channel has gone.
type GoOn = bool;

impl<'a> Manager<'a> {
    fn new(
        connection: &'a RustConnection,
        atoms: Atoms,
        own: Window,
        events: &'a SyncSender<ManagerEvent>,
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
            clients: Vec::new(),
            stacking: Vec::new(),
            restacked: Vec::new(),
            order: vec![own],
            lists_changed: false,
            events,
        })
    }

    /// Reads X11's events and Yuna's commands until Xwayland closes the connection or Yuna lets
    /// the commands go. Sleeps in `poll` between them: nothing is polled on a timer.
    fn serve(
        &mut self,
        commands: &mpsc::Receiver<Command>,
        woken: &UnixStream,
    ) -> Result<(), Failure> {
        loop {
            loop {
                match commands.try_recv() {
                    Ok(command) => self.command(command)?,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                }
            }
            // Flushed before the events are read: a flush that would block reads X11's packets
            // into x11rb's own queue, where `poll` would not see them, so that an event could
            // wait for unrelated traffic. Handling events sends requests too: flushed again, and
            // the queue read again, until a flush leaves nothing to handle. At the sleep, the
            // queue and the socket are both empty.
            loop {
                self.write_lists();
                self.connection.flush()?;
                let mut handled = false;
                while let Some(event) = self.connection.poll_for_event()? {
                    handled = true;
                    if !self.event(event)? {
                        return Ok(());
                    }
                }
                if !handled {
                    break;
                }
            }
            let mut sleep = [
                PollFd::new(self.connection.stream(), PollFlags::IN),
                PollFd::new(woken, PollFlags::IN),
            ];
            match rustix::event::poll(&mut sleep, None) {
                Ok(_) | Err(Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
            if !drain(woken)? {
                return Ok(());
            }
        }
    }

    /// Posts `event` to Yuna's loop; false once it is gone.
    fn post(&self, event: ManagerEvent) -> GoOn {
        self.events.send(event).is_ok()
    }

    /// Handles one X11 event.
    fn event(&mut self, event: Event) -> Result<GoOn, Failure> {
        let go_on = match event {
            Event::CreateNotify(created) if created.parent == self.root => self.track(
                created.window,
                X11Rect {
                    x: i32::from(created.x),
                    y: i32::from(created.y),
                    width: u32::from(created.width),
                    height: u32::from(created.height),
                },
                created.override_redirect,
            )?,
            Event::ReparentNotify(moved) if moved.window != self.own => {
                if moved.parent == self.root {
                    let geometry = X11Rect {
                        x: i32::from(moved.x),
                        y: i32::from(moved.y),
                        ..X11Rect::default()
                    };
                    self.track(moved.window, geometry, moved.override_redirect)?
                } else {
                    self.forget(moved.window)
                }
            }
            Event::DestroyNotify(destroyed) => self.forget(destroyed.window),
            Event::MapRequest(request) => {
                if let Some(tracked) = self.windows.get_mut(&request.window) {
                    tracked.override_redirect = false;
                }
                self.post(ManagerEvent::MapRequest(request.window))
            }
            Event::MapNotify(mapped) => match self.windows.get_mut(&mapped.window) {
                Some(tracked) if !tracked.mapped => {
                    tracked.mapped = true;
                    tracked.override_redirect = mapped.override_redirect;
                    let window = mapped.window;
                    self.post(ManagerEvent::Mapped {
                        window,
                        override_redirect: mapped.override_redirect,
                    }) && self.post_place_in_stacking(window)
                }
                _ => true,
            },
            // A client withdrawing its window sends a synthetic one too (ICCCM 4.1.4): either
            // comes first, the other finds it unmapped.
            Event::UnmapNotify(unmapped) => self.unmapped(unmapped.window)?,
            Event::ConfigureNotify(configured) => self.configured(&configured),
            Event::CirculateNotify(circulated) => {
                let window = circulated.window;
                let above = match circulated.place {
                    Place::ON_TOP => self.order.iter().rev().find(|&&w| w != window).copied(),
                    _ => None,
                };
                self.restacked(window, above)
            }
            Event::ConfigureRequest(request) => self.configure_request(&request),
            Event::PropertyNotify(changed) => {
                self.property_changed(changed.window, changed.atom)?
            }
            Event::ClientMessage(message) => self.client_message(&message),
            Event::SyncAlarmNotify(alarm) => match self.alarms.get(&alarm.alarm) {
                Some(&window) => {
                    let value = alarm.counter_value;
                    // A counter below 0 has reached no value Yuna sends.
                    let value = (i64::from(value.hi) << 32 | i64::from(value.lo)).max(0) as u64;
                    self.post(ManagerEvent::SyncCounter { window, value })
                }
                None => true,
            },
            Event::Error(error) => {
                // A request on a window its client destroyed meanwhile, most often.
                tracing::debug!(?error, "an X11 request of the manager failed");
                true
            }
            event => {
                tracing::trace!(?event, "an X11 event the manager leaves");
                true
            }
        };
        Ok(go_on)
    }

    /// Follows a new top-level window: its properties' changes, its properties now and its
    /// process, posted together.
    fn track(
        &mut self,
        window: Window,
        geometry: X11Rect,
        override_redirect: bool,
    ) -> Result<GoOn, Failure> {
        if window == self.own || self.windows.contains_key(&window) {
            return Ok(true);
        }
        // Selected before the properties are read, so that no change between falls through.
        self.connection.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        let mut tracked = Tracked {
            properties: Properties::default(),
            geometry,
            override_redirect,
            mapped: false,
            managed: false,
            logged: false,
            alarm: None,
            commits_held: false,
        };
        let pid = self.pid_of(window)?;
        for (read, raw) in self.read_all(window)? {
            self.apply(window, &mut tracked, read, raw.as_ref());
        }
        let properties = Box::new(tracked.properties.clone());
        self.windows.insert(window, tracked);
        // A window made or reparented to the root goes on top of its siblings.
        self.order.push(window);
        Ok(self.post(ManagerEvent::Created {
            window,
            client: window & !self.resource_mask,
            override_redirect,
            geometry,
            pid,
            properties,
        }))
    }

    /// Forgets a window gone from the root, and tells Yuna.
    fn forget(&mut self, window: Window) -> GoOn {
        let Some(tracked) = self.windows.remove(&window) else {
            return true;
        };
        if let Some((alarm, _)) = tracked.alarm {
            self.destroy_alarm(alarm);
        }
        self.order.retain(|&stacked| stacked != window);
        self.unlist(window);
        self.post(ManagerEvent::Destroyed(window))
    }

    /// A window moved, changed size or was restacked. An override-redirect window's new
    /// rectangle goes to Yuna, which draws it where its client puts it; so does its new place in
    /// the stacking once it is mapped. A managed window's place is Yuna's own already.
    fn configured(&mut self, configured: &ConfigureNotifyEvent) -> GoOn {
        let window = configured.window;
        let Some(tracked) = self.windows.get_mut(&window) else {
            return true;
        };
        let rect = X11Rect {
            x: i32::from(configured.x),
            y: i32::from(configured.y),
            width: u32::from(configured.width),
            height: u32::from(configured.height),
        };
        let moved = std::mem::replace(&mut tracked.geometry, rect) != rect;
        tracked.override_redirect = configured.override_redirect;
        let override_redirect = tracked.override_redirect;
        let above = Some(configured.above_sibling).filter(|&sibling| sibling != x11rb::NONE);
        if override_redirect && moved && !self.post(ManagerEvent::Configured { window, rect }) {
            return false;
        }
        self.restacked(window, above)
    }

    /// `window` now lies just above `above` in X11's stacking, or at the bottom with none; a
    /// sibling the manager does not follow puts it at the bottom. Yuna is told when it is a mapped
    /// override-redirect window whose place changed.
    fn restacked(&mut self, window: Window, above: Option<Window>) -> GoOn {
        let Some(from) = self.order.iter().position(|&stacked| stacked == window) else {
            return true;
        };
        self.order.remove(from);
        let to = above
            .and_then(|above| self.order.iter().position(|&stacked| stacked == above))
            .map_or(0, |below| below + 1);
        self.order.insert(to, window);
        if from == to {
            return true;
        }
        self.post_place_in_stacking(window)
    }

    /// Tells Yuna where `window`, if it is a mapped override-redirect window, lies among the
    /// others: just above the first one mapped below it.
    fn post_place_in_stacking(&self, window: Window) -> GoOn {
        let is_menu = |window: &Window| {
            self.windows
                .get(window)
                .is_some_and(|tracked| tracked.mapped && tracked.override_redirect)
        };
        if !is_menu(&window) {
            return true;
        }
        let above = self
            .order
            .iter()
            .take_while(|&&stacked| stacked != window)
            .filter(|stacked| is_menu(stacked))
            .last()
            .copied();
        self.post(ManagerEvent::Restacked { window, above })
    }

    /// A window was unmapped: a managed one is withdrawn (ICCCM 4.1.4) and leaves the lists. Its
    /// `_NET_WM_STATE` goes, as EWMH asks of a manager at withdrawal: mapped again, the window
    /// starts from what its client sets then, not from the states it had.
    fn unmapped(&mut self, window: Window) -> Result<GoOn, Failure> {
        let Some(tracked) = self.windows.get_mut(&window) else {
            return Ok(true);
        };
        if !tracked.mapped {
            return Ok(true);
        }
        tracked.mapped = false;
        if std::mem::take(&mut tracked.managed) {
            self.connection.change_property32(
                PropMode::REPLACE,
                window,
                self.atoms.WM_STATE,
                self.atoms.WM_STATE,
                &[WITHDRAWN_STATE, x11rb::NONE],
            )?;
            self.connection
                .delete_property(window, self.atoms._NET_WM_STATE)?;
            // Held for a sync request that will not be answered now: Xwayland reads it again
            // when the window is mapped, and would never commit it.
            if std::mem::take(&mut tracked.commits_held) {
                self.connection
                    .delete_property(window, self.atoms._XWAYLAND_ALLOW_COMMITS)?;
            }
            self.unlist(window);
        }
        Ok(self.post(ManagerEvent::Unmapped(window)))
    }

    /// Takes `window` out of `_NET_CLIENT_LIST` and `_NET_CLIENT_LIST_STACKING`, if it is there.
    fn unlist(&mut self, window: Window) {
        let listed = self.clients.contains(&window);
        self.clients.retain(|&listed| listed != window);
        self.stacking.retain(|&listed| listed != window);
        if listed {
            self.publish_lists();
        }
    }

    /// Marks the root's `_NET_CLIENT_LIST` and `_NET_CLIENT_LIST_STACKING` changed: they are set
    /// once the events and commands at hand are handled, so that a thousand windows mapped or
    /// gone together set them once, not a thousand times.
    fn publish_lists(&mut self) {
        self.lists_changed = true;
    }

    /// Sets the root's lists of windows, if they changed. Errors go to the event queue: nothing
    /// waits for them.
    fn write_lists(&mut self) {
        if !std::mem::take(&mut self.lists_changed) {
            return;
        }
        for (atom, list) in [
            (self.atoms._NET_CLIENT_LIST, &self.clients),
            (self.atoms._NET_CLIENT_LIST_STACKING, &self.stacking),
        ] {
            if let Err(error) = self.connection.change_property32(
                PropMode::REPLACE,
                self.root,
                atom,
                AtomEnum::WINDOW,
                list,
            ) {
                tracing::debug!(%error, "the root's lists of windows cannot be set");
            }
        }
    }

    /// Passes a client's `ConfigureRequest` to Yuna: what it asks, its current place on the axes
    /// it does not name. Its stacking is left: Yuna stacks windows.
    fn configure_request(&self, request: &ConfigureRequestEvent) -> GoOn {
        let current = self
            .windows
            .get(&request.window)
            .map_or_else(X11Rect::default, |tracked| tracked.geometry);
        let mask = request.value_mask;
        let asked = X11Rect {
            x: if mask.contains(ConfigWindow::X) {
                i32::from(request.x)
            } else {
                current.x
            },
            y: if mask.contains(ConfigWindow::Y) {
                i32::from(request.y)
            } else {
                current.y
            },
            width: if mask.contains(ConfigWindow::WIDTH) {
                u32::from(request.width)
            } else {
                current.width
            },
            height: if mask.contains(ConfigWindow::HEIGHT) {
                u32::from(request.height)
            } else {
                current.height
            },
        };
        self.post(ManagerEvent::ConfigureRequest {
            window: request.window,
            asked,
        })
    }

    /// A property changed: when Yuna reads it, read again, and every property posted.
    fn property_changed(&mut self, window: Window, atom: Atom) -> Result<GoOn, Failure> {
        let Some(read) = Read::ALL
            .into_iter()
            .find(|read| read.atom(&self.atoms) == atom)
        else {
            return Ok(true);
        };
        let Some(mut tracked) = self.windows.remove(&window) else {
            return Ok(true);
        };
        let raw = self.read_one(window, read)?;
        self.apply(window, &mut tracked, read, raw.as_ref());
        // Another counter: the next sync request watches it from 0, as Yuna counts again.
        if let Some((alarm, counter)) = tracked.alarm
            && tracked.properties.sync_counter != Some(counter)
        {
            tracked.alarm = None;
            self.destroy_alarm(alarm);
        }
        let properties = Box::new(tracked.properties.clone());
        self.windows.insert(window, tracked);
        Ok(self.post(ManagerEvent::Properties { window, properties }))
    }

    /// The client messages the manager takes; others are left.
    fn client_message(&self, message: &ClientMessageEvent) -> GoOn {
        let atoms = &self.atoms;
        let data = message.data.as_data32();
        let window = message.window;
        if message.format != 32 {
            return true;
        }
        if message.type_ == atoms.WL_SURFACE_SERIAL {
            // Xwayland delivers its own from within the server; a copy with SendEvent's bit came
            // from a client, which could pair its window with another's surface.
            if message.response_type & SENT_EVENT != 0 {
                tracing::debug!(window, "a WL_SURFACE_SERIAL sent by a client is ignored");
                return true;
            }
            let serial = u64::from(data[0]) | (u64::from(data[1]) << 32);
            return self.post(ManagerEvent::SurfaceSerial { window, serial });
        }
        // A ping's answer comes back on the root, naming its window (EWMH, `_NET_WM_PING`).
        if window == self.root
            && message.type_ == atoms.WM_PROTOCOLS
            && data[0] == atoms._NET_WM_PING
        {
            let pinged = data[2];
            if self.windows.contains_key(&pinged) {
                return self.post(ManagerEvent::PingAnswered(pinged));
            }
            return true;
        }
        if !self.windows.contains_key(&window) {
            return true;
        }
        if message.type_ == atoms._NET_WM_STATE {
            let action = match data[0] {
                STATE_REMOVE => StateAction::Remove,
                STATE_ADD => StateAction::Add,
                STATE_TOGGLE => StateAction::Toggle,
                _ => return true,
            };
            let names = |atom: Atom| data[1] == atom || data[2] == atom;
            let change = StateChange {
                action,
                fullscreen: names(atoms._NET_WM_STATE_FULLSCREEN),
                maximized: names(atoms._NET_WM_STATE_MAXIMIZED_HORZ),
            };
            if names(atoms._NET_WM_STATE_HIDDEN) {
                tracing::debug!(
                    window,
                    "an X11 window asks to be hidden: Yuna minimises nothing"
                );
            }
            if change.fullscreen || change.maximized {
                return self.post(ManagerEvent::StateRequest { window, change });
            }
        } else if message.type_ == atoms._NET_ACTIVE_WINDOW {
            return self.post(ManagerEvent::ActivateRequest(window));
        } else if message.type_ == atoms._NET_WM_MOVERESIZE {
            tracing::debug!(
                window,
                "an X11 window asks to be moved: Yuna has no interactive move"
            );
        } else if message.type_ == atoms.WM_CHANGE_STATE && data[0] == ICONIC_STATE {
            tracing::debug!(
                window,
                "an X11 window asks to be iconified: Yuna minimises nothing"
            );
        }
        true
    }

    /// Carries out one of Yuna's commands. A request on a window gone meanwhile fails in the
    /// event queue, logged there.
    fn command(&mut self, command: Command) -> Result<(), Failure> {
        let connection = self.connection;
        let atoms = &self.atoms;
        match command {
            Command::Map(window) => {
                // Xwayland reads it when it makes the window's surface: a hold left from before
                // would keep the window from ever showing.
                if let Some(tracked) = self.windows.get_mut(&window)
                    && std::mem::take(&mut tracked.commits_held)
                {
                    connection.delete_property(window, atoms._XWAYLAND_ALLOW_COMMITS)?;
                }
                connection.change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms.WM_STATE,
                    atoms.WM_STATE,
                    &[NORMAL_STATE, x11rb::NONE],
                )?;
                // Yuna draws no frame: none is added around the window.
                connection.change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms._NET_FRAME_EXTENTS,
                    AtomEnum::CARDINAL,
                    &[0, 0, 0, 0],
                )?;
                connection.map_window(window)?;
                if let Some(tracked) = self.windows.get_mut(&window) {
                    tracked.managed = true;
                    if !self.clients.contains(&window) {
                        self.clients.push(window);
                        self.stacking.push(window);
                        self.publish_lists();
                    }
                }
            }
            Command::Configure { window, rect, sync } => {
                if let Some(value) = sync {
                    self.request_sync(window, value)?;
                }
                configure(connection, window, rect)?;
            }
            Command::AllowCommits(window) => {
                if let Some(tracked) = self.windows.get_mut(&window)
                    && std::mem::take(&mut tracked.commits_held)
                {
                    connection.change_property32(
                        PropMode::REPLACE,
                        window,
                        atoms._XWAYLAND_ALLOW_COMMITS,
                        AtomEnum::CARDINAL,
                        &[1],
                    )?;
                }
            }
            Command::Ping(window) => {
                let message = ClientMessageEvent::new(
                    32,
                    window,
                    atoms.WM_PROTOCOLS,
                    [atoms._NET_WM_PING, x11rb::CURRENT_TIME, window, 0, 0],
                );
                connection.send_event(false, window, EventMask::NO_EVENT, message)?;
            }
            Command::State { window, state } => {
                // A command sent before Yuna learnt of a withdrawal would set again the states
                // the withdrawal deleted.
                if !self
                    .windows
                    .get(&window)
                    .is_some_and(|tracked| tracked.managed)
                {
                    return Ok(());
                }
                let mut names = Vec::new();
                if state.fullscreen {
                    names.push(atoms._NET_WM_STATE_FULLSCREEN);
                }
                if state.maximized {
                    names.push(atoms._NET_WM_STATE_MAXIMIZED_HORZ);
                    names.push(atoms._NET_WM_STATE_MAXIMIZED_VERT);
                }
                if state.hidden {
                    names.push(atoms._NET_WM_STATE_HIDDEN);
                }
                connection.change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms._NET_WM_STATE,
                    AtomEnum::ATOM,
                    &names,
                )?;
            }
            Command::Focus(window) => self.focus(window)?,
            Command::Close(window) => {
                let deletes = self
                    .windows
                    .get(&window)
                    .is_some_and(|tracked| tracked.properties.protocols.delete_window);
                if deletes {
                    let message = ClientMessageEvent::new(
                        32,
                        window,
                        atoms.WM_PROTOCOLS,
                        [atoms.WM_DELETE_WINDOW, x11rb::CURRENT_TIME, 0, 0, 0],
                    );
                    connection.send_event(false, window, EventMask::NO_EVENT, message)?;
                } else {
                    connection.kill_client(window)?;
                }
            }
            Command::Restack(order) => {
                // From the first window whose place in the order changed: those below it stand
                // as they were, so that one window opening among a thousand restacks a few.
                let kept = self
                    .restacked
                    .iter()
                    .zip(&order)
                    .take_while(|(was, is)| was == is)
                    .count();
                if kept == 0
                    && let Some(&bottom) = order.first()
                {
                    connection.configure_window(
                        bottom,
                        &ConfigureWindowAux::new().stack_mode(StackMode::BELOW),
                    )?;
                }
                for pair in order.windows(2).skip(kept.saturating_sub(1)) {
                    connection.configure_window(
                        pair[1],
                        &ConfigureWindowAux::new()
                            .sibling(pair[0])
                            .stack_mode(StackMode::ABOVE),
                    )?;
                }
                self.restacked.clone_from(&order);
                let listed: Vec<Window> = order
                    .into_iter()
                    .filter(|window| self.clients.contains(window))
                    .collect();
                if listed != self.stacking {
                    self.stacking = listed;
                    self.publish_lists();
                }
            }
        }
        Ok(())
    }

    /// Sends `window` a sync request of `value` (`_NET_WM_SYNC_REQUEST`), ahead of a configure,
    /// and has Xwayland hold its commits until Yuna lets them go: then the frame drawn at the
    /// new size shows whole. Its sync counter is watched by an alarm made at its first request,
    /// which reports each change; the counter is set to 0 then, as KWin and Smithay set it, so
    /// that Yuna's values, counted from 1, are ahead of it. Without XSync, without a counter, or
    /// for a window the manager no longer sees mapped, which Yuna asked before it learnt of the
    /// unmapping, the answer is posted at once: nothing waits, and nothing holds the commits of a
    /// window mapped again.
    fn request_sync(&mut self, window: Window, value: u64) -> Result<(), Failure> {
        let connection = self.connection;
        let atoms = &self.atoms;
        let has_sync = self.sync;
        let Some(tracked) = self.windows.get_mut(&window) else {
            return Ok(());
        };
        let counter = tracked
            .properties
            .sync_counter
            .filter(|_| has_sync && tracked.mapped && tracked.managed);
        let Some(counter) = counter else {
            self.post(ManagerEvent::SyncCounter { window, value });
            return Ok(());
        };
        if tracked.alarm.is_none() {
            connection.sync_set_counter(counter, Int64 { hi: 0, lo: 0 })?;
            // Relative 1, delta 1, positive comparison: the alarm fires at each increase of the
            // counter, as Smithay's (`xwayland/xwm/surface.rs`, MIT) does.
            let alarm = connection.generate_id()?;
            let one = Int64 { hi: 0, lo: 1 };
            connection.sync_create_alarm(
                alarm,
                &CreateAlarmAux::new()
                    .counter(counter)
                    .value_type(VALUETYPE::RELATIVE)
                    .value(one)
                    .test_type(TESTTYPE::POSITIVE_COMPARISON)
                    .delta(one)
                    .events(1),
            )?;
            tracked.alarm = Some((alarm, counter));
            self.alarms.insert(alarm, window);
        }
        tracked.commits_held = true;
        connection.change_property32(
            PropMode::REPLACE,
            window,
            atoms._XWAYLAND_ALLOW_COMMITS,
            AtomEnum::CARDINAL,
            &[0],
        )?;
        let message = ClientMessageEvent::new(
            32,
            window,
            atoms.WM_PROTOCOLS,
            [
                atoms._NET_WM_SYNC_REQUEST,
                x11rb::CURRENT_TIME,
                value as u32,
                (value >> 32) as u32,
                0,
            ],
        );
        connection.send_event(false, window, EventMask::NO_EVENT, message)?;
        Ok(())
    }

    /// Destroys a sync alarm; an error, for a client gone, goes to the event queue.
    fn destroy_alarm(&mut self, alarm: Alarm) {
        self.alarms.remove(&alarm);
        if let Err(error) = self.connection.sync_destroy_alarm(alarm) {
            tracing::debug!(%error, "a sync alarm cannot be destroyed");
        }
    }

    /// Gives the X input focus to `window` as its model asks (ICCCM 4.1.7): Passive and Locally
    /// Active windows, which take input, get `SetInputFocus`; Locally and Globally Active ones,
    /// which list `WM_TAKE_FOCUS`, are sent it; a No Input window gets neither. A window without
    /// `WM_HINTS` takes input, as most managers have it. With none, the focus goes to none.
    /// `_NET_ACTIVE_WINDOW` names it.
    fn focus(&self, window: Option<Window>) -> Result<(), Failure> {
        let connection = self.connection;
        let atoms = &self.atoms;
        match window {
            Some(window) => {
                let (input, take_focus) = self.windows.get(&window).map_or((true, false), |w| {
                    (
                        w.properties.accepts_input.unwrap_or(true),
                        w.properties.protocols.take_focus,
                    )
                });
                if input {
                    connection.set_input_focus(InputFocus::NONE, window, x11rb::CURRENT_TIME)?;
                }
                if take_focus {
                    let message = ClientMessageEvent::new(
                        32,
                        window,
                        atoms.WM_PROTOCOLS,
                        [atoms.WM_TAKE_FOCUS, x11rb::CURRENT_TIME, 0, 0, 0],
                    );
                    connection.send_event(false, window, EventMask::NO_EVENT, message)?;
                }
            }
            None => {
                connection.set_input_focus(InputFocus::NONE, x11rb::NONE, x11rb::CURRENT_TIME)?;
            }
        }
        connection.change_property32(
            PropMode::REPLACE,
            self.root,
            atoms._NET_ACTIVE_WINDOW,
            AtomEnum::WINDOW,
            &[window.unwrap_or(x11rb::NONE)],
        )?;
        Ok(())
    }

    /// The process of `window`'s client, as XRes gives it from its connection; none without
    /// XRes, or for a window gone.
    fn pid_of(&self, window: Window) -> Result<Option<i32>, Failure> {
        if !self.xres {
            return Ok(None);
        }
        let spec = ClientIdSpec {
            client: window,
            mask: ClientIdMask::LOCAL_CLIENT_PID,
        };
        let reply = match self.connection.res_query_client_ids(&[spec])?.reply() {
            Ok(reply) => reply,
            Err(ReplyError::X11Error(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(reply
            .ids
            .iter()
            .find(|id| id.spec.mask.contains(ClientIdMask::LOCAL_CLIENT_PID))
            .and_then(|id| id.value.first())
            .and_then(|&pid| i32::try_from(pid).ok())
            .filter(|&pid| pid > 0))
    }

    /// Every property the manager reads of `window`, asked at once, then waited for: none for a
    /// window gone meanwhile.
    fn read_all(&self, window: Window) -> Result<Vec<(Read, Option<Reply>)>, Failure> {
        let cookies = Read::ALL
            .into_iter()
            .map(|read| {
                let cookie = self.connection.get_property(
                    false,
                    window,
                    read.atom(&self.atoms),
                    AtomEnum::ANY,
                    0,
                    read.bound(),
                )?;
                Ok((read, cookie))
            })
            .collect::<Result<Vec<_>, ConnectionError>>()?;
        cookies
            .into_iter()
            .map(|(read, cookie)| Ok((read, answered(cookie.reply())?)))
            .collect()
    }

    /// One property of `window`; none for a window gone meanwhile.
    fn read_one(&self, window: Window, read: Read) -> Result<Option<Reply>, Failure> {
        let cookie = self.connection.get_property(
            false,
            window,
            read.atom(&self.atoms),
            AtomEnum::ANY,
            0,
            read.bound(),
        )?;
        answered(cookie.reply())
    }

    /// Decodes `raw`, `read`'s value, into `tracked`'s properties; a property not set, or
    /// malformed, sets its field to its default, as if it were not set, whatever it held before.
    /// A malformed value, or a title read from `COMPOUND_TEXT`, is logged once per window.
    fn apply(&self, window: Window, tracked: &mut Tracked, read: Read, raw: Option<&Reply>) {
        let unset = Reply {
            type_: 0,
            format: 0,
            value: Vec::new(),
            truncated: false,
        };
        let raw = raw.unwrap_or(&unset).raw();
        let outcome = decode(&self.atoms, read, raw, &mut tracked.properties);
        if outcome.is_err() {
            // Decoding what is not set gives the default, and never fails.
            let _ = decode(&self.atoms, read, unset.raw(), &mut tracked.properties);
        }
        if std::mem::replace(&mut tracked.logged, true) {
            return;
        }
        match outcome {
            Err(malformed) => tracing::info!(
                window,
                ?read,
                %malformed,
                "an X11 window's property is malformed and ignored; said once per window"
            ),
            Ok(Decoding::Compound) => tracing::info!(
                window,
                "an X11 window's title is COMPOUND_TEXT, read as Latin-1; said once per window"
            ),
            Ok(Decoding::Plain) => tracked.logged = false,
        }
    }
}

/// How a property was decoded, as far as the log is concerned.
enum Decoding {
    Plain,
    /// A title of `COMPOUND_TEXT`, read as Latin-1.
    Compound,
}

/// Decodes `raw`, `read`'s value, into its field of `properties`: the default when it is not
/// set; when it is malformed, the field is left as it was and the reason given (`apply` sets it
/// to its default then).
fn decode(
    atoms: &Atoms,
    read: Read,
    raw: Raw<'_>,
    properties: &mut Properties,
) -> Result<Decoding, Malformed> {
    let types = TextTypes {
        utf8_string: atoms.UTF8_STRING,
        string: AtomEnum::STRING.into(),
        compound_text: atoms.COMPOUND_TEXT,
    };
    let cardinal: Atom = AtomEnum::CARDINAL.into();
    let atom: Atom = AtomEnum::ATOM.into();
    match read {
        Read::NetWmName | Read::WmName => {
            let text = properties::decode_text(raw, types)?;
            let compound = text.as_ref().is_some_and(|text| text.compound);
            let text = text.map(|text| text.text);
            if read == Read::NetWmName {
                properties.net_wm_name = text;
            } else {
                properties.wm_name = text;
            }
            if compound {
                return Ok(Decoding::Compound);
            }
        }
        Read::Class => properties.class = properties::decode_class(raw, types.string)?,
        Read::NormalHints => {
            properties.size_hints =
                properties::decode_size_hints(raw, AtomEnum::WM_SIZE_HINTS.into())?
                    .unwrap_or_default();
        }
        Read::Hints => {
            properties.accepts_input =
                properties::decode_input_hint(raw, AtomEnum::WM_HINTS.into())?;
        }
        Read::Protocols => {
            let listed = properties::decode_atoms(raw, atom)?.unwrap_or_default();
            properties.protocols = Protocols {
                delete_window: listed.contains(&atoms.WM_DELETE_WINDOW),
                take_focus: listed.contains(&atoms.WM_TAKE_FOCUS),
                ping: listed.contains(&atoms._NET_WM_PING),
                sync_request: listed.contains(&atoms._NET_WM_SYNC_REQUEST),
            };
        }
        Read::TransientFor => {
            properties.transient_for = properties::decode_window(raw, AtomEnum::WINDOW.into())?;
        }
        Read::WindowType => {
            let listed = properties::decode_atoms(raw, atom)?.unwrap_or_default();
            properties.window_type = window_type(atoms, &listed);
        }
        Read::Motif => {
            properties.decorations =
                properties::decode_motif_decorations(raw, atoms._MOTIF_WM_HINTS)?;
        }
        Read::FrameExtents => {
            properties.frame_extents =
                properties::decode_frame_extents(raw, cardinal)?.unwrap_or_default();
        }
        Read::Pid => properties.pid = properties::decode_pid(raw, cardinal)?,
        Read::State => {
            let listed = properties::decode_atoms(raw, atom)?.unwrap_or_default();
            properties.wishes = Wishes {
                fullscreen: listed.contains(&atoms._NET_WM_STATE_FULLSCREEN),
                maximized: listed.contains(&atoms._NET_WM_STATE_MAXIMIZED_HORZ),
            };
        }
        Read::Emulated => {
            properties.emulated = properties::decode_emulated(raw, cardinal)?.unwrap_or_default();
        }
        Read::SyncCounter => {
            properties.sync_counter = properties::decode_counter(raw, cardinal)?;
        }
        Read::StartupId => {
            properties.startup_id = properties::decode_text(raw, types)?.map(|text| text.text);
        }
    }
    Ok(Decoding::Plain)
}

/// A property's value as the manager keeps it while it decodes it.
struct Reply {
    type_: Atom,
    format: u8,
    value: Vec<u8>,
    truncated: bool,
}

impl Reply {
    fn raw(&self) -> Raw<'_> {
        Raw {
            type_: self.type_,
            format: self.format,
            value: &self.value,
            truncated: self.truncated,
        }
    }
}

/// A `GetProperty` reply as the manager keeps it: none when X11 refused the request, for a
/// window gone; a lost connection fails.
fn answered(
    reply: Result<x11rb::protocol::xproto::GetPropertyReply, ReplyError>,
) -> Result<Option<Reply>, Failure> {
    match reply {
        Ok(reply) => Ok(Some(Reply {
            type_: reply.type_,
            format: reply.format,
            truncated: reply.bytes_after > 0,
            value: reply.value,
        })),
        Err(ReplyError::X11Error(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The first type of `listed` the manager knows, EWMH listing them by preference; normal
/// without one.
fn window_type(atoms: &Atoms, listed: &[Atom]) -> WindowType {
    listed
        .iter()
        .find_map(|&atom| {
            Some(match atom {
                a if a == atoms._NET_WM_WINDOW_TYPE_NORMAL => WindowType::Normal,
                a if a == atoms._NET_WM_WINDOW_TYPE_DIALOG => WindowType::Dialog,
                a if a == atoms._NET_WM_WINDOW_TYPE_UTILITY => WindowType::Utility,
                a if a == atoms._NET_WM_WINDOW_TYPE_TOOLBAR => WindowType::Toolbar,
                a if a == atoms._NET_WM_WINDOW_TYPE_SPLASH => WindowType::Splash,
                a if a == atoms._NET_WM_WINDOW_TYPE_DOCK => WindowType::Dock,
                a if a == atoms._NET_WM_WINDOW_TYPE_DESKTOP => WindowType::Desktop,
                a if [
                    atoms._NET_WM_WINDOW_TYPE_MENU,
                    atoms._NET_WM_WINDOW_TYPE_DROPDOWN_MENU,
                    atoms._NET_WM_WINDOW_TYPE_POPUP_MENU,
                    atoms._NET_WM_WINDOW_TYPE_TOOLTIP,
                    atoms._NET_WM_WINDOW_TYPE_NOTIFICATION,
                    atoms._NET_WM_WINDOW_TYPE_COMBO,
                    atoms._NET_WM_WINDOW_TYPE_DND,
                ]
                .contains(&a) =>
                {
                    WindowType::Transient
                }
                _ => return None,
            })
        })
        .unwrap_or_default()
}

/// Moves and sizes `window` to `rect`, without a border, and tells it with a synthetic
/// `ConfigureNotify`, as ICCCM 4.1.5 asks of a manager that moves a window: the real one does
/// not come when only its place relative to its parent is unchanged. The rectangle is cut to
/// X11's 16-bit coordinates.
fn configure(connection: &RustConnection, window: Window, rect: X11Rect) -> Result<(), Failure> {
    let x = i16::try_from(rect.x).unwrap_or(if rect.x < 0 { i16::MIN } else { i16::MAX });
    let y = i16::try_from(rect.y).unwrap_or(if rect.y < 0 { i16::MIN } else { i16::MAX });
    let side = |value: u32| u16::try_from(value.clamp(1, 32_767)).unwrap_or(u16::MAX);
    let (width, height) = (side(rect.width), side(rect.height));
    connection.configure_window(
        window,
        &ConfigureWindowAux::new()
            .x(i32::from(x))
            .y(i32::from(y))
            .width(u32::from(width))
            .height(u32::from(height))
            .border_width(0),
    )?;
    let notify = ConfigureNotifyEvent {
        response_type: x11rb::protocol::xproto::CONFIGURE_NOTIFY_EVENT,
        sequence: 0,
        event: window,
        window,
        above_sibling: x11rb::NONE,
        x,
        y,
        width,
        height,
        border_width: 0,
        override_redirect: false,
    };
    connection.send_event(false, window, EventMask::STRUCTURE_NOTIFY, notify)?;
    Ok(())
}

/// Reads every byte waiting on the wake-up socket; false once Yuna's end has closed.
fn drain(woken: &UnixStream) -> Result<bool, Failure> {
    let mut bytes = [0; 64];
    loop {
        match rustix::net::recv(woken.as_fd(), &mut bytes, RecvFlags::DONTWAIT) {
            Ok((0, _)) => return Ok(false),
            Ok(_) => {}
            Err(Errno::AGAIN) => return Ok(true),
            Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
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
