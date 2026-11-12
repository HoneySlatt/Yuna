//! `xdg-shell` and `xdg-decoration` in front of the layout model
//! ([ADR-0024](../../../docs/adr/0024-no-title-bars.md),
//! [ADR-0026](../../../docs/adr/0026-real-clients-in-the-layout.md)).
//!
//! A toplevel's life: its initial commit gets a first configure with the size
//! [`Layout::size_on_open`](yuna_layout::Layout::size_on_open) foretells; its first buffer opens
//! it in the layout, with its parent, its limits and its window geometry; each later commit
//! passes on its limits and geometry when they change; a commit without a buffer, or the
//! destruction of its toplevel or of its surface, closes it. Its configures follow the layout's
//! arrangement, at refresh boundaries (`frames.rs`). Fullscreen and maximise requests always get
//! a configure, as xdg-shell asks, even when the layout refuses them or nothing changes. A window
//! the layout refuses to open, or that would pass a cap on windows ([`MAX_WINDOWS_PER_CLIENT`],
//! [`MAX_WINDOWS`]), is not shown; its client gets frame callbacks at the hidden windows' rate.
//! Popups are in `popups.rs`; activation requests, in `activation.rs`.

use std::sync::atomic::Ordering;
use std::time::Duration;

use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::{wl_output, wl_seat, wl_surface::WlSurface};
use smithay::utils::{Logical, Rectangle, Serial};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, SurfaceCachedState, ToplevelStateSet, ToplevelSurface,
    XdgShellHandler, XdgShellState,
};
use yuna_layout::geometry::Size;
use yuna_layout::{MAX_SIZE, OpenOptions, OutputId, WindowId};

use crate::activation::TokenValidity;
use crate::state::{ClientState, OWNER_SEAT, State};

/// The `xdg_toplevel` version that has the `suspended` state.
const SUSPENDED_SINCE: u32 = 6;

/// The most windows one client may have in the layout, so that one client cannot fill it alone
/// (see [`MAX_WINDOWS`]). A window over the cap is not shown, never a protocol error, so that its
/// client keeps its other windows and its user's data.
pub const MAX_WINDOWS_PER_CLIENT: usize = 1_000;

/// The most windows the layout holds, all clients together. The layout walks a dwindle tree by
/// recursion, as deep as it has windows: 10,000 windows in one tree work, 100,000 overflow the
/// stack (ADR-0020). A window over the cap is treated as one over its client's cap.
pub const MAX_WINDOWS: usize = 10_000;

/// What Yuna keeps of an `xdg_toplevel`.
#[derive(Debug)]
pub(crate) struct Toplevel {
    pub surface: ToplevelSurface,
    /// Its id in the layout, while it is mapped.
    pub window: Option<WindowId>,
    /// Asked before it was mapped, applied when it is.
    pub wants_fullscreen: bool,
    pub wants_full_width: bool,
    /// The minimum and maximum sizes last given to the layout.
    pub limits: (Size, Size),
    /// Its window geometry, from its last commit: where its window starts in its surface.
    pub geometry: Rectangle<i32, Logical>,
    /// The output it is shown on, from the last arrangement; none when it is hidden.
    pub shown_on: Option<OutputId>,
    /// When its surfaces were last sent frame callbacks.
    pub last_frame_sent: Option<Duration>,
    /// The layout refused to open it: it has a buffer, but no output shows it.
    pub refused: bool,
    /// A request it made is owed a configure, even if nothing changes (xdg-shell).
    pub configure_owed: bool,
    /// An activation was refused, or came without a valid token, and the window has not had the
    /// seat's focus since (`activation.rs`).
    pub urgent: bool,
    /// An activation asked before it was mapped, applied when it is.
    pub waiting_activation: Option<TokenValidity>,
}

impl Toplevel {
    fn new(surface: ToplevelSurface) -> Toplevel {
        Toplevel {
            surface,
            window: None,
            wants_fullscreen: false,
            wants_full_width: false,
            limits: (Size::default(), Size::default()),
            geometry: Rectangle::default(),
            shown_on: None,
            last_frame_sent: None,
            refused: false,
            configure_owed: false,
            urgent: false,
            waiting_activation: None,
        }
    }

    pub fn wl_surface(&self) -> &WlSurface {
        self.surface.wl_surface()
    }

    /// Whether it has a buffer that no output shows: mapped on no output, or refused by the
    /// layout. Its content updates are discarded and its frame callbacks come at the hidden
    /// windows' rate.
    pub fn is_unseen(&self) -> bool {
        match self.window {
            Some(_) => self.shown_on.is_none(),
            None => self.refused,
        }
    }
}

/// The states a toplevel is told: tiled on all four edges always (ADR-0024), then fullscreen,
/// activated and suspended as given. Built in one order, so that equal sets compare equal and
/// an unchanged state sends no configure.
fn toplevel_states(fullscreen: bool, activated: bool, suspended: bool) -> ToplevelStateSet {
    let mut states = ToplevelStateSet::default();
    for state in [
        xdg_toplevel::State::TiledLeft,
        xdg_toplevel::State::TiledRight,
        xdg_toplevel::State::TiledTop,
        xdg_toplevel::State::TiledBottom,
    ] {
        states.set(state);
    }
    if fullscreen {
        states.set(xdg_toplevel::State::Fullscreen);
    }
    if activated {
        states.set(xdg_toplevel::State::Activated);
    }
    if suspended {
        states.set(xdg_toplevel::State::Suspended);
    }
    states
}

/// A size the layout asked, in whole logical units, as a configure carries it.
fn configure_size(size: Size) -> smithay::utils::Size<i32, Logical> {
    (size.w.round() as i32, size.h.round() as i32).into()
}

/// A client's minimum and maximum sizes as the layout takes them: 0 on an axis for none, never
/// above the layout's limit, and a maximum below its minimum dropped, so that a client's
/// nonsense never keeps its window from opening.
fn size_limits(surface: &WlSurface) -> (Size, Size) {
    let (min, max) = with_states(surface, |states| {
        let mut cached = states.cached_state.get::<SurfaceCachedState>();
        let current = cached.current();
        (current.min_size, current.max_size)
    });
    let axis = |min: i32, max: i32| {
        let min = f64::from(min).clamp(0.0, MAX_SIZE);
        let max = f64::from(max).clamp(0.0, MAX_SIZE);
        (min, if max < min { 0.0 } else { max })
    };
    let (min_w, max_w) = axis(min.w, max.w);
    let (min_h, max_h) = axis(min.h, max.h);
    (Size { w: min_w, h: min_h }, Size { w: max_w, h: max_h })
}

/// A toplevel's window geometry: the one it set, within its surfaces' bounding box, or that
/// box.
fn window_geometry(surface: &WlSurface) -> Rectangle<i32, Logical> {
    let bbox = bbox_from_surface_tree(surface, (0, 0));
    let set = with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
    });
    set.and_then(|geometry| geometry.intersection(bbox))
        .unwrap_or(bbox)
}

/// The window geometry's size as the layout takes it; none when empty or beyond its limit.
fn committed_size(geometry: Rectangle<i32, Logical>) -> Option<Size> {
    let size = Size {
        w: f64::from(geometry.size.w),
        h: f64::from(geometry.size.h),
    };
    let valid = |side: f64| side > 0.0 && side <= MAX_SIZE;
    (valid(size.w) && valid(size.h)).then_some(size)
}

fn has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
}

impl State {
    /// A commit of `surface`, part of the toplevel whose surface is `root`.
    pub(crate) fn toplevel_committed(&mut self, root: &WlSurface, surface: &WlSurface) {
        let Some(toplevel) = self.toplevels.get(&root.id()) else {
            return;
        };
        let mapped = toplevel.window.is_some();
        let buffer = has_buffer(root);
        match (mapped, buffer) {
            (true, true) => self.update_window(root),
            (true, false) => self.unmap(root),
            (false, true) => self.map(root),
            (false, false) => {
                // The initial commit, or one again after the client unmapped its window.
                let opening = !toplevel.surface.is_initial_configure_sent();
                if let Some(toplevel) = self.toplevels.get_mut(&root.id()) {
                    toplevel.refused = false;
                }
                if opening {
                    self.send_opening_configure(root);
                }
            }
        }
        self.schedule_for_commit(root, surface);
    }

    /// What the layout needs to open the toplevel of `root`, from what its client asked.
    fn open_options(&self, root: &WlSurface) -> OpenOptions {
        let Some(toplevel) = self.toplevels.get(&root.id()) else {
            return OpenOptions::default();
        };
        let (min_size, max_size) = size_limits(root);
        let parent = toplevel
            .surface
            .parent()
            .and_then(|parent| self.toplevels.get(&parent.id()))
            .and_then(|parent| parent.window);
        OpenOptions {
            fullscreen: toplevel.wants_fullscreen,
            parent,
            min_size,
            max_size,
            ..OpenOptions::default()
        }
    }

    /// A configure before the window is mapped: the size it will open at, so that its client
    /// draws its first frame at it (ADR-0026), and the fullscreen state it asked.
    fn send_opening_configure(&mut self, root: &WlSurface) {
        let options = self.open_options(root);
        let size = self
            .layout
            .size_on_open(OWNER_SEAT, &options)
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "no size foretold for a new window; letting it choose");
                Size::default()
            });
        let Some(toplevel) = self.toplevels.get(&root.id()) else {
            return;
        };
        let surface = toplevel.surface.clone();
        let fullscreen = toplevel.wants_fullscreen;
        surface.with_pending_state(|state| {
            state.size = Some(configure_size(size));
            state.states = toplevel_states(fullscreen, false, false);
        });
        surface.send_configure();
        self.counters.configures += 1;
    }

    /// The toplevel's first buffer: it opens in the layout, its size known, unless it would pass
    /// a cap on windows.
    fn map(&mut self, root: &WlSurface) {
        if let Some(cap) = self.window_cap_passed(root) {
            self.refuse_over_cap(root, cap);
            return;
        }
        let geometry = window_geometry(root);
        let mut options = self.open_options(root);
        options.committed = committed_size(geometry);
        let limits = (options.min_size, options.max_size);
        let window = WindowId(self.next_window_id);
        if let Err(error) = self.layout.open_window(OWNER_SEAT, window, options) {
            // Its client may commit many times; the first refusal says it.
            if let Some(toplevel) = self.toplevels.get_mut(&root.id())
                && !std::mem::replace(&mut toplevel.refused, true)
            {
                tracing::warn!(%error, "the layout refuses a window; it is not shown");
            }
            return;
        }
        self.next_window_id += 1;
        self.windows.insert(window, root.id());
        let Some(toplevel) = self.toplevels.get_mut(&root.id()) else {
            return;
        };
        toplevel.window = Some(window);
        toplevel.refused = false;
        toplevel.limits = limits;
        toplevel.geometry = geometry;
        if toplevel.wants_full_width
            && let Err(error) = self.layout.set_full_width(window, true)
        {
            tracing::debug!(%error, window = window.0, "full width refused");
        }
        let waiting_activation = toplevel.waiting_activation.take();
        tracing::debug!(window = window.0, "window opened");
        self.mark_layout_changed();
        if let Some(validity) = waiting_activation {
            self.activate(window, validity);
        }
    }

    /// The cap a new window of the client of `root` would pass, if any. A window counts from its
    /// mapping to its closing.
    fn window_cap_passed(&self, root: &WlSurface) -> Option<&'static str> {
        if self.windows.len() >= MAX_WINDOWS {
            return Some("the layout holds its most windows");
        }
        let of_client = self
            .windows
            .values()
            .filter(|surface| surface.same_client_as(&root.id()))
            .count();
        (of_client >= MAX_WINDOWS_PER_CLIENT).then_some("its client has its most windows")
    }

    /// Refuses the toplevel of `root` a place in the layout, as the layout's own refusal does:
    /// not shown, its frame callbacks at the hidden windows' rate. Logged once per client, which
    /// may try many windows.
    fn refuse_over_cap(&mut self, root: &WlSurface, cap: &str) {
        if let Some(toplevel) = self.toplevels.get_mut(&root.id()) {
            toplevel.refused = true;
        }
        let first = root
            .client()
            .and_then(|client| {
                client
                    .get_data::<ClientState>()
                    .map(|state| !state.window_cap_logged.swap(true, Ordering::Relaxed))
            })
            .unwrap_or(true);
        if first {
            tracing::warn!(cap, "a window is not shown; said once per client");
        }
    }

    /// A commit of a mapped toplevel: its limits and window geometry go to the layout when they
    /// changed.
    fn update_window(&mut self, root: &WlSurface) {
        let geometry = window_geometry(root);
        let limits = size_limits(root);
        let Some(toplevel) = self.toplevels.get_mut(&root.id()) else {
            return;
        };
        let Some(window) = toplevel.window else {
            return;
        };
        let mut changed = false;
        if limits != toplevel.limits {
            toplevel.limits = limits;
            match self.layout.set_size_limits(window, limits.0, limits.1) {
                Ok(()) => changed = true,
                Err(error) => tracing::debug!(%error, window = window.0, "limits refused"),
            }
        }
        // A geometry that only moves in its surface changes where the surface is drawn, which
        // the commit's frame redraws, not the layout.
        let resized = geometry.size != toplevel.geometry.size;
        toplevel.geometry = geometry;
        if resized && let Some(size) = committed_size(geometry) {
            match self.layout.commit_size(window, size) {
                Ok(()) => changed = true,
                Err(error) => tracing::debug!(%error, window = window.0, "size refused"),
            }
        }
        if changed {
            self.mark_layout_changed();
        }
    }

    /// The toplevel committed without a buffer: it leaves the layout, and must start again with
    /// an initial commit, as xdg-shell says.
    fn unmap(&mut self, root: &WlSurface) {
        self.close(root);
        if let Some(toplevel) = self.toplevels.get_mut(&root.id()) {
            toplevel.wants_fullscreen = false;
            toplevel.wants_full_width = false;
        }
    }

    /// Takes the toplevel of `root` out of the layout and off its output, and dismisses its
    /// popups.
    fn close(&mut self, root: &WlSurface) {
        let Some(toplevel) = self.toplevels.get_mut(&root.id()) else {
            return;
        };
        let Some(window) = toplevel.window.take() else {
            return;
        };
        let shown_on = toplevel.shown_on.take();
        toplevel.limits = (Size::default(), Size::default());
        toplevel.geometry = Rectangle::default();
        toplevel.urgent = false;
        self.windows.remove(&window);
        if let Some(output) = shown_on {
            self.leave_output(root, output);
        }
        // After `leave_output`, which reaches the popups through their window's tree.
        self.dismiss_popups_of(root);
        if let Err(error) = self.layout.close_window(window) {
            tracing::warn!(%error, window = window.0, "closing a window failed");
        }
        tracing::debug!(window = window.0, "window closed");
        self.surfaces_gone = true;
        self.mark_layout_changed();
    }

    /// The toplevel of `root` is gone, or its surface is: its window closes and Yuna forgets it.
    pub(crate) fn toplevel_gone(&mut self, root: &WlSurface) {
        self.close(root);
        self.toplevels.remove(&root.id());
    }

    /// Tells a mapped toplevel its size and states, if they changed since its last configure, or
    /// if a request of its client is owed one. A size of none keeps the last one. `suspended`
    /// reaches only clients whose version has it.
    pub(crate) fn configure(
        &mut self,
        root: &WlSurface,
        size: Option<Size>,
        fullscreen: bool,
        activated: bool,
        suspended: bool,
    ) {
        let Some(toplevel) = self.toplevels.get_mut(&root.id()) else {
            return;
        };
        let owed = std::mem::take(&mut toplevel.configure_owed);
        let surface = &toplevel.surface;
        let suspended = suspended && surface.version() >= SUSPENDED_SINCE;
        surface.with_pending_state(|state| {
            if let Some(size) = size {
                state.size = Some(configure_size(size));
            }
            state.states = toplevel_states(fullscreen, activated, suspended);
        });
        let changed = surface.send_pending_configure().is_some();
        if !changed && owed {
            surface.send_configure();
        }
        if changed || owed {
            self.counters.configures += 1;
        }
    }

    /// Makes the decoration server side, and tells the client so when it is past its first
    /// configure: xdg-decoration asks for a configure after each request (ADR-0024).
    fn decorate_on_server_side(&mut self, toplevel: &ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_configure();
            self.counters.configures += 1;
        }
    }

    fn window_of(&self, surface: &ToplevelSurface) -> Option<WindowId> {
        self.toplevels
            .get(&surface.wl_surface().id())
            .and_then(|toplevel| toplevel.window)
    }

    /// Fullscreen asked by a client. Mapped, the layout decides: the next arrangement answers
    /// with a configure, changed or not; a refusal is logged and answered at once with the
    /// configure it has. Before it is mapped, the window will open so, and a configure says it
    /// if the first one is past.
    fn request_fullscreen(&mut self, surface: &ToplevelSurface, fullscreen: bool) {
        let root = surface.wl_surface().clone();
        let result = self
            .window_of(surface)
            .map(|window| (window, self.layout.set_fullscreen(window, fullscreen)));
        match result {
            Some((window, outcome)) => self.answer_request(&root, window, outcome),
            None => {
                if let Some(toplevel) = self.toplevels.get_mut(&root.id()) {
                    toplevel.wants_fullscreen = fullscreen;
                }
                self.answer_before_mapping(&root);
            }
        }
    }

    /// Maximised asked by a client: full width in the layout, answered as fullscreen is.
    fn request_full_width(&mut self, surface: &ToplevelSurface, full_width: bool) {
        let root = surface.wl_surface().clone();
        let result = self
            .window_of(surface)
            .map(|window| (window, self.layout.set_full_width(window, full_width)));
        match result {
            Some((window, outcome)) => self.answer_request(&root, window, outcome),
            None => {
                if let Some(toplevel) = self.toplevels.get_mut(&root.id()) {
                    toplevel.wants_full_width = full_width;
                }
                self.answer_before_mapping(&root);
            }
        }
    }

    /// Answers a mapped window's request with a configure: at the next arrangement if the layout
    /// took it, at once with the state it has if the layout refused it.
    fn answer_request(
        &mut self,
        root: &WlSurface,
        window: WindowId,
        outcome: Result<(), yuna_layout::LayoutError>,
    ) {
        match outcome {
            Ok(()) => {
                if let Some(toplevel) = self.toplevels.get_mut(&root.id()) {
                    toplevel.configure_owed = true;
                }
                self.mark_layout_changed();
            }
            Err(error) => {
                tracing::debug!(%error, window = window.0, "a request is refused");
                if let Some(toplevel) = self.toplevels.get(&root.id()) {
                    toplevel.surface.send_configure();
                    self.counters.configures += 1;
                }
            }
        }
    }

    /// Answers a request made before the window is mapped: after its first configure, with a new
    /// one that carries what it asked; before, the first configure will.
    fn answer_before_mapping(&mut self, root: &WlSurface) {
        let opened = self
            .toplevels
            .get(&root.id())
            .is_some_and(|toplevel| toplevel.surface.is_initial_configure_sent());
        if opened {
            self.send_opening_configure(root);
        }
    }
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // The configure waits for the initial commit, where the client's limits are known.
        let id = surface.wl_surface().id();
        self.toplevels.insert(id, Toplevel::new(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.toplevel_gone(surface.wl_surface());
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        self.popup_created(surface, positioner);
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        self.popup_grab_requested(surface, seat, serial);
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        self.popup_reposition_requested(surface, positioner, token);
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        self.popup_gone(surface.wl_surface());
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<wl_output::WlOutput>,
    ) {
        self.request_fullscreen(&surface, true);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.request_fullscreen(&surface, false);
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.request_full_width(&surface, true);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.request_full_width(&surface, false);
    }
}

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.decorate_on_server_side(&toplevel);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        self.decorate_on_server_side(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.decorate_on_server_side(&toplevel);
    }
}
