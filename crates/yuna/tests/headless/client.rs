//! A Wayland client for tests, in the test's own thread: it speaks to a [`Compositor`] driven by
//! the same thread, so that a test controls exactly when each side runs.
//!
//! It binds the globals a window needs, opens `xdg_toplevel`s with server-side decorations, and
//! records every event a test checks: configures, decoration modes, frame callbacks and
//! presentation feedback. Viewports, fractional scale and dmabufs, its popups, its keyboard, its
//! activation tokens and its dialogs are in the modules below.

mod activation;
mod buffers;
mod dialog;
mod keyboard;
mod popups;

pub use buffers::{ProtocolError, memory_file, sealed_memory_file};

use std::fs::File;
use std::io::Write;
use std::os::fd::AsFd;
use std::time::Duration;

use smithay::reexports::rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::backend::WaylandError;
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_registry, wl_seat, wl_shm,
    wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_client::{
    Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop,
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1;
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1;
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use yuna::Compositor;

pub use wayland_protocols::wp::presentation_time::client::wp_presentation_feedback::Kind;
pub use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::Mode;
pub use wayland_protocols::xdg::shell::client::xdg_toplevel::State;

pub use popups::{Anchor, ConstraintAdjustment, Gravity, Parent, Placement};

/// One `xdg_surface.configure`, with the `xdg_toplevel.configure` that came before it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Configure {
    pub serial: u32,
    pub width: i32,
    pub height: i32,
    /// Sorted, so that tests compare sets.
    pub states: Vec<State>,
    /// How many `preferred_scale` and `preferred_buffer_scale` events its window had when it
    /// came: whether the window knew a new scale before the configure it draws for.
    pub preferred_scales_before: usize,
    pub preferred_buffer_scales_before: usize,
}

/// What `wp_presentation_feedback` said of one content update.
#[derive(Debug, Clone, PartialEq)]
pub enum Feedback {
    Presented {
        time: Duration,
        refresh: Duration,
        seq: u64,
        flags: Kind,
    },
    Discarded,
}

/// A toplevel window and what it received.
pub struct Window {
    pub surface: wl_surface::WlSurface,
    xdg_surface: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
    decoration: zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    /// The toplevel configure waiting for its `xdg_surface.configure`.
    pending: Configure,
    pub configures: Vec<Configure>,
    /// The serial it last acknowledged.
    pub acked: Option<u32>,
    pub decoration_mode: Option<Mode>,
    /// The time each frame callback carried, in milliseconds.
    pub frames: Vec<u32>,
    pub feedback: Vec<Feedback>,
    /// Its `wp_viewport`, once made.
    pub viewport: Option<wp_viewport::WpViewport>,
    /// Its `wp_fractional_scale_v1`, once made.
    pub fractional_scale: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    /// Each `wp_fractional_scale_v1.preferred_scale`, in 120ths.
    pub preferred_scales: Vec<u32>,
    /// Each `wl_surface.preferred_buffer_scale`.
    pub preferred_buffer_scales: Vec<i32>,
}

/// What a subsurface of a window received: its `wl_surface.enter` events, and each
/// `wl_surface.preferred_buffer_scale`.
pub struct Subsurface {
    pub enters: u32,
    pub preferred_buffer_scales: Vec<i32>,
}

/// Which surface a `wl_surface`'s events are for.
#[derive(Debug, Clone, Copy)]
enum SurfaceOwner {
    Window(usize),
    Subsurface(usize),
}

/// A connected client.
pub struct Client {
    connection: Connection,
    queue: EventQueue<ClientState>,
    pub state: ClientState,
}

/// What the client's objects hold and received.
#[derive(Default)]
pub struct ClientState {
    compositor: Option<wl_compositor::WlCompositor>,
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    decorations: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    presentation: Option<wp_presentation::WpPresentation>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    fractional_scale_manager: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    /// `zwp_linux_dmabuf_v1`, bound at version 3 at most, and the version the global offers.
    dmabuf: Option<(zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, u32)>,
    /// The formats and modifiers `zwp_linux_dmabuf_v1` offered.
    pub dmabuf_formats: Vec<(u32, u64)>,
    /// What each `zwp_linux_buffer_params_v1.create` came to: `created` (true) or `failed`.
    pub dmabuf_results: Vec<bool>,
    _outputs: Vec<wl_output::WlOutput>,
    seat: Option<wl_seat::WlSeat>,
    pub windows: Vec<Window>,
    pub subsurfaces: Vec<Subsurface>,
    syncs: u64,
    popups: Vec<popups::Popup>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    keyboard_focus: keyboard::KeyboardFocus,
    activation: Option<xdg_activation_v1::XdgActivationV1>,
    /// The tokens asked, by the order they were asked, once they came.
    tokens: Vec<Option<String>>,
    wm_dialog: Option<xdg_wm_dialog_v1::XdgWmDialogV1>,
    dialogs: Vec<xdg_dialog_v1::XdgDialogV1>,
}

/// What a `wl_callback` is for.
#[derive(Debug, Clone, Copy)]
enum CallbackKind {
    Sync,
    Frame(usize),
}

impl Client {
    /// Connects to `compositor` through a socket pair, and binds its globals.
    pub fn connect(compositor: &mut Compositor) -> Client {
        let stream = compositor.connect_client().expect("a client connects");
        let connection = Connection::from_socket(stream).expect("a connection");
        let queue = connection.new_event_queue();
        connection.display().get_registry(&queue.handle(), ());
        let mut client = Client {
            connection,
            queue,
            state: ClientState::default(),
        };
        // Once for the globals, once for the events of the objects bound.
        client.roundtrip(compositor);
        client.roundtrip(compositor);
        assert!(
            client.state.compositor.is_some()
                && client.state.subcompositor.is_some()
                && client.state.shm.is_some()
                && client.state.wm_base.is_some()
                && client.state.decorations.is_some()
                && client.state.presentation.is_some(),
            "a global is missing"
        );
        client
    }

    /// Sends what the client wrote, lets the compositor handle it, and reads every event, until
    /// the compositor has answered all that came before.
    pub fn roundtrip(&mut self, compositor: &mut Compositor) {
        if let Err(error) = self.try_roundtrip(compositor) {
            panic!("the client's connection failed: {error}");
        }
    }

    /// A roundtrip for a client that may break the protocol: the error that ended its
    /// connection, a protocol error or another, rather than a panic. The compositor must still
    /// dispatch.
    pub fn try_roundtrip(&mut self, compositor: &mut Compositor) -> Result<(), DispatchError> {
        let target = self.state.syncs + 1;
        self.connection
            .display()
            .sync(&self.queue.handle(), CallbackKind::Sync);
        for _ in 0..100 {
            self.queue.flush()?;
            compositor
                .dispatch(Some(Duration::ZERO))
                .expect("the compositor dispatches");
            self.read()?;
            if self.state.syncs >= target {
                return Ok(());
            }
        }
        panic!("the compositor did not answer");
    }

    /// Reads and handles the events the compositor sent, without sending anything: for a story
    /// in which no client may speak.
    pub fn read_events(&mut self) {
        if let Err(error) = self.read() {
            panic!("the client's connection failed: {error}");
        }
    }

    /// Reads what came on the socket, without waiting, and handles every event read.
    fn read(&mut self) -> Result<(), DispatchError> {
        if let Some(guard) = self.queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
        }
        self.queue.dispatch_pending(&mut self.state)?;
        Ok(())
    }

    /// Asks the compositor for an answer it writes into the socket, without ever reading it, as
    /// a client that no longer reads does; false once the compositor has closed the connection.
    pub fn sync_without_reading(&mut self) -> bool {
        self.connection
            .display()
            .sync(&self.queue.handle(), CallbackKind::Sync);
        self.queue.flush().is_ok()
    }

    /// Sends what the client wrote, without waiting for an answer: for a client that a protocol
    /// error is about to end.
    pub fn send(&mut self) {
        self.queue.flush().expect("the client sends");
    }

    pub fn window(&self, index: usize) -> &Window {
        &self.state.windows[index]
    }

    /// Its connection, for protocols that other modules speak on event queues of their own.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Opens a toplevel with a decoration object, makes its initial commit, and waits for its
    /// first configure.
    pub fn open_window(&mut self, compositor: &mut Compositor) -> usize {
        let window = self.create_window();
        self.commit(window);
        self.roundtrip(compositor);
        window
    }

    /// Creates a toplevel with a decoration object, before its initial commit.
    pub fn create_window(&mut self) -> usize {
        let qh = self.queue.handle();
        let index = self.state.windows.len();
        let state = &self.state;
        let surface = state
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&qh, SurfaceOwner::Window(index));
        let xdg_surface = state
            .wm_base
            .as_ref()
            .expect("xdg_wm_base")
            .get_xdg_surface(&surface, &qh, index);
        self.add_toplevel(surface, xdg_surface)
    }

    /// Gives the window's surface a second `xdg_toplevel`, on its own `xdg_surface` again when
    /// `same_xdg_surface`, else on a second one: while its first lives, the client's protocol
    /// error. The second toplevel is recorded as a window of its own. None when the client itself
    /// destroyed the object it would be made from, and `wayland-client` cannot send the request.
    pub fn create_second_toplevel(
        &mut self,
        window: usize,
        same_xdg_surface: bool,
    ) -> Option<usize> {
        let qh = self.queue.handle();
        let index = self.state.windows.len();
        let first = &self.state.windows[window];
        let surface = first.surface.clone();
        let xdg_surface = if same_xdg_surface {
            first.xdg_surface.clone()
        } else {
            self.state
                .wm_base
                .as_ref()
                .expect("xdg_wm_base")
                .get_xdg_surface(&surface, &qh, index)
        };
        xdg_surface
            .is_alive()
            .then(|| self.add_toplevel(surface, xdg_surface))
    }

    /// Records a new window: an `xdg_toplevel` on `xdg_surface`, with a decoration object.
    fn add_toplevel(
        &mut self,
        surface: wl_surface::WlSurface,
        xdg_surface: xdg_surface::XdgSurface,
    ) -> usize {
        let qh = self.queue.handle();
        let index = self.state.windows.len();
        let toplevel = xdg_surface.get_toplevel(&qh, index);
        let decoration = self
            .state
            .decorations
            .as_ref()
            .expect("zxdg_decoration_manager_v1")
            .get_toplevel_decoration(&toplevel, &qh, index);
        self.state.windows.push(Window {
            surface,
            xdg_surface,
            toplevel,
            decoration,
            pending: Configure::default(),
            configures: Vec::new(),
            acked: None,
            decoration_mode: None,
            frames: Vec::new(),
            feedback: Vec::new(),
            viewport: None,
            fractional_scale: None,
            preferred_scales: Vec::new(),
            preferred_buffer_scales: Vec::new(),
        });
        index
    }

    /// Acknowledges the window's last configure.
    pub fn ack(&mut self, window: usize) {
        let serial = self.state.windows[window]
            .configures
            .last()
            .expect("a configure")
            .serial;
        self.ack_serial(window, serial);
    }

    /// Attaches a new buffer of `width` × `height` pixels of one colour, `0xAARRGGBB`, damaged
    /// whole.
    pub fn attach(&mut self, window: usize, width: i32, height: i32, argb: u32) {
        let pixels = vec![argb; pixel_count(width, height)];
        self.attach_pixels(window, width, height, &pixels);
    }

    /// Attaches a new buffer of `width` × `height` pixels, `0xAARRGGBB` row by row, damaged
    /// whole.
    pub fn attach_pixels(&mut self, window: usize, width: i32, height: i32, pixels: &[u32]) {
        let buffer = self.buffer(width, height, pixels);
        let surface = &self.state.windows[window].surface;
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
    }

    /// Attaches a buffer of `width` × `height` pixels of `content`, framed by `margin` pixels of
    /// `margin_colour` on each side, as a toolkit's shadow, and sets the window geometry to the
    /// content.
    pub fn attach_with_margin(
        &mut self,
        window: usize,
        (width, height): (i32, i32),
        (margin_x, margin_y): (i32, i32),
        content: u32,
        margin_colour: u32,
    ) {
        let (full_width, full_height) = (width + 2 * margin_x, height + 2 * margin_y);
        let mut pixels = Vec::with_capacity(pixel_count(full_width, full_height));
        for y in 0..full_height {
            for x in 0..full_width {
                let inside = (margin_x..margin_x + width).contains(&x)
                    && (margin_y..margin_y + height).contains(&y);
                pixels.push(if inside { content } else { margin_colour });
            }
        }
        let buffer = self.buffer(full_width, full_height, &pixels);
        let window = &self.state.windows[window];
        window
            .xdg_surface
            .set_window_geometry(margin_x, margin_y, width, height);
        window.surface.attach(Some(&buffer), 0, 0);
        window.surface.damage_buffer(0, 0, full_width, full_height);
    }

    /// A shared-memory buffer of `pixels`, `0xAARRGGBB`, row by row.
    fn buffer(&mut self, width: i32, height: i32, pixels: &[u32]) -> wl_buffer::WlBuffer {
        let file = pixel_file(pixels);
        self.shm_buffer(&file, (width, height), wl_shm::Format::Argb8888)
    }

    /// A shared-memory buffer of `width` × `height` pixels of `format`, four bytes each, from
    /// the start of `file`. Its pool is destroyed at once: the buffer keeps the memory mapped.
    fn shm_buffer(
        &mut self,
        file: &File,
        (width, height): (i32, i32),
        format: wl_shm::Format,
    ) -> wl_buffer::WlBuffer {
        let qh = self.queue.handle();
        let stride = width * 4;
        let shm = self.state.shm.as_ref().expect("wl_shm");
        let pool = shm.create_pool(file.as_fd(), stride * height, &qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, format, &qh, ());
        pool.destroy();
        buffer
    }

    /// Adds a desynchronised subsurface of 50 × 50 pixels at (10, 10) in `window`, and commits
    /// it; its parent is not committed.
    pub fn add_subsurface(&mut self, window: usize) -> usize {
        let qh = self.queue.handle();
        let index = self.state.subsurfaces.len();
        let surface = self
            .state
            .compositor
            .as_ref()
            .expect("wl_compositor")
            .create_surface(&qh, SurfaceOwner::Subsurface(index));
        let parent = &self.state.windows[window].surface;
        let subsurface = self
            .state
            .subcompositor
            .as_ref()
            .expect("wl_subcompositor")
            .get_subsurface(&surface, parent, &qh, ());
        subsurface.set_desync();
        subsurface.set_position(10, 10);
        let buffer = self.buffer(50, 50, &[0xFF_00_FF_00; 50 * 50]);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 50, 50);
        surface.commit();
        self.state.subsurfaces.push(Subsurface {
            enters: 0,
            preferred_buffer_scales: Vec::new(),
        });
        index
    }

    pub fn subsurface(&self, index: usize) -> &Subsurface {
        &self.state.subsurfaces[index]
    }

    /// Asks a frame callback with the next commit.
    pub fn request_frame(&mut self, window: usize) {
        let qh = self.queue.handle();
        self.state.windows[window]
            .surface
            .frame(&qh, CallbackKind::Frame(window));
    }

    /// Asks presentation feedback for the next commit.
    pub fn request_feedback(&mut self, window: usize) {
        let qh = self.queue.handle();
        let surface = &self.state.windows[window].surface;
        self.state
            .presentation
            .as_ref()
            .expect("wp_presentation")
            .feedback(surface, &qh, window);
    }

    pub fn commit(&mut self, window: usize) {
        self.state.windows[window].surface.commit();
    }

    /// Damages `width` × `height` pixels of the window's buffer, for the next commit, without a
    /// new buffer: its content changed in place.
    pub fn damage(&mut self, window: usize, width: i32, height: i32) {
        self.state.windows[window]
            .surface
            .damage_buffer(0, 0, width, height);
    }

    /// Acknowledges `serial`, whatever it is: one never sent, or acknowledged already, is the
    /// client's protocol error.
    pub fn ack_serial(&mut self, window: usize, serial: u32) {
        let window = &mut self.state.windows[window];
        window.xdg_surface.ack_configure(serial);
        window.acked = Some(serial);
    }

    /// Asks a minimum size, for the next commit.
    pub fn set_min_size(&mut self, window: usize, width: i32, height: i32) {
        self.state.windows[window]
            .toplevel
            .set_min_size(width, height);
    }

    /// Asks a maximum size, for the next commit.
    pub fn set_max_size(&mut self, window: usize, width: i32, height: i32) {
        self.state.windows[window]
            .toplevel
            .set_max_size(width, height);
    }

    /// Sets the window geometry, for the next commit, whatever its values.
    pub fn set_window_geometry(&mut self, window: usize, (x, y): (i32, i32), (w, h): (i32, i32)) {
        self.state.windows[window]
            .xdg_surface
            .set_window_geometry(x, y, w, h);
    }

    /// Asks for fullscreen, or to leave it.
    pub fn set_fullscreen(&mut self, window: usize, fullscreen: bool) {
        let toplevel = &self.state.windows[window].toplevel;
        if fullscreen {
            toplevel.set_fullscreen(None);
        } else {
            toplevel.unset_fullscreen();
        }
    }

    /// Asks to be maximised.
    pub fn set_maximized(&mut self, window: usize) {
        self.state.windows[window].toplevel.set_maximized();
    }

    /// Asks to leave the maximised state.
    pub fn unset_maximized(&mut self, window: usize) {
        self.state.windows[window].toplevel.unset_maximized();
    }

    /// Unmaps the window: a commit without a buffer.
    pub fn unmap(&mut self, window: usize) {
        let surface = &self.state.windows[window].surface;
        surface.attach(None, 0, 0);
        surface.commit();
    }

    /// Destroys the window's `wl_surface` alone, before its toplevel, as a client may.
    pub fn destroy_surface(&mut self, window: usize) {
        self.state.windows[window].surface.destroy();
    }

    /// Destroys the window's `xdg_surface` alone, before its toplevel: the client's protocol
    /// error.
    pub fn destroy_xdg_surface(&mut self, window: usize) {
        self.state.windows[window].xdg_surface.destroy();
    }

    /// Destroys the window's `xdg_toplevel` alone: before its decoration object, the client's
    /// protocol error; after it, the window is unmapped.
    pub fn destroy_toplevel(&mut self, window: usize) {
        self.state.windows[window].toplevel.destroy();
    }

    /// Destroys the window's decoration object alone.
    pub fn destroy_decoration(&mut self, window: usize) {
        self.state.windows[window].decoration.destroy();
    }

    /// Closes the window as a client does: its decoration object, then its toplevel, then its
    /// `xdg_surface`, in the order xdg-shell and xdg-decoration ask.
    pub fn close(&mut self, window: usize) {
        let window = &self.state.windows[window];
        window.decoration.destroy();
        window.toplevel.destroy();
        window.xdg_surface.destroy();
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for ClientState {
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
        match interface.as_str() {
            "wl_compositor" => {
                state.compositor = Some(registry.bind(name, version.min(6), qh, ()));
            }
            "wl_subcompositor" => state.subcompositor = Some(registry.bind(name, 1, qh, ())),
            "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
            "xdg_wm_base" => state.wm_base = Some(registry.bind(name, version.min(7), qh, ())),
            "zxdg_decoration_manager_v1" => {
                state.decorations = Some(registry.bind(name, 1, qh, ()));
            }
            "wp_presentation" => state.presentation = Some(registry.bind(name, 1, qh, ())),
            "wl_output" => state
                ._outputs
                .push(registry.bind(name, version.min(4), qh, ())),
            "wl_seat" => state.seat = Some(registry.bind(name, 1, qh, ())),
            "xdg_activation_v1" => state.activation = Some(registry.bind(name, 1, qh, ())),
            "xdg_wm_dialog_v1" => state.wm_dialog = Some(registry.bind(name, 1, qh, ())),
            "wp_viewporter" => state.viewporter = Some(registry.bind(name, 1, qh, ())),
            "wp_fractional_scale_manager_v1" => {
                state.fractional_scale_manager = Some(registry.bind(name, 1, qh, ()));
            }
            "zwp_linux_dmabuf_v1" => {
                state.dmabuf = Some((registry.bind(name, version.min(3), qh, ()), version));
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for ClientState {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, usize> for ClientState {
    fn event(
        state: &mut Self,
        _: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            let window = &mut state.windows[*index];
            let mut configure = window.pending.clone();
            configure.serial = serial;
            configure.preferred_scales_before = window.preferred_scales.len();
            configure.preferred_buffer_scales_before = window.preferred_buffer_scales.len();
            window.configures.push(configure);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, usize> for ClientState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure {
            width,
            height,
            states,
        } = event
        {
            // An array of 32-bit values in the machine's order, as Wayland sends arrays.
            let mut states: Vec<State> = states
                .as_chunks::<4>()
                .0
                .iter()
                .filter_map(|bytes| State::try_from(u32::from_ne_bytes(*bytes)).ok())
                .collect();
            states.sort_by_key(|state| *state as u32);
            state.windows[*index].pending = Configure {
                width,
                height,
                states,
                ..Configure::default()
            };
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, usize> for ClientState {
    fn event(
        state: &mut Self,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure {
            mode: WEnum::Value(mode),
        } = event
        {
            state.windows[*index].decoration_mode = Some(mode);
        }
    }
}

impl Dispatch<wl_callback::WlCallback, CallbackKind> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        kind: &CallbackKind,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let wl_callback::Event::Done { callback_data } = event else {
            return;
        };
        match kind {
            CallbackKind::Sync => state.syncs += 1,
            CallbackKind::Frame(index) => state.windows[*index].frames.push(callback_data),
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, usize> for ClientState {
    fn event(
        state: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(feedback) = Feedback::from_event(event) {
            state.windows[*index].feedback.push(feedback);
        }
    }
}

impl Feedback {
    /// What a `wp_presentation_feedback` event says; none for an event that ends nothing.
    fn from_event(event: wp_presentation_feedback::Event) -> Option<Feedback> {
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            } => Some(Feedback::Presented {
                time: Duration::new((u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo), tv_nsec),
                refresh: Duration::from_nanos(u64::from(refresh)),
                seq: (u64::from(seq_hi) << 32) | u64::from(seq_lo),
                flags: match flags {
                    WEnum::Value(flags) => flags,
                    WEnum::Unknown(raw) => panic!("unknown presentation flags {raw}"),
                },
            }),
            wp_presentation_feedback::Event::Discarded => Some(Feedback::Discarded),
            _ => None,
        }
    }
}

delegate_noop!(ClientState: ignore wl_compositor::WlCompositor);
delegate_noop!(ClientState: ignore wl_shm::WlShm);
delegate_noop!(ClientState: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(ClientState: ignore wl_subsurface::WlSubsurface);
delegate_noop!(ClientState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(ClientState: ignore wl_buffer::WlBuffer);
delegate_noop!(ClientState: ignore wl_output::WlOutput);
delegate_noop!(ClientState: ignore wl_seat::WlSeat);
delegate_noop!(ClientState: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(ClientState: ignore wp_presentation::WpPresentation);

impl Dispatch<wl_surface::WlSurface, SurfaceOwner> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        owner: &SurfaceOwner,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match (event, owner) {
            (wl_surface::Event::Enter { .. }, SurfaceOwner::Subsurface(index)) => {
                state.subsurfaces[*index].enters += 1;
            }
            (wl_surface::Event::PreferredBufferScale { factor }, SurfaceOwner::Window(index)) => {
                state.windows[*index].preferred_buffer_scales.push(factor);
            }
            (
                wl_surface::Event::PreferredBufferScale { factor },
                SurfaceOwner::Subsurface(index),
            ) => {
                state.subsurfaces[*index]
                    .preferred_buffer_scales
                    .push(factor);
            }
            _ => {}
        }
    }
}

fn pixel_count(width: i32, height: i32) -> usize {
    usize::try_from(width * height).expect("a positive size")
}

/// A file in memory that holds `pixels`, `0xAARRGGBB` row by row, as `wl_shm` takes them.
fn pixel_file(pixels: &[u32]) -> File {
    let fd = memfd_create("yuna-test-buffer", MemfdFlags::CLOEXEC).expect("a memfd");
    let mut file = File::from(fd);
    let bytes: Vec<u8> = pixels
        .iter()
        .flat_map(|pixel| pixel.to_le_bytes())
        .collect();
    file.write_all(&bytes).expect("the buffer is written");
    file
}
