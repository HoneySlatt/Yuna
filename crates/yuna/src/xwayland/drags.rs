//! Drag and drop between a seat's X11 programs and its Wayland programs, on Yuna's side: the thin
//! glue between each seat's drags, whose decisions are `drag_session.rs`'s, Smithay's data device
//! and the seat's X11 manager, which speaks XDND (`xdnd.rs`). ARCHITECTURE tells the flow.
//!
//! What it feeds the decisions: what a drag is over at each motion of the pointer
//! ([`State::drag_moved`], from `input.rs`), the pointer's release, the manager's events, and
//! the Wayland side's answers. What it carries out: Smithay's public data device on a Wayland
//! target (`DndFocus` for a surface: enter, motion, leave, drop), the manager's commands, events
//! to a Wayland source. It never waits, and keeps nothing once a drag ends.
//!
//! **An X11 program's drag** offers its Wayland targets an [`X11DragSource`]: the MIME types its
//! `XdndEnter` gave, and the action its positions propose, with Copy, which XDND lets any target
//! fall back on. What a target answers or reads Smithay notes in it while it dispatches the
//! clients; at the round's end, the answers go to the decisions, and each read to the manager
//! with its pipe, among the [`MAX_X11_TRANSFERS`] each way that the clipboard's transfers count
//! too.
//!
//! **A Wayland client's drag** keeps Smithay's drag grab, given a [`WaylandDragSource`] that
//! passes everything to the client's source but a cancel asked while the grab lives: Smithay
//! cancels a source released over no Wayland target, an X11 window among them, then says the
//! grab dropped or was cancelled, and the decisions resolve it. The manager is told the drag's
//! latest point once per round at most, so that a burst of motions sends one.

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use smithay::input::Seat;
use smithay::input::dnd::{DndAction, DndFocus, OfferData, Source, SourceMetadata};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_data_source::{self, WlDataSource};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Point, SERIAL_COUNTER};
use smithay::wayland::selection::data_device::WlOfferData;
use yuna_layout::SeatId;

use super::bridge::MAX_X11_TRANSFERS;
use super::drag_session::{DragAction, DragKey, Effect, Pick, Pressed, SeatDrags};
use super::manager::{Command, ManagerEvent};
use crate::clock::Deadline;
use crate::input::Hit;
use crate::selection::{for_x11, reader_pipe};
use crate::state::State;

/// Locks `mutex`, which only Yuna's thread holds: a panic while it was held is fatal to Yuna
/// already (ADR-0080), so a poisoned lock is read as it is.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a drag's action is in Smithay's terms.
fn dnd_action(action: DragAction) -> DndAction {
    match action {
        DragAction::None => DndAction::None,
        DragAction::Copy => DndAction::Copy,
        DragAction::Move => DndAction::Move,
        DragAction::Ask => DndAction::Ask,
    }
}

fn drag_action(action: DndAction) -> DragAction {
    match action {
        DndAction::None => DragAction::None,
        DndAction::Copy => DragAction::Copy,
        DndAction::Move => DragAction::Move,
        DndAction::Ask => DragAction::Ask,
    }
}

/// The action a Wayland source's drag proposes to X11: Copy if it offers it, else Move, else
/// Ask; Copy for a source that offers none, as Wayland reads one older than actions.
fn proposed_action(offered: &[DndAction]) -> DragAction {
    [DndAction::Copy, DndAction::Move, DndAction::Ask]
        .into_iter()
        .find(|action| offered.contains(action))
        .map_or(DragAction::Copy, drag_action)
}

/// An X11 program's drag, as Smithay's data device offers it to a Wayland target.
#[derive(Debug, Default)]
pub(crate) struct X11DragSource {
    state: Mutex<X11DragState>,
}

#[derive(Debug, Default)]
struct X11DragState {
    mime_types: Vec<String>,
    actions: Vec<DndAction>,
    /// The type the target last accepted, among those offered.
    accepted: Option<String>,
    /// The action the target last chose.
    action: DndAction,
    /// The reads asked since the round began, each with its pipe; [`MAX_X11_TRANSFERS`] at most.
    reads: Vec<(String, OwnedFd)>,
    finished: bool,
    cancelled: bool,
    /// True once the drag ended: a read is then refused, its pipe closed.
    ended: bool,
}

/// What a Wayland target of an X11 drag said since the last round.
struct TargetNews {
    reads: Vec<(String, OwnedFd)>,
    /// Whether it accepted a type and an action, and which.
    answer: (bool, DragAction),
    finished: bool,
    cancelled: bool,
}

impl X11DragSource {
    fn new() -> X11DragSource {
        let source = X11DragSource::default();
        source.propose(DragAction::Copy);
        source
    }

    /// Its source's types, as its `XdndEnter` gave them.
    fn offer(&self, mime_types: Vec<String>) {
        lock(&self.state).mime_types = mime_types;
    }

    /// Its source's action changed: offered with Copy, which XDND lets any target fall back on.
    fn propose(&self, action: DragAction) {
        let mut state = lock(&self.state);
        state.actions = vec![dnd_action(action)];
        if action != DragAction::Copy {
            state.actions.push(DndAction::Copy);
        }
    }

    /// A new target is entered: it has answered nothing yet.
    fn forget_answer(&self) {
        let mut state = lock(&self.state);
        state.accepted = None;
        state.action = DndAction::None;
    }

    /// What the target said since the last call; `validated` when Smithay's offer is.
    fn news(&self, validated: bool) -> TargetNews {
        let mut state = lock(&self.state);
        let accepted = validated && state.accepted.is_some();
        let action = match drag_action(state.action) {
            // An offer older than actions is validated without one: Wayland reads it as a copy.
            DragAction::None => DragAction::Copy,
            action => action,
        };
        TargetNews {
            reads: std::mem::take(&mut state.reads),
            answer: (accepted, action),
            finished: std::mem::take(&mut state.finished),
            cancelled: std::mem::take(&mut state.cancelled),
        }
    }

    /// Whether the target accepted a type, once Smithay's offer is `validated`: the drop goes.
    fn accepts(&self, validated: bool) -> bool {
        validated && lock(&self.state).accepted.is_some()
    }

    /// The drag ended: reads are refused from now on, and those waiting are dropped, their
    /// pipes closed, so that their readers see them end.
    fn end(&self) {
        let mut state = lock(&self.state);
        state.ended = true;
        state.reads.clear();
    }
}

impl IsAlive for X11DragSource {
    fn alive(&self) -> bool {
        !lock(&self.state).ended
    }
}

impl Source for X11DragSource {
    fn metadata(&self) -> Option<SourceMetadata> {
        let state = lock(&self.state);
        let mut metadata = SourceMetadata {
            mime_types: state.mime_types.clone(),
            ..SourceMetadata::default()
        };
        for &action in &state.actions {
            metadata.dnd_actions.push(action);
        }
        Some(metadata)
    }

    fn accepted(&self, mime_type: Option<String>) {
        let mut state = lock(&self.state);
        let offered = mime_type.filter(|mime_type| state.mime_types.contains(mime_type));
        state.accepted = offered;
    }

    fn choose_action(&self, action: DndAction) {
        lock(&self.state).action = action;
    }

    /// A read waits for the round's end; past [`MAX_X11_TRANSFERS`] waiting, or once the drag
    /// ended, its pipe closes at once, and its reader sees it end.
    fn send(&self, mime_type: &str, fd: OwnedFd) {
        let mut state = lock(&self.state);
        if !state.ended && state.reads.len() < MAX_X11_TRANSFERS {
            state.reads.push((mime_type.to_owned(), fd));
        }
    }

    fn drop_performed(&self) {}

    fn cancel(&self) {
        lock(&self.state).cancelled = true;
    }

    fn finished(&self) {
        lock(&self.state).finished = true;
    }
}

/// A Wayland client's drag source, as Smithay's drag grab holds it: everything goes to the
/// client's own, but a cancel asked while the grab lives, which waits for the grab's end.
pub(crate) struct WaylandDragSource {
    inner: Arc<dyn Source>,
    grab: Arc<Mutex<GrabState>>,
}

/// Whether Smithay's grab of a Wayland drag lives, and whether it asked a cancel meanwhile.
#[derive(Debug)]
struct GrabState {
    live: bool,
    cancel_asked: bool,
}

impl fmt::Debug for WaylandDragSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WaylandDragSource")
            .field("grab", &self.grab)
            .finish_non_exhaustive()
    }
}

impl IsAlive for WaylandDragSource {
    fn alive(&self) -> bool {
        self.inner.alive()
    }
}

impl Source for WaylandDragSource {
    fn is_client_local(&self, target: &dyn Any) -> bool {
        self.inner.is_client_local(target)
    }

    fn metadata(&self) -> Option<SourceMetadata> {
        self.inner.metadata()
    }

    fn accepted(&self, mime_type: Option<String>) {
        self.inner.accepted(mime_type);
    }

    fn choose_action(&self, action: DndAction) {
        self.inner.choose_action(action);
    }

    fn send(&self, mime_type: &str, fd: OwnedFd) {
        self.inner.send(mime_type, fd);
    }

    fn drop_performed(&self) {
        self.inner.drop_performed();
    }

    fn cancel(&self) {
        let mut grab = lock(&self.grab);
        if grab.live {
            grab.cancel_asked = true;
        } else {
            drop(grab);
            self.inner.cancel();
        }
    }

    fn finished(&self) {
        self.inner.finished();
    }
}

/// Every seat's drags, and what the glue holds for each.
#[derive(Debug, Default)]
pub(crate) struct Drags {
    seats: HashMap<SeatId, SeatDrags<WlSurface>>,
    from_x11: HashMap<DragKey, FromX11>,
    from_wayland: HashMap<u64, FromWayland>,
    /// The next Wayland drag's number: numbers increase and are never reused.
    next_wayland: u64,
    /// Where each seat's pointer was at its last motion during a drag.
    pointers: HashMap<SeatId, PointerAt>,
    /// Smithay's drag grabs of each seat's Wayland drags, oldest first, until each ends: a
    /// second drag started in one grab makes Smithay end the first grab, the oldest, so that a
    /// grab's end names its own drag, even one whose decisions ended it already.
    grabs: HashMap<SeatId, VecDeque<Grab>>,
}

/// Smithay's drag grab of a Wayland drag, while it runs.
struct Grab {
    drag: u64,
    source: Arc<dyn Source>,
    state: Arc<Mutex<GrabState>>,
}

impl fmt::Debug for Grab {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Grab")
            .field("drag", &self.drag)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// Where a seat's pointer was at its last motion during a drag, and the origin of the Wayland
/// surface under it, if one was.
#[derive(Debug, Clone, Copy)]
struct PointerAt {
    location: Point<f64, Logical>,
    origin: Option<Point<f64, Logical>>,
}

/// A Wayland drag's latest point for an Xwayland's manager: the X11 window it is over and the
/// root point, or none.
#[derive(Debug, Clone, Copy)]
struct Pending {
    server: u64,
    over: Option<(u32, (i16, i16))>,
}

/// What Smithay's data device is called with for an X11 drag's Wayland target: the seat, the
/// drag's source, and its offer, taken out meanwhile.
struct TargetCall {
    seat: Seat<State>,
    source: Arc<X11DragSource>,
    offer: Option<WlOfferData<X11DragSource>>,
}

/// What the glue holds for an X11 program's drag.
#[derive(Debug)]
struct FromX11 {
    seat: SeatId,
    source: Arc<X11DragSource>,
    /// Smithay's offer to the Wayland target entered.
    offer: Option<WlOfferData<X11DragSource>>,
}

/// What the glue holds for a Wayland client's drag.
struct FromWayland {
    seat: SeatId,
    inner: Arc<dyn Source>,
    /// The client's source, for the X11 target's answers: none for a drag within one client.
    data_source: Option<WlDataSource>,
    grab: Arc<Mutex<GrabState>>,
    /// Its MIME types as X11 is offered them, and the first it offers, which an X11 target's
    /// acceptance names.
    mime_types: Vec<String>,
    first_mime_type: Option<String>,
    action: DragAction,
    /// The Xwayland whose manager was told of it.
    started_on: Option<u64>,
    /// The latest point to tell.
    pending: Option<Pending>,
}

impl fmt::Debug for FromWayland {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FromWayland")
            .field("started_on", &self.started_on)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

impl State {
    fn seat_drags(&mut self, seat: SeatId) -> &mut SeatDrags<WlSurface> {
        self.x11.drags.seats.entry(seat).or_default()
    }

    /// What a drag is over at `location` on `seat` (`drag_session.rs`): the surface under it in
    /// a Wayland window, with that surface's origin, or the X11 window under it, with the root
    /// point, among the layout's windows shown: never an X11 menu, tooltip or drag icon.
    fn drag_pick(
        &self,
        seat: SeatId,
        location: Point<f64, Logical>,
    ) -> (Pick<WlSurface>, Option<Point<f64, Logical>>) {
        match self.hit_test(seat, location, false) {
            None => (Pick::Nothing, None),
            Some(Hit {
                x11: Some(x11),
                origin,
                ..
            }) => {
                let point =
                    self.x11
                        .root_point_in(&x11, (location.x, location.y), (origin.x, origin.y));
                let pick = Pick::X11 {
                    server: x11.server,
                    window: x11.window,
                    point,
                };
                (pick, None)
            }
            Some(Hit {
                surface,
                origin,
                x11: None,
            }) => (Pick::Wayland(surface), Some(origin)),
        }
    }

    /// `seat`'s pointer moves to `location`, over `under`, as Yuna's motion path finds it:
    /// while a drag is in the hand, what it is over goes to its decisions. Returns the focus the
    /// pointer's motion goes to: for a Wayland client's drag, the Wayland surface it is over, or
    /// none, so that Smithay's grab leaves any Wayland target over an X11 window; else `under`.
    pub(crate) fn drag_moved(
        &mut self,
        seat: SeatId,
        location: Point<f64, Logical>,
        under: Option<(WlSurface, Point<f64, Logical>)>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let follows = self
            .x11
            .drags
            .seats
            .get(&seat)
            .is_some_and(SeatDrags::follows_motion);
        if !follows {
            return under;
        }
        let (pick, origin) = self.drag_pick(seat, location);
        self.x11
            .drags
            .pointers
            .insert(seat, PointerAt { location, origin });
        let (focus, effects) = self.seat_drags(seat).moved(pick);
        self.apply_drag_effects(seat, effects);
        match focus {
            Some(focus) => focus.zip(origin),
            None => under,
        }
    }

    /// `seat`'s pointer holds no button any more.
    pub(crate) fn drag_released(&mut self, seat: SeatId) {
        let now = self.clock.now();
        let Some(drags) = self.x11.drags.seats.get_mut(&seat) else {
            return;
        };
        let effects = drags.released(now);
        self.apply_drag_effects(seat, effects);
    }

    /// A Wayland client of `seat` starts a drag with `source`, which Smithay's grab is given
    /// wrapped.
    pub(crate) fn wayland_drag_started<S: Source>(
        &mut self,
        seat: SeatId,
        source: S,
    ) -> WaylandDragSource {
        let data_source = (&source as &dyn Any)
            .downcast_ref::<WlDataSource>()
            .cloned();
        let metadata = source.metadata();
        let data = metadata.is_some();
        let (mime_types, action) = metadata.map_or_else(
            || (Vec::new(), DragAction::Copy),
            |metadata| {
                let action = proposed_action(&metadata.dnd_actions);
                (metadata.mime_types, action)
            },
        );
        let inner: Arc<dyn Source> = Arc::new(source);
        let grab = Arc::new(Mutex::new(GrabState {
            live: true,
            cancel_asked: false,
        }));
        let drags = &mut self.x11.drags;
        drags.next_wayland += 1;
        let drag = drags.next_wayland;
        drags.from_wayland.insert(
            drag,
            FromWayland {
                seat,
                inner: inner.clone(),
                data_source,
                grab: grab.clone(),
                first_mime_type: mime_types.first().cloned(),
                mime_types: for_x11(mime_types),
                action,
                started_on: None,
                pending: None,
            },
        );
        drags.grabs.entry(seat).or_default().push_back(Grab {
            drag,
            source: inner.clone(),
            state: grab.clone(),
        });
        let effects = self.seat_drags(seat).wayland_started(drag, data);
        self.apply_drag_effects(seat, effects);
        WaylandDragSource { inner, grab }
    }

    /// Smithay's oldest drag grab of `seat` ends, released or cancelled: the cancel it asked
    /// meanwhile goes to the decisions of its drag, and cancels from now on go to the source at
    /// once. A drag the decisions ended already gets the cancel itself.
    pub(crate) fn wayland_drag_ended(&mut self, seat: SeatId, released: bool) {
        let Some(grab) = self
            .x11
            .drags
            .grabs
            .get_mut(&seat)
            .and_then(VecDeque::pop_front)
        else {
            return;
        };
        let cancel_asked = {
            let mut state = lock(&grab.state);
            state.live = false;
            std::mem::take(&mut state.cancel_asked)
        };
        let drags = self.seat_drags(seat);
        if !drags.holds_wayland(grab.drag) {
            if cancel_asked {
                grab.source.cancel();
            }
            return;
        }
        let effects = if released {
            drags.grab_dropped(grab.drag, cancel_asked)
        } else {
            drags.grab_cancelled(grab.drag, cancel_asked)
        };
        self.apply_drag_effects(seat, effects);
    }

    /// The X11 window `seat`'s pointer holds a button on, if it holds one on an X11 window: the
    /// surface its grab started on, paired with a window of an Xwayland.
    fn pressed_x11(&self, seat: SeatId) -> Option<Pressed> {
        let data = self.seat(seat)?;
        if data.pressed_buttons.is_empty() {
            return None;
        }
        let (surface, _) = data.pointer.as_ref()?.grab_start_data()?.focus?;
        let (server, window) = self.x11_window_of_surface(&surface)?;
        let client = self.x11.servers.get(&server)?.windows.get(&window)?.client;
        Some(Pressed { server, client })
    }

    /// What Xwayland `server`'s manager posted of a drag.
    pub(super) fn x11_drag_event(&mut self, server: u64, event: ManagerEvent) {
        let Some(seat) = self.x11.servers.get(&server).map(|record| record.seat) else {
            return;
        };
        let effects = match event {
            ManagerEvent::X11DragOffered { drag, client } => {
                self.x11_drag_offered(seat, server, drag, client)
            }
            ManagerEvent::X11DragEntered {
                drag,
                mime_types,
                action,
            } => {
                let key = DragKey::FromX11 { server, drag };
                if let Some(glue) = self.x11.drags.from_x11.get(&key) {
                    glue.source.propose(action);
                    glue.source.offer(mime_types);
                }
                self.seat_drags(seat).x11_entered(server, drag)
            }
            ManagerEvent::X11DragLeft(drag) => self.seat_drags(seat).x11_left(server, drag),
            ManagerEvent::X11DragDropped(drag) => {
                let key = DragKey::FromX11 { server, drag };
                let validated = self.x11.drags.from_x11.get(&key).is_some_and(|glue| {
                    let validated = glue.offer.as_ref().is_some_and(OfferData::validated);
                    glue.source.accepts(validated)
                });
                self.seat_drags(seat).x11_dropped(server, drag, validated)
            }
            ManagerEvent::X11DragAction { drag, action } => {
                let key = DragKey::FromX11 { server, drag };
                if let Some(glue) = self.x11.drags.from_x11.get(&key) {
                    glue.source.propose(action);
                }
                Vec::new()
            }
            ManagerEvent::X11DragEnded(drag) => self.seat_drags(seat).x11_ended(server, drag),
            ManagerEvent::DragProxyMapped(drag) => self.seat_drags(seat).proxy_mapped(server, drag),
            ManagerEvent::WaylandDragStatus {
                drag,
                accepted,
                action,
            } => self
                .seat_drags(seat)
                .wayland_status(server, drag, accepted, action),
            ManagerEvent::ReadWaylandDrag {
                drag,
                mime_type,
                transfer,
            } => {
                let source = self.read_wayland_drag(drag, &mime_type);
                self.x11
                    .send(server, Command::WaylandData { transfer, source });
                Vec::new()
            }
            ManagerEvent::WaylandDragEnded { drag, performed } => {
                self.seat_drags(seat).wayland_ended(server, drag, performed)
            }
            _ => Vec::new(),
        };
        self.apply_drag_effects(seat, effects);
    }

    /// An X11 client of Xwayland `server` took `XdndSelection`, its `drag`-th taking: a drag
    /// starts if `seat`'s pointer holds a button on a window of that client.
    fn x11_drag_offered(
        &mut self,
        seat: SeatId,
        server: u64,
        drag: u64,
        client: u32,
    ) -> Vec<Effect<WlSurface>> {
        let pressed = self.pressed_x11(seat);
        let Some(location) = self
            .seat(seat)
            .and_then(|data| data.pointer.as_ref())
            .map(|pointer| pointer.current_location())
        else {
            return Vec::new();
        };
        let (pick, origin) = self.drag_pick(seat, location);
        let Some(effects) = self
            .seat_drags(seat)
            .x11_offered(server, drag, client, pressed, pick)
        else {
            return Vec::new();
        };
        self.x11
            .drags
            .pointers
            .insert(seat, PointerAt { location, origin });
        self.x11.drags.from_x11.insert(
            DragKey::FromX11 { server, drag },
            FromX11 {
                seat,
                source: Arc::new(X11DragSource::new()),
                offer: None,
            },
        );
        effects
    }

    /// The read end of a pipe Wayland drag `drag`'s source writes `mime_type` into, for its X11
    /// target; none when the drag or its source is gone, or it lacks the type.
    fn read_wayland_drag(&self, drag: u64, mime_type: &str) -> Option<OwnedFd> {
        let glue = self.x11.drags.from_wayland.get(&drag)?;
        let offered = glue.inner.alive()
            && glue
                .inner
                .metadata()
                .is_some_and(|metadata| metadata.mime_types.iter().any(|m| m == mime_type));
        if !offered {
            return None;
        }
        let (read, write) = reader_pipe()?;
        glue.inner.send(mime_type, write);
        Some(read)
    }

    /// At the round's end: what the Wayland targets of X11 drags answered and asked goes to the
    /// decisions and the managers, and each Wayland drag's latest point to its manager.
    pub(super) fn x11_drag_commands(&mut self) {
        let keys: Vec<(DragKey, SeatId)> = self
            .x11
            .drags
            .from_x11
            .iter()
            .map(|(&key, glue)| (key, glue.seat))
            .collect();
        for (key, seat) in keys {
            let Some(glue) = self.x11.drags.from_x11.get(&key) else {
                continue;
            };
            let validated = glue.offer.as_ref().is_some_and(OfferData::validated);
            let news = glue.source.news(validated);
            for (mime_type, pipe) in news.reads {
                self.x11_send_drag(key, mime_type, pipe);
            }
            let drags = self.seat_drags(seat);
            let (accepted, action) = news.answer;
            let mut effects = drags.target_answered(key, accepted, action);
            if news.finished {
                effects.extend(drags.target_finished(key, action));
            } else if news.cancelled {
                effects.extend(drags.target_cancelled(key));
            }
            self.apply_drag_effects(seat, effects);
        }
        // A source destroyed, or its client gone, tells nothing: it is found here, while its
        // drag lives.
        let gone: Vec<(u64, SeatId)> = self
            .x11
            .drags
            .from_wayland
            .iter()
            .filter(|(_, glue)| !glue.inner.alive())
            .map(|(&drag, glue)| (drag, glue.seat))
            .collect();
        for (drag, seat) in gone {
            let effects = self.seat_drags(seat).wayland_source_gone(drag);
            self.apply_drag_effects(seat, effects);
        }
        let pending: Vec<u64> = self
            .x11
            .drags
            .from_wayland
            .iter()
            .filter(|(_, glue)| glue.pending.is_some())
            .map(|(&drag, _)| drag)
            .collect();
        for drag in pending {
            self.flush_drag_point(drag);
        }
    }

    /// A Wayland target reads X11 drag `key` as `mime_type` into `pipe`: the pipe goes to its
    /// manager, unless its Xwayland is gone or [`MAX_X11_TRANSFERS`] run already; then it
    /// closes, and the reader sees it end.
    fn x11_send_drag(&mut self, key: DragKey, mime_type: String, pipe: OwnedFd) {
        let DragKey::FromX11 { server, drag } = key else {
            return;
        };
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        if record.transfers_to_wayland >= MAX_X11_TRANSFERS {
            tracing::debug!(
                seat = record.seat.0,
                "a Wayland target's read of an X11 drag is refused: too many transfers run"
            );
            return;
        }
        record.transfers_to_wayland += 1;
        self.x11.send(
            server,
            Command::SendX11Drag {
                drag,
                mime_type,
                pipe,
            },
        );
    }

    /// Tells Wayland drag `drag`'s manager its latest point, if one waits: its MIME types first,
    /// the first time.
    fn flush_drag_point(&mut self, drag: u64) {
        let Some(glue) = self.x11.drags.from_wayland.get_mut(&drag) else {
            return;
        };
        let Some(Pending { server, over }) = glue.pending.take() else {
            return;
        };
        if glue.started_on != Some(server) {
            // Leaving an X11 window its manager never heard of says nothing.
            if over.is_none() {
                return;
            }
            glue.started_on = Some(server);
            let start = Command::StartWaylandDrag {
                drag,
                mime_types: glue.mime_types.clone(),
                action: glue.action,
            };
            self.x11.send(server, start);
        }
        self.x11
            .send(server, Command::WaylandDragOver { drag, over });
    }

    /// Carries out `effects`, decided for `seat`'s drags, in order; then forgets the seats left
    /// without a drag, so that nothing is kept at rest, and arms the clock for the first drag
    /// that waits on it.
    fn apply_drag_effects(&mut self, seat: SeatId, effects: Vec<Effect<WlSurface>>) {
        for effect in effects {
            self.apply_drag_effect(seat, effect);
        }
        let drags = &mut self.x11.drags;
        drags.seats.retain(|_, seat_drags| !seat_drags.is_empty());
        drags
            .pointers
            .retain(|seat, _| drags.seats.contains_key(seat));
        self.arm_drags_deadline();
    }

    fn apply_drag_effect(&mut self, seat: SeatId, effect: Effect<WlSurface>) {
        match effect {
            Effect::ShowProxy {
                server,
                drag,
                shown,
            } => self
                .x11
                .send(server, Command::ShowDragProxy { drag, shown }),
            Effect::Answer {
                server,
                drag,
                accepted,
                action,
            } => self.x11.send(
                server,
                Command::AnswerX11Drag {
                    drag,
                    accepted,
                    action,
                },
            ),
            Effect::Finish {
                server,
                drag,
                accepted,
                action,
            } => self.x11.send(
                server,
                Command::FinishX11Drag {
                    drag,
                    accepted,
                    action,
                },
            ),
            Effect::Forget { server, drag } => {
                self.x11.send(server, Command::ForgetX11Drag(drag));
            }
            Effect::Enter { key, target } => self.enter_drag_target(seat, key, &target),
            Effect::Motion { key, target } => self.move_over_drag_target(seat, key, &target),
            Effect::Leave { key, target } => self.leave_drag_target(seat, key, &target),
            Effect::Drop { key, target } => self.drop_on_drag_target(seat, key, &target),
            Effect::RepeatMotion => self.repeat_pointer_motion(seat),
            Effect::Over { drag, server, over } => {
                if let Some(glue) = self.x11.drags.from_wayland.get_mut(&drag) {
                    glue.pending = Some(Pending { server, over });
                }
            }
            Effect::DropOnX11 { drag, server } => {
                self.flush_drag_point(drag);
                self.x11.send(server, Command::DropWaylandDrag(drag));
            }
            Effect::CancelX11 { drag, server } => {
                if let Some(glue) = self.x11.drags.from_wayland.get_mut(&drag) {
                    glue.pending = None;
                    if glue.started_on.take() == Some(server) {
                        self.x11.send(server, Command::CancelWaylandDrag(drag));
                    }
                }
            }
            Effect::Status {
                drag,
                accepted,
                action,
            } => self.tell_wayland_source(drag, accepted, action),
            Effect::Cancel { drag } => {
                if let Some(glue) = self.x11.drags.from_wayland.get(&drag) {
                    glue.inner.cancel();
                }
            }
            Effect::Performed { drag, action } => {
                if let Some(glue) = self.x11.drags.from_wayland.get(&drag) {
                    tell_action(glue.data_source.as_ref(), action);
                    glue.inner.drop_performed();
                    glue.inner.finished();
                }
            }
            Effect::Ended(key) => self.forget_drag(key),
        }
    }

    /// The X11 target's answer to Wayland drag `drag` goes to its source: `target` names the
    /// first type it offers, or none, and `action` the target's.
    fn tell_wayland_source(&self, drag: u64, accepted: bool, action: DragAction) {
        let Some(glue) = self.x11.drags.from_wayland.get(&drag) else {
            return;
        };
        let Some(data_source) = glue.data_source.as_ref().filter(|source| source.alive()) else {
            return;
        };
        let mime_type = glue.first_mime_type.clone().filter(|_| accepted);
        data_source.target(mime_type);
        tell_action(Some(data_source), action);
    }

    /// The glue forgets drag `key`, ended: an X11 drag's offer refuses reads from now on, and a
    /// Wayland drag's source takes cancels at once.
    fn forget_drag(&mut self, key: DragKey) {
        match key {
            DragKey::FromX11 { .. } => {
                if let Some(glue) = self.x11.drags.from_x11.remove(&key) {
                    glue.source.end();
                }
            }
            DragKey::FromWayland(drag) => {
                if let Some(glue) = self.x11.drags.from_wayland.remove(&drag) {
                    lock(&glue.grab).live = false;
                }
            }
        }
    }

    /// What Smithay's data device is called with for X11 drag `key`'s target on `seat`.
    fn drag_target_call(&mut self, seat: SeatId, key: DragKey) -> Option<TargetCall> {
        let seat = self.seat(seat)?.seat.clone();
        let glue = self.x11.drags.from_x11.get_mut(&key)?;
        Some(TargetCall {
            seat,
            source: glue.source.clone(),
            offer: glue.offer.take(),
        })
    }

    /// Where the drag is in the surface `target`: the pointer's last place less the origin of the
    /// surface under it then.
    fn drag_point_in(&self, seat: SeatId) -> Point<f64, Logical> {
        match self.x11.drags.pointers.get(&seat) {
            Some(&PointerAt {
                location,
                origin: Some(origin),
            }) => location - origin,
            Some(&PointerAt { location, .. }) => location,
            None => Point::default(),
        }
    }

    fn enter_drag_target(&mut self, seat: SeatId, key: DragKey, target: &WlSurface) {
        let Some(call) = self.drag_target_call(seat, key) else {
            return;
        };
        call.source.forget_answer();
        let display = self.display_handle.clone();
        let point = self.drag_point_in(seat);
        let serial = SERIAL_COUNTER.next_serial();
        let offer = <WlSurface as DndFocus<State>>::enter(
            target,
            self,
            &display,
            call.source,
            &call.seat,
            point,
            &serial,
        );
        if let Some(glue) = self.x11.drags.from_x11.get_mut(&key) {
            glue.offer = offer;
        }
    }

    fn move_over_drag_target(&mut self, seat: SeatId, key: DragKey, target: &WlSurface) {
        let Some(mut call) = self.drag_target_call(seat, key) else {
            return;
        };
        let point = self.drag_point_in(seat);
        let time = self.input_time();
        <WlSurface as DndFocus<State>>::motion(
            target,
            self,
            call.offer.as_mut(),
            &call.seat,
            point,
            time,
        );
        if let Some(glue) = self.x11.drags.from_x11.get_mut(&key) {
            glue.offer = call.offer;
        }
    }

    /// Leaves `target`, its offer disabled, as Smithay's grab leaves a surface.
    fn leave_drag_target(&mut self, seat: SeatId, key: DragKey, target: &WlSurface) {
        let Some(mut call) = self.drag_target_call(seat, key) else {
            return;
        };
        <WlSurface as DndFocus<State>>::leave(target, self, call.offer.as_mut(), &call.seat);
        if let Some(offer) = call.offer {
            offer.disable();
        }
        call.source.forget_answer();
    }

    /// Drops on `target`, its offer marked dropped, then leaves it, as Smithay's grab does at a
    /// validated release: the target reads, then finishes.
    fn drop_on_drag_target(&mut self, seat: SeatId, key: DragKey, target: &WlSurface) {
        let Some(mut call) = self.drag_target_call(seat, key) else {
            return;
        };
        <WlSurface as DndFocus<State>>::drop(target, self, call.offer.as_mut(), &call.seat);
        if let Some(offer) = &call.offer {
            offer.drop();
        }
        <WlSurface as DndFocus<State>>::leave(target, self, call.offer.as_mut(), &call.seat);
    }

    /// Sends `seat`'s pointer's motion again where it is, and a frame: the X11 source of a drag
    /// looks again for the proxy just mapped.
    fn repeat_pointer_motion(&mut self, seat: SeatId) {
        let Some(pointer) = self.seat(seat).and_then(|data| data.pointer.clone()) else {
            return;
        };
        let location = pointer.current_location();
        let under = self.surface_under(seat, location);
        self.move_pointer(seat, &pointer, location, under);
    }

    /// When the first drag released waits on Yuna's clock no more.
    fn arm_drags_deadline(&mut self) {
        let next = self
            .x11
            .drags
            .seats
            .values()
            .filter_map(SeatDrags::next_deadline)
            .min();
        self.clock.disarm(Deadline::Drags);
        if let Some(at) = next {
            self.clock.arm(Deadline::Drags, at);
        }
    }

    /// The clock reached the first drag's deadline: those overdue end.
    pub(crate) fn drags_due(&mut self) {
        let now = self.clock.now();
        let seats: Vec<SeatId> = self.x11.drags.seats.keys().copied().collect();
        for seat in seats {
            let effects = self.seat_drags(seat).timed_out(now);
            self.apply_drag_effects(seat, effects);
        }
        self.arm_drags_deadline();
    }

    /// Xwayland `server` is gone: its drags end.
    pub(super) fn x11_drags_gone(&mut self, server: u64) {
        let seats: Vec<SeatId> = self.x11.drags.seats.keys().copied().collect();
        for seat in seats {
            let effects = self.seat_drags(seat).x11_gone(server);
            self.apply_drag_effects(seat, effects);
        }
    }

    /// `seat` closes: its drags end, refused.
    pub(crate) fn drags_seat_closed(&mut self, seat: SeatId) {
        let Some(mut drags) = self.x11.drags.seats.remove(&seat) else {
            return;
        };
        let effects = drags.close();
        self.apply_drag_effects(seat, effects);
        self.x11.drags.pointers.remove(&seat);
        self.x11.drags.grabs.remove(&seat);
        self.arm_drags_deadline();
    }
}

/// Tells `data_source`, if any and alive, the action its drag will take, from version 3.
fn tell_action(data_source: Option<&WlDataSource>, action: DragAction) {
    if let Some(source) = data_source
        && source.alive()
        && source.version() >= wl_data_source::EVT_ACTION_SINCE
    {
        source.action(dnd_action(action).into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R13: a Wayland drag proposes Copy if offered, else Move, else Ask; Copy with none.
    #[test]
    fn a_wayland_drag_proposes_copy_first() {
        assert_eq!(
            proposed_action(&[DndAction::Move, DndAction::Copy]),
            DragAction::Copy
        );
        assert_eq!(
            proposed_action(&[DndAction::Ask, DndAction::Move]),
            DragAction::Move
        );
        assert_eq!(proposed_action(&[DndAction::Ask]), DragAction::Ask);
        assert_eq!(proposed_action(&[]), DragAction::Copy);
    }

    /// An X11 drag's offer proposes its source's action with Copy, and a target's answer counts
    /// only once Smithay validated it and the type was offered.
    #[test]
    fn an_x11_drags_offer_proposes_its_action_with_copy() {
        let source = X11DragSource::new();
        let actions = |source: &X11DragSource| {
            source
                .metadata()
                .map(|metadata| metadata.dnd_actions.to_vec())
                .unwrap_or_default()
        };
        assert_eq!(actions(&source), [DndAction::Copy]);
        source.propose(DragAction::Move);
        assert_eq!(actions(&source), [DndAction::Move, DndAction::Copy]);
        source.offer(vec!["text/plain".to_owned()]);
        source.accepted(Some("image/png".to_owned()));
        assert!(!source.accepts(true), "a type not offered");
        source.accepted(Some("text/plain".to_owned()));
        assert!(!source.accepts(false), "not validated");
        assert!(source.accepts(true));
        source.choose_action(DndAction::Move);
        assert_eq!(source.news(true).answer, (true, DragAction::Move));
        source.end();
        assert!(!source.alive(), "ended, it is read no more");
    }

    /// M2: the reads waiting when the drag ends are dropped, their pipes closed: their readers
    /// see them end; a read asked after the end is closed at once.
    #[test]
    fn an_ended_x11_drag_closes_the_pipes_of_its_waiting_reads() {
        let source = X11DragSource::new();
        let Some((read, write)) = reader_pipe() else {
            panic!("a pipe");
        };
        source.send("text/plain", write);
        source.end();
        let mut byte = [0; 1];
        assert_eq!(
            smithay::reexports::rustix::io::read(&read, &mut byte),
            Ok(0),
            "the reader sees its pipe end"
        );
        let Some((late_read, late_write)) = reader_pipe() else {
            panic!("a pipe");
        };
        source.send("text/plain", late_write);
        assert_eq!(
            smithay::reexports::rustix::io::read(&late_read, &mut byte),
            Ok(0),
            "a read after the end is closed at once"
        );
        assert!(source.news(false).reads.is_empty());
    }

    /// R20: reads wait for the round's end, [`MAX_X11_TRANSFERS`] at most.
    #[test]
    fn an_x11_drags_offer_keeps_a_bounded_number_of_reads() {
        let source = X11DragSource::new();
        for _ in 0..MAX_X11_TRANSFERS + 3 {
            let Some((_, write)) = reader_pipe() else {
                panic!("a pipe");
            };
            source.send("text/plain", write);
        }
        assert_eq!(source.news(false).reads.len(), MAX_X11_TRANSFERS);
        assert!(source.news(false).reads.is_empty(), "taken once");
    }
}
