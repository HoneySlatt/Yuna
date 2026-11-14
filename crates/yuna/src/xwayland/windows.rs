//! X11 windows on Yuna's side: what each seat's X11 window manager posts (`manager.rs`), turned
//! into windows of the layout as `xdg_toplevel`s are, and the layout's decisions turned back into
//! the manager's commands
//! ([ADR-0066](../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md),
//! [ADR-0068](../../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md),
//! [ADR-0069](../../../../docs/adr/0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md)).
//!
//! An X11 window's life:
//!
//! 1. It is created: Yuna records it, its X11 client and its properties ([`X11Windows`]).
//! 2. It asks to be mapped: a managed window, neither override-redirect nor a dock or a desktop,
//!    is configured to the size [`Layout::size_on_open`](yuna_layout::Layout::size_on_open)
//!    foretells, at its seat's active output, so that its first buffer has it, then mapped. A
//!    dock or a desktop is mapped as it asks, and stays out of the layout.
//! 3. Xwayland gives it a Wayland surface, and both sides a serial (xwayland-shell-v1,
//!    `shell.rs`): the window and its surface are paired, in either order. A managed window
//!    paired gets its record in [`State::toplevels`], [`Shell::X11`], beside xdg-shell's.
//! 4. Mapped, paired and with a buffer, it opens in the layout as an `xdg_toplevel` does at its
//!    first buffer (`shell.rs`): its limits from its size hints, in logical units, its parent
//!    from `WM_TRANSIENT_FOR`, its size committed from its surface, less its frame extents. Its
//!    X11 client, not Xwayland, counts against the cap on windows (ADR-0028).
//! 5. At each arrangement it is told its place: an X11 rectangle of its place's corner times
//!    XWayland's scale S, its size round(S × L) for the logical size L asked, widened by its
//!    frame extents; its `_NET_WM_STATE`, fullscreen, maximised for full width, hidden for
//!    suspended. A hidden window keeps its X11 place. A fullscreen window on an output whose mode
//!    its client emulates gets that mode's rectangle, as Xwayland gives it, so that its surface
//!    gets Xwayland's viewport (ADR-0069). Only what changed is sent.
//! 6. The seat's keyboard on it gives it the X input focus (`manager.rs` follows its model).
//! 7. Unmapped, it leaves the layout; destroyed, Yuna forgets it. Its surface's destruction, which
//!    Xwayland does on unmapping, closes it too.
//!
//! Override-redirect windows, menus and tooltips, are recorded and paired, never shown yet.
//!
//! Commands wait in an outbox until the round's end, when the compositor passes them to each
//! manager (`Compositor::flush`): Yuna's state never holds a manager, whose life is Xwayland's
//! (`mod.rs`).

use std::collections::{HashMap, HashSet};

use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Buffer, Point};
use x11rb::protocol::xproto::Window;
use yuna_layout::geometry::Size;
use yuna_layout::{MAX_SIZE, OpenOptions, OutputId, SeatId, WindowId};
use yuna_motion::Change;

use super::manager::{Command, ManagerEvent, NetState, StateAction, StateChange};
use super::properties::{FrameExtents, Properties, SizeHints, Wishes, X11Rect};
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
    servers: HashMap<u64, X11Server>,
    /// Commands for the managers, passed on at the round's end.
    outbox: Vec<(u64, Command)>,
}

/// One Xwayland's windows.
#[derive(Debug)]
struct X11Server {
    seat: SeatId,
    /// XWayland's scale, S, fixed for its life.
    scale: f64,
    windows: HashMap<Window, X11Window>,
    /// The surfaces whose serial was committed before any window had it, by serial.
    waiting: HashMap<u64, WlSurface>,
    /// The window last given the X input focus.
    focused: Option<Window>,
    /// The stacking last sent, bottom to top.
    stacking: Vec<Window>,
    /// The X11 clients whose window past the cap on windows was said: once each.
    refusals_said: HashSet<u32>,
}

/// One X11 window, as its manager described it.
#[derive(Debug)]
struct X11Window {
    client: u32,
    override_redirect: bool,
    /// Its process, as X11 knows its client's: none without XRes.
    pid: Option<i32>,
    properties: Properties,
    /// The serial Xwayland gave its surface, until the surface with it commits.
    serial: Option<u64>,
    surface: Option<WlSurface>,
    /// It asked to be mapped and Yuna mapped it; cleared when it is unmapped.
    requested: bool,
    /// X11 says it is mapped.
    mapped: bool,
    /// What it asked of `_NET_WM_STATE`, before it opened.
    wishes: Wishes,
    /// Its rectangle as Yuna last configured it.
    told: Option<X11Rect>,
    /// Its states as Yuna last set them.
    state: Option<NetState>,
    /// The size last given to the layout, in logical units.
    committed: Option<Size>,
}

impl X11Windows {
    /// Passes `command` to Xwayland `server`'s manager at the round's end.
    fn send(&mut self, server: u64, command: Command) {
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
        self.window(x11)
            .is_some_and(|window| window.requested && window.mapped)
    }

    /// Whether Xwayland `server` is known: from its start until it is gone.
    pub(crate) fn has_server(&self, server: u64) -> bool {
        self.servers.contains_key(&server)
    }
}

/// An X11 rectangle: a corner in X11 pixels and a logical size times XWayland's scale, the size
/// round(S × L), widened by the window's frame extents, whose geometry starts past them.
fn x11_rect(corner: (i32, i32), size: Size, scale: f64, extents: FrameExtents) -> X11Rect {
    let side = |logical: f64| (logical * scale).round().max(1.0) as u32;
    X11Rect {
        x: corner.0.saturating_sub(to_i32(extents.left)),
        y: corner.1.saturating_sub(to_i32(extents.top)),
        width: side(size.w).saturating_add(extents.left + extents.right),
        height: side(size.h).saturating_add(extents.top + extents.bottom),
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
fn x11_origin(output: &VirtualOutput, scale: f64) -> (i32, i32) {
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
                waiting: HashMap::new(),
                focused: None,
                stacking: Vec::new(),
                refusals_said: HashSet::new(),
            },
        );
    }

    /// Xwayland `server` has ended: its windows close, and its commands go.
    pub(crate) fn x11_server_gone(&mut self, server: u64) {
        if self.x11.servers.remove(&server).is_none() {
            return;
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
                            told: None,
                            state: None,
                            committed: None,
                        },
                    );
                }
            }
            ManagerEvent::Properties { window, properties } => {
                self.x11_properties_changed(server, window, *properties);
            }
            ManagerEvent::MapRequest(window) => self.x11_map_request(server, window),
            ManagerEvent::Mapped(window) => {
                if let Some(record) = self.x11_window_mut(server, window) {
                    record.mapped = true;
                }
                self.x11_reconcile(server, window);
            }
            ManagerEvent::Unmapped(window) => self.x11_unmapped(server, window),
            ManagerEvent::Destroyed(window) => self.x11_destroyed(server, window),
            ManagerEvent::ConfigureRequest { window, asked } => {
                self.x11_configure_request(server, window, asked);
            }
            ManagerEvent::SurfaceSerial { window, serial } => {
                let waiting = self
                    .x11
                    .servers
                    .get_mut(&server)
                    .and_then(|record| record.waiting.remove(&serial));
                match waiting {
                    Some(surface) => self.x11_pair(server, window, surface),
                    None => {
                        if let Some(record) = self.x11_window_mut(server, window) {
                            record.serial = Some(serial);
                        }
                    }
                }
            }
            ManagerEvent::StateRequest { window, change } => {
                self.x11_state_request(server, window, change);
            }
            ManagerEvent::ActivateRequest(window) => self.x11_activate_request(server, window),
            // The phases of Xwayland's start and end, `mod.rs`'s.
            ManagerEvent::Ready | ManagerEvent::Stopped(_) => {}
        }
    }

    fn x11_window_mut(&mut self, server: u64, window: Window) -> Option<&mut X11Window> {
        self.x11.servers.get_mut(&server)?.windows.get_mut(&window)
    }

    /// The toplevel of `window`'s surface, if it has one.
    fn x11_root(&self, server: u64, window: Window) -> Option<WlSurface> {
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
    fn x11_layout_window(&self, server: u64, window: Window) -> Option<WindowId> {
        let root = self.x11_root(server, window)?;
        self.toplevels.get(&root.id())?.window
    }

    /// A surface of Xwayland `server` committed `serial`: it is paired with the window that has
    /// it, or waits for it.
    pub(crate) fn x11_surface_serial(&mut self, server: u64, surface: WlSurface, serial: u64) {
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        let window = record
            .windows
            .iter()
            .find(|(_, window)| window.serial == Some(serial))
            .map(|(&window, _)| window);
        match window {
            Some(window) => self.x11_pair(server, window, surface),
            None => {
                record.waiting.insert(serial, surface);
            }
        }
    }

    /// An Xwayland's surface is gone: it no longer waits, nor is any window's.
    pub(crate) fn x11_surface_destroyed(&mut self, surface: &WlSurface) {
        for record in self.x11.servers.values_mut() {
            record.waiting.retain(|_, waiting| waiting != surface);
            for window in record.windows.values_mut() {
                if window.surface.as_ref() == Some(surface) {
                    window.surface = None;
                }
            }
        }
    }

    /// `window` and `surface` have the same serial: they are paired, and a managed window gets
    /// its record of a window.
    fn x11_pair(&mut self, server: u64, window: Window, surface: WlSurface) {
        let Some(record) = self.x11_window_mut(server, window) else {
            return;
        };
        record.serial = None;
        record.surface = Some(surface);
        self.x11_reconcile(server, window);
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
            (true, true) => self.x11_update_window(root),
            (true, false) => self.close(root),
            (false, true) => self.x11_try_open(root),
            (false, false) => {}
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
        record.properties = properties;
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
    /// at its seat's active output, so that its first buffer has it (ADR-0026); then it is
    /// mapped. A dock or a desktop is mapped as it asks.
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
        let (fullscreen, extents) = (
            x11_window.wishes.fullscreen,
            x11_window.properties.frame_extents,
        );
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
                Ok(size) if size.w > 0.0 && size.h > 0.0 => {
                    let rect = x11_rect(x11_point(corner, scale), size, scale, extents);
                    if let Some(x11_window) = self.x11_window_mut(server, window) {
                        x11_window.told = Some(rect);
                    }
                    self.x11.send(server, Command::Configure { window, rect });
                    self.counters.configures += 1;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "no size foretold for a new X11 window; letting it choose");
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
        if let Some(x11_window) = record.windows.get_mut(&window) {
            x11_window.mapped = false;
            x11_window.requested = false;
            x11_window.state = None;
            x11_window.committed = None;
        }
        if let Some(root) = root {
            self.unmap(&root);
        }
    }

    /// `window` is gone: Yuna forgets it, and its record of a window.
    fn x11_destroyed(&mut self, server: u64, window: Window) {
        let root = self.x11_root(server, window);
        let Some(record) = self.x11.servers.get_mut(&server) else {
            return;
        };
        record.windows.remove(&window);
        record.stacking.retain(|&stacked| stacked != window);
        if record.focused == Some(window) {
            record.focused = None;
        }
        if let Some(root) = root {
            self.toplevel_gone(&root);
        }
    }

    /// `window` asks to be moved or resized to `asked`. A window the layout holds is answered
    /// with its place, which the layout decides, but a fullscreen window shown on an output asking
    /// the rectangle Xwayland gives its client's emulated mode there: granted (ADR-0069). A
    /// hidden fullscreen window is never moved. A window not in the layout is granted what it
    /// asks.
    fn x11_configure_request(&mut self, server: u64, window: Window, asked: X11Rect) {
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
        let answer = match in_layout {
            None => asked,
            Some((id, shown_on)) => {
                let fullscreen = self.layout.window(id).is_some_and(|info| info.fullscreen);
                let emulated = shown_on
                    .and_then(|output| self.output_index(output))
                    .and_then(|index| {
                        emulated_rect(&x11_window.properties, &self.outputs[index], scale)
                    });
                match (fullscreen, emulated) {
                    (true, Some(emulated)) if emulated == asked => asked,
                    _ => x11_window.told.unwrap_or(asked),
                }
            }
        };
        if let Some(x11_window) = self.x11_window_mut(server, window) {
            x11_window.told = Some(answer);
        }
        self.x11.send(
            server,
            Command::Configure {
                window,
                rect: answer,
            },
        );
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
        let full_width = self
            .toplevels
            .get(&x11.surface.id())
            .and_then(|toplevel| toplevel.window)
            .and_then(|window| self.layout.window(window))
            .is_some_and(|info| info.full_width);
        let Some(x11_window) = self.x11.window(x11) else {
            return;
        };
        let fullscreen_on = shown_on
            .filter(|_| told.fullscreen)
            .and_then(|output| self.output_index(output))
            .map(|index| &self.outputs[index]);
        let emulated = fullscreen_on
            .and_then(|output| emulated_rect(&x11_window.properties, output, x11.scale));
        let placed = told.corner.zip(told.size).map(|(corner, size)| {
            let corner = match fullscreen_on {
                Some(output) => x11_origin(output, x11.scale),
                None => x11_point(corner, x11.scale),
            };
            x11_rect(corner, size, x11.scale, x11_window.properties.frame_extents)
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
        if let Some(x11_window) = self.x11_window_mut(x11.server, x11.window) {
            if let Some(rect) = configure {
                x11_window.told = Some(rect);
            }
            if set_state {
                x11_window.state = Some(state);
            }
        }
        if let Some(rect) = configure {
            self.x11.send(
                x11.server,
                Command::Configure {
                    window: x11.window,
                    rect,
                },
            );
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
            let mut below: Vec<Window> = record
                .windows
                .iter()
                .filter(|(window, x11_window)| {
                    x11_window.requested && x11_window.mapped && !on_top.contains(window)
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

    #[test]
    fn an_x11_rectangle_is_the_place_times_s_its_size_rounded() {
        // A tile of 1256 × 1408 at (16, 16), at S = 1.5: 1884 × 2112 at (24, 24).
        let tile = Size {
            w: 1256.0,
            h: 1408.0,
        };
        assert_eq!(
            x11_rect(x11_point((16.0, 16.0), 1.5), tile, 1.5, NO_EXTENTS),
            X11Rect {
                x: 24,
                y: 24,
                width: 1884,
                height: 2112
            }
        );
        // 935 units at 1.5 are 1402.5 X11 pixels: rounded half away, 1403.
        let odd = Size { w: 935.0, h: 1.0 };
        assert_eq!(x11_rect((0, 0), odd, 1.5, NO_EXTENTS).width, 1403);
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
                extents
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
