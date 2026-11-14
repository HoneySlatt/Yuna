//! Pointer and keyboard input on an agent's seat
//! ([ADR-0037](../../../docs/adr/0037-seats-tools-input-and-closing.md)), and drag and drop.
//!
//! Input reaches a seat only through these calls, which the MCP server makes for the seat an
//! agent names; the owner's seat takes none. A pointer moves in logical units of one of its
//! seat's outputs; what is under it is found among the windows that output draws at the event's
//! time, where the motion draws them, top to bottom, each window's popups before it: the surface
//! whose input region holds the point. While windows move under a still pointer, its focus waits:
//! a button or a scroll first puts it on what is drawn under it then, and the refresh where the
//! motion ends puts it there once ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
//! A button press focuses the window under it, which takes the seat's keyboard at the next
//! refresh (`frames.rs`), and its serial may start a popup grab or a drag on that window. Every
//! focus of a seat is on its own clients' surfaces: no event of one seat reaches another's
//! client.
//!
//! Keys are pressed by their XKB keycode. Text is typed through the seat's `us` keymap, Shift
//! held where a character needs it. A character the keymap lacks is committed to the text field
//! enabled where the keyboard is, through `text-input-v3` (`text_input.rs`); with none, it is
//! typed through Smithay's `inject_text_keysyms`, which gives the focused client a keymap of its
//! own for it, then the seat's back. Events carry Yuna's clock's time.

use std::any::Any;
use std::time::Duration;

use smithay::backend::input::{Axis, AxisSource, ButtonState, InputTime, KeyState};
use smithay::desktop::PopupManager;
use smithay::desktop::WindowSurfaceType;
use smithay::desktop::utils::under_from_surface_tree;
use smithay::input::Seat;
use smithay::input::dnd::{DnDGrab, DndGrabHandler, GrabType, Source};
use smithay::input::keyboard::{FilterResult, Keycode, Keysym, xkb};
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, CursorImageStatus, Focus, MotionEvent, PointerHandle,
};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER, Serial};
use smithay::wayland::compositor::get_role;
use smithay::wayland::selection::data_device::WaylandDndGrabHandler;
use smithay::wayland::shell::xdg::XDG_POPUP_ROLE;
use yuna_layout::{OutputId, SeatId, WindowId};
use yuna_motion::Change;

use crate::Error;
use crate::output::VirtualOutput;
use crate::popups::popup_toplevel;
use crate::seats::{seat_id, seat_of_surface};
use crate::state::{OWNER_SEAT, State, tree_root};

/// The most characters one `type_text` types: a paragraph, so that one call cannot keep Yuna
/// busy for long.
pub const MAX_TYPED: usize = 4_096;

/// The most wheel steps one scroll takes on each axis.
pub const MAX_SCROLL_STEPS: f64 = 100.0;

/// How far one wheel step scrolls, in logical units, as libinput reports a mouse wheel's click.
const SCROLL_STEP: f64 = 15.0;

/// The left button, as Linux's `input-event-codes.h` numbers it (`BTN_LEFT`).
pub const BUTTON_LEFT: u32 = 0x110;
/// The right button (`BTN_RIGHT`).
pub const BUTTON_RIGHT: u32 = 0x111;
/// The middle button (`BTN_MIDDLE`).
pub const BUTTON_MIDDLE: u32 = 0x112;
/// The back button (`BTN_SIDE`, which toolkits read as back).
pub const BUTTON_BACK: u32 = 0x113;
/// The forward button (`BTN_EXTRA`, which toolkits read as forward).
pub const BUTTON_FORWARD: u32 = 0x114;

/// Where a seat's pointer is and what is under it, as
/// [`Compositor::pointer`](crate::Compositor::pointer) describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct PointerDescription {
    /// The seat's output it is on; none before it first moves.
    pub output: Option<OutputId>,
    /// Its place across, in logical units from that output's left edge.
    pub x: f64,
    /// Its place down, from that output's top edge.
    pub y: f64,
    /// The window under it, if any.
    pub window: Option<WindowId>,
    /// Its place in the surface under it, as that surface's client is told.
    pub surface_position: Option<(f64, f64)>,
    /// The cursor the client under it asked for: a `cursor-shape` name, `hidden`, or `surface`
    /// for a cursor it draws itself.
    pub cursor: String,
    /// The buttons held.
    pub buttons: Vec<u32>,
}

/// The toplevel surface of the window `surface` belongs to: through its subsurfaces' parents,
/// then its popups' parents. None for a popup whose chain of parents is broken.
pub(crate) fn window_of_surface(surface: &WlSurface) -> Option<WlSurface> {
    let root = tree_root(surface);
    if get_role(&root) == Some(XDG_POPUP_ROLE) {
        popup_toplevel(&root)
    } else {
        Some(root)
    }
}

/// A key's keycode and whether it needs Shift, for a keysym of `keymap`'s first layout: its
/// first key that gives it with no modifier, else with Shift alone.
fn key_for_keysym(keymap: &xkb::Keymap, keysym: Keysym) -> Option<(Keycode, bool)> {
    let shift = 1 << keymap.mod_get_index(xkb::MOD_NAME_SHIFT);
    let mut found_with_shift = None;
    for raw in keymap.min_keycode().raw()..=keymap.max_keycode().raw() {
        let key = Keycode::new(raw);
        for level in 0..keymap.num_levels_for_key(key, 0) {
            if !keymap
                .key_get_syms_by_level(key, 0, level)
                .contains(&keysym)
            {
                continue;
            }
            let mut masks = [0; 8];
            let count = keymap.key_get_mods_for_level(key, 0, level, &mut masks);
            let masks = &masks[..count.min(masks.len())];
            if masks.contains(&0) {
                return Some((key, false));
            }
            if found_with_shift.is_none() && masks.contains(&shift) {
                found_with_shift = Some((key, true));
            }
        }
    }
    found_with_shift
}

/// The keysym a character types: Return, Tab and BackSpace for their control characters.
fn keysym_of_char(character: char) -> Keysym {
    match character {
        '\n' | '\r' => Keysym::Return,
        '\t' => Keysym::Tab,
        '\u{8}' => Keysym::BackSpace,
        _ => xkb::utf32_to_keysym(u32::from(character)),
    }
}

/// The keysym an XKB name gives: exactly as written, else ignoring case ("return"). None for a
/// name holding a NUL, which xkbcommon cannot take.
fn keysym_of_name(name: &str) -> Option<Keysym> {
    if name.contains('\0') {
        return None;
    }
    [xkb::KEYSYM_NO_FLAGS, xkb::KEYSYM_CASE_INSENSITIVE]
        .into_iter()
        .map(|flags| xkb::keysym_from_name(name, flags))
        .find(|keysym| keysym.raw() != 0)
}

/// What a step of typed text does: press a key, Shift with it or not; commit characters the
/// keymap lacks to the text field enabled where the keyboard is; or, with none, type them
/// through an injected keymap.
enum Typed {
    Key(Keycode, bool),
    Committed(String),
    Injected(Vec<Keysym>),
}

impl State {
    /// The time input events carry: Yuna's clock's.
    pub(crate) fn input_time(&self) -> InputTime {
        InputTime::from_micros(u64::try_from(self.clock.now().as_micros()).unwrap_or(u64::MAX))
    }

    /// The agent seat `seat`'s pointer; refused for the owner's seat, which takes no input from
    /// here, and for a seat that does not exist.
    pub(crate) fn agent_pointer(&self, seat: SeatId) -> Result<PointerHandle<State>, Error> {
        if seat == OWNER_SEAT {
            return Err(Error::Refused(
                "input goes to agents' seats alone".to_owned(),
            ));
        }
        self.seat(seat)
            .and_then(|data| data.pointer.clone())
            .ok_or_else(|| Error::Refused(format!("no seat {}", seat.0)))
    }

    /// The output of `seat` that holds `location`, in logical coordinates.
    fn output_at(&self, seat: SeatId, location: Point<f64, Logical>) -> Option<&VirtualOutput> {
        self.outputs.iter().find(|output| {
            output.seat == seat
                && location.x >= output.rect.x
                && location.x < output.rect.right()
                && location.y >= output.rect.y
                && location.y < output.rect.bottom()
        })
    }

    /// The output `seat`'s pointer is on; none before it first moves.
    pub(crate) fn pointer_output(&self, seat: SeatId) -> Option<OutputId> {
        let data = self.seat(seat).filter(|data| data.pointer_moved)?;
        let location = data.pointer.as_ref()?.current_location();
        self.output_at(seat, location).map(|output| output.id)
    }

    /// Whether windows move at `at` on the output `seat`'s pointer is on: its focus then waits
    /// for a pointer event or the motion's end, rather than follow every frame
    /// ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
    pub(crate) fn pointer_in_motion(&self, seat: SeatId, at: Duration) -> bool {
        self.pointer_output(seat)
            .is_some_and(|output| self.motion.end(output, at).is_some())
    }

    /// The surface under `location`, in logical coordinates, among the windows that the output of
    /// `seat` holding it draws at `at`, with that surface's origin in the same coordinates.
    fn surface_under(
        &self,
        seat: SeatId,
        location: Point<f64, Logical>,
        at: Duration,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.output_at(seat, location)?;
        let hit = |surface: &WlSurface, origin: Point<f64, Logical>| {
            under_from_surface_tree(surface, location - origin, (0, 0), WindowSurfaceType::ALL)
                .map(|(found, at)| (found, origin + at.to_f64()))
        };
        // What the output draws then, as its frames do (`frames.rs`): a window moving is where
        // it is drawn, not at its place; a window scrolled out is not, nor its popups.
        let frame = self.motion.frame(output.id, at);
        let shown = frame
            .windows
            .iter()
            .rev()
            .filter(|drawn| drawn.rect.overlaps(&output.rect));
        for drawn in shown {
            let Some(toplevel) = self
                .windows
                .get(&drawn.window)
                .and_then(|surface| self.toplevels.get(surface))
            else {
                continue;
            };
            let rect = drawn.rect;
            let corner = Point::<f64, Logical>::from((rect.x, rect.y));
            // Topmost first; a menu may overflow its window.
            for (popup, at) in PopupManager::popups_for_surface(toplevel.wl_surface()) {
                let origin = corner + at.to_f64() - popup.geometry().loc.to_f64();
                if let Some(found) = hit(popup.wl_surface(), origin) {
                    return Some(found);
                }
            }
            let inside = location.x >= rect.x
                && location.x < rect.right()
                && location.y >= rect.y
                && location.y < rect.bottom();
            if inside
                && let Some(found) = hit(
                    toplevel.wl_surface(),
                    corner - toplevel.geometry.loc.to_f64(),
                )
            {
                return Some(found);
            }
        }
        None
    }

    /// Sends a pointer's motion to `location`, onto `focus`, and ends its frame.
    pub(crate) fn send_pointer_motion(
        &mut self,
        pointer: &PointerHandle<State>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        location: Point<f64, Logical>,
    ) {
        let event = MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: self.input_time(),
        };
        pointer.motion(self, focus, &event);
        pointer.frame(self);
    }

    /// Puts `seat`'s pointer on what is under it as drawn at `at`, after windows moved: a motion
    /// where it is, sent only when the surface under it or its place in it changed.
    pub(crate) fn refresh_pointer_focus(&mut self, seat: SeatId, at: Duration) {
        let Some(pointer) = self.seat(seat).and_then(|data| data.pointer.clone()) else {
            return;
        };
        if self.seat(seat).is_some_and(|data| !data.pointer_moved) {
            return;
        }
        let location = pointer.current_location();
        let under = self.surface_under(seat, location, at);
        let current = pointer.current_focus();
        let unchanged = match (&under, &current) {
            (Some((surface, origin)), Some(focus)) => {
                surface == focus
                    && self
                        .seat(seat)
                        .is_some_and(|data| data.pointer_origin == Some(*origin))
            }
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            self.move_pointer(seat, &pointer, location, under);
        }
    }

    fn move_pointer(
        &mut self,
        seat: SeatId,
        pointer: &PointerHandle<State>,
        location: Point<f64, Logical>,
        under: Option<(WlSurface, Point<f64, Logical>)>,
    ) {
        if let Some(data) = self.seat_mut(seat) {
            data.pointer_moved = true;
            data.pointer_origin = under.as_ref().map(|(_, origin)| *origin);
        }
        self.send_pointer_motion(pointer, under, location);
    }

    /// Moves `seat`'s pointer to (`x`, `y`), logical units from the top-left corner of `output`,
    /// one of the seat's, or of its first output. Refused for a place off that output.
    pub(crate) fn pointer_move(
        &mut self,
        seat: SeatId,
        output: Option<OutputId>,
        x: f64,
        y: f64,
    ) -> Result<PointerDescription, Error> {
        let pointer = self.agent_pointer(seat)?;
        let output = match output {
            Some(output) => output,
            None => *self
                .layout
                .outputs(seat)
                .map_err(|e| Error::Layout(e.to_string()))?
                .first()
                .ok_or_else(|| Error::Refused(format!("seat {} has no output", seat.0)))?,
        };
        let rect = self
            .outputs
            .iter()
            .find(|candidate| candidate.id == output && candidate.seat == seat)
            .map(|candidate| candidate.rect)
            .ok_or_else(|| Error::Refused(format!("seat {} has no output {}", seat.0, output.0)))?;
        let on_output = |value: f64, side: f64| value.is_finite() && value >= 0.0 && value < side;
        if !on_output(x, rect.w) || !on_output(y, rect.h) {
            return Err(Error::Refused(format!(
                "({x}, {y}) is off output {}, of {} × {} logical units",
                output.0, rect.w, rect.h
            )));
        }
        let location = Point::from((rect.x + x, rect.y + y));
        let under = self.surface_under(seat, location, self.clock.now());
        self.move_pointer(seat, &pointer, location, under);
        Ok(self.describe_pointer(seat))
    }

    /// Presses or releases `button` of `seat`'s pointer where it is. A press focuses the window
    /// under it, and is recorded with that window, for a popup grab or an activation token it
    /// may carry. A press of a button held, or a release of one not held, is refused.
    pub(crate) fn pointer_button(
        &mut self,
        seat: SeatId,
        button: u32,
        pressed: bool,
    ) -> Result<PointerDescription, Error> {
        let pointer = self.agent_pointer(seat)?;
        let held = self
            .seat(seat)
            .is_some_and(|data| data.pressed_buttons.contains(&button));
        if held == pressed {
            let state = if pressed { "held already" } else { "not held" };
            return Err(Error::Refused(format!("button {button:#x} is {state}")));
        }
        self.focus_what_is_drawn(seat);
        let serial = SERIAL_COUNTER.next_serial();
        if pressed {
            let window = pointer
                .current_focus()
                .and_then(|surface| window_of_surface(&surface));
            if let Some(id) = window
                .as_ref()
                .and_then(|root| self.toplevels.get(&root.id()))
                .and_then(|toplevel| toplevel.window)
                && self.layout.focused(seat) != Some(id)
            {
                match self.layout.focus_window(id) {
                    Ok(()) => self.mark_layout_changed(Change::Animated),
                    Err(error) => tracing::debug!(%error, window = id.0, "focus refused"),
                }
            }
            if let Some(data) = self.seat_mut(seat) {
                data.pressed_buttons.push(button);
                data.last_button_press = window.map(|window| (serial, window));
            }
        } else if let Some(data) = self.seat_mut(seat) {
            data.pressed_buttons.retain(|held| *held != button);
        }
        self.send_button(&pointer, button, pressed, serial);
        self.after_popup_grab_input(seat);
        Ok(self.describe_pointer(seat))
    }

    /// Before a button or a scroll of `seat`'s pointer, while windows move under it: puts its
    /// focus on what is drawn under it now, so that the event reaches what the screen shows
    /// there. At rest its focus is current already.
    fn focus_what_is_drawn(&mut self, seat: SeatId) {
        let now = self.clock.now();
        if self.pointer_in_motion(seat, now) {
            self.refresh_pointer_focus(seat, now);
        }
    }

    /// After input that a popup grab may have ended, Smithay's grab dismissing its popups on a
    /// click outside them: the seat lets the grab go, its keyboard goes back to the window, and
    /// the window's output draws again without the popups.
    fn after_popup_grab_input(&mut self, seat: SeatId) {
        let ended = self
            .seat(seat)
            .and_then(|data| data.popup_grab.as_ref())
            .filter(|active| active.grab.has_ended())
            .map(|active| active.toplevel.clone());
        if let Some(toplevel) = ended {
            self.give_keyboard_to(seat, Some(toplevel.clone()));
            self.refresh_output_of(&toplevel);
        }
    }

    fn send_button(
        &mut self,
        pointer: &PointerHandle<State>,
        button: u32,
        pressed: bool,
        serial: Serial,
    ) {
        let event = ButtonEvent {
            serial,
            time: self.input_time(),
            button,
            state: if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            },
        };
        pointer.button(self, &event);
        pointer.frame(self);
    }

    /// Scrolls `seat`'s wheel by `dx` and `dy` steps where the pointer is: positive is right and
    /// down. Refused past [`MAX_SCROLL_STEPS`] on an axis, and for no step at all.
    pub(crate) fn pointer_scroll(
        &mut self,
        seat: SeatId,
        dx: f64,
        dy: f64,
    ) -> Result<PointerDescription, Error> {
        let pointer = self.agent_pointer(seat)?;
        let valid = |steps: f64| steps.is_finite() && steps.abs() <= MAX_SCROLL_STEPS;
        if !valid(dx) || !valid(dy) || (dx == 0.0 && dy == 0.0) {
            return Err(Error::Refused(format!(
                "a scroll takes 1 to {MAX_SCROLL_STEPS} steps on an axis"
            )));
        }
        self.focus_what_is_drawn(seat);
        let time = self.input_time();
        let mut frame = AxisFrame::new(time).source(AxisSource::Wheel);
        for (axis, steps) in [(Axis::Horizontal, dx), (Axis::Vertical, dy)] {
            if steps != 0.0 {
                frame = frame
                    .value(axis, steps * SCROLL_STEP)
                    .v120(axis, (steps * 120.0).round() as i32);
            }
        }
        pointer.axis(self, frame);
        pointer.frame(self);
        Ok(self.describe_pointer(seat))
    }

    /// Where `seat`'s pointer is, and what is under it.
    pub(crate) fn describe_pointer(&self, seat: SeatId) -> PointerDescription {
        let Some(data) = self.seat(seat) else {
            return PointerDescription {
                output: None,
                x: 0.0,
                y: 0.0,
                window: None,
                surface_position: None,
                cursor: "default".to_owned(),
                buttons: Vec::new(),
            };
        };
        let location = data
            .pointer
            .as_ref()
            .map_or_else(Point::default, PointerHandle::current_location);
        let output = data
            .pointer_moved
            .then(|| self.output_at(seat, location))
            .flatten();
        let focus = data.pointer.as_ref().and_then(PointerHandle::current_focus);
        let window = focus
            .as_ref()
            .and_then(window_of_surface)
            .and_then(|root| self.toplevels.get(&root.id()))
            .and_then(|toplevel| toplevel.window);
        let surface_position = focus
            .and(data.pointer_origin)
            .map(|origin| (location.x - origin.x, location.y - origin.y));
        let cursor = match &data.cursor {
            CursorImageStatus::Hidden => "hidden".to_owned(),
            CursorImageStatus::Named(icon) => icon.name().to_owned(),
            CursorImageStatus::Surface(_) => "surface".to_owned(),
        };
        PointerDescription {
            output: output.map(|output| output.id),
            x: location.x - output.map_or(0.0, |output| output.rect.x),
            y: location.y - output.map_or(0.0, |output| output.rect.y),
            window,
            surface_position,
            cursor,
            buttons: data.pressed_buttons.clone(),
        }
    }

    /// The window `seat`'s keyboard is on, or the window of the popup it is on.
    pub(crate) fn keyboard_window(&self, seat: SeatId) -> Option<WindowId> {
        self.seat(seat)?
            .keyboard
            .current_focus()
            .as_ref()
            .and_then(window_of_surface)
            .and_then(|root| self.toplevels.get(&root.id()))
            .and_then(|toplevel| toplevel.window)
    }

    /// The keycode of the key XKB names `name` ("Return", "a", "F5") on `seat`'s keymap.
    pub(crate) fn keycode_of(&self, seat: SeatId, name: &str) -> Result<Keycode, Error> {
        let keymap = self
            .seat(seat)
            .and_then(|data| data.keymap.as_ref())
            .ok_or_else(|| Error::Refused(format!("no agent seat {}", seat.0)))?;
        let keysym = keysym_of_name(name)
            .ok_or_else(|| Error::Refused(format!("no key is named {name:?}")))?;
        key_for_keysym(keymap, keysym)
            .map(|(key, _)| key)
            .ok_or_else(|| {
                Error::Refused(format!(
                    "the seat's keymap has no key {name:?}; type text with `type`"
                ))
            })
    }

    /// Presses or releases `key` on `seat`'s keyboard, to the surface it is on. A press of a key
    /// held, or a release of one not held, is refused.
    pub(crate) fn key(&mut self, seat: SeatId, key: Keycode, pressed: bool) -> Result<(), Error> {
        self.agent_pointer(seat)?;
        let Some(keyboard) = self.seat(seat).map(|data| data.keyboard.clone()) else {
            return Err(Error::Refused(format!("no seat {}", seat.0)));
        };
        if keyboard.pressed_keys().contains(&key) == pressed {
            let state = if pressed { "held already" } else { "not held" };
            return Err(Error::Refused(format!("that key is {state}")));
        }
        let serial = SERIAL_COUNTER.next_serial();
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        let time = self.input_time();
        keyboard.input::<(), _>(self, key, state, serial, time, |_, _, _| {
            FilterResult::Forward
        });
        if pressed
            && let Some(focus) = keyboard.current_focus()
            && let Some(data) = self.seat_mut(seat)
        {
            data.last_key_press = Some((serial, focus));
        }
        Ok(())
    }

    /// Taps `combination` on `seat`'s keyboard: keys joined by `+`, the last one pressed while
    /// the others are held ("ctrl+shift+t", "Return"). Besides XKB's names, `ctrl`, `shift`,
    /// `alt`, `super` and `altgr` name the left modifiers. Each key is pressed in order, then
    /// released in the reverse order. Refused, with nothing pressed, for a name no key has, a
    /// key named twice, or a key held already.
    pub(crate) fn tap_keys(&mut self, seat: SeatId, combination: &str) -> Result<(), Error> {
        let mut keys = Vec::new();
        for name in combination.split('+') {
            let name = match name.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => "Control_L",
                "shift" => "Shift_L",
                "alt" => "Alt_L",
                "super" | "logo" | "meta" | "win" => "Super_L",
                "altgr" => "ISO_Level3_Shift",
                _ => name,
            };
            let key = self.keycode_of(seat, name)?;
            if keys.contains(&key) {
                return Err(Error::Refused(format!("{combination:?} names a key twice")));
            }
            keys.push(key);
        }
        let held = self
            .seat(seat)
            .map(|data| data.keyboard.pressed_keys())
            .unwrap_or_default();
        if keys.iter().any(|key| held.contains(key)) {
            return Err(Error::Refused(format!(
                "a key of {combination:?} is held already"
            )));
        }
        for &key in &keys {
            self.key(seat, key, true)?;
        }
        for &key in keys.iter().rev() {
            self.key(seat, key, false)?;
        }
        Ok(())
    }

    /// Types `text` on `seat`'s keyboard, to the surface it is on: each character by its key in
    /// the seat's keymap, Shift held where it needs it; lacking one, committed to the text field
    /// enabled there, or, with none, through an injected keymap. Text being composed in that
    /// field is cleared first, as an input method's commit replaces it. Refused, with nothing
    /// typed, past [`MAX_TYPED`] characters, for a character that is no keysym, and when a key
    /// the text needs is held.
    pub(crate) fn type_text(&mut self, seat: SeatId, text: &str) -> Result<(), Error> {
        self.agent_pointer(seat)?;
        if text.chars().count() > MAX_TYPED {
            return Err(Error::Refused(format!(
                "a call types {MAX_TYPED} characters at most"
            )));
        }
        let steps = self.typing_steps(seat, text)?;
        let shift = self.keycode_of(seat, "Shift_L")?;
        // Checked before anything is pressed, so that a refusal types nothing and leaves no
        // Shift held.
        let held = self
            .seat(seat)
            .map(|data| data.keyboard.pressed_keys())
            .unwrap_or_default();
        let needs_held = steps.iter().any(|step| match step {
            Typed::Key(key, with_shift) => {
                held.contains(key) || (*with_shift && held.contains(&shift))
            }
            Typed::Committed(_) | Typed::Injected(_) => false,
        });
        if needs_held {
            return Err(Error::Refused(
                "a key the text needs is held: release it first".to_owned(),
            ));
        }
        if !text.is_empty() && self.text_field_enabled(seat) {
            self.compose_text(seat, "")?;
        }
        for step in steps {
            match step {
                Typed::Key(key, with_shift) => {
                    if with_shift {
                        self.key(seat, shift, true)?;
                    }
                    self.key(seat, key, true)?;
                    self.key(seat, key, false)?;
                    if with_shift {
                        self.key(seat, shift, false)?;
                    }
                }
                Typed::Committed(text) => self.commit_text(seat, &text)?,
                Typed::Injected(keysyms) => {
                    let Some(keyboard) = self.seat(seat).map(|data| data.keyboard.clone()) else {
                        continue;
                    };
                    let time = self.input_time();
                    // Its keymap has room for 247 keys, from keycode 9 to 255.
                    for chunk in keysyms.chunks(247) {
                        keyboard.inject_text_keysyms(self, chunk, time);
                    }
                }
            }
        }
        Ok(())
    }

    /// What typing `text` on `seat` does, step by step, its committed or injected characters
    /// grouped.
    fn typing_steps(&self, seat: SeatId, text: &str) -> Result<Vec<Typed>, Error> {
        let text_field = self.text_field_enabled(seat);
        let keymap = self
            .seat(seat)
            .and_then(|data| data.keymap.as_ref())
            .ok_or_else(|| Error::Refused(format!("no agent seat {}", seat.0)))?;
        let mut steps: Vec<Typed> = Vec::new();
        for character in text.chars() {
            let keysym = keysym_of_char(character);
            if keysym.raw() == 0 {
                return Err(Error::Refused(format!(
                    "{character:?} cannot be typed: it has no keysym"
                )));
            }
            match key_for_keysym(keymap, keysym) {
                Some((key, shift)) => steps.push(Typed::Key(key, shift)),
                None if text_field => match steps.last_mut() {
                    Some(Typed::Committed(committed)) => committed.push(character),
                    _ => steps.push(Typed::Committed(character.to_string())),
                },
                None => match steps.last_mut() {
                    Some(Typed::Injected(keysyms)) => keysyms.push(keysym),
                    _ => steps.push(Typed::Injected(vec![keysym])),
                },
            }
        }
        Ok(steps)
    }

    /// Releases every key and button `seat` holds, as when its agent lets go: before its seat
    /// closes.
    pub(crate) fn release_held_input(&mut self, seat: SeatId) {
        let Some(data) = self.seat(seat) else {
            return;
        };
        let keys: Vec<Keycode> = data.keyboard.pressed_keys().into_iter().collect();
        let buttons = data.pressed_buttons.clone();
        for key in keys {
            let _ = self.key(seat, key, false);
        }
        for button in buttons {
            let _ = self.pointer_button(seat, button, false);
        }
    }
}

/// The client of a drag's source: a `wl_data_source`, or, for a drag within one client, its
/// origin surface.
fn source_seat<S: Source>(source: &S) -> Option<SeatId> {
    let source: &dyn Any = source;
    if let Some(data_source) = source.downcast_ref::<WlDataSource>() {
        return data_source
            .client()
            .map(|client| crate::seats::seat_of_client(&client));
    }
    source.downcast_ref::<WlSurface>().map(seat_of_surface)
}

/// A drag a client starts from a button it holds: Smithay checks that the request carries the
/// serial of the pointer's grab, which a press starts. The drag takes the pointer until the
/// button is released, and offers its source to the surfaces it crosses, all of its seat's.
/// Refused, its source cancelled, from a client of another seat than the pointer's: one that
/// bound another agent's `wl_seat` and guessed its serial. Touch has no drag: Yuna has no touch.
/// The drag's icon is not drawn: Yuna draws no cursor yet.
impl WaylandDndGrabHandler for State {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        _icon: Option<WlSurface>,
        seat: Seat<State>,
        serial: Serial,
        type_: GrabType,
    ) {
        let pointer = seat.get_pointer();
        let start = pointer.as_ref().and_then(PointerHandle::grab_start_data);
        let same_seat = seat_id(&seat).is_some_and(|id| source_seat(&source) == Some(id));
        match (type_, pointer, start) {
            (GrabType::Pointer, Some(pointer), Some(start)) if same_seat => {
                let grab = DnDGrab::new_pointer(&self.display_handle, start, source, seat);
                pointer.set_grab(self, grab, serial, Focus::Keep);
            }
            _ => {
                tracing::debug!("a drag is refused");
                source.cancel();
            }
        }
    }
}

/// A drag's end: Smithay tells the source and the target themselves.
impl DndGrabHandler for State {}
