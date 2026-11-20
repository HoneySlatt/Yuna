//! The lockstep client, `yuna-bench client lockstep --windows <n> --control <socket>`: one
//! process opening `n`
//! `xdg_toplevel` windows of `wl_shm` that draw only when the parity script tells them to, so that
//! two builds of Yuna are given exactly the same work, frame by frame.
//!
//! It talks to the parity script through a Unix socket, `--control <socket>`, as lines: the first,
//! `wayland <socket>`, names the compositor's socket. Until every window has drawn once, it
//! answers the compositor in round trips: all it has to send in one go, then the compositor's
//! answer to all of it. Then it says `ready <n>` and reads nothing more from the compositor until
//! told: each line is a command, answered by one line. A configure is acknowledged when the window draws
//! it, in the same batch, as a client that draws at its own pace does.
//!
//! - `step none`, `step square <i>` or `step full`: reads what the compositor sent since the last
//!   step (configures, frame callbacks, buffer releases, pointer events), draws again each window
//!   given a new size or scale, then draws window `i`'s 64 × 64 square in its other colour
//!   (`square`), or every window whole in its other colour (`full`). Each commit asks a frame
//!   callback. If it committed anything, it makes a `wl_display.sync` round trip, so that the
//!   compositor has read every commit before the answer; with nothing to commit it sends nothing,
//!   and wakes nothing. It answers `stepped commits=<c> callbacks=<f> pointer=<p>
//!   configures=<g> buffers=<b>`: its commits, the frame callbacks done, the pointer events
//!   received, the configures received and the buffers made, since it started.
//! - `quit`, or its input closed: it disconnects and ends.
//!
//! No test of this crate runs it against a headless Yuna: `yuna-bench` depends on nothing of
//! `yuna`, so that it measures Yuna from outside (ADR-0075), and `yuna`'s own tests cannot start
//! this crate's binary. Every parity run tests it instead: its counts are part of the work two
//! builds must share, and a run whose client stops answering fails.
//!
//! It honours `wp_fractional_scale_v1` and `wp_viewporter` when the compositor offers both, as the
//! still client does: a buffer of the logical size times the preferred scale. It binds the seat's
//! pointer and counts its events, so that what the compositor sends a client is seen. Its windows
//! are titled `parity <n>`, from 1, which MCP's `windows` gives back.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;

use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::backend::WaylandError;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

/// The size a window takes when the compositor lets it choose, in logical pixels.
const DEFAULT_SIZE: (i32, i32) = (640, 480);
/// The square a `step square` draws: its top-left corner and its side, in buffer pixels.
pub const SQUARE: (i32, i32, i32) = (16, 16, 64);
/// The square's colour, opaque.
const SQUARE_COLOUR: u32 = 0xffff_d000;
/// The buffers a window may hold at once, the compositor not having released them: more means
/// the compositor holds them, and the client stops rather than grow without bound.
const BUFFERS_A_WINDOW: usize = 8;

/// What a window's buffer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    /// The window's colour.
    Plain,
    /// The window's colour and the square.
    Square,
    /// The window's other colour, everywhere.
    Other,
}

/// What a step draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Damage {
    /// Nothing but what a configure asks.
    None,
    /// Window `0`-based `n`'s square, in its other look.
    Square(usize),
    /// Every window, whole, in its other look.
    Full,
}

/// One command of the parity script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// A step.
    Step(Damage),
    /// The end.
    Quit,
}

/// Reads one command line; `None` if it is not one.
pub fn parse_command(line: &str) -> Option<Command> {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words[..] {
        ["step", "none"] => Some(Command::Step(Damage::None)),
        ["step", "full"] => Some(Command::Step(Damage::Full)),
        ["step", "square", n] => n
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .map(|n| Command::Step(Damage::Square(n - 1))),
        ["quit"] => Some(Command::Quit),
        _ => None,
    }
}

/// What the client did since it started, as a step's answer gives it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// `wl_surface.commit` requests.
    pub commits: u64,
    /// Frame callbacks the compositor said done.
    pub callbacks: u64,
    /// `wl_pointer` events received.
    pub pointer: u64,
    /// `xdg_surface.configure` events received.
    pub configures: u64,
    /// Buffers made.
    pub buffers: u64,
}

impl Counts {
    /// The answer line of a step.
    pub fn line(&self) -> String {
        format!(
            "stepped commits={} callbacks={} pointer={} configures={} buffers={}",
            self.commits, self.callbacks, self.pointer, self.configures, self.buffers
        )
    }

    /// Reads a step's answer line.
    pub fn parse(line: &str) -> Option<Counts> {
        let rest = line.trim().strip_prefix("stepped ")?;
        let mut counts = Counts::default();
        let mut seen = BTreeSet::new();
        for pair in rest.split_whitespace() {
            let (key, value) = pair.split_once('=')?;
            let value: u64 = value.parse().ok()?;
            let slot = match key {
                "commits" => &mut counts.commits,
                "callbacks" => &mut counts.callbacks,
                "pointer" => &mut counts.pointer,
                "configures" => &mut counts.configures,
                "buffers" => &mut counts.buffers,
                _ => return None,
            };
            *slot = value;
            seen.insert(key);
        }
        (seen.len() == 5).then_some(counts)
    }
}

struct PoolBuffer {
    id: u64,
    buffer: wl_buffer::WlBuffer,
    look: Look,
    pixels: (i32, i32),
    busy: bool,
}

struct Window {
    surface: wl_surface::WlSurface,
    xdg_surface: xdg_surface::XdgSurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    viewport: Option<wp_viewport::WpViewport>,
    _fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    /// The size the last configure asked, 0 on an axis it left to the window.
    asked: (i32, i32),
    /// The preferred scale, in 120ths.
    scale_120: u32,
    /// A configure, or a scale changed, that the window has not drawn yet.
    to_draw: bool,
    /// The serial of the last configure, acknowledged when the window draws it: acknowledged as
    /// it comes, it would be sent while the client still reads, in a batch of its own.
    unacknowledged: Option<u32>,
    /// The logical size and buffer size it last drew at.
    drawn: Option<((i32, i32), (i32, i32))>,
    /// What it shows now.
    look: Look,
    buffers: Vec<PoolBuffer>,
    colour: u32,
}

struct Lockstep {
    shm: wl_shm::WlShm,
    windows: Vec<Window>,
    pointer: Option<wl_pointer::WlPointer>,
    counts: Counts,
    next_buffer: u64,
    error: Option<String>,
}

/// Runs the lockstep client with `count` windows, told what to do through the script's socket
/// at `control`, until told to quit, the socket closes or the compositor goes away.
pub fn run(count: usize, control: &Path) -> Result<(), String> {
    let said = |e: std::io::Error| format!("the script's socket: {e}");
    let mut script = UnixStream::connect(control).map_err(said)?;
    let mut lines = BufReader::new(script.try_clone().map_err(said)?).lines();
    let first = lines
        .next()
        .ok_or("the script's socket closed before it named the compositor's")?
        .map_err(said)?;
    let socket = first
        .strip_prefix("wayland ")
        .ok_or_else(|| format!("expected `wayland <socket>`, not {first:?}"))?;
    let stream = UnixStream::connect(socket).map_err(|e| format!("{socket}: {e}"))?;
    let connection = Connection::from_socket(stream).map_err(|e| format!("Wayland: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<Lockstep>(&connection).map_err(|e| format!("Wayland: {e}"))?;
    let qh = queue.handle();
    let bind_error = |name: &'static str| move |e| format!("the compositor offers no {name}: {e}");
    let compositor: wl_compositor::WlCompositor = globals
        .bind(&qh, 1..=6, ())
        .map_err(bind_error("wl_compositor"))?;
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(bind_error("wl_shm"))?;
    let wm_base: xdg_wm_base::XdgWmBase = globals
        .bind(&qh, 1..=6, ())
        .map_err(bind_error("xdg_wm_base"))?;
    // A compositor without a seat gives no pointer events, which the client then never counts.
    let _seat: Option<wl_seat::WlSeat> = globals.bind(&qh, 1..=7, ()).ok();
    let fractional: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1> =
        globals.bind(&qh, 1..=1, ()).ok();
    let viewporter: Option<wp_viewporter::WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    let scaled = fractional.is_some() && viewporter.is_some();

    let mut client = Lockstep {
        shm,
        windows: Vec::with_capacity(count),
        pointer: None,
        counts: Counts::default(),
        next_buffer: 0,
        error: None,
    };
    for n in 0..count {
        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, n);
        let toplevel = xdg_surface.get_toplevel(&qh, n);
        toplevel.set_title(format!("parity {}", n + 1));
        toplevel.set_app_id("yuna-bench-parity".to_owned());
        let (viewport, fractional) = match (&viewporter, &fractional, scaled) {
            (Some(viewporter), Some(fractional), true) => (
                Some(viewporter.get_viewport(&surface, &qh, ())),
                Some(fractional.get_fractional_scale(&surface, &qh, n)),
            ),
            _ => (None, None),
        };
        surface.commit();
        client.windows.push(Window {
            surface,
            xdg_surface,
            _toplevel: toplevel,
            viewport,
            _fractional: fractional,
            asked: (0, 0),
            scale_120: 120,
            to_draw: false,
            unacknowledged: None,
            drawn: None,
            look: Look::Plain,
            buffers: Vec::new(),
            colour: crate::still::colour(n),
        });
    }
    settle(&mut queue, &mut client, &qh)?;
    writeln!(script, "ready {count}").map_err(said)?;
    for line in lines {
        let line = line.map_err(said)?;
        let command =
            parse_command(&line).ok_or_else(|| format!("not a lockstep command: {line:?}"))?;
        let Command::Step(damage) = command else {
            break;
        };
        step(&mut queue, &mut client, &qh, damage)?;
        writeln!(script, "{}", client.counts.line()).map_err(said)?;
    }
    Ok(())
}

/// Answers the compositor until every window has drawn and a round trip brings nothing new to
/// draw: the compositor then waits on nothing of this client's. Each round sends all it has in
/// one go, then waits for the compositor's answer to all of it, so that the compositor reads it in
/// the same rounds in every run.
fn settle(
    queue: &mut EventQueue<Lockstep>,
    client: &mut Lockstep,
    qh: &QueueHandle<Lockstep>,
) -> Result<(), String> {
    loop {
        queue
            .roundtrip(client)
            .map_err(|e| format!("Wayland: {e}"))?;
        client.check()?;
        let drawn = client.windows.iter().all(|window| window.drawn.is_some());
        if drawn && !client.windows.iter().any(|window| window.to_draw) {
            return Ok(());
        }
        client.draw_asked(qh);
        client.check()?;
    }
}

/// One step: reads what came, draws what it asks and what `damage` asks, and makes sure the
/// compositor read it.
fn step(
    queue: &mut EventQueue<Lockstep>,
    client: &mut Lockstep,
    qh: &QueueHandle<Lockstep>,
    damage: Damage,
) -> Result<(), String> {
    read_waiting(queue, client)?;
    let commits_before = client.counts.commits;
    match damage {
        Damage::None => {}
        Damage::Square(n) => {
            if n >= client.windows.len() {
                return Err(format!("no window {}", n + 1));
            }
            let look = match client.windows[n].look {
                Look::Square => Look::Plain,
                _ => Look::Square,
            };
            let (x, y, side) = SQUARE;
            client.show(n, look, Some((x, y, side, side)), qh);
        }
        Damage::Full => {
            for n in 0..client.windows.len() {
                let look = match client.windows[n].look {
                    Look::Other => Look::Plain,
                    _ => Look::Other,
                };
                client.show(n, look, None, qh);
            }
        }
    }
    // The windows the damage did not draw, which a configure or a new scale asked to.
    client.draw_asked(qh);
    client.check()?;
    if client.counts.commits != commits_before {
        queue
            .roundtrip(client)
            .map_err(|e| format!("Wayland: {e}"))?;
        client.check()?;
    }
    Ok(())
}

/// Reads and dispatches what the compositor already sent, without waiting for more.
fn read_waiting(queue: &mut EventQueue<Lockstep>, client: &mut Lockstep) -> Result<(), String> {
    loop {
        queue
            .dispatch_pending(client)
            .map_err(|e| format!("Wayland: {e}"))?;
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        match guard.read() {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return queue
                    .dispatch_pending(client)
                    .map(|_| ())
                    .map_err(|e| format!("Wayland: {e}"));
            }
            Err(error) => return Err(format!("Wayland: {error}")),
        }
    }
}

impl Lockstep {
    fn check(&mut self) -> Result<(), String> {
        self.error.take().map_or(Ok(()), Err)
    }

    /// Draws again, whole, each window a configure or a new scale asked to.
    fn draw_asked(&mut self, qh: &QueueHandle<Lockstep>) {
        for n in 0..self.windows.len() {
            if self.windows[n].to_draw {
                let look = self.windows[n].look;
                self.show(n, look, None, qh);
            }
        }
    }

    /// Window `n`'s logical size and buffer size, from its last configure and scale.
    fn sizes(&self, n: usize) -> ((i32, i32), (i32, i32)) {
        let window = &self.windows[n];
        let pick = |asked: i32, default: i32| if asked > 0 { asked } else { default };
        let logical = (
            pick(window.asked.0, DEFAULT_SIZE.0),
            pick(window.asked.1, DEFAULT_SIZE.1),
        );
        let scale = f64::from(window.scale_120) / 120.0;
        // The fractional scale protocol rounds the buffer's size half away from zero.
        let pixels = if window.viewport.is_some() {
            (
                (f64::from(logical.0) * scale).round() as i32,
                (f64::from(logical.1) * scale).round() as i32,
            )
        } else {
            logical
        };
        (logical, pixels)
    }

    /// Shows `look` in window `n` and commits, with `damage` in buffer pixels, or the whole buffer
    /// when it is none or the window's size changed. Asks a frame callback.
    fn show(
        &mut self,
        n: usize,
        look: Look,
        damage: Option<(i32, i32, i32, i32)>,
        qh: &QueueHandle<Lockstep>,
    ) {
        let (logical, pixels) = self.sizes(n);
        // A configure may change more than the size: the whole window is drawn again.
        let whole = self.windows[n].to_draw || self.windows[n].drawn != Some((logical, pixels));
        let buffer = match self.buffer(n, look, pixels, qh) {
            Ok(buffer) => buffer,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let window = &mut self.windows[n];
        if let Some(serial) = window.unacknowledged.take() {
            window.xdg_surface.ack_configure(serial);
        }
        window.surface.attach(Some(&buffer), 0, 0);
        match damage {
            Some((x, y, width, height)) if !whole => {
                window.surface.damage_buffer(x, y, width, height);
            }
            _ => window.surface.damage_buffer(0, 0, pixels.0, pixels.1),
        }
        if let Some(viewport) = &window.viewport {
            viewport.set_destination(logical.0, logical.1);
        }
        window.surface.frame(qh, ());
        window.surface.commit();
        window.drawn = Some((logical, pixels));
        window.look = look;
        window.to_draw = false;
        self.counts.commits += 1;
    }

    /// A buffer of window `n` showing `look` at `pixels` that the compositor does not hold, made
    /// if none is; marked held.
    fn buffer(
        &mut self,
        n: usize,
        look: Look,
        pixels: (i32, i32),
        qh: &QueueHandle<Lockstep>,
    ) -> Result<wl_buffer::WlBuffer, String> {
        let window = &mut self.windows[n];
        if let Some(free) = window
            .buffers
            .iter_mut()
            .find(|b| !b.busy && b.look == look && b.pixels == pixels)
        {
            free.busy = true;
            return Ok(free.buffer.clone());
        }
        // Buffers of an older size are no use any more: those the compositor released go now.
        window.buffers.retain(|b| {
            let stale = !b.busy && b.pixels != pixels;
            if stale {
                b.buffer.destroy();
            }
            !stale
        });
        if window.buffers.len() >= BUFFERS_A_WINDOW {
            return Err(format!(
                "window {} has {BUFFERS_A_WINDOW} buffers the compositor holds",
                n + 1
            ));
        }
        let id = self.next_buffer;
        self.next_buffer += 1;
        let buffer = make_buffer(&self.shm, pixels, window.colour, look, id, qh)?;
        window.buffers.push(PoolBuffer {
            id,
            buffer: buffer.clone(),
            look,
            pixels,
            busy: true,
        });
        self.counts.buffers += 1;
        Ok(buffer)
    }

    /// The compositor released buffer `id`.
    fn released(&mut self, id: u64) {
        for window in &mut self.windows {
            if let Some(buffer) = window.buffers.iter_mut().find(|b| b.id == id) {
                buffer.busy = false;
                return;
            }
        }
    }
}

/// A buffer of `size` pixels showing `look` in `colour`, in a memfd of its own.
fn make_buffer(
    shm: &wl_shm::WlShm,
    size: (i32, i32),
    colour: u32,
    look: Look,
    id: u64,
    qh: &QueueHandle<Lockstep>,
) -> Result<wl_buffer::WlBuffer, String> {
    let (width, height) = (size.0.max(1), size.1.max(1));
    let stride = width * 4;
    let length = stride
        .checked_mul(height)
        .ok_or_else(|| format!("a window of {width} × {height} is too large"))?;
    let fd = memfd_create("yuna-bench-lockstep", MemfdFlags::CLOEXEC)
        .map_err(|e| format!("memfd: {e}"))?;
    let file = File::from(fd);
    let fill = match look {
        Look::Other => !colour | 0xff00_0000,
        Look::Plain | Look::Square => colour,
    };
    let plain: Vec<u8> = std::iter::repeat_n(fill.to_le_bytes(), width as usize)
        .flatten()
        .collect();
    let (x, y, side) = SQUARE;
    let mut squared = plain.clone();
    for column in x..(x + side).min(width) {
        let at = column as usize * 4;
        squared[at..at + 4].copy_from_slice(&SQUARE_COLOUR.to_le_bytes());
    }
    let mut writer = BufWriter::new(&file);
    for row in 0..height {
        let line = if look == Look::Square && (y..y + side).contains(&row) {
            &squared
        } else {
            &plain
        };
        writer.write_all(line).map_err(|e| format!("memfd: {e}"))?;
    }
    writer.flush().map_err(|e| format!("memfd: {e}"))?;
    drop(writer);
    let pool = shm.create_pool(file.as_fd(), length, qh, ());
    let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Xrgb8888, qh, id);
    pool.destroy();
    Ok(buffer)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Lockstep {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Lockstep {
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

impl Dispatch<xdg_surface::XdgSurface, usize> for Lockstep {
    fn event(
        client: &mut Self,
        _: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        n: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            client.counts.configures += 1;
            client.windows[*n].unacknowledged = Some(serial);
            client.windows[*n].to_draw = true;
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, usize> for Lockstep {
    fn event(
        client: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        n: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            client.windows[*n].asked = (width, height);
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, usize> for Lockstep {
    fn event(
        client: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        n: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let window = &mut client.windows[*n];
            if window.scale_120 != scale {
                window.scale_120 = scale;
                // Drawn again at the next chance; a window not configured yet draws at its
                // configure.
                window.to_draw = window.drawn.is_some();
            }
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, u64> for Lockstep {
    fn event(
        client: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            client.released(*id);
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for Lockstep {
    fn event(
        client: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            client.counts.callbacks += 1;
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Lockstep {
    fn event(
        client: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
            && capabilities.contains(wl_seat::Capability::Pointer)
            && client.pointer.is_none()
        {
            client.pointer = Some(seat.get_pointer(qh, ()));
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Lockstep {
    fn event(
        client: &mut Self,
        _: &wl_pointer::WlPointer,
        _: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        client.counts.pointer += 1;
    }
}

delegate_noop!(Lockstep: ignore wl_compositor::WlCompositor);
delegate_noop!(Lockstep: ignore wl_shm::WlShm);
delegate_noop!(Lockstep: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Lockstep: ignore wl_surface::WlSurface);
delegate_noop!(Lockstep: ignore wp_viewporter::WpViewporter);
delegate_noop!(Lockstep: ignore wp_viewport::WpViewport);
delegate_noop!(Lockstep: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_read_and_bad_ones_refused() {
        assert_eq!(
            parse_command("step none"),
            Some(Command::Step(Damage::None))
        );
        assert_eq!(
            parse_command("step square 2"),
            Some(Command::Step(Damage::Square(1)))
        );
        assert_eq!(
            parse_command("step full"),
            Some(Command::Step(Damage::Full))
        );
        assert_eq!(parse_command("quit"), Some(Command::Quit));
        // Windows count from 1, as their titles do.
        assert_eq!(parse_command("step square 0"), None);
        assert_eq!(parse_command("step square"), None);
        assert_eq!(parse_command("step"), None);
        assert_eq!(parse_command(""), None);
    }

    #[test]
    fn a_steps_answer_reads_back() {
        let counts = Counts {
            commits: 12,
            callbacks: 11,
            pointer: 40,
            configures: 3,
            buffers: 4,
        };
        assert_eq!(Counts::parse(&counts.line()), Some(counts));
        assert_eq!(Counts::parse("stepped commits=1"), None);
        assert_eq!(
            Counts::parse("stepped commits=1 callbacks=1 pointer=1 configures=1 bufers=1"),
            None
        );
        assert_eq!(Counts::parse("ready 2"), None);
    }
}
