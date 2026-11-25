//! The compositor's state, and the protocol handlers that are not the shell's: surfaces, shared
//! memory, the seats' handler and the cursors their clients ask, outputs and presentation time.
//! The seats themselves are in `seats.rs`, their input in `input.rs`, `linux-dmabuf` in
//! `dmabuf.rs`, the preferred scale in `scale.rs`, captures in `capture.rs` and
//! `screencopy.rs`; viewporter needs no handler.

use crate::colour::ColourState;
use crate::command::refusal;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use smithay::backend::renderer::Renderer;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::desktop::PopupManager;
use smithay::input::pointer::CursorImageStatus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::utils::ClockSource;
use smithay::utils::Monotonic;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes, TraversalAction,
    add_pre_commit_hook, get_parent, get_role, is_sync_subsurface, with_surface_tree_downward,
};
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::{DmabufState, get_dmabuf};
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::pointer_constraints::PointerConstraintsHandler;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::dialog::XdgDialogState;
use smithay::wayland::shell::xdg::{XDG_POPUP_ROLE, XdgShellState};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::xdg_activation::XdgActivationState;
use wayland_server::backend::{ClientData, ClientId, DisconnectReason, GlobalId, ObjectId};
use wayland_server::protocol::wl_buffer::WlBuffer;
use wayland_server::protocol::wl_surface::WlSurface;
use wayland_server::{Client, DisplayHandle, Resource};
use yuna_layout::Layout;
use yuna_motion::{Change, Motion, Settings};
use yuna_types::{OutputId, SeatId, WindowId};

use crate::capture::{CaptureTicket, CapturedFrame, FailedCopies, PendingCapture};
use crate::clock::{Clock, ClockMode};
use crate::compositor::Counters;
use crate::output::VirtualOutput;
use crate::render::Device;
use crate::screencopy::ScreencopyState;
use crate::seats::{SeatData, seat_id};
use crate::selection::Selections;
use crate::shell::Toplevel;
use crate::{ClientArrival, ClientProcess, Error};

/// The owner's seat, which always exists; agents' seats come after it, from 2
/// ([ADR-0078](../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)).
pub const OWNER_SEAT: SeatId = SeatId(1);

/// A hook run when a frame's drawing has ended, before Yuna reads the time it ended at.
pub(crate) type DrawHook = Box<dyn FnMut(&mut Clock)>;

/// A hook run just before the renderer imports a client's dmabuf, once Yuna's checks took it.
pub(crate) type DmabufImportHook = Box<dyn FnMut()>;

/// The clients the display ended since the end of the last round, which Yuna frees then
/// (`Compositor::flush`). Each client's [`ClientState`] holds it and records its own end there.
pub(crate) type DepartedClients = Arc<Mutex<Vec<Departed>>>;

/// A client the display ended, as its [`ClientState`] recorded it then: what Yuna frees, and
/// tells as `yuna_ipc::Event::ClientLeft` (`events.rs`).
#[derive(Debug)]
pub(crate) struct Departed {
    pub client: ClientId,
    /// Its number in the rest auditor.
    pub number: u64,
    pub seat: SeatId,
    pub process: ClientProcess,
    pub reason: DisconnectReason,
}

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
    /// The disabled globals of removed outputs and closed seats, with when each was removed on
    /// Yuna's clock: destroyed later (`output.rs`).
    pub retired_globals: Vec<(Duration, GlobalId)>,
    /// Every `xdg_toplevel`, by its surface's id.
    pub toplevels: HashMap<ObjectId, Toplevel>,
    /// The surface of each window the layout holds.
    pub windows: HashMap<WindowId, ObjectId>,
    /// The windows refused by the layout or a cap that wait for frame callbacks, by when they are
    /// due (`frames.rs`).
    pub refused_frames: crate::frames::RefusedFrames,
    /// How many windows the layout holds of each client, for its cap (`shell.rs`).
    pub window_counts: HashMap<crate::shell::WindowOwner, usize>,
    /// The next window's id: ids increase and are never reused.
    pub next_window_id: u64,
    /// Whether the layout changed since it was last arranged.
    pub layout_dirty: bool,
    /// What events tell, marked where it changes, and what they told, once someone watches
    /// (`Compositor::watch_events`); nothing is marked before (`events.rs`).
    pub events: crate::events::EventMarks,
    /// How the layout changed since it was last arranged: the changes made since, combined
    /// ([`Change::and`]), instant on the outputs where one of them must show at once
    /// (`frames.rs`).
    pub layout_change: Change,
    /// The motion of every output's arrangement: what each frame draws, and which windows are
    /// told they are visible (`frames.rs`).
    pub motion: Motion,
    /// Whether a window closed, a surface or a dmabuf's buffer was destroyed, or a dmabuf was
    /// imported for a client that left or then refused, since the last round: their textures
    /// wait to be freed.
    pub surfaces_gone: bool,
    pub counters: Counters,
    pub draw_hook: Option<DrawHook>,
    pub dmabuf_import_hook: Option<DmabufImportHook>,
    /// Set by the event loop's sources during a dispatch, read after it.
    pub clients_readable: bool,
    pub timer_fired: bool,
    pub stop_requested: bool,
    pub woke: bool,
    /// The seats whose socket paused on an `accept` error in this round, and those whose pause
    /// ended (`Compositor::pause_and_resume_sockets`).
    pub paused_sockets: Vec<SeatId>,
    pub resumed_sockets: Vec<SeatId>,
    pub departed_clients: DepartedClients,
    /// The clients served since they were last taken, once someone watches them
    /// (`Compositor::watch_client_arrivals`): none is kept otherwise. It holds at most the
    /// clients served between two takes.
    pub client_arrivals: Option<Vec<ClientArrival>>,
    /// Whether Yuna has warned that the kernel gives no pidfd of a client's peer: once for the
    /// compositor ([`ClientProcess::accepted`]).
    pub pidfd_refusal_warned: bool,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    _xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<State>,
    /// Every seat, the owner's first (`seats.rs`).
    pub seats: Vec<SeatData>,
    /// The next agent seat's id: ids increase and are never reused.
    pub next_seat_id: u64,
    _output_manager_state: OutputManagerState,
    _presentation_state: PresentationState,
    _cursor_shape_state: CursorShapeManagerState,
    pub selections: Selections,
    pub dmabuf_state: DmabufState,
    /// What the `linux-dmabuf` global offers, and how the renderer reads what it takes.
    pub dmabuf_offer: crate::dmabuf::Offer,
    _viewporter_state: ViewporterState,
    _fractional_scale_state: FractionalScaleManagerState,
    /// Every window's popups, in trees kept on their toplevels' surfaces.
    pub popups: PopupManager,
    pub xdg_activation_state: XdgActivationState,
    _xdg_dialog_state: XdgDialogState,
    /// `ext-image-copy-capture-v1` and its sessions (`screencopy.rs`).
    pub screencopy: ScreencopyState,
    /// `text-input-v3` (`text_input.rs`).
    _text_input_state: TextInputManagerState,
    /// The virtual keyboard and pointer of agents' seats (`virtual_input.rs`).
    pub(crate) virtual_input: crate::virtual_input::VirtualInputState,
    /// The captures waiting for their output's refresh (`capture.rs`).
    pub pending_captures: Vec<PendingCapture>,
    /// The outputs whose clients' captures fail to be read, for what is logged of them.
    pub failed_client_copies: FailedCopies,
    /// The agents' captures copied or failed, until the compositor's user takes them.
    pub captured: Vec<(CaptureTicket, Result<CapturedFrame, String>)>,
    /// The next agent's capture's ticket: tickets increase and are never reused.
    pub next_capture_ticket: u64,
    /// The surface whose commit request is being handled, from its pre-commit hook until the
    /// commit handler's call for it ([`Committing`]); none between commit requests.
    pub committing: Option<Committing>,
    /// Whether the commit being handled attached or removed a buffer: something new to draw.
    pub committed_content: bool,
    /// The number of the client whose commit is being handled, while it is: a frame it arms
    /// names it (`rest.rs`).
    pub committing_client: Option<u64>,
    /// What woke Yuna and what made it draw (`rest.rs`).
    pub rest: crate::rest::RestAudit,
    /// `color-management-v1`'s identities and feedback objects (`colour/`).
    pub colour: ColourState,
    /// What XWayland's sources noted in this round, for the compositor to act on after it
    /// (`xwayland/`).
    pub xwayland_notes: Vec<crate::xwayland::Note>,
    /// Every Xwayland's X11 windows, and the commands for their managers (`xwayland/windows.rs`).
    pub x11: crate::xwayland::X11Windows,
}

impl State {
    /// The state of a compositor whose owner's seat announces a `wl_seat` when
    /// `announce_owner_seat`, and whose layout's changes move as `animations` says.
    pub(crate) fn new(
        display_handle: DisplayHandle,
        clock_mode: ClockMode,
        device: Device,
        announce_owner_seat: bool,
        animations: Settings,
    ) -> Result<State, Error> {
        let dh = &display_handle;
        let (renderer, max_output_side) = crate::render::open_renderer(device)?;
        let mut layout = Layout::new(yuna_layout::Options::default())
            .map_err(|e| Error::Layout(refusal(e).to_string()))?;
        layout
            .add_seat(OWNER_SEAT)
            .map_err(|e| Error::Layout(refusal(e).to_string()))?;
        let mut seat_state = SeatState::new();
        let owner = SeatData::owner(&mut seat_state, dh, announce_owner_seat)?;
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_offer = crate::dmabuf::create_global(dh, &mut dmabuf_state, &renderer, device);
        let clock = Clock::new(clock_mode);
        let mut rest = crate::rest::RestAudit::new(clock.now());
        rest.add_seat(OWNER_SEAT);
        Ok(State {
            display_handle: display_handle.clone(),
            rest,
            clock,
            layout,
            renderer,
            max_output_side,
            outputs: Vec::new(),
            next_output_id: 1,
            retired_globals: Vec::new(),
            toplevels: HashMap::new(),
            windows: HashMap::new(),
            refused_frames: crate::frames::RefusedFrames::default(),
            window_counts: HashMap::new(),
            next_window_id: 1,
            layout_dirty: false,
            events: crate::events::EventMarks::default(),
            layout_change: Change::Animated,
            motion: Motion::new(animations),
            surfaces_gone: false,
            counters: Counters::default(),
            draw_hook: None,
            dmabuf_import_hook: None,
            clients_readable: false,
            timer_fired: false,
            stop_requested: false,
            woke: false,
            paused_sockets: Vec::new(),
            resumed_sockets: Vec::new(),
            departed_clients: DepartedClients::default(),
            client_arrivals: None,
            pidfd_refusal_warned: false,
            // Version 6, so that surfaces are told their preferred buffer scale (`scale.rs`).
            compositor_state: CompositorState::new_v6::<State>(dh),
            xdg_shell_state: XdgShellState::new::<State>(dh),
            _xdg_decoration_state: XdgDecorationState::new::<State>(dh),
            shm_state: ShmState::new::<State>(dh, Vec::new()),
            seat_state,
            seats: vec![owner],
            next_seat_id: OWNER_SEAT.0 + 1,
            _output_manager_state: OutputManagerState::new_with_xdg_output::<State>(dh),
            _presentation_state: PresentationState::new::<State>(dh, Monotonic::ID as u32),
            _cursor_shape_state: CursorShapeManagerState::new::<State>(dh),
            selections: Selections::new(dh),
            dmabuf_state,
            dmabuf_offer,
            _viewporter_state: ViewporterState::new::<State>(dh),
            _fractional_scale_state: FractionalScaleManagerState::new::<State>(dh),
            popups: PopupManager::default(),
            xdg_activation_state: XdgActivationState::new::<State>(dh),
            _xdg_dialog_state: XdgDialogState::new::<State>(dh),
            screencopy: {
                crate::wlr_screencopy::manager_global(dh);
                ScreencopyState::new(dh)
            },
            _text_input_state: crate::text_input::text_input_global(dh),
            virtual_input: crate::virtual_input::VirtualInputState::new(dh),
            pending_captures: Vec::new(),
            failed_client_copies: FailedCopies::default(),
            captured: Vec::new(),
            next_capture_ticket: 1,
            committing: None,
            committed_content: false,
            committing_client: None,
            colour: {
                crate::colour::protocol::manager_global(dh);
                ColourState::new()
            },
            xwayland_notes: Vec::new(),
            x11: {
                // Seen by each seat's Xwayland alone (`xwayland/shell.rs`).
                crate::xwayland::shell_global(dh);
                crate::xwayland::X11Windows::default()
            },
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

    /// Whether the commit handler's call for `surface` comes before the call for the surface its
    /// client is committing, in the same commit ([`Committing`]): `surface`'s state waited for
    /// that commit, having been committed while `surface` was synchronised, and that later call
    /// reads the whole tree.
    pub(crate) fn ahead_of_committed_surface(&self, surface: &WlSurface) -> bool {
        self.committing
            .as_ref()
            .is_some_and(|committing| committing.surface != *surface)
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
}

/// The surface whose `wl_surface.commit` request is being handled, and the root of its tree.
///
/// Smithay applies a commit as one transaction: the surface committed and the subsurfaces under
/// it whose state waited for that commit, each committed while it was synchronised, though it may
/// be desynchronised since. It calls the commit handler once for each of them, in the
/// transaction's order, which puts the surface committed last (`Transaction::apply`;
/// `PrivateSurfaceData::commit` inserts it after the subsurfaces it takes). The calls before it
/// each do only what is their own surface's, such as entering its window's output, and the call
/// for the surface committed reads the whole tree once: its buffers, its window's geometry, what
/// its commit arms. One commit of a window costs Yuna one walk of its tree, not one a surface. A
/// subsurface detached after its state went into its parent's transaction is applied with it all
/// the same, and enters the output of the window it left.
///
/// A surface that committed while it was synchronised keeps its first place in its next
/// transaction: made desynchronised, it may then come before subsurfaces its commit applies too.
/// The calls after its own find no surface committing, and do everything, as for a commit of
/// their own.
///
/// A synchronised subsurface's own commit applies nothing yet: it is not noted. This holds while
/// each transaction applies within the commit request that made it: Yuna adds no blocker to a
/// transaction, as a `fifo` or a `drm_syncobj` barrier would.
#[derive(Debug)]
pub(crate) struct Committing {
    /// The surface committed, noted by the pre-commit hook every surface has.
    pub surface: WlSurface,
    /// The root of its tree, found once for every call of its commit.
    pub root: WlSurface,
}

/// What Yuna keeps for each client.
#[derive(Debug)]
pub(crate) struct ClientState {
    pub compositor: CompositorClientState,
    /// The seat it belongs to: the one whose socket it connected through.
    pub seat: SeatId,
    /// Whether it is shown every seat's and output's globals, not only its own seat's: only for
    /// tests of what a client holding another seat's globals would get
    /// (`Compositor::connect_client_seeing_every_seat`).
    pub sees_every_seat: bool,
    /// The Xwayland it is, by its id, if it is its seat's Xwayland: one Wayland client for all
    /// of the seat's X11 programs
    /// ([ADR-0067](../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)).
    pub xwayland: Option<u64>,
    /// Whether a window of this client was refused for passing a cap on windows: said once.
    pub window_cap_logged: AtomicBool,
    /// Who it is, read when it connected: what its departure tells.
    pub process: ClientProcess,
    /// Its number in the rest auditor, given when it is inserted (`seats.rs`).
    pub rest_number: u64,
    /// Where the client records that the display ended it: the compositor's list.
    departed: DepartedClients,
}

impl ClientState {
    pub(crate) fn new(
        seat: SeatId,
        sees_every_seat: bool,
        process: ClientProcess,
        departed: DepartedClients,
    ) -> ClientState {
        ClientState {
            compositor: CompositorClientState::default(),
            seat,
            sees_every_seat,
            xwayland: None,
            window_cap_logged: AtomicBool::new(false),
            process,
            rest_number: 0,
            departed,
        }
    }
}

impl ClientData for ClientState {
    fn initialized(&self, _client: ClientId) {}

    /// The display ended the client: it left, broke the protocol, or could not take its events.
    /// Its objects are freed later, by a dispatch that reads a client or at the round's end. The
    /// display calls this under its own lock, so it only records the client, who it was and why,
    /// in the compositor's list: a call back into the display would wait for ever.
    fn disconnected(&self, client: ClientId, reason: DisconnectReason) {
        tracing::debug!(?reason, xwayland = self.xwayland.is_some(), "a client left");
        let departed = Departed {
            client,
            number: self.rest_number,
            seat: self.seat,
            process: self.process.clone(),
            reason,
        };
        self.departed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(departed);
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

    /// Each commit request starts with its surface's pre-commit hook, before Smithay applies the
    /// commit: it notes the surface committed, unless its commit waits for its parent's
    /// ([`Committing`]).
    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<State, _>(surface, |state, _, surface| {
            state.committing = (!is_sync_subsurface(surface)).then(|| Committing {
                surface: surface.clone(),
                root: tree_root(surface),
            });
        });
    }

    /// A surface's state applied, once for each surface of a commit, the surface committed last
    /// ([`Committing`]).
    fn commit(&mut self, surface: &WlSurface) {
        self.counters.commits += 1;
        // One lookup of its client: its seat counts the commit, and the rest auditor too.
        let client = surface.client();
        let data = client
            .as_ref()
            .and_then(|client| client.get_data::<ClientState>());
        let (seat, number) = (
            data.map(|data| data.seat),
            data.map(|data| data.rest_number),
        );
        if let Some(seat) = seat.and_then(|seat| self.seat_mut(seat)) {
            seat.commits += 1;
        }
        if let Some(number) = number {
            self.rest.client_committed(number);
        }
        self.committing_client = number;
        let ahead = self.ahead_of_committed_surface(surface);
        let root = match &self.committing {
            Some(committing) => committing.root.clone(),
            None => tree_root(surface),
        };
        if !ahead {
            self.committing = None;
        }
        // Read before the buffer handler takes them: a buffer attached or removed anywhere in the
        // tree this commit applies is something new to draw. Damage alone is not: Smithay keeps
        // it until a buffer comes, and draws nothing new for it. A synchronised subsurface's
        // buffer is read with its tree, at the surface committed, and Smithay's buffer handler
        // takes it there too: it does nothing for a synchronised subsurface.
        let mut content = false;
        if !ahead {
            with_surface_tree_downward(
                surface,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |_, states, _| {
                    content |= states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .buffer
                        .is_some();
                },
                |_, _, _| true,
            );
            on_commit_buffer_handler::<State>(surface);
        }
        self.committed_content = content;
        self.tell_surface_preferred_scale(&root, surface);
        // Its serial first: it may pair the surface with its X11 window, which then takes this
        // very commit as a window's.
        if get_role(&root) == Some(crate::xwayland::SURFACE_ROLE) {
            self.xwayland_surface_committed(&root);
        }
        if self.toplevels.contains_key(&root.id()) {
            self.toplevel_committed(&root, surface);
        } else if get_role(&root) == Some(XDG_POPUP_ROLE) {
            self.popup_committed(&root, surface);
        } else if get_role(&root) == Some(crate::xwayland::SURFACE_ROLE) {
            // An override-redirect window's: a menu or a tooltip.
            self.x11_menu_committed(&root);
        }
        self.committing_client = None;
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        self.surfaces_gone = true;
        // A client may destroy a toplevel's surface before the toplevel: its window closes then.
        if self.toplevels.contains_key(&surface.id()) {
            self.toplevel_gone(surface);
        } else if get_role(surface) == Some(XDG_POPUP_ROLE) {
            self.popup_gone(surface);
        }
        if get_role(surface) == Some(crate::xwayland::SURFACE_ROLE) {
            self.xwayland_surface_destroyed(surface);
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

    /// The cursor the client under a seat's pointer asked for, through `wl_pointer.set_cursor`
    /// or `cursor-shape`; Smithay passes on only a request whose serial is the pointer's last
    /// `enter` to that client.
    fn cursor_image(&mut self, seat: &Seat<State>, image: CursorImageStatus) {
        if let Some(data) = seat_id(seat).and_then(|id| self.seat_mut(id)) {
            data.cursor = image;
        }
    }
}

/// Needed by `cursor-shape`, which also sets a tablet tool's cursor; Yuna has no tablet.
impl smithay::input::tablet::TabletSeatHandler for State {
    type ToolFocus = WlSurface;
}

/// Needed by Smithay's pointer on a `WlSurface`; no pointer constraint is offered yet.
impl PointerConstraintsHandler for State {}

impl OutputHandler for State {}

smithay::delegate_dispatch2!(State);
