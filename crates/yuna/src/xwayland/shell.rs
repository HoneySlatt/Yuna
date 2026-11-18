//! `xwayland-shell-v1`, Yuna's own handler on wayland-server, version 1: how Xwayland pairs each
//! X11 window with the Wayland surface it draws it into
//! ([ADR-0066](../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md)).
//!
//! Only a seat's Xwayland sees the global ([`ClientState::xwayland`]). For each window it maps,
//! Xwayland gives a new surface the role (`get_xwayland_surface`), sets a serial on it
//! (`set_serial`), applied at the surface's commit, and sends the same serial to the window's
//! manager in a `WL_SURFACE_SERIAL` client message (`manager/x11_events.rs`). The two meet in
//! either order (`windows.rs`). A surface that has a role already is Xwayland's own mistake, and
//! the only one posted to it as a protocol error: what one X11 client does never ends Xwayland's
//! connection, and with it every X11 program of the seat (ADR-0067). A serial of 0, which the
//! protocol calls invalid, or a second serial on one surface is logged and ignored.
//!
//! Smithay's own handler (`src/wayland/xwayland_shell.rs`, MIT) was read: the serial kept in
//! the surface's cached state until its commit is its way too.

use std::sync::{Mutex, PoisonError};

use smithay::reexports::wayland_protocols::xwayland::shell::v1::server::{
    xwayland_shell_v1::{self, XwaylandShellV1},
    xwayland_surface_v1::{self, XwaylandSurfaceV1},
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::wayland::compositor::{Cacheable, get_role, give_role, with_states};

use crate::state::{ClientState, State};

/// The role Xwayland's surfaces have.
pub(crate) const SURFACE_ROLE: &str = "xwayland_surface_v1";

/// The version Yuna offers: the protocol's only one.
const VERSION: u32 = 1;

/// The global's data.
pub(crate) struct ShellGlobal;

/// A surface's serial, double-buffered: set by `set_serial`, applied at its commit.
#[derive(Debug, Default, Clone, Copy)]
struct Serial(Option<u64>);

impl Cacheable for Serial {
    fn commit(&mut self, _dh: &DisplayHandle) -> Serial {
        *self
    }

    fn merge_into(self, into: &mut Serial, _dh: &DisplayHandle) {
        *into = self;
    }
}

/// The serial a surface was paired by, once its commit applied one: a surface is paired once.
#[derive(Default)]
struct Paired(Mutex<Option<u64>>);

/// Creates the global.
pub(crate) fn shell_global(display: &DisplayHandle) {
    display.create_global::<State, XwaylandShellV1, _>(VERSION, ShellGlobal);
}

/// The Xwayland a client is, if it is one.
fn server_of(client: &Client) -> Option<u64> {
    client.get_data::<ClientState>()?.xwayland
}

impl GlobalDispatch<XwaylandShellV1, ShellGlobal> for State {
    fn bind(
        _state: &mut State,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<XwaylandShellV1>,
        _global: &ShellGlobal,
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, ());
    }

    /// Xwayland's alone, as the protocol asks: another client neither sees it nor can bind it.
    fn can_view(client: Client, _global: &ShellGlobal) -> bool {
        server_of(&client).is_some()
    }
}

impl Dispatch<XwaylandShellV1, ()> for State {
    fn request(
        _state: &mut State,
        _client: &Client,
        shell: &XwaylandShellV1,
        request: xwayland_shell_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            xwayland_shell_v1::Request::GetXwaylandSurface { id, surface } => {
                if get_role(&surface).is_some() || give_role(&surface, SURFACE_ROLE).is_err() {
                    shell.post_error(
                        xwayland_shell_v1::Error::Role,
                        "the surface has a role already",
                    );
                    return;
                }
                data_init.init(id, surface);
            }
            // Its surfaces keep their role.
            xwayland_shell_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<XwaylandSurfaceV1, WlSurface> for State {
    fn request(
        _state: &mut State,
        _client: &Client,
        _xwayland_surface: &XwaylandSurfaceV1,
        request: xwayland_surface_v1::Request,
        surface: &WlSurface,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            xwayland_surface_v1::Request::SetSerial {
                serial_lo,
                serial_hi,
            } => {
                if !surface.is_alive() {
                    return;
                }
                let serial = u64::from(serial_lo) | (u64::from(serial_hi) << 32);
                with_states(surface, |states| {
                    states.cached_state.get::<Serial>().pending().0 = Some(serial);
                });
            }
            // A pairing made stays.
            xwayland_surface_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl State {
    /// A commit of `surface`, which has Xwayland's role: a serial it applies for the first time
    /// pairs it with its window, or waits for the window.
    pub(crate) fn xwayland_surface_committed(&mut self, surface: &WlSurface) {
        let Some(server) = surface.client().as_ref().and_then(server_of) else {
            return;
        };
        let (serial, before) = with_states(surface, |states| {
            let serial = states.cached_state.get::<Serial>().current().0;
            let paired = states.data_map.get_or_insert_threadsafe(Paired::default);
            let mut paired = paired.0.lock().unwrap_or_else(PoisonError::into_inner);
            let before = *paired;
            if before.is_none() {
                *paired = serial;
            }
            (serial, before)
        });
        match (serial, before) {
            (Some(0), None) => {
                tracing::debug!("Xwayland gave a surface the serial 0, which pairs nothing");
            }
            (Some(serial), None) => self.x11_surface_serial(server, surface.clone(), serial),
            (Some(serial), Some(before)) if serial != before => {
                tracing::debug!("Xwayland gave a paired surface a second serial; it is ignored");
            }
            _ => {}
        }
    }

    /// `surface`, which had Xwayland's role, is destroyed: looked for in every Xwayland, since a
    /// destroyed surface may no longer name its client.
    pub(crate) fn xwayland_surface_destroyed(&mut self, surface: &WlSurface) {
        self.x11_surface_destroyed(surface);
    }
}
