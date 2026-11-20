//! The virtual keyboard and pointer, `zwp_virtual_keyboard_v1` and `zwlr_virtual_pointer_v1`, on
//! Smithay's modules: input a client makes for its own seat, as sway gives it to the test drivers
//! of the owner's other projects ([ADR-0058](../../../docs/adr/0058-what-yuna-takes-from-the-owners-other-projects.md),
//! [ADR-0078](../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md),
//! [ADR-0089](../../../docs/adr/0089-yuna-for-the-owners-other-projects.md)).
//!
//! Only the clients of agents' seats see the two globals; the owner's seat never takes their
//! input. A device acts on its client's own seat, the one its client connected through: a device
//! made for another seat's `wl_seat`, which no real client holds, does nothing. Its seat is read
//! when it is made, and kept while it lives, so that the keys and buttons Smithay releases when
//! its client dies still reach that seat. Each client keeps
//! [`MAX_VIRTUAL_DEVICES_PER_CLIENT`] devices that act; one made past it does nothing. A key
//! press past the keys a seat may hold, or of a keycode the device's keymap does not have, and a
//! button press past the buttons it may hold, are dropped the same way, with a debug log
//! ([`MAX_HELD_KEYS`](crate::input::MAX_HELD_KEYS),
//! [`MAX_HELD_BUTTONS`](crate::input::MAX_HELD_BUTTONS)).
//!
//! A virtual keyboard's keys are read with its own keymap: the seat's keyboard switches to it
//! before them, its clients told of the new keymap, and switches back to the seat's own before
//! Yuna's keys (`State::key`); one that sent `no_keymap` uses the seat's own. Its `modifiers`
//! request sets the modifiers under its keymap, and those it set are released when it goes. The
//! seat's keyboard has one state for all its devices, Yuna's own keys included. A
//! virtual pointer's absolute motion is a fraction of its seat's outputs together, or of the
//! output it names; its relative motion moves from where the pointer is, or from the seat's first
//! output's corner before it ever moved, and stops at the edge of the seat's outputs, as
//! wlroots' cursor does. Its buttons and scrolls go where Yuna's own go
//! (`input.rs`), a press focusing the window under it.

use std::collections::HashMap;

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, Event, InputEvent, KeyboardKeyEvent, PointerAxisEvent,
    PointerButtonEvent, PointerMotionEvent,
};
use smithay::input::pointer::AxisFrame;
use smithay::utils::{Logical, Point};
use smithay::wayland::virtual_keyboard::{
    VirtualKeyboardBackend, VirtualKeyboardDevice, VirtualKeyboardHandler,
    VirtualKeyboardManagerState, VirtualKeyboardSpecialEvent,
};
use smithay::wayland::virtual_pointer::{
    VirtualPointerBackend, VirtualPointerDevice, VirtualPointerHandler, VirtualPointerManagerState,
};
use wayland_server::backend::ClientId;
use wayland_server::protocol::wl_seat::WlSeat;
use wayland_server::{Client, DisplayHandle};
use yuna_layout::SeatId;

use crate::seats::seat_of_wl_seat;
use crate::state::{ClientState, OWNER_SEAT, State};

/// The most virtual devices, keyboards and pointers together, one client keeps acting: a test
/// driver holds one of each
/// ([ADR-0131](../../../docs/adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)).
pub const MAX_VIRTUAL_DEVICES_PER_CLIENT: usize = 8;

/// The two protocols' globals, and the seat of each device that acts.
pub(crate) struct VirtualInputState {
    _keyboards: VirtualKeyboardManagerState,
    _pointers: VirtualPointerManagerState,
    keyboards: HashMap<VirtualKeyboardDevice, Acting>,
    pointers: HashMap<VirtualPointerDevice, Acting>,
    /// The modifiers each keyboard last set through its `modifiers` request, depressed, latched
    /// and locked: released when it goes.
    modifiers: HashMap<VirtualKeyboardDevice, (u32, u32, u32)>,
}

/// A device that acts: its seat and its client.
struct Acting {
    seat: SeatId,
    client: ClientId,
}

impl VirtualInputState {
    /// Creates both globals, seen by the clients of agents' seats alone.
    pub(crate) fn new(display: &DisplayHandle) -> VirtualInputState {
        VirtualInputState {
            _keyboards: VirtualKeyboardManagerState::new::<State, _>(display, of_an_agents_seat),
            _pointers: VirtualPointerManagerState::new::<State, _>(display, of_an_agents_seat),
            keyboards: HashMap::new(),
            pointers: HashMap::new(),
            modifiers: HashMap::new(),
        }
    }

    fn devices_of(&self, client: &ClientId) -> usize {
        let of_client = |acting: &Acting| acting.client == *client;
        self.keyboards.values().filter(|a| of_client(a)).count()
            + self.pointers.values().filter(|a| of_client(a)).count()
    }

    /// Whether a device of `client`, made for `wl_seat` or for its client's seat when none,
    /// acts, and on which seat: its client's own, an agent's, while the client keeps fewer than
    /// [`MAX_VIRTUAL_DEVICES_PER_CLIENT`].
    fn acting(&self, client: Option<Client>, wl_seat: Option<&WlSeat>) -> Option<Acting> {
        let client = client?;
        let seat = client.get_data::<ClientState>()?.seat;
        if seat == OWNER_SEAT {
            return None;
        }
        if let Some(wl_seat) = wl_seat
            && seat_of_wl_seat(wl_seat) != Some(seat)
        {
            tracing::debug!(
                seat = seat.0,
                "a virtual device for another seat does nothing"
            );
            return None;
        }
        if self.devices_of(&client.id()) >= MAX_VIRTUAL_DEVICES_PER_CLIENT {
            tracing::debug!(
                seat = seat.0,
                "a client's virtual device past its cap does nothing"
            );
            return None;
        }
        Some(Acting {
            seat,
            client: client.id(),
        })
    }
}

/// Whether `client` belongs to an agent's seat; none that Yuna did not insert does.
fn of_an_agents_seat(client: &Client) -> bool {
    client
        .get_data::<ClientState>()
        .is_some_and(|state| state.seat != OWNER_SEAT)
}

impl VirtualKeyboardHandler for State {
    fn process_virtual_keyboard_event(&mut self, event: InputEvent<VirtualKeyboardBackend>) {
        match event {
            InputEvent::DeviceAdded { device } => {
                if let Some(acting) = self
                    .virtual_input
                    .acting(device.client(), Some(device.wl_seat()))
                {
                    self.virtual_input.keyboards.insert(device, acting);
                }
            }
            InputEvent::DeviceRemoved { device } => {
                let acting = self.virtual_input.keyboards.remove(&device);
                let set = self.virtual_input.modifiers.remove(&device);
                if let (Some(acting), Some(set)) = (acting, set) {
                    self.release_modifiers(acting.seat, &device, set);
                }
            }
            InputEvent::Keyboard { event } => {
                let device = event.device();
                let Some(seat) = self.virtual_input.keyboards.get(&device).map(|a| a.seat) else {
                    return;
                };
                let result = self.use_device_keymap(seat, &device).and_then(|()| {
                    self.send_key(seat, device.source(), event.key_code(), event.state())
                });
                if let Err(error) = result {
                    tracing::debug!(%error, seat = seat.0, "a virtual key was dropped");
                }
            }
            InputEvent::Special(VirtualKeyboardSpecialEvent::Modifiers {
                device,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            }) => {
                let Some(seat) = self.virtual_input.keyboards.get(&device).map(|a| a.seat) else {
                    return;
                };
                let set = (mods_depressed, mods_latched, mods_locked);
                self.virtual_input.modifiers.insert(device.clone(), set);
                let result = self.use_device_keymap(seat, &device).and_then(|()| {
                    let keyboard = self.agent_keyboard(seat)?;
                    keyboard.with_xkb_state(self, |mut context| {
                        context.set_modifier_mask(mods_depressed, mods_latched, mods_locked, group);
                    });
                    Ok(())
                });
                if let Err(error) = result {
                    tracing::debug!(%error, seat = seat.0, "virtual modifiers were dropped");
                }
            }
            // A new keymap is switched to at the device's next key.
            _ => {}
        }
    }
}

impl State {
    /// Switches `seat`'s keyboard to the keymap of `device`, or to the seat's own for a device
    /// that sent `no_keymap`, whatever keymap another device left.
    fn use_device_keymap(
        &mut self,
        seat: SeatId,
        device: &VirtualKeyboardDevice,
    ) -> Result<(), crate::Error> {
        let keymap = match device.keymap() {
            Some(keymap) => keymap,
            None => self
                .seat(seat)
                .map(|data| data.own_keymap.clone())
                .ok_or_else(|| crate::Error::Refused(format!("no seat {}", seat.0)))?,
        };
        self.use_keymap(seat, &keymap)
    }

    /// Releases the modifiers `device` set, `set` as depressed, latched and locked masks of its
    /// keymap, when it goes: the others' stay.
    fn release_modifiers(
        &mut self,
        seat: SeatId,
        device: &VirtualKeyboardDevice,
        (depressed, latched, locked): (u32, u32, u32),
    ) {
        let result = self.use_device_keymap(seat, device).and_then(|()| {
            let keyboard = self.agent_keyboard(seat)?;
            let now = keyboard.modifier_state().serialized;
            keyboard.with_xkb_state(self, |mut context| {
                context.set_modifier_mask(
                    now.depressed & !depressed,
                    now.latched & !latched,
                    now.locked & !locked,
                    now.layout_effective,
                );
            });
            Ok(())
        });
        if let Err(error) = result {
            tracing::debug!(%error, seat = seat.0, "a virtual keyboard's modifiers stay");
        }
    }

    /// The box of `seat`'s outputs together, or of `output` when the device names one of them,
    /// in the layout's logical coordinates: what a virtual pointer's absolute motion spans.
    fn absolute_motion_box(
        &self,
        seat: SeatId,
        device: &VirtualPointerDevice,
    ) -> Option<(Point<f64, Logical>, Point<f64, Logical>)> {
        let named = device.output();
        let rects: Vec<_> = self
            .outputs
            .iter()
            .filter(|output| output.seat == seat)
            .filter(|output| named.as_ref().is_none_or(|named| *named == output.output))
            .map(|output| output.rect)
            .collect();
        let left = rects.iter().map(|rect| rect.x).reduce(f64::min)?;
        let top = rects.iter().map(|rect| rect.y).reduce(f64::min)?;
        let right = rects.iter().map(|rect| rect.right()).reduce(f64::max)?;
        let bottom = rects.iter().map(|rect| rect.bottom()).reduce(f64::max)?;
        Some((Point::from((left, top)), Point::from((right, bottom))))
    }
}

impl VirtualPointerHandler for State {
    fn process_virtual_pointer_event(&mut self, event: InputEvent<VirtualPointerBackend>) {
        let result = match event {
            InputEvent::DeviceAdded { device } => {
                if let Some(acting) = self.virtual_input.acting(device.client(), device.wl_seat()) {
                    self.virtual_input.pointers.insert(device, acting);
                }
                Ok(())
            }
            InputEvent::DeviceRemoved { device } => {
                self.virtual_input.pointers.remove(&device);
                Ok(())
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let device = event.device();
                let Some(seat) = self.virtual_input.pointers.get(&device).map(|a| a.seat) else {
                    return;
                };
                let Some((corner, end)) = self.absolute_motion_box(seat, &device) else {
                    return;
                };
                let (width, height) = (end.x - corner.x, end.y - corner.y);
                // The last unit of the box is its inside's: x_extent itself would be on the
                // next output, or on none.
                let x = (corner.x + event.x_transformed(1) * width).min(end.x - 1.0);
                let y = (corner.y + event.y_transformed(1) * height).min(end.y - 1.0);
                self.pointer_move_to(seat, Point::from((x, y)))
            }
            InputEvent::PointerMotion { event } => {
                let device = event.device();
                let Some(seat) = self.virtual_input.pointers.get(&device).map(|a| a.seat) else {
                    return;
                };
                self.pointer_location(seat).and_then(|location| {
                    let moved = location + Point::from((event.delta_x(), event.delta_y()));
                    match self.closest_on_seat(seat, moved) {
                        Some(on_seat) => self.pointer_move_to(seat, on_seat),
                        None => Ok(()),
                    }
                })
            }
            InputEvent::PointerButton { event } => {
                let device = event.device();
                let Some(seat) = self.virtual_input.pointers.get(&device).map(|a| a.seat) else {
                    return;
                };
                let pressed = event.state() == smithay::backend::input::ButtonState::Pressed;
                self.pointer_button(seat, event.button_code(), pressed)
                    .map(|_| ())
            }
            InputEvent::PointerAxis { event } => {
                let device = event.device();
                let Some(seat) = self.virtual_input.pointers.get(&device).map(|a| a.seat) else {
                    return;
                };
                self.agent_pointer(seat).map(|pointer| {
                    let mut frame = AxisFrame::new(self.input_time()).source(event.source());
                    for axis in [Axis::Horizontal, Axis::Vertical] {
                        if let Some(amount) = event.amount(axis) {
                            frame = if amount == 0.0 {
                                frame.stop(axis)
                            } else {
                                frame.value(axis, amount)
                            };
                        }
                        if let Some(v120) = event.amount_v120(axis) {
                            frame = frame.v120(axis, v120 as i32);
                        }
                    }
                    self.send_axis(seat, &pointer, frame);
                })
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            tracing::debug!(%error, "a virtual pointer's request was dropped");
        }
    }
}
