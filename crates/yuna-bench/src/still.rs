//! The still client, `yuna-bench client still --windows <n>`: one process opening `n`
//! `xdg_toplevel` windows of `wl_shm`, each a plain colour drawn once.
//!
//! It honours `wp_fractional_scale_v1` and `wp_viewporter` when the compositor offers both, and
//! nothing else: a buffer of the logical size times the preferred scale, shown at the logical
//! size. It asks no frame callback, so that it never makes a compositor draw. A window is drawn
//! again only when the compositor gives it a new size or scale: a tiling compositor resizes every
//! window as each opens, and waits for each to answer. It prints `ready` once every window has
//! shown, then sleeps in the compositor's events until they close.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::fd::AsFd;

use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

/// The size a window takes when the compositor lets it choose, in logical pixels.
const DEFAULT_SIZE: (i32, i32) = (640, 480);

struct Window {
    surface: wl_surface::WlSurface,
    _xdg_surface: xdg_surface::XdgSurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    viewport: Option<wp_viewport::WpViewport>,
    _fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    /// The size the last configure asked, 0 on an axis it left to the window.
    asked: (i32, i32),
    /// The preferred scale, in 120ths.
    scale_120: u32,
    /// The logical size and buffer size it last drew at.
    drawn: Option<((i32, i32), (i32, i32))>,
    configured: bool,
    closed: bool,
    colour: u32,
}

struct Still {
    shm: wl_shm::WlShm,
    windows: Vec<Window>,
    ready: bool,
    error: Option<String>,
}

/// Runs the still client with `count` windows until the compositor closes them or goes away.
pub fn run(count: usize) -> Result<(), String> {
    let connection = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<Still>(&connection).map_err(|e| format!("Wayland: {e}"))?;
    let qh = queue.handle();
    let bind_error = |name: &'static str| move |e| format!("the compositor offers no {name}: {e}");
    let compositor: wl_compositor::WlCompositor = globals
        .bind(&qh, 1..=6, ())
        .map_err(bind_error("wl_compositor"))?;
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(bind_error("wl_shm"))?;
    let wm_base: xdg_wm_base::XdgWmBase = globals
        .bind(&qh, 1..=6, ())
        .map_err(bind_error("xdg_wm_base"))?;
    let fractional: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1> =
        globals.bind(&qh, 1..=1, ()).ok();
    let viewporter: Option<wp_viewporter::WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    // The scale is honoured only with both: a buffer of another size needs a viewport.
    let scaled = fractional.is_some() && viewporter.is_some();

    let mut still = Still {
        shm,
        windows: Vec::with_capacity(count),
        ready: false,
        error: None,
    };
    for n in 0..count {
        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, n);
        let toplevel = xdg_surface.get_toplevel(&qh, n);
        toplevel.set_title(format!("yuna-bench still {}", n + 1));
        toplevel.set_app_id("yuna-bench-still".to_owned());
        let (viewport, fractional) = match (&viewporter, &fractional, scaled) {
            (Some(viewporter), Some(fractional), true) => (
                Some(viewporter.get_viewport(&surface, &qh, ())),
                Some(fractional.get_fractional_scale(&surface, &qh, n)),
            ),
            _ => (None, None),
        };
        surface.commit();
        still.windows.push(Window {
            surface,
            _xdg_surface: xdg_surface,
            _toplevel: toplevel,
            viewport,
            _fractional: fractional,
            asked: (0, 0),
            scale_120: 120,
            drawn: None,
            configured: false,
            closed: false,
            colour: colour(n),
        });
    }
    loop {
        queue
            .blocking_dispatch(&mut still)
            .map_err(|e| format!("Wayland: {e}"))?;
        if let Some(error) = still.error.take() {
            return Err(error);
        }
        if !still.ready && still.windows.iter().all(|w| w.drawn.is_some()) {
            still.ready = true;
            println!("ready {count}");
            let _ = std::io::stdout().flush();
        }
        if still.windows.iter().all(|window| window.closed) {
            return Ok(());
        }
    }
}

/// A plain colour of its own for window `n`, opaque.
fn colour(n: usize) -> u32 {
    let n = n as u32;
    0xff00_0000 | ((n * 0x3d) & 0xff) << 16 | ((n * 0x7b + 0x40) & 0xff) << 8 | 0x90
}

impl Still {
    /// Draws window `n` at its asked size and scale if either changed, and commits.
    fn draw(&mut self, n: usize, qh: &QueueHandle<Still>) {
        let window = &self.windows[n];
        let logical = (
            if window.asked.0 > 0 {
                window.asked.0
            } else {
                DEFAULT_SIZE.0
            },
            if window.asked.1 > 0 {
                window.asked.1
            } else {
                DEFAULT_SIZE.1
            },
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
        if window.drawn != Some((logical, pixels)) {
            match buffer(&self.shm, pixels, window.colour, qh) {
                Ok(buffer) => {
                    let window = &mut self.windows[n];
                    window.surface.attach(Some(&buffer), 0, 0);
                    window.surface.damage_buffer(0, 0, pixels.0, pixels.1);
                    if let Some(viewport) = &window.viewport {
                        viewport.set_destination(logical.0, logical.1);
                    }
                    window.drawn = Some((logical, pixels));
                }
                Err(error) => self.error = Some(error),
            }
        }
        self.windows[n].surface.commit();
    }
}

/// A buffer of `size` pixels, all `colour`, in a memfd of its own; destroyed when released.
fn buffer(
    shm: &wl_shm::WlShm,
    size: (i32, i32),
    colour: u32,
    qh: &QueueHandle<Still>,
) -> Result<wl_buffer::WlBuffer, String> {
    let (width, height) = (size.0.max(1), size.1.max(1));
    let stride = width * 4;
    let length = stride
        .checked_mul(height)
        .ok_or_else(|| format!("a window of {width} × {height} is too large"))?;
    let fd =
        memfd_create("yuna-bench-still", MemfdFlags::CLOEXEC).map_err(|e| format!("memfd: {e}"))?;
    let file = File::from(fd);
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
    let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Xrgb8888, qh, ());
    pool.destroy();
    Ok(buffer)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Still {
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

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Still {
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

impl Dispatch<xdg_surface::XdgSurface, usize> for Still {
    fn event(
        still: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        n: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            still.windows[*n].configured = true;
            still.draw(*n, qh);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, usize> for Still {
    fn event(
        still: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        n: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                still.windows[*n].asked = (width, height);
            }
            xdg_toplevel::Event::Close => still.windows[*n].closed = true,
            _ => {}
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, usize> for Still {
    fn event(
        still: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        n: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let window = &mut still.windows[*n];
            if window.scale_120 != scale {
                window.scale_120 = scale;
                if window.configured {
                    still.draw(*n, qh);
                }
            }
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for Still {
    fn event(
        _: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Released: the compositor holds no reference to it any more. A window's buffer stays
        // attached until the window draws again, so this only frees replaced ones.
        if let wl_buffer::Event::Release = event {
            buffer.destroy();
        }
    }
}

delegate_noop!(Still: ignore wl_compositor::WlCompositor);
delegate_noop!(Still: ignore wl_shm::WlShm);
delegate_noop!(Still: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Still: ignore wl_surface::WlSurface);
delegate_noop!(Still: ignore wp_viewporter::WpViewporter);
delegate_noop!(Still: ignore wp_viewport::WpViewport);
delegate_noop!(Still: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_window_has_an_opaque_colour_of_its_own() {
        let colours: std::collections::BTreeSet<u32> = (0..150).map(colour).collect();
        assert!(colours.len() > 100, "{} colours", colours.len());
        assert!(colours.iter().all(|c| c >> 24 == 0xff));
    }
}
