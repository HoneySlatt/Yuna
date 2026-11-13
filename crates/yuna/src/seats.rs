//! Seats ([ADR-0015](../../../docs/adr/0015-agents-workspaces.md),
//! [ADR-0035](../../../docs/adr/0035-agents-seats.md),
//! [ADR-0037](../../../docs/adr/0037-seats-tools-input-and-closing.md)): the owner's, and one for
//! each agent.
//!
//! Each seat is a Smithay [`Seat`] with a keyboard, and, for an agent's, a pointer. Each has its own
//! keyboard focus, popup grab, selections and cursor. Every client belongs to one seat: the seat
//! whose socket it connected through, recorded in its [`ClientState`]. Its windows open on that
//! seat's workspaces, and only that seat's input reaches them (`input.rs`). The owner's seat
//! exists from the start, with or without a `wl_seat` global; in `yuna mcp` it has none, so that
//! an agent's programs see only agents' seats. Upstream Smithay announces every `wl_seat` to
//! every client: a client of one agent may bind another's, and then gets nothing, since every
//! focus of a seat is on its own clients' surfaces.
//!
//! An agent's seat has the `us` layout, whatever the machine's settings
//! ([`AGENT_KEYBOARD_LAYOUT`]). When it is removed, the keys and buttons it holds are released,
//! its focus leaves, and only then are its clients ended; its outputs, its `wl_seat` global and its
//! place in the layout go once they are freed (`Compositor::remove_seat`).

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Instant;

use smithay::input::keyboard::{KeyboardHandle, XkbConfig, xkb};
use smithay::input::pointer::{CursorImageStatus, PointerHandle};
use smithay::input::{Seat, SeatState};
use smithay::reexports::wayland_server::backend::{ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::utils::{Logical, Point, Serial};
use yuna_layout::SeatId;

use crate::Error;
use crate::popups::ActivePopupGrab;
use crate::state::{ClientState, OWNER_SEAT, State};

/// The layout of every agent's keyboard: the same on every machine and in CI, whatever the
/// `XKB_DEFAULT_*` variables say (ADR-0037).
pub const AGENT_KEYBOARD_LAYOUT: &str = "us";

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
    /// The buttons its pointer holds.
    pub pressed_buttons: Vec<u32>,
    /// Whether its pointer has moved since the seat was made: until then it is nowhere.
    pub pointer_moved: bool,
    /// The origin, in logical coordinates, of the surface under its pointer, as last sent.
    pub pointer_origin: Option<Point<f64, Logical>>,
    /// An agent seat's keymap, compiled as its keyboard's, to find which key types a character.
    pub keymap: Option<xkb::Keymap>,
    /// The clients connected through its socket, or for it through `connect_client_to`, while
    /// they are served.
    pub clients: Vec<ClientId>,
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
            seat_state.new_wl_seat(display, "seat0")
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
        let mut seat = seat_state.new_wl_seat(display, format!("agent-{}", id.0));
        let config = XkbConfig {
            rules: "evdev",
            model: "pc105",
            layout: AGENT_KEYBOARD_LAYOUT,
            variant: "",
            // Some, and no environment names: an empty field would take XKB_DEFAULT_OPTIONS.
            options: Some(String::new()),
        };
        let keyboard = seat
            .add_keyboard_with_context_flags(
                config,
                REPEAT_DELAY_MS,
                REPEAT_RATE,
                xkb::CONTEXT_NO_ENVIRONMENT_NAMES,
            )
            .map_err(|e| Error::Keyboard(e.to_string()))?;
        let pointer = seat.add_pointer();
        crate::text_input::act_as_input_method(&seat);
        let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "evdev",
            "pc105",
            AGENT_KEYBOARD_LAYOUT,
            "",
            Some(String::new()),
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| Error::Keyboard("the agents' keymap does not compile".to_owned()))?;
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
        SeatData {
            id,
            seat,
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
        }
    }
}

/// The seat a client belongs to; the owner's for a client Yuna did not insert, which cannot
/// happen.
pub(crate) fn seat_of_client(client: &Client) -> SeatId {
    client
        .get_data::<ClientState>()
        .map_or(OWNER_SEAT, |state| state.seat)
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
            .map_err(|e| Error::Layout(e.to_string()))?;
        self.next_seat_id += 1;
        self.seats.push(data);
        tracing::info!(seat = id.0, "seat added");
        Ok(id)
    }

    /// Serves a client of `seat` on `stream`, and records it on the seat, for its removal.
    /// Refused for a seat that does not exist.
    pub(crate) fn insert_client(&mut self, seat: SeatId, stream: UnixStream) -> Result<(), Error> {
        if self.seat(seat).is_none() {
            return Err(Error::Refused(format!("no seat {}", seat.0)));
        }
        let data = Arc::new(ClientState::new(seat, self.departed_clients.clone()));
        let client = self
            .display_handle
            .insert_client(stream, data)
            .map_err(|e| Error::EventLoop(e.to_string()))?;
        if let Some(seat) = self.seat_mut(seat) {
            seat.clients.push(client.id());
        }
        Ok(())
    }

    /// The seat of the client `client`, while the display serves it.
    pub(crate) fn seat_of_client_id(&self, client: &ClientId) -> Option<SeatId> {
        self.seats
            .iter()
            .find(|seat| seat.clients.contains(client))
            .map(|seat| seat.id)
    }

    /// The process ids of `seat`'s clients, as the kernel gave them for their sockets.
    pub(crate) fn client_pids(&self, seat: SeatId) -> Vec<i32> {
        let backend = self.display_handle.backend_handle();
        self.seat(seat)
            .into_iter()
            .flat_map(|seat| &seat.clients)
            .filter_map(|client| backend.get_client_credentials(client.clone()).ok())
            .map(|credentials| credentials.pid)
            .collect()
    }

    /// Forgets `client` in its seat's list once the display has ended it.
    pub(crate) fn forget_client(&mut self, client: &ClientId) {
        for seat in &mut self.seats {
            seat.clients.retain(|id| id != client);
        }
    }

    /// The first step of removing agent seat `id`: releases the keys and buttons it holds, ends
    /// its popup grab, and moves its focus off every surface, so that its clients are told.
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
            backend.kill_client(client, DisconnectReason::ConnectionClosed);
        }
        pids
    }

    /// The last step, once its clients are freed: removes its outputs, its place in the layout
    /// and its `wl_seat` global, which is disabled now and destroyed later, as a removed
    /// output's is. Smithay keeps a record of every seat it made, with no way to drop one: a
    /// removed seat's record stays, without its keyboard and pointer.
    pub(crate) fn finish_removing_seat(&mut self, id: SeatId) -> Result<(), Error> {
        let outputs = self.layout.outputs(id).unwrap_or_default();
        for output in outputs {
            if let Some(index) = self.output_index(output) {
                self.drop_output(index);
            }
        }
        if let Err(error) = self.layout.remove_seat(id) {
            tracing::error!(%error, seat = id.0, "the layout keeps a removed seat");
        }
        let Some(index) = self.seats.iter().position(|seat| seat.id == id) else {
            return Ok(());
        };
        let mut removed = self.seats.remove(index);
        if let Some(global) = removed.seat.global() {
            self.display_handle.disable_global::<State>(global.clone());
            self.retired_globals.push((Instant::now(), global));
        }
        removed.seat.remove_keyboard();
        removed.seat.remove_pointer();
        self.mark_layout_changed();
        tracing::info!(seat = id.0, "seat removed");
        Ok(())
    }
}
