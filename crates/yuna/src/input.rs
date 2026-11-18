//! Pointer and keyboard input on an agent's seat
//! ([ADR-0095](../../../docs/adr/0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)), and drag and drop.
//!
//! Input reaches a seat only through these calls, which the MCP server makes for the seat an
//! agent names; the owner's seat takes none. A pointer moves in logical units of one of its
//! seat's outputs; what is under it is found among the windows that output draws at the event's
//! time, or at the time its last frame sampled if that is later (`frames.rs`), where the motion
//! draws them, top to bottom, each window's popups before it: the surface whose input region
//! holds the point. While windows move under a still pointer, its focus waits: a button or a
//! scroll first puts it on what is drawn under it then, and the refresh where the motion ends
//! puts it there once ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
//! A button press focuses the window under it, which takes the seat's keyboard at the next
//! refresh (`frames.rs`), and its serial may start a popup grab or a drag on that window. During a
//! drag, each motion and the release also go to the seat's drags between X11 and Wayland
//! (`xwayland/drags.rs`), which pick the drag's target among the layout's windows alone, never
//! an X11 menu. Every focus of a seat is on its own clients' surfaces: no event of one seat
//! reaches another's client.
//!
//! Keys are pressed by their XKB keycode, a key named by a keysym it types. Text is typed through
//! the seat's keymap, `us` unless its agent changed it (`keymap.rs`): each character by its key,
//! Shift, AltGr or level 5's key held where its level needs it, in its group, which the keymap's
//! own switch locks first and locks back at the end. A character the keymap's keys do not type
//! is committed to the text field enabled where the keyboard is, through `text-input-v3`
//! (`text_input.rs`); with none, it is typed through Smithay's `inject_text_keysyms`, which gives
//! the focused client a keymap of its own for it, then the seat's back. Events carry Yuna's
//! clock's time.

use std::any::Any;
use std::collections::HashSet;

use smithay::backend::input::{Axis, AxisSource, ButtonState, InputTime, KeyState};
use smithay::desktop::PopupManager;
use smithay::desktop::WindowSurfaceType;
use smithay::desktop::utils::under_from_surface_tree;
use smithay::input::Seat;
use smithay::input::dnd::{DnDGrab, DndGrabHandler, DndTarget, GrabType, Source};
use smithay::input::keyboard::{
    FilterResult, KeyboardHandle, KeyboardSource, Keycode, KeymapFile, Keysym, xkb,
};
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
use crate::keymap::{Chord, KeyPlanner, KeyboardState};
use crate::output::VirtualOutput;
use crate::popups::popup_toplevel;
use crate::render;
use crate::seats::{seat_id, seat_of_surface};
use crate::state::{OWNER_SEAT, State, tree_root};
use crate::xwayland::X11Toplevel;

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

/// What a hit test found under a point: the surface whose input region holds it, that surface's
/// origin in logical coordinates, and the X11 window, when the surface is an X11 window's own.
#[derive(Debug)]
pub(crate) struct Hit {
    pub surface: WlSurface,
    pub origin: Point<f64, Logical>,
    pub x11: Option<X11Toplevel>,
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

/// What a step of typed text does: tap a chord, the keys that pick a character's level or lock
/// its group held before its key; commit characters the keymap's keys do not type to the text
/// field enabled where the keyboard is; or, with none, type them through an injected keymap.
enum Typed {
    Keys(Chord),
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
    fn output_under_pointer(&self, seat: SeatId) -> Option<&VirtualOutput> {
        let data = self.seat(seat).filter(|data| data.pointer_moved)?;
        let location = data.pointer.as_ref()?.current_location();
        self.output_at(seat, location)
    }

    /// The id of the output `seat`'s pointer is on; none before it first moves.
    pub(crate) fn pointer_output(&self, seat: SeatId) -> Option<OutputId> {
        self.output_under_pointer(seat).map(|output| output.id)
    }

    /// Whether windows move now, as hit tests read them, on the output `seat`'s pointer is on:
    /// its focus then waits for a pointer event or the motion's end, rather than follow every
    /// frame ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
    pub(crate) fn pointer_in_motion(&self, seat: SeatId) -> bool {
        let now = self.clock.now();
        self.output_under_pointer(seat).is_some_and(|output| {
            self.motion
                .end(output.id, output.motion_read_at(now))
                .is_some()
        })
    }

    /// The surface under `location`, in logical coordinates, among the windows that the output of
    /// `seat` holding it draws now, read at [`VirtualOutput::motion_read_at`], with that surface's
    /// origin in the same coordinates.
    pub(crate) fn surface_under(
        &self,
        seat: SeatId,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.hit_test(seat, location, true)
            .map(|hit| (hit.surface, hit.origin))
    }

    /// What is under `location`, as [`State::surface_under`] finds it; X11 menus left out unless
    /// `with_menus`, as a drag's target is picked among the layout's windows alone.
    pub(crate) fn hit_test(
        &self,
        seat: SeatId,
        location: Point<f64, Logical>,
        with_menus: bool,
    ) -> Option<Hit> {
        let output = self.output_at(seat, location)?;
        let at = output.motion_read_at(self.clock.now());
        let hit = |surface: &WlSurface, origin: Point<f64, Logical>| {
            under_from_surface_tree(surface, location - origin, (0, 0), WindowSurfaceType::ALL)
                .map(|(found, at)| (found, origin + at.to_f64()))
        };
        let found = |(surface, origin): (WlSurface, Point<f64, Logical>)| Hit {
            surface,
            origin,
            x11: None,
        };
        // What the output draws then, as its frames do (`frames.rs`): a window moving is where
        // it is drawn, not at its place; a window scrolled out is not, nor its popups. X11 menus
        // over no window are over every window.
        let menus = with_menus.then(|| self.x11_shown_menus(output));
        for lone in menus.iter().flat_map(|menus| &menus.lone) {
            let origin =
                render::x11_menu_origin(&lone.surface, lone.corner, lone.x11_scale, output);
            if let Some(under) = origin.and_then(|origin| hit(&lone.surface, origin)) {
                return Some(found(under));
            }
        }
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
            // An X11 window's menus, topmost first, where the frame draws them, so that the
            // pointer lands on the X11 pixel drawn under it, as on the window.
            if let (Some(x11), Some(menus)) = (toplevel.x11(), &menus) {
                let window_corner = render::x11_surface_corner(
                    self.x11.geometry_offset(x11),
                    rect,
                    x11.scale,
                    output,
                );
                for menu in menus.with(drawn.window) {
                    let at = render::x11_offset_on(window_corner, menu.offset, x11.scale, output);
                    let origin = render::x11_menu_origin(&menu.surface, at, x11.scale, output);
                    if let Some(under) = origin.and_then(|origin| hit(&menu.surface, origin)) {
                        return Some(found(under));
                    }
                }
            }
            // Topmost first; a menu may overflow its window.
            for (popup, at) in PopupManager::popups_for_surface(toplevel.wl_surface()) {
                let origin = corner + at.to_f64() - popup.geometry().loc.to_f64();
                if let Some(under) = hit(popup.wl_surface(), origin) {
                    return Some(found(under));
                }
            }
            let inside = location.x >= rect.x
                && location.x < rect.right()
                && location.y >= rect.y
                && location.y < rect.bottom();
            if !inside {
                continue;
            }
            // Where the surface is drawn: for an X11 window, by Yuna's own element, so that the
            // X11 pixel the pointer lands on, its position times XWayland's scale, is the one
            // drawn under it (`render/x11_surface.rs`).
            let origin = match toplevel.x11() {
                Some(x11) => render::x11_surface_origin(
                    &x11.surface,
                    self.x11.geometry_offset(x11),
                    rect,
                    x11.scale,
                    output,
                ),
                None => Some(corner - toplevel.geometry.loc.to_f64()),
            };
            if let Some((surface, origin)) =
                origin.and_then(|origin| hit(toplevel.wl_surface(), origin))
            {
                return Some(Hit {
                    surface,
                    origin,
                    x11: toplevel.x11().cloned(),
                });
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

    /// Puts `seat`'s pointer on what is under it as drawn now, after windows moved: a motion
    /// where it is, sent only when the surface under it or its place in it changed.
    pub(crate) fn refresh_pointer_focus(&mut self, seat: SeatId) {
        let Some(pointer) = self.seat(seat).and_then(|data| data.pointer.clone()) else {
            return;
        };
        if self.seat(seat).is_some_and(|data| !data.pointer_moved) {
            return;
        }
        let location = pointer.current_location();
        let under = self.surface_under(seat, location);
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

    /// Moves `seat`'s pointer to `location`, over `under`: during a drag, what the drag is over
    /// decides where the motion goes (`xwayland/drags.rs`).
    pub(crate) fn move_pointer(
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
        let focus = self.drag_moved(seat, location, under);
        self.send_pointer_motion(pointer, focus, location);
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
        let under = self.surface_under(seat, location);
        self.move_pointer(seat, &pointer, location, under);
        Ok(self.describe_pointer(seat))
    }

    /// Moves `seat`'s pointer to `location`, in the layout's logical coordinates. Refused for a
    /// place on none of the seat's outputs.
    pub(crate) fn pointer_move_to(
        &mut self,
        seat: SeatId,
        location: Point<f64, Logical>,
    ) -> Result<(), Error> {
        let pointer = self.agent_pointer(seat)?;
        if self.output_at(seat, location).is_none() {
            return Err(Error::Refused(format!(
                "({}, {}) is on none of seat {}'s outputs",
                location.x, location.y, seat.0
            )));
        }
        let under = self.surface_under(seat, location);
        self.move_pointer(seat, &pointer, location, under);
        Ok(())
    }

    /// Where `seat`'s pointer is, in the layout's logical coordinates; one that never moved is
    /// at its seat's first output's top-left corner, where a relative motion starts from.
    pub(crate) fn pointer_location(&self, seat: SeatId) -> Result<Point<f64, Logical>, Error> {
        let pointer = self.agent_pointer(seat)?;
        if self.seat(seat).is_some_and(|data| data.pointer_moved) {
            return Ok(pointer.current_location());
        }
        let first = self
            .layout
            .outputs(seat)
            .ok()
            .and_then(|outputs| outputs.first().copied())
            .and_then(|id| self.outputs.iter().find(|output| output.id == id))
            .ok_or_else(|| Error::Refused(format!("seat {} has no output", seat.0)))?;
        Ok(Point::from((first.rect.x, first.rect.y)))
    }

    /// The point of `seat`'s outputs closest to `location`, in the layout's logical coordinates,
    /// as wlroots keeps its cursor on its outputs; none for a seat without an output.
    pub(crate) fn closest_on_seat(
        &self,
        seat: SeatId,
        location: Point<f64, Logical>,
    ) -> Option<Point<f64, Logical>> {
        // Inside an output's last unit, which `output_at` takes as on it.
        const INSIDE: f64 = 1e-3;
        self.outputs
            .iter()
            .filter(|output| output.seat == seat)
            .map(|output| {
                let rect = output.rect;
                let x = location.x.clamp(rect.x, rect.right() - INSIDE);
                let y = location.y.clamp(rect.y, rect.bottom() - INSIDE);
                Point::from((x, y))
            })
            .min_by(|a: &Point<f64, Logical>, b: &Point<f64, Logical>| {
                let distance = |p: &Point<f64, Logical>| {
                    (p.x - location.x).powi(2) + (p.y - location.y).powi(2)
                };
                distance(a).total_cmp(&distance(b))
            })
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
        if !pressed
            && self
                .seat(seat)
                .is_some_and(|data| data.pressed_buttons.is_empty())
        {
            self.drag_released(seat);
        }
        Ok(self.describe_pointer(seat))
    }

    /// Before a button or a scroll of `seat`'s pointer, while windows move under it: puts its
    /// focus on what is drawn under it now, so that the event reaches what the screen shows
    /// there. At rest its focus is current already.
    fn focus_what_is_drawn(&mut self, seat: SeatId) {
        if self.pointer_in_motion(seat) {
            self.refresh_pointer_focus(seat);
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
        let time = self.input_time();
        let mut frame = AxisFrame::new(time).source(AxisSource::Wheel);
        for (axis, steps) in [(Axis::Horizontal, dx), (Axis::Vertical, dy)] {
            if steps != 0.0 {
                frame = frame
                    .value(axis, steps * SCROLL_STEP)
                    .v120(axis, (steps * 120.0).round() as i32);
            }
        }
        self.send_axis(seat, &pointer, frame);
        Ok(self.describe_pointer(seat))
    }

    /// Sends `frame`, a scroll, to what `seat`'s pointer is on as drawn now.
    pub(crate) fn send_axis(
        &mut self,
        seat: SeatId,
        pointer: &PointerHandle<State>,
        frame: AxisFrame,
    ) {
        self.focus_what_is_drawn(seat);
        pointer.axis(self, frame);
        pointer.frame(self);
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

    /// The keycode of the key XKB names `name` ("Return", "a", "F5") on `seat`'s keymap
    /// (`SeatKeymap::key_named`): in the keyboard's group first, a key that types that keysym
    /// alone, else the key of the chord that types it with the fewest keys held. It is pressed
    /// alone: a tap names the modifiers it holds.
    pub(crate) fn keycode_of(&self, seat: SeatId, name: &str) -> Result<Keycode, Error> {
        let (keymap, start) = self
            .seat(seat)
            .and_then(|data| data.keymap.as_ref())
            .zip(self.typing_start(seat))
            .ok_or_else(|| Error::Refused(format!("no agent seat {}", seat.0)))?;
        let keysym = keysym_of_name(name)
            .ok_or_else(|| Error::Refused(format!("no key is named {name:?}")))?;
        keymap.key_named(keysym, start.group).ok_or_else(|| {
            Error::Refused(format!(
                "the seat's keymap has no key {name:?}; type text with `type`"
            ))
        })
    }

    /// The key a release of `name` lets go: the held key, lowest first, that types that keysym
    /// in any group, so that a key pressed in one group is released after another is locked;
    /// else the key [`State::keycode_of`] names, which the release then finds not held.
    pub(crate) fn held_keycode_of(&self, seat: SeatId, name: &str) -> Result<Keycode, Error> {
        let keymap = self.seat(seat).and_then(|data| data.keymap.as_ref());
        let keysym = keysym_of_name(name);
        if let (Some(keymap), Some(keysym)) = (keymap, keysym) {
            let mut held: Vec<Keycode> = self
                .seat(seat)
                .map(|data| data.keyboard.pressed_keys().into_iter().collect())
                .unwrap_or_default();
            held.sort_by_key(|key| key.raw());
            if let Some(key) = held.into_iter().find(|&key| keymap.carries(key, keysym)) {
                return Ok(key);
            }
        }
        self.keycode_of(seat, name)
    }

    /// The state `seat`'s keyboard is in when Yuna's next key reaches it, as its clients read
    /// it: its own, or, while a virtual keyboard's keymap is in use, the state the seat's own
    /// keymap starts with when Yuna's key switches back to it. None for a seat with no keymap of
    /// its own.
    fn typing_start(&self, seat: SeatId) -> Option<KeyboardState> {
        let data = self.seat(seat)?;
        let keymap = data.keymap.as_ref()?;
        if data.keyboard.keymap_file().id() != data.own_keymap.id() {
            return Some(keymap.state_with(&data.keyboard.pressed_keys()));
        }
        let modifiers = data.keyboard.modifier_state().serialized;
        Some(KeyboardState {
            depressed: modifiers.depressed,
            latched: modifiers.latched,
            locked: modifiers.locked,
            group: modifiers.layout_effective,
        })
    }

    /// Presses or releases `key` on `seat`'s keyboard, to the surface it is on, read with the
    /// seat's own keymap. A press of a key held, or a release of one not held, is refused.
    pub(crate) fn key(&mut self, seat: SeatId, key: Keycode, pressed: bool) -> Result<(), Error> {
        self.agent_pointer(seat)?;
        let Some((keyboard, own_keymap)) = self
            .seat(seat)
            .map(|data| (data.keyboard.clone(), data.own_keymap.clone()))
        else {
            return Err(Error::Refused(format!("no seat {}", seat.0)));
        };
        if keyboard.pressed_keys().contains(&key) == pressed {
            let state = if pressed { "held already" } else { "not held" };
            return Err(Error::Refused(format!("that key is {state}")));
        }
        self.use_keymap(seat, &own_keymap)?;
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        self.send_key(seat, KeyboardSource::MAIN, key, state)
    }

    /// Switches `seat`'s keyboard to `keymap` unless it has it: its clients are sent the new
    /// keymap. A virtual keyboard's keys are read with its own keymap, Yuna's with the seat's.
    pub(crate) fn use_keymap(&mut self, seat: SeatId, keymap: &KeymapFile) -> Result<(), Error> {
        let keyboard = self.agent_keyboard(seat)?;
        if keyboard.keymap_file().id() != keymap.id() {
            keyboard
                .set_keymap_from_keymap_file(self, keymap.clone())
                .map_err(|e| Error::Keyboard(e.to_string()))?;
        }
        Ok(())
    }

    /// The agent seat `seat`'s keyboard; refused for the owner's seat, as its pointer is.
    pub(crate) fn agent_keyboard(&self, seat: SeatId) -> Result<KeyboardHandle<State>, Error> {
        self.agent_pointer(seat)?;
        self.seat(seat)
            .map(|data| data.keyboard.clone())
            .ok_or_else(|| Error::Refused(format!("no seat {}", seat.0)))
    }

    /// Sends `key` from `source` to the surface `seat`'s keyboard is on, with the keymap it has,
    /// and records a press with that surface, for an activation token or a popup grab. Smithay
    /// counts each key's presses by source, so that a key two sources hold is released by the
    /// last of them.
    pub(crate) fn send_key(
        &mut self,
        seat: SeatId,
        source: KeyboardSource,
        key: Keycode,
        state: KeyState,
    ) -> Result<(), Error> {
        let keyboard = self.agent_keyboard(seat)?;
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.input_time();
        keyboard.input_from_source::<(), _>(source, self, key, state, serial, time, |_, _, _| {
            FilterResult::Forward
        });
        if state == KeyState::Pressed
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
    /// key named twice, or a key held already; a key that fails to press or release leaves no
    /// key of the tap held ([`tap_in_order`]).
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
        tap_in_order(&keys, |key, pressed| self.key(seat, key, pressed))
    }

    /// Types `text` on `seat`'s keyboard, to the surface it is on: each character by its keys in
    /// the seat's keymap, the keys that pick its level or lock its group first, the group the
    /// keyboard had locked again at the end; lacking keys, committed to the text field enabled
    /// there, or, with none, through an injected keymap. Text being composed in that field is
    /// cleared first, as an input method's commit replaces it. Refused, with nothing typed, past
    /// [`MAX_TYPED`] characters, for a character that is no keysym, and when a key the text
    /// needs is held. A key that fails midway stops the typing; the group it started in is
    /// locked again, if the keymap leads back there, before the error is returned.
    pub(crate) fn type_text(&mut self, seat: SeatId, text: &str) -> Result<(), Error> {
        self.agent_pointer(seat)?;
        if text.chars().count() > MAX_TYPED {
            return Err(Error::Refused(format!(
                "a call types {MAX_TYPED} characters at most"
            )));
        }
        let held = self
            .seat(seat)
            .map(|data| data.keyboard.pressed_keys())
            .unwrap_or_default();
        let (steps, start_group) = self.typing_steps(seat, text, &held)?;
        // Checked before anything is pressed, so that a refusal types nothing and leaves no key
        // held.
        let needs_held = steps.iter().any(|step| match step {
            Typed::Keys(chord) => chord.iter().any(|key| held.contains(key)),
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
            let done = match step {
                Typed::Keys(chord) => {
                    tap_in_order(&chord, |key, pressed| self.key(seat, key, pressed))
                }
                Typed::Committed(text) => self.commit_text(seat, &text),
                Typed::Injected(keysyms) => {
                    self.inject_keysyms(seat, &keysyms);
                    Ok(())
                }
            };
            if let Err(error) = done {
                self.lock_group_again(seat, start_group);
                return Err(error);
            }
        }
        Ok(())
    }

    /// Types `keysyms` through an injected keymap, to the surface `seat`'s keyboard is on.
    fn inject_keysyms(&mut self, seat: SeatId, keysyms: &[Keysym]) {
        let Some(keyboard) = self.seat(seat).map(|data| data.keyboard.clone()) else {
            return;
        };
        let time = self.input_time();
        // Its keymap has room for 247 keys, from keycode 9 to 255.
        for chunk in keysyms.chunks(247) {
            keyboard.inject_text_keysyms(self, chunk, time);
        }
    }

    /// After a key failed midway through typing, locks `group` again on `seat`'s keyboard, from
    /// the state it is in now, if the keymap leads there; a key that fails here leaves it as it
    /// is, logged.
    fn lock_group_again(&mut self, seat: SeatId, group: u32) {
        let back = self
            .seat(seat)
            .and_then(|data| data.keymap.as_ref())
            .zip(self.typing_start(seat))
            .and_then(|(keymap, now)| keymap.chords_to_group(now, group));
        let Some(chords) = back else {
            tracing::warn!(
                seat = seat.0,
                group,
                "typing failed, and no way back to its group"
            );
            return;
        };
        for chord in chords {
            if let Err(error) = tap_in_order(&chord, |key, pressed| self.key(seat, key, pressed)) {
                tracing::warn!(%error, seat = seat.0, group, "typing failed, its group stays");
                return;
            }
        }
    }

    /// What typing `text` on `seat` does, step by step, from the keyboard's state with `held`
    /// pressed, its committed or injected characters grouped, and the group typing starts in.
    /// When the keymap's switches lead back to that group from nowhere typing ends, typing ends
    /// in another, logged.
    fn typing_steps(
        &self,
        seat: SeatId,
        text: &str,
        held: &HashSet<Keycode>,
    ) -> Result<(Vec<Typed>, u32), Error> {
        let text_field = self.text_field_enabled(seat);
        let (keymap, start) = self
            .seat(seat)
            .and_then(|data| data.keymap.as_ref())
            .zip(self.typing_start(seat))
            .ok_or_else(|| Error::Refused(format!("no agent seat {}", seat.0)))?;
        let mut planner = KeyPlanner::new(keymap, start, held);
        let mut steps: Vec<Typed> = Vec::new();
        for character in text.chars() {
            let keysym = keysym_of_char(character);
            if keysym.raw() == 0 {
                return Err(Error::Refused(format!(
                    "{character:?} cannot be typed: it has no keysym"
                )));
            }
            match planner.chords_for(keysym) {
                Some(chords) => steps.extend(chords.into_iter().map(Typed::Keys)),
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
        let start_group = planner.start_group();
        match planner.finish() {
            Some(back) => steps.extend(back.into_iter().map(Typed::Keys)),
            None => tracing::warn!(
                seat = seat.0,
                group = start_group,
                "typing ends with another group locked: no way back"
            ),
        }
        Ok((steps, start_group))
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

/// Presses `keys` in order, then releases them in the reverse order, each through `key` (the key,
/// and whether it is pressed). When one fails to press or release, every key the tap still holds
/// is released, in the reverse order, each even if another fails, before the first error is
/// returned: a tap that fails leaves no key held.
fn tap_in_order(
    keys: &[Keycode],
    mut key: impl FnMut(Keycode, bool) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut result = Ok(());
    let mut held = Vec::with_capacity(keys.len());
    for &pressed in keys {
        if let Err(error) = key(pressed, true) {
            result = Err(error);
            break;
        }
        held.push(pressed);
    }
    while let Some(released) = held.pop() {
        if let Err(error) = key(released, false)
            && result.is_ok()
        {
            result = Err(error);
        }
    }
    result
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
/// button is released, and offers its source to the surfaces it crosses, all of its seat's; over
/// an X11 window it speaks XDND instead, its source wrapped so that Smithay's cancel at a release
/// there waits for X11's answer (`xwayland/drags.rs`). Refused, its source cancelled, from a
/// client of another seat than the pointer's: one that bound another agent's `wl_seat` and
/// guessed its serial. Touch has no drag: Yuna has no touch. The drag's icon is not drawn: Yuna
/// draws no cursor yet.
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
        let id = seat_id(&seat).filter(|&id| source_seat(&source) == Some(id));
        match (type_, pointer, start, id) {
            (GrabType::Pointer, Some(pointer), Some(start), Some(id)) => {
                let source = self.wayland_drag_started(id, source);
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

/// A drag's end: Smithay tells a Wayland target and the source itself; over an X11 window, or with
/// none, the seat's drags decide (`xwayland/drags.rs`). Both are called from within Smithay's
/// pointer grab: neither calls the pointer.
impl DndGrabHandler for State {
    fn dropped(
        &mut self,
        _target: Option<DndTarget<'_, Self>>,
        _validated: bool,
        seat: Seat<Self>,
        _location: Point<f64, Logical>,
    ) {
        if let Some(seat) = seat_id(&seat) {
            self.wayland_drag_ended(seat, true);
        }
    }

    fn cancelled(&mut self, seat: Seat<Self>, _location: Point<f64, Logical>) {
        if let Some(seat) = seat_id(&seat) {
            self.wayland_drag_ended(seat, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tap's keys pressed and released through a keyboard that fails the call numbered
    /// `failing` (from 0), if any: returns the tap's result and the calls made, each a key and
    /// whether it is pressed.
    fn tap(keys: &[u32], failing: Option<usize>) -> (Result<(), Error>, Vec<(u32, bool)>) {
        let keys: Vec<Keycode> = keys.iter().map(|&raw| Keycode::new(raw)).collect();
        let mut calls = Vec::new();
        let result = tap_in_order(&keys, |key, pressed| {
            let call = calls.len();
            calls.push((key.raw(), pressed));
            if Some(call) == failing {
                Err(Error::Keyboard(format!("call {call} fails")))
            } else {
                Ok(())
            }
        });
        (result, calls)
    }

    #[test]
    fn a_tap_presses_in_order_and_releases_in_reverse() {
        let (result, calls) = tap(&[37, 50, 28], None);
        assert!(result.is_ok());
        assert_eq!(
            calls,
            [
                (37, true),
                (50, true),
                (28, true),
                (28, false),
                (50, false),
                (37, false)
            ]
        );
    }

    /// The third key fails to press: the two held are released, and the press's error returned.
    #[test]
    fn a_key_that_fails_to_press_releases_those_pressed_before() {
        let (result, calls) = tap(&[37, 50, 28], Some(2));
        assert!(
            matches!(&result, Err(Error::Keyboard(why)) if why == "call 2 fails"),
            "{result:?}"
        );
        assert_eq!(
            calls,
            [(37, true), (50, true), (28, true), (50, false), (37, false)]
        );
    }

    /// The first key fails: nothing was pressed, nothing is released.
    #[test]
    fn a_first_key_that_fails_releases_nothing() {
        let (result, calls) = tap(&[37, 50], Some(0));
        assert!(result.is_err());
        assert_eq!(calls, [(37, true)]);
    }

    /// The middle key fails to release: the first is released all the same, and the release's
    /// error returned.
    #[test]
    fn a_key_that_fails_to_release_leaves_the_others_released() {
        let (result, calls) = tap(&[37, 50, 28], Some(4));
        assert!(
            matches!(&result, Err(Error::Keyboard(why)) if why == "call 4 fails"),
            "{result:?}"
        );
        assert_eq!(
            calls,
            [
                (37, true),
                (50, true),
                (28, true),
                (28, false),
                (50, false),
                (37, false)
            ]
        );
    }
}
