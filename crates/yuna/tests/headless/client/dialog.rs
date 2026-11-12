//! The test client's dialogs: a toplevel's parent (`xdg_toplevel.set_parent`) and its
//! `xdg_dialog_v1` object, modal or not.

use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};

use super::{Client, ClientState};

impl Client {
    /// Makes `window` a dialog of `parent`, as xdg-shell's `set_parent` does.
    pub fn set_parent(&mut self, window: usize, parent: usize) {
        let parent = &self.state.windows[parent].toplevel;
        self.state.windows[window].toplevel.set_parent(Some(parent));
    }

    /// Gets `window`'s `xdg_dialog_v1` object.
    pub fn get_dialog(&mut self, window: usize) -> usize {
        let qh = self.queue.handle();
        let dialog = self
            .state
            .wm_dialog
            .as_ref()
            .expect("xdg_wm_dialog_v1")
            .get_xdg_dialog(&self.state.windows[window].toplevel, &qh, ());
        self.state.dialogs.push(dialog);
        self.state.dialogs.len() - 1
    }

    /// Says the dialog is modal, or no longer.
    pub fn set_modal(&mut self, dialog: usize, modal: bool) {
        let dialog = &self.state.dialogs[dialog];
        if modal {
            dialog.set_modal();
        } else {
            dialog.unset_modal();
        }
    }

    pub fn destroy_dialog(&mut self, dialog: usize) {
        self.state.dialogs[dialog].destroy();
    }
}

impl Dispatch<xdg_wm_dialog_v1::XdgWmDialogV1, ()> for ClientState {
    fn event(
        _: &mut Self,
        _: &xdg_wm_dialog_v1::XdgWmDialogV1,
        _: xdg_wm_dialog_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<xdg_dialog_v1::XdgDialogV1, ()> for ClientState {
    fn event(
        _: &mut Self,
        _: &xdg_dialog_v1::XdgDialogV1,
        _: xdg_dialog_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
