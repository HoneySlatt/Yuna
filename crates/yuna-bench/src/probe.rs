//! The probe, `yuna-bench client probe`: one fullscreen window whose own frames say when the
//! compositor presented them, through `wp_presentation`. A client cannot read another client's
//! presentation, so the bench measures with frames of its own: the latency round's, and the real
//! refresh a compositor presents at.
//!
//! - `--keys`: it draws a frame on each key it is given, and nothing else. The tour's keyboard
//!   types the keys into the same compositor (`keys`); the time from a key's sending to its
//!   frame's presentation is input to present, end to end
//!   ([ADR-0032](../../../docs/adr/0032-input-to-present-read-by-mode.md)).
//! - `--frames <n>`: it draws `n` frames, each at the last one's frame callback, as a game at the
//!   refresh rate; their presentations give the refresh.
//!
//! It draws with `wl_shm`: a small buffer the compositor scales to the window where it offers
//! `wp_viewporter`, so that a frame costs the compositor little to take; a buffer of the window's
//! size otherwise. Frames alternate between two close dark greys (`COLOURS`), so that each one
//! changes what shows without flashing the screen.
//!
//! It prints, flushed at once, `clock <id>` (the presentation's clock, which must be
//! CLOCK_MONOTONIC, 1, for the bench to compare), `focus` when the keyboard enters it, `key <n>
//! <ns>` when it is given its `n`-th key, `presented <n> <ns> <refresh_ns> <seq> <flags>` and
//! `discarded <n>` for its frame `n`, and `ready` once its first frame was presented. Frame 0 is
//! its first; frame `n` is the one key `n` drew, or the `n`-th after the first. Times are on
//! CLOCK_MONOTONIC, in nanoseconds.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::fd::AsFd;

use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use crate::clock::monotonic_ns;

/// The size of a buffer the compositor scales to the window, in pixels.
const SMALL_BUFFER: (i32, i32) = (64, 64);
/// The window's size when the compositor lets it choose, in logical pixels.
const DEFAULT_SIZE: (i32, i32) = (640, 480);
/// The most buffers it keeps: a compositor that holds them all longer than that is waited for.
const MOST_BUFFERS: usize = 6;
/// The colours it alternates between: two dark greys close enough that a frame a key never
/// flashes the whole screen, up to 16 times a second in the latency round.
const COLOURS: [u32; 2] = [0xff20_2024, 0xff28_282c];

/// What the probe is told to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A frame on each key.
    Keys,
    /// This many frames after its first, each at the last one's frame callback.
    Frames(u32),
}

/// One `presented` line of the probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Presented {
    /// The frame.
    pub frame: u64,
    /// When it was presented, on the presentation's clock, in ns.
    pub at_ns: u64,
    /// The output's refresh then, in ns; 0 when the compositor does not know it.
    pub refresh_ns: u64,
    /// The output's refresh counter then.
    pub seq: u64,
    /// `wp_presentation_feedback`'s flags: vsync 1, hardware clock 2, hardware completion 4,
    /// zero copy 8.
    pub flags: u32,
}

impl Presented {
    /// The line the probe prints.
    pub fn line(&self) -> String {
        format!(
            "presented {} {} {} {} {}",
            self.frame, self.at_ns, self.refresh_ns, self.seq, self.flags
        )
    }

    /// Reads a line the probe printed; none if it is no `presented` line.
    pub fn read(line: &str) -> Option<Presented> {
        let mut words = line.strip_prefix("presented ")?.split_whitespace();
        let mut next = || words.next()?.parse::<u64>().ok();
        Some(Presented {
            frame: next()?,
            at_ns: next()?,
            refresh_ns: next()?,
            seq: next()?,
            flags: u32::try_from(next()?).ok()?,
        })
    }
}

/// A buffer and the colour it holds.
struct Buffer {
    buffer: wl_buffer::WlBuffer,
    colour: usize,
    size: (i32, i32),
    busy: bool,
}

struct Probe {
    mode: Mode,
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    presentation: wp_presentation::WpPresentation,
    viewport: Option<wp_viewport::WpViewport>,
    /// Its one keyboard: a compositor tells the seat's capabilities again as devices come, KWin
    /// as its fake input's keyboard does, and each keyboard taken would get every key.
    keyboard: Option<wl_keyboard::WlKeyboard>,
    buffers: Vec<Buffer>,
    /// The window's logical size, once configured.
    size: Option<(i32, i32)>,
    /// The colour shown last.
    shown: usize,
    /// The next frame's number.
    next_frame: u64,
    keys: u64,
    /// Frames whose presentation is not known yet.
    pending: u64,
    ready: bool,
    closed: bool,
    out: BufWriter<std::io::Stdout>,
    error: Option<String>,
}

/// Runs the probe until the compositor closes its window, or, with [`Mode::Frames`], its frames
/// are all presented or discarded.
pub fn run(mode: Mode) -> Result<(), String> {
    let connection = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<Probe>(&connection).map_err(|e| format!("Wayland: {e}"))?;
    let qh = queue.handle();
    let missing = |name: &'static str| move |e| format!("the compositor offers no {name}: {e}");
    let compositor: wl_compositor::WlCompositor = globals
        .bind(&qh, 1..=4, ())
        .map_err(missing("wl_compositor"))?;
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(missing("wl_shm"))?;
    let wm_base: xdg_wm_base::XdgWmBase = globals
        .bind(&qh, 1..=2, ())
        .map_err(missing("xdg_wm_base"))?;
    let presentation: wp_presentation::WpPresentation = globals
        .bind(&qh, 1..=1, ())
        .map_err(missing("wp_presentation"))?;
    let viewporter: Option<wp_viewporter::WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    // Keys matter in `--keys` alone; a compositor without a seat cannot give them.
    let seat: Option<wl_seat::WlSeat> = globals.bind(&qh, 1..=7, ()).ok();
    if mode == Mode::Keys && seat.is_none() {
        return Err("the compositor offers no wl_seat: no key can reach the probe".to_owned());
    }
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_title("yuna-bench probe".to_owned());
    toplevel.set_app_id("yuna-bench-probe".to_owned());
    toplevel.set_fullscreen(None);
    let viewport = viewporter
        .as_ref()
        .map(|viewporter| viewporter.get_viewport(&surface, &qh, ()));
    surface.commit();
    let mut probe = Probe {
        mode,
        shm,
        surface,
        presentation,
        viewport,
        keyboard: None,
        buffers: Vec::new(),
        size: None,
        shown: 0,
        next_frame: 0,
        keys: 0,
        pending: 0,
        ready: false,
        closed: false,
        out: BufWriter::new(std::io::stdout()),
        error: None,
    };
    loop {
        queue
            .blocking_dispatch(&mut probe)
            .map_err(|e| format!("Wayland: {e}"))?;
        if let Some(error) = probe.error.take() {
            return Err(error);
        }
        if probe.closed {
            return Ok(());
        }
        if let Mode::Frames(frames) = probe.mode
            && probe.next_frame > u64::from(frames)
            && probe.pending == 0
        {
            return Ok(());
        }
    }
}

impl Probe {
    fn say(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
    }

    /// The size its buffers take: small where the compositor scales them, else the window's.
    fn buffer_size(&self) -> (i32, i32) {
        if self.viewport.is_some() {
            SMALL_BUFFER
        } else {
            self.size.unwrap_or(DEFAULT_SIZE)
        }
    }

    /// Draws the next frame, of the other colour, asks its presentation, and commits; with a
    /// frame callback in `--frames`.
    fn draw(&mut self, qh: &QueueHandle<Probe>) {
        let Some(size) = self.size else {
            return;
        };
        let colour = 1 - self.shown;
        let buffer_size = self.buffer_size();
        let free = self.buffers.iter().position(|buffer| {
            !buffer.busy && buffer.colour == colour && buffer.size == buffer_size
        });
        let index = match free {
            Some(index) => index,
            None if self.buffers.len() < MOST_BUFFERS => {
                match buffer(&self.shm, buffer_size, COLOURS[colour], qh) {
                    Ok(made) => {
                        self.buffers.push(Buffer {
                            buffer: made,
                            colour,
                            size: buffer_size,
                            busy: false,
                        });
                        self.buffers.len() - 1
                    }
                    Err(error) => {
                        self.error = Some(error);
                        return;
                    }
                }
            }
            None => {
                self.error = Some(format!(
                    "the compositor holds all {MOST_BUFFERS} of the probe's buffers"
                ));
                return;
            }
        };
        let frame = self.next_frame;
        self.next_frame += 1;
        let chosen = &mut self.buffers[index];
        chosen.busy = true;
        self.surface.attach(Some(&chosen.buffer), 0, 0);
        self.surface
            .damage_buffer(0, 0, buffer_size.0, buffer_size.1);
        if let Some(viewport) = &self.viewport {
            viewport.set_destination(size.0, size.1);
        }
        self.presentation.feedback(&self.surface, qh, frame);
        if matches!(self.mode, Mode::Frames(_)) {
            self.surface.frame(qh, ());
        }
        self.pending += 1;
        self.shown = colour;
        self.surface.commit();
    }
}

/// A buffer of `size` pixels, all `colour`, in a memfd of its own.
fn buffer(
    shm: &wl_shm::WlShm,
    size: (i32, i32),
    colour: u32,
    qh: &QueueHandle<Probe>,
) -> Result<wl_buffer::WlBuffer, String> {
    let (width, height) = (size.0.max(1), size.1.max(1));
    let stride = width.checked_mul(4).ok_or("a window too wide")?;
    let length = stride
        .checked_mul(height)
        .ok_or_else(|| format!("a window of {width} × {height} is too large"))?;
    let file = File::from(
        memfd_create("yuna-bench-probe", MemfdFlags::CLOEXEC).map_err(|e| format!("memfd: {e}"))?,
    );
    let row: Vec<u8> = std::iter::repeat_n(colour.to_le_bytes(), width as usize)
        .flatten()
        .collect();
    let mut writer = BufWriter::new(&file);
    for _ in 0..height {
        writer.write_all(&row).map_err(|e| format!("memfd: {e}"))?;
    }
    writer.flush().map_err(|e| format!("memfd: {e}"))?;
    drop(writer);
    let pool = shm.create_pool(file.as_fd(), length, qh, ());
    let made = pool.create_buffer(0, width, height, stride, wl_shm::Format::Xrgb8888, qh, ());
    pool.destroy();
    Ok(made)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Probe {
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

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Probe {
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

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Probe {
    fn event(
        probe: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                let size = (
                    if width > 0 { width } else { DEFAULT_SIZE.0 },
                    if height > 0 { height } else { DEFAULT_SIZE.1 },
                );
                probe.size = Some(size);
            }
            xdg_toplevel::Event::Close => probe.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Probe {
    fn event(
        probe: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            // The first frame, once: later ones come from keys or frame callbacks. A new size
            // reaches the next frame through the viewport, or its buffers.
            if probe.next_frame == 0 {
                probe.draw(qh);
            }
        }
    }
}

impl Dispatch<wp_presentation::WpPresentation, ()> for Probe {
    fn event(
        probe: &mut Self,
        _: &wp_presentation::WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            probe.say(&format!("clock {clk_id}"));
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, u64> for Probe {
    fn event(
        probe: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        frame: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                let presented = Presented {
                    frame: *frame,
                    at_ns: seconds * 1_000_000_000 + u64::from(tv_nsec),
                    refresh_ns: u64::from(refresh),
                    seq: (u64::from(seq_hi) << 32) | u64::from(seq_lo),
                    flags: match flags {
                        WEnum::Value(flags) => flags.bits(),
                        WEnum::Unknown(bits) => bits,
                    },
                };
                probe.pending = probe.pending.saturating_sub(1);
                probe.say(&presented.line());
                if !probe.ready {
                    probe.ready = true;
                    probe.say("ready");
                }
            }
            wp_presentation_feedback::Event::Discarded => {
                probe.pending = probe.pending.saturating_sub(1);
                probe.say(&format!("discarded {frame}"));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for Probe {
    fn event(
        probe: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let (wl_callback::Event::Done { .. }, Mode::Frames(frames)) = (event, probe.mode)
            && probe.next_frame <= u64::from(frames)
        {
            probe.draw(qh);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Probe {
    fn event(
        probe: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
            && capabilities.contains(wl_seat::Capability::Keyboard)
            && probe.keyboard.is_none()
        {
            probe.keyboard = Some(seat.get_keyboard(qh, ()));
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Probe {
    fn event(
        probe: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter { .. } => probe.say("focus"),
            wl_keyboard::Event::Key {
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } if probe.mode == Mode::Keys => {
                let at = monotonic_ns();
                probe.keys += 1;
                let line = format!("key {} {at}", probe.keys);
                probe.say(&line);
                // Frame n answers key n: the first frame was frame 0.
                probe.next_frame = probe.keys;
                probe.draw(qh);
            }
            // The keymap's file is closed as its descriptor drops.
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for Probe {
    fn event(
        probe: &mut Self,
        released: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event
            && let Some(buffer) = probe
                .buffers
                .iter_mut()
                .find(|buffer| buffer.buffer == *released)
        {
            buffer.busy = false;
        }
    }
}

delegate_noop!(Probe: ignore wl_compositor::WlCompositor);
delegate_noop!(Probe: ignore wl_shm::WlShm);
delegate_noop!(Probe: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Probe: ignore wl_surface::WlSurface);
delegate_noop!(Probe: ignore wp_viewporter::WpViewporter);
delegate_noop!(Probe: ignore wp_viewport::WpViewport);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_presented_line_is_read_back_as_written() {
        let presented = Presented {
            frame: 12,
            at_ns: 5_000_000_123,
            refresh_ns: 4_166_666,
            seq: 77,
            flags: 7,
        };
        assert_eq!(Presented::read(&presented.line()), Some(presented));
        assert_eq!(Presented::read("discarded 3"), None);
        assert_eq!(Presented::read("presented 1 2"), None);
        assert_eq!(Presented::read("presented a 2 3 4 5"), None);
    }
}
