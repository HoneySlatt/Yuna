//! X11 windows on Yuna's side: what each seat's X11 window manager posts (`manager.rs`) becomes
//! windows of the layout, as `xdg_toplevel`s are, and the layout's decisions become the manager's
//! commands ([ADR-0066](../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md),
//! [ADR-0068](../../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md),
//! [ADR-0069](../../../../docs/adr/0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md)).
//! A window's life, from its creation to its end, is told once, in
//! [ARCHITECTURE](../../../../docs/ARCHITECTURE.md#an-x11-windows-life).
//!
//! [`X11Windows`] holds every Xwayland's windows as their manager described them. A managed window
//! paired with its surface has its record in [`State::toplevels`], [`Shell::X11`], beside
//! xdg-shell's. Commands wait in an outbox until the round's end, when the compositor passes them
//! to each manager (`Compositor::flush`): Yuna's state never holds a manager, whose life is
//! Xwayland's (`mod.rs`).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Buffer, Point};
use x11rb::protocol::xproto::Window;
use yuna_layout::geometry::Size;
use yuna_layout::{MAX_SIZE, OpenOptions, OutputId, SeatId, WindowId};
use yuna_motion::Change;

use super::answers::Waits;
use super::bridge::Selection;
use super::drags::Drags;
use super::manager::{Command, ManagerEvent, NetState, StateAction, StateChange};
use super::menus::MenuPlace;
use super::properties::{FrameExtents, Properties, SizeHints, Wishes, X11Rect};
use crate::activation::TokenOrigin;
use crate::input::window_of_surface;
use crate::output::VirtualOutput;
use crate::render::x11_surface_size;
use crate::shell::{Shell, Told, Toplevel, has_buffer};
use crate::state::State;

/// An X11 client of one Xwayland: the bits of its windows' ids that name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct X11Client {
    pub server: u64,
    pub base: u32,
}

/// What [`Toplevel`] keeps of an X11 window: which one, its surface, and XWayland's scale.
#[derive(Debug, Clone)]
pub(crate) struct X11Toplevel {
    /// The Xwayland it is a window of.
    pub server: u64,
    pub window: Window,
    /// Its Wayland surface, paired with it through xwayland-shell-v1.
    pub surface: WlSurface,
    /// XWayland's scale, S: its X11 pixels per logical unit.
    pub scale: f64,
    client: u32,
}

impl X11Toplevel {
    /// The X11 client behind it, which the cap on windows counts.
    pub(crate) fn client(&self) -> X11Client {
        X11Client {
            server: self.server,
            base: self.client,
        }
    }
}

/// Every Xwayland's X11 windows, and the commands that wait for their managers.
#[derive(Debug, Default)]
pub(crate) struct X11Windows {
    pub(super) servers: HashMap<u64, X11Server>,
    /// Commands for the managers, passed on at the round's end.
    outbox: Vec<(u64, Command)>,
    /// The seats' Wayland selections that Wayland clients changed this round, for the managers
    /// (`selections.rs`).
    pub(super) wayland_changed: Vec<(SeatId, Selection)>,
    /// The seats' drags between X11 and Wayland (`drags.rs`).
    pub(super) drags: Drags,
}

/// One Xwayland's windows.
#[derive(Debug)]
pub(super) struct X11Server {
    pub(super) seat: SeatId,
    /// XWayland's scale, S, fixed for its life.
    pub(super) scale: f64,
    pub(super) windows: HashMap<Window, X11Window>,
    /// The window of each surface paired.
    pub(super) surfaces: HashMap<ObjectId, Window>,
    /// The surfaces whose serial was committed before any window had it, by serial.
    waiting: HashMap<u64, WlSurface>,
    /// The windows whose serial came before their surface committed it, by serial: thousands of
    /// surfaces pairing at once find theirs without a walk of every window.
    serials: HashMap<u64, Window>,
    /// The window last given the X input focus.
    focused: Option<Window>,
    /// The stacking last sent, bottom to top.
    pub(super) stacking: Vec<Window>,
    /// The override-redirect windows mapped, bottom to top as X11 stacks them (`menus.rs`).
    pub(super) menus: Vec<Window>,
    /// Those of them shown, in the same order: what the frame loop and the pointer walk.
    pub(super) shown_menus: Vec<Window>,
    /// How many menus each X11 client has shown, for its cap.
    pub(super) menus_shown_of: HashMap<u32, usize>,
    /// The menus shown with each managed window.
    pub(super) menus_with: HashMap<Window, Vec<Window>>,
    /// The X11 clients whose window past the cap on windows was said: once each.
    refusals_said: HashSet<u32>,
    /// The X11 clients whose override-redirect window past their cap was said: once each.
    pub(super) menu_refusals_said: HashSet<u32>,
    /// The transfers of its selections to Wayland clients passed to its manager and not ended.
    pub(super) transfers_to_wayland: usize,
}

/// One X11 window, as its manager described it.
#[derive(Debug)]
pub(super) struct X11Window {
    pub(super) client: u32,
    pub(super) override_redirect: bool,
    /// Its process, as X11 knows its client's: none without XRes.
    pid: Option<i32>,
    pub(super) properties: Properties,
    /// The serial Xwayland gave its surface, until the surface with it commits.
    serial: Option<u64>,
    pub(super) surface: Option<WlSurface>,
    /// It asked to be mapped and Yuna mapped it; cleared when it is unmapped.
    requested: bool,
    /// X11 says it is mapped.
    pub(super) mapped: bool,
    /// What it asked of `_NET_WM_STATE`, before it opened.
    wishes: Wishes,
    /// Its rectangle as its client made it, and, for an override-redirect window, as its client
    /// moves it: a managed window's own size until Yuna configures it.
    pub(super) geometry: X11Rect,
    /// Its rectangle as Yuna last configured it.
    pub(super) told: Option<X11Rect>,
    /// The logical size the layout last asked of it: 0 on an axis the window chooses itself,
    /// whose requests are granted.
    asked: Option<Size>,
    /// Its states as Yuna last set them.
    state: Option<NetState>,
    /// The size last given to the layout, in logical units.
    committed: Option<Size>,
    /// Yuna told it a size while the layout awaits its commit, after its output's change of mode
    /// asked it another size: its next commit reaches the layout whatever its size. X11
    /// acknowledges no configure, so the first commit after it answers.
    size_asked: bool,
    /// Where it is drawn while it is an override-redirect window shown (`menus.rs`).
    pub(super) menu: Option<MenuPlace>,
    /// When its surface was last called back while it was a menu no output drew.
    pub(super) menu_frame_sent: Option<Duration>,
    /// The sync request and the ping it is waited for (`answers.rs`).
    pub(super) waits: Waits,
    /// The activation its `_NET_STARTUP_ID` carried when it asked to be mapped: a token Yuna
    /// gave, judged when it opens.
    activation: Option<TokenOrigin>,
}

impl X11Window {
    /// Whether it is a managed window X11 maps, Yuna having mapped it when it asked.
    pub(super) fn is_shown_managed(&self) -> bool {
        self.requested && self.mapped
    }

    /// The size of its whole X11 window as it is: as Yuna last configured it, else as its client
    /// made it.
    fn own_size(&self) -> (u32, u32) {
        let rect = self.told.unwrap_or(self.geometry);
        (rect.width, rect.height)
    }
}

impl X11Server {
    /// Puts the shown menus in X11's stacking order again: one was shown, hidden or restacked.
    pub(super) fn order_shown_menus(&mut self) {
        let windows = &self.windows;
        self.shown_menus = self
            .menus
            .iter()
            .copied()
            .filter(|menu| {
                windows
                    .get(menu)
                    .is_some_and(|x11_window| x11_window.menu.is_some())
            })
            .collect();
    }
}

impl X11Windows {
    /// Passes `command` to Xwayland `server`'s manager at the round's end.
    pub(super) fn send(&mut self, server: u64, command: Command) {
        self.outbox.push((server, command));
    }

    /// The commands waiting, in order, for the compositor to pass on.
    pub(crate) fn take_outbox(&mut self) -> Vec<(u64, Command)> {
        std::mem::take(&mut self.outbox)
    }

    fn window(&self, x11: &X11Toplevel) -> Option<&X11Window> {
        self.servers.get(&x11.server)?.windows.get(&x11.window)
    }

    /// Whether `client`'s refusal for a cap is the first: it is said once per X11 client.
    pub(crate) fn first_refusal_of(&mut self, client: X11Client) -> bool {
        self.servers
            .get_mut(&client.server)
            .is_some_and(|server| server.refusals_said.insert(client.base))
    }

    /// Its application id, `WM_CLASS`'s class; its title; its process: X11's record of its
    /// client's, else what `_NET_WM_PID` says. Never Xwayland's.
    pub(crate) fn describe(
        &self,
        x11: &X11Toplevel,
    ) -> (Option<String>, Option<String>, Option<i32>) {
        let Some(window) = self.window(x11) else {
            return (None, None, None);
        };
        let properties = &window.properties;
        (
            properties.class.clone(),
            properties.title().map(str::to_owned),
            window.pid.or(properties.pid),
        )
    }

    /// Where its window geometry starts in its surface, in X11 pixels: past its frame extents'
    /// left and top.
    pub(crate) fn geometry_offset(&self, x11: &X11Toplevel) -> Point<i32, Buffer> {
        let extents = self
            .window(x11)
            .map(|window| window.properties.frame_extents)
            .unwrap_or_default();
        (to_i32(extents.left), to_i32(extents.top)).into()
    }

    /// Whether X11 maps it, Yuna having mapped it when it asked.
    pub(crate) fn is_mapped(&self, x11: &X11Toplevel) -> bool {
        self.window(x11).is_some_and(X11Window::is_shown_managed)
    }

    /// Whether Xwayland `server` is known: from its start until it is gone.
    pub(crate) fn has_server(&self, server: u64) -> bool {
        self.servers.contains_key(&server)
    }

    /// The point in X11's root pixels of `x11`'s window under the pointer at `pointer`, its
    /// surface drawn with its top-left corner at `origin`, both logical: none beyond X11's
    /// coordinates, or for a window gone.
    pub(crate) fn root_point_in(
        &self,
        x11: &X11Toplevel,
        pointer: (f64, f64),
        origin: (f64, f64),
    ) -> Option<(i16, i16)> {
        let window = self.window(x11)?;
        let told = window.told.unwrap_or(window.geometry);
        root_point((told.x, told.y), pointer, origin, x11.scale)
    }
}

/// The point in X11's root pixels a pointer at `pointer` is at, over an X11 window whose surface
/// is drawn with its top-left corner at `origin`, both logical, and whose X11 corner Yuna told it
/// is `corner`: that corner plus the pointer's place in the surface times XWayland's scale,
/// rounded. Measured from what is drawn, so that a window that moves, a fullscreen window centred
/// at a smaller size, or one shown in an emulated mode is read where it shows; as Xwayland itself
/// reads the pointer in its window. None beyond X11's 16-bit coordinates.
fn root_point(
    corner: (i32, i32),
    pointer: (f64, f64),
    origin: (f64, f64),
    scale: f64,
) -> Option<(i16, i16)> {
    let axis = |corner: i32, pointer: f64, origin: f64| {
        let point = f64::from(corner) + ((pointer - origin) * scale).round();
        let in_range =
            point.is_finite() && point >= f64::from(i16::MIN) && point <= f64::from(i16::MAX);
        // In range and whole: the cast is exact.
        in_range.then_some(point as i16)
    };
    Some((
        axis(corner.0, pointer.0, origin.0)?,
        axis(corner.1, pointer.1, origin.1)?,
    ))
}

/// An X11 rectangle: a corner in X11 pixels and a logical size times XWayland's scale, the size
/// round(S × L), widened by the window's frame extents, whose geometry starts past them. On an
/// axis asked 0, which the layout leaves to the window, the window keeps `own`, the side of its
/// whole X11 window as it is.
fn x11_rect(
    corner: (i32, i32),
    size: Size,
    scale: f64,
    extents: FrameExtents,
    own: (u32, u32),
) -> X11Rect {
    let side = |logical: f64, extent: u32, own: u32| {
        if logical > 0.0 {
            ((logical * scale).round().max(1.0) as u32).saturating_add(extent)
        } else {
            own
        }
    };
    X11Rect {
        x: corner.0.saturating_sub(to_i32(extents.left)),
        y: corner.1.saturating_sub(to_i32(extents.top)),
        width: side(size.w, extents.left + extents.right, own.0),
        height: side(size.h, extents.top + extents.bottom, own.1),
    }
}

/// `answer` with each axis the layout leaves to the window, asked 0 in `asked`, taken from what
/// its client asks, `requested`.
fn grant_free_axes(answer: X11Rect, requested: X11Rect, asked: Option<Size>) -> X11Rect {
    let Some(asked) = asked else {
        return answer;
    };
    X11Rect {
        width: if asked.w > 0.0 {
            answer.width
        } else {
            requested.width
        },
        height: if asked.h > 0.0 {
            answer.height
        } else {
            requested.height
        },
        ..answer
    }
}

/// A logical point in X11 pixels: times XWayland's scale, rounded, from the root's origin, which
/// is Yuna's logical origin.
fn x11_point(point: (f64, f64), scale: f64) -> (i32, i32) {
    (
        (point.0 * scale).round() as i32,
        (point.1 * scale).round() as i32,
    )
}

/// An output's top-left corner in X11 pixels, as Xwayland is told it: its logical position, in
/// whole units, times XWayland's scale, rounded, as Smithay gives `xdg_output` to a client of
/// that scale.
pub(super) fn x11_origin(output: &VirtualOutput, scale: f64) -> (i32, i32) {
    let origin = |logical: f64| (logical.round() * scale).round() as i32;
    (origin(output.rect.x), origin(output.rect.y))
}

/// A window's size hints in logical units, as the layout takes them: its window, its X11 size
/// less its frame extents, divided by XWayland's scale and rounded outward, so that the layout
/// never refuses a size the hints allow: the minimum down, the maximum up. 0 for none; a maximum
/// below the minimum is dropped, as for an xdg-shell window.
fn logical_limits(hints: SizeHints, extents: FrameExtents, scale: f64) -> (Size, Size) {
    let inside = |(w, h): (u32, u32)| {
        (
            f64::from(w.saturating_sub(extents.left + extents.right)) / scale,
            f64::from(h.saturating_sub(extents.top + extents.bottom)) / scale,
        )
    };
    let (min_w, min_h) = hints
        .min
        .map(inside)
        .map_or((0.0, 0.0), |(w, h)| (w.floor(), h.floor()));
    let (max_w, max_h) = hints
        .max
        .map(inside)
        .map_or((0.0, 0.0), |(w, h)| (w.ceil(), h.ceil()));
    let axis = |min: f64, max: f64| {
        let (min, max) = (min.clamp(0.0, MAX_SIZE), max.clamp(0.0, MAX_SIZE));
        (min, if max < min { 0.0 } else { max })
    };
    let (min_w, max_w) = axis(min_w, max_w);
    let (min_h, max_h) = axis(min_h, max_h);
    (Size { w: min_w, h: min_h }, Size { w: max_w, h: max_h })
}

/// The size a surface of `x11_size` X11 pixels commits, its frame extents left out, in logical
/// units; none when nothing is left or it is beyond the layout's limit.
fn committed_size(x11_size: (i32, i32), extents: FrameExtents, scale: f64) -> Option<Size> {
    let inside = |side: i32, a: u32, b: u32| f64::from(side) - f64::from(a) - f64::from(b);
    let size = Size {
        w: inside(x11_size.0, extents.left, extents.right) / scale,
        h: inside(x11_size.1, extents.top, extents.bottom) / scale,
    };
    let valid = |side: f64| side > 0.0 && side <= MAX_SIZE;
    (valid(size.w) && valid(size.h)).then_some(size)
}

fn to_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

impl State {
    /// Xwayland `server` of `seat` started, at XWayland's scale `scale`.
    pub(crate) fn x11_server_started(&mut self, server: u64, seat: SeatId, scale: f64) {
        self.x11.servers.insert(
            server,
            X11Server {
                seat,
                scale,
                windows: HashMap::new(),
                surfaces: HashMap::new(),
                waiting: HashMap::new(),
                serials: HashMap::new(),
                focused: None,
                stacking: Vec::new(),
                menus: Vec::new(),
                shown_menus: Vec::new(),
                menus_shown_of: HashMap::new(),
                menus_with: HashMap::new(),
                refusals_said: HashSet::new(),
                menu_refusals_said: HashSet::new(),
                transfers_to_wayland: 0,
            },
        );
    }

    /// Xwayland `server` has ended: its windows close, its commands go, the selections Yuna
    /// held for its X11 clients empty, and its drags end.
    pub(crate) fn x11_server_gone(&mut self, server: u64) {
        self.x11_selections_gone(server);
        self.x11_drags_gone(server);
        // Its menus drawn alone leave the screen now: when Xwayland died in a busy round, its
        // surfaces are destroyed later in the round, once its record, which would redraw them,
        // is gone.
        if self
            .x11
            .servers
            .get(&server)
            .is_some_and(|record| !record.shown_menus.is_empty())
        {
            self.x11_redraw(server);
        }
        let Some(gone) = self.x11.servers.remove(&server) else {
            return;
        };
        if gone
            .windows
            .values()
            .any(|x11_window| x11_window.waits.waiting())
        {
            self.arm_x11_answers();
        }
        self.x11.outbox.retain(|(of, _)| *of != server);
        let roots: Vec<WlSurface> = self
            .toplevels
            .values()
            .filter_map(Toplevel::x11)
            .filter(|x11| x11.server == server)
            .map(|x11| x11.surface.clone())
            .collect();
        for root in roots {
            self.toplevel_gone(&root);
        }
    }

    /// What Xwayland `server`'s manager posted of its windows.
    pub(crate) fn x11_event(&mut self, server: u64, event: ManagerEvent) {
        if !self.x11.has_server(server) {
            return;
        }
        match event {
            ManagerEvent::Created {
                window,
                client,
                override_redirect,
                geometry,
                pid,
                properties,
            } => {
                if let Some(record) = self.x11.servers.get_mut(&server) {
                    record.windows.insert(
                        window,
                        X11Window {
                            client,
                            override_redirect,
                            pid,
                            properties: *properties,
                            serial: None,
                            surface: None,
                            requested: false,
                            mapped: false,
                            wishes: Wishes::default(),
                            geometry,
                            told: None,
                            asked: None,
                            state: None,
                            committed: None,
                            size_asked: false,
                            menu: None,
                            menu_frame_sent: None,
                            waits: Waits::default(),
                            activation: None,
                        },
                    );
                }
            }
            ManagerEvent::Properties { window, properties } => {
                self.x11_properties_changed(server, window, *properties);
            }
            ManagerEvent::MapRequest(window) => self.x11_map_request(server, window),
            ManagerEvent::Mapped {
                window,
                override_redirect,
            } => {
                if let Some(record) = self.x11_window_mut(server, window) {
                    record.mapped = true;
                    record.override_redirect |= override_redirect;
                }
                self.x11_reconcile(server, window);
                self.x11_update_menu(server, window);
            }
            ManagerEvent::Configured { window, rect } => {
                self.x11_menu_configured(server, window, rect);
            }
            ManagerEvent::Restacked { window, above } => {
                self.x11_menu_restacked(server, window, above);
            }
            ManagerEvent::Unmapped(window) => self.x11_unmapped(server, window),
            ManagerEvent::Destroyed(window) => self.x11_destroyed(server, window),
            ManagerEvent::ConfigureRequest { window, asked } => {
                self.x11_configure_request(server, window, asked);
            }
            ManagerEvent::SurfaceSerial { window, serial } => {
                self.x11_window_serial(server, window, serial);
            }
            ManagerEvent::StateRequest { window, change } => {
                self.x11_state_request(server, window, change);
            }
            ManagerEvent::ActivateRequest(window) => self.x11_activate_request(server, window),
            ManagerEvent::SyncCounter { window, value } => {
                self.x11_sync_counter(server, window, value);
            }
            ManagerEvent::PingAnswered(window) => self.x11_ping_answered(server, window),
            ManagerEvent::SelectionTaken { .. }
            | ManagerEvent::SelectionDropped(_)
            | ManagerEvent::ReadWayland { .. }
            | ManagerEvent::TransferToWaylandEnded => self.x11_selection_event(server, event),
            ManagerEvent::X11DragOffered { .. }
            | ManagerEvent::X11DragEntered { .. }
            | ManagerEvent::X11DragLeft(_)
            | ManagerEvent::X11DragDropped(_)
            | ManagerEvent::X11DragAction { .. }
            | ManagerEvent::X11DragEnded(_)
            | ManagerEvent::DragProxyMapped(_)
            | ManagerEvent::DragProxyUnmapped(_)
            | ManagerEvent::WaylandDragStatus { .. }
            | ManagerEvent::ReadWaylandDrag { .. }
            | ManagerEvent::WaylandDragEnded { .. } => self.x11_drag_event(server, event),
            // The phases of Xwayland's start and end, `mod.rs`'s.
            ManagerEvent::Ready | ManagerEvent::Stopped(_) => {}
        }
    }

    pub(super) fn x11_window_mut(&mut self, server: u64, window: Window) -> Option<&mut X11Window> {
        self.x11.servers.get_mut(&server)?.windows.get_mut(&window)
    }

    /// The toplevel of `window`'s surface, if it has one.
    pub(super) fn x11_root(&self, server: u64, window: Window) -> Option<WlSurface> {
        let surface = self
            .x11
            .servers
            .get(&server)?
            .windows
            .get(&window)?
            .surface
            .clone()?;
        self.toplevels
            .contains_key(&surface.id())
            .then_some(surface)
    }

    /// The window `window` is in the layout as, if it is.
    pub(super) fn x11_layout_window(&self, server: u64, window: Window) -> Option<WindowId> {
        let root = self.x11_root(server, window)?;
        self.toplevels.get(&root.id())?.window
    }

    /// Xwayland `server` says `window`'s surface has `serial`: the window is paired with the
    /// surface that committed it already, or waits for it. A window Yuna does not know takes no
    /// surface: the one waiting stays for its own window.
    fn x11_window_serial(&mut self, server: u64, window: Window, serial: u64) {
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        let Some(x11_window) = record.windows.get_mut(&window) else {
            tracing::debug!(
                window,
                "the serial of an X11 window Yuna does not know is ignored"
            );
            return;
        };
        match record.waiting.remove(&serial) {
            Some(surface) => self.x11_pair(server, window, surface),
            None => {
                // Its surface's serial before: that surface never committed it, and is gone.
                if let Some(before) = x11_window.serial.replace(serial) {
                    record.serials.remove(&before);
                }
                record.serials.insert(serial, window);
            }
        }
    }

    /// A surface of Xwayland `server` committed `serial`: it is paired with the window that has
    /// it, or waits for it.
    pub(crate) fn x11_surface_serial(&mut self, server: u64, surface: WlSurface, serial: u64) {
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        match record.serials.remove(&serial) {
            Some(window) => self.x11_pair(server, window, surface),
            None => {
                record.waiting.insert(serial, surface);
            }
        }
    }

    /// An Xwayland's surface is gone: it no longer waits, nor is any window's.
    pub(crate) fn x11_surface_destroyed(&mut self, surface: &WlSurface) {
        let mut unpaired = Vec::new();
        for (&server, record) in &mut self.x11.servers {
            record.waiting.retain(|_, waiting| waiting != surface);
            if let Some(window) = record.surfaces.remove(&surface.id()) {
                if let Some(x11_window) = record.windows.get_mut(&window) {
                    x11_window.surface = None;
                }
                unpaired.push((server, window));
            }
        }
        for (server, window) in unpaired {
            self.x11_update_menu(server, window);
            // Its window may leave the layout before its manager says it is unmapped: its
            // menus go to the rules again now, not then.
            self.x11_place_menus_of(server, window);
        }
    }

    /// `window` and `surface` have the same serial: they are paired, and a managed window gets
    /// its record of a window.
    fn x11_pair(&mut self, server: u64, window: Window, surface: WlSurface) {
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        let Some(x11_window) = record.windows.get_mut(&window) else {
            return;
        };
        if let Some(serial) = x11_window.serial.take() {
            record.serials.remove(&serial);
        }
        if let Some(before) = x11_window.surface.replace(surface.clone()) {
            record.surfaces.remove(&before.id());
        }
        record.surfaces.insert(surface.id(), window);
        self.x11_reconcile(server, window);
        self.x11_update_menu(server, window);
    }

    /// The window of Xwayland surface `surface`, once paired.
    pub(super) fn x11_window_of_surface(&self, surface: &WlSurface) -> Option<(u64, Window)> {
        let id = surface.id();
        self.x11
            .servers
            .iter()
            .find_map(|(&server, record)| Some((server, *record.surfaces.get(&id)?)))
    }

    /// Brings `window`'s record of a window in line with what is known of it: a managed window,
    /// paired, gets one; one mapped, paired and with a buffer opens in the layout.
    fn x11_reconcile(&mut self, server: u64, window: Window) {
        let Some(record) = self.x11.servers.get(&server) else {
            return;
        };
        let scale = record.scale;
        let Some(x11_window) = record.windows.get(&window) else {
            return;
        };
        let managed = x11_window.requested
            && !x11_window.override_redirect
            && x11_window.properties.window_type.is_managed();
        let (Some(surface), true) = (x11_window.surface.clone(), managed) else {
            return;
        };
        if !surface.is_alive() {
            return;
        }
        let wishes = x11_window.wishes;
        let client = x11_window.client;
        let activation = self
            .x11_window_mut(server, window)
            .and_then(|x11_window| x11_window.activation.take());
        self.toplevels.entry(surface.id()).or_insert_with(|| {
            let mut toplevel = Toplevel::new(Shell::X11(X11Toplevel {
                server,
                window,
                surface: surface.clone(),
                scale,
                client,
            }));
            toplevel.wants_fullscreen = wishes.fullscreen;
            toplevel.wants_full_width = wishes.maximized;
            toplevel.waiting_activation = activation;
            toplevel
        });
        self.x11_try_open(&surface);
    }

    /// Opens the X11 window of `root` in the layout if it is mapped, paired and has a buffer.
    fn x11_try_open(&mut self, root: &WlSurface) {
        let Some(toplevel) = self.toplevels.get(&root.id()) else {
            return;
        };
        let Some(x11) = toplevel.x11() else {
            return;
        };
        if toplevel.window.is_none() && has_buffer(root) && self.x11.is_mapped(x11) {
            self.map(root);
            self.x11_record_committed(root);
        }
    }

    /// What the layout needs to open `x11`'s window, from its properties: its limits, its parent,
    /// whether it asked for fullscreen, and the size it committed.
    pub(crate) fn x11_open_options(&self, x11: &X11Toplevel) -> OpenOptions {
        let fullscreen = self
            .toplevels
            .get(&x11.surface.id())
            .is_some_and(|toplevel| toplevel.wants_fullscreen);
        let mut options = self.x11_options(x11.server, x11.window, fullscreen);
        let extents = self
            .x11
            .window(x11)
            .map(|window| window.properties.frame_extents)
            .unwrap_or_default();
        options.committed = x11_surface_size(&x11.surface, x11.scale)
            .and_then(|size| committed_size((size.w, size.h), extents, x11.scale));
        options
    }

    /// The options of `window`, of Xwayland `server`, that its properties give: its limits, its
    /// parent when that window is in the layout, and `fullscreen`.
    fn x11_options(&self, server: u64, window: Window, fullscreen: bool) -> OpenOptions {
        let Some(record) = self.x11.servers.get(&server) else {
            return OpenOptions::default();
        };
        let Some(x11_window) = record.windows.get(&window) else {
            return OpenOptions::default();
        };
        let properties = &x11_window.properties;
        let (min_size, max_size) = logical_limits(
            properties.size_hints,
            properties.frame_extents,
            record.scale,
        );
        let parent = properties
            .transient_for
            .and_then(|parent| self.x11_layout_window(server, parent));
        OpenOptions {
            fullscreen,
            parent,
            min_size,
            max_size,
            ..OpenOptions::default()
        }
    }

    /// Records the size the X11 window of `root` committed, once it opened, as the layout took
    /// it.
    fn x11_record_committed(&mut self, root: &WlSurface) {
        let Some(x11) = self
            .toplevels
            .get(&root.id())
            .and_then(|toplevel| toplevel.x11().cloned())
        else {
            return;
        };
        let committed = self.x11_open_options(&x11).committed;
        if let Some(window) = self.x11_window_mut(x11.server, x11.window) {
            window.committed = committed;
        }
    }

    /// A commit of the X11 window of `root`, `mapped` in the layout or not, with a `buffer` or
    /// not: as an xdg-shell window's, it opens at its first buffer once X11 maps it, passes on
    /// its size and limits while it is open, and closes without a buffer.
    pub(crate) fn x11_toplevel_committed(&mut self, root: &WlSurface, mapped: bool, buffer: bool) {
        match (mapped, buffer) {
            (true, true) => {
                self.x11_answer_size_asked(root);
                self.x11_update_window(root);
            }
            (true, false) => {
                self.close(root);
                // It left the layout though X11 still maps it: its menus go to the rules again.
                let x11 = self
                    .toplevels
                    .get(&root.id())
                    .and_then(Toplevel::x11)
                    .map(|x11| (x11.server, x11.window));
                if let Some((server, window)) = x11 {
                    self.x11_place_menus_of(server, window);
                }
            }
            (false, true) => self.x11_try_open(root),
            (false, false) => {}
        }
    }

    /// A commit of the open X11 window of `root` that answers the size its output's change of
    /// mode asked: the size it last gave the layout is forgotten, so that
    /// [`State::x11_update_window`] gives it again, changed or not.
    fn x11_answer_size_asked(&mut self, root: &WlSurface) {
        let Some(x11) = self
            .toplevels
            .get(&root.id())
            .and_then(|toplevel| toplevel.x11().cloned())
        else {
            return;
        };
        if let Some(record) = self.x11_window_mut(x11.server, x11.window)
            && std::mem::take(&mut record.size_asked)
        {
            record.committed = None;
        }
    }

    /// The size and limits of the open X11 window of `root` go to the layout when they changed.
    fn x11_update_window(&mut self, root: &WlSurface) {
        let Some(toplevel) = self.toplevels.get(&root.id()) else {
            return;
        };
        let (Some(x11), Some(window)) = (toplevel.x11().cloned(), toplevel.window) else {
            return;
        };
        let previous_limits = toplevel.limits;
        let options = self.x11_open_options(&x11);
        let limits = (options.min_size, options.max_size);
        let mut changed = false;
        if limits != previous_limits {
            if let Some(toplevel) = self.toplevels.get_mut(&root.id()) {
                toplevel.limits = limits;
            }
            match self.layout.set_size_limits(window, limits.0, limits.1) {
                Ok(()) => changed = true,
                Err(error) => tracing::debug!(%error, window = window.0, "limits refused"),
            }
        }
        let Some(record) = self.x11_window_mut(x11.server, x11.window) else {
            return;
        };
        if options.committed != record.committed {
            record.committed = options.committed;
            if let Some(size) = options.committed {
                match self.layout.commit_size(window, size) {
                    Ok(()) => changed = true,
                    Err(error) => tracing::debug!(%error, window = window.0, "size refused"),
                }
            }
        }
        if changed {
            self.mark_layout_changed(Change::Animated);
        }
    }

    /// `window`'s properties changed: an open window passes on its new limits and frame extents.
    fn x11_properties_changed(&mut self, server: u64, window: Window, properties: Properties) {
        let Some(record) = self.x11_window_mut(server, window) else {
            return;
        };
        let parent_changed = record.properties.transient_for != properties.transient_for;
        let counter_changed = record.properties.sync_counter != properties.sync_counter;
        record.properties = properties;
        // Its title and class among them.
        self.note_description_changed();
        if parent_changed {
            self.x11_place_menu_again(server, window);
        }
        if counter_changed {
            self.x11_sync_counter_changed(server, window);
        }
        if let Some(root) = self.x11_root(server, window)
            && self
                .toplevels
                .get(&root.id())
                .is_some_and(|toplevel| toplevel.window.is_some())
        {
            self.x11_update_window(&root);
        }
    }

    /// `window` asks to be mapped. A managed window is first configured to the size it opens at,
    /// at its seat's active output, so that its first buffer has it (ADR-0026), keeping its own
    /// size on an axis the layout leaves to it; then it is mapped. A dock or a desktop is mapped
    /// as it asks.
    fn x11_map_request(&mut self, server: u64, window: Window) {
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        let (seat, scale) = (record.seat, record.scale);
        let Some(x11_window) = record.windows.get_mut(&window) else {
            return;
        };
        x11_window.override_redirect = false;
        x11_window.requested = true;
        let managed = x11_window.properties.window_type.is_managed();
        x11_window.wishes = x11_window.properties.wishes;
        let (fullscreen, extents, own) = (
            x11_window.wishes.fullscreen,
            x11_window.properties.frame_extents,
            x11_window.own_size(),
        );
        let startup_id = x11_window.properties.startup_id.clone();
        // Read when it asks to be mapped: a token Yuna gave makes its opening an activation.
        let activation = startup_id.and_then(|token| self.take_activation_token(&token));
        if let Some(x11_window) = self.x11_window_mut(server, window) {
            x11_window.activation = activation;
        }
        if managed {
            let options = self.x11_options(server, window, fullscreen);
            let size = self.layout.size_on_open(seat, &options);
            let corner = self
                .layout
                .active_output(seat)
                .and_then(|output| self.output_index(output))
                .map_or((0.0, 0.0), |index| {
                    let rect = self.outputs[index].rect;
                    (rect.x, rect.y)
                });
            match size {
                Ok(size) => {
                    let rect = x11_rect(x11_point(corner, scale), size, scale, extents, own);
                    if let Some(x11_window) = self.x11_window_mut(server, window) {
                        x11_window.asked = Some(size);
                    }
                    self.x11_configure(server, window, rect);
                    self.counters.configures += 1;
                }
                // Its client's values are checked before the layout sees them: a refusal is
                // the layout's own, and the window keeps the place its client gave it.
                Err(error) => {
                    tracing::debug!(%error, "no size foretold for a new X11 window; letting it choose");
                }
            }
        }
        self.x11.send(server, Command::Map(window));
        self.x11_reconcile(server, window);
    }

    /// `window` was unmapped: it leaves the layout, and must ask again to be mapped.
    fn x11_unmapped(&mut self, server: u64, window: Window) {
        let root = self.x11_root(server, window);
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        if record.focused == Some(window) {
            record.focused = None;
        }
        record.menus.retain(|&menu| menu != window);
        if let Some(x11_window) = record.windows.get_mut(&window) {
            x11_window.mapped = false;
            x11_window.requested = false;
            x11_window.state = None;
            x11_window.committed = None;
            x11_window.size_asked = false;
            x11_window.asked = None;
            // Its manager deletes `_NET_WM_STATE` at withdrawal; the property's own change may
            // come after the window asks to be mapped again.
            x11_window.properties.wishes = Wishes::default();
        }
        if let Some(root) = root {
            self.unmap(&root);
        }
        self.x11_waits_end(server, window);
        self.x11_update_menu(server, window);
        self.x11_place_menus_of(server, window);
    }

    /// `window` is gone: Yuna forgets it, and its record of a window.
    fn x11_destroyed(&mut self, server: u64, window: Window) {
        let root = self.x11_root(server, window);
        let shown = self
            .x11
            .servers
            .get(&server)
            .and_then(|record| record.windows.get(&window))
            .is_some_and(|x11_window| x11_window.menu.is_some());
        if shown {
            self.x11_set_menu(server, window, None);
        }
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        let removed = record.windows.remove(&window);
        if let Some(serial) = removed.as_ref().and_then(|x11_window| x11_window.serial) {
            record.serials.remove(&serial);
        }
        let waited = removed
            .as_ref()
            .is_some_and(|x11_window| x11_window.waits.waiting());
        record.surfaces.retain(|_, paired| *paired != window);
        record.stacking.retain(|&stacked| stacked != window);
        record.menus.retain(|&menu| menu != window);
        if record.focused == Some(window) {
            record.focused = None;
        }
        if let Some(root) = root {
            self.toplevel_gone(&root);
        }
        if shown {
            self.x11_redraw(server);
        }
        if waited {
            self.arm_x11_answers();
        }
        self.x11_place_menus_of(server, window);
    }

    /// `window` asks to be moved or resized to `requested`. A window that has not asked to be
    /// mapped is granted it: no manager holds it yet. One that has is answered with the place Yuna
    /// gave it, the one foretold until it opens, then its place in the layout, but on an axis the
    /// layout leaves to it, where its request is granted; and a fullscreen window shown on an
    /// output asking the rectangle Xwayland gives its client's emulated mode there is granted it
    /// (ADR-0069). A hidden fullscreen window is never moved.
    fn x11_configure_request(&mut self, server: u64, window: Window, requested: X11Rect) {
        let Some(scale) = self.x11.servers.get(&server).map(|record| record.scale) else {
            return;
        };
        let in_layout = self.x11_root(server, window).and_then(|root| {
            let toplevel = self.toplevels.get(&root.id())?;
            Some((toplevel.window?, toplevel.shown_on))
        });
        let Some(x11_window) = self
            .x11
            .servers
            .get(&server)
            .and_then(|record| record.windows.get(&window))
        else {
            return;
        };
        let given = grant_free_axes(
            x11_window.told.unwrap_or(requested),
            requested,
            x11_window.asked,
        );
        let answer = match in_layout {
            _ if !x11_window.requested => requested,
            None => given,
            Some((id, shown_on)) => {
                let fullscreen = self.layout.window(id).is_some_and(|info| info.fullscreen);
                let emulated = shown_on
                    .and_then(|output| self.output_index(output))
                    .and_then(|index| {
                        emulated_rect(&x11_window.properties, &self.outputs[index], scale)
                    });
                match (fullscreen, emulated) {
                    (true, Some(emulated)) if emulated == requested => requested,
                    _ => given,
                }
            }
        };
        self.x11_configure(server, window, answer);
        self.counters.configures += 1;
    }

    /// `window`'s client asks to change its fullscreen or maximised state. Open, the layout
    /// decides, and the request is answered as an xdg-shell one is; before, the window will open
    /// so.
    fn x11_state_request(&mut self, server: u64, window: Window, change: StateChange) {
        let wanted = |current: bool| match change.action {
            StateAction::Add => true,
            StateAction::Remove => false,
            StateAction::Toggle => !current,
        };
        let root = self.x11_root(server, window);
        let open = root.as_ref().and_then(|root| {
            let id = self.toplevels.get(&root.id())?.window?;
            Some((root.clone(), id))
        });
        match open {
            Some((root, id)) => {
                let info = self.layout.window(id);
                if change.fullscreen {
                    let current = info.as_ref().is_some_and(|info| info.fullscreen);
                    let outcome = self.layout.set_fullscreen(id, wanted(current));
                    self.answer_request(&root, id, outcome);
                }
                if change.maximized {
                    let current = info.as_ref().is_some_and(|info| info.full_width);
                    let outcome = self.layout.set_full_width(id, wanted(current));
                    self.answer_request(&root, id, outcome);
                }
            }
            None => {
                let toplevel = root
                    .as_ref()
                    .and_then(|root| self.toplevels.get_mut(&root.id()));
                if let Some(toplevel) = toplevel {
                    if change.fullscreen {
                        toplevel.wants_fullscreen = wanted(toplevel.wants_fullscreen);
                    }
                    if change.maximized {
                        toplevel.wants_full_width = wanted(toplevel.wants_full_width);
                    }
                } else if let Some(x11_window) = self.x11_window_mut(server, window) {
                    if change.fullscreen {
                        x11_window.wishes.fullscreen = wanted(x11_window.wishes.fullscreen);
                    }
                    if change.maximized {
                        x11_window.wishes.maximized = wanted(x11_window.wishes.maximized);
                    }
                }
            }
        }
    }

    /// `window`'s client asks for it to be activated (`_NET_ACTIVE_WINDOW`), as xdg-activation
    /// without a token: the layout takes it when the client has its seat's focus, moving it
    /// among its own windows; otherwise the window is marked urgent.
    fn x11_activate_request(&mut self, server: u64, window: Window) {
        let Some(id) = self.x11_layout_window(server, window) else {
            tracing::debug!(window, "an activation of an X11 window not open is ignored");
            return;
        };
        let Some(client) = self
            .x11
            .servers
            .get(&server)
            .and_then(|record| record.windows.get(&window))
            .map(|x11_window| X11Client {
                server,
                base: x11_window.client,
            })
        else {
            return;
        };
        let seat = self.layout.window(id).map(|info| info.seat);
        let focused_client = seat
            .and_then(|seat| self.layout.focused(seat))
            .and_then(|focused| self.windows.get(&focused))
            .and_then(|surface| self.toplevels.get(surface))
            .and_then(Toplevel::x11)
            .map(X11Toplevel::client);
        if focused_client == Some(client) {
            match self.layout.request_activation(id) {
                Ok(true) => {
                    self.mark_layout_changed(Change::Animated);
                    return;
                }
                Ok(false) => {
                    tracing::debug!(window = id.0, "a fullscreen window holds the screen");
                }
                Err(error) => tracing::debug!(%error, window = id.0, "activation refused"),
            }
        }
        self.mark_urgent(id);
    }

    /// Tells the X11 window `x11` what an arrangement says: its rectangle when it is placed, kept
    /// when it is hidden or slides out of view; its states. A fullscreen window is at its
    /// output's corner, as X11 has a fullscreen window, though the layout centres it at the size
    /// it committed until it takes the output's; on an output whose mode its client emulates, it
    /// gets that mode's rectangle. Only a change is sent, unless a request of its client is
    /// `owed` an answer.
    pub(crate) fn tell_x11(
        &mut self,
        x11: &X11Toplevel,
        told: Told,
        shown_on: Option<OutputId>,
        owed: bool,
    ) {
        let Some(x11_window) = self.x11.window(x11) else {
            return;
        };
        // A hidden window keeps the state it was told: the layout's lookup walks the workspaces,
        // which thousands of hidden windows at each arrangement would make quadratic.
        let full_width = if told.hidden {
            x11_window.state.is_some_and(|state| state.maximized)
        } else {
            self.toplevels
                .get(&x11.surface.id())
                .and_then(|toplevel| toplevel.window)
                .and_then(|window| self.layout.window(window))
                .is_some_and(|info| info.full_width)
        };
        let fullscreen_on = shown_on
            .filter(|_| told.fullscreen)
            .and_then(|output| self.output_index(output))
            .map(|index| &self.outputs[index]);
        let emulated = fullscreen_on
            .and_then(|output| emulated_rect(&x11_window.properties, output, x11.scale));
        let own = x11_window.own_size();
        let placed = told.corner.zip(told.size).map(|(corner, size)| {
            let corner = match fullscreen_on {
                Some(output) => x11_origin(output, x11.scale),
                None => x11_point(corner, x11.scale),
            };
            x11_rect(
                corner,
                size,
                x11.scale,
                x11_window.properties.frame_extents,
                own,
            )
        });
        let rect = if told.hidden {
            None
        } else {
            emulated.or(placed)
        };
        let state = NetState {
            fullscreen: told.fullscreen,
            maximized: full_width,
            hidden: told.hidden,
        };
        let configure = match rect {
            Some(rect) if Some(rect) != x11_window.told || owed => Some(rect),
            None if owed => x11_window.told,
            _ => None,
        };
        let set_state = Some(state) != x11_window.state || owed;
        let awaits = self
            .toplevels
            .get(&x11.surface.id())
            .and_then(|toplevel| toplevel.window)
            .is_some_and(|window| self.layout.awaits_commit(window));
        if let Some(x11_window) = self.x11_window_mut(x11.server, x11.window) {
            if set_state {
                x11_window.state = Some(state);
            }
            if placed.is_some() && !told.hidden {
                x11_window.asked = told.size;
                // Configured to it, or kept at a rectangle that already gives it, as an
                // emulated mode's: either way its next commit is its answer.
                x11_window.size_asked |= awaits;
            }
        }
        if let Some(rect) = configure {
            self.x11_configure(x11.server, x11.window, rect);
        }
        if set_state {
            self.x11.send(
                x11.server,
                Command::State {
                    window: x11.window,
                    state,
                },
            );
        }
        if configure.is_some() || set_state {
            self.counters.configures += 1;
        }
    }

    /// Sets the X11 window `x11`'s states again, as they are: the answer to a request the layout
    /// refused.
    pub(crate) fn x11_tell_state_again(&mut self, x11: &X11Toplevel) {
        let Some(state) = self
            .x11
            .window(x11)
            .map(|window| window.state.unwrap_or_default())
        else {
            return;
        };
        self.x11.send(
            x11.server,
            Command::State {
                window: x11.window,
                state,
            },
        );
        self.counters.configures += 1;
    }

    /// `seat`'s keyboard went to `focus`: each of the seat's Xwaylands gives its X input focus to
    /// the X11 window of that surface, or to none when it is not one of its windows.
    pub(crate) fn focus_x11(&mut self, seat: SeatId, focus: Option<&WlSurface>) {
        let target = focus
            .and_then(window_of_surface)
            .and_then(|root| self.toplevels.get(&root.id()))
            .and_then(Toplevel::x11)
            .map(|x11| (x11.server, x11.window));
        let mut commands = Vec::new();
        for (&server, record) in &mut self.x11.servers {
            if record.seat != seat {
                continue;
            }
            let wanted = target
                .filter(|(of, _)| *of == server)
                .map(|(_, window)| window);
            if wanted != record.focused {
                record.focused = wanted;
                commands.push((server, Command::Focus(wanted)));
            }
        }
        self.x11.outbox.extend(commands);
    }

    /// Stacks each Xwayland's windows as Yuna shows them, after an arrangement: those no output
    /// tells visible at the bottom; then the tiled windows shown, which never overlap, by id, so
    /// that a column moving restacks nothing; then the others shown, floating or fullscreen,
    /// bottom to top as the layout draws them. Xwayland finds the X11 window under the pointer
    /// through X11's stacking, and a hidden window keeps its X11 place, maybe under a shown one.
    /// Sent only when it changed.
    pub(crate) fn restack_x11_windows(&mut self) {
        if self.x11.servers.is_empty() {
            return;
        }
        // Each server's shown windows: tiled, then over them.
        let mut shown: HashMap<u64, (Vec<Window>, Vec<Window>)> = HashMap::new();
        for output in &self.outputs {
            let told: HashSet<WindowId> = output.told.iter().copied().collect();
            for placed in output
                .placed
                .iter()
                .filter(|placed| told.contains(&placed.window))
            {
                let x11 = self
                    .windows
                    .get(&placed.window)
                    .and_then(|surface| self.toplevels.get(surface))
                    .and_then(Toplevel::x11);
                if let Some(x11) = x11 {
                    let (tiled, over) = shown.entry(x11.server).or_default();
                    if placed.tiled && !placed.fullscreen {
                        tiled.push(x11.window);
                    } else {
                        over.push(x11.window);
                    }
                }
            }
        }
        let mut commands = Vec::new();
        for (&server, record) in &mut self.x11.servers {
            let (mut tiled, over) = shown.remove(&server).unwrap_or_default();
            tiled.sort_unstable();
            let on_top: Vec<Window> = tiled.into_iter().chain(over).collect();
            // A set, so that sorting a thousand windows stays linear in them.
            let shown_on_top: HashSet<Window> = on_top.iter().copied().collect();
            let mut below: Vec<Window> = record
                .windows
                .iter()
                .filter(|(window, x11_window)| {
                    x11_window.requested && x11_window.mapped && !shown_on_top.contains(window)
                })
                .map(|(&window, _)| window)
                .collect();
            below.sort_unstable();
            below.extend(on_top);
            if below != record.stacking {
                record.stacking = below.clone();
                commands.push((server, Command::Restack(below)));
            }
        }
        self.x11.outbox.extend(commands);
    }

    /// Asks the X11 window `x11` to close: `WM_DELETE_WINDOW` when it takes it, else its client
    /// is ended (`manager.rs`).
    pub(crate) fn close_x11(&mut self, x11: &X11Toplevel) {
        self.x11.send(x11.server, Command::Close(x11.window));
        self.x11_ping(x11.server, x11.window);
    }
}

/// The rectangle of the mode `properties`' client emulates on `output`, as Xwayland put it in
/// `_XWAYLAND_RANDR_EMU_MONITOR_RECTS`: the one at that output's X11 position.
fn emulated_rect(properties: &Properties, output: &VirtualOutput, scale: f64) -> Option<X11Rect> {
    let origin = x11_origin(output, scale);
    properties
        .emulated
        .iter()
        .copied()
        .find(|rect| (rect.x, rect.y) == origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_EXTENTS: FrameExtents = FrameExtents {
        left: 0,
        right: 0,
        top: 0,
        bottom: 0,
    };

    /// The window's own size, where no axis is left to it.
    const NO_OWN: (u32, u32) = (0, 0);

    #[test]
    fn an_x11_rectangle_is_the_place_times_s_its_size_rounded() {
        // A tile of 1256 × 1408 at (16, 16), at S = 1.5: 1884 × 2112 at (24, 24).
        let tile = Size {
            w: 1256.0,
            h: 1408.0,
        };
        assert_eq!(
            x11_rect(x11_point((16.0, 16.0), 1.5), tile, 1.5, NO_EXTENTS, NO_OWN),
            X11Rect {
                x: 24,
                y: 24,
                width: 1884,
                height: 2112
            }
        );
        // 935 units at 1.5 are 1402.5 X11 pixels: rounded half away, 1403.
        let odd = Size { w: 935.0, h: 1.0 };
        assert_eq!(x11_rect((0, 0), odd, 1.5, NO_EXTENTS, NO_OWN).width, 1403);
        // A shadow of 26 left, 20 right, 23 top, 29 bottom widens the X11 window around the
        // place, which is the window's geometry.
        let extents = FrameExtents {
            left: 26,
            right: 20,
            top: 23,
            bottom: 29,
        };
        assert_eq!(
            x11_rect(
                x11_point((100.0, 10.0), 1.0),
                Size { w: 200.0, h: 100.0 },
                1.0,
                extents,
                NO_OWN
            ),
            X11Rect {
                x: 74,
                y: -13,
                width: 246,
                height: 152
            }
        );
    }

    #[test]
    fn an_axis_asked_0_keeps_the_windows_own_side() {
        // A floating window choosing its width, its height fixed at 100 units: 300 X11 pixels
        // wide as it is, 150 tall at S = 1.5, shadow included.
        let extents = FrameExtents {
            left: 10,
            right: 10,
            top: 5,
            bottom: 5,
        };
        let asked = Size { w: 0.0, h: 100.0 };
        assert_eq!(
            x11_rect((60, 30), asked, 1.5, extents, (300, 999)),
            X11Rect {
                x: 50,
                y: 25,
                width: 300,
                height: 160
            }
        );
        // Both axes its own: never the 1 × 1 a 0 would round to.
        let free = x11_rect((0, 0), Size::default(), 1.5, NO_EXTENTS, (640, 480));
        assert_eq!((free.width, free.height), (640, 480));
    }

    #[test]
    fn a_request_is_granted_on_the_axes_left_to_the_window() {
        let told = X11Rect {
            x: 24,
            y: 24,
            width: 640,
            height: 480,
        };
        let requested = X11Rect {
            x: 5,
            y: 5,
            width: 800,
            height: 600,
        };
        // Its width its own: granted; its height and its place the layout's.
        let answer = grant_free_axes(told, requested, Some(Size { w: 0.0, h: 320.0 }));
        assert_eq!(answer, X11Rect { width: 800, ..told });
        // A tile chooses nothing; before any size is asked, nothing is granted either.
        let tile = Size {
            w: 936.0,
            h: 1048.0,
        };
        assert_eq!(grant_free_axes(told, requested, Some(tile)), told);
        assert_eq!(grant_free_axes(told, requested, None), told);
    }

    #[test]
    fn size_hints_become_logical_limits_rounded_outward() {
        // At S = 1.5: a minimum of 301 × 151 X11 pixels is 200.67 × 100.67 units, 200 × 100 down;
        // a maximum of 899 × 449 is 599.33 × 299.33, 600 × 300 up.
        let hints = SizeHints {
            min: Some((301, 151)),
            max: Some((899, 449)),
            ..SizeHints::default()
        };
        assert_eq!(
            logical_limits(hints, NO_EXTENTS, 1.5),
            (Size { w: 200.0, h: 100.0 }, Size { w: 600.0, h: 300.0 })
        );
        // The frame extents are no part of the window the layout sizes.
        let extents = FrameExtents {
            left: 10,
            right: 10,
            top: 5,
            bottom: 5,
        };
        let hints = SizeHints {
            min: Some((120, 110)),
            ..SizeHints::default()
        };
        assert_eq!(
            logical_limits(hints, extents, 1.0).0,
            Size { w: 100.0, h: 100.0 }
        );
        // A maximum below the minimum is dropped; none is 0.
        let hints = SizeHints {
            min: Some((300, 300)),
            max: Some((150, 600)),
            ..SizeHints::default()
        };
        assert_eq!(
            logical_limits(hints, NO_EXTENTS, 1.0),
            (Size { w: 300.0, h: 300.0 }, Size { w: 0.0, h: 600.0 })
        );
        assert_eq!(
            logical_limits(SizeHints::default(), NO_EXTENTS, 1.5),
            (Size::default(), Size::default())
        );
    }

    /// R10: a drag's point is the corner Yuna told the window plus the pointer's place in its
    /// drawn surface times S, rounded; none beyond X11's coordinates.
    #[test]
    fn a_drags_root_point_is_the_told_corner_plus_the_drawn_offset_times_s() {
        // S = 1: the window told (100, 50), drawn there; 10.5 rounds half away from zero.
        assert_eq!(
            root_point((100, 50), (110.5, 60.25), (100.0, 50.0), 1.0),
            Some((111, 60))
        );
        // S = 1.5: told (150, 75), its surface drawn at (100, 50) logical.
        assert_eq!(
            root_point((150, 75), (110.0, 60.0), (100.0, 50.0), 1.5),
            Some((165, 90))
        );
        // A window drawn away from its told place, moving: read where it is drawn.
        assert_eq!(
            root_point((150, 75), (310.0, 60.0), (300.0, 50.0), 1.5),
            Some((165, 90))
        );
        // Fullscreen at S = 1.5 on a 1920 × 1080 output at (0, 0), told the output's corner,
        // centred at 1280 × 720 X11 pixels, 853.33 × 480 logical: the output's centre is the
        // window's.
        let width = 1280.0 / 1.5;
        let origin = ((1920.0 - width) / 2.0, (1080.0 - 480.0) / 2.0);
        assert_eq!(
            root_point((0, 0), (960.0, 540.0), origin, 1.5),
            Some((640, 360))
        );
        // Beyond 16 bits, on either axis.
        assert_eq!(
            root_point((32_000, 0), (1000.0, 0.0), (0.0, 0.0), 1.0),
            None
        );
        assert_eq!(
            root_point((0, -32_000), (0.0, -1000.0), (0.0, 0.0), 1.0),
            None
        );
        assert_eq!(root_point((0, 0), (f64::NAN, 0.0), (0.0, 0.0), 1.0), None);
    }

    #[test]
    fn the_committed_size_leaves_the_frame_extents_out() {
        assert_eq!(
            committed_size((1884, 2112), NO_EXTENTS, 1.5),
            Some(Size {
                w: 1256.0,
                h: 1408.0
            })
        );
        let extents = FrameExtents {
            left: 30,
            right: 30,
            top: 20,
            bottom: 40,
        };
        assert_eq!(
            committed_size((260, 160), extents, 1.0),
            Some(Size { w: 200.0, h: 100.0 })
        );
        // Extents as wide as the window leave nothing.
        assert_eq!(committed_size((60, 160), extents, 1.0), None);
    }
}
