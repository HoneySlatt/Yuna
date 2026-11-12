//! `xdg-dialog-v1`: a client says which of its toplevels are dialogs, and which of those are
//! modal.
//!
//! Dialogs already open on their parent through `xdg_toplevel.set_parent`
//! ([ADR-0026](../../../docs/adr/0026-real-clients-in-the-layout.md)); this protocol changes
//! nothing of that yet. Smithay keeps the hint on the toplevel's role, where Yuna reads whether a
//! window is modal; nothing uses it yet.

use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::dialog::{ToplevelDialogHint, XdgDialogHandler};
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgToplevelSurfaceData};
use yuna_layout::WindowId;

use crate::state::State;

impl XdgDialogHandler for State {
    fn dialog_hint_changed(&mut self, toplevel: ToplevelSurface, hint: ToplevelDialogHint) {
        tracing::debug!(surface = %toplevel.wl_surface().id(), ?hint, "dialog hint");
    }
}

impl State {
    /// Whether the client of `window` said it is a modal dialog, and has not taken it back.
    pub(crate) fn is_modal(&self, window: WindowId) -> bool {
        let Some(toplevel) = self
            .windows
            .get(&window)
            .and_then(|surface| self.toplevels.get(surface))
        else {
            return false;
        };
        with_states(toplevel.wl_surface(), |states| {
            let role = states
                .data_map
                .get::<XdgToplevelSurfaceData>()?
                .lock()
                .ok()?;
            Some(role.dialog_hint == ToplevelDialogHint::Modal)
        })
        .unwrap_or(false)
    }
}
