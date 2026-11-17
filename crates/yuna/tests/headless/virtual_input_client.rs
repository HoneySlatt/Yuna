//! A test driver's client, as uwui's `wayland.py` and Pluto's `pluto-input` are: it makes
//! virtual pointers (`zwlr_virtual_pointer_v1`) and virtual keyboards (`zwp_virtual_keyboard_v1`)
//! and sends their requests, reading nothing back but its globals.
//!
//! A [`VirtualDriver`] is a test [`Client`] with these objects on an event queue of their own,
//! over the same connection, as `input_client.rs` does.

use std::fs::File;
use std::io::Write;
use std::os::fd::AsFd;

use smithay::input::keyboard::xkb;
use smithay::reexports::rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::protocol::{wl_pointer, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1,
};
use yuna::Compositor;
use yuna::yuna_layout::SeatId;

use crate::client::Client;

/// `wl_keyboard`'s keymap format for XKB's text form.
const KEYMAP_FORMAT_XKB_V1: u32 = 1;

/// The client's globals and objects.
#[derive(Default)]
pub struct DriverState {
    /// Every global's interface, in the order announced.
    pub globals: Vec<String>,
    /// Each `wl_seat` bound, with the name it gave.
    seats: Vec<(wl_seat::WlSeat, Option<String>)>,
    pointer_manager: Option<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1>,
    keyboard_manager: Option<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1>,
}

/// A connected test driver.
pub struct VirtualDriver {
    pub client: Client,
    queue: EventQueue<DriverState>,
    pub state: DriverState,
    pointers: Vec<zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1>,
    keyboards: Vec<zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1>,
}

impl VirtualDriver {
    /// Connects a driver of `seat`, and binds its globals.
    pub fn connect(compositor: &mut Compositor, seat: SeatId) -> VirtualDriver {
        VirtualDriver::bind(Client::connect_to(compositor, seat), compositor)
    }

    /// As [`VirtualDriver::connect`], for a spy shown every seat's globals
    /// (`Client::connect_seeing_every_seat`).
    pub fn connect_spy(compositor: &mut Compositor, seat: SeatId) -> VirtualDriver {
        VirtualDriver::bind(
            Client::connect_seeing_every_seat(compositor, seat),
            compositor,
        )
    }

    fn bind(client: Client, compositor: &mut Compositor) -> VirtualDriver {
        let queue = client.connection().new_event_queue();
        client
            .connection()
            .display()
            .get_registry(&queue.handle(), ());
        let mut driver = VirtualDriver {
            client,
            queue,
            state: DriverState::default(),
            pointers: Vec::new(),
            keyboards: Vec::new(),
        };
        // The globals, then the seats' names.
        driver.roundtrip(compositor);
        driver.roundtrip(compositor);
        driver
    }

    /// Sends what it wrote, lets the compositor handle it, and reads its events.
    pub fn roundtrip(&mut self, compositor: &mut Compositor) {
        self.client.roundtrip(compositor);
        self.queue
            .dispatch_pending(&mut self.state)
            .expect("the driver's events are read");
    }

    fn seat(&self, name: &str) -> wl_seat::WlSeat {
        self.state
            .seats
            .iter()
            .find(|(_, seat)| seat.as_deref() == Some(name))
            .map(|(seat, _)| seat.clone())
            .unwrap_or_else(|| panic!("no wl_seat named {name}"))
    }

    /// A virtual pointer for the `wl_seat` named `seat`, or for its client's seat with none, as
    /// `wayland.py` makes one; its index.
    pub fn add_pointer(&mut self, seat: Option<&str>) -> usize {
        let seat = seat.map(|name| self.seat(name));
        let manager = self
            .state
            .pointer_manager
            .as_ref()
            .expect("zwlr_virtual_pointer_manager_v1 is offered");
        let pointer = manager.create_virtual_pointer(seat.as_ref(), &self.queue.handle(), ());
        self.pointers.push(pointer);
        self.pointers.len() - 1
    }

    /// Moves pointer `index` to (`x`, `y`) of an extent of `width` × `height`, and ends the
    /// frame.
    pub fn move_to(&self, index: usize, (x, y): (u32, u32), (width, height): (u32, u32)) {
        let pointer = &self.pointers[index];
        pointer.motion_absolute(0, x, y, width, height);
        pointer.frame();
    }

    /// Moves pointer `index` by (`dx`, `dy`), and ends the frame.
    pub fn move_by(&self, index: usize, dx: f64, dy: f64) {
        let pointer = &self.pointers[index];
        pointer.motion(0, dx, dy);
        pointer.frame();
    }

    /// Presses or releases `button` of pointer `index`, and ends the frame.
    pub fn button(&self, index: usize, button: u32, pressed: bool) {
        let state = if pressed {
            wl_pointer::ButtonState::Pressed
        } else {
            wl_pointer::ButtonState::Released
        };
        let pointer = &self.pointers[index];
        pointer.button(0, button, state);
        pointer.frame();
    }

    /// Turns pointer `index`'s wheel by `notches` downwards, as `wayland.py`'s `wheel` does.
    pub fn wheel(&self, index: usize, notches: i32) {
        let pointer = &self.pointers[index];
        pointer.axis_source(wl_pointer::AxisSource::Wheel);
        pointer.axis_discrete(
            0,
            wl_pointer::Axis::VerticalScroll,
            15.0 * f64::from(notches),
            notches,
        );
        pointer.frame();
    }

    /// A virtual keyboard for the `wl_seat` named `seat`, with the keymap of XKB's `layout`
    /// sent through a file, as `pluto-input` sends its own; its index.
    pub fn add_keyboard(&mut self, seat: &str, layout: &str) -> usize {
        self.add_keyboard_claiming(seat, layout, None)
    }

    /// As [`VirtualDriver::add_keyboard`], its keymap's size told as `claimed` bytes when given,
    /// whatever the file holds: a client that lies about it.
    pub fn add_keyboard_claiming(
        &mut self,
        seat: &str,
        layout: &str,
        claimed: Option<u32>,
    ) -> usize {
        let seat = self.seat(seat);
        let manager = self
            .state
            .keyboard_manager
            .as_ref()
            .expect("zwp_virtual_keyboard_manager_v1 is offered");
        let keyboard = manager.create_virtual_keyboard(&seat, &self.queue.handle(), ());
        let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "evdev",
            "pc105",
            layout,
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("the layout compiles")
        .get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
        // A file of its own: tests of one process run at once, and a named file would be shared.
        let fd = memfd_create("yuna-test-keymap", MemfdFlags::CLOEXEC).expect("a memfd");
        let mut file = File::from(fd);
        file.write_all(keymap.as_bytes())
            .expect("the keymap is written");
        file.write_all(&[0]).expect("the keymap is written");
        let size = claimed.unwrap_or_else(|| {
            u32::try_from(keymap.len() + 1).expect("a keymap of a few hundred kB")
        });
        keyboard.keymap(KEYMAP_FORMAT_XKB_V1, file.as_fd(), size);
        self.keyboards.push(keyboard);
        self.keyboards.len() - 1
    }

    /// Presses or releases evdev key `key` on keyboard `index`.
    pub fn key(&self, index: usize, key: u32, pressed: bool) {
        self.keyboards[index].key(0, key, u32::from(pressed));
    }

    /// Destroys pointer `index`, as a driver that leaves does.
    pub fn destroy_pointer(&self, index: usize) {
        self.pointers[index].destroy();
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for DriverState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        state.globals.push(interface.clone());
        match interface.as_str() {
            "wl_seat" => {
                let index = state.seats.len();
                state
                    .seats
                    .push((registry.bind(name, version.min(8), qh, index), None));
            }
            "zwlr_virtual_pointer_manager_v1" => {
                state.pointer_manager = Some(registry.bind(name, 1, qh, ()));
            }
            "zwp_virtual_keyboard_manager_v1" => {
                state.keyboard_manager = Some(registry.bind(name, 1, qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, usize> for DriverState {
    fn event(
        state: &mut Self,
        _: &wl_seat::WlSeat,
        event: wl_seat::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Name { name } = event {
            state.seats[*index].1 = Some(name);
        }
    }
}

delegate_noop!(DriverState: ignore zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1);
delegate_noop!(DriverState: ignore zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1);
delegate_noop!(DriverState: ignore zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1);
delegate_noop!(DriverState: ignore zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1);
