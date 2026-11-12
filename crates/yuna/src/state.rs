//! The compositor's state, and the protocol handlers that are not the shell's: surfaces, shared
//! memory, the seat, outputs and presentation time.

use std::collections::HashMap;
use std::sync::LazyLock;

use smithay::backend::renderer::Renderer;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::input::keyboard::{KeyboardHandle, XkbConfig};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::backend::{
    ClientData, ClientId, DisconnectReason, ObjectId,
};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::utils::ClockSource;
use smithay::utils::Monotonic;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, get_parent,
};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::pointer_constraints::PointerConstraintsHandler;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shm::{ShmHandler, ShmState};
use yuna_layout::{Layout, OutputId, SeatId, WindowId};

use crate::Error;
use crate::clock::{Clock, ClockMode};
use crate::compositor::Counters;
use crate::output::VirtualOutput;
use crate::render::Device;
use crate::shell::Toplevel;

/// The owner's seat: the only one until agents get theirs (M1.5).
pub const OWNER_SEAT: SeatId = SeatId(1);

/// A hook run when a frame's drawing has ended, before Yuna reads the time it ended at.
pub(crate) type DrawHook = Box<dyn FnMut(&mut Clock)>;

/// Everything the compositor holds, beside its event loop and display.
pub(crate) struct State {
    pub display_handle: DisplayHandle,
    pub clock: Clock,
    pub layout: Layout,
    pub renderer: GlesRenderer,
    /// `GL_MAX_RENDERBUFFER_SIZE`: the largest side an output may have.
    pub max_output_side: u32,
    pub outputs: Vec<VirtualOutput>,
    next_output_id: u64,
    /// Every `xdg_toplevel`, by its surface's id.
    pub toplevels: HashMap<ObjectId, Toplevel>,
    /// The surface of each window the layout holds.
    pub windows: HashMap<WindowId, ObjectId>,
    /// The next window's id: ids increase and are never reused.
    pub next_window_id: u64,
    /// Whether the layout changed since it was last arranged.
    pub layout_dirty: bool,
    /// Whether a window closed or a surface was destroyed since the last round: their textures
    /// wait to be freed.
    pub surfaces_gone: bool,
    pub counters: Counters,
    pub draw_hook: Option<DrawHook>,
    /// Set by the event loop's sources during a dispatch, read after it.
    pub clients_readable: bool,
    pub timer_fired: bool,
    pub stop_requested: bool,
    pub woke: bool,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    _xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<State>,
    pub keyboard: KeyboardHandle<State>,
    _output_manager_state: OutputManagerState,
    _presentation_state: PresentationState,
    _seat: Seat<State>,
}

impl State {
    pub(crate) fn new(
        display_handle: DisplayHandle,
        clock_mode: ClockMode,
        device: Device,
    ) -> Result<State, Error> {
        let dh = &display_handle;
        let (renderer, max_output_side) = crate::render::open_renderer(device)?;
        let mut layout = Layout::new(yuna_layout::Options::default())
            .map_err(|e| Error::Layout(e.to_string()))?;
        layout
            .add_seat(OWNER_SEAT)
            .map_err(|e| Error::Layout(e.to_string()))?;
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "seat0");
        // The default keymap, from XKB_CONFIG_ROOT; repeat as most desktops have it.
        let keyboard = seat
            .add_keyboard(XkbConfig::default(), 600, 25)
            .map_err(|e| Error::Keyboard(e.to_string()))?;
        Ok(State {
            display_handle: display_handle.clone(),
            clock: Clock::new(clock_mode),
            layout,
            renderer,
            max_output_side,
            outputs: Vec::new(),
            next_output_id: 1,
            toplevels: HashMap::new(),
            windows: HashMap::new(),
            next_window_id: 1,
            layout_dirty: false,
            surfaces_gone: false,
            counters: Counters::default(),
            draw_hook: None,
            clients_readable: false,
            timer_fired: false,
            stop_requested: false,
            woke: false,
            compositor_state: CompositorState::new::<State>(dh),
            xdg_shell_state: XdgShellState::new::<State>(dh),
            _xdg_decoration_state: XdgDecorationState::new::<State>(dh),
            shm_state: ShmState::new::<State>(dh, Vec::new()),
            seat_state,
            keyboard,
            _output_manager_state: OutputManagerState::new_with_xdg_output::<State>(dh),
            _presentation_state: PresentationState::new::<State>(dh, Monotonic::ID as u32),
            _seat: seat,
        })
    }

    /// The next output's id: ids increase and are never reused.
    pub(crate) fn take_output_id(&mut self) -> OutputId {
        let id = OutputId(self.next_output_id);
        self.next_output_id += 1;
        id
    }

    pub(crate) fn output_index(&self, id: OutputId) -> Option<usize> {
        self.outputs.iter().position(|o| o.id == id)
    }

    /// Frees the renderer's textures of the surfaces that went since the last round, without
    /// drawing. A frame frees them too, but nothing draws for a hidden window that closes, and
    /// its textures would stay in memory at rest.
    pub(crate) fn release_textures(&mut self) {
        if std::mem::take(&mut self.surfaces_gone)
            && let Err(error) = self.renderer.cleanup_texture_cache()
        {
            tracing::warn!(%error, "the textures of closed windows could not be freed");
        }
    }
}

/// What Yuna keeps for each client.
#[derive(Debug, Default)]
pub(crate) struct ClientState {
    pub compositor: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client: ClientId) {}

    fn disconnected(&self, _client: ClientId, reason: DisconnectReason) {
        tracing::debug!(?reason, "a client left");
    }
}

/// The state Smithay asks for a client that Yuna did not insert: none exists, since every
/// client comes through `insert_client` with a `ClientState`, but a reference must be returned.
static NO_CLIENT_STATE: LazyLock<CompositorClientState> = LazyLock::new(Default::default);

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        client
            .get_data::<ClientState>()
            .map_or(&NO_CLIENT_STATE, |state| &state.compositor)
    }

    fn commit(&mut self, surface: &WlSurface) {
        self.counters.commits += 1;
        on_commit_buffer_handler::<State>(surface);
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if self.toplevels.contains_key(&root.id()) {
            self.toplevel_committed(&root, surface);
        }
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        self.surfaces_gone = true;
        // A client may destroy a toplevel's surface before the toplevel: its window closes then.
        if self.toplevels.contains_key(&surface.id()) {
            self.toplevel_gone(surface);
        }
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }
}

/// Needed by Smithay's pointer on a `WlSurface`; no pointer constraint is offered yet.
impl PointerConstraintsHandler for State {}

impl OutputHandler for State {}

smithay::delegate_dispatch2!(State);
