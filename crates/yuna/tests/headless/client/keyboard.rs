//! The test client's keyboard: which of its surfaces the owner's seat's keyboard is on, and the
//! serial of the `enter` that put it there, which activation tokens and popup grabs carry.

use wayland_client::protocol::{wl_keyboard, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use yuna::Compositor;

use super::{Client, ClientState};

/// Where the keyboard is, as the client was told.
#[derive(Debug, Default)]
pub struct KeyboardFocus {
    /// The surface the keyboard last entered and has not left.
    pub surface: Option<wl_surface::WlSurface>,
    /// The serial of the last `enter`.
    pub enter_serial: Option<u32>,
}

impl Client {
    /// Gets the seat's keyboard, and reads where it is: a client that has the focus is sent an
    /// `enter` at once.
    pub fn bind_keyboard(&mut self, compositor: &mut Compositor) {
        let qh = self.queue.handle();
        let keyboard = self
            .state
            .seat
            .as_ref()
            .expect("wl_seat")
            .get_keyboard(&qh, ());
        self.state.keyboard = Some(keyboard);
        self.roundtrip(compositor);
    }

    pub fn keyboard_focus(&self) -> &KeyboardFocus {
        &self.state.keyboard_focus
    }

    /// The serial of the last keyboard `enter` this client got.
    pub fn enter_serial(&self) -> u32 {
        self.state
            .keyboard_focus
            .enter_serial
            .expect("a keyboard enter")
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let focus = &mut state.keyboard_focus;
        match event {
            wl_keyboard::Event::Enter {
                serial, surface, ..
            } => {
                focus.surface = Some(surface);
                focus.enter_serial = Some(serial);
            }
            wl_keyboard::Event::Leave { surface, .. }
                if focus.surface.as_ref() == Some(&surface) =>
            {
                focus.surface = None;
            }
            // The keymap's file descriptor closes as the event is dropped.
            _ => {}
        }
    }
}
