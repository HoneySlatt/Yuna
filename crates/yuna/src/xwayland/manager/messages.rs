//! What passes between a manager's thread and Yuna's loop: the events the thread posts,
//! [`ManagerEvent`], and the commands Yuna's loop sends back, [`Command`], with the values they
//! carry. They are plain data, apart from the code on either side, so that one file shows all
//! the manager tells Yuna and all it takes.
//!
//! The parent module re-exports them: Yuna's side names them `manager::ManagerEvent`,
//! `manager::Command` and so on. On the thread, `x11_events.rs` posts what X11 windows do and
//! `commands.rs` carries the commands out, the selections' and the drags' through
//! `xwayland/bridge.rs` and `xwayland/xdnd.rs`; on Yuna's loop, `xwayland/mod.rs` holds the
//! events' channel and takes the manager's start and end, and `windows.rs`, `answers.rs`,
//! `selections.rs` and `drags.rs`, in `xwayland/` too, take the other events and send the
//! commands.

use std::os::fd::OwnedFd;

use x11rb::protocol::xproto::Window;

use crate::xwayland::bridge::Selection;
use crate::xwayland::drag_session::DragAction;
use crate::xwayland::properties::{Properties, X11Rect};

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
    /// An X11 client took `selection`, the `owner`-th owner the manager saw. Its targets read,
    /// it offers these MIME types: none when Wayland can read nothing of it. `wayland_serial` is
    /// the serial of the last Wayland selection the manager was told of then.
    SelectionTaken {
        selection: Selection,
        owner: u64,
        mime_types: Vec<String>,
        wayland_serial: u64,
    },
    /// No one owns `selection` any more: its X11 owner set none, or its window or its client
    /// went.
    SelectionDropped(Selection),
    /// An X11 client reads the seat's Wayland `selection` as `mime_type`: Yuna answers transfer
    /// `transfer` with [`Command::WaylandData`].
    ReadWayland {
        selection: Selection,
        mime_type: String,
        transfer: u64,
    },
    /// A transfer of [`Command::SendToWayland`] or [`Command::SendX11Drag`] ended, whatever its
    /// outcome, refused at once included: one fewer runs.
    TransferToWaylandEnded,
    /// An X11 client took `XdndSelection`, the `drag`-th taking the manager saw: an X11 drag, if
    /// Yuna starts it. `client` is the bits of the owner's id that name its client.
    X11DragOffered { drag: u64, client: u32 },
    /// X11 drag `drag`'s source sent `XdndEnter` to the proxy, then its first `XdndPosition`: it
    /// offers these MIME types, and proposes `action`.
    X11DragEntered {
        drag: u64,
        mime_types: Vec<String>,
        action: DragAction,
    },
    /// X11 drag `drag`'s source sent `XdndLeave` to the proxy.
    X11DragLeft(u64),
    /// X11 drag `drag`'s source sent `XdndDrop` to the proxy: Yuna answers with
    /// [`Command::FinishX11Drag`].
    X11DragDropped(u64),
    /// The action X11 drag `drag`'s positions propose changed.
    X11DragAction { drag: u64, action: DragAction },
    /// X11 drag `drag` ended on X11's side: its source gave `XdndSelection` up or left, another
    /// took it, or its drop was not finished in time.
    X11DragEnded(u64),
    /// The proxy was mapped for X11 drag `drag`.
    DragProxyMapped(u64),
    /// The proxy was unmapped while X11 drag `drag` is in the hand.
    DragProxyUnmapped(u64),
    /// The X11 target of Wayland drag `drag` changed its answer: whether it accepts, its action.
    WaylandDragStatus {
        drag: u64,
        accepted: bool,
        action: DragAction,
    },
    /// The X11 target of Wayland drag `drag` reads it as `mime_type`: Yuna answers transfer
    /// `transfer` with [`Command::WaylandData`].
    ReadWaylandDrag {
        drag: u64,
        mime_type: String,
        transfer: u64,
    },
    /// Wayland drag `drag` ended on X11's side: performed with this action, as its target's
    /// `XdndFinished` said; or not, refused, never answered, its target or its `XdndSelection`
    /// gone.
    WaylandDragEnded {
        drag: u64,
        performed: Option<DragAction>,
    },
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
    /// The seat's Wayland `selection`, its `serial`-th, is a Wayland client's offering these MIME
    /// types, for which the manager takes the X11 selection; or none, and the manager releases
    /// it if it holds it.
    OwnSelection {
        selection: Selection,
        serial: u64,
        mime_types: Option<Vec<String>>,
    },
    /// A Wayland client reads `selection`, offered by X11 owner `owner`, as `mime_type`, into
    /// `pipe`. Its end is posted, [`ManagerEvent::TransferToWaylandEnded`].
    SendToWayland {
        selection: Selection,
        owner: u64,
        mime_type: String,
        pipe: OwnedFd,
    },
    /// The read end of the pipe the Wayland source writes into for transfer `transfer` of
    /// [`ManagerEvent::ReadWayland`] or [`ManagerEvent::ReadWaylandDrag`]; none when the source
    /// cannot be read.
    WaylandData {
        transfer: u64,
        source: Option<OwnedFd>,
    },
    /// Shows the proxy for X11 drag `drag`, over a Wayland window or over nothing, or hides it.
    ShowDragProxy { drag: u64, shown: bool },
    /// The Wayland target's answer to X11 drag `drag`, which its source's `XdndStatus` carries.
    AnswerX11Drag {
        drag: u64,
        accepted: bool,
        action: DragAction,
    },
    /// A Wayland target reads X11 drag `drag` as `mime_type` into `pipe`. Its end is posted,
    /// [`ManagerEvent::TransferToWaylandEnded`].
    SendX11Drag {
        drag: u64,
        mime_type: String,
        pipe: OwnedFd,
    },
    /// Ends X11 drag `drag` with `XdndFinished`: accepted and the action performed, or refused.
    FinishX11Drag {
        drag: u64,
        accepted: bool,
        action: DragAction,
    },
    /// Ends X11 drag `drag` with nothing more said to its source.
    ForgetX11Drag(u64),
    /// Wayland drag `drag` reached an X11 window for the first time: its MIME types, 64 at most
    /// of 255 bytes at most, and the action it proposes.
    StartWaylandDrag {
        drag: u64,
        mime_types: Vec<String>,
        action: DragAction,
    },
    /// Wayland drag `drag` is over this X11 window at this point in root pixels, or over none.
    WaylandDragOver {
        drag: u64,
        over: Option<(Window, (i16, i16))>,
    },
    /// Wayland drag `drag` was released over its X11 target: dropped if the target accepted, else
    /// left, and the outcome posted, [`ManagerEvent::WaylandDragEnded`].
    DropWaylandDrag(u64),
    /// Wayland drag `drag` ends on X11's side, with nothing posted: left if not dropped, its
    /// `XdndSelection` released.
    CancelWaylandDrag(u64),
}
