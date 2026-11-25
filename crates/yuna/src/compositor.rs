//! The headless compositor as a library: what the `yuna` program runs, and what tests drive in
//! their own process ([ADR-0025](../../../docs/adr/0025-applications-in-ci.md)).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use calloop::generic::Generic;
use calloop::ping::{Ping, make_ping};
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use rustix::io::Errno;
use rustix::process::{Pid, WaitStatus};
use smithay::utils::{Physical, Rectangle};
use wayland_server::backend::{ClientId, DisconnectReason};
use wayland_server::{BindError, Display, ListeningSocket};
use yuna_ipc::{Caller, Command, Event, ProtocolError, Refusal};
use yuna_layout::{Layout, OpenOptions};
use yuna_motion::{Change, Drawn, Settings};
use yuna_types::{OutputId, PixelRect, Rect, SeatId, WindowId, WindowShell};

use crate::Error;
use crate::canary::ParityCanary;
use crate::capture::{CaptureSize, CaptureTarget, CaptureTicket, CapturedFrame};
use crate::clock::{Clock, ClockMode};
use crate::colour::SurfaceColourDescription;
use crate::frame_record::{FrameId, InputDevice, Reading};
use crate::input::PointerDescription;
use crate::keymap::KeyboardLayout;
use crate::output::OutputMode;
use crate::process::{has_exited, lineage_under_yuna};
use crate::render::{Device, GpuPriority};
use crate::rest::{RestCounts, Source, Wake, insert_source};
use crate::selection::SelectionRead;
use crate::state::{OWNER_SEAT, State};
use crate::xwayland::{DrawnX11Menu, Selection, X11Display, Xwayland, XwaylandConfig};

/// The most bytes of events Yuna keeps for one client that does not read them: 1 MiB, as KWin,
/// Mutter, sway, Hyprland and niri give libwayland
/// ([ADR-0136](../../../docs/adr/0136-a-client-that-stops-reading-1-mib-and-its-motion-merged.md)).
/// A client whose next event passes it is ended by the display. The buffer starts at 4 KiB and
/// grows by doubling when a round's events to the client pass what it holds, and wayland-rs never
/// shrinks it: what a seat may hold is bounded by its clients, [`MAX_CLIENTS_PER_SEAT`]
/// ([ADR-0147](../../../docs/adr/0147-a-seat-connects-256-clients-at-most.md)).
///
/// [`MAX_CLIENTS_PER_SEAT`]: crate::MAX_CLIENTS_PER_SEAT
pub const CLIENT_BUFFER_BYTES: usize = 1 << 20;

/// How long a seat's socket rests after `accept` failed, out of descriptors most often, before it
/// listens again.
pub const SOCKET_PAUSE: Duration = Duration::from_millis(100);

/// How to start a compositor.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Its virtual outputs, on the owner's seat, laid out left to right; their ids are 1, 2, …
    /// in this order. None for `yuna mcp`, whose outputs are agents'.
    pub outputs: Vec<OutputMode>,
    /// The EGL device it draws on.
    pub device: Device,
    /// How its time moves.
    pub clock: ClockMode,
    /// Whether it listens on a Wayland socket in `XDG_RUNTIME_DIR`, for the owner's seat.
    /// Without one, the owner's clients come only through [`Compositor::connect_client`].
    pub listen: bool,
    /// Whether the owner's seat is announced as a `wl_seat` global. `yuna mcp` announces none,
    /// so that an agent's programs see only agents' seats
    /// ([ADR-0078](../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)).
    pub announce_owner_seat: bool,
    /// How the layout's changes move on screen. Off by default, as in both headless modes: an
    /// operation shows its result at once; `--animations` turns them on
    /// ([ADR-0055](../../../docs/adr/0055-what-moves-first-and-animations-off-headless.md)).
    pub animations: Settings,
    /// XWayland: each seat with a socket gets an X display, whose first X11 connection starts
    /// Xwayland for it ([ADR-0067](../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)).
    /// Off with none, as in tests; `yuna --headless` and `yuna mcp` turn it on.
    pub xwayland: Option<XwaylandConfig>,
}

impl Default for Config {
    /// What `yuna --headless` runs, but for XWayland, which it turns on: one default output,
    /// llvmpipe, real time, a socket, no animation.
    fn default() -> Config {
        Config {
            outputs: vec![OutputMode::default()],
            device: Device::Software,
            clock: ClockMode::Real,
            listen: true,
            announce_owner_seat: true,
            animations: Settings {
                off: true,
                ..Settings::default()
            },
            xwayland: None,
        }
    }
}

/// What the compositor did, counted since it started, for tests and measures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Frames drawn, on every output: refresh boundaries where something was damaged.
    pub frames_drawn: u64,
    /// `wl_callback.done` events sent for frame callbacks.
    pub frame_callbacks: u64,
    /// `xdg_toplevel` and `xdg_popup` configures sent, and X11 windows' configures and states
    /// set.
    pub configures: u64,
    /// `wl_surface.commit` requests received.
    pub commits: u64,
    /// Times the event loop woke: a dispatch in which a client, a new connection or the clock's
    /// timer had something; with the manual clock, each distinct time at which an advance ran
    /// deadlines, as the timer would have woken it. The timer of the rest audit's log of each
    /// second is left out: it is the auditor's own (`rest.rs`).
    pub wakeups: u64,
    /// Clients' dmabufs handed to the renderer to import, once Yuna's own checks took them.
    pub dmabuf_imports: u64,
    /// Windows looked at to arm the hidden windows' deadline again, after each arrangement, each
    /// end of a motion and each time the deadline fired: the windows in the layout, never those
    /// the layout or a cap refused, so that an arrangement's cost follows the windows the layout
    /// holds (`frames.rs`).
    pub windows_looked_at_for_hidden_frames: u64,
    /// X11 windows paired with their surface once the manager named their serial, the surface
    /// having committed it first (`xwayland/windows.rs`).
    pub x11_paired_commit_first: u64,
    /// X11 windows paired with their surface once it committed their serial, the manager having
    /// named it first.
    pub x11_paired_serial_first: u64,
}

/// A window, as [`Compositor::windows`] describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowDescription {
    /// Its id in the layout.
    pub id: WindowId,
    /// The shell behind it.
    pub shell: WindowShell,
    /// The application id its client set (`xdg_toplevel.set_app_id`), or an X11 window's
    /// `WM_CLASS` class, if any.
    pub app_id: Option<String>,
    /// Its title (`xdg_toplevel.set_title`, an X11 window's `_NET_WM_NAME` or `WM_NAME`), if any.
    pub title: Option<String>,
    /// The process id of its client, as the kernel gives it for the client's socket; for an X11
    /// window, as X11 records its client's connection (XRes), else as the window says
    /// (`_NET_WM_PID`), never Xwayland's.
    pub pid: Option<i32>,
    /// The seat whose workspaces hold it: its client's.
    pub seat: SeatId,
    /// The output it is shown on, the one that tells it it is visible: where its place is, or,
    /// while it slides out of view, where it is still drawn; none when it is hidden.
    pub output: Option<OutputId>,
    /// Its workspace's name; none for an unnamed workspace.
    pub workspace: Option<String>,
    /// Where it is drawn now, in logical coordinates: its place moved by its motion, read now,
    /// or at the time its output's last frame sampled if that is later, as hit tests read it;
    /// none when its output's frame does not draw it: hidden, slid out of view, or told visible
    /// while it has not yet slid into view.
    pub rect: Option<Rect>,
    /// Its place in the layout, as last arranged: where it is drawn once its motion ends; none
    /// when it is hidden, or no longer arranged while it slides out of view
    /// ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
    pub place: Option<Rect>,
    /// When the motion of its output ends, on the clock's scale, read as `rect` is; none at
    /// rest, from the refresh that drew the motion's end on, and when it is hidden.
    pub motion_ends: Option<Duration>,
    /// Whether its seat focuses it.
    pub focused: bool,
    /// Whether it floats.
    pub floating: bool,
    /// Whether it is fullscreen.
    pub fullscreen: bool,
    /// Whether it left a ping unanswered and has answered none since: an X11 window asked to
    /// close, its program frozen ([`X11_PING_TIMEOUT`](crate::X11_PING_TIMEOUT)). Never true of
    /// an xdg-shell window: Yuna pings none.
    pub unresponsive: bool,
}

/// Who a client is, read once, when Yuna accepts its connection: its process, and, for a client
/// of an agent's seat, the processes above it up to Yuna, which name the program it belongs to
/// (`mcp/programs/mod.rs`). The MCP server resolves that program once, from this record, as the
/// compositor tells it the client arrived ([`Compositor::take_client_arrivals`]), and keeps it
/// with the client, even once the program is forgotten.
///
/// For an agent's seat, the peer's pidfd, taken at the accept (`SO_PEERPIDFD`), checks the
/// record ([ADR-0156](../../../docs/adr/0156-a-clients-pidfd-at-accept-its-life-read-by-poll.md)):
/// the lineage is read in `/proc`, then the pidfd polled without waiting. Not ready, the peer had
/// not exited, so no other process had its pid while Yuna read: the process, the lineage's first,
/// and its group are the client's own ([`Identity::Exact`]). Ready, the peer had exited, before
/// the accept or while Yuna read: it has no lineage and no group, so no program ([`Identity::Exited`]). The pidfd is
/// closed then, so a client costs no descriptor more. What stays inexact: each parent above the
/// client may have been replaced while Yuna read; a kernel that refuses the pidfd, one before
/// Linux 6.5, leaves the walk alone ([`Identity::Inexact`]); and an X11 client's pid, which XRes
/// gives ([debt](../../../docs/debt.md#the-mcp-server)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientProcess {
    /// Its process id, as the kernel gave it for its socket, as [`Compositor::windows`] gives
    /// its windows': Yuna's own for a socket pair made in Yuna's process, a test's client;
    /// Xwayland's own for its socket pair.
    pub pid: Option<i32>,
    /// Its process and those above it up to Yuna, Yuna left out, nearest first, as `/proc` gave
    /// them then; empty when it was not under Yuna, when it had exited before the accept, and for
    /// a client not of an agent's seat, or an Xwayland, whose lineage is not read.
    pub lineage: Vec<i32>,
    /// Its process group then; none when its lineage is empty.
    pub group: Option<i32>,
    /// How far this record is its own process's.
    pub identity: Identity,
    /// When it connected, on real time.
    pub connected_at: Instant,
}

/// How far a client's record ([`ClientProcess`]) is its own process's, as Yuna checked it
/// through the peer's pidfd when it accepted the connection (ADR-0156).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Identity {
    /// The peer's pidfd, polled once its lineage was read, said that it had not exited: its pid
    /// was its own while Yuna read, so its process, the lineage's first, and its group are the
    /// client's. Each parent above it, read from `/proc`, may still have been replaced.
    Exact,
    /// The kernel gave no pidfd of the peer, `ENOPROTOOPT` before Linux 6.5, or out of
    /// descriptors, or the pidfd could not be polled: the lineage is the walk alone. Were the peer
    /// reaped before the accept and its pid taken by another process, the lineage would be that
    /// process's.
    Inexact,
    /// The client's lineage is not read, so nothing was checked: the owner's seat's clients, a
    /// seat's Xwayland, whose pid is Yuna's own child's, a peer whose pid the kernel did not give,
    /// and a peer that is Yuna itself, a test's socket pair. It names no program.
    Unread,
    /// The peer had exited by the time Yuna polled its pidfd, once its lineage was read, reaped or
    /// not: before the accept, or while Yuna read. The record holds no lineage and no group, so
    /// no program.
    Exited,
}

impl ClientProcess {
    /// A client whose lineage Yuna does not read, process `pid`, connecting now: no lineage, no
    /// group, [`Identity::Unread`]. For the owner's seat's clients, Xwayland, and tests.
    pub(crate) fn unread(pid: Option<i32>) -> ClientProcess {
        ClientProcess {
            pid,
            lineage: Vec::new(),
            group: None,
            identity: Identity::Unread,
            connected_at: Instant::now(),
        }
    }

    /// A client of an agent's seat, connecting now: process `pid`, as its socket's credentials
    /// give it, and `pidfd`, the kernel's answer to `SO_PEERPIDFD`
    /// ([`peer_pidfd`](crate::process::peer_pidfd)). Its lineage is read in `/proc`, then the
    /// pidfd polled ([`Identity`]): not ready, the record is exact; ready, or `EINVAL`, the answer
    /// of Linux 6.5 to 6.15 for a peer already reaped, the peer had exited, and the record holds
    /// no lineage. Any other answer leaves the walk alone, inexact: `ENOPROTOOPT`, a kernel before
    /// 6.5, is warned of once for the process, as `refusal_warned` remembers, and the others, out
    /// of descriptors for one, are logged. A peer whose pid the kernel did not give, or that is
    /// Yuna itself, has no lineage to read: unread. The pidfd is closed on return.
    pub(crate) fn accepted(
        pid: Option<i32>,
        pidfd: io::Result<OwnedFd>,
        refusal_warned: &mut bool,
    ) -> ClientProcess {
        let yuna = rustix::process::getpid().as_raw_nonzero().get();
        let Some(pid) = pid.filter(|&pid| pid != yuna) else {
            return ClientProcess::unread(pid);
        };
        let connected_at = Instant::now();
        // Read before the pidfd is polled: a peer that had not exited then had its pid all along.
        let (lineage, group) = lineage_under_yuna(pid);
        let identity = match pidfd {
            Ok(pidfd) => match has_exited(pidfd.as_fd()) {
                Ok(false) => Identity::Exact,
                Ok(true) => Identity::Exited,
                Err(error) => {
                    tracing::debug!(%error, "a client's pidfd could not be polled");
                    Identity::Inexact
                }
            },
            Err(error) if error.raw_os_error() == Some(Errno::INVAL.raw_os_error()) => {
                Identity::Exited
            }
            Err(error) => {
                let refused = error.raw_os_error() == Some(Errno::NOPROTOOPT.raw_os_error());
                if refused && !std::mem::replace(refusal_warned, true) {
                    tracing::warn!(
                        %error,
                        "the kernel gives no client's pidfd (SO_PEERPIDFD, Linux 6.5): clients' \
                         programs are found from /proc alone, marked inexact (ADR-0156)"
                    );
                } else {
                    tracing::debug!(%error, "a client's pidfd cannot be taken");
                }
                Identity::Inexact
            }
        };
        // What was read of an exited peer may be another process's, which took its pid.
        let (lineage, group) = match identity {
            Identity::Exited => (Vec::new(), None),
            Identity::Exact | Identity::Inexact | Identity::Unread => (lineage, group),
        };
        ClientProcess {
            pid: Some(pid),
            lineage,
            group,
            identity,
            connected_at,
        }
    }
}

/// A client the compositor serves, as [`Compositor::take_client_arrivals`] tells it: what the MCP
/// server resolves its program from, once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientArrival {
    /// The seat it belongs to.
    pub seat: SeatId,
    /// Its number: its key in [`RestCounts::clients`](crate::rest::RestCounts::clients) while it
    /// is connected, given by the `yuna_ipc::Event::ClientLeft` that tells it left too. Numbers
    /// are never reused.
    pub number: u64,
    /// Who it is, as its connection was read when Yuna accepted it.
    pub process: ClientProcess,
}

/// An output, as [`Compositor::output_descriptions`] describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputDescription {
    /// Its id.
    pub id: OutputId,
    /// The seat it belongs to.
    pub seat: SeatId,
    /// Its name, as `wl_output` gives it: `HEADLESS-<id>`.
    pub name: String,
    /// Its size, rate and scale; its VRR range's least rate
    /// ([`OutputMode::vrr_min_millihertz`]), none at a fixed rate; and the monitor whose HDR
    /// colours it announces ([`OutputMode::hdr`]), none for an SDR output.
    pub mode: OutputMode,
    /// Its place in logical coordinates.
    pub rect: Rect,
}

/// A test seam's handle (ADR-0149) that wakes a [`Compositor`] from another thread
/// ([`Compositor::waker`]). It may be cloned and sent to any thread, and outlive the compositor:
/// a wake then does nothing.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct Waker(Ping);

impl Waker {
    /// Makes the compositor's next round, or the one waiting now, return at once: wakes given
    /// before that round runs count as one.
    pub fn wake(&self) {
        self.0.ping();
    }
}

/// The compositor's input from one device, which every input it delivers names in the frame
/// record: [`Compositor`]'s own methods are the interface's, MCP's calls an agent's.
pub(crate) struct InputFrom<'a> {
    state: &'a mut State,
    device: InputDevice,
}

impl InputFrom<'_> {
    /// As [`Compositor::pointer_move`].
    pub(crate) fn pointer_move(
        &mut self,
        seat: SeatId,
        output: Option<OutputId>,
        x: f64,
        y: f64,
    ) -> Result<PointerDescription, Error> {
        self.state.pointer_move(seat, output, x, y, self.device)
    }

    /// As [`Compositor::pointer_button`].
    pub(crate) fn pointer_button(
        &mut self,
        seat: SeatId,
        button: u32,
        pressed: bool,
    ) -> Result<PointerDescription, Error> {
        self.state
            .pointer_button(seat, button, pressed, self.device)
    }

    /// As [`Compositor::pointer_scroll`].
    pub(crate) fn pointer_scroll(
        &mut self,
        seat: SeatId,
        dx: f64,
        dy: f64,
    ) -> Result<PointerDescription, Error> {
        self.state.pointer_scroll(seat, dx, dy, self.device)
    }

    /// As [`Compositor::key`].
    pub(crate) fn key(&mut self, seat: SeatId, name: &str, pressed: bool) -> Result<(), Error> {
        let key = if pressed {
            self.state.keycode_of(seat, name)?
        } else {
            self.state.held_keycode_of(seat, name)?
        };
        self.state.key(seat, key, pressed, self.device)
    }

    /// As [`Compositor::tap_keys`].
    pub(crate) fn tap_keys(&mut self, seat: SeatId, combination: &str) -> Result<(), Error> {
        self.state.tap_keys(seat, combination, self.device)
    }

    /// As [`Compositor::type_text`].
    pub(crate) fn type_text(&mut self, seat: SeatId, text: &str) -> Result<(), Error> {
        self.state.type_text(seat, text, self.device)
    }

    /// As [`Compositor::compose_text`].
    pub(crate) fn compose_text(&mut self, seat: SeatId, text: &str) -> Result<(), Error> {
        self.state.compose_text(seat, text, self.device)
    }
}

/// A test seam's name (ADR-0149) for a client connected by [`Compositor::connect_client_keyed`],
/// which [`Compositor::window_of_surface`] takes. It holds nothing of the client: once the client
/// has left, it names none.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct ClientKey(ClientId);

/// A headless compositor: its event loop, its Wayland display and its state.
///
/// Everything runs on the thread that created it: drive it with [`Compositor::dispatch`] and,
/// with the manual clock, [`Compositor::advance`], or let [`Compositor::run_until`] run it.
pub struct Compositor {
    /// First, so that it drops first: its Xwaylands end while the display still serves them.
    xwayland: Option<Xwayland>,
    event_loop: EventLoop<'static, State>,
    display: Display<State>,
    state: State,
    socket_name: Option<OsString>,
    /// The calloop timer armed at the clock's earliest deadline, in real mode, its time, and
    /// what the rest auditor names its wake.
    timer: Option<(RegistrationToken, Duration, Wake)>,
    /// The source of each seat's socket: the owner's, when it listens, and each agent's that
    /// listens.
    seat_sockets: HashMap<SeatId, RegistrationToken>,
    /// The parity script's injected cost, with the `parity-canary` feature alone (`canary.rs`).
    canary: ParityCanary,
    /// What every [`Waker`] it gave pings, once one was asked: kept, so that its source stays in
    /// the loop while no other handle lives.
    waker: Option<Ping>,
}

impl Compositor {
    /// Starts a compositor: its renderer, its globals, its outputs, and its socket if
    /// `config.listen`. Each output's first frame is armed. With XWayland on, the owner's seat
    /// gets an X display when it has a socket, or a folder for its Xauthority file.
    pub fn new(config: Config) -> Result<Compositor, Error> {
        // The socket first, so that a missing XDG_RUNTIME_DIR is refused before the renderer
        // starts.
        let socket = if config.listen {
            Some(ListeningSocket::bind_auto("wayland", 1..33).map_err(socket_error)?)
        } else {
            None
        };
        let event_loop: EventLoop<'static, State> =
            EventLoop::try_new().map_err(|e| Error::EventLoop(e.to_string()))?;
        let mut display: Display<State> =
            Display::new().map_err(|e| Error::EventLoop(e.to_string()))?;
        // Before any client, so that every one gets it: those of seats' sockets, in process, and
        // Xwaylands.
        display
            .handle()
            .set_default_max_buffer_size(CLIENT_BUFFER_BYTES);
        let mut state = State::new(
            display.handle(),
            config.clock,
            config.device,
            config.announce_owner_seat,
            config.animations,
        )?;
        for mode in config.outputs {
            state.add_output(OWNER_SEAT, mode)?;
        }
        let handle = event_loop.handle();
        // The display is read after each dispatch, outside the event loop's callbacks, so that it
        // can borrow the state.
        let display_fd = display
            .backend()
            .poll_fd()
            .try_clone_to_owned()
            .map_err(|e| Error::EventLoop(e.to_string()))?;
        insert_source(
            &handle,
            Wake::of(Source::Clients),
            Generic::new(display_fd, Interest::READ, Mode::Level),
            |_, _, state| {
                state.clients_readable = true;
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| Error::EventLoop(e.error.to_string()))?;
        let mut seat_sockets = HashMap::new();
        let socket_name = match socket {
            Some(socket) => {
                let name = socket
                    .socket_name()
                    .map(OsStr::to_os_string)
                    .ok_or_else(|| Error::Socket("the socket has no name".to_owned()))?;
                let token = insert_seat_socket(&handle, OWNER_SEAT, socket)?;
                seat_sockets.insert(OWNER_SEAT, token);
                tracing::info!(socket = ?name, "listening");
                Some(name)
            }
            None => None,
        };
        let mut xwayland = config
            .xwayland
            .as_ref()
            .and_then(|xwayland| Xwayland::new(xwayland, config.device));
        if let Some(xwayland) = &mut xwayland {
            // Beside the socket in XDG_RUNTIME_DIR, which the socket's creation required.
            let folder = match (&socket_name, config.xwayland.as_ref()) {
                (Some(name), _) => std::env::var_os("XDG_RUNTIME_DIR").map(|runtime| {
                    crate::xwayland::folder_beside(&PathBuf::from(runtime).join(name))
                }),
                (None, Some(xwayland)) => xwayland.owner_display_folder.clone(),
                (None, None) => None,
            };
            if let Some(folder) = folder {
                xwayland.add_seat(OWNER_SEAT, &folder, &handle);
            }
        }
        let canary = ParityCanary::from_environment();
        if canary.wakes() {
            // Counted as a waiting call's timer: a canary's build is never shipped, and the rest
            // tool's list of sources stays the contract it is.
            insert_source(
                &handle,
                Wake::of(Source::WaitingCall),
                Timer::from_duration(Duration::from_secs(1)),
                |_, _, _| TimeoutAction::ToDuration(Duration::from_secs(1)),
            )
            .map_err(|e| Error::EventLoop(e.error.to_string()))?;
        }
        if let Some(spin) = canary.drawing_spin() {
            // The hook runs before Yuna reads the time a drawing ended at: the frame's time
            // counts the spin. No shipped build has a canary, so no hook of the tests' is replaced.
            state.draw_hook = Some(Box::new(move |_| crate::canary::spin_for(spin)));
        }
        Ok(Compositor {
            xwayland,
            event_loop,
            display,
            state,
            socket_name,
            timer: None,
            seat_sockets,
            canary,
            waker: None,
        })
    }

    /// The name of its Wayland socket in `XDG_RUNTIME_DIR`, for `WAYLAND_DISPLAY`.
    pub fn socket_name(&self) -> Option<&OsStr> {
        self.socket_name.as_deref()
    }

    /// Connects a client of the owner's seat through a socket pair, without the socket file:
    /// its end, for `wayland-client`'s `Connection::from_socket`.
    pub fn connect_client(&mut self) -> Result<UnixStream, Error> {
        self.connect_client_to(OWNER_SEAT)
    }

    /// Connects a client of `seat` through a socket pair, as one that came through the seat's
    /// socket. Refused for a seat that does not exist, and for one that serves
    /// [`MAX_CLIENTS_PER_SEAT`](crate::MAX_CLIENTS_PER_SEAT) clients already, its Xwayland
    /// apart: the error names the seat and the bound, and nothing is connected.
    pub fn connect_client_to(&mut self, seat: SeatId) -> Result<UnixStream, Error> {
        self.connect_client_keyed(seat).map(|(_, stream)| stream)
    }

    /// Connects a client of `seat` that is shown every seat's `wl_seat` and every output's
    /// `wl_output`, as the per-client filter of ADR-0042 never shows a client. Only for tests of
    /// the layer below the filter: that a client holding another seat's globals gets none of its
    /// input, focus or serials. Refused as [`Compositor::connect_client_to`] is.
    pub fn connect_client_seeing_every_seat(&mut self, seat: SeatId) -> Result<UnixStream, Error> {
        let (ours, theirs) = UnixStream::pair().map_err(|e| Error::EventLoop(e.to_string()))?;
        self.state.insert_client(seat, ours, true)?;
        Ok(theirs)
    }

    /// Adds an agent's seat, with no output yet: its `wl_seat` global is announced at the end of
    /// the next round. Its id is never reused.
    pub fn add_seat(&mut self) -> Result<SeatId, Error> {
        self.state.add_agent_seat()
    }

    /// Every seat, the owner's first.
    pub fn seats(&self) -> Vec<SeatId> {
        self.state.seat_ids()
    }

    /// Listens on a Wayland socket at `path` for agent seat `seat`: every client that connects
    /// through it belongs to that seat. The socket file and its lock are removed with the seat.
    /// With XWayland on, the seat gets an X display too, its Xauthority file in the folder
    /// `<path>.x11`. Refused for the owner's seat, a seat that does not exist or listens already,
    /// and a path where a socket cannot be made.
    pub fn listen_for_seat(&mut self, seat: SeatId, path: PathBuf) -> Result<(), Error> {
        self.check_seat_may_listen(seat)?;
        let socket = ListeningSocket::bind_absolute(path.clone())
            .map_err(|e| Error::Socket(format!("{}: {e}", path.display())))?;
        self.serve_seat_socket(seat, socket, &path)
    }

    /// Listens for agent seat `seat` on a Wayland socket of its own in `XDG_RUNTIME_DIR`, named
    /// as the owner's would be, `wayland-1` to `wayland-32`, the first free: what `yuna
    /// --headless --agent-seat` serves. Returns its name, for `WAYLAND_DISPLAY`. Refused as
    /// [`Compositor::listen_for_seat`] is, and when `XDG_RUNTIME_DIR` is not set.
    pub fn listen_for_seat_in_runtime_dir(&mut self, seat: SeatId) -> Result<OsString, Error> {
        self.check_seat_may_listen(seat)?;
        let socket = ListeningSocket::bind_auto("wayland", 1..33).map_err(socket_error)?;
        let name = socket
            .socket_name()
            .map(OsStr::to_os_string)
            .ok_or_else(|| Error::Socket("the socket has no name".to_owned()))?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        let path = runtime.unwrap_or_default().join(&name);
        self.serve_seat_socket(seat, socket, &path)?;
        tracing::info!(socket = ?name, seat = seat.0, "listening for an agent's seat");
        Ok(name)
    }

    fn check_seat_may_listen(&self, seat: SeatId) -> Result<(), Error> {
        if seat == OWNER_SEAT || self.state.seat(seat).is_none() {
            return Err(Error::Refused(format!("no agent seat {}", seat.0)));
        }
        if self.seat_sockets.contains_key(&seat) {
            return Err(Error::Refused(format!("seat {} listens already", seat.0)));
        }
        Ok(())
    }

    /// Serves agent seat `seat` on `socket`, made at `path`, and gives it an X display beside.
    fn serve_seat_socket(
        &mut self,
        seat: SeatId,
        socket: ListeningSocket,
        path: &Path,
    ) -> Result<(), Error> {
        let token = insert_seat_socket(&self.event_loop.handle(), seat, socket)?;
        self.seat_sockets.insert(seat, token);
        if let Some(xwayland) = &mut self.xwayland {
            let folder = crate::xwayland::folder_beside(path);
            xwayland.add_seat(seat, &folder, &self.event_loop.handle());
        }
        Ok(())
    }

    /// `seat`'s X display, while it has one: its number, its Xauthority file, and its
    /// Xwayland's process id while one runs. None when XWayland is off, when Xwayland is not
    /// found, and for a seat without a socket.
    pub fn x11_display(&self, seat: SeatId) -> Option<X11Display> {
        self.xwayland.as_ref()?.display(seat)
    }

    /// Tells the compositor that the process it runs in reaped its child `pid`, which ended
    /// with `status`. A process that reaps every child of its own, as `yuna mcp` does, gives it
    /// each child it reaped that is not one of its own programs: an Xwayland's end is then known
    /// as it was, a crash counted as one, though the compositor's own wait on it finds it
    /// reaped. Any other pid is ignored. Handled at the next round, which does not wait.
    pub fn child_reaped(&mut self, pid: Pid, status: WaitStatus) {
        if let Some(xwayland) = &self.xwayland {
            xwayland.child_reaped(pid, status, &mut self.state);
        }
    }

    /// Removes agent seat `seat` ([ADR-0095](../../../docs/adr/0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)):
    /// releases the keys and buttons it holds and moves its focus off every surface; removes its
    /// socket and its X display, ending its Xwayland; ends its clients and frees them now, their
    /// windows closed; then removes its outputs and its place in the layout, and disables its
    /// `wl_seat` global. Returns the process ids of the clients it ended. Refused for the owner's
    /// seat and for a seat that does not exist.
    pub fn remove_seat(&mut self, seat: SeatId) -> Result<Vec<i32>, Error> {
        self.state.release_seat(seat)?;
        // The releases and the leaves reach the clients before their connections close.
        if let Err(error) = self.display.flush_clients() {
            tracing::debug!(%error, "flushing the clients failed");
        }
        if let Some(token) = self.seat_sockets.remove(&seat) {
            // Dropping the source drops its socket, which removes its file and its lock.
            self.event_loop.handle().remove(token);
        }
        if let Some(xwayland) = &mut self.xwayland {
            xwayland.remove_seat(seat, &self.event_loop.handle());
        }
        let pids = self.state.end_seat_clients(seat);
        self.free_departed_clients();
        self.state.finish_removing_seat(seat)?;
        self.state.rest.remove_seat(seat);
        self.state.frame_record.remove_seat(seat);
        self.flush();
        Ok(pids)
    }

    /// The commits of `seat`'s clients' surfaces since it was made; none for a seat that is not.
    pub fn seat_commits(&self, seat: SeatId) -> u64 {
        self.state.seat(seat).map_or(0, |seat| seat.commits)
    }

    /// The process ids of `seat`'s clients, as the kernel gave them for their sockets.
    pub fn client_pids(&self, seat: SeatId) -> Vec<i32> {
        self.state.client_pids(seat)
    }

    /// From now on, keeps each client the compositor serves, with its seat, its number and who it
    /// is, until [`Compositor::take_client_arrivals`] takes them: what the MCP server resolves
    /// each client's program from, once (ADR-0156).
    pub fn watch_client_arrivals(&mut self) {
        self.state.client_arrivals.get_or_insert_with(Vec::new);
    }

    /// The clients served since the last call, in the order they were served, Xwaylands
    /// included; none unless [`Compositor::watch_client_arrivals`] was called.
    pub fn take_client_arrivals(&mut self) -> Vec<ClientArrival> {
        self.state
            .client_arrivals
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// From now on, marks what events tell where it changes, and marks every window and output
    /// there is now, so that the first [`Compositor::take_events`] tells them
    /// ([ADR-0153](../../../docs/adr/0153-one-event-stream-marks-at-the-change-mcps-log-per-seat-the-bound.md)):
    /// what the MCP server does as it starts. Before, nothing is marked, kept or built. Watching
    /// again changes nothing.
    pub fn watch_events(&mut self) {
        crate::events::watch(&mut self.state);
    }

    /// What happened since the last call, on every seat, each event with its seat, in the order
    /// `yuna_ipc::Event` gives: each fact marked compared with what was last told, so that one
    /// changed many times is one event, and a window opened and closed since is told opened then
    /// closed. A window's place is told once no arrangement is pending, at a later call
    /// otherwise. Its cost follows what changed, not the windows: it reads the windows marked
    /// alone, counted by [`Compositor::windows_described_for_events`]; a build that places a
    /// window also reads each output's arrangement once, which the count leaves out. None unless
    /// [`Compositor::watch_events`] was called.
    pub fn take_events(&mut self) -> Vec<Event> {
        crate::events::take(&mut self.state)
    }

    /// How many windows [`Compositor::take_events`] read since [`Compositor::watch_events`]: one
    /// for each window a call read, so that a test checks a call's cost follows what changed.
    /// For tests.
    #[doc(hidden)]
    pub fn windows_described_for_events(&self) -> u64 {
        self.state.events.windows_described()
    }

    /// Moves agent seat `seat`'s pointer to (`x`, `y`), logical units from the top-left corner
    /// of `output`, one of the seat's, or of its first output; it enters the surface under it.
    /// Refused for the owner's seat, which takes no input from here, and for a place off the
    /// output.
    pub fn pointer_move(
        &mut self,
        seat: SeatId,
        output: Option<OutputId>,
        x: f64,
        y: f64,
    ) -> Result<PointerDescription, Error> {
        self.input_from(InputDevice::Interface)
            .pointer_move(seat, output, x, y)
    }

    /// Presses or releases `button` (Linux's code, such as [`BUTTON_LEFT`](crate::BUTTON_LEFT))
    /// of agent seat `seat`'s pointer where it is. A press focuses the window under it, which
    /// takes the seat's keyboard at the next refresh. Refused for a press of a button held, a
    /// release of one not held, and a press while the seat holds
    /// [`MAX_HELD_BUTTONS`](crate::MAX_HELD_BUTTONS) buttons.
    pub fn pointer_button(
        &mut self,
        seat: SeatId,
        button: u32,
        pressed: bool,
    ) -> Result<PointerDescription, Error> {
        self.input_from(InputDevice::Interface)
            .pointer_button(seat, button, pressed)
    }

    /// Scrolls agent seat `seat`'s wheel by `dx` and `dy` steps where its pointer is, positive
    /// right and down; at most [`MAX_SCROLL_STEPS`](crate::MAX_SCROLL_STEPS) on an axis.
    pub fn pointer_scroll(
        &mut self,
        seat: SeatId,
        dx: f64,
        dy: f64,
    ) -> Result<PointerDescription, Error> {
        self.input_from(InputDevice::Interface)
            .pointer_scroll(seat, dx, dy)
    }

    /// The window `seat`'s keyboard is on: its own, or the window of the popup it is on.
    pub fn keyboard_window(&self, seat: SeatId) -> Option<WindowId> {
        self.state.keyboard_window(seat)
    }

    /// Where `seat`'s pointer is and what is under it.
    pub fn pointer(&self, seat: SeatId) -> PointerDescription {
        self.state.describe_pointer(seat)
    }

    /// Presses or releases the key XKB names `name` ("Return", "a", "F5") on agent seat
    /// `seat`'s keyboard, to the surface it is on; no modifier is added. A press takes a key that
    /// types that keysym alone, else the key that types it with the fewest keys held, in the
    /// keyboard's group first; a release lets go of the held key that types it in any group.
    /// Refused for a name the seat's keymap has no key for, a press of a key held, a release of
    /// one not held, and a press while the seat holds [`MAX_HELD_KEYS`](crate::MAX_HELD_KEYS)
    /// keys.
    pub fn key(&mut self, seat: SeatId, name: &str, pressed: bool) -> Result<(), Error> {
        self.input_from(InputDevice::Interface)
            .key(seat, name, pressed)
    }

    /// Taps `combination` on agent seat `seat`'s keyboard: keys joined by `+`, the last pressed
    /// while the others are held ("ctrl+shift+t"); `ctrl`, `shift`, `alt`, `super` and `altgr`
    /// name the left modifiers.
    pub fn tap_keys(&mut self, seat: SeatId, combination: &str) -> Result<(), Error> {
        self.input_from(InputDevice::Interface)
            .tap_keys(seat, combination)
    }

    /// Types `text` on agent seat `seat`'s keyboard, to the surface it is on, through its
    /// keymap: each character by its key, Shift, AltGr or level 5's key held where it needs it,
    /// in its group, which the keymap's own switch locks, and locks back at the end. A character
    /// the keymap's keys do not type is committed to the text field enabled there, through
    /// `text-input-v3`, or, with none, typed through a keymap given to the client for it. At most
    /// [`MAX_TYPED`](crate::MAX_TYPED) characters.
    pub fn type_text(&mut self, seat: SeatId, text: &str) -> Result<(), Error> {
        self.input_from(InputDevice::Interface)
            .type_text(seat, text)
    }

    /// Changes agent seat `seat`'s keyboard layout to `layout`, under XKB's evdev rules and the
    /// pc105 model: every client of the seat is sent the new keymap, Xwayland included, and
    /// [`key`](Compositor::key), [`tap_keys`](Compositor::tap_keys) and
    /// [`type_text`](Compositor::type_text) go through it. The keyboard's locked modifiers and
    /// group start again from none. Refused, nothing changed, for the owner's seat, a seat that
    /// does not exist, no layout named, an empty layout between commas, a layout or variant XKB
    /// does not know, an option its rules file does not name (where Yuna finds that file; else
    /// XKB ignores it), a character XKB's names never hold, a field past
    /// [`MAX_LAYOUT_NAME`](crate::MAX_LAYOUT_NAME) bytes, more than four layouts, and while the
    /// seat holds a key.
    pub fn set_keyboard_layout(
        &mut self,
        seat: SeatId,
        layout: KeyboardLayout,
    ) -> Result<(), Error> {
        self.state.set_keyboard_layout(seat, layout)
    }

    /// Agent seat `seat`'s keyboard layout; none for the owner's seat and a seat that does not
    /// exist.
    pub fn keyboard_layout(&self, seat: SeatId) -> Option<KeyboardLayout> {
        self.state.keyboard_layout(seat)
    }

    /// The MIME types `seat`'s `selection` offers, none when it is empty, for MCP's `clipboard`.
    pub(crate) fn selection_mime_types(&self, seat: SeatId, selection: Selection) -> Vec<String> {
        self.state.selection_mime_types(seat, selection)
    }

    /// Reads `seat`'s `selection` as `mime_type`, one of its types, as a Wayland client's paste
    /// would, for MCP's `clipboard get`: an agent's text whole, or a pipe its source writes into.
    pub(crate) fn read_selection(
        &mut self,
        seat: SeatId,
        selection: Selection,
        mime_type: String,
    ) -> Result<SelectionRead, Error> {
        self.state.read_selection(seat, selection, mime_type)
    }

    /// Makes `text` `seat`'s `selection`, held by Yuna for an agent, for MCP's `clipboard set`.
    pub(crate) fn hold_agent_selection(
        &mut self,
        seat: SeatId,
        selection: Selection,
        text: Arc<[u8]>,
    ) {
        self.state.hold_agent_selection(seat, selection, text);
    }

    /// Empties `seat`'s `selection`, whoever holds it, for MCP's `clipboard clear`.
    pub(crate) fn clear_selection(&mut self, seat: SeatId, selection: Selection) {
        self.state.clear_selection_for_agent(seat, selection);
    }

    /// Whether a text field is enabled where agent seat `seat`'s keyboard is: its client asked
    /// for an input method through `text-input-v3`, and Yuna is its seat's.
    pub fn text_field_enabled(&self, seat: SeatId) -> bool {
        self.state.text_field_enabled(seat)
    }

    /// Shows `text` as being composed in the text field enabled where agent seat `seat`'s
    /// keyboard is, as an input method does, its cursor at its end, until text is committed;
    /// empty text clears it. Refused when no text field is enabled there, and past
    /// [`MAX_TEXT_BYTES`](crate::MAX_TEXT_BYTES) bytes.
    pub fn compose_text(&mut self, seat: SeatId, text: &str) -> Result<(), Error> {
        self.input_from(InputDevice::Interface)
            .compose_text(seat, text)
    }

    /// Runs one round of the event loop: waits up to `timeout` for a client, a connection, the
    /// clock's timer or XWayland (`None`: as long as it takes), handles what came, runs the
    /// deadlines due by now, and sends the events written. While X11 events wait from an earlier
    /// round, which a round handles in a slice that ends before the next deadline, it does not
    /// wait ([`MANAGER_SLICE`](crate::MANAGER_SLICE)). With the manual clock, time does not
    /// move: only the deadlines armed at its current time run, such as an idle output's refresh.
    pub fn dispatch(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        self.arm_timer()?;
        // X11 events waiting from the last round are handled now, not after a sleep.
        let timeout = if Xwayland::has_backlog(&self.state) {
            Some(Duration::ZERO)
        } else {
            timeout
        };
        self.state.rest.begin_round();
        let frames_before = self.state.counters.frames_drawn;
        self.event_loop
            .dispatch(timeout, &mut self.state)
            .map_err(|e| Error::EventLoop(e.to_string()))?;
        if std::mem::take(&mut self.state.timer_fired) {
            // A timer that fired has removed itself.
            self.timer = None;
        }
        self.pause_and_resume_sockets()?;
        if let Some(xwayland) = &mut self.xwayland {
            xwayland.handle_notes(&mut self.state, &self.event_loop.handle());
        }
        if std::mem::take(&mut self.state.clients_readable) {
            // Before the display reads them, while what they sent waits.
            self.state.rest.clients_readable();
            self.display
                .dispatch_clients(&mut self.state)
                .map_err(|e| Error::EventLoop(e.to_string()))?;
            // After the dispatch, never inside it (`colour/protocol.rs`).
            self.state.send_pending_information();
        }
        let now = self.state.clock.now();
        while let Some((_, deadline)) = self.state.clock.take_due(now) {
            self.state.fire(deadline);
        }
        if std::mem::take(&mut self.state.woke) {
            self.state.counters.wakeups += 1;
        }
        self.arm_rest_second();
        self.flush();
        if cfg!(feature = "parity-canary") {
            let frames = self.state.counters.frames_drawn - frames_before;
            let wakes = self.canary.after(frames, true);
            self.wake_for_the_canary(wakes)?;
        }
        Ok(())
    }

    /// Inserts `wakes` timers already due, which wake the next round, each counted as a waiting
    /// call's: what the `frame-wake` canary asks (`canary.rs`).
    fn wake_for_the_canary(&self, wakes: u64) -> Result<(), Error> {
        for _ in 0..wakes {
            insert_source(
                &self.event_loop.handle(),
                Wake::of(Source::WaitingCall),
                Timer::from_duration(Duration::ZERO),
                |_, _, _| TimeoutAction::Drop,
            )
            .map_err(|e| Error::EventLoop(e.error.to_string()))?;
        }
        Ok(())
    }

    /// Arms a timer of [`SOCKET_PAUSE`] for each seat socket that paused on an `accept` error,
    /// and listens again on each whose pause ended, while its seat lives.
    fn pause_and_resume_sockets(&mut self) -> Result<(), Error> {
        let handle = self.event_loop.handle();
        for seat in std::mem::take(&mut self.state.paused_sockets) {
            insert_source(
                &handle,
                Wake::of_seat(Source::SocketPause, seat),
                Timer::from_duration(SOCKET_PAUSE),
                move |_, _, state| {
                    state.resumed_sockets.push(seat);
                    TimeoutAction::Drop
                },
            )
            .map_err(|e| Error::EventLoop(e.error.to_string()))?;
        }
        for seat in std::mem::take(&mut self.state.resumed_sockets) {
            if let Some(token) = self.seat_sockets.get(&seat) {
                handle
                    .enable(token)
                    .map_err(|e| Error::EventLoop(e.to_string()))?;
            }
        }
        Ok(())
    }

    /// Runs until `stop` can be read, then returns: the program's loop. `stop` is the read end
    /// of the pipe its signal handler writes to. After each round that woke, it logs its
    /// [`Counters`] at the `debug` level, in one line whose message is `counters`; with
    /// [`Compositor::log_rest_each_second`], each second that woke it, its clients named by
    /// process and seat.
    pub fn run_until(&mut self, stop: OwnedFd) -> Result<(), Error> {
        insert_source(
            &self.event_loop.handle(),
            Wake::of(Source::StopSignal),
            Generic::new(stop, Interest::READ, Mode::Level),
            |_, _, state| {
                state.stop_requested = true;
                Ok(PostAction::Remove)
            },
        )
        .map_err(|e| Error::EventLoop(e.error.to_string()))?;
        while !self.state.stop_requested {
            let wakeups = self.state.counters.wakeups;
            self.dispatch(None)?;
            self.log_counters_after(wakeups);
            self.log_ended_rest_second(&|_| None);
        }
        tracing::info!("stopping");
        Ok(())
    }

    /// Logs the [`Counters`] at the `debug` level, in one line whose message is `counters`,
    /// if the compositor woke since it had counted `wakeups`: one line per round that woke, so
    /// that a test outside the process reads what the program did, and that nothing was done at
    /// rest.
    pub(crate) fn log_counters_after(&self, wakeups: u64) {
        if self.state.counters.wakeups != wakeups {
            let counters = self.state.counters;
            tracing::debug!(
                frames_drawn = counters.frames_drawn,
                frame_callbacks = counters.frame_callbacks,
                configures = counters.configures,
                commits = counters.commits,
                wakeups = counters.wakeups,
                "counters"
            );
        }
    }

    /// With the manual clock, moves time forward by `by`, running every deadline it passes in
    /// order, each at its own time, then sends the events written. Refused with the real clock.
    pub fn advance(&mut self, by: Duration) -> Result<(), Error> {
        if self.state.clock.mode() != ClockMode::Manual {
            return Err(Error::NotManual);
        }
        let target = self.state.clock.now().saturating_add(by);
        let frames_before = self.state.counters.frames_drawn;
        let mut last_wakeup = None;
        while let Some((at, deadline)) = self.state.clock.take_due(target) {
            self.state.clock.move_to(at);
            if last_wakeup != Some(at) {
                self.state.counters.wakeups += 1;
                let wake = self.state.wake_for(deadline);
                self.state.rest.clock_woke(wake);
                last_wakeup = Some(at);
            }
            self.state.fire(deadline);
        }
        self.state.clock.move_to(target);
        self.arm_rest_second();
        self.flush();
        if cfg!(feature = "parity-canary") {
            let frames = self.state.counters.frames_drawn - frames_before;
            let wakes = self.canary.after(frames, false);
            self.wake_for_the_canary(wakes)?;
        }
        Ok(())
    }

    /// The clock's time, on `CLOCK_MONOTONIC`'s scale.
    pub fn now(&self) -> Duration {
        self.state.clock.now()
    }

    /// How the clock's time moves.
    pub fn clock_mode(&self) -> ClockMode {
        self.state.clock.mode()
    }

    /// The real time, `CLOCK_MONOTONIC`, whatever the clock's mode: the limit of a manual
    /// clock that an agent steps ([ADR-0023](../../../docs/adr/0023-frames-presentation-and-the-clock.md)).
    pub fn real_now(&self) -> Duration {
        self.state.clock.real_now()
    }

    /// Whether the layout changed since it was last arranged: the windows' places, as
    /// [`Compositor::windows`] gives them, are those of the last arrangement until the next
    /// refresh, and no event tells a place meanwhile. A test seam, for the events' oracle.
    #[doc(hidden)]
    pub fn arrangement_pending(&self) -> bool {
        self.state.layout_dirty
    }

    /// The event loop's handle, for the MCP server's sources.
    pub(crate) fn loop_handle(&self) -> LoopHandle<'static, State> {
        self.event_loop.handle()
    }

    /// The clock's earliest armed deadline; none at rest.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.state.clock.next_deadline()
    }

    /// How many dmabuf formats, each a fourcc code and a modifier, clients are offered through
    /// `linux-dmabuf`: those its renderer imports. 0 when the renderer imports none, and Yuna
    /// offers no `linux-dmabuf` global.
    pub fn dmabuf_format_count(&self) -> usize {
        self.state.dmabuf_offer.formats
    }

    /// Its input from `device`: what each input delivered through it is named by in the frame
    /// record.
    pub(crate) fn input_from(&mut self, device: InputDevice) -> InputFrom<'_> {
        InputFrom {
            state: &mut self.state,
            device,
        }
    }

    /// `seat`'s frames numbered above `after`, of `output` alone when given, oldest first, at
    /// most `limit`, with the inputs and commits each covers that its rings still keep
    /// (`frame_record/`); none for a seat that does not exist. A frame is
    /// recorded when its refresh runs, whether it drew, found nothing changed or failed.
    pub fn frames(
        &self,
        seat: SeatId,
        output: Option<OutputId>,
        after: u64,
        limit: usize,
    ) -> Option<Reading<'_>> {
        self.state.frame_record.read(seat, output, after, limit)
    }

    /// The id of `output`'s armed refresh, a test seam: none when none is armed, or for an output
    /// that does not exist.
    #[doc(hidden)]
    pub fn armed_frame(&self, output: OutputId) -> Option<FrameId> {
        let armed = self
            .state
            .clock
            .is_armed(crate::clock::Deadline::Refresh(output));
        self.state
            .outputs
            .iter()
            .find(|candidate| candidate.id == output)
            .and_then(|output| output.armed_frame)
            .filter(|_| armed)
            .map(|frame| frame.id)
    }

    /// The number of `seat`'s newest frame: 0 before its first, and for a seat that does not
    /// exist.
    pub fn last_frame(&self, seat: SeatId) -> u64 {
        self.state.frame_record.last_frame(seat)
    }

    /// The GPU priority its renderer's context was granted, read once when the renderer was
    /// opened, and logged then.
    pub fn gpu_priority(&self) -> GpuPriority {
        self.state.gpu_priority
    }

    /// What it did so far.
    pub fn counters(&self) -> Counters {
        self.state.counters
    }

    /// What woke it and what it drew, since it started, named by source, seat, client and
    /// cause (`rest.rs`); [`RestCounts::since`] gives what happened between two of them.
    pub fn rest_counts(&self) -> RestCounts {
        let mut counts = self.state.rest.counts().clone();
        counts.woke = self.state.counters.wakeups;
        counts.at = self.state.clock.now();
        counts
    }

    /// Names and counts the clients that have something to send now, as a wake of the display
    /// does before the display reads them. For measures.
    #[doc(hidden)]
    pub fn count_readable_clients_now(&mut self) {
        self.state.rest.clients_readable();
    }

    /// How many sources had something in the last round of its loop: a round a source woke
    /// alone was that source's.
    pub(crate) fn rest_round_sources(&self) -> u32 {
        self.state.rest.round_sources()
    }

    /// From now on, logs each second in which something woke it, in one line at `info`, on real
    /// time whatever the clock: what woke it, by source, by client, and what it drew, by cause.
    /// A second's timer is armed by the first wake after a quiet second, never by itself: at
    /// rest nothing runs and nothing is logged. [`Compositor::run_until`] logs them, and the MCP
    /// server ([`Server::dispatch`](crate::mcp::Server::dispatch)) at the end of each round.
    pub fn log_rest_each_second(&mut self) {
        self.state.rest.log_each_second();
    }

    /// Logs the second that ended, if one did since the last call, each client's program named
    /// by `program_of`, from the client's number.
    pub(crate) fn log_ended_rest_second(&mut self, program_of: &dyn Fn(u64) -> Option<u64>) {
        if let Some(second) = self.state.rest.take_ended_second() {
            crate::rest::log_second(&second, self.state.rest.counts(), program_of);
        }
    }

    /// Arms the timer that ends the log's second, once one has begun. A log never stops Yuna:
    /// without a timer, the second ends at once, said.
    fn arm_rest_second(&mut self) {
        if !self.state.rest.take_second_needing_timer() {
            return;
        }
        let armed = insert_source(
            &self.event_loop.handle(),
            Wake::of_the_auditor(),
            Timer::from_duration(crate::rest::SECOND),
            |_, _, state| {
                state.rest.end_second();
                TimeoutAction::Drop
            },
        );
        if let Err(error) = armed {
            tracing::warn!(error = %error.error, "the rest audit's second cannot be timed");
            self.state.rest.end_second();
        }
    }

    /// Its outputs' ids, in the order they were created.
    pub fn outputs(&self) -> Vec<OutputId> {
        self.state.outputs.iter().map(|o| o.id).collect()
    }

    /// Adds a virtual output of `mode` to `seat`, to the right of every output, and arms its
    /// first frame. Refused for a seat that does not exist and for a side beyond the renderer's
    /// limit. Its global is announced at the end of the next round.
    pub fn add_output(&mut self, seat: SeatId, mode: OutputMode) -> Result<OutputId, Error> {
        self.state.add_output(seat, mode)
    }

    /// Removes `output`: its windows go to another output of its seat, its global is disabled at
    /// once and destroyed [`RETIRED_GLOBAL_LIFETIME`](crate::RETIRED_GLOBAL_LIFETIME) later on
    /// Yuna's clock. Refused for an output that does not exist, and for its seat's last one.
    pub fn remove_output(&mut self, output: OutputId) -> Result<(), Error> {
        self.state.remove_output(output)
    }

    /// Gives `output` the mode `mode`, as a monitor set to another one: its size, rate, scale,
    /// VRR range and HDR, applied whole, or refused with nothing changed, for an output that
    /// does not exist, a side beyond the renderer's limit, and a mode the layout refuses. It
    /// keeps its top-left corner; its seat's outputs on its right move with its right edge. Its
    /// clients are told at once, its windows placed again at its next refresh.
    pub fn configure_output(&mut self, output: OutputId, mode: OutputMode) -> Result<(), Error> {
        self.state.configure_output(output, mode)
    }

    /// Every output, in the order they were created.
    pub fn output_descriptions(&self) -> Vec<OutputDescription> {
        self.state.describe_outputs()
    }

    /// Every window the layout holds, all seats together, in the order they opened: where each
    /// is drawn now, its place as last arranged, and when its output's motion ends.
    pub fn windows(&self) -> Vec<WindowDescription> {
        self.state.describe_windows()
    }

    /// Where `output`'s last frame drew each X11 override-redirect window, a menu or a tooltip,
    /// in its physical pixels, and the window it was drawn with. For tests.
    #[doc(hidden)]
    pub fn drawn_x11_menus(&self, output: OutputId) -> Vec<DrawnX11Menu> {
        self.state.describe_x11_menus(output)
    }

    /// The colours the surfaces of `window` declared through `color-management-v1`, at their
    /// last commit: its own surface's, its subsurfaces' and its popups'; those that declared
    /// none are left out, and an unknown window has none.
    pub fn window_colours(&self, window: WindowId) -> Vec<SurfaceColourDescription> {
        self.state.window_colours(window)
    }

    /// The layout model, to read. Windows' ids start at 1 and increase in the order they map.
    pub fn layout(&self) -> &Layout {
        &self.state.layout
    }

    /// Whether `window` is urgent: an activation of it was refused, or came without a valid
    /// token, and it has not had the seat's focus since. False for a window that is not mapped.
    pub fn is_urgent(&self, window: WindowId) -> bool {
        self.state.is_urgent(window)
    }

    /// Checks that the layout and the shell agree on windows: each window Yuna gave an id is in
    /// the layout exactly while a live toplevel holds it, and the reverse; each toplevel's client
    /// is one the display still serves; and a toplevel has a window when, and only when, it has a
    /// buffer the layout did not refuse, once configured. For tests, beside
    /// [`Layout::check_invariants`]; its cost grows with every window ever opened, and with the
    /// clients.
    #[doc(hidden)]
    pub fn check_windows(&self) -> Result<(), String> {
        self.state.check_windows()
    }

    /// Checks the popups and the keyboard: every popup tracked is in the tree of the mapped
    /// window its parents lead to, once; the popup grab, while there is one, holds popups its
    /// window tracks; and the keyboard is on a live surface, or on none, or an arrangement is
    /// pending that moves it. For tests, beside [`Compositor::check_windows`]; its cost grows
    /// with the square of the popups.
    #[doc(hidden)]
    pub fn check_popups_and_keyboard(&self) -> Result<(), String> {
        self.state.check_popups_and_keyboard()
    }

    /// Whether the client of `window` said, with `xdg-dialog-v1`, that it is a modal dialog.
    /// False for a window that is not mapped.
    pub fn is_modal(&self, window: WindowId) -> bool {
        self.state.is_modal(window)
    }

    /// Runs `command` for `caller`, as a key binding of the caller's seat would
    /// ([ADR-0146](../../../docs/adr/0146-the-compositors-interface-yuna-types-and-yuna-ipcs-commands.md)):
    /// every output arranges the layout again at its next refresh, and what moved moves there on
    /// screen when animations are on. `CloseWindow` asks the window's client to close it: an
    /// `xdg_toplevel` is sent `close`; an X11 window `WM_DELETE_WINDOW` when it takes it, else
    /// its X11 client is ended (`KillClient`); the window stays until its client closes it. A
    /// refused command changes nothing and arms no refresh.
    pub fn command(&mut self, caller: Caller, command: Command) -> Result<(), Refusal> {
        crate::command::run(&mut self.state, caller, command)
    }

    /// A test seam (ADR-0146): puts `window` into `seat`'s layout with no client behind it, as
    /// if it had opened, so that a test can make the layout refuse the next window given that id.
    /// It breaks what [`Compositor::check_windows`] holds: a window of the layout has a toplevel.
    #[doc(hidden)]
    pub fn hold_window_id_in_layout(
        &mut self,
        seat: SeatId,
        window: WindowId,
    ) -> Result<(), Refusal> {
        let held = self
            .state
            .layout
            .open_window(seat, window, OpenOptions::default())
            .map_err(crate::command::refusal);
        if held.is_ok() {
            self.state.mark_layout_changed(Change::Animated);
        }
        held
    }

    /// A test seam (ADR-0146): every output arranges the layout again at its next refresh,
    /// nothing in it changed. It breaks no invariant; it only draws what a change would have.
    #[doc(hidden)]
    pub fn rearrange(&mut self) {
        self.state.mark_layout_changed(Change::Animated);
    }

    /// A test seam (ADR-0149): a handle another thread wakes the compositor with, so that a
    /// [`Compositor::dispatch`] waiting on this thread, `None` its timeout, returns, rather than
    /// the thread polling for what the other one sends it. Each round it wakes in is counted
    /// under [`Source::Waker`]. Every handle it gives shares one source, made at the first call,
    /// which stays in the loop while the compositor lives. It breaks no invariant: a wake only
    /// runs a round, as any source does; Yuna's own code holds no handle, so that nothing wakes
    /// it at rest. Fails only if the source cannot be made, out of descriptors.
    #[doc(hidden)]
    pub fn waker(&mut self) -> Result<Waker, Error> {
        if let Some(ping) = &self.waker {
            return Ok(Waker(ping.clone()));
        }
        let (ping, source) = make_ping().map_err(|e| Error::EventLoop(e.to_string()))?;
        insert_source(
            &self.event_loop.handle(),
            Wake::of(Source::Waker),
            source,
            |(), &mut (), _| {},
        )
        .map_err(|e| Error::EventLoop(e.error.to_string()))?;
        self.waker = Some(ping.clone());
        Ok(Waker(ping))
    }

    /// A test seam (ADR-0149): [`Compositor::connect_client_to`], with a key that names the
    /// client to [`Compositor::window_of_surface`]. It breaks no invariant: the client is any
    /// other of the seat's, refused as `connect_client_to` refuses.
    #[doc(hidden)]
    pub fn connect_client_keyed(&mut self, seat: SeatId) -> Result<(ClientKey, UnixStream), Error> {
        let (ours, theirs) = UnixStream::pair().map_err(|e| Error::EventLoop(e.to_string()))?;
        let client = self.state.insert_client(seat, ours, false)?;
        Ok((ClientKey(client), theirs))
    }

    /// A test seam (ADR-0149): the window whose own surface is the `wl_surface` of protocol id
    /// `surface` in `client`, as the client's side names it. None at once for a client gone, an
    /// id that names no `wl_surface` of that client, and a surface that is no window's, or not
    /// yet mapped. It reads only, and breaks no invariant.
    #[doc(hidden)]
    pub fn window_of_surface(&self, client: &ClientKey, surface: u32) -> Option<WindowId> {
        self.state.window_of_client_surface(&client.0, surface)
    }

    /// A test seam (ADR-0149): from now on, the new xdg-shell windows of `seat`'s clients open
    /// floating when `floating`, tiled otherwise, as a window rule would; those open already stay
    /// as they are. A floating window is asked 0 × 0, chooses its own size and follows its
    /// client's commits (`yuna-layout`'s `floating.rs`). It breaks what Yuna promises of every
    /// seat: that its new windows tile, unless their client's sizes or parent make them float.
    /// And it breaks what [`Compositor::command`] holds: it takes no [`Caller`], so that it
    /// changes any seat, where a caller reaches its own seat alone (ADR-0138, ADR-0146). Refused
    /// for a seat that does not exist.
    #[doc(hidden)]
    pub fn set_seat_opens_floating(&mut self, seat: SeatId, floating: bool) -> Result<(), Refusal> {
        let data = self
            .state
            .seat_mut(seat)
            .ok_or(Refusal::UnknownSeat(seat))?;
        data.opens_floating = floating;
        Ok(())
    }

    /// A test seam (ADR-0149): moves floating `window` to (`x`, `y`), logical units from the
    /// top-left corner of its output, at the size it is drawn at; enough of it stays on the
    /// output to grab it, as for any floating window. A move alone: the window is not raised or
    /// focused, its client is sent no configure, and its seat's outputs show it there at their
    /// next refresh, without motion. It moves a window where no command of Yuna's can yet. It
    /// breaks what [`Compositor::command`] holds: it takes no [`Caller`], so that it moves any
    /// seat's window, where a caller reaches its own seat's alone (ADR-0138, ADR-0146). The
    /// layout's own invariants hold: the move is the layout's `set_floating_rect`. Refused for an
    /// unknown or tiled window, and one its output does not show: on a workspace not shown, or
    /// under a fullscreen window.
    #[doc(hidden)]
    pub fn place_window(&mut self, window: WindowId, x: f64, y: f64) -> Result<(), Refusal> {
        self.state.place_floating_window(window, x, y)
    }

    /// What `output` drew in its last frame, bottom to top: each window at its rectangle and
    /// alpha then, windows outside the output included, as [`Motion::frame`] gave it at the
    /// boundary the frame aimed at. Empty for an unknown output and before its first frame.
    ///
    /// [`Motion::frame`]: yuna_motion::Motion::frame
    pub fn drawn(&self, output: OutputId) -> &[Drawn] {
        self.state
            .output_index(output)
            .map_or(&[], |index| &self.state.outputs[index].drawn)
    }

    /// Sets the hook run each time a frame's drawing ends, before Yuna reads the time it ended
    /// at. With the manual clock, a hook that passes time ([`Clock::pass`]) makes drawing
    /// take that long.
    pub fn set_draw_hook(&mut self, hook: impl FnMut(&mut Clock) + 'static) {
        self.state.draw_hook = Some(Box::new(hook));
    }

    /// Sets the hook run just before the renderer imports a client's dmabuf, once Yuna's checks
    /// before the import took it: a test makes there what a client could do while the renderer
    /// imports, such as seal the file against writing.
    pub fn set_dmabuf_import_hook(&mut self, hook: impl FnMut() + 'static) {
        self.state.dmabuf_import_hook = Some(Box::new(hook));
    }

    /// The pixels of `region` of `output`'s last frame, in physical pixels from its top-left
    /// corner, as bytes R, G, B, A, row by row from the top. Refused for an unknown output, an
    /// empty region, and a region not wholly on the output.
    pub fn read_pixels(&mut self, output: OutputId, region: PixelRect) -> Result<Vec<u8>, Error> {
        let index = self
            .state
            .output_index(output)
            .ok_or_else(|| Error::Refused(format!("no output {}", output.0)))?;
        let virtual_output = &mut self.state.outputs[index];
        let (width, height) = (virtual_output.mode.width(), virtual_output.mode.height());
        if region.width == 0 || region.height == 0 {
            return Err(Error::Refused(format!(
                "the region {region:?} holds no pixel"
            )));
        }
        let fits = |start: u32, length: u32, side: u32| {
            u64::from(start) + u64::from(length) <= u64::from(side)
        };
        if !fits(region.x, region.width, width) || !fits(region.y, region.height, height) {
            return Err(Error::Refused(format!(
                "the region {region:?} is not wholly on output {}",
                output.0
            )));
        }
        let edge = |value: u32| {
            i32::try_from(value).map_err(|_| {
                Error::Refused(format!(
                    "the region {region:?} is past what the renderer reads"
                ))
            })
        };
        let region = Rectangle::<i32, Physical>::new(
            (edge(region.x)?, edge(region.y)?).into(),
            (edge(region.width)?, edge(region.height)?).into(),
        );
        crate::render::read_pixels(&mut self.state.renderer, virtual_output, region)
    }

    /// Asks a capture of `target` on agent seat `seat`, at `size`
    /// ([ADR-0038](../../../docs/adr/0038-captures-coordinates-windows-and-wayland.md)): its
    /// output's newest frame, the one its armed refresh draws, else the one it holds, copied at
    /// once. Its pixels, or why it failed, come from [`Compositor::take_captures`]. Refused for
    /// another seat's output or window, a hidden window, and a region not wholly on its output.
    pub fn capture(
        &mut self,
        seat: SeatId,
        target: CaptureTarget,
        size: CaptureSize,
    ) -> Result<CaptureTicket, Error> {
        if seat == OWNER_SEAT || self.state.seat(seat).is_none() {
            return Err(Error::Refused(format!("no agent seat {}", seat.0)));
        }
        self.state.capture(seat, target, size)
    }

    /// The captures copied since the last call, each with its ticket, or why it failed: its
    /// output removed, or its window gone from it.
    pub fn take_captures(&mut self) -> Vec<(CaptureTicket, Result<CapturedFrame, String>)> {
        self.state.take_captures()
    }

    /// How many captures, agents' and clients', wait for their output's refresh. For tests.
    #[doc(hidden)]
    pub fn pending_captures(&self) -> usize {
        self.state.pending_captures.len()
    }

    /// Drops a capture still waiting for its frame.
    pub fn cancel_capture(&mut self, ticket: CaptureTicket) {
        self.state.cancel_capture(ticket);
    }

    /// Arms the calloop timer at the clock's earliest deadline, in real mode; with the manual
    /// clock, deadlines run only when a test advances it.
    fn arm_timer(&mut self) -> Result<(), Error> {
        if self.state.clock.mode() != ClockMode::Real {
            return Ok(());
        }
        let next = self
            .state
            .clock
            .next_due()
            .map(|(at, deadline)| (at, self.state.wake_for(deadline)));
        if next == self.timer.map(|(_, at, wake)| (at, wake)) {
            return Ok(());
        }
        let handle = self.event_loop.handle();
        if let Some((token, _, _)) = self.timer.take() {
            handle.remove(token);
        }
        if let Some((at, wake)) = next {
            // calloop's timers take an `Instant`, which reads the same CLOCK_MONOTONIC.
            let deadline = Instant::now() + at.saturating_sub(self.state.clock.now());
            let token = insert_source(
                &handle,
                wake,
                Timer::from_deadline(deadline),
                |_, _, state| {
                    state.timer_fired = true;
                    TimeoutAction::Drop
                },
            )
            .map_err(|e| Error::EventLoop(e.error.to_string()))?;
            self.timer = Some((token, at, wake));
        }
        Ok(())
    }

    /// Ends a round: frees the clients the display ended, goes on with Yuna's writes of agents'
    /// selections (`selection.rs`), puts back on what is under it each pointer whose drag's
    /// grab ended (`xwayland/drags.rs`), then frees what closed windows, destroyed popups and
    /// surfaces left behind, then sends the events written to every client. A client that
    /// cannot take them is ended by the display, and freed at the next round's end; the others
    /// go on.
    pub(crate) fn flush(&mut self) {
        self.free_departed_clients();
        self.state.run_clipboard_writes(&self.event_loop.handle());
        // Before the managers' commands: a pointer put back tells a drag what it is over.
        self.state.refocus_after_drag_grabs();
        if let Some(xwayland) = &mut self.xwayland {
            xwayland.send_commands(&mut self.state);
        }
        // Smithay's popup trees and grabs forget their destroyed popups.
        self.state.popups.cleanup();
        // Each output forgets the surfaces destroyed since, which it keeps to send `enter` once.
        for output in &self.state.outputs {
            output.output.cleanup();
        }
        self.state.release_textures();
        self.state.destroy_retired_globals();
        self.state.screencopy.cleanup();
        if let Err(error) = self.display.flush_clients() {
            tracing::debug!(%error, "flushing the clients failed");
        }
    }

    /// Frees the clients the display ended since the last round's end: their objects are
    /// destroyed as when a client leaves, so that their windows close now. wayland-backend frees
    /// an ended client only after a dispatch that read some client; one it ended while Yuna drew
    /// its buffer (SIGBUS) or wrote it an event its full buffer could not take would keep its
    /// windows, shown and focused, until another client spoke: at rest, for ever. Freeing a client
    /// may end another, whose buffer the departure's events fill: the loop frees it too, and ends,
    /// since each client is ended once.
    fn free_departed_clients(&mut self) {
        loop {
            let departed = std::mem::take(
                &mut *self
                    .state
                    .departed_clients
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner),
            );
            if departed.is_empty() {
                return;
            }
            for departed in departed {
                self.state.forget_client(&departed.client);
                self.state.rest.client_left(departed.number);
                // An ended client is not read: the call fails, and frees it on its way out. A
                // client a dispatch freed already is not found, and nothing happens.
                let _ = self
                    .display
                    .backend()
                    .dispatch_single_client(&mut self.state, departed.client);
                let error = match departed.reason {
                    DisconnectReason::ProtocolError(error) => Some(ProtocolError::new(
                        error.object_interface,
                        error.object_id,
                        error.code,
                        error.message,
                    )),
                    DisconnectReason::ConnectionClosed => None,
                };
                self.state.events.client_left(
                    departed.seat,
                    departed.number,
                    departed.process.pid,
                    error,
                );
            }
        }
    }
}

/// Serves `seat` on `socket`: each client that connects through it belongs to that seat. Past
/// [`MAX_CLIENTS_PER_SEAT`](crate::MAX_CLIENTS_PER_SEAT), a connection is accepted and closed at
/// once, and the socket listens on (`State::accept_connection`). An `accept` that fails, out of
/// descriptors most often, pauses the socket for [`SOCKET_PAUSE`] rather than stop Yuna, the
/// owner's socket as an agent's: one client holding many descriptors must not end the others
/// ([ADR-0080](../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)). The
/// connection waits meanwhile in the socket's queue.
fn insert_seat_socket(
    handle: &LoopHandle<'static, State>,
    seat: SeatId,
    socket: ListeningSocket,
) -> Result<RegistrationToken, Error> {
    insert_source(
        handle,
        Wake::of_seat(Source::SeatSocket, seat),
        Generic::new(socket, Interest::READ, Mode::Level),
        move |_, socket, state| {
            loop {
                match socket.accept() {
                    Ok(Some(stream)) => state.accept_connection(seat, stream),
                    Ok(None) => return Ok(PostAction::Continue),
                    // Rests a moment rather than spin on a connection it cannot take.
                    Err(error) => {
                        tracing::warn!(%error, seat = seat.0, "a seat's socket pauses");
                        state.paused_sockets.push(seat);
                        return Ok(PostAction::Disable);
                    }
                }
            }
        },
    )
    .map_err(|e| Error::EventLoop(e.error.to_string()))
}

/// Why a socket in `XDG_RUNTIME_DIR` could not be made, for its user.
fn socket_error(error: BindError) -> Error {
    match error {
        BindError::RuntimeDirNotSet => Error::Socket(
            "XDG_RUNTIME_DIR is not set, or not an absolute path; Yuna creates its Wayland \
             socket there"
                .to_owned(),
        ),
        error => Error::Socket(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command};

    use rustix::event::{PollFd, PollFlags, Timespec};
    use rustix::process::{PidfdFlags, pidfd_open};

    use super::*;

    /// A `sleep` this test started, under it: killed and reaped when it drops, an assertion that
    /// failed included.
    struct Sleeper(Child);

    impl Sleeper {
        fn start() -> Sleeper {
            Sleeper(
                Command::new("sleep")
                    .arg("30")
                    .spawn()
                    .expect("sleep starts"),
            )
        }

        fn pid(&self) -> i32 {
            Pid::from_child(&self.0).as_raw_nonzero().get()
        }

        /// A new pidfd of it.
        fn pidfd(&self) -> OwnedFd {
            pidfd_open(Pid::from_child(&self.0), PidfdFlags::empty()).expect("its pidfd")
        }
    }

    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// The kernel's refusal `errno`, as `getsockopt` gives it.
    fn refused(errno: Errno) -> io::Result<OwnedFd> {
        Err(io::Error::from_raw_os_error(errno.raw_os_error()))
    }

    fn test_group() -> i32 {
        rustix::process::getpgrp().as_raw_nonzero().get()
    }

    /// A kernel that refuses `SO_PEERPIDFD`, as one before Linux 6.5 does, leaves the walk alone:
    /// a child of the test is found under it, in the test's group, marked inexact, and the
    /// refusal is remembered as warned of.
    #[test]
    fn a_record_the_kernel_gives_no_pidfd_for_is_the_walk_alone_marked_inexact() {
        let sleeper = Sleeper::start();
        let mut warned = false;
        let record =
            ClientProcess::accepted(Some(sleeper.pid()), refused(Errno::NOPROTOOPT), &mut warned);
        assert_eq!(record.identity, Identity::Inexact);
        assert_eq!(record.pid, Some(sleeper.pid()));
        assert_eq!(record.lineage, [sleeper.pid()]);
        assert_eq!(record.group, Some(test_group()));
        assert!(warned, "the first refusal is warned of");
    }

    /// `EINVAL`, the answer of Linux 6.5 to 6.15 for a peer already reaped, says the peer had
    /// exited: the record holds no lineage, even where the walk found one, and no refusal is
    /// warned of.
    #[test]
    fn a_record_given_einval_is_exited_with_no_lineage() {
        let sleeper = Sleeper::start();
        let mut warned = false;
        let record =
            ClientProcess::accepted(Some(sleeper.pid()), refused(Errno::INVAL), &mut warned);
        assert_eq!(record.identity, Identity::Exited);
        assert_eq!(record.lineage, Vec::<i32>::new());
        assert_eq!(record.group, None);
        assert!(!warned, "EINVAL is no refusal");
    }

    /// Another failure, out of descriptors for one, leaves the walk alone, inexact, but is no
    /// kernel's refusal: the warning stays for one.
    #[test]
    fn a_record_whose_pidfd_failed_otherwise_is_inexact_and_not_warned_of() {
        let sleeper = Sleeper::start();
        let mut warned = false;
        let record =
            ClientProcess::accepted(Some(sleeper.pid()), refused(Errno::MFILE), &mut warned);
        assert_eq!(record.identity, Identity::Inexact);
        assert_eq!(record.lineage, [sleeper.pid()]);
        assert!(!warned, "running out of descriptors is no kernel's refusal");
    }

    /// A peer that is Yuna itself, a test's socket pair, has nothing to check: unread, whatever
    /// its pidfd says.
    #[test]
    fn a_record_of_yuna_itself_is_unread() {
        let yuna = rustix::process::getpid();
        let pidfd = pidfd_open(yuna, PidfdFlags::empty()).expect("Yuna's own pidfd");
        let mut warned = false;
        let record =
            ClientProcess::accepted(Some(yuna.as_raw_nonzero().get()), Ok(pidfd), &mut warned);
        assert_eq!(record.identity, Identity::Unread);
        assert_eq!(record.lineage, Vec::<i32>::new());
        assert_eq!(record.group, None);
    }

    /// A live peer's pidfd makes its record exact, its lineage the walk's; once the peer has
    /// exited, its pidfd ready, its record is exited, though it is not yet reaped and `/proc`
    /// still shows it.
    #[test]
    fn a_live_peers_pidfd_makes_its_record_exact_and_an_exited_ones_exited() {
        let mut sleeper = Sleeper::start();
        let mut warned = false;
        let live = ClientProcess::accepted(Some(sleeper.pid()), Ok(sleeper.pidfd()), &mut warned);
        assert_eq!(live.identity, Identity::Exact);
        assert_eq!(live.lineage, [sleeper.pid()]);
        assert_eq!(live.group, Some(test_group()));

        let pidfd = sleeper.pidfd();
        sleeper.0.kill().expect("sleep is killed");
        let mut exited = [PollFd::from_borrowed_fd(pidfd.as_fd(), PollFlags::IN)];
        let patience = Timespec {
            tv_sec: 30,
            tv_nsec: 0,
        };
        let ready = rustix::event::poll(&mut exited, Some(&patience)).expect("polled");
        assert_eq!(ready, 1, "sleep never exited");
        assert_eq!(
            crate::process::lineage_under_yuna(sleeper.pid()).0,
            [sleeper.pid()],
            "not yet reaped, it is still under the test"
        );
        let gone = ClientProcess::accepted(Some(sleeper.pid()), Ok(pidfd), &mut warned);
        assert_eq!(gone.identity, Identity::Exited);
        assert_eq!(gone.lineage, Vec::<i32>::new());
        assert_eq!(gone.group, None);
        assert!(!warned);
    }
}
