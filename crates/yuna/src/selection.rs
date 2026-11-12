//! The clipboard and the primary selection: `wl_data_device`, the primary selection
//! (`zwp_primary_selection_device_manager_v1`) and `ext-data-control-v1`, on Smithay's handlers.
//!
//! The selections follow the owner seat's keyboard focus: only the client that holds it may set
//! them, and it is offered each one. Smithay checks the focus, not the serial a request names.
//! An `ext-data-control-v1` client, such as a clipboard manager, is offered each selection too,
//! and sets them without the focus; every client may bind it for now (`docs/debt.md`). A
//! selection lasts as long as its source: when the source goes, or its client leaves, the
//! selection empties, and the client with the focus and the data-control clients are told.
//!
//! Yuna owns no selection of its own. Data goes from one client to the other through the file
//! descriptor that the receiving client sends with its request: Yuna passes it on to the source's
//! client and never reads, writes or waits on it.
//!
//! Drag and drop needs the pointer, which comes with M1.5. Until then Smithay refuses every drag:
//! a `start_drag` must answer a pointer or touch grab, and the seat has neither.

use smithay::input::Seat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus,
};
use smithay::wayland::selection::ext_data_control::{DataControlHandler, DataControlState};
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
};

use crate::state::State;

/// The selection protocols' globals and Smithay's state for each.
pub(crate) struct Selections {
    data_device: DataDeviceState,
    primary: PrimarySelectionState,
    data_control: DataControlState,
}

impl Selections {
    /// Creates the three globals. Data control reaches the primary selection too, and every
    /// client sees it.
    pub(crate) fn new(display_handle: &DisplayHandle) -> Selections {
        let data_device = DataDeviceState::new::<State>(display_handle);
        let primary = PrimarySelectionState::new::<State>(display_handle);
        let data_control =
            DataControlState::new::<State, _>(display_handle, Some(&primary), |_| true);
        Selections {
            data_device,
            primary,
            data_control,
        }
    }
}

impl State {
    /// Gives the clipboard and the primary selection of `seat` to the client of `focused`, the
    /// surface with its keyboard focus; with none, no client may set them or is offered them.
    /// A client that gets them is told what they hold.
    pub(crate) fn give_selections_to(&mut self, seat: &Seat<State>, focused: Option<&WlSurface>) {
        let client = focused.and_then(Resource::client);
        set_data_device_focus(&self.display_handle, seat, client.clone());
        set_primary_focus(&self.display_handle, seat, client);
    }
}

/// Yuna sets no selection of its own, so no client reads one from it, and it needs no word of the
/// clients' selections: the defaults do nothing.
impl SelectionHandler for State {
    type SelectionUserData = ();
}

/// Smithay calls it only for a drag that a pointer or touch grab started, which cannot happen
/// before the pointer comes (M1.5); its default would cancel the drag.
impl WaylandDndGrabHandler for State {}

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
