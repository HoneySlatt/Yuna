//! The headless compositor as a library: what the `yuna` program runs, and what tests drive in
//! their own process ([ADR-0025](../../../docs/adr/0025-applications-in-ci.md)).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::PoisonError;
use std::time::{Duration, Instant};

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{
    EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
};
use smithay::reexports::wayland_server::{BindError, Display, ListeningSocket};
use smithay::utils::{Physical, Rectangle};
use smithay::wayland::socket::ListeningSocketSource;
use yuna_layout::geometry::Rect;
use yuna_layout::{Layout, OutputId, SeatId, WindowId};

use crate::Error;
use crate::capture::{CaptureSize, CaptureTarget, CaptureTicket, CapturedFrame};
use crate::clock::{Clock, ClockMode};
use crate::input::PointerDescription;
use crate::output::OutputMode;
use crate::render::Device;
use crate::state::{OWNER_SEAT, State};

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
    /// ([ADR-0035](../../../docs/adr/0035-agents-seats.md)).
    pub announce_owner_seat: bool,
}

impl Default for Config {
    /// What `yuna --headless` runs: one default output, llvmpipe, real time, a socket.
    fn default() -> Config {
        Config {
            outputs: vec![OutputMode::default()],
            device: Device::Software,
            clock: ClockMode::Real,
            listen: true,
            announce_owner_seat: true,
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
    /// `xdg_toplevel` and `xdg_popup` configures sent.
    pub configures: u64,
    /// `wl_surface.commit` requests received.
    pub commits: u64,
    /// Times the event loop woke: a dispatch in which a client, a new connection or the clock's
    /// timer had something; with the manual clock, each distinct time at which an advance ran
    /// deadlines, as the timer would have woken it.
    pub wakeups: u64,
}

/// A window, as [`Compositor::windows`] describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowDescription {
    /// Its id in the layout.
    pub id: WindowId,
    /// The application id its client set (`xdg_toplevel.set_app_id`), if any.
    pub app_id: Option<String>,
    /// Its title (`xdg_toplevel.set_title`), if any.
    pub title: Option<String>,
    /// The process id of its client, as the kernel gives it for the client's socket.
    pub pid: Option<i32>,
    /// The seat whose workspaces hold it: its client's.
    pub seat: SeatId,
    /// The output that shows it, as last arranged; none when it is hidden.
    pub output: Option<OutputId>,
    /// Its workspace's name; none for an unnamed workspace.
    pub workspace: Option<String>,
    /// Where it is drawn, in logical coordinates, as last arranged; none when it is hidden.
    pub rect: Option<Rect>,
    /// Whether its seat focuses it.
    pub focused: bool,
    /// Whether it floats.
    pub floating: bool,
    /// Whether it is fullscreen.
    pub fullscreen: bool,
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
    /// Its size, rate and scale.
    pub mode: OutputMode,
    /// Its place in logical coordinates.
    pub rect: Rect,
}

/// A headless compositor: its event loop, its Wayland display and its state.
///
/// Everything runs on the thread that created it: drive it with [`Compositor::dispatch`] and,
/// with the manual clock, [`Compositor::advance`], or let [`Compositor::run_until`] run it.
pub struct Compositor {
    event_loop: EventLoop<'static, State>,
    display: Display<State>,
    state: State,
    socket_name: Option<OsString>,
    /// The calloop timer armed at the clock's earliest deadline, in real mode, and its time.
    timer: Option<(RegistrationToken, Duration)>,
    /// The source of each agent seat's socket.
    seat_sockets: HashMap<SeatId, RegistrationToken>,
}

impl Compositor {
    /// Starts a compositor: its renderer, its globals, its outputs, and its socket if
    /// `config.listen`. Each output's first frame is armed.
    pub fn new(config: Config) -> Result<Compositor, Error> {
        // The socket first, so that a missing XDG_RUNTIME_DIR is refused before the renderer
        // starts.
        let socket = if config.listen {
            Some(ListeningSocketSource::new_auto().map_err(|e| {
                match e {
                    BindError::RuntimeDirNotSet => Error::Socket(
                        "XDG_RUNTIME_DIR is not set, or not an absolute path; \
                     Yuna creates its Wayland socket there"
                            .to_owned(),
                    ),
                    e => Error::Socket(e.to_string()),
                }
            })?)
        } else {
            None
        };
        let event_loop: EventLoop<'static, State> =
            EventLoop::try_new().map_err(|e| Error::EventLoop(e.to_string()))?;
        let mut display: Display<State> =
            Display::new().map_err(|e| Error::EventLoop(e.to_string()))?;
        let mut state = State::new(
            display.handle(),
            config.clock,
            config.device,
            config.announce_owner_seat,
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
        handle
            .insert_source(
                Generic::new(display_fd, Interest::READ, Mode::Level),
                |_, _, state| {
                    state.clients_readable = true;
                    state.woke = true;
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|e| Error::EventLoop(e.error.to_string()))?;
        let socket_name = match socket {
            Some(socket) => {
                let name = socket.socket_name().to_os_string();
                handle
                    .insert_source(socket, |stream, _, state| {
                        state.woke = true;
                        if let Err(error) = state.insert_client(OWNER_SEAT, stream) {
                            tracing::warn!(%error, "a client could not connect");
                        }
                    })
                    .map_err(|e| Error::EventLoop(e.error.to_string()))?;
                tracing::info!(socket = ?name, "listening");
                Some(name)
            }
            None => None,
        };
        Ok(Compositor {
            event_loop,
            display,
            state,
            socket_name,
            timer: None,
            seat_sockets: HashMap::new(),
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
    /// socket. Refused for a seat that does not exist.
    pub fn connect_client_to(&mut self, seat: SeatId) -> Result<UnixStream, Error> {
        let (ours, theirs) = UnixStream::pair().map_err(|e| Error::EventLoop(e.to_string()))?;
        self.state.insert_client(seat, ours)?;
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
    /// Refused for the owner's seat, a seat that does not exist or listens already, and a path
    /// where a socket cannot be made.
    pub fn listen_for_seat(&mut self, seat: SeatId, path: PathBuf) -> Result<(), Error> {
        if seat == OWNER_SEAT || self.state.seat(seat).is_none() {
            return Err(Error::Refused(format!("no agent seat {}", seat.0)));
        }
        if self.seat_sockets.contains_key(&seat) {
            return Err(Error::Refused(format!("seat {} listens already", seat.0)));
        }
        let socket = ListeningSocket::bind_absolute(path.clone())
            .map_err(|e| Error::Socket(format!("{}: {e}", path.display())))?;
        let token = self
            .event_loop
            .handle()
            .insert_source(
                Generic::new(socket, Interest::READ, Mode::Level),
                move |_, socket, state| {
                    state.woke = true;
                    loop {
                        match socket.accept() {
                            Ok(Some(stream)) => {
                                if let Err(error) = state.insert_client(seat, stream) {
                                    tracing::warn!(%error, "a client could not connect");
                                }
                            }
                            Ok(None) => return Ok(PostAction::Continue),
                            // Out of descriptors, most often: one seat's programs must not stop
                            // Yuna (ADR-0007). The socket rests a moment rather than spin.
                            Err(error) => {
                                tracing::warn!(%error, seat = seat.0, "a seat's socket pauses");
                                state.paused_sockets.push(seat);
                                return Ok(PostAction::Disable);
                            }
                        }
                    }
                },
            )
            .map_err(|e| Error::EventLoop(e.error.to_string()))?;
        self.seat_sockets.insert(seat, token);
        Ok(())
    }

    /// Removes agent seat `seat` ([ADR-0037](../../../docs/adr/0037-seats-tools-input-and-closing.md)):
    /// releases the keys and buttons it holds and moves its focus off every surface; removes its
    /// socket; ends its clients and frees them now, their windows closed; then removes its
    /// outputs and its place in the layout, and disables its `wl_seat` global. Returns the
    /// process ids of the clients it ended. Refused for the owner's seat and for a seat that does
    /// not exist.
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
        let pids = self.state.end_seat_clients(seat);
        self.free_departed_clients();
        self.state.finish_removing_seat(seat)?;
        self.flush();
        Ok(pids)
    }

    /// The process ids of `seat`'s clients, as the kernel gave them for their sockets.
    pub fn client_pids(&self, seat: SeatId) -> Vec<i32> {
        self.state.client_pids(seat)
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
        self.state.pointer_move(seat, output, x, y)
    }

    /// Presses or releases `button` (Linux's code, such as [`BUTTON_LEFT`](crate::BUTTON_LEFT))
    /// of agent seat `seat`'s pointer where it is. A press focuses the window under it, which
    /// takes the seat's keyboard at the next refresh. Refused for a press of a button held and a
    /// release of one not held.
    pub fn pointer_button(
        &mut self,
        seat: SeatId,
        button: u32,
        pressed: bool,
    ) -> Result<PointerDescription, Error> {
        self.state.pointer_button(seat, button, pressed)
    }

    /// Scrolls agent seat `seat`'s wheel by `dx` and `dy` steps where its pointer is, positive
    /// right and down; at most [`MAX_SCROLL_STEPS`](crate::MAX_SCROLL_STEPS) on an axis.
    pub fn pointer_scroll(
        &mut self,
        seat: SeatId,
        dx: f64,
        dy: f64,
    ) -> Result<PointerDescription, Error> {
        self.state.pointer_scroll(seat, dx, dy)
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
    /// `seat`'s keyboard, to the surface it is on; no modifier is added. Refused for a name the
    /// seat's keymap has no key for, a press of a key held and a release of one not held.
    pub fn key(&mut self, seat: SeatId, name: &str, pressed: bool) -> Result<(), Error> {
        let key = self.state.keycode_of(seat, name)?;
        self.state.key(seat, key, pressed)
    }

    /// Taps `combination` on agent seat `seat`'s keyboard: keys joined by `+`, the last pressed
    /// while the others are held ("ctrl+shift+t"); `ctrl`, `shift`, `alt`, `super` and `altgr`
    /// name the left modifiers.
    pub fn tap_keys(&mut self, seat: SeatId, combination: &str) -> Result<(), Error> {
        self.state.tap_keys(seat, combination)
    }

    /// Types `text` on agent seat `seat`'s keyboard, to the surface it is on, through its `us`
    /// keymap, Shift held where a character needs it; a character the keymap lacks is typed
    /// through a keymap given to the client for it. At most [`MAX_TYPED`](crate::MAX_TYPED)
    /// characters.
    pub fn type_text(&mut self, seat: SeatId, text: &str) -> Result<(), Error> {
        self.state.type_text(seat, text)
    }

    /// Runs one round of the event loop: waits up to `timeout` for a client, a connection or
    /// the clock's timer (`None`: as long as it takes), handles what came, runs the deadlines due
    /// by now, and sends the events written. With the manual clock, time does not move: only
    /// the deadlines armed at its current time run, such as an idle output's refresh.
    pub fn dispatch(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        self.arm_timer()?;
        self.event_loop
            .dispatch(timeout, &mut self.state)
            .map_err(|e| Error::EventLoop(e.to_string()))?;
        if std::mem::take(&mut self.state.timer_fired) {
            // A timer that fired has removed itself.
            self.timer = None;
        }
        self.pause_and_resume_sockets()?;
        if std::mem::take(&mut self.state.clients_readable) {
            self.display
                .dispatch_clients(&mut self.state)
                .map_err(|e| Error::EventLoop(e.to_string()))?;
        }
        let now = self.state.clock.now();
        while let Some((_, deadline)) = self.state.clock.take_due(now) {
            self.state.fire(deadline);
        }
        if std::mem::take(&mut self.state.woke) {
            self.state.counters.wakeups += 1;
        }
        self.flush();
        Ok(())
    }

    /// Arms a timer of [`SOCKET_PAUSE`] for each seat socket that paused on an `accept` error,
    /// and listens again on each whose pause ended, while its seat lives.
    fn pause_and_resume_sockets(&mut self) -> Result<(), Error> {
        let handle = self.event_loop.handle();
        for seat in std::mem::take(&mut self.state.paused_sockets) {
            handle
                .insert_source(Timer::from_duration(SOCKET_PAUSE), move |_, _, state| {
                    state.woke = true;
                    state.resumed_sockets.push(seat);
                    TimeoutAction::Drop
                })
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
    /// [`Counters`] at the `debug` level, in one line whose message is `counters`.
    pub fn run_until(&mut self, stop: OwnedFd) -> Result<(), Error> {
        self.event_loop
            .handle()
            .insert_source(
                Generic::new(stop, Interest::READ, Mode::Level),
                |_, _, state| {
                    state.stop_requested = true;
                    state.woke = true;
                    Ok(PostAction::Remove)
                },
            )
            .map_err(|e| Error::EventLoop(e.error.to_string()))?;
        while !self.state.stop_requested {
            let wakeups = self.state.counters.wakeups;
            self.dispatch(None)?;
            self.log_counters_after(wakeups);
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
        let mut last_wakeup = None;
        while let Some((at, deadline)) = self.state.clock.take_due(target) {
            self.state.clock.move_to(at);
            if last_wakeup != Some(at) {
                self.state.counters.wakeups += 1;
                last_wakeup = Some(at);
            }
            self.state.fire(deadline);
        }
        self.state.clock.move_to(target);
        self.flush();
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

    /// Whether the layout changed since it was last arranged: the windows' places are those of
    /// the last arrangement until the next refresh.
    pub(crate) fn arrangement_pending(&self) -> bool {
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
        self.state.dmabuf_formats
    }

    /// What it did so far.
    pub fn counters(&self) -> Counters {
        self.state.counters
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
    /// once and destroyed [`RETIRED_GLOBAL_LIFETIME`](crate::RETIRED_GLOBAL_LIFETIME) later.
    /// Refused for an output that does not exist, and for its seat's last one.
    pub fn remove_output(&mut self, output: OutputId) -> Result<(), Error> {
        self.state.remove_output(output)
    }

    /// Every output, in the order they were created.
    pub fn output_descriptions(&self) -> Vec<OutputDescription> {
        self.state.describe_outputs()
    }

    /// Every window the layout holds, all seats together, in the order they opened, as last
    /// arranged.
    pub fn windows(&self) -> Vec<WindowDescription> {
        self.state.describe_windows()
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

    /// Changes the layout model, as a key binding would: every output arranges it again at its
    /// next refresh.
    pub fn change_layout<R>(&mut self, change: impl FnOnce(&mut Layout) -> R) -> R {
        let result = change(&mut self.state.layout);
        self.state.mark_layout_changed();
        result
    }

    /// Sets the hook run each time a frame's drawing ends, before Yuna reads the time it ended
    /// at. With the manual clock, a hook that passes time ([`Clock::pass`]) makes drawing
    /// take that long.
    pub fn set_draw_hook(&mut self, hook: impl FnMut(&mut Clock) + 'static) {
        self.state.draw_hook = Some(Box::new(hook));
    }

    /// The pixels of `region` of `output`'s last frame, in physical pixels from its top-left
    /// corner, as bytes R, G, B, A, row by row from the top. Refused for a region not wholly on
    /// the output.
    pub fn read_pixels(
        &mut self,
        output: OutputId,
        region: Rectangle<i32, Physical>,
    ) -> Result<Vec<u8>, Error> {
        let index = self
            .state
            .output_index(output)
            .ok_or_else(|| Error::Refused(format!("no output {}", output.0)))?;
        let virtual_output = &mut self.state.outputs[index];
        let (width, height) = (virtual_output.mode.width(), virtual_output.mode.height());
        let fits = |start: i32, length: i32, side: u32| {
            start >= 0 && length > 0 && i64::from(start) + i64::from(length) <= i64::from(side)
        };
        if !fits(region.loc.x, region.size.w, width) || !fits(region.loc.y, region.size.h, height) {
            return Err(Error::Refused(format!(
                "the region {region:?} is not wholly on output {}",
                output.0
            )));
        }
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
        let next = self.state.clock.next_deadline();
        if next == self.timer.map(|(_, at)| at) {
            return Ok(());
        }
        let handle = self.event_loop.handle();
        if let Some((token, _)) = self.timer.take() {
            handle.remove(token);
        }
        if let Some(at) = next {
            // calloop's timers take an `Instant`, which reads the same CLOCK_MONOTONIC.
            let deadline = Instant::now() + at.saturating_sub(self.state.clock.now());
            let token = handle
                .insert_source(Timer::from_deadline(deadline), |_, _, state| {
                    state.timer_fired = true;
                    state.woke = true;
                    TimeoutAction::Drop
                })
                .map_err(|e| Error::EventLoop(e.error.to_string()))?;
            self.timer = Some((token, at));
        }
        Ok(())
    }

    /// Ends a round: frees the clients the display ended, then what closed windows, destroyed
    /// popups and surfaces left behind, then sends the events written to every client. A client
    /// that cannot take them is ended by the display, and freed at the next round's end; the
    /// others go on.
    pub(crate) fn flush(&mut self) {
        self.free_departed_clients();
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
            for client in departed {
                self.state.forget_client(&client);
                // An ended client is not read: the call fails, and frees it on its way out. A
                // client a dispatch freed already is not found, and nothing happens.
                let _ = self
                    .display
                    .backend()
                    .dispatch_single_client(&mut self.state, client);
            }
        }
    }
}
