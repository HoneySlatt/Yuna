//! The test client's `xdg-activation-v1`: tokens asked with or without a serial and a surface,
//! and activation requests.

use wayland_client::protocol::wl_surface;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};
use yuna::Compositor;

use super::{Client, ClientState};

impl Client {
    /// Asks an activation token, with the seat's `serial` and for `window`'s surface when given,
    /// and waits for it.
    pub fn request_token(
        &mut self,
        compositor: &mut Compositor,
        serial: Option<u32>,
        window: Option<usize>,
    ) -> String {
        let surface = window.map(|window| self.state.windows[window].surface.clone());
        self.request_token_for_surface(compositor, serial, surface.as_ref())
    }

    /// Asks an activation token, with the seat's `serial` and for `surface` when given, whatever
    /// the surface is (a popup's), and waits for it.
    pub fn request_token_for_surface(
        &mut self,
        compositor: &mut Compositor,
        serial: Option<u32>,
        surface: Option<&wl_surface::WlSurface>,
    ) -> String {
        let qh = self.queue.handle();
        let index = self.state.tokens.len();
        self.state.tokens.push(None);
        let token = self
            .state
            .activation
            .as_ref()
            .expect("xdg_activation_v1")
            .get_activation_token(&qh, index);
        if let Some(serial) = serial {
            token.set_serial(serial, self.state.seat.as_ref().expect("wl_seat"));
        }
        if let Some(surface) = surface {
            token.set_surface(surface);
        }
        token.commit();
        self.roundtrip(compositor);
        token.destroy();
        self.state.tokens[index].clone().expect("a token")
    }

    /// Asks that `window` be activated with `token`.
    pub fn activate(&self, token: &str, window: usize) {
        self.activate_surface(token, &self.state.windows[window].surface);
    }

    /// Asks that the window of `surface` be activated with `token`, whatever the surface is.
    pub fn activate_surface(&self, token: &str, surface: &wl_surface::WlSurface) {
        self.state
            .activation
            .as_ref()
            .expect("xdg_activation_v1")
            .activate(token.to_owned(), surface);
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, usize> for ClientState {
    fn event(
        state: &mut Self,
        _: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            state.tokens[*index] = Some(token);
        }
    }
}

impl Dispatch<xdg_activation_v1::XdgActivationV1, ()> for ClientState {
    fn event(
        _: &mut Self,
        _: &xdg_activation_v1::XdgActivationV1,
        _: xdg_activation_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
