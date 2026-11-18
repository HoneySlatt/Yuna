//! Measures what `yuna mcp`'s look for events costs a round: describing every window and
//! output and comparing them with the last look (`mcp/events.rs`), at 100, 1,000 and 2,000
//! windows, the figures `docs/measurements.md` keeps.
//!
//! It runs the MCP server in its own process, on a manual clock, with one agent seat and its
//! output, 1920 × 1080 at 60 Hz and scale 1. Its windows come from clients of its own, at most
//! 1,000 a client (`MAX_WINDOWS_PER_CLIENT`), each window a surface of 64 × 64 pixels. For each
//! count it times, `--rounds` times each, 1,000 by default:
//!
//! - **no change**: a look after nothing changed, as at the end of a round in which a client
//!   only drew: the server reads the compositor's description generation and stops there;
//! - **a frame**: the same after one window committed a new buffer and its output refreshed, as
//!   a game drawing makes each round: nothing events tell changed;
//! - **a title**: a look after one window's title changed: every window and output described
//!   and compared, and one event recorded.
//!
//! Each round's look is timed alone, the client's requests and the compositor's work before it
//! left out. It prints the median, the 99th percentile and the longest of each, in
//! microseconds, and the load of the machine before and after, which a figure taken while
//! other programs load the processor reports beside it. Build it for release, then run it in the
//! development shell, alone among measurements; it needs no screen:
//!
//! ```sh
//! nix develop -c cargo build --release -p yuna --example events_look
//! nix develop -c target/release/examples/events_look
//! ```

#[path = "common/timing.rs"]
mod timing;

use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use smithay::reexports::rustix::fs::{MemfdFlags, ftruncate, memfd_create};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use yuna::mcp::Server;
use yuna::yuna_layout::SeatId;
use yuna::{ClockMode, Compositor, Config, MAX_WINDOWS_PER_CLIENT, OutputMode};

/// The window counts measured.
const COUNTS: [usize; 3] = [100, 1_000, 2_000];
/// Each window's buffer's side, in pixels.
const SIDE: i32 = 64;

fn main() -> std::process::ExitCode {
    let rounds = match timing::read_rounds(std::env::args().skip(1), 1_000, "events_look") {
        Ok(rounds) => rounds,
        Err(reason) => {
            eprintln!("events_look: {reason}");
            return std::process::ExitCode::from(2);
        }
    };
    println!("load before: {}", timing::load_average());
    println!("windows  case        median_us  p99_us  longest_us");
    for count in COUNTS {
        if let Err(reason) = measure(count, rounds) {
            eprintln!("events_look: {reason}");
            return std::process::ExitCode::FAILURE;
        }
    }
    println!("load after: {}", timing::load_average());
    std::process::ExitCode::SUCCESS
}

/// Opens `count` windows on a seat of a server of its own, then times its looks.
fn measure(count: usize, rounds: usize) -> Result<(), String> {
    let failed = |what: &str, error: &dyn std::fmt::Display| format!("{what}: {error}");
    let compositor = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Manual,
        listen: false,
        announce_owner_seat: false,
        ..Config::default()
    })
    .map_err(|e| failed("the compositor", &e))?;
    // The server reads nothing here: its input and output only need to exist.
    let (input, _requests) = UnixStream::pair().map_err(|e| failed("a socket pair", &e))?;
    let (output, _replies) = UnixStream::pair().map_err(|e| failed("a socket pair", &e))?;
    let mut server = Server::new(
        compositor,
        OwnedFd::from(input),
        OwnedFd::from(output),
        None,
    )
    .map_err(|e| failed("the server", &e))?;
    let seat = server
        .compositor()
        .add_seat()
        .map_err(|e| failed("a seat", &e))?;
    let mode = OutputMode::new(1920, 1080, 60.0, 1.0).map_err(|e| failed("a mode", &e))?;
    server
        .compositor()
        .add_output(seat, mode)
        .map_err(|e| failed("an output", &e))?;
    let mut clients = Vec::new();
    let mut opened = 0;
    while opened < count {
        let windows = (count - opened).min(MAX_WINDOWS_PER_CLIENT);
        clients.push(WindowsClient::open(&mut server, seat, windows)?);
        opened += windows;
    }
    run_deadlines(server.compositor())?;
    let described = server.compositor().windows().len();
    if described != count {
        return Err(format!("{described} windows of {count} opened"));
    }
    // The first look sees every window open; the measures start after it.
    server.record_events_now();

    let mut still = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        still.push(timing::time(|| server.record_events_now()));
    }
    let mut framed = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        clients[0].draw(&mut server)?;
        run_deadlines(server.compositor())?;
        framed.push(timing::time(|| server.record_events_now()));
    }
    let looks = server.event_looks();
    let mut retitled = Vec::with_capacity(rounds);
    for round in 0..rounds {
        clients[0].retitle(&mut server, &format!("renamed {round}"))?;
        retitled.push(timing::time(|| server.record_events_now()));
    }
    if server.event_looks() != looks + rounds as u64 {
        return Err(format!(
            "{} looks for {rounds} titles changed",
            server.event_looks() - looks
        ));
    }
    for (case, times) in [
        ("no change", still),
        ("a frame", framed),
        ("a title", retitled),
    ] {
        print_times(count, case, times);
    }
    Ok(())
}

/// Runs every deadline the manual clock has armed, its refreshes and their arrangements.
fn run_deadlines(compositor: &mut Compositor) -> Result<(), String> {
    for _ in 0..8 {
        let Some(at) = compositor.next_deadline() else {
            return Ok(());
        };
        compositor
            .advance(at.saturating_sub(compositor.now()))
            .map_err(|e| format!("the clock: {e}"))?;
    }
    Ok(())
}

fn print_times(count: usize, case: &str, times: Vec<Duration>) {
    let [median, p99, longest] = timing::summary(times);
    println!("{count:>7}  {case:<10}  {median:>9.1}  {p99:>6.1}  {longest:>10.1}");
}

/// A client of the seat with its windows, all showing one buffer.
struct WindowsClient {
    connection: Connection,
    queue: EventQueue<ClientState>,
    state: ClientState,
    windows: Vec<(wl_surface::WlSurface, xdg_toplevel::XdgToplevel)>,
    buffer: wl_buffer::WlBuffer,
    /// Kept for the buffer's memory.
    _memory: OwnedFd,
}

/// What the client's events tell.
#[derive(Default)]
struct ClientState {
    /// Each global: its name, interface and version.
    globals: Vec<(u32, String, u32)>,
    /// Each window's last configure's serial, by its `xdg_surface`, not yet acknowledged.
    configures: Vec<(xdg_surface::XdgSurface, u32)>,
    synced: bool,
}

impl WindowsClient {
    /// Connects to `seat` and opens `count` windows, each acknowledged and drawn.
    fn open(server: &mut Server, seat: SeatId, count: usize) -> Result<WindowsClient, String> {
        let stream = server
            .compositor()
            .connect_client_to(seat)
            .map_err(|e| format!("a client: {e}"))?;
        let connection = Connection::from_socket(stream).map_err(|e| format!("a client: {e}"))?;
        let mut queue = connection.new_event_queue();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let mut state = ClientState::default();
        // Its globals, read as the compositor, on this thread, answers.
        roundtrip(server, &connection, &mut queue, &mut state)?;
        let global = |interface: &str| {
            state
                .globals
                .iter()
                .find(|(_, name, _)| name == interface)
                .map(|(name, _, version)| (*name, *version))
                .ok_or_else(|| format!("no {interface}"))
        };
        let (name, version) = global("wl_compositor")?;
        let compositor: wl_compositor::WlCompositor = registry.bind(name, version.min(6), &qh, ());
        let (name, _) = global("wl_shm")?;
        let shm: wl_shm::WlShm = registry.bind(name, 1, &qh, ());
        let (name, version) = global("xdg_wm_base")?;
        let wm_base: xdg_wm_base::XdgWmBase = registry.bind(name, version.min(6), &qh, ());
        let size = SIDE * SIDE * 4;
        let memory = memfd_create("events-look", MemfdFlags::CLOEXEC)
            .map_err(|e| format!("a buffer's memory: {e}"))?;
        ftruncate(&memory, size as u64).map_err(|e| format!("a buffer's memory: {e}"))?;
        let pool = shm.create_pool(memory.as_fd(), size, &qh, ());
        let buffer = pool.create_buffer(0, SIDE, SIDE, SIDE * 4, wl_shm::Format::Argb8888, &qh, ());
        pool.destroy();
        let mut windows = Vec::with_capacity(count);
        for index in 0..count {
            let surface = compositor.create_surface(&qh, ());
            let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg_surface.get_toplevel(&qh, ());
            toplevel.set_title(format!("window {index}"));
            surface.commit();
            windows.push((surface, toplevel));
        }
        roundtrip(server, &connection, &mut queue, &mut state)?;
        for (xdg_surface, serial) in std::mem::take(&mut state.configures) {
            xdg_surface.ack_configure(serial);
        }
        for (surface, _) in &windows {
            surface.attach(Some(&buffer), 0, 0);
            surface.damage_buffer(0, 0, SIDE, SIDE);
            surface.commit();
        }
        roundtrip(server, &connection, &mut queue, &mut state)?;
        Ok(WindowsClient {
            connection,
            queue,
            state,
            windows,
            buffer,
            _memory: memory,
        })
    }

    /// Commits its first window's buffer again, damaged whole: a frame of a client that draws.
    fn draw(&mut self, server: &mut Server) -> Result<(), String> {
        let (surface, _) = &self.windows[0];
        surface.attach(Some(&self.buffer), 0, 0);
        surface.damage_buffer(0, 0, SIDE, SIDE);
        surface.commit();
        roundtrip(server, &self.connection, &mut self.queue, &mut self.state)
    }

    /// Gives its first window the title `title`.
    fn retitle(&mut self, server: &mut Server, title: &str) -> Result<(), String> {
        self.windows[0].1.set_title(title.to_owned());
        roundtrip(server, &self.connection, &mut self.queue, &mut self.state)
    }
}

/// Sends what the client wrote and lets the compositor handle it, until it has answered all of
/// it, the client's events handled; the server's own round, which would look, is not run.
fn roundtrip(
    server: &mut Server,
    connection: &Connection,
    queue: &mut EventQueue<ClientState>,
    state: &mut ClientState,
) -> Result<(), String> {
    state.synced = false;
    connection.display().sync(&queue.handle(), ());
    for _ in 0..10_000 {
        queue.flush().map_err(|e| format!("the client: {e}"))?;
        server
            .compositor()
            .dispatch(Some(Duration::ZERO))
            .map_err(|e| format!("the compositor: {e}"))?;
        if let Some(guard) = queue.prepare_read() {
            // Nothing to read yet is no failure.
            let _ = guard.read();
        }
        queue
            .dispatch_pending(state)
            .map_err(|e| format!("the client: {e}"))?;
        if state.synced {
            return Ok(());
        }
    }
    Err("the compositor did not answer".to_owned())
}

impl Dispatch<wl_registry::WlRegistry, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals.push((name, interface, version));
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.synced = true;
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

impl Dispatch<xdg_surface::XdgSurface, ()> for ClientState {
    fn event(
        state: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            state.configures.push((xdg_surface.clone(), serial));
        }
    }
}

delegate_noop!(ClientState: ignore wl_compositor::WlCompositor);
delegate_noop!(ClientState: ignore wl_surface::WlSurface);
delegate_noop!(ClientState: ignore wl_shm::WlShm);
delegate_noop!(ClientState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(ClientState: ignore wl_buffer::WlBuffer);
delegate_noop!(ClientState: ignore xdg_toplevel::XdgToplevel);
