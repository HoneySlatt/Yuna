//! XWayland: X11 programs on Yuna, one Xwayland per seat, started on demand, behind a cookie
//! ([ADR-0066](../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md),
//! [ADR-0067](../../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md),
//! [ADR-0068](../../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)).
//!
//! When XWayland is on ([`XwaylandConfig`]), each seat that gets a Wayland socket gets an X
//! display of its own: a number, two listening sockets, a lock and a cookie (`display.rs`). Yuna
//! watches the sockets; nothing runs until an X11 program connects. Each seat's display then goes
//! through three phases, all on Yuna's loop, none of them waiting:
//!
//! - **Watching**: the first connection starts Xwayland (`server.rs`) on the display's sockets,
//!   with a Wayland connection Yuna records as a client of the seat, at the client scale S, the
//!   largest of the seat's outputs then, fixed for that Xwayland's life. Yuna stops watching: the
//!   waiting program stays in the socket's backlog until Xwayland accepts it.
//! - **Serving**: once `-displayfd` gives the number, the manager's thread connects and takes
//!   charge (`manager.rs`); once it owns `WM_S0`, Xwayland accepts the waiting program. A start
//!   that has not reached that point within [`START_TIMEOUT`] is ended. Xwayland ends itself
//!   [`XwaylandConfig::idle_seconds`] after its last X11 client leaves; it may also crash, be
//!   killed, or be ended by Yuna when its manager fails. Its pidfd reaps it, its Wayland client
//!   goes, and the display is watched again.
//! - **Paused**: after starts that fail in a row, the display waits before it is watched again
//!   ([`pause_after`]); programs that connect meanwhile wait in the backlog.
//!
//! A seat removed ends its Xwayland and removes its display's files; so does the compositor when
//! it drops, waiting for its Xwaylands to be reaped. At rest, before the first connection and
//! after Xwayland exits, Yuna holds two sockets per seat and nothing else: no process, no timer.

mod display;
mod manager;
mod server;

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use smithay::reexports::calloop::channel::{self, Channel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use smithay::reexports::rustix;
use smithay::reexports::rustix::io::Errno;
use smithay::reexports::rustix::process::Pid;
use yuna_layout::SeatId;

use crate::render::Device;
use crate::state::State;
use display::Display;
pub use display::MAX_DISPLAYS;
use manager::{Command, Commands, ManagerEvent, Stop};
use server::End;

/// How long Xwayland waits, by default, once its last X11 client has left, before it ends:
/// `-terminate`'s delay, as KWin has it. A program started again within it finds Xwayland
/// running.
pub const IDLE_SECONDS: u32 = 10;

/// How long a start may take, from the spawn to the manager owning `WM_S0`, before Yuna ends it
/// as failed: a start takes a fraction of a second, a few seconds on a loaded machine.
pub const START_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an Xwayland that closed its manager's connection has to exit before Yuna ends it.
/// It closes it when it exits, its teardown taking milliseconds; an Xwayland still there after
/// it lost its manager otherwise, to another client's `KillClient`.
pub const MANAGER_GRACE: Duration = Duration::from_secs(5);

/// How long a failed start is remembered: the next failure within it is one more in a row.
pub const FAILURE_MEMORY: Duration = Duration::from_secs(60);

/// The pause after the second failure in a row, doubled at each further one up to
/// [`LONGEST_PAUSE`].
pub const FIRST_PAUSE: Duration = Duration::from_secs(1);

/// The longest pause between starts that fail.
pub const LONGEST_PAUSE: Duration = Duration::from_secs(30);

/// XWayland's settings in the compositor's [`Config`](crate::Config).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandConfig {
    /// How long Xwayland stays once its last X11 client has gone, in whole seconds
    /// (`-terminate`); [`IDLE_SECONDS`] by default.
    pub idle_seconds: u32,
    /// The folders Xwayland is looked for in, as `PATH` lists them; Yuna's own `PATH` when none.
    pub search_path: Option<OsString>,
    /// The folder of the owner's seat's Xauthority file, made by Yuna, when the compositor does
    /// not listen: the owner's seat then gets a display without a Wayland socket. For tests.
    /// When it listens, the folder is beside its socket.
    pub owner_display_folder: Option<PathBuf>,
}

impl Default for XwaylandConfig {
    fn default() -> XwaylandConfig {
        XwaylandConfig {
            idle_seconds: IDLE_SECONDS,
            search_path: None,
            owner_display_folder: None,
        }
    }
}

/// A seat's X display, as [`Compositor::x11_display`](crate::Compositor::x11_display) gives
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X11Display {
    /// Its number.
    pub number: u32,
    /// Its Xauthority file, the cookie X11 programs give: `XAUTHORITY`.
    pub xauthority: PathBuf,
    /// The process id of its Xwayland, from its start until it is reaped; none at rest.
    pub server: Option<i32>,
}

impl X11Display {
    /// Its name, as `DISPLAY` gives it: `:<number>`.
    pub fn name(&self) -> String {
        format!(":{}", self.number)
    }
}

/// The folder of the Xauthority file of the seat whose Wayland socket is at `socket`: beside it,
/// named after it, `<socket>.x11`.
pub(crate) fn folder_beside(socket: &Path) -> PathBuf {
    let mut folder = socket.as_os_str().to_owned();
    folder.push(".x11");
    PathBuf::from(folder)
}

/// What the event loop's sources noted during a round, for [`Xwayland::handle_notes`].
#[derive(Debug)]
pub(crate) enum Note {
    /// A seat's socket has a connection waiting.
    Connection(SeatId),
    /// An Xwayland wrote its display number on `-displayfd`, or closed it without one.
    Displayed { server: u64, number: Option<u32> },
    /// An Xwayland's manager posted this.
    Manager { server: u64, event: ManagerEvent },
    /// An Xwayland's manager's thread has ended.
    ManagerGone { server: u64 },
    /// An Xwayland has not started, or not exited, in time.
    Overdue { server: u64 },
    /// An Xwayland has ended, and was reaped.
    Exited { server: u64, end: End },
    /// A seat's pause after failed starts has ended.
    PauseEnded(SeatId),
}

/// XWayland in a compositor: Xwayland's program, each seat's display, and each Xwayland that
/// runs or ends.
#[derive(Debug)]
pub(crate) struct Xwayland {
    program: PathBuf,
    idle_seconds: u32,
    /// Whether Xwayland draws on the CPU alone, without GLX: when Yuna draws on llvmpipe
    /// (`server.rs`).
    software: bool,
    seats: HashMap<SeatId, SeatDisplay>,
    /// Every Xwayland from its start until it is reaped, by its id; ids are never reused.
    servers: HashMap<u64, XServer>,
    next_server: u64,
    /// Whether `/tmp`'s refusal of a lock or a file socket was said: once.
    tmp_logged: bool,
}

/// A seat's display and where it stands.
#[derive(Debug)]
struct SeatDisplay {
    display: Display,
    /// The sources that watch its sockets.
    watchers: Vec<RegistrationToken>,
    phase: Phase,
    /// The starts that failed in a row, and when the last one did.
    failures: u32,
    last_failure: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Its sockets are watched for a first connection.
    Watching,
    /// Its sockets wait, after failed starts, until this timer fires.
    Paused(RegistrationToken),
    /// This Xwayland serves it.
    Serving(u64),
}

/// An Xwayland, from its start until it is reaped.
#[derive(Debug)]
struct XServer {
    seat: SeatId,
    pid: Pid,
    /// Signals it; the event loop watches another descriptor of it, which reaps it.
    pidfd: OwnedFd,
    /// The client scale its Wayland connection got, S, fixed for its life (ADR-0068).
    scale: f64,
    started: Instant,
    /// Whether its manager owns `WM_S0`: X11 clients are let in.
    ready: bool,
    /// Its `-wm` connection, until `-displayfd` says it is ready and the manager takes it.
    wm: Option<UnixStream>,
    manager: Option<Commands>,
    /// Whether its manager said why it stopped: an end without it is a panic.
    manager_stopped: bool,
    /// Its `-displayfd` and manager's sources.
    sources: Vec<RegistrationToken>,
    /// The timer that ends it if it does not start, or exit, in time.
    overdue: Option<RegistrationToken>,
}

impl XServer {
    /// Ends it: SIGKILL, its sources but its pidfd's removed, its manager let go. Its pidfd's
    /// source reaps it, and its exit is handled then.
    fn end(&mut self, handle: &LoopHandle<'static, State>) {
        server::kill(&self.pidfd);
        self.release(handle);
    }

    /// Removes its sources but its pidfd's, and lets its manager's thread go.
    fn release(&mut self, handle: &LoopHandle<'static, State>) {
        for token in self.sources.drain(..).chain(self.overdue.take()) {
            handle.remove(token);
        }
        self.manager = None;
        self.wm = None;
    }

    /// Arms the timer that ends it after `delay`, replacing the one armed.
    fn arm_overdue(&mut self, id: u64, delay: Duration, handle: &LoopHandle<'static, State>) {
        if let Some(token) = self.overdue.take() {
            handle.remove(token);
        }
        let armed = handle.insert_source(Timer::from_duration(delay), move |_, _, state| {
            state.woke = true;
            state.xwayland_notes.push(Note::Overdue { server: id });
            TimeoutAction::Drop
        });
        match armed {
            Ok(token) => self.overdue = Some(token),
            Err(error) => tracing::warn!(error = %error.error, "Xwayland's timer cannot be armed"),
        }
    }
}

impl Xwayland {
    /// XWayland as `config` asks, for a compositor drawing on `device`, Xwayland found on its
    /// search path; none when it is not there, which is said: X11 programs then get no display.
    pub(crate) fn new(config: &XwaylandConfig, device: Device) -> Option<Xwayland> {
        let search_path = config
            .search_path
            .clone()
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        let Some(program) = find_program("Xwayland", &search_path) else {
            tracing::warn!("Xwayland is not on PATH: X11 programs get no display");
            return None;
        };
        Some(Xwayland {
            program,
            idle_seconds: config.idle_seconds,
            software: device == Device::Software,
            seats: HashMap::new(),
            servers: HashMap::new(),
            next_server: 1,
            tmp_logged: false,
        })
    }

    /// Gives `seat` an X display, its Xauthority file in `folder`, and watches its sockets. A
    /// display that cannot be made is logged, and the seat goes without: X11 programs never stop
    /// a seat ([ADR-0007](../../../../docs/adr/0007-surviving-a-crash.md)).
    pub(crate) fn add_seat(
        &mut self,
        seat: SeatId,
        folder: &Path,
        handle: &LoopHandle<'static, State>,
    ) {
        let (display, shortfall) = match Display::reserve(folder) {
            Ok(reserved) => reserved,
            Err(error) => {
                tracing::warn!(%error, seat = seat.0, "no X display for a seat");
                return;
            }
        };
        if let Some(error) = shortfall.tmp
            && !std::mem::replace(&mut self.tmp_logged, true)
        {
            tracing::warn!(
                %error,
                "/tmp holds no X11 lock or file socket: X11 programs reach Yuna's displays \
                 through their abstract sockets alone"
            );
        }
        let mut watchers = Vec::new();
        for socket in display.sockets() {
            match watch(socket, seat, handle) {
                Ok(token) => watchers.push(token),
                Err(error) => {
                    tracing::warn!(%error, seat = seat.0, "no X display for a seat");
                    for token in watchers {
                        handle.remove(token);
                    }
                    return;
                }
            }
        }
        let number = display.number;
        tracing::info!(seat = seat.0, display = number, "X display reserved");
        self.seats.insert(
            seat,
            SeatDisplay {
                display,
                watchers,
                phase: Phase::Watching,
                failures: 0,
                last_failure: None,
            },
        );
    }

    /// Takes `seat`'s display away: ends its Xwayland, stops watching its sockets, and removes
    /// its files.
    pub(crate) fn remove_seat(&mut self, seat: SeatId, handle: &LoopHandle<'static, State>) {
        let Some(removed) = self.seats.remove(&seat) else {
            return;
        };
        for &token in &removed.watchers {
            handle.remove(token);
        }
        match removed.phase {
            Phase::Watching => {}
            Phase::Paused(timer) => handle.remove(timer),
            Phase::Serving(id) => {
                if let Some(server) = self.servers.get_mut(&id) {
                    server.end(handle);
                }
            }
        }
        tracing::info!(
            seat = seat.0,
            display = removed.display.number,
            "X display removed"
        );
    }

    /// `seat`'s display, while it has one.
    pub(crate) fn display(&self, seat: SeatId) -> Option<X11Display> {
        let seat_display = self.seats.get(&seat)?;
        let server = match seat_display.phase {
            Phase::Serving(id) => self
                .servers
                .get(&id)
                .map(|server| server.pid.as_raw_nonzero().get()),
            Phase::Watching | Phase::Paused(_) => None,
        };
        Some(X11Display {
            number: seat_display.display.number,
            xauthority: seat_display.display.xauthority.path.clone(),
            server,
        })
    }

    /// Acts on what the event loop's sources noted in this round: starts, readiness, the
    /// managers' events, ends.
    pub(crate) fn handle_notes(&mut self, state: &mut State, handle: &LoopHandle<'static, State>) {
        for note in std::mem::take(&mut state.xwayland_notes) {
            match note {
                Note::Connection(seat) => self.start(seat, state, handle),
                Note::Displayed { server, number } => self.displayed(server, number, handle),
                Note::Manager { server, event } => self.manager_event(server, event, handle),
                Note::ManagerGone { server } => self.manager_gone(server, handle),
                Note::Overdue { server } => self.overdue(server, handle),
                Note::Exited { server, end } => self.exited(server, end, handle),
                Note::PauseEnded(seat) => self.watch(seat, handle),
            }
        }
    }

    /// Starts Xwayland for `seat`, whose socket has a connection waiting, if it is watched.
    fn start(&mut self, seat: SeatId, state: &mut State, handle: &LoopHandle<'static, State>) {
        let Some(seat_display) = self.seats.get_mut(&seat) else {
            return;
        };
        if seat_display.phase != Phase::Watching {
            return;
        }
        // Xwayland accepts on them from now on; Yuna would only wake for nothing.
        for token in &seat_display.watchers {
            if let Err(error) = handle.disable(token) {
                tracing::warn!(%error, "an X11 socket cannot stop being watched");
            }
        }
        let scale = largest_scale(state, seat);
        let spawned = match server::spawn(
            &self.program,
            &seat_display.display,
            self.idle_seconds,
            self.software,
        ) {
            Ok(spawned) => spawned,
            Err(error) => {
                tracing::warn!(%error, seat = seat.0, "Xwayland cannot start");
                self.after_server(seat, true, handle);
                return;
            }
        };
        let id = self.next_server;
        self.next_server += 1;
        if let Err(error) = reap_on_exit(&spawned.pidfd, id, handle) {
            tracing::warn!(%error, seat = seat.0, "Xwayland cannot be watched: it ends");
            server::kill_and_reap(&spawned.pidfd);
            self.after_server(seat, true, handle);
            return;
        }
        seat_display.phase = Phase::Serving(id);
        let number = seat_display.display.number;
        let mut xserver = XServer {
            seat,
            pid: spawned.pid,
            pidfd: spawned.pidfd,
            scale,
            started: Instant::now(),
            ready: false,
            wm: Some(spawned.wm),
            manager: None,
            manager_stopped: false,
            sources: Vec::new(),
            overdue: None,
        };
        tracing::info!(
            seat = seat.0,
            display = number,
            pid = spawned.pid.as_raw_nonzero().get(),
            scale,
            "Xwayland started"
        );
        xserver.arm_overdue(id, START_TIMEOUT, handle);
        let connected = state
            .insert_xwayland_client(seat, spawned.wayland, scale)
            .map_err(|error| error.to_string())
            .and_then(|()| watch_displayfd(spawned.displayfd, id, handle));
        if let Err(reason) = connected {
            tracing::warn!(%reason, seat = seat.0, "Xwayland cannot start: it ends");
            xserver.end(handle);
        }
        self.servers.insert(id, xserver);
    }

    /// Xwayland `id` wrote its display number, or closed `-displayfd` without one: its manager
    /// starts, or it ends.
    fn displayed(&mut self, id: u64, number: Option<u32>, handle: &LoopHandle<'static, State>) {
        let Some(xserver) = self.servers.get_mut(&id) else {
            return;
        };
        let Some(number) = number else {
            tracing::warn!(
                seat = xserver.seat.0,
                "Xwayland closed -displayfd without a display number: it ends"
            );
            xserver.end(handle);
            return;
        };
        let Some(wm) = xserver.wm.take() else {
            return;
        };
        tracing::debug!(
            seat = xserver.seat.0,
            display = number,
            "Xwayland is ready for its manager"
        );
        let (events, channel) = channel::sync_channel(manager::EVENT_QUEUE);
        let started = listen_to_manager(channel, id, handle).and_then(|token| {
            xserver.sources.push(token);
            manager::start(xserver.seat, wm, xserver.scale, events)
                .map_err(|error| format!("its manager's thread cannot start: {error}"))
        });
        match started {
            Ok(commands) => xserver.manager = Some(commands),
            Err(reason) => {
                tracing::warn!(%reason, seat = xserver.seat.0, "Xwayland cannot start: it ends");
                xserver.end(handle);
            }
        }
    }

    fn manager_event(&mut self, id: u64, event: ManagerEvent, handle: &LoopHandle<'static, State>) {
        let Some(xserver) = self.servers.get_mut(&id) else {
            return;
        };
        match event {
            ManagerEvent::Ready => {
                xserver.ready = true;
                if let Some(token) = xserver.overdue.take() {
                    handle.remove(token);
                }
                tracing::info!(
                    seat = xserver.seat.0,
                    took_ms = xserver.started.elapsed().as_millis(),
                    "Xwayland ready"
                );
            }
            // Until Yuna places X11 windows: mapped as the program asks.
            ManagerEvent::MapRequest(window) => {
                if let Some(manager) = &xserver.manager {
                    manager.send(Command::Map(window));
                }
            }
            ManagerEvent::Stopped(stop) => {
                xserver.manager_stopped = true;
                match stop {
                    Stop::Closed => xserver.arm_overdue(id, MANAGER_GRACE, handle),
                    Stop::Released => {}
                    Stop::Failed(reason) => {
                        tracing::warn!(
                            %reason,
                            seat = xserver.seat.0,
                            "Xwayland's manager failed: Xwayland ends"
                        );
                        xserver.end(handle);
                    }
                }
            }
        }
    }

    /// Manager `id`'s thread has ended: without saying why, it panicked, and its Xwayland ends.
    fn manager_gone(&mut self, id: u64, handle: &LoopHandle<'static, State>) {
        let Some(xserver) = self.servers.get_mut(&id) else {
            return;
        };
        xserver.manager = None;
        if !xserver.manager_stopped {
            tracing::error!(
                seat = xserver.seat.0,
                "Xwayland's manager panicked: Xwayland ends"
            );
            xserver.end(handle);
        }
    }

    fn overdue(&mut self, id: u64, handle: &LoopHandle<'static, State>) {
        let Some(xserver) = self.servers.get_mut(&id) else {
            return;
        };
        xserver.overdue = None;
        if xserver.ready {
            tracing::warn!(
                seat = xserver.seat.0,
                "Xwayland outlived its manager: it ends"
            );
        } else {
            tracing::warn!(
                seat = xserver.seat.0,
                timeout = ?START_TIMEOUT,
                "Xwayland did not start in time: it ends"
            );
        }
        xserver.end(handle);
    }

    /// Xwayland `id` has ended and been reaped: its seat's display is watched again, or pauses
    /// after starts that failed in a row.
    fn exited(&mut self, id: u64, end: End, handle: &LoopHandle<'static, State>) {
        let Some(mut xserver) = self.servers.remove(&id) else {
            return;
        };
        xserver.release(handle);
        // `-terminate` exits with 0; another process may have reaped it, in `yuna mcp`, and then
        // its end is known only to be after its start.
        let failed = !xserver.ready || !matches!(end, End::Exited(0) | End::Unknown);
        let seat = xserver.seat;
        if failed {
            tracing::warn!(seat = seat.0, ?end, ready = xserver.ready, "Xwayland ended");
        } else {
            tracing::info!(seat = seat.0, ?end, "Xwayland stopped");
        }
        let serving = self
            .seats
            .get(&seat)
            .is_some_and(|seat_display| seat_display.phase == Phase::Serving(id));
        if serving {
            self.after_server(seat, failed, handle);
        }
    }

    /// Once `seat`'s Xwayland has ended, or failed to start: counts a failure in a row, then
    /// watches the display again, at once or after a pause.
    fn after_server(&mut self, seat: SeatId, failed: bool, handle: &LoopHandle<'static, State>) {
        let Some(seat_display) = self.seats.get_mut(&seat) else {
            return;
        };
        let now = Instant::now();
        if failed {
            let in_a_row = seat_display
                .last_failure
                .is_some_and(|at| now.duration_since(at) < FAILURE_MEMORY);
            seat_display.failures = if in_a_row {
                seat_display.failures + 1
            } else {
                1
            };
            seat_display.last_failure = Some(now);
        } else {
            seat_display.failures = 0;
        }
        let Some(pause) = pause_after(seat_display.failures) else {
            self.watch(seat, handle);
            return;
        };
        if pause >= LONGEST_PAUSE {
            // Xwayland fails before it can serve anyone: the connections waiting since would
            // start it again at each pause's end, for ever. Their programs gave up long ago.
            seat_display.display.refuse_waiting();
        }
        tracing::warn!(
            seat = seat.0,
            failures = seat_display.failures,
            ?pause,
            "Xwayland failed in a row: its X11 programs wait"
        );
        let armed = handle.insert_source(Timer::from_duration(pause), move |_, _, state| {
            state.woke = true;
            state.xwayland_notes.push(Note::PauseEnded(seat));
            TimeoutAction::Drop
        });
        match armed {
            Ok(timer) => seat_display.phase = Phase::Paused(timer),
            // Without a timer, at once rather than never.
            Err(_) => self.watch(seat, handle),
        }
    }

    /// Watches `seat`'s sockets again for a connection.
    fn watch(&mut self, seat: SeatId, handle: &LoopHandle<'static, State>) {
        let Some(seat_display) = self.seats.get_mut(&seat) else {
            return;
        };
        seat_display.phase = Phase::Watching;
        for token in &seat_display.watchers {
            if let Err(error) = handle.enable(token) {
                tracing::warn!(%error, seat = seat.0, "an X11 socket cannot be watched again");
            }
        }
    }
}

impl Drop for Xwayland {
    /// Ends every Xwayland and waits until each is reaped, with no loop left to watch them; the
    /// displays' files go after.
    fn drop(&mut self) {
        for xserver in self.servers.values() {
            server::kill_and_reap(&xserver.pidfd);
        }
    }
}

/// How long `seat`'s display pauses after `failures` starts failed in a row: none after one, so
/// that a crash costs a program nothing; then [`FIRST_PAUSE`], doubled at each further failure,
/// up to [`LONGEST_PAUSE`]. A start costs Xwayland a fraction of a second of a core: a crash
/// loop takes at most about a quarter of one, and once its cause is gone, X11 programs wait at
/// most [`LONGEST_PAUSE`].
pub(crate) fn pause_after(failures: u32) -> Option<Duration> {
    let doublings = failures.checked_sub(2)?;
    let pause = FIRST_PAUSE.saturating_mul(1 << doublings.min(16));
    Some(pause.min(LONGEST_PAUSE))
}

/// The largest scale among `seat`'s outputs; 1 for a seat without one.
fn largest_scale(state: &State, seat: SeatId) -> f64 {
    state
        .outputs
        .iter()
        .filter(|output| output.seat == seat)
        .map(|output| output.mode.scale())
        .reduce(f64::max)
        .unwrap_or(1.0)
}

/// Watches a display's listening socket for a connection, for `seat`.
fn watch(
    socket: &OwnedFd,
    seat: SeatId,
    handle: &LoopHandle<'static, State>,
) -> Result<RegistrationToken, String> {
    let socket = socket.try_clone().map_err(|error| error.to_string())?;
    handle
        .insert_source(
            Generic::new(socket, Interest::READ, Mode::Level),
            move |_, _, state| {
                state.woke = true;
                state.xwayland_notes.push(Note::Connection(seat));
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| error.error.to_string())
}

/// Watches Xwayland `id`'s process, through a copy of `pidfd`, until it ends, then reaps it.
fn reap_on_exit(
    pidfd: &OwnedFd,
    id: u64,
    handle: &LoopHandle<'static, State>,
) -> Result<RegistrationToken, String> {
    let pidfd = pidfd.try_clone().map_err(|error| error.to_string())?;
    handle
        .insert_source(
            Generic::new(pidfd, Interest::READ, Mode::Level),
            move |_, pidfd, state| {
                let Some(end) = server::reap(pidfd.as_fd()) else {
                    return Ok(PostAction::Continue);
                };
                state.woke = true;
                state.xwayland_notes.push(Note::Exited { server: id, end });
                Ok(PostAction::Remove)
            },
        )
        .map_err(|error| error.error.to_string())
}

/// Reads Xwayland `id`'s `-displayfd` until a whole line, its display number, or its end.
fn watch_displayfd(
    displayfd: OwnedFd,
    id: u64,
    handle: &LoopHandle<'static, State>,
) -> Result<(), String> {
    let mut line = Vec::new();
    handle
        .insert_source(
            Generic::new(displayfd, Interest::READ, Mode::Level),
            move |_, displayfd, state| {
                state.woke = true;
                let mut bytes = [0; 16];
                let number = loop {
                    match rustix::io::read(displayfd.as_fd(), &mut bytes) {
                        Ok(0) => break None,
                        Ok(read) => {
                            line.extend_from_slice(&bytes[..read]);
                            if let Some(end) = line.iter().position(|&byte| byte == b'\n') {
                                break std::str::from_utf8(&line[..end])
                                    .ok()
                                    .and_then(|text| text.trim().parse().ok());
                            }
                            // A number is a few digits: a longer line is no number.
                            if line.len() > 32 {
                                break None;
                            }
                        }
                        Err(Errno::AGAIN) => return Ok(PostAction::Continue),
                        Err(Errno::INTR) => {}
                        Err(_) => break None,
                    }
                };
                state
                    .xwayland_notes
                    .push(Note::Displayed { server: id, number });
                Ok(PostAction::Remove)
            },
        )
        .map(|_| ())
        .map_err(|error| error.error.to_string())
}

/// Passes manager `id`'s events to Yuna's loop.
fn listen_to_manager(
    channel: Channel<ManagerEvent>,
    id: u64,
    handle: &LoopHandle<'static, State>,
) -> Result<RegistrationToken, String> {
    handle
        .insert_source(channel, move |event, _, state| {
            state.woke = true;
            state.xwayland_notes.push(match event {
                channel::Event::Msg(event) => Note::Manager { server: id, event },
                channel::Event::Closed => Note::ManagerGone { server: id },
            });
        })
        .map_err(|error| error.error.to_string())
}

/// The first executable file named `name` in the folders of `search_path`.
fn find_program(name: &str, search_path: &OsString) -> Option<PathBuf> {
    std::env::split_paths(search_path)
        .filter(|folder| folder.is_absolute())
        .map(|folder| folder.join(name))
        .find(|path| {
            fs::metadata(path).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_failure_does_not_pause_and_more_pause_twice_as_long_up_to_a_bound() {
        let seconds = |failures| pause_after(failures).map(|pause| pause.as_secs());
        assert_eq!(seconds(0), None);
        assert_eq!(seconds(1), None);
        assert_eq!(seconds(2), Some(1));
        assert_eq!(seconds(3), Some(2));
        assert_eq!(seconds(6), Some(16));
        assert_eq!(seconds(7), Some(30));
        assert_eq!(seconds(u32::MAX), Some(30));
    }

    #[test]
    fn the_folder_of_a_seats_cookie_is_beside_its_socket() {
        assert_eq!(
            folder_beside(Path::new("/run/user/1000/yuna-mcp-7/seat-2")),
            Path::new("/run/user/1000/yuna-mcp-7/seat-2.x11")
        );
    }

    #[test]
    fn a_program_is_found_only_in_absolute_folders_and_executable() {
        let folder = std::env::temp_dir().join(format!("yuna-find-test-{}", std::process::id()));
        let (plain, executable) = (folder.join("plain"), folder.join("executable"));
        fs::create_dir_all(&plain).expect("a folder");
        fs::create_dir_all(&executable).expect("a folder");
        fs::write(plain.join("Xwayland"), "").expect("a file");
        fs::write(executable.join("Xwayland"), "").expect("a file");
        fs::set_permissions(
            executable.join("Xwayland"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("its mode");
        let path = |folders: &[&Path]| std::env::join_paths(folders).expect("a PATH");
        assert_eq!(
            find_program("Xwayland", &path(&[&plain, &executable])),
            Some(executable.join("Xwayland"))
        );
        assert_eq!(find_program("Xwayland", &path(&[&plain])), None);
        assert_eq!(
            find_program("Xwayland", &path(&[Path::new("relative")])),
            None
        );
        assert_eq!(find_program("Xwayland", &OsString::new()), None);
        fs::remove_dir_all(&folder).expect("the folder goes");
    }
}
