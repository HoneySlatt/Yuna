//! Drag and drop between a seat's X11 programs and its Wayland programs, on X11's side: a part of
//! the seat's X11 window manager, on its thread (`manager.rs`), which alone speaks XDND, version 5
//! (<https://www.freedesktop.org/wiki/Specifications/XDND/>), and owns the proxy and
//! `XdndSelection`. Yuna's loop decides (`drag_session.rs`, carried out by `drags.rs`) and never
//! waits on X11; the manager answers each X11 peer at once from what Yuna last told it. A drag's
//! data moves through the bridge's transfers, within their bounds and count (`bridge.rs`).
//!
//! **X11 to Wayland.** When an X11 client takes `XdndSelection`, the manager posts it, numbered,
//! with the client; Yuna starts a drag only from a button its pointer holds on a window of that
//! client. The proxy is one InputOnly override-redirect window of the manager's over the whole
//! root, `XdndAware` 5: never a window or a menu for Yuna, never in `_NET_CLIENT_LIST`, mapped
//! and raised while Yuna asks, so that the source, which looks for its target in X11, finds it
//! over Wayland windows. The source's `XdndEnter` on the proxy, from a window of the drag's
//! client and of a version from 3 to 5, gives its types, inline or in `XdndTypeList`, read up to
//! [`MAX_TYPES`], mapped to MIME types as the clipboard's (`offer_of_targets`). Each
//! `XdndPosition` is answered at once with `XdndStatus`: accepted as Yuna last said, bit 1 set and
//! an empty rectangle, so that every motion sends a position. Yuna is told only of the enter, at
//! the first position after it, with the action it proposes, so that a Wayland target is offered
//! types and action at once; of the leave, the drop and the action proposed changing; an answer
//! that changes is sent at once. A
//! message whose first value is not the drag's source window is left. A Wayland target's read
//! converts `XdndSelection` with the time of the latest position, or of the drop; refused while
//! the source sent a newer position, as Qt refuses every time but its last, it is asked again
//! with the newer one, a few times at most (`bridge.rs`). Yuna's finish is the source's
//! `XdndFinished`, after a `DELETE` conversion for a move; without it [`X11_DRAG_TIMEOUT`] after
//! the drop, with no transfer moving, ended transfers included, the drop is refused.
//!
//! **Wayland to X11.** Yuna says which X11 window a Wayland client's drag is over, at which root
//! point. The manager takes `XdndSelection` on its own window once per drag, and waits for XFixes
//! to report that taking, whose time every message names and every request is checked against.
//! The target is the window's `XdndProxy`, when it names a window whose own `XdndProxy` names
//! itself, else the window; it needs `XdndAware` 3 or more, and is spoken to at the lower of its
//! version and 5. `XdndEnter` carries three types, `XdndTypeList` on the manager's window the
//! rest; each motion sends `XdndPosition`, one at a time awaiting its status, the latest point
//! kept meanwhile. Every message names the window under the pointer, as XDND asks, and the
//! target's answers are read by the first value naming that window. Each `XdndStatus` changing
//! the answer is posted. At the release, the kept position goes, its status is waited for
//! [`X11_DRAG_TIMEOUT`], then `XdndDrop` if the target accepted, else `XdndLeave`; a target that
//! never answered is left at once, as XDND asks. Its `XdndFinished` ends the drag, as performed or
//! not; none [`X11_DRAG_TIMEOUT`] after the drop with no transfer moving cancels it. The target's
//! conversions are served from the Wayland source only to windows of the client its messages go
//! to: the window's own, or its proxy's. Another client taking `XdndSelection` before the drop,
//! or Yuna's word, ends the drag on X11's side, and the selection is released; a target destroyed
//! while the button is held is forgotten, and one destroyed after the release ends the drag.
//!
//! **Bounds.** At most [`MAX_DRAGS`] each way: the one in the hand and one ending, the oldest
//! ending one going first. One position at most waits, replaced, never queued. Nothing waits
//! when no drag lives.

use std::time::{Duration, Instant};

use smithay::reexports::calloop::channel::SyncSender;
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{self, SelectionEvent};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask,
    PropMode, SelectionNotifyEvent, SelectionRequestEvent, StackMode, Timestamp, Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::bridge::{
    Bridge, Served, X11_TRANSFER_TIMEOUT, offer_of_targets, refused_is_none, targets_of_offer,
};
use super::drag_session::DragAction;
use super::manager::{Atoms, Failure, GoOn, ManagerEvent};

/// The XDND version Yuna speaks: the last.
const XDND_VERSION: u8 = 5;

/// The oldest XDND version Yuna takes: version 3 moved `XdndAware` to top-level windows, and
/// XDND asks no less.
const OLDEST_VERSION: u8 = 3;

/// How long the manager waits on a drag's X11 peer or on Yuna: a target's status at the
/// release; a target's `XdndFinished`, or a Wayland target's finish, after the drop, with no
/// transfer of the drag moving; a source's answer to a move's `DELETE`. As long as a transfer
/// waits ([`X11_TRANSFER_TIMEOUT`]): KWin gives a target 2 s, Mutter waits on.
const X11_DRAG_TIMEOUT: Duration = X11_TRANSFER_TIMEOUT;

/// The most types read of a source's `XdndTypeList`: the bound on every list of atoms the
/// manager reads (ADR-0070).
const MAX_TYPES: u32 = 64;

/// How many drags each way the manager follows: the one in the hand, and one ending.
const MAX_DRAGS: usize = 2;

/// The side of the proxy, in X11 pixels: the largest a window's position reaches, so that it
/// covers any root.
const PROXY_SIDE: u16 = 32_767;

/// The 32-bit data of an XDND client message.
type Data = [u32; 5];

/// A position's root point in a 32-bit value: x in its high half, y in its low.
fn packed((x, y): (i16, i16)) -> u32 {
    // Each 16-bit pattern, sign bits included, as XDND packs it.
    (u32::from(x as u16) << 16) | u32::from(y as u16)
}

/// An `XdndEnter`, read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Enter {
    source: Window,
    version: u8,
    /// Whether the source has more than three types, listed in its `XdndTypeList`.
    type_list: bool,
    types: [Atom; 3],
}

/// `XdndEnter` from `source`, at `version`, with `types`: three inline, unused slots none, and
/// bit 0 saying the rest are in `XdndTypeList`.
fn enter_data(source: Window, version: u8, types: &[Atom]) -> Data {
    let mut data = [
        source,
        (u32::from(version) << 24) | u32::from(types.len() > 3),
        x11rb::NONE,
        x11rb::NONE,
        x11rb::NONE,
    ];
    for (slot, &atom) in data[2..].iter_mut().zip(types) {
        *slot = atom;
    }
    data
}

/// An `XdndEnter`'s data, or none for a version Yuna does not take: below 3, which XDND no longer
/// asks to support, or above 5, which XDND asks a target to ignore.
fn read_enter(data: Data) -> Option<Enter> {
    let version = (data[1] >> 24) as u8;
    (OLDEST_VERSION..=XDND_VERSION)
        .contains(&version)
        .then_some(Enter {
            source: data[0],
            version,
            type_list: data[1] & 1 != 0,
            types: [data[2], data[3], data[4]],
        })
}

/// `XdndPosition` from `source`: the pointer at `point` in root pixels, the data named with
/// `time`, `action` asked.
fn position_data(source: Window, point: (i16, i16), time: Timestamp, action: Atom) -> Data {
    [source, 0, packed(point), time, action]
}

/// An `XdndPosition`, read: its source, its time and its action; its point is Yuna's pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Position {
    source: Window,
    time: Timestamp,
    action: Atom,
}

fn read_position(data: Data) -> Position {
    Position {
        source: data[0],
        time: data[3],
        action: data[4],
    }
}

/// `XdndStatus` from `target`: bit 0 when it accepts, with `action`, none otherwise; bit 1 always
/// and an empty rectangle, so that the source sends a position at every motion.
fn status_data(target: Window, accepted: bool, action: Atom) -> Data {
    let action = if accepted { action } else { x11rb::NONE };
    [target, u32::from(accepted) | 2, 0, 0, action]
}

/// An `XdndStatus`, read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Status {
    target: Window,
    accepted: bool,
    action: Atom,
}

fn read_status(data: Data) -> Status {
    Status {
        target: data[0],
        accepted: data[1] & 1 != 0,
        action: data[4],
    }
}

/// `XdndLeave` from `source`.
fn leave_data(source: Window) -> Data {
    [source, 0, 0, 0, 0]
}

/// `XdndDrop` from `source`, the data named with `time`.
fn drop_data(source: Window, time: Timestamp) -> Data {
    [source, 0, time, 0, 0]
}

/// `XdndFinished` from `target`, to a source of `version`: whether it accepted, and the action it
/// performed, from version 5; nothing before, as XDND asks of every unused field.
fn finished_data(target: Window, version: u8, accepted: bool, action: Atom) -> Data {
    if version < 5 {
        return [target, 0, 0, 0, 0];
    }
    let action = if accepted { action } else { x11rb::NONE };
    [target, u32::from(accepted), action, 0, 0]
}

/// An `XdndFinished`, read from a target of `version`: before version 5 it always accepted, its
/// action unsaid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Finished {
    target: Window,
    accepted: bool,
    action: Atom,
}

fn read_finished(data: Data, version: u8) -> Finished {
    if version < 5 {
        return Finished {
            target: data[0],
            accepted: true,
            action: x11rb::NONE,
        };
    }
    Finished {
        target: data[0],
        accepted: data[1] & 1 != 0,
        action: data[2],
    }
}

/// The atoms of XDND's actions.
#[derive(Debug, Clone, Copy)]
struct Actions {
    copy: Atom,
    move_: Atom,
    link: Atom,
    ask: Atom,
    private: Atom,
}

impl Actions {
    fn of(atoms: &Atoms) -> Actions {
        Actions {
            copy: atoms.XdndActionCopy,
            move_: atoms.XdndActionMove,
            link: atoms.XdndActionLink,
            ask: atoms.XdndActionAsk,
            private: atoms.XdndActionPrivate,
        }
    }

    /// The atom of `action`: none for none.
    fn atom(self, action: DragAction) -> Atom {
        match action {
            DragAction::None => x11rb::NONE,
            DragAction::Copy => self.copy,
            DragAction::Move => self.move_,
            DragAction::Ask => self.ask,
        }
    }

    /// What an X11 peer's action reads as: Link, which only cooperating programs share, and
    /// Private, a copy the target does something of its own with, read as Copy, Wayland having
    /// neither. None for none and for an action XDND does not name.
    fn action(self, atom: Atom) -> Option<DragAction> {
        match atom {
            x11rb::NONE => None,
            a if a == self.copy || a == self.link || a == self.private => Some(DragAction::Copy),
            a if a == self.move_ => Some(DragAction::Move),
            a if a == self.ask => Some(DragAction::Ask),
            _ => None,
        }
    }
}

/// An X11 program's drag, on the manager's side.
#[derive(Debug)]
struct FromX11 {
    /// Its number among the takings of `XdndSelection` the manager saw.
    drag: u64,
    /// The bits of the owner's window that name its client.
    client: u32,
    /// The time a conversion of its data names: its taking's, then its latest position's or its
    /// drop's.
    time: Timestamp,
    /// The window its messages come from and their version, once its `XdndEnter` came, and
    /// until its `XdndLeave`.
    source: Option<(Window, u8)>,
    /// What it offers: each MIME type with its target.
    offer: Vec<(String, Atom)>,
    /// The action its positions propose, as Yuna was last told.
    proposed: DragAction,
    /// Whether Yuna was told of its `XdndEnter`: at the first `XdndPosition` after it, with the
    /// action that position proposes, so that a Wayland target is offered the types and the
    /// action at once, and never chooses its action before the source's is known.
    announced: bool,
    /// The Wayland target's answer, as Yuna last gave it.
    answer: (bool, DragAction),
    /// The answer its source was last sent.
    sent: Option<(bool, DragAction)>,
    /// Whether Yuna wants the proxy shown for it.
    proxy_wanted: bool,
    /// When its `XdndDrop` came: Yuna's finish is waited for from then.
    dropped: Option<Instant>,
    /// A move's `DELETE`, converted before its `XdndFinished`: the window, and when it was asked.
    deleting: Option<(Window, Instant)>,
}

impl FromX11 {
    /// What `XdndFinished` its source is sent when Yuna forgets it: a refusal once it dropped,
    /// none before, its source having dropped nothing.
    fn finished_when_forgotten(&self) -> Option<(bool, DragAction)> {
        self.dropped.map(|_| (false, DragAction::None))
    }
}

/// A Wayland client's drag over X11 windows, on the manager's side.
#[derive(Debug)]
struct FromWayland {
    /// Yuna's number for it.
    drag: u64,
    /// Its types as X11 targets, each with its MIME type.
    targets: Vec<(Atom, String)>,
    /// The action it proposes.
    action: DragAction,
    taken: Taken,
    /// The X11 window it is over, as Yuna last said, and the root point.
    over: Option<(Window, (i16, i16))>,
    /// The X11 target it speaks to.
    target: Option<Target>,
    /// A position was sent whose status has not come.
    in_flight: bool,
    /// `over`'s point waits to be sent.
    kept: bool,
    /// The target's last status: whether it accepts, and its action.
    status: Option<(bool, DragAction)>,
    /// The status Yuna was last told.
    told: (bool, DragAction),
    stage: Stage,
}

/// Whether a Wayland client's drag took `XdndSelection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Taken {
    No,
    /// Asked, XFixes has not reported it yet.
    Asked,
    /// Reported, at this time.
    At(Timestamp),
}

/// The X11 window a Wayland client's drag speaks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    /// The top-level window under the pointer.
    window: Window,
    /// The window its messages go to: its `XdndProxy`, or itself.
    dest: Window,
    version: u8,
    /// The bits of `dest` that name its client: the program that gets the messages, and may
    /// read the data.
    client: u32,
    /// Whether `XdndEnter` was sent.
    entered: bool,
}

impl Target {
    /// Whether `window` is one of its two windows, gone with it.
    fn involves(&self, window: Window) -> bool {
        window == self.window || window == self.dest
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// In the hand.
    Dragging,
    /// Released: the last position's status is waited for, since then.
    Releasing { since: Instant },
    /// `XdndDrop` sent then: the target's `XdndFinished` is waited for.
    Dropped { at: Instant },
}

/// Drag and drop on X11's side of a seat's manager.
pub(super) struct Xdnd<'a> {
    connection: &'a RustConnection,
    events: &'a SyncSender<ManagerEvent>,
    atoms: Atoms,
    actions: Actions,
    /// The manager's own window: the source of Wayland clients' drags, and their
    /// `XdndSelection`'s owner.
    own: Window,
    /// The window X11 sources find over Wayland windows.
    proxy: Window,
    /// The bits of a resource id that name the resource within its client.
    resource_mask: u32,
    /// Whether the proxy is mapped, as the manager last asked X11.
    proxy_mapped: bool,
    /// X11 programs' drags, oldest first.
    from_x11: Vec<FromX11>,
    next_from_x11: u64,
    /// The X11 drag whose taking of `XdndSelection` is the latest the manager saw, while no
    /// other took it since.
    x11_owner: Option<u64>,
    /// Wayland clients' drags, oldest first.
    from_wayland: Vec<FromWayland>,
    /// How many of the manager's takings of `XdndSelection` XFixes has not reported yet.
    taking: u32,
    /// The Wayland drag that holds `XdndSelection`, and the time XFixes reported for it.
    owner: Option<(u64, Timestamp)>,
}

impl<'a> Xdnd<'a> {
    pub(super) fn new(
        connection: &'a RustConnection,
        events: &'a SyncSender<ManagerEvent>,
        atoms: Atoms,
        own: Window,
        proxy: Window,
    ) -> Xdnd<'a> {
        Xdnd {
            connection,
            events,
            atoms,
            actions: Actions::of(&atoms),
            own,
            proxy,
            resource_mask: connection.setup().resource_id_mask,
            proxy_mapped: false,
            from_x11: Vec::new(),
            next_from_x11: 1,
            x11_owner: None,
            from_wayland: Vec::new(),
            taking: 0,
            owner: None,
        }
    }

    /// The proxy, a window of the manager's that it never follows as a client's.
    pub(super) fn proxy(&self) -> Window {
        self.proxy
    }

    fn post(&self, event: ManagerEvent) -> GoOn {
        self.events.send(event).is_ok()
    }

    fn client_of(&self, window: Window) -> u32 {
        window & !self.resource_mask
    }

    /// Sends `data`, an XDND message of type `type_` about `window`, to `to`, delivered to the
    /// client that made it.
    fn send(&self, to: Window, window: Window, type_: Atom, data: Data) -> Result<(), Failure> {
        let message = ClientMessageEvent::new(32, window, type_, data);
        self.connection
            .send_event(false, to, EventMask::NO_EVENT, message)?;
        Ok(())
    }

    /// Whether `message` is XDND's: the manager's only.
    pub(super) fn takes(&self, message: &ClientMessageEvent) -> bool {
        let atoms = &self.atoms;
        message.format == 32
            && [
                atoms.XdndEnter,
                atoms.XdndPosition,
                atoms.XdndStatus,
                atoms.XdndLeave,
                atoms.XdndDrop,
                atoms.XdndFinished,
            ]
            .contains(&message.type_)
    }

    /// An XDND message: a source's to the proxy, or a target's to the manager's window.
    pub(super) fn client_message(
        &mut self,
        message: &ClientMessageEvent,
        bridge: &mut Bridge<'_>,
    ) -> Result<GoOn, Failure> {
        let data = message.data.as_data32();
        let type_ = message.type_;
        let atoms = self.atoms;
        if type_ == atoms.XdndEnter {
            self.entered(data, bridge)
        } else if type_ == atoms.XdndPosition {
            self.positioned(data)
        } else if type_ == atoms.XdndLeave {
            self.left(data)
        } else if type_ == atoms.XdndDrop {
            self.dropped(data)
        } else if type_ == atoms.XdndStatus {
            self.status(data)
        } else {
            self.finished(data)
        }
    }

    // X11 programs' drags.

    /// The X11 drag in the hand, if one is: the newest, not dropped.
    fn current_x11(&mut self) -> Option<&mut FromX11> {
        self.from_x11
            .last_mut()
            .filter(|drag| drag.dropped.is_none())
    }

    /// The X11 drag in the hand, if `source` is its source window.
    fn x11_from(&mut self, source: Window) -> Option<&mut FromX11> {
        self.current_x11()
            .filter(|drag| drag.source.is_some_and(|(window, _)| window == source))
    }

    fn x11_drag(&mut self, drag: u64) -> Option<&mut FromX11> {
        self.from_x11.iter_mut().find(|of| of.drag == drag)
    }

    /// `XdndEnter` on the proxy: from a window of the client of the drag in the hand, at a version
    /// Yuna takes; its types are read, and posted with the first position's action.
    fn entered(&mut self, data: Data, bridge: &mut Bridge<'_>) -> Result<GoOn, Failure> {
        let Some(enter) = read_enter(data) else {
            tracing::debug!(
                version = data[1] >> 24,
                "an XdndEnter of a version Yuna does not take is ignored"
            );
            return Ok(true);
        };
        let client = self.client_of(enter.source);
        if !self.current_x11().is_some_and(|drag| drag.client == client) {
            return Ok(true);
        }
        let types: Vec<Atom> = if enter.type_list {
            self.type_list(enter.source)?
        } else {
            enter
                .types
                .into_iter()
                .filter(|&atom| atom != x11rb::NONE)
                .collect()
        };
        let named = bridge.names_of(&types)?;
        let offer = offer_of_targets(&named);
        let Some(drag) = self.current_x11() else {
            return Ok(true);
        };
        drag.source = Some((enter.source, enter.version));
        drag.offer = offer;
        drag.sent = None;
        drag.announced = false;
        Ok(true)
    }

    /// The atoms of `source`'s `XdndTypeList`, at most [`MAX_TYPES`]; none when it holds no list.
    fn type_list(&self, source: Window) -> Result<Vec<Atom>, Failure> {
        let reply = self
            .connection
            .get_property(
                false,
                source,
                self.atoms.XdndTypeList,
                AtomEnum::ATOM,
                0,
                MAX_TYPES,
            )?
            .reply();
        let Some(reply) = refused_is_none(reply)? else {
            return Ok(Vec::new());
        };
        Ok(reply
            .value32()
            .map(|atoms| atoms.filter(|&atom| atom != x11rb::NONE).collect())
            .unwrap_or_default())
    }

    /// `XdndPosition` from the source: its time names the conversions from now on, its action
    /// changing is posted, and it is answered at once.
    fn positioned(&mut self, data: Data) -> Result<GoOn, Failure> {
        let position = read_position(data);
        let actions = self.actions;
        let Some(drag) = self.x11_from(position.source) else {
            return Ok(true);
        };
        if position.time != x11rb::CURRENT_TIME {
            drag.time = position.time;
        }
        // XDND's default, for an action Yuna does not know.
        let proposed = actions.action(position.action).unwrap_or(DragAction::Copy);
        let changed = std::mem::replace(&mut drag.proposed, proposed) != proposed;
        let number = drag.drag;
        let event = if !std::mem::replace(&mut drag.announced, true) {
            let mime_types = drag.offer.iter().map(|(mime_type, _)| mime_type.clone());
            Some(ManagerEvent::X11DragEntered {
                drag: number,
                mime_types: mime_types.collect(),
                action: proposed,
            })
        } else {
            changed.then_some(ManagerEvent::X11DragAction {
                drag: number,
                action: proposed,
            })
        };
        if let Some(event) = event
            && !self.post(event)
        {
            return Ok(false);
        }
        self.send_status(number)?;
        Ok(true)
    }

    /// Sends X11 drag `drag`'s source the Wayland target's answer.
    fn send_status(&mut self, drag: u64) -> Result<(), Failure> {
        let (proxy, actions, status) = (self.proxy, self.actions, self.atoms.XdndStatus);
        let Some(of) = self.x11_drag(drag) else {
            return Ok(());
        };
        let Some((source, _)) = of.source else {
            return Ok(());
        };
        of.sent = Some(of.answer);
        let (accepted, action) = of.answer;
        let data = status_data(
            proxy,
            accepted,
            actions.atom(answered_action(accepted, action)),
        );
        self.send(source, source, status, data)
    }

    /// `XdndLeave` from the source: posted; a later `XdndEnter` starts again.
    fn left(&mut self, data: Data) -> Result<GoOn, Failure> {
        let Some(drag) = self.x11_from(data[0]) else {
            return Ok(true);
        };
        drag.source = None;
        let drag = drag.drag;
        Ok(self.post(ManagerEvent::X11DragLeft(drag)))
    }

    /// `XdndDrop` from the source: its time names the conversions, Yuna's finish is waited for,
    /// and the proxy hides.
    fn dropped(&mut self, data: Data) -> Result<GoOn, Failure> {
        let Some(drag) = self.x11_from(data[0]) else {
            return Ok(true);
        };
        if data[2] != x11rb::CURRENT_TIME {
            drag.time = data[2];
        }
        drag.dropped = Some(Instant::now());
        let drag = drag.drag;
        self.update_proxy()?;
        Ok(self.post(ManagerEvent::X11DragDropped(drag)))
    }

    /// Shows the proxy for X11 drag `drag`, or hides it, as Yuna asks.
    pub(super) fn show_proxy(&mut self, drag: u64, shown: bool) -> Result<(), Failure> {
        if let Some(of) = self.x11_drag(drag) {
            of.proxy_wanted = shown;
        }
        self.update_proxy()
    }

    /// Maps the proxy, raised over every window, while the drag in the hand wants it; unmaps it
    /// otherwise: mapped, it would take X11's pointer from every window once no source grabs it.
    /// Yuna asks it shown only over no X11 window; a drag dropped or ended hides it.
    fn update_proxy(&mut self) -> Result<(), Failure> {
        let wanted = self
            .from_x11
            .last()
            .is_some_and(|drag| drag.dropped.is_none() && drag.proxy_wanted);
        if wanted == self.proxy_mapped {
            return Ok(());
        }
        self.proxy_mapped = wanted;
        if wanted {
            self.raise_proxy()?;
            self.connection.map_window(self.proxy)?;
        } else {
            self.connection.unmap_window(self.proxy)?;
        }
        Ok(())
    }

    fn raise_proxy(&self) -> Result<(), Failure> {
        self.connection.configure_window(
            self.proxy,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        Ok(())
    }

    /// A child of the root was mapped: the proxy, which Yuna is told of, so that the source is
    /// shown the pointer again; or another window while the proxy is mapped, which the proxy
    /// goes over again.
    pub(super) fn mapped(&mut self, window: Window) -> Result<GoOn, Failure> {
        if window == self.proxy {
            return match self.current_x11() {
                Some(drag) => {
                    let drag = drag.drag;
                    Ok(self.post(ManagerEvent::DragProxyMapped(drag)))
                }
                None => Ok(true),
            };
        }
        if self.proxy_mapped {
            self.raise_proxy()?;
        }
        Ok(true)
    }

    /// The Wayland target's answer to X11 drag `drag`: its next status says it, and one goes at
    /// once when it changed.
    pub(super) fn answer(
        &mut self,
        drag: u64,
        accepted: bool,
        action: DragAction,
    ) -> Result<(), Failure> {
        let Some(of) = self.x11_drag(drag) else {
            return Ok(());
        };
        of.answer = (accepted, action);
        if of.dropped.is_none() && of.source.is_some() && of.sent != Some(of.answer) {
            return self.send_status(drag);
        }
        Ok(())
    }

    /// The time X11 drag `drag`'s data is read with now: its latest position's or its drop's;
    /// none once it ended.
    pub(super) fn drag_time(&self, drag: u64) -> Option<Timestamp> {
        self.from_x11
            .iter()
            .find(|of| of.drag == drag)
            .map(|of| of.time)
    }

    /// A Wayland target reads X11 drag `drag` as `mime_type` into `pipe`: `XdndSelection`
    /// converted with the drag's latest time, through the bridge.
    pub(super) fn send_to_wayland(
        &mut self,
        drag: u64,
        mime_type: &str,
        pipe: std::os::fd::OwnedFd,
        bridge: &mut Bridge<'_>,
    ) -> Result<GoOn, Failure> {
        let (target, time) = match self.x11_drag(drag) {
            Some(of) => (
                of.offer
                    .iter()
                    .find(|(offered, _)| offered == mime_type)
                    .map(|&(_, target)| target),
                of.time,
            ),
            None => (None, x11rb::CURRENT_TIME),
        };
        bridge.send_drag_to_wayland(drag, target, time, pipe)
    }

    /// Yuna ends X11 drag `drag` with `XdndFinished`: for a move accepted, once the source
    /// answered its `DELETE`, as XDND asks.
    pub(super) fn finish(
        &mut self,
        drag: u64,
        accepted: bool,
        action: DragAction,
        bridge: &Bridge<'_>,
    ) -> Result<(), Failure> {
        let (selection, delete, property) = (
            self.atoms.XdndSelection,
            self.atoms.DELETE,
            self.atoms._YUNA_SELECTION,
        );
        let Some(of) = self.x11_drag(drag) else {
            return Ok(());
        };
        if accepted && action == DragAction::Move && of.source.is_some() {
            let time = of.time;
            let window = bridge.transfer_window()?;
            self.connection
                .convert_selection(window, selection, delete, property, time)?;
            if let Some(of) = self.x11_drag(drag) {
                of.deleting = Some((window, Instant::now()));
            }
            return Ok(());
        }
        self.end_x11(drag, Some((accepted, action)))
    }

    /// Ends X11 drag `drag`: `XdndFinished` sent to its source, when `finished` says so; the
    /// proxy updated.
    fn end_x11(&mut self, drag: u64, finished: Option<(bool, DragAction)>) -> Result<(), Failure> {
        let Some(index) = self.from_x11.iter().position(|of| of.drag == drag) else {
            return Ok(());
        };
        let of = self.from_x11.remove(index);
        if let Some((window, _)) = of.deleting {
            self.connection.destroy_window(window)?;
        }
        if let (Some((accepted, action)), Some((source, version))) = (finished, of.source) {
            let data = finished_data(self.proxy, version, accepted, self.actions.atom(action));
            self.send(source, source, self.atoms.XdndFinished, data)?;
        }
        if self.x11_owner == Some(drag) {
            self.x11_owner = None;
        }
        self.update_proxy()
    }

    /// Whether the manager still follows X11 drag `drag`, an X11 program's.
    pub(super) fn follows_x11(&self, drag: u64) -> bool {
        self.from_x11.iter().any(|of| of.drag == drag)
    }

    /// Whether the manager still follows Wayland drag `drag`, a Wayland client's.
    pub(super) fn follows_wayland(&self, drag: u64) -> bool {
        self.from_wayland.iter().any(|of| of.drag == drag)
    }

    /// Yuna forgets X11 drag `drag`: nothing more is said to its source, but a refused
    /// `XdndFinished` if it dropped already, its drop on its way to Yuna when Yuna's time ran
    /// out.
    pub(super) fn forget(&mut self, drag: u64) -> Result<(), Failure> {
        let finished = self
            .from_x11
            .iter()
            .find(|of| of.drag == drag)
            .and_then(FromX11::finished_when_forgotten);
        self.end_x11(drag, finished)
    }

    /// An answer to a conversion of the manager's: a move's `DELETE`, after which its
    /// `XdndFinished` goes. Others are the bridge's.
    pub(super) fn converted(&mut self, answer: &SelectionNotifyEvent) -> Result<GoOn, Failure> {
        let deleted =
            answer.selection == self.atoms.XdndSelection && answer.target == self.atoms.DELETE;
        let drag = self
            .from_x11
            .iter()
            .find(|of| {
                of.deleting
                    .is_some_and(|(window, _)| window == answer.requestor)
            })
            .map(|of| of.drag);
        if let Some(drag) = drag.filter(|_| deleted) {
            self.end_x11(drag, Some((true, DragAction::Move)))?;
        }
        Ok(true)
    }

    // The owner of `XdndSelection`.

    /// XFixes says `XdndSelection`'s owner changed: the manager's own taking, reported; or an
    /// X11 client's, a new X11 drag; or none. Before its drop, a Wayland client's drag that held
    /// it ends; an X11 drag whose owner gave it up or left ends.
    pub(super) fn owner_changed(
        &mut self,
        changed: &xfixes::SelectionNotifyEvent,
        bridge: &mut Bridge<'_>,
    ) -> Result<GoOn, Failure> {
        if changed.owner == self.own {
            self.taking = self.taking.saturating_sub(1);
            if self.taking == 0 {
                return self.taken(changed.selection_timestamp);
            }
            return Ok(true);
        }
        // A change before one of the manager's takings, which ends it.
        if self.taking > 0 {
            return Ok(true);
        }
        self.owner = None;
        if !self.lose_selection()? {
            return Ok(false);
        }
        let previous = self.x11_owner.take();
        if changed.owner == x11rb::NONE {
            let Some(drag) = previous else {
                return Ok(true);
            };
            if changed.subtype == SelectionEvent::SELECTION_CLIENT_CLOSE
                && !bridge.end_drag_to_wayland(drag)?
            {
                return Ok(false);
            }
            self.end_x11(drag, None)?;
            return Ok(self.post(ManagerEvent::X11DragEnded(drag)));
        }
        // A new X11 drag: one in the hand before it is over, the oldest beyond the bound ends.
        if let Some(drag) = previous.filter(|&drag| {
            self.from_x11
                .iter()
                .any(|of| of.drag == drag && of.dropped.is_none())
        }) {
            self.end_x11(drag, None)?;
            if !self.post(ManagerEvent::X11DragEnded(drag)) {
                return Ok(false);
            }
        }
        while self.from_x11.len() >= MAX_DRAGS {
            let oldest = self.from_x11[0].drag;
            self.end_x11(oldest, Some((false, DragAction::None)))?;
            if !self.post(ManagerEvent::X11DragEnded(oldest)) {
                return Ok(false);
            }
        }
        let drag = self.next_from_x11;
        self.next_from_x11 += 1;
        let client = self.client_of(changed.owner);
        self.from_x11.push(FromX11 {
            drag,
            client,
            time: changed.selection_timestamp,
            source: None,
            offer: Vec::new(),
            proposed: DragAction::Copy,
            announced: false,
            answer: (false, DragAction::None),
            sent: None,
            proxy_wanted: false,
            dropped: None,
            deleting: None,
        });
        self.x11_owner = Some(drag);
        Ok(self.post(ManagerEvent::X11DragOffered { drag, client }))
    }

    /// The manager's taking of `XdndSelection` was reported, at `time`: the newest Wayland drag
    /// that asked it holds it, and speaks; an X11 drag in the hand lost it.
    fn taken(&mut self, time: Timestamp) -> Result<GoOn, Failure> {
        if let Some(drag) = self.x11_owner.take()
            && self
                .from_x11
                .iter()
                .any(|of| of.drag == drag && of.dropped.is_none())
        {
            self.end_x11(drag, None)?;
            if !self.post(ManagerEvent::X11DragEnded(drag)) {
                return Ok(false);
            }
        }
        let Some(index) = self
            .from_wayland
            .iter()
            .rposition(|of| of.taken == Taken::Asked)
        else {
            // Its drag ended before the report: the selection goes back at once.
            self.connection
                .set_selection_owner(x11rb::NONE, self.atoms.XdndSelection, time)?;
            return Ok(true);
        };
        self.from_wayland[index].taken = Taken::At(time);
        self.owner = Some((self.from_wayland[index].drag, time));
        self.advance(index)
    }

    /// Another took `XdndSelection`: the Wayland drags that held it before their drop end.
    fn lose_selection(&mut self) -> Result<GoOn, Failure> {
        let stolen: Vec<u64> = self
            .from_wayland
            .iter()
            .filter(|of| {
                matches!(of.taken, Taken::At(_)) && !matches!(of.stage, Stage::Dropped { .. })
            })
            .map(|of| of.drag)
            .collect();
        for drag in stolen {
            tracing::debug!(
                drag,
                "a Wayland drag's XdndSelection was taken by X11: it ends"
            );
            if !self.end_wayland(drag, true)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // Wayland clients' drags.

    fn wayland_index(&self, drag: u64) -> Option<usize> {
        self.from_wayland.iter().position(|of| of.drag == drag)
    }

    /// Wayland drag `drag` reached an X11 window for the first time, offering `mime_types` and
    /// proposing `action`.
    pub(super) fn start_wayland(
        &mut self,
        drag: u64,
        mime_types: &[String],
        action: DragAction,
        bridge: &mut Bridge<'_>,
    ) -> Result<GoOn, Failure> {
        if self.wayland_index(drag).is_some() {
            return Ok(true);
        }
        while self.from_wayland.len() >= MAX_DRAGS {
            let oldest = self.from_wayland[0].drag;
            if !self.end_wayland(oldest, true)? {
                return Ok(false);
            }
        }
        let targets = bridge.intern(&targets_of_offer(mime_types))?;
        self.from_wayland.push(FromWayland {
            drag,
            targets,
            action,
            taken: Taken::No,
            over: None,
            target: None,
            in_flight: false,
            kept: false,
            status: None,
            told: (false, DragAction::None),
            stage: Stage::Dragging,
        });
        Ok(true)
    }

    /// Wayland drag `drag` is over X11 window `window` at a root point, or over none: a new
    /// window is resolved to its target, once, the last one left; the point goes once the target
    /// answered the last.
    pub(super) fn wayland_over(
        &mut self,
        drag: u64,
        over: Option<(Window, (i16, i16))>,
    ) -> Result<GoOn, Failure> {
        let Some(index) = self.wayland_index(drag) else {
            return Ok(true);
        };
        if self.from_wayland[index].stage != Stage::Dragging {
            return Ok(true);
        }
        let window = over.map(|(window, _)| window);
        let current = self.from_wayland[index].over.map(|(window, _)| window);
        if current != window {
            if !self.leave_target(index)? {
                return Ok(false);
            }
            let target = match window {
                Some(window) => self.resolve(window)?,
                None => None,
            };
            self.from_wayland[index].target = target;
        }
        let of = &mut self.from_wayland[index];
        of.over = over;
        of.kept = over.is_some() && of.target.is_some();
        self.advance(index)
    }

    /// The XDND target of top-level window `window`: its `XdndProxy` when that names a window
    /// whose own `XdndProxy` names itself, else the window; none without `XdndAware` 3 or more.
    fn resolve(&self, window: Window) -> Result<Option<Target>, Failure> {
        let proxy = self.window_property(window, self.atoms.XdndProxy)?;
        let dest = match proxy {
            Some(proxy) if self.window_property(proxy, self.atoms.XdndProxy)? == Some(proxy) => {
                proxy
            }
            _ => window,
        };
        let Some(version) = self.window_property(dest, self.atoms.XdndAware)? else {
            return Ok(None);
        };
        let version = u8::try_from(version).unwrap_or(u8::MAX);
        if version < OLDEST_VERSION {
            tracing::debug!(
                window,
                version,
                "an X11 window of too old an XDND is no target"
            );
            return Ok(None);
        }
        Ok(Some(Target {
            window,
            dest,
            version: version.min(XDND_VERSION),
            client: self.client_of(dest),
            entered: false,
        }))
    }

    /// The first 32-bit value of `window`'s `property`; none when it is not set, not of 32-bit
    /// values, or the window is gone.
    fn window_property(&self, window: Window, property: Atom) -> Result<Option<u32>, Failure> {
        let reply = self
            .connection
            .get_property(false, window, property, AtomEnum::ANY, 0, 1)?
            .reply();
        let Some(reply) = refused_is_none(reply)? else {
            return Ok(None);
        };
        Ok(reply.value32().and_then(|mut values| values.next()))
    }

    /// Leaves drag `index`'s target, if it entered it: `XdndLeave`, and no status any more.
    fn leave_target(&mut self, index: usize) -> Result<GoOn, Failure> {
        let (own, leave) = (self.own, self.atoms.XdndLeave);
        let of = &mut self.from_wayland[index];
        of.in_flight = false;
        of.kept = false;
        of.status = None;
        if let Some(target) = of.target.take()
            && target.entered
        {
            self.send(target.dest, target.window, leave, leave_data(own))?;
        }
        self.tell_status(index)
    }

    /// Posts drag `index`'s status to Yuna, when it changed: not accepted without a target.
    fn tell_status(&mut self, index: usize) -> Result<GoOn, Failure> {
        let of = &mut self.from_wayland[index];
        let status = match (of.target, of.status) {
            (Some(_), Some(status)) => status,
            _ => (false, DragAction::None),
        };
        if status == of.told {
            return Ok(true);
        }
        of.told = status;
        let drag = of.drag;
        Ok(self.post(ManagerEvent::WaylandDragStatus {
            drag,
            accepted: status.0,
            action: status.1,
        }))
    }

    /// Moves drag `index` as far as it goes: takes `XdndSelection`, once it has a target; once
    /// the taking is reported, enters the target and sends the point kept, one awaiting its
    /// status at a time; released, decides once nothing is awaited.
    fn advance(&mut self, index: usize) -> Result<GoOn, Failure> {
        let Some(target) = self.from_wayland[index].target else {
            return self.decide_if_released(index);
        };
        let time = match self.from_wayland[index].taken {
            Taken::No => {
                self.from_wayland[index].taken = Taken::Asked;
                self.taking += 1;
                self.connection.set_selection_owner(
                    self.own,
                    self.atoms.XdndSelection,
                    x11rb::CURRENT_TIME,
                )?;
                return Ok(true);
            }
            Taken::Asked => return Ok(true),
            Taken::At(time) => time,
        };
        if !target.entered {
            self.enter(index, target)?;
        }
        let actions = self.actions;
        let (own, position) = (self.own, self.atoms.XdndPosition);
        let of = &mut self.from_wayland[index];
        if of.kept
            && !of.in_flight
            && let Some((_, point)) = of.over
        {
            of.kept = false;
            of.in_flight = true;
            let data = position_data(own, point, time, actions.atom(of.action));
            self.send(target.dest, target.window, position, data)?;
        }
        self.decide_if_released(index)
    }

    /// Sends drag `index`'s `XdndEnter` to `target`: its types, three inline, all in
    /// `XdndTypeList` on the manager's window when there are more; for Ask, the actions its
    /// source offers in `XdndActionList` and `XdndActionDescription` ([`listed_actions`]).
    fn enter(&mut self, index: usize, target: Target) -> Result<(), Failure> {
        let atoms = self.atoms;
        let of = &self.from_wayland[index];
        let types: Vec<Atom> = of.targets.iter().map(|&(atom, _)| atom).collect();
        if types.len() > 3 {
            self.connection.change_property32(
                PropMode::REPLACE,
                self.own,
                atoms.XdndTypeList,
                AtomEnum::ATOM,
                &types,
            )?;
        }
        if let Some((list, descriptions)) = listed_actions(self.actions, of.action) {
            self.connection.change_property32(
                PropMode::REPLACE,
                self.own,
                atoms.XdndActionList,
                AtomEnum::ATOM,
                &list,
            )?;
            self.connection.change_property8(
                PropMode::REPLACE,
                self.own,
                atoms.XdndActionDescription,
                AtomEnum::STRING,
                descriptions,
            )?;
        }
        let data = enter_data(self.own, target.version, &types);
        self.send(target.dest, target.window, atoms.XdndEnter, data)?;
        if let Some(entered) = self.from_wayland[index].target.as_mut() {
            entered.entered = true;
        }
        Ok(())
    }

    /// Drag `index`, released, drops or leaves once no position awaits its status; at once when
    /// its target never answered, as XDND asks.
    fn decide_if_released(&mut self, index: usize) -> Result<GoOn, Failure> {
        let of = &self.from_wayland[index];
        if !matches!(of.stage, Stage::Releasing { .. }) {
            return Ok(true);
        }
        if awaits_status(of.status.is_some(), of.in_flight, of.kept) {
            return Ok(true);
        }
        self.decide(index)
    }

    /// Drag `index` released: `XdndDrop` with its taking's time if the target's last status
    /// accepted, else `XdndLeave`, and the source is cancelled.
    fn decide(&mut self, index: usize) -> Result<GoOn, Failure> {
        let of = &mut self.from_wayland[index];
        let accepted = of.status.is_some_and(|(accepted, _)| accepted);
        match (of.target, of.taken) {
            (Some(target), Taken::At(time)) if accepted && target.entered => {
                of.stage = Stage::Dropped { at: Instant::now() };
                let data = drop_data(self.own, time);
                self.send(target.dest, target.window, self.atoms.XdndDrop, data)?;
                Ok(true)
            }
            _ => {
                let drag = of.drag;
                self.end_wayland(drag, true)
            }
        }
    }

    /// Wayland drag `drag` was released.
    pub(super) fn drop_wayland(&mut self, drag: u64) -> Result<GoOn, Failure> {
        let Some(index) = self.wayland_index(drag) else {
            return Ok(true);
        };
        let of = &mut self.from_wayland[index];
        if of.stage != Stage::Dragging {
            return Ok(true);
        }
        of.stage = Stage::Releasing {
            since: Instant::now(),
        };
        self.advance(index)
    }

    /// Ends Wayland drag `drag` on X11's side: `XdndLeave` to a target entered before the drop,
    /// `XdndSelection` released if it holds it, and, when `tell`, Yuna told it was not performed.
    fn end_wayland(&mut self, drag: u64, tell: bool) -> Result<GoOn, Failure> {
        let Some(index) = self.wayland_index(drag) else {
            return Ok(true);
        };
        let of = self.from_wayland.remove(index);
        if let Some(target) = of.target
            && target.entered
            && !matches!(of.stage, Stage::Dropped { .. })
        {
            let data = leave_data(self.own);
            self.send(target.dest, target.window, self.atoms.XdndLeave, data)?;
        }
        if let Some((holder, time)) = self.owner
            && holder == drag
        {
            self.owner = None;
            // At its own time, so that an X11 client that took it since keeps it.
            self.connection
                .set_selection_owner(x11rb::NONE, self.atoms.XdndSelection, time)?;
        }
        if !tell {
            return Ok(true);
        }
        Ok(self.post(ManagerEvent::WaylandDragEnded {
            drag,
            performed: None,
        }))
    }

    /// Yuna ends Wayland drag `drag`: its source cancelled, gone, or its seat closing.
    pub(super) fn cancel_wayland(&mut self, drag: u64) -> Result<GoOn, Failure> {
        self.end_wayland(drag, false)
    }

    /// The Wayland drag whose target is `window`, the window under the pointer that an answer's
    /// first value names, in the stages `wanted` accepts, newest first.
    fn wayland_with_target(&self, window: Window, wanted: impl Fn(Stage) -> bool) -> Option<usize> {
        self.from_wayland.iter().rposition(|of| {
            wanted(of.stage) && of.target.is_some_and(|target| target.window == window)
        })
    }

    /// `XdndStatus` from a target: its drag's answer, posted when it changed; the point kept
    /// goes; released, the drag decides.
    fn status(&mut self, data: Data) -> Result<GoOn, Failure> {
        let status = read_status(data);
        let not_dropped = |stage: Stage| !matches!(stage, Stage::Dropped { .. });
        let Some(index) = self.wayland_with_target(status.target, not_dropped) else {
            return Ok(true);
        };
        let action = match self.actions.action(status.action) {
            Some(action) => action,
            // Accepted without a known action: a copy, XDND's default.
            None if status.accepted => DragAction::Copy,
            None => DragAction::None,
        };
        let of = &mut self.from_wayland[index];
        of.in_flight = false;
        of.status = Some((status.accepted, action));
        if !self.tell_status(index)? {
            return Ok(false);
        }
        self.advance(index)
    }

    /// `XdndFinished` from a target after its drop: the drag performed, or not; one before the
    /// drop, or from another window, is left.
    fn finished(&mut self, data: Data) -> Result<GoOn, Failure> {
        let dropped = |stage: Stage| matches!(stage, Stage::Dropped { .. });
        let Some(index) = self.wayland_with_target(data[0], dropped) else {
            return Ok(true);
        };
        let of = &self.from_wayland[index];
        let version = of.target.map_or(XDND_VERSION, |target| target.version);
        let finished = read_finished(data, version);
        let last = of.status.map_or(DragAction::Copy, |(_, action)| action);
        let performed = finished
            .accepted
            .then(|| self.actions.action(finished.action).unwrap_or(last));
        let drag = of.drag;
        if !self.end_wayland(drag, false)? {
            return Ok(false);
        }
        Ok(self.post(ManagerEvent::WaylandDragEnded { drag, performed }))
    }

    /// A Wayland drag's request for `XdndSelection` from an X11 client: served from the drag that
    /// holds it, only to windows of its target's client; refused otherwise.
    pub(super) fn requested(
        &mut self,
        asked: &SelectionRequestEvent,
        bridge: &mut Bridge<'_>,
    ) -> Result<GoOn, Failure> {
        let requestor = self.client_of(asked.requestor);
        let served = self.owner.and_then(|(drag, time)| {
            let of = self.from_wayland.iter().find(|of| of.drag == drag)?;
            let target = of.target?;
            (target.client == requestor).then(|| (drag, time, of.targets.clone()))
        });
        let Some((drag, time, targets)) = served else {
            tracing::debug!("a request for a drag's data from no target of it: refused");
            return bridge.refuse_request(asked);
        };
        bridge.serve(asked, Some(targets), Some(time), Served::Drag(drag))
    }

    /// `window` was destroyed: a Wayland drag over it forgets it, and one dropping on it ends.
    pub(super) fn window_destroyed(&mut self, window: Window) -> Result<GoOn, Failure> {
        let gone: Vec<usize> = self
            .from_wayland
            .iter()
            .enumerate()
            .filter(|(_, of)| of.target.is_some_and(|target| target.involves(window)))
            .map(|(index, _)| index)
            .collect();
        for index in gone.into_iter().rev() {
            let of = &mut self.from_wayland[index];
            if of.stage == Stage::Dragging {
                // Gone, it is sent nothing more; the next motion finds another window.
                of.target = None;
                of.over = None;
                of.in_flight = false;
                of.kept = false;
                of.status = None;
                if !self.tell_status(index)? {
                    return Ok(false);
                }
            } else {
                let drag = of.drag;
                if let Some(target) = self.from_wayland[index].target.as_mut() {
                    target.entered = false;
                }
                if !self.end_wayland(drag, true)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Moves the drags waiting on time: a release whose status did not come leaves; a drop or a
    /// finish not answered in time ends, refused.
    pub(super) fn run(&mut self, bridge: &Bridge<'_>) -> Result<GoOn, Failure> {
        let now = Instant::now();
        let late: Vec<u64> = self
            .from_wayland
            .iter()
            .filter(|of| {
                self.wayland_deadline(of, bridge)
                    .is_some_and(|deadline| deadline <= now)
            })
            .map(|of| of.drag)
            .collect();
        for drag in late {
            tracing::debug!(drag, "a Wayland drag's X11 target did not answer in time");
            if !self.end_wayland(drag, true)? {
                return Ok(false);
            }
        }
        let late: Vec<(u64, bool)> = self
            .from_x11
            .iter()
            .filter(|of| {
                self.x11_deadline(of, bridge)
                    .is_some_and(|deadline| deadline <= now)
            })
            .map(|of| (of.drag, of.deleting.is_some()))
            .collect();
        for (drag, deleting) in late {
            tracing::debug!(drag, "an X11 drag's Wayland target did not finish in time");
            // A move whose `DELETE` went unanswered was performed all the same.
            let finished = if deleting {
                (true, DragAction::Move)
            } else {
                (false, DragAction::None)
            };
            self.end_x11(drag, Some(finished))?;
            if !self.post(ManagerEvent::X11DragEnded(drag)) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn wayland_deadline(&self, of: &FromWayland, bridge: &Bridge<'_>) -> Option<Instant> {
        match of.stage {
            Stage::Dragging => None,
            Stage::Releasing { since } => Some(since + X11_DRAG_TIMEOUT),
            Stage::Dropped { at } => {
                let moved = bridge
                    .drag_to_x11_progress(of.drag)
                    .map_or(at, |progress| progress.max(at));
                Some(moved + X11_DRAG_TIMEOUT)
            }
        }
    }

    fn x11_deadline(&self, of: &FromX11, bridge: &Bridge<'_>) -> Option<Instant> {
        if let Some((_, asked)) = of.deleting {
            return Some(asked + X11_DRAG_TIMEOUT);
        }
        let dropped = of.dropped?;
        let moved = bridge
            .drag_to_wayland_progress(of.drag)
            .map_or(dropped, |progress| progress.max(dropped));
        Some(moved + X11_DRAG_TIMEOUT)
    }

    /// When the first drag waiting on time runs out of it; none when none waits.
    pub(super) fn next_deadline(&self, bridge: &Bridge<'_>) -> Option<Instant> {
        let wayland = self
            .from_wayland
            .iter()
            .filter_map(|of| self.wayland_deadline(of, bridge));
        let x11 = self
            .from_x11
            .iter()
            .filter_map(|of| self.x11_deadline(of, bridge));
        wayland.chain(x11).min()
    }
}

/// The action an `XdndStatus` names: none when it refuses; when it accepts, its action, Copy if
/// it has none, since GTK 4 drops only on an accepted status that names an action.
fn answered_action(accepted: bool, action: DragAction) -> DragAction {
    match (accepted, action) {
        (false, _) => DragAction::None,
        (true, DragAction::None) => DragAction::Copy,
        (true, action) => action,
    }
}

/// Whether a released drag still waits for its target's status: a position in flight or kept,
/// from a target that answered before; one that never answered is left at once (XDND: "If the
/// source never received any XdndStatus messages at all, it should send XdndLeave without
/// waiting"), as is one not yet entered, which has not either.
fn awaits_status(answered: bool, in_flight: bool, kept: bool) -> bool {
    answered && (in_flight || kept)
}

/// The actions a Wayland drag proposing `proposed` lists for its X11 target, with their
/// descriptions: for Ask, which it proposes only when its source offers neither Copy nor Move,
/// Ask alone, the one its source offers; none for another action, which needs no list.
fn listed_actions(actions: Actions, proposed: DragAction) -> Option<(Vec<Atom>, &'static [u8])> {
    (proposed == DragAction::Ask).then(|| (vec![actions.ask], &b"Ask\0"[..]))
}

/// Makes the proxy, unmapped, over X11's whole root, `XdndAware` 5: before the manager listens to
/// the root, so that no `CreateNotify` makes a window of it.
pub(super) fn create_proxy(
    connection: &RustConnection,
    root: Window,
    atoms: &Atoms,
) -> Result<Window, Failure> {
    let proxy = connection.generate_id()?;
    connection.create_window(
        0,
        proxy,
        root,
        0,
        0,
        PROXY_SIDE,
        PROXY_SIDE,
        0,
        x11rb::protocol::xproto::WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &x11rb::protocol::xproto::CreateWindowAux::new().override_redirect(1),
    )?;
    connection.change_property32(
        PropMode::REPLACE,
        proxy,
        atoms.XdndAware,
        AtomEnum::ATOM,
        &[u32::from(XDND_VERSION)],
    )?;
    Ok(proxy)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIONS: Actions = Actions {
        copy: 101,
        move_: 102,
        link: 103,
        ask: 104,
        private: 105,
    };

    /// R4: versions 3 to 5 are taken; 2, below XDND's floor, and 6, above Yuna's, are ignored.
    #[test]
    fn an_enter_is_read_only_at_versions_3_to_5() {
        let data = |version: u32| [0x0040_0001, version << 24, 1, 2, 0];
        assert_eq!(read_enter(data(2)), None);
        assert_eq!(read_enter(data(6)), None);
        for version in 3..=5 {
            assert_eq!(
                read_enter(data(version)),
                Some(Enter {
                    source: 0x0040_0001,
                    version: version as u8,
                    type_list: false,
                    types: [1, 2, 0]
                })
            );
        }
        let listed = read_enter([9, (5 << 24) | 1, 1, 2, 3]);
        assert!(listed.is_some_and(|enter| enter.type_list));
    }

    /// R13: three types inline, the rest in `XdndTypeList`, which bit 0 announces.
    #[test]
    fn an_enter_carries_three_types_and_says_when_there_are_more() {
        assert_eq!(enter_data(7, 5, &[1, 2]), [7, 5 << 24, 1, 2, 0]);
        assert_eq!(enter_data(7, 4, &[1, 2, 3, 4]), [7, (4 << 24) | 1, 1, 2, 3]);
        assert_eq!(enter_data(7, 3, &[]), [7, 3 << 24, 0, 0, 0]);
        let back = read_enter(enter_data(7, 5, &[1, 2, 3, 4]));
        assert_eq!(
            back,
            Some(Enter {
                source: 7,
                version: 5,
                type_list: true,
                types: [1, 2, 3]
            })
        );
    }

    /// R5, F8: a status always sets bit 1 with an empty rectangle; accepted sets bit 0 and the
    /// action, refused names none.
    #[test]
    fn a_status_asks_every_position_with_an_empty_rectangle() {
        assert_eq!(status_data(9, true, 101), [9, 3, 0, 0, 101]);
        assert_eq!(status_data(9, false, 101), [9, 2, 0, 0, 0]);
        let read = read_status(status_data(9, true, 102));
        assert_eq!(
            read,
            Status {
                target: 9,
                accepted: true,
                action: 102
            }
        );
        assert!(!read_status([9, 2, 0, 0, 101]).accepted);
    }

    /// A position packs its root point, x high and y low, its sign bits kept.
    #[test]
    fn a_position_packs_its_point_and_names_its_time() {
        assert_eq!(
            position_data(7, (300, 200), 55, 101),
            [7, 0, (300 << 16) | 200, 55, 101]
        );
        // A point left of or above the root keeps its 16-bit patterns.
        assert_eq!(packed((-1, -2)), 0xFFFF_FFFE);
        assert_eq!(packed((i16::MAX, 0)), 0x7FFF_0000);
    }

    /// R9, F13: a message is read with its first value, the window a target checks against the
    /// peer it speaks to.
    #[test]
    fn each_message_names_its_peer_first() {
        assert_eq!(read_position([7, 0, 0, 55, 101]).source, 7);
        assert_eq!(read_position([7, 0, 0, 55, 101]).time, 55);
        assert_eq!(leave_data(7)[0], 7);
        assert_eq!(drop_data(7, 55), [7, 0, 55, 0, 0]);
        assert_eq!(read_finished([9, 1, 102, 0, 0], 5).target, 9);
    }

    /// R16, F12: a v5 target says whether it accepted and its action; before v5 it accepted,
    /// its action unsaid; Yuna sends a v3 or v4 source no flags.
    #[test]
    fn finished_is_read_and_written_by_version() {
        assert_eq!(
            read_finished([9, 0, 0, 0, 0], 5),
            Finished {
                target: 9,
                accepted: false,
                action: 0
            }
        );
        assert_eq!(
            read_finished([9, 0, 0, 0, 0], 4),
            Finished {
                target: 9,
                accepted: true,
                action: 0
            }
        );
        assert_eq!(finished_data(9, 5, true, 102), [9, 1, 102, 0, 0]);
        assert_eq!(finished_data(9, 5, false, 102), [9, 0, 0, 0, 0]);
        assert_eq!(finished_data(9, 3, true, 102), [9, 0, 0, 0, 0]);
    }

    /// An accepted status always names an action, Copy when the answer has none; a refusal
    /// names none.
    #[test]
    fn an_accepted_status_always_names_an_action() {
        assert_eq!(answered_action(true, DragAction::None), DragAction::Copy);
        assert_eq!(answered_action(true, DragAction::Move), DragAction::Move);
        assert_eq!(answered_action(false, DragAction::Move), DragAction::None);
        let accepted = status_data(
            9,
            true,
            ACTIONS.atom(answered_action(true, DragAction::None)),
        );
        assert_eq!(accepted[4], ACTIONS.copy);
    }

    /// L2: released, a drag waits for the status of a position in flight or kept only from a
    /// target that answered before; one that never answered, or not yet entered, is left at
    /// once.
    #[test]
    fn a_target_that_never_answered_is_not_waited_for() {
        assert!(awaits_status(true, true, false));
        assert!(awaits_status(true, false, true));
        assert!(!awaits_status(true, false, false), "nothing awaited");
        assert!(!awaits_status(false, true, false), "never answered");
        assert!(!awaits_status(false, false, true));
    }

    /// L3: Ask lists the one action its source offers; no other action lists any.
    #[test]
    fn ask_lists_only_what_the_source_offers() {
        assert_eq!(
            listed_actions(ACTIONS, DragAction::Ask),
            Some((vec![104], &b"Ask\0"[..]))
        );
        for action in [DragAction::Copy, DragAction::Move, DragAction::None] {
            assert_eq!(listed_actions(ACTIONS, action), None);
        }
    }

    /// L1: a dropped X11 drag that Yuna forgets, its time out before the drop reached it, is
    /// refused with `XdndFinished`; one that never dropped is sent nothing.
    #[test]
    fn a_dropped_drag_forgotten_is_refused() {
        let drag = |dropped: Option<Instant>| FromX11 {
            drag: 1,
            client: 0x0040_0000,
            time: 0,
            source: Some((0x0040_0001, 5)),
            offer: Vec::new(),
            proposed: DragAction::Copy,
            announced: false,
            answer: (false, DragAction::None),
            sent: None,
            proxy_wanted: false,
            dropped,
            deleting: None,
        };
        assert_eq!(
            drag(Some(Instant::now())).finished_when_forgotten(),
            Some((false, DragAction::None))
        );
        assert_eq!(drag(None).finished_when_forgotten(), None);
    }

    /// R14, F11: Copy, Move and Ask map both ways; a target's Link or Private reads as Copy;
    /// none and unknown actions read as none.
    #[test]
    fn actions_map_both_ways_link_and_private_reading_as_copy() {
        for action in [DragAction::Copy, DragAction::Move, DragAction::Ask] {
            assert_eq!(ACTIONS.action(ACTIONS.atom(action)), Some(action));
        }
        assert_eq!(ACTIONS.atom(DragAction::None), x11rb::NONE);
        assert_eq!(ACTIONS.action(103), Some(DragAction::Copy));
        assert_eq!(ACTIONS.action(105), Some(DragAction::Copy));
        assert_eq!(ACTIONS.action(x11rb::NONE), None);
        assert_eq!(ACTIONS.action(999), None);
    }
}
