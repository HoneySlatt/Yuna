//! A client with a seat's input, for tests: it binds every `wl_seat`, takes the pointer and the
//! keyboard of the one a test names, and records each event in order. It decodes the keys it
//! gets through the keymaps it is sent, as an application does, so that a test reads the text
//! typed. It sets cursor shapes (`cursor-shape-v1`) and drags (`wl_data_device`). It holds a
//! text field (`text-input-v3`) of that seat from the start, as an application binds one before
//! its window has the keyboard, and applies what each `done` gives it.
//!
//! An [`InputClient`] is a test [`Client`] with these objects on an event queue of their own,
//! over the same connection, as `selection_client.rs` does: the windows and the input are one
//! Wayland client, which is what a seat's focus looks at.

use std::fs::File;
use std::os::unix::fs::FileExt;

use smithay::input::keyboard::xkb;
use wayland_client::protocol::{
    wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source, wl_keyboard, wl_pointer,
    wl_registry, wl_seat, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, event_created_child};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1,
};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};
use yuna::Compositor;
use yuna::yuna_layout::SeatId;

use crate::client::Client;

pub use wp_cursor_shape_device_v1::Shape;

/// One event of the seat's pointer, keyboard or data device, in the order it came.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    PointerEnter {
        surface: wl_surface::WlSurface,
        x: f64,
        y: f64,
        serial: u32,
    },
    PointerLeave {
        surface: wl_surface::WlSurface,
    },
    Motion {
        x: f64,
        y: f64,
    },
    Button {
        button: u32,
        pressed: bool,
        serial: u32,
    },
    /// A scroll's distance, horizontal or vertical.
    Axis {
        horizontal: bool,
        value: f64,
    },
    /// A wheel's steps, in 120ths.
    Value120 {
        horizontal: bool,
        steps: i32,
    },
    Frame,
    KeyboardEnter {
        surface: wl_surface::WlSurface,
        serial: u32,
        /// The keys held, as XKB keycodes minus 8.
        keys: Vec<u32>,
    },
    KeyboardLeave {
        surface: wl_surface::WlSurface,
    },
    Key {
        key: u32,
        pressed: bool,
        serial: u32,
    },
    Modifiers {
        depressed: u32,
    },
    Keymap,
    DragEnter {
        surface: wl_surface::WlSurface,
        serial: u32,
    },
    DragMotion,
    DragLeave,
    Drop,
}

/// One event of the client's text field, in the order it came.
#[derive(Debug, Clone, PartialEq)]
pub enum TextEvent {
    Enter(wl_surface::WlSurface),
    Leave(wl_surface::WlSurface),
    Preedit {
        text: Option<String>,
        begin: i32,
        end: i32,
    },
    Commit(Option<String>),
    Done(u32),
}

/// What a drag's source was told.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DragSource {
    pub cancelled: bool,
    pub drop_performed: bool,
}

/// A test client with a seat's pointer and keyboard.
pub struct InputClient {
    /// Its windows' side.
    pub client: Client,
    queue: EventQueue<InputState>,
    pub state: InputState,
}

/// What the client's input objects hold and received.
#[derive(Default)]
pub struct InputState {
    /// Every `wl_seat`, with the name it gave and whether it has a pointer.
    seats: Vec<(wl_seat::WlSeat, Option<String>, bool)>,
    cursor_shapes: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    data_device_manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    cursor_shape: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    data_device: Option<wl_data_device::WlDataDevice>,
    text_inputs: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    /// The `commit` requests the client sent its text field, which each `done` names.
    text_commits: u32,
    /// What the text field was sent since its last `done`.
    pending_preedit: Option<String>,
    pending_commit: Option<String>,
    /// The events of its text field.
    pub text_events: Vec<TextEvent>,
    /// The text being composed in its text field, as the last `done` applied it.
    pub preedit: Option<String>,
    /// The last offer a drag entered with.
    drag_offer: Option<wl_data_offer::WlDataOffer>,
    pub events: Vec<InputEvent>,
    /// The interface of every global the client was told of.
    pub globals: Vec<String>,
    /// The keymap last sent, and its state with the modifiers last sent.
    xkb: Option<(xkb::Keymap, xkb::State)>,
    /// What the keys pressed type, through the keymap they came with.
    pub typed: String,
    /// The text the client took, in order: what its keys typed, and what its text field was
    /// given as committed.
    pub received: String,
    pub sources: Vec<DragSource>,
}

impl InputClient {
    /// Connects a client of `seat`, and takes the pointer and the keyboard of the `wl_seat`
    /// named `seat_name`: its own seat's (`agent-<id>`), or another's, as a client that would
    /// listen to another seat does.
    pub fn connect(compositor: &mut Compositor, seat: SeatId, seat_name: &str) -> InputClient {
        let client = Client::connect_to(compositor, seat);
        InputClient::bind_input(client, compositor, seat_name)
    }

    /// As [`InputClient::connect`], for a spy: a client of `seat` shown every seat's globals,
    /// which may take another seat's `wl_seat` (`Client::connect_seeing_every_seat`).
    pub fn connect_spy(compositor: &mut Compositor, seat: SeatId, seat_name: &str) -> InputClient {
        let client = Client::connect_seeing_every_seat(compositor, seat);
        InputClient::bind_input(client, compositor, seat_name)
    }

    fn bind_input(client: Client, compositor: &mut Compositor, seat_name: &str) -> InputClient {
        let queue = client.connection().new_event_queue();
        client
            .connection()
            .display()
            .get_registry(&queue.handle(), ());
        let mut input = InputClient {
            client,
            queue,
            state: InputState::default(),
        };
        // The seats, then their names.
        input.roundtrip(compositor);
        input.roundtrip(compositor);
        let qh = input.queue.handle();
        let state = &mut input.state;
        let (wl_seat, has_pointer) = state
            .seats
            .iter()
            .find(|(_, name, _)| name.as_deref() == Some(seat_name))
            .map(|(seat, _, pointer)| (seat.clone(), *pointer))
            .unwrap_or_else(|| panic!("no wl_seat named {seat_name}: {:?}", state.seats));
        // The owner's seat has no pointer headless.
        if has_pointer {
            let pointer = wl_seat.get_pointer(&qh, ());
            state.cursor_shape = state
                .cursor_shapes
                .as_ref()
                .map(|manager| manager.get_pointer(&pointer, &qh, ()));
            state.pointer = Some(pointer);
        }
        state.keyboard = Some(wl_seat.get_keyboard(&qh, ()));
        state.text_input = state
            .text_inputs
            .as_ref()
            .map(|manager| manager.get_text_input(&wl_seat, &qh, ()));
        state.data_device = state
            .data_device_manager
            .as_ref()
            .map(|manager| manager.get_data_device(&wl_seat, &qh, ()));
        input.roundtrip(compositor);
        input
    }

    /// A roundtrip of the client, then the input events it read.
    pub fn roundtrip(&mut self, compositor: &mut Compositor) {
        self.client.roundtrip(compositor);
        self.queue
            .dispatch_pending(&mut self.state)
            .expect("the input events are read");
    }

    /// The `wl_seat` named `name`, to name in a request.
    pub fn wl_seat(&self, name: &str) -> wl_seat::WlSeat {
        self.state
            .seats
            .iter()
            .find(|(_, seat, _)| seat.as_deref() == Some(name))
            .map(|(seat, _, _)| seat.clone())
            .unwrap_or_else(|| panic!("no wl_seat named {name}"))
    }

    /// Reads what the compositor sent before it closed the connection, as a client whose seat
    /// was closed does.
    pub fn read_until_closed(&mut self) {
        for _ in 0..10 {
            let Some(guard) = self.queue.prepare_read() else {
                break;
            };
            if guard.read().is_err() {
                break;
            }
        }
        let _ = self.queue.dispatch_pending(&mut self.state);
    }

    /// The names of the `wl_seat`s the client sees.
    pub fn seat_names(&self) -> Vec<String> {
        self.state
            .seats
            .iter()
            .filter_map(|(_, name, _)| name.clone())
            .collect()
    }

    /// The events since the last call.
    pub fn take_events(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.state.events)
    }

    /// The serial of the last pointer `enter`.
    pub fn pointer_enter_serial(&self) -> u32 {
        self.state
            .events
            .iter()
            .rev()
            .find_map(|event| match event {
                InputEvent::PointerEnter { serial, .. } => Some(*serial),
                _ => None,
            })
            .expect("a pointer enter")
    }

    /// Asks the cursor `shape`, with the serial `serial` of the pointer's `enter`.
    pub fn set_cursor_shape(&mut self, serial: u32, shape: Shape) {
        self.state
            .cursor_shape
            .as_ref()
            .expect("wp_cursor_shape_device_v1")
            .set_shape(serial, shape);
    }

    /// Enables its text field, or disables it, and commits that state, as an application does
    /// when a field takes or loses the focus.
    pub fn enable_text_field(&mut self, enabled: bool) {
        let field = self.state.text_input.as_ref().expect("zwp_text_input_v3");
        if enabled {
            field.enable();
        } else {
            field.disable();
        }
        field.commit();
        self.state.text_commits += 1;
    }

    /// Starts a drag from `origin` of a new source offering `text/plain` for a copy, naming
    /// `serial`; returns the source's index.
    pub fn start_drag(&mut self, origin: &wl_surface::WlSurface, serial: u32) -> usize {
        let index = self.state.sources.len();
        let source = self
            .state
            .data_device_manager
            .as_ref()
            .expect("wl_data_device_manager")
            .create_data_source(&self.queue.handle(), index);
        source.offer("text/plain".to_owned());
        source.set_actions(wl_data_device_manager::DndAction::Copy);
        self.state
            .data_device
            .as_ref()
            .expect("wl_data_device")
            .start_drag(Some(&source), origin, None, serial);
        self.state.sources.push(DragSource::default());
        index
    }

    /// Accepts the offer of the drag that entered last, as `text/plain`, for a copy, naming the
    /// `enter`'s `serial`.
    pub fn accept_drag(&mut self, serial: u32) {
        let offer = self.state.drag_offer.as_ref().expect("a drag's offer");
        offer.accept(serial, Some("text/plain".to_owned()));
        offer.set_actions(
            wl_data_device_manager::DndAction::Copy,
            wl_data_device_manager::DndAction::Copy,
        );
    }
}

/// A keymap's text from its file, without the NUL that ends it. Read from its start whatever
/// the file's offset: the compositor may share one open file among its clients, as a mapping
/// would read it.
fn read_keymap(fd: std::os::fd::OwnedFd, size: u32) -> Option<String> {
    let mut bytes = vec![0; usize::try_from(size).ok()?];
    File::from(fd).read_exact_at(&mut bytes, 0).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    Some(text.trim_end_matches('\0').to_owned())
}

impl Dispatch<wl_registry::WlRegistry, ()> for InputState {
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
                    .push((registry.bind(name, version.min(8), qh, index), None, false));
            }
            "wp_cursor_shape_manager_v1" => {
                state.cursor_shapes = Some(registry.bind(name, 1, qh, ()));
            }
            "wl_data_device_manager" => {
                state.data_device_manager = Some(registry.bind(name, version.min(3), qh, ()));
            }
            "zwp_text_input_manager_v3" => {
                state.text_inputs = Some(registry.bind(name, 1, qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, usize> for InputState {
    fn event(
        state: &mut Self,
        _: &wl_seat::WlSeat,
        event: wl_seat::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_seat::Event::Name { name } => state.seats[*index].1 = Some(name),
            wl_seat::Event::Capabilities {
                capabilities: WEnum::Value(capabilities),
            } => {
                state.seats[*index].2 = capabilities.contains(wl_seat::Capability::Pointer);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for InputState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let horizontal = |axis: &WEnum<wl_pointer::Axis>| {
            *axis == WEnum::Value(wl_pointer::Axis::HorizontalScroll)
        };
        let event = match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => InputEvent::PointerEnter {
                surface,
                x: surface_x,
                y: surface_y,
                serial,
            },
            wl_pointer::Event::Leave { surface, .. } => InputEvent::PointerLeave { surface },
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => InputEvent::Motion {
                x: surface_x,
                y: surface_y,
            },
            wl_pointer::Event::Button {
                serial,
                button,
                state: button_state,
                ..
            } => InputEvent::Button {
                button,
                pressed: button_state == WEnum::Value(wl_pointer::ButtonState::Pressed),
                serial,
            },
            wl_pointer::Event::Axis { axis, value, .. } => InputEvent::Axis {
                horizontal: horizontal(&axis),
                value,
            },
            wl_pointer::Event::AxisValue120 { axis, value120 } => InputEvent::Value120 {
                horizontal: horizontal(&axis),
                steps: value120,
            },
            wl_pointer::Event::Frame => InputEvent::Frame,
            _ => return,
        };
        state.events.push(event);
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for InputState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let event = match event {
            wl_keyboard::Event::Keymap { fd, size, .. } => {
                let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                state.xkb = read_keymap(fd, size)
                    .and_then(|text| {
                        xkb::Keymap::new_from_string(
                            &context,
                            text,
                            xkb::KEYMAP_FORMAT_TEXT_V1,
                            xkb::KEYMAP_COMPILE_NO_FLAGS,
                        )
                    })
                    .map(|keymap| {
                        let xkb_state = xkb::State::new(&keymap);
                        (keymap, xkb_state)
                    });
                assert!(state.xkb.is_some(), "a keymap that does not compile");
                InputEvent::Keymap
            }
            wl_keyboard::Event::Enter {
                serial,
                surface,
                keys,
            } => InputEvent::KeyboardEnter {
                surface,
                serial,
                keys: keys
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| u32::from_ne_bytes(*bytes))
                    .collect(),
            },
            wl_keyboard::Event::Leave { surface, .. } => InputEvent::KeyboardLeave { surface },
            wl_keyboard::Event::Key {
                serial,
                key,
                state: key_state,
                ..
            } => {
                let pressed = key_state == WEnum::Value(wl_keyboard::KeyState::Pressed);
                if pressed && let Some((_, xkb_state)) = &state.xkb {
                    let text = xkb_state.key_get_utf8(xkb::Keycode::new(key + 8));
                    state.typed.push_str(&text);
                    state.received.push_str(&text);
                }
                InputEvent::Key {
                    key,
                    pressed,
                    serial,
                }
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some((_, xkb_state)) = &mut state.xkb {
                    xkb_state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
                InputEvent::Modifiers {
                    depressed: mods_depressed,
                }
            }
            _ => return,
        };
        state.events.push(event);
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for InputState {
    fn event(
        state: &mut Self,
        _: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let event = match event {
            zwp_text_input_v3::Event::Enter { surface } => TextEvent::Enter(surface),
            zwp_text_input_v3::Event::Leave { surface } => TextEvent::Leave(surface),
            zwp_text_input_v3::Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => {
                state.pending_preedit.clone_from(&text);
                TextEvent::Preedit {
                    text,
                    begin: cursor_begin,
                    end: cursor_end,
                }
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                state.pending_commit.clone_from(&text);
                TextEvent::Commit(text)
            }
            zwp_text_input_v3::Event::Done { serial } => {
                // As the protocol asks: a done applies what came before it, the preedit shown
                // replaced by the new one or cleared, and only when it names the client's last
                // commit.
                let preedit = state.pending_preedit.take();
                let commit = state.pending_commit.take();
                if serial == state.text_commits {
                    state.preedit = preedit;
                    if let Some(commit) = commit {
                        state.received.push_str(&commit);
                    }
                }
                TextEvent::Done(serial)
            }
            _ => return,
        };
        state.text_events.push(event);
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for InputState {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let event = match event {
            wl_data_device::Event::Enter {
                serial,
                surface,
                id,
                ..
            } => {
                state.drag_offer = id;
                InputEvent::DragEnter { surface, serial }
            }
            wl_data_device::Event::Motion { .. } => InputEvent::DragMotion,
            wl_data_device::Event::Leave => InputEvent::DragLeave,
            wl_data_device::Event::Drop => InputEvent::Drop,
            _ => return,
        };
        state.events.push(event);
    }

    event_created_child!(InputState, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_source::WlDataSource, usize> for InputState {
    fn event(
        state: &mut Self,
        _: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let source = &mut state.sources[*index];
        match event {
            wl_data_source::Event::Cancelled => source.cancelled = true,
            wl_data_source::Event::DndDropPerformed => source.drop_performed = true,
            _ => {}
        }
    }
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for InputState {
    fn event(
        _: &mut Self,
        _: &wl_data_offer::WlDataOffer,
        _: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(InputState: ignore wl_data_device_manager::WlDataDeviceManager);
wayland_client::delegate_noop!(InputState: ignore zwp_text_input_manager_v3::ZwpTextInputManagerV3);
wayland_client::delegate_noop!(InputState: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
wayland_client::delegate_noop!(InputState: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);
