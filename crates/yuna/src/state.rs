//! The compositor's state, and the protocol handlers that are not the shell's: surfaces, shared
//! memory, the seat, outputs and presentation time. `linux-dmabuf` is in `dmabuf.rs`, the
//! preferred scale in `scale.rs`; viewporter needs no handler.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Instant;

use smithay::backend::renderer::Renderer;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::PopupManager;
use smithay::input::keyboard::{KeyboardHandle, XkbConfig};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::backend::{
    ClientData, ClientId, DisconnectReason, GlobalId, ObjectId,
};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::utils::ClockSource;
use smithay::utils::Monotonic;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, get_parent, get_role,
};
use smithay::wayland::dmabuf::{DmabufState, get_dmabuf};
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::pointer_constraints::PointerConstraintsHandler;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::dialog::XdgDialogState;
use smithay::wayland::shell::xdg::{XDG_POPUP_ROLE, XdgShellState};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::xdg_activation::XdgActivationState;
use yuna_layout::{Layout, OutputId, SeatId, WindowId};

use crate::Error;
use crate::clock::{Clock, ClockMode};
use crate::compositor::Counters;
use crate::output::VirtualOutput;
use crate::popups::ActivePopupGrab;
use crate::render::Device;
use crate::selection::Selections;
use crate::shell::Toplevel;

/// The owner's seat: the only one until agents get theirs (M1.5).
pub const OWNER_SEAT: SeatId = SeatId(1);

/// A hook run when a frame's drawing has ended, before Yuna reads the time it ended at.
pub(crate) type DrawHook = Box<dyn FnMut(&mut Clock)>;

/// The clients the display ended since the end of the last round, which Yuna frees then
/// (`Compositor::flush`). Each client's [`ClientState`] holds it and records its own end there.
pub(crate) type DepartedClients = Arc<Mutex<Vec<ClientId>>>;

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
    /// The disabled globals of removed outputs, with when each was removed: destroyed later
    /// (`output.rs`).
    pub retired_globals: Vec<(Instant, GlobalId)>,
    /// Every `xdg_toplevel`, by its surface's id.
    pub toplevels: HashMap<ObjectId, Toplevel>,
    /// The surface of each window the layout holds.
    pub windows: HashMap<WindowId, ObjectId>,
    /// The next window's id: ids increase and are never reused.
    pub next_window_id: u64,
    /// Whether the layout changed since it was last arranged.
    pub layout_dirty: bool,
    /// Whether a window closed, a surface or a dmabuf's buffer was destroyed, or a dmabuf was
    /// imported for a client that left, since the last round: their textures wait to be freed.
    pub surfaces_gone: bool,
    pub counters: Counters,
    pub draw_hook: Option<DrawHook>,
    /// Set by the event loop's sources during a dispatch, read after it.
    pub clients_readable: bool,
    pub timer_fired: bool,
    pub stop_requested: bool,
    pub woke: bool,
    pub departed_clients: DepartedClients,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    _xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<State>,
    pub keyboard: KeyboardHandle<State>,
    _output_manager_state: OutputManagerState,
    _presentation_state: PresentationState,
    /// The owner's seat.
    pub seat: Seat<State>,
    pub selections: Selections,
    pub dmabuf_state: DmabufState,
    /// How many dmabuf formats clients are offered; 0 when there is no `linux-dmabuf` global.
    pub dmabuf_formats: usize,
    _viewporter_state: ViewporterState,
    _fractional_scale_state: FractionalScaleManagerState,
    /// Every window's popups, in trees kept on their toplevels' surfaces.
    pub popups: PopupManager,
    /// The popup grab that holds the keyboard, if any.
    pub popup_grab: Option<ActivePopupGrab>,
    pub xdg_activation_state: XdgActivationState,
    _xdg_dialog_state: XdgDialogState,
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
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_formats = crate::dmabuf::create_global(dh, &mut dmabuf_state, &renderer, device);
        Ok(State {
            display_handle: display_handle.clone(),
            clock: Clock::new(clock_mode),
            layout,
            renderer,
            max_output_side,
            outputs: Vec::new(),
            next_output_id: 1,
            retired_globals: Vec::new(),
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
            departed_clients: DepartedClients::default(),
            // Version 6, so that surfaces are told their preferred buffer scale (`scale.rs`).
            compositor_state: CompositorState::new_v6::<State>(dh),
            xdg_shell_state: XdgShellState::new::<State>(dh),
            _xdg_decoration_state: XdgDecorationState::new::<State>(dh),
            shm_state: ShmState::new::<State>(dh, Vec::new()),
            seat_state,
            keyboard,
            _output_manager_state: OutputManagerState::new_with_xdg_output::<State>(dh),
            _presentation_state: PresentationState::new::<State>(dh, Monotonic::ID as u32),
            seat,
            selections: Selections::new(dh),
            dmabuf_state,
            dmabuf_formats,
            _viewporter_state: ViewporterState::new::<State>(dh),
            _fractional_scale_state: FractionalScaleManagerState::new::<State>(dh),
            popups: PopupManager::default(),
            popup_grab: None,
            xdg_activation_state: XdgActivationState::new::<State>(dh),
            _xdg_dialog_state: XdgDialogState::new::<State>(dh),
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

    /// Frees the renderer's textures of the surfaces and dmabufs that went since the last round,
    /// without drawing. A frame frees them too, but nothing draws for a hidden window that closes,
    /// and its textures would stay in memory at rest.
    pub(crate) fn release_textures(&mut self) {
        if std::mem::take(&mut self.surfaces_gone)
            && let Err(error) = self.renderer.cleanup_texture_cache()
        {
            tracing::warn!(%error, "the textures of closed windows could not be freed");
        }
    }

    /// The seat a client named, when it is the owner's: the one whose keyboard Yuna holds.
    pub(crate) fn owner_seat_of(&self, seat: &WlSeat) -> Option<Seat<State>> {
        Seat::<State>::from_resource(seat)
            .filter(|seat| seat.get_keyboard().as_ref() == Some(&self.keyboard))
    }
}

/// What Yuna keeps for each client.
#[derive(Debug)]
pub(crate) struct ClientState {
    pub compositor: CompositorClientState,
    /// Whether a window of this client was refused for passing a cap on windows: said once.
    pub window_cap_logged: AtomicBool,
    /// Where the client records that the display ended it: the compositor's list.
    departed: DepartedClients,
}

impl ClientState {
    pub(crate) fn new(departed: DepartedClients) -> ClientState {
        ClientState {
            compositor: CompositorClientState::default(),
            window_cap_logged: AtomicBool::new(false),
            departed,
        }
    }
}

impl ClientData for ClientState {
    fn initialized(&self, _client: ClientId) {}

    /// The display ended the client: it left, broke the protocol, or could not take its events.
    /// Its objects are freed later, by a dispatch that reads a client or at the round's end. The
    /// display calls this under its own lock, so it only records the client: a call back into
    /// the display would wait for ever.
    fn disconnected(&self, client: ClientId, reason: DisconnectReason) {
        tracing::debug!(?reason, "a client left");
        self.departed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(client);
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
        let root = tree_root(surface);
        self.tell_surface_preferred_scale(&root, surface);
        if self.toplevels.contains_key(&root.id()) {
            self.toplevel_committed(&root, surface);
        } else if get_role(&root) == Some(XDG_POPUP_ROLE) {
            self.popup_committed(&root, surface);
        }
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        self.surfaces_gone = true;
        // A client may destroy a toplevel's surface before the toplevel: its window closes then.
        if self.toplevels.contains_key(&surface.id()) {
            self.toplevel_gone(surface);
        } else if get_role(surface) == Some(XDG_POPUP_ROLE) {
            self.popup_gone(surface);
        }
    }
}

/// The root of the surface tree `surface` belongs to: itself, or the surface its subsurfaces
/// hang from.
pub(crate) fn tree_root(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, buffer: &WlBuffer) {
        // A dmabuf was imported when its buffer was created (`dmabuf.rs`): its texture goes with
        // it, even if nothing draws again.
        if get_dmabuf(buffer).is_ok() {
            self.surfaces_gone = true;
        }
    }
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
