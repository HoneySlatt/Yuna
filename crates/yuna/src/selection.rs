//! The clipboard and the primary selection: `wl_data_device`, the primary selection
//! (`zwp_primary_selection_device_manager_v1`) and `ext-data-control-v1`, on Smithay's handlers.
//! Yuna owns no selection of its own and never touches the data clients pass each other.
//! `docs/ARCHITECTURE.md` (*Selections*) gives the flow; `docs/debt.md`, what falls short.

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
    /// Offers the owner seat's clipboard and primary selection to the client of `focused`, the
    /// surface about to get its keyboard focus; with none, to no client. A client that gets them
    /// is told what they hold. Who may set them is the keyboard focus's business, in Smithay.
    pub(crate) fn give_selections_to(&self, focused: Option<&WlSurface>) {
        let client = focused.and_then(Resource::client);
        set_data_device_focus(&self.display_handle, &self.seat, client.clone());
        set_primary_focus(&self.display_handle, &self.seat, client);
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
