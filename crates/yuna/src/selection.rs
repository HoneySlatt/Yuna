//! The clipboard and the primary selection: `wl_data_device`, the primary selection
//! (`zwp_primary_selection_device_manager_v1`) and `ext-data-control-v1`, on Smithay's handlers.
//! Between Wayland clients, Yuna never touches the data they pass each other.
//!
//! Yuna also holds a seat's selection itself, for one kind of owner: an X11 client of the seat
//! ([`X11Offer`]). It offers the owner's MIME types as the seat's selection, and each read of it
//! goes, with the reader's pipe, to the seat's X11 window manager, which writes the data on its
//! own thread (`xwayland/selections.rs`). And it learns when a Wayland client sets a selection,
//! clears it, or its source goes: it records the MIME types, which X11 is offered. Yuna's fork of
//! Smithay tells of a source that goes (`docs/smithay-patches.md`, patch 6).
//! `docs/ARCHITECTURE.md` (*Selections*) gives the flow; `docs/debt.md`, what falls short.

use std::collections::HashMap;
use std::os::fd::OwnedFd;

use smithay::input::Seat;
use smithay::reexports::rustix::fs::{OFlags, fcntl_setfl};
use smithay::reexports::rustix::pipe::{PipeFlags, pipe_with};
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

use crate::seats::{seat_id, seat_of_client};
use crate::state::{OWNER_SEAT, State};
use crate::xwayland::{MAX_MIME_TYPE_BYTES, MAX_MIME_TYPES, Selection};

/// The selection protocols' globals and Smithay's state for each, and what Wayland clients set
/// as each seat's selections, for X11.
pub(crate) struct Selections {
    data_device: DataDeviceState,
    primary: PrimarySelectionState,
    data_control: DataControlState,
    set_by_clients: HashMap<(SeatId, Selection), ClientSelection>,
}

/// A seat's selection as Wayland clients last set it, for X11: a serial counting each change,
/// and the MIME types of the selection a client set, none once it was cleared, replaced by one
/// Yuna holds, or its source is gone.
#[derive(Debug, Clone, Default)]
pub(crate) struct ClientSelection {
    pub serial: u64,
    pub mime_types: Option<Vec<String>>,
}

/// A seat's selection Yuna holds itself: X11's `selection` of Xwayland `server`, whose manager
/// saw its owner `owner`-th. A Wayland client's read of it goes to that manager, which reads
/// that owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct X11Offer {
    pub server: u64,
    pub selection: Selection,
    pub owner: u64,
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
            set_by_clients: HashMap::new(),
        }
    }

    /// `seat`'s `selection` as Wayland clients last set it, if one ever did.
    pub(crate) fn set_by_client(
        &self,
        seat: SeatId,
        selection: Selection,
    ) -> Option<&ClientSelection> {
        self.set_by_clients.get(&(seat, selection))
    }

    /// Forgets what clients set on `seat`, removed.
    pub(crate) fn forget_seat(&mut self, seat: SeatId) {
        self.set_by_clients.retain(|&(of, _), _| of != seat);
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
fn for_x11(mime_types: Vec<String>) -> Vec<String> {
    mime_types
        .into_iter()
        .filter(|mime_type| mime_type.len() <= MAX_MIME_TYPE_BYTES)
        .take(MAX_MIME_TYPES)
        .collect()
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

    /// Makes `mime_types` `seat`'s `selection`, held by Yuna for `offer`: a client that reads it
    /// is answered through [`SelectionHandler::send_selection`]. A client's selection it replaces
    /// is cancelled, as when another client sets one.
    pub(crate) fn hold_selection(
        &mut self,
        seat: SeatId,
        selection: Selection,
        mime_types: Vec<String>,
        offer: X11Offer,
    ) {
        let Some(seat_data) = self.seat(seat) else {
            return;
        };
        let handle = &self.display_handle;
        match selection {
            Selection::Clipboard => {
                set_data_device_selection(handle, &seat_data.seat, mime_types, offer);
            }
            Selection::Primary => set_primary_selection(handle, &seat_data.seat, mime_types, offer),
        }
        self.client_selection_replaced(seat, selection);
    }

    /// Empties `seat`'s `selection`, whoever holds it; a client's selection is cancelled.
    pub(crate) fn clear_selection(&mut self, seat: SeatId, selection: Selection) {
        let Some(seat_data) = self.seat(seat) else {
            return;
        };
        match selection {
            Selection::Clipboard => {
                clear_data_device_selection(&self.display_handle, &seat_data.seat)
            }
            Selection::Primary => clear_primary_selection(&self.display_handle, &seat_data.seat),
        }
        self.client_selection_replaced(seat, selection);
    }

    /// The selection Yuna holds as `seat`'s `selection`, if it holds one.
    pub(crate) fn held_selection(&self, seat: SeatId, selection: Selection) -> Option<X11Offer> {
        let seat = &self.seat(seat)?.seat;
        match selection {
            Selection::Clipboard => {
                current_data_device_selection_userdata(seat).map(|offer| *offer)
            }
            Selection::Primary => current_primary_selection_userdata(seat).map(|offer| *offer),
        }
    }

    /// Asks the client whose selection is `seat`'s `selection` to write it as `mime_type` into a
    /// new pipe, and gives its read end, non-blocking; none when no client's selection is there,
    /// it lacks the type, or no pipe can be made. Its write end goes to the client: a blocking
    /// one, as a client writes it.
    pub(crate) fn read_client_selection(
        &mut self,
        seat: SeatId,
        selection: Selection,
        mime_type: String,
    ) -> Option<OwnedFd> {
        let seat_data = self.seat(seat)?;
        let (read, write) = match pipe_with(PipeFlags::CLOEXEC) {
            Ok(ends) => ends,
            Err(error) => {
                tracing::warn!(%error, "no pipe for a selection: X11's request is refused");
                return None;
            }
        };
        fcntl_setfl(&read, OFlags::NONBLOCK).ok()?;
        let gone = match selection {
            Selection::Clipboard => {
                match request_data_device_client_selection(&seat_data.seat, mime_type, write) {
                    Ok(()) => return Some(read),
                    Err(error) => matches!(error, data_device::SelectionRequestError::NoSelection),
                }
            }
            Selection::Primary => {
                match request_primary_client_selection(&seat_data.seat, mime_type, write) {
                    Ok(()) => return Some(read),
                    Err(error) => {
                        matches!(error, primary_selection::SelectionRequestError::NoSelection)
                    }
                }
            }
        };
        if gone {
            self.client_selection_gone(seat, selection);
        }
        None
    }

    /// A client's selection of `seat` is replaced by one Yuna holds, or by none: Smithay has
    /// cancelled its source. X11 already knows: what replaced it came from X11.
    fn client_selection_replaced(&mut self, seat: SeatId, selection: Selection) {
        if let Some(set) = self.selections.set_by_clients.get_mut(&(seat, selection)) {
            set.mime_types = None;
        }
    }

    /// A client's selection of `seat` turns out gone though Smithay did not say, a data-control
    /// source destroyed: X11 is told at the round's end.
    fn client_selection_gone(&mut self, seat: SeatId, selection: Selection) {
        let Some(set) = self.selections.set_by_clients.get_mut(&(seat, selection)) else {
            return;
        };
        if set.mime_types.take().is_some() {
            set.serial += 1;
            self.x11_wayland_selection_changed(seat, selection);
        }
    }
}

/// Yuna holds a seat's selection for an X11 owner ([`X11Offer`]): a read of it goes to that
/// owner's manager. A client that sets a selection is recorded, for X11.
impl SelectionHandler for State {
    type SelectionUserData = X11Offer;

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
        let selection = selection_of(target);
        // The source lives: Smithay calls this as its client sets it.
        let mime_types = source.map(|source| for_x11(source.mime_types()));
        let set = self
            .selections
            .set_by_clients
            .entry((seat, selection))
            .or_default();
        set.serial += 1;
        set.mime_types = mime_types;
        self.x11_wayland_selection_changed(seat, selection);
    }

    /// A client reads a selection Yuna holds: its pipe goes to the X11 owner's manager, and is
    /// never read nor written here.
    fn send_selection(
        &mut self,
        _target: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<State>,
        offer: &X11Offer,
    ) {
        self.x11_send_selection(*offer, mime_type, fd);
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
