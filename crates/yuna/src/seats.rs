//! Seats ([ADR-0138](../../../docs/adr/0138-agents-seats-one-seat-per-yuna-from-stage-2s-core.md),
//! [ADR-0117](../../../docs/adr/0117-an-agents-seat-through-mcp-tools-input-keyboard-text-and-closing.md)):
//! the owner's, and one for each agent.
//!
//! Each seat is a Smithay [`Seat`] with a keyboard, and, for an agent's, a pointer. Each has its own
//! keyboard focus, popup grab, selections and cursor. Seats isolate focus and input, not
//! processes: any program of the user may reach any seat's socket, X display or accessibility
//! bus; seats are no security boundary between agents. Every client belongs to one seat: the seat
//! whose socket it connected through, recorded in its [`ClientState`]. Its windows open on that
//! seat's workspaces, and only that seat's input reaches them (`input.rs`). The owner's seat
//! exists from the start, with or without a `wl_seat` global; in `yuna mcp` it has none, so that
//! a client sees no other seat's `wl_seat`, nor its outputs' `wl_output`s
//! ([ADR-0112](../../../docs/adr/0112-smithay-pinned-patched-in-a-public-fork-left-at-stage-2.md)): Yuna's fork of Smithay shows
//! those globals only to the clients [`belongs_to`] accepts. Every focus of a seat is on its own
//! clients' surfaces as well, so that a client that held another seat's global would get nothing.
//!
//! An agent's seat starts with the `us` layout, whatever the machine's settings
//! ([`AGENT_KEYBOARD_LAYOUT`]), and its agent may change it while the seat holds no key
//! ([`State::set_keyboard_layout`], `keymap.rs`). When it is removed, the keys and buttons it
//! holds are released, its focus leaves, and only then are its clients ended; its outputs, its
//! `wl_seat` global and its place in the layout go once they are freed
//! (`Compositor::remove_seat`).

use std::collections::BTreeSet;
use std::net::Shutdown;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use smithay::input::keyboard::{KeyboardHandle, KeymapFile, XkbConfig};
use smithay::input::pointer::{CursorImageStatus, PointerHandle};
use smithay::input::{Seat, SeatState};
use smithay::utils::{Logical, Point, Serial};
use wayland_server::backend::{ClientId, DisconnectReason};
use wayland_server::protocol::wl_seat::WlSeat;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, DisplayHandle, Resource};
use xkbcommon::xkb;
use yuna_motion::Change;
use yuna_types::SeatId;

use crate::command::refusal;
use crate::keymap::{KeyboardLayout, SeatKeymap};
use crate::popups::ActivePopupGrab;
use crate::state::{ClientState, OWNER_SEAT, State};
use crate::{ClientArrival, ClientProcess, Error};

/// The layout every agent's keyboard starts with: the same on every machine and in CI, whatever
/// the `XKB_DEFAULT_*` variables say (ADR-0037).
pub const AGENT_KEYBOARD_LAYOUT: &str = "us";

/// The most clients one seat connects at once, its Xwayland apart
/// ([ADR-0147](../../../docs/adr/0147-a-seat-connects-256-clients-at-most.md)): one program
/// opening connection after connection would otherwise multiply what a client that stops reading
/// can make Yuna hold, its buffer and the descriptors it keeps
/// ([ADR-0136](../../../docs/adr/0136-a-client-that-stops-reading-1-mib-and-its-motion-merged.md)).
///
/// Past it, a connection on the seat's socket is accepted and closed at once, the socket
/// listening on and the seat's other clients served; its refusal is told, with those before it,
/// at most once a second of real time, in a warning and to MCP's agents (`Refusals`). An
/// in-process connection ([`Compositor::connect_client_to`](crate::Compositor::connect_client_to))
/// is an error instead. The seat's own Xwayland is Yuna's: it is neither counted nor refused, so
/// that the seat's X11 never breaks. A client frees its place once the display has ended it,
/// before Yuna frees what it held at the round's end.
pub const MAX_CLIENTS_PER_SEAT: usize = 256;

/// How often a seat's refusals past [`MAX_CLIENTS_PER_SEAT`] are told at most, on real time
/// whatever the clock: in Yuna's log, and to MCP's agents.
const REFUSALS_TOLD_EVERY: Duration = Duration::from_secs(1);

/// How long a key waits before it repeats, and how many times a second it then repeats, as most
/// desktops have it. Clients repeat a held key themselves.
const REPEAT_DELAY_MS: i32 = 600;
const REPEAT_RATE: i32 = 25;

/// A seat's id, kept in its Smithay seat's data, so that a `wl_seat` leads back to it.
struct SeatTag(SeatId);

/// One seat and its input.
pub(crate) struct SeatData {
    pub id: SeatId,
    pub seat: Seat<State>,
    pub keyboard: KeyboardHandle<State>,
    /// An agent's seat has one; the owner's, headless, has none yet.
    pub pointer: Option<PointerHandle<State>>,
    /// The popup grab that holds this seat's keyboard and pointer, if any.
    pub popup_grab: Option<ActivePopupGrab>,
    /// The cursor the client under the pointer asked for (`wl_pointer.set_cursor`,
    /// `cursor-shape`). Yuna does not draw it yet.
    pub cursor: CursorImageStatus,
    /// The serial of the last key press and the surface it went to, which an activation token or
    /// a popup grab may carry.
    pub last_key_press: Option<(Serial, WlSurface)>,
    /// The serial of the last button press and the toplevel surface of the window under the
    /// pointer then.
    pub last_button_press: Option<(Serial, WlSurface)>,
    /// The buttons its pointer holds, in the order they were pressed:
    /// [`MAX_HELD_BUTTONS`](crate::input::MAX_HELD_BUTTONS) at most.
    pub pressed_buttons: Vec<u32>,
    /// Whether its pointer has moved since the seat was made: until then it is nowhere.
    pub pointer_moved: bool,
    /// The origin, in logical coordinates, of the surface under its pointer, as last sent.
    pub pointer_origin: Option<Point<f64, Logical>>,
    /// An agent seat's keymap, the one its keyboard has, to find which keys type a keysym. It
    /// changes with `own_keymap`, together.
    pub keymap: Option<SeatKeymap>,
    /// The keymap its keyboard was made with, or last given by a change of layout: a virtual
    /// keyboard's keys switch the keyboard to that keyboard's own keymap, and Yuna's own keys
    /// switch it back (`virtual_input.rs`).
    pub own_keymap: KeymapFile,
    /// The clients connected through its socket, or for it through `connect_client_to`, and its
    /// Xwayland's, while they are served: [`MAX_CLIENTS_PER_SEAT`] at most, its Xwayland apart.
    pub clients: Vec<SeatClient>,
    /// The connections its socket refused past [`MAX_CLIENTS_PER_SEAT`], not yet told.
    pub refusals: Refusals,
    /// The commits of its clients' surfaces: a seat's accessibility call walks its trees again
    /// when they move, never for another seat's drawing.
    pub commits: u64,
    /// Whether its clients' new xdg-shell windows open floating rather than tiled: a test
    /// seam's, the WLCS shim's (`Compositor::set_seat_opens_floating`).
    pub opens_floating: bool,
}

impl SeatData {
    /// The owner's seat: a keyboard of the machine's layout, and a `wl_seat` global if
    /// `announced`.
    pub(crate) fn owner(
        seat_state: &mut SeatState<State>,
        display: &DisplayHandle,
        announced: bool,
    ) -> Result<SeatData, Error> {
        let mut seat = if announced {
            seat_state
                .new_wl_seat_with_filter(display, "seat0", |client| belongs_to(client, OWNER_SEAT))
        } else {
            seat_state.new_seat("seat0")
        };
        // The default keymap, from XKB_CONFIG_ROOT and the XKB_DEFAULT_* variables.
        let keyboard = seat
            .add_keyboard(XkbConfig::default(), REPEAT_DELAY_MS, REPEAT_RATE)
            .map_err(|e| Error::Keyboard(e.to_string()))?;
        Ok(SeatData::new(OWNER_SEAT, seat, keyboard, None))
    }

    /// An agent's seat: its `wl_seat` global, named `agent-<id>`, a keyboard of
    /// [`AGENT_KEYBOARD_LAYOUT`], a pointer, and Yuna as its input method (`text_input.rs`).
    pub(crate) fn agent(
        id: SeatId,
        seat_state: &mut SeatState<State>,
        display: &DisplayHandle,
    ) -> Result<SeatData, Error> {
        let keymap = SeatKeymap::compile(KeyboardLayout::agents_default())
            .map_err(|e| Error::Keyboard(format!("the agents' keymap: {e}")))?;
        let mut seat =
            seat_state.new_wl_seat_with_filter(display, format!("agent-{}", id.0), move |client| {
                belongs_to(client, id)
            });
        // No environment names: the XKB_DEFAULT_* variables never reach an agent's keyboard.
        let keyboard = seat
            .add_keyboard_with_context_flags(
                keymap.layout().xkb_config(),
                REPEAT_DELAY_MS,
                REPEAT_RATE,
                xkb::CONTEXT_NO_ENVIRONMENT_NAMES,
            )
            .map_err(|e| Error::Keyboard(e.to_string()))?;
        let pointer = seat.add_pointer();
        crate::text_input::act_as_input_method(&seat);
        let mut data = SeatData::new(id, seat, keyboard, Some(pointer));
        data.keymap = Some(keymap);
        Ok(data)
    }

    fn new(
        id: SeatId,
        seat: Seat<State>,
        keyboard: KeyboardHandle<State>,
        pointer: Option<PointerHandle<State>>,
    ) -> SeatData {
        seat.user_data().insert_if_missing(|| SeatTag(id));
        let own_keymap = keyboard.keymap_file();
        SeatData {
            id,
            seat,
            own_keymap,
            keyboard,
            pointer,
            popup_grab: None,
            cursor: CursorImageStatus::default_named(),
            last_key_press: None,
            last_button_press: None,
            pressed_buttons: Vec::new(),
            pointer_moved: false,
            pointer_origin: None,
            keymap: None,
            clients: Vec::new(),
            refusals: Refusals::default(),
            commits: 0,
            opens_floating: false,
        }
    }
}

/// A client a seat serves.
#[derive(Debug, Clone)]
pub(crate) struct SeatClient {
    pub id: ClientId,
    /// Whether it is the seat's Xwayland, which [`MAX_CLIENTS_PER_SEAT`] does not count.
    pub xwayland: bool,
}

/// The connections a seat's socket refused past [`MAX_CLIENTS_PER_SEAT`]: told at the first
/// refusal, then at the first one [`REFUSALS_TOLD_EVERY`] or more after the last told, with
/// every refusal since. No timer is armed for them: the refusals after the last told wait for a
/// refusal a second or more after it, and are never told if none comes, nor if the seat is
/// removed meanwhile.
#[derive(Debug, Default)]
pub(crate) struct Refusals {
    /// Those refused since the last told.
    untold: u64,
    /// When they were last told, on real time; none before the first.
    told_at: Option<Duration>,
}

impl Refusals {
    /// Counts a refusal at `now`, on real time, and gives how many refusals to tell now, this one
    /// included; none while the last told is under [`REFUSALS_TOLD_EVERY`] old.
    fn refused(&mut self, now: Duration) -> Option<u64> {
        self.untold += 1;
        if self
            .told_at
            .is_some_and(|at| now.saturating_sub(at) < REFUSALS_TOLD_EVERY)
        {
            return None;
        }
        self.told_at = Some(now);
        Some(std::mem::take(&mut self.untold))
    }
}

/// The seat a client belongs to; the owner's for a client Yuna did not insert, which cannot
/// happen.
pub(crate) fn seat_of_client(client: &Client) -> SeatId {
    client
        .get_data::<ClientState>()
        .map_or(OWNER_SEAT, |state| state.seat)
}

/// Whether `client` belongs to `seat`: what decides which seat's and outputs' globals it sees. A
/// client Yuna did not insert, which cannot happen, belongs to none.
pub(crate) fn belongs_to(client: &Client, seat: SeatId) -> bool {
    client
        .get_data::<ClientState>()
        .is_some_and(|state| state.seat == seat || state.sees_every_seat)
}

/// The seat the client of `surface` belongs to; the owner's for a surface whose client is gone.
pub(crate) fn seat_of_surface(surface: &WlSurface) -> SeatId {
    surface
        .client()
        .map_or(OWNER_SEAT, |client| seat_of_client(&client))
}

/// The id of a Smithay seat of Yuna's.
pub(crate) fn seat_id(seat: &Seat<State>) -> Option<SeatId> {
    seat.user_data().get::<SeatTag>().map(|tag| tag.0)
}

/// The seat a client's `wl_seat` is of; none once that seat is gone.
pub(crate) fn seat_of_wl_seat(seat: &WlSeat) -> Option<SeatId> {
    Seat::<State>::from_resource(seat).and_then(|seat| seat_id(&seat))
}

impl State {
    /// The seat `id`, while it exists.
    pub(crate) fn seat(&self, id: SeatId) -> Option<&SeatData> {
        self.seats.iter().find(|seat| seat.id == id)
    }

    pub(crate) fn seat_mut(&mut self, id: SeatId) -> Option<&mut SeatData> {
        self.seats.iter_mut().find(|seat| seat.id == id)
    }

    /// The ids of every seat, the owner's first.
    pub(crate) fn seat_ids(&self) -> Vec<SeatId> {
        self.seats.iter().map(|seat| seat.id).collect()
    }

    /// Adds an agent's seat with no output: in the layout, and as a `wl_seat` global announced
    /// at the round's end.
    pub(crate) fn add_agent_seat(&mut self) -> Result<SeatId, Error> {
        let id = SeatId(self.next_seat_id);
        let data = SeatData::agent(id, &mut self.seat_state, &self.display_handle)?;
        self.layout
            .add_seat(id)
            .map_err(|e| Error::Layout(refusal(e).to_string()))?;
        self.next_seat_id += 1;
        self.seats.push(data);
        self.rest.add_seat(id);
        tracing::info!(seat = id.0, "seat added");
        Ok(id)
    }

    /// Changes agent seat `seat`'s keyboard layout: its keyboard's keymap, which every client of
    /// the seat is sent, Xwayland's included, and the keymap Yuna types through, together. The
    /// keyboard's locked modifiers and group start again from none, as with a new keyboard.
    /// Refused, nothing changed, for the owner's seat, a seat that does not exist, a layout
    /// [`SeatKeymap::compile`] refuses, and while the seat holds a key, its own or a virtual
    /// keyboard's.
    pub(crate) fn set_keyboard_layout(
        &mut self,
        seat: SeatId,
        layout: KeyboardLayout,
    ) -> Result<(), Error> {
        let keyboard = self.agent_keyboard(seat)?;
        if !keyboard.pressed_keys().is_empty() {
            return Err(Error::Refused(
                "the seat holds a key: release it before changing the layout".to_owned(),
            ));
        }
        let keymap = SeatKeymap::compile(layout)?;
        // From the text of Yuna's own: half the time of compiling the names again (`layout_time`).
        keyboard
            .set_keymap_from_string(self, keymap.text())
            .map_err(|e| Error::Keyboard(e.to_string()))?;
        let own_keymap = keyboard.keymap_file();
        let layout = keymap.layout().clone();
        if let Some(data) = self.seat_mut(seat) {
            data.keymap = Some(keymap);
            data.own_keymap = own_keymap;
        }
        tracing::info!(
            seat = seat.0,
            layout = %layout.layout,
            variant = %layout.variant,
            options = %layout.options,
            "keyboard layout changed"
        );
        Ok(())
    }

    /// Agent seat `seat`'s keyboard layout; none for the owner's seat and a seat that does not
    /// exist.
    pub(crate) fn keyboard_layout(&self, seat: SeatId) -> Option<KeyboardLayout> {
        self.seat(seat)?
            .keymap
            .as_ref()
            .map(|keymap| keymap.layout().clone())
    }

    /// Serves a client of `seat` on `stream`, and records it on the seat, for its removal, with
    /// who it is: its peer's process, from the socket's credentials, and, for an agent's seat,
    /// the processes above it, read in `/proc` now and checked through the peer's pidfd, which
    /// says whether it had exited since it connected ([`ClientProcess::accepted`]). Refused for
    /// a seat that does not exist, and, before anything of the client is read, for one that
    /// serves [`MAX_CLIENTS_PER_SEAT`] clients already. With `sees_every_seat`, it is shown
    /// every seat's globals, for tests alone. Returns the client's id.
    pub(crate) fn insert_client(
        &mut self,
        seat: SeatId,
        stream: UnixStream,
        sees_every_seat: bool,
    ) -> Result<ClientId, Error> {
        if self.is_full(seat) {
            return Err(Error::Refused(format!(
                "seat {} serves {MAX_CLIENTS_PER_SEAT} clients, the most a seat connects \
                 (ADR-0147): another connects once one has left",
                seat.0
            )));
        }
        self.serve_client(seat, stream, sees_every_seat)
    }

    /// Serves a client that connected through `seat`'s socket, or, past
    /// [`MAX_CLIENTS_PER_SEAT`], closes its connection at once and counts it
    /// ([`State::refuse_connection`]).
    pub(crate) fn accept_connection(&mut self, seat: SeatId, stream: UnixStream) {
        if self.is_full(seat) {
            self.refuse_connection(seat, stream);
        } else if let Err(error) = self.serve_client(seat, stream, false) {
            tracing::warn!(%error, seat = seat.0, "a client could not connect");
        }
    }

    /// Whether `seat` serves [`MAX_CLIENTS_PER_SEAT`] clients, its Xwayland's apart, and those
    /// the display has ended and Yuna not yet freed: their place is free already. False for a
    /// seat that does not exist, which serving refuses.
    fn is_full(&self, seat: SeatId) -> bool {
        let departed = self
            .departed_clients
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.seat(seat).is_some_and(|data| {
            data.clients
                .iter()
                .filter(|client| !client.xwayland)
                .filter(|client| !departed.iter().any(|gone| gone.client == client.id))
                .count()
                >= MAX_CLIENTS_PER_SEAT
        })
    }

    /// Closes a connection `seat`'s socket took past [`MAX_CLIENTS_PER_SEAT`], and tells it, with
    /// the refusals before it not yet told, at most once every [`REFUSALS_TOLD_EVERY`] of real
    /// time ([`Refusals`]): in a warning, and as an event (`events.rs`). The connection is shut
    /// down before it is closed, so that its peer reads its end of file at once, even while a
    /// child that has not yet run its program, an Xwayland starting, holds a copy of the
    /// descriptor.
    fn refuse_connection(&mut self, seat: SeatId, stream: UnixStream) {
        // A peer gone already is what shutting down would have made it.
        let _ = stream.shutdown(Shutdown::Both);
        drop(stream);
        let now = self.clock.real_now();
        let Some(refused) = self
            .seat_mut(seat)
            .and_then(|data| data.refusals.refused(now))
        else {
            return;
        };
        tracing::warn!(
            seat = seat.0,
            bound = MAX_CLIENTS_PER_SEAT,
            refused,
            "a seat refuses connections past its bound of clients"
        );
        self.events.client_refused(seat, refused);
    }

    /// Serves a client of `seat` on `stream`, as [`State::insert_client`] says, whatever the
    /// clients the seat serves.
    fn serve_client(
        &mut self,
        seat: SeatId,
        stream: UnixStream,
        sees_every_seat: bool,
    ) -> Result<ClientId, Error> {
        let pid = rustix::net::sockopt::socket_peercred(&stream)
            .ok()
            .map(|credentials| credentials.pid.as_raw_nonzero().get());
        let process = if seat == OWNER_SEAT {
            ClientProcess::unread(pid)
        } else {
            let pidfd = crate::process::peer_pidfd(stream.as_fd());
            ClientProcess::accepted(pid, pidfd, &mut self.pidfd_refusal_warned)
        };
        let data = ClientState::new(
            seat,
            sees_every_seat,
            process,
            self.departed_clients.clone(),
        );
        self.insert_client_state(stream, data)
    }

    /// Serves `seat`'s Xwayland `server`, process `pid`, on `stream`, marked as such, with the
    /// client scale `scale`: it is told outputs and pointer in X11 pixels, `scale` per logical
    /// unit ([ADR-0068](../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)).
    /// Its socket pair's credentials are Yuna's own, so its pid is given; its lineage, Yuna's
    /// child, names no program, and is not read. Refused for a seat that does not exist.
    pub(crate) fn insert_xwayland_client(
        &mut self,
        seat: SeatId,
        stream: UnixStream,
        scale: f64,
        server: u64,
        pid: i32,
    ) -> Result<(), Error> {
        let process = ClientProcess::unread(Some(pid));
        let mut data = ClientState::new(seat, false, process, self.departed_clients.clone());
        data.xwayland = Some(server);
        // Before it can bind a global, so that it never sees an output unscaled.
        data.compositor.set_client_scale(scale);
        self.insert_client_state(stream, data).map(drop)
    }

    /// Serves the client of `data`'s seat on `stream`, and records it on the seat and in the
    /// rest auditor, with a second descriptor of its socket, which names it when it wakes Yuna
    /// (`rest.rs`), and among the arrivals, when they are watched
    /// (`Compositor::watch_client_arrivals`). Out of descriptors, it is served without a second
    /// descriptor, unnamed. Returns its id.
    fn insert_client_state(
        &mut self,
        stream: UnixStream,
        mut data: ClientState,
    ) -> Result<ClientId, Error> {
        let seat = data.seat;
        if self.seat(seat).is_none() {
            return Err(Error::Refused(format!("no seat {}", seat.0)));
        }
        let second = stream
            .try_clone()
            .inspect_err(|error| {
                tracing::debug!(%error, "a client is served without a second descriptor");
            })
            .ok();
        let number = self.rest.next_client_number();
        data.rest_number = number;
        let process = data.process.clone();
        let xwayland = data.xwayland.is_some();
        let client = self
            .display_handle
            .insert_client(stream, Arc::new(data))
            .map_err(|e| Error::EventLoop(e.to_string()))?;
        if let Some(arrivals) = &mut self.client_arrivals {
            arrivals.push(ClientArrival {
                seat,
                number,
                process: process.clone(),
            });
        }
        self.rest
            .client_connected(number, seat, process, xwayland, second);
        if let Some(seat) = self.seat_mut(seat) {
            seat.clients.push(SeatClient {
                id: client.id(),
                xwayland,
            });
        }
        Ok(client.id())
    }

    /// The seat of the client `client`, while the display serves it.
    pub(crate) fn seat_of_client_id(&self, client: &ClientId) -> Option<SeatId> {
        self.seats
            .iter()
            .find(|seat| seat.clients.iter().any(|served| served.id == *client))
            .map(|seat| seat.id)
    }

    /// The process ids of `seat`'s clients, as the kernel gave them for their sockets.
    pub(crate) fn client_pids(&self, seat: SeatId) -> Vec<i32> {
        let backend = self.display_handle.backend_handle();
        self.seat(seat)
            .into_iter()
            .flat_map(|seat| &seat.clients)
            .filter_map(|client| backend.get_client_credentials(client.id.clone()).ok())
            .map(|credentials| credentials.pid)
            .collect()
    }

    /// Forgets `client` in its seat's list once the display has ended it.
    pub(crate) fn forget_client(&mut self, client: &ClientId) {
        for seat in &mut self.seats {
            seat.clients.retain(|served| served.id != *client);
        }
    }

    /// The first step of removing agent seat `id`: releases the keys and buttons it holds, ends
    /// its drags and its popup grab, and moves its focus off every surface, so that its clients
    /// are told.
    /// Refused for the owner's seat and for a seat that does not exist.
    pub(crate) fn release_seat(&mut self, id: SeatId) -> Result<(), Error> {
        if id == OWNER_SEAT {
            return Err(Error::Refused(
                "the owner's seat is never removed".to_owned(),
            ));
        }
        if self.seat(id).is_none() {
            return Err(Error::Refused(format!("no seat {}", id.0)));
        }
        self.release_held_input(id);
        self.drags_seat_closed(id);
        self.end_popup_grab(id);
        self.give_keyboard_to(id, None);
        if let Some(pointer) = self.seat(id).and_then(|seat| seat.pointer.clone()) {
            let location = pointer.current_location();
            self.send_pointer_motion(&pointer, None, location);
        }
        Ok(())
    }

    /// The second step: ends the seat's clients, whose windows close when the display frees
    /// them, and returns their process ids, as the kernel gave them for their sockets.
    pub(crate) fn end_seat_clients(&mut self, id: SeatId) -> Vec<i32> {
        let pids = self.client_pids(id);
        let clients = self
            .seat(id)
            .map(|seat| seat.clients.clone())
            .unwrap_or_default();
        let backend = self.display_handle.backend_handle();
        for client in clients {
            backend.kill_client(client.id, DisconnectReason::ConnectionClosed);
        }
        pids
    }

    /// The last step, once its clients are freed: removes its outputs, its place in the layout
    /// and its `wl_seat` global, which is disabled now and destroyed later, as a removed
    /// output's is. Smithay keeps a record of every seat it made, with no way to drop one: a
    /// removed seat's record stays, without its keyboard, its pointer and its selections.
    pub(crate) fn finish_removing_seat(&mut self, id: SeatId) -> Result<(), Error> {
        let outputs = self.layout.outputs(id).unwrap_or_default();
        for output in outputs {
            if let Some(index) = self.output_index(output) {
                self.drop_output(index);
            }
        }
        if let Err(error) = self.layout.remove_seat(id) {
            tracing::error!(error = %refusal(error), seat = id.0, "the layout keeps a removed seat");
        }
        let Some(index) = self.seats.iter().position(|seat| seat.id == id) else {
            return Ok(());
        };
        // While Yuna still knows the seat: Smithay keeps its record, and the selections on it.
        self.forget_seat_selections(id);
        let mut removed = self.seats.remove(index);
        if let Some(global) = removed.seat.global() {
            self.display_handle.disable_global::<State>(global.clone());
            self.retired_globals.push((self.clock.now(), global));
        }
        removed.seat.remove_keyboard();
        removed.seat.remove_pointer();
        // A seat removed takes its outputs away at once, as an output removed does; the other
        // seats' motions go on (ADR-0015).
        self.mark_layout_changed(Change::InstantFor(BTreeSet::from([id])));
        tracing::info!(seat = id.0, "seat removed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::time::Instant;

    use rustix::io::Errno;

    use super::*;
    use crate::process::{has_exited, peer_pidfd};
    use crate::{Compositor, Config, Identity};

    /// The variable that names the socket [`connect_to_the_socket_the_environment_names`]
    /// connects to.
    const CONNECT_TO: &str = "YUNA_TEST_CONNECT_TO";

    /// The longest a step waits: a debug build on a loaded machine is slow.
    const PATIENCE: Duration = Duration::from_secs(30);

    /// Not a test of its own: what a [`ConnectingChild`] runs, this test binary run again,
    /// filtered to it. It connects to the socket [`CONNECT_TO`] names, then stays until its
    /// standard input ends. Without the variable, it does nothing.
    #[test]
    #[ignore = "a helper that the accept's tests run in a process of its own"]
    fn connect_to_the_socket_the_environment_names() {
        let Some(path) = std::env::var_os(CONNECT_TO) else {
            return;
        };
        let _connection = UnixStream::connect(path).expect("the socket takes the connection");
        let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut Vec::new());
    }

    /// A process this test started that connects to the socket at `path`, through
    /// [`connect_to_the_socket_the_environment_names`]: under a shell when `under_a_shell`, and,
    /// when `stays`, until it drops. Ended and reaped when it drops, an assertion that failed
    /// included: its input closed, it leaves by itself, the shell reaping it; only past
    /// [`PATIENCE`] is the test's own child killed.
    struct ConnectingChild(Child);

    impl ConnectingChild {
        fn start(path: &Path, under_a_shell: bool, stays: bool) -> ConnectingChild {
            let helper = std::env::current_exe().expect("the test binary");
            let filter = [
                "seats::tests::connect_to_the_socket_the_environment_names",
                "--exact",
                "--ignored",
                "--test-threads=1",
            ];
            let mut command = if under_a_shell {
                // Not the last command, so that the shell waits for it rather than running it in
                // its own place.
                let mut shell = Command::new("sh");
                shell
                    .args(["-c", "\"$0\" \"$@\"; :"])
                    .arg(helper)
                    .args(filter);
                shell
            } else {
                let mut command = Command::new(helper);
                command.args(filter);
                command
            };
            let child = command
                .env(CONNECT_TO, path)
                .stdin(if stays { Stdio::piped() } else { Stdio::null() })
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("the helper starts");
            ConnectingChild(child)
        }

        fn pid(&self) -> i32 {
            rustix::process::Pid::from_child(&self.0)
                .as_raw_nonzero()
                .get()
        }

        /// Waits for it to end, and reaps it.
        fn wait(&mut self) -> ExitStatus {
            self.0.wait().expect("the helper is waited for")
        }
    }

    impl Drop for ConnectingChild {
        fn drop(&mut self) {
            drop(self.0.stdin.take());
            let until = Instant::now() + PATIENCE;
            while Instant::now() < until {
                if !matches!(self.0.try_wait(), Ok(None)) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// A folder of the test's own, short enough for a socket's path, removed when it drops.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Folder {
            let temp = std::env::temp_dir();
            let base = if temp.as_os_str().len() <= 16 {
                temp
            } else {
                PathBuf::from("/tmp")
            };
            let path = base.join(format!("yuna-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir(&path).expect("the test's folder");
            Folder(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Whether this kernel gives a pidfd of a socket's peer: Linux 6.5 and later.
    fn kernel_gives_peer_pidfds() -> bool {
        let (ours, _theirs) = UnixStream::pair().expect("a socket pair");
        match peer_pidfd(ours.as_fd()) {
            Ok(_) => true,
            Err(error) if error.raw_os_error() == Some(Errno::NOPROTOOPT.raw_os_error()) => {
                eprintln!("this kernel refuses SO_PEERPIDFD");
                false
            }
            Err(error) => panic!("SO_PEERPIDFD failed: {error}"),
        }
    }

    /// A compositor with an agent's seat that listens on a socket in `folder`, its arrivals
    /// watched.
    fn listening_compositor(folder: &Folder) -> (Compositor, PathBuf) {
        let mut compositor = Compositor::new(Config {
            outputs: Vec::new(),
            listen: false,
            announce_owner_seat: false,
            ..Config::default()
        })
        .expect("the compositor starts");
        let seat = compositor.add_seat().expect("a seat");
        let path = folder.0.join("seat");
        compositor
            .listen_for_seat(seat, path.clone())
            .expect("the seat listens");
        compositor.watch_client_arrivals();
        (compositor, path)
    }

    /// The first client the compositor serves, its rounds run until it does.
    fn first_arrival(compositor: &mut Compositor) -> ClientArrival {
        let until = Instant::now() + PATIENCE;
        loop {
            compositor
                .dispatch(Some(Duration::from_millis(10)))
                .expect("a round");
            if let Some(arrival) = compositor.take_client_arrivals().into_iter().next() {
                return arrival;
            }
            assert!(Instant::now() < until, "no client connected");
        }
    }

    /// A peer that connected, then exited and was reaped before the accept, gives `EINVAL` on
    /// Linux 6.5 to 6.15, or, from 6.16, a pidfd that reads as exited; a kernel before 6.5
    /// refuses the option. The test says which this kernel gave.
    #[test]
    fn a_peer_reaped_before_the_accept_is_refused_or_its_pidfd_reads_exited() {
        let folder = Folder::new("reaped-peer");
        let path = folder.0.join("socket");
        let listener = UnixListener::bind(&path).expect("the socket listens");
        let mut child = ConnectingChild::start(&path, false, false);
        assert!(child.wait().success(), "the helper did not connect");
        let (stream, _) = listener.accept().expect("its connection waits");
        match peer_pidfd(stream.as_fd()) {
            Ok(pidfd) => {
                eprintln!("the kernel gives a pidfd of a peer already reaped");
                assert!(has_exited(pidfd.as_fd()).expect("polled"));
            }
            Err(error) => {
                eprintln!("the kernel refuses a peer already reaped: {error}");
                let refusals = [Errno::INVAL, Errno::NOPROTOOPT].map(Errno::raw_os_error);
                assert!(
                    error
                        .raw_os_error()
                        .is_some_and(|raw| refusals.contains(&raw)),
                    "{error}"
                );
            }
        }
    }

    /// A child that connected to an agent seat's socket, then exited and was reaped before the
    /// compositor accepted its connection, is recorded as exited, with no lineage and no group,
    /// whatever the kernel answered for it, `EINVAL` or a pidfd that reads as exited. A kernel
    /// that refuses the option leaves the walk alone, marked inexact, which finds no lineage.
    #[test]
    fn a_client_whose_peer_was_reaped_before_the_accept_is_recorded_exited() {
        let checked = kernel_gives_peer_pidfds();
        let folder = Folder::new("reaped-client");
        let (mut compositor, path) = listening_compositor(&folder);
        let mut child = ConnectingChild::start(&path, false, false);
        assert!(child.wait().success(), "the helper did not connect");
        let process = first_arrival(&mut compositor).process;
        let identity = if checked {
            Identity::Exited
        } else {
            Identity::Inexact
        };
        assert_eq!(process.identity, identity, "{process:?}");
        assert_eq!(process.lineage, Vec::<i32>::new(), "{process:?}");
        assert_eq!(process.group, None, "{process:?}");
    }

    /// A child's client, connected and accepted while it runs, under a shell the test started,
    /// is recorded exact: its lineage is the child and the shell, nearest first, its group the
    /// test's, which the shell did not leave.
    #[test]
    fn a_live_childs_client_is_recorded_exact_its_lineage_nearest_first() {
        let checked = kernel_gives_peer_pidfds();
        let folder = Folder::new("live-client");
        let (mut compositor, path) = listening_compositor(&folder);
        let shell = ConnectingChild::start(&path, true, true);
        let process = first_arrival(&mut compositor).process;
        let identity = if checked {
            Identity::Exact
        } else {
            Identity::Inexact
        };
        assert_eq!(process.identity, identity, "{process:?}");
        let helper = process.pid.expect("the peer's pid");
        assert_ne!(
            helper,
            shell.pid(),
            "the shell ran the helper in its own place"
        );
        assert_eq!(process.lineage, [helper, shell.pid()], "{process:?}");
        let group = rustix::process::getpgrp().as_raw_nonzero().get();
        assert_eq!(process.group, Some(group), "{process:?}");
    }

    #[test]
    fn refusals_are_told_at_the_first_then_once_a_second_with_those_between() {
        let at = Duration::from_millis;
        let mut refusals = Refusals::default();
        assert_eq!(
            refusals.refused(at(5_000)),
            Some(1),
            "the first is told at once"
        );
        assert_eq!(refusals.refused(at(5_400)), None);
        assert_eq!(refusals.refused(at(5_999)), None);
        assert_eq!(
            refusals.refused(at(6_000)),
            Some(3),
            "a second after the last told: those between, and this one"
        );
        assert_eq!(refusals.refused(at(6_500)), None);
        // No timer: the one refused at 6.5 s waits for the next refusal, however late.
        assert_eq!(refusals.refused(at(60_000)), Some(2));
        // A clock read earlier than the last told, which real time never gives, tells nothing.
        assert_eq!(refusals.refused(at(1_000)), None);
    }
}
