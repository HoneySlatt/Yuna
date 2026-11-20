//! The scene's client: two `xdg_toplevel` windows, each drawn into `wl_shm` buffers of its
//! physical size, `XRGB8888`, its viewport set to its logical size as a client that follows
//! `wp_fractional_scale_v1` does, so that Yuna draws one buffer texel to one pixel. Frames go at
//! window 0's frame callbacks: each callback, the next frame is committed. It also captures the
//! output with `ext-image-copy-capture-v1`, at its physical size.
//!
//! Everything the scene is, sizes, texels, the damaged region, comes from `scene_definition.rs`;
//! the client checks that Yuna configures and shows it as defined, and fails otherwise.

use std::ffi::{CStr, c_char, c_int, c_uint};
use std::fs::File;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::path::Path;

use wayland_client::globals::{BindError, GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1, ext_output_image_capture_source_manager_v1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1, ext_image_copy_capture_manager_v1,
    ext_image_copy_capture_session_v1,
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use crate::scene_definition::{
    OUTPUT_HEIGHT, OUTPUT_RATE_HZ, OUTPUT_SCALE_120THS, OUTPUT_WIDTH, REGION, WINDOWS, texel,
};
use crate::scene_report::Damage;

/// Each window's buffers at most. Two alternate while Yuna releases each before the next frame;
/// a third is made only if it keeps one longer.
const MOST_BUFFERS: usize = 3;
const MFD_CLOEXEC: c_uint = 0x1;

// SAFETY: glibc's declaration of memfd_create on x86_64 Linux, the only target the flake builds.
unsafe extern "C" {
    fn memfd_create(name: *const c_char, flags: c_uint) -> c_int;
}

/// A new memory file of `length` bytes, zeroed, as a `wl_shm` client shares its pixels.
fn memory_file(name: &CStr, length: u64) -> Result<File, String> {
    // SAFETY: `name` is a NUL-terminated string that lives for the call.
    let fd = unsafe { memfd_create(name.as_ptr(), MFD_CLOEXEC) };
    if fd < 0 {
        return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: memfd_create returned a new descriptor that nothing else owns.
    let file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    file.set_len(length)
        .map_err(|error| format!("cannot size a memfd: {error}"))?;
    Ok(file)
}

/// What `GlobalList::bind` failing says.
fn bind_error(name: &'static str) -> impl FnOnce(BindError) -> String {
    move |error| format!("Yuna offers no {name}: {error}")
}

/// One window's surface, its roles, and its buffers in one `wl_shm` pool.
struct Window {
    surface: wl_surface::WlSurface,
    _xdg_surface: xdg_surface::XdgSurface,
    _toplevel: xdg_toplevel::XdgToplevel,
    viewport: wp_viewport::WpViewport,
    _fractional: wp_fractional_scale_v1::WpFractionalScaleV1,
    /// The size the last `xdg_toplevel.configure` gave, applied at its `xdg_surface.configure`.
    pending_size: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
    scale_120ths: Option<u32>,
    file: File,
    pool: wl_shm_pool::WlShmPool,
    /// Each buffer and whether Yuna holds it.
    buffers: Vec<(wl_buffer::WlBuffer, bool)>,
}

/// What a capture's session asked, then the frame's answer.
#[derive(Default)]
struct Capture {
    size: Option<(u32, u32)>,
    formats: Vec<wl_shm::Format>,
    constraints_done: bool,
    stopped: bool,
    /// Ready (true) or failed (false), with the failure's reason.
    outcome: Option<Result<(), String>>,
}

/// What the event handlers change.
struct State {
    windows: Vec<Window>,
    output_mode: Option<(i32, i32, i32)>,
    /// Set by a frame callback, cleared when the next frame is committed.
    called_back: bool,
    capture: Capture,
}

/// The client, connected to Yuna.
pub struct Client {
    queue: EventQueue<State>,
    handle: QueueHandle<State>,
    globals: GlobalList,
    state: State,
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    wm_base: xdg_wm_base::XdgWmBase,
    viewporter: wp_viewporter::WpViewporter,
    fractional: wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    output: wl_output::WlOutput,
}

impl Client {
    pub fn connect(socket: &Path) -> Result<Self, String> {
        let stream = UnixStream::connect(socket)
            .map_err(|error| format!("cannot reach {}: {error}", socket.display()))?;
        let connection =
            Connection::from_socket(stream).map_err(|error| format!("connect: {error}"))?;
        let (globals, queue) =
            registry_queue_init::<State>(&connection).map_err(|error| format!("{error}"))?;
        let handle = queue.handle();
        let compositor = globals
            .bind(&handle, 4..=6, ())
            .map_err(bind_error("wl_compositor 4"))?;
        let shm = globals
            .bind(&handle, 1..=1, ())
            .map_err(bind_error("wl_shm"))?;
        let wm_base = globals
            .bind(&handle, 1..=6, ())
            .map_err(bind_error("xdg_wm_base"))?;
        let viewporter = globals
            .bind(&handle, 1..=1, ())
            .map_err(bind_error("wp_viewporter"))?;
        let fractional = globals
            .bind(&handle, 1..=1, ())
            .map_err(bind_error("wp_fractional_scale_manager_v1"))?;
        let output = globals
            .bind(&handle, 1..=4, ())
            .map_err(bind_error("wl_output"))?;
        let mut client = Self {
            queue,
            handle,
            globals,
            state: State {
                windows: Vec::new(),
                output_mode: None,
                called_back: false,
                capture: Capture::default(),
            },
            compositor,
            shm,
            wm_base,
            viewporter,
            fractional,
            output,
        };
        client.roundtrip()?;
        let mode = (
            OUTPUT_WIDTH as i32,
            OUTPUT_HEIGHT as i32,
            (OUTPUT_RATE_HZ * 1000.0) as i32,
        );
        if client.state.output_mode != Some(mode) {
            return Err(format!(
                "Yuna's output is {:?} (width, height, mHz), the scene's {mode:?}",
                client.state.output_mode
            ));
        }
        Ok(client)
    }

    fn roundtrip(&mut self) -> Result<(), String> {
        self.queue
            .roundtrip(&mut self.state)
            .map(|_| ())
            .map_err(|error| format!("Wayland: {error}"))
    }

    fn dispatch(&mut self) -> Result<(), String> {
        self.queue
            .blocking_dispatch(&mut self.state)
            .map(|_| ())
            .map_err(|error| format!("Wayland: {error}"))
    }

    /// Opens the scene's windows one after the other, as a user would, each shown with its
    /// first buffer, frame 0, before the next opens; checks their sizes and scale against the
    /// scene's.
    pub fn open_windows(&mut self) -> Result<(), String> {
        for index in 0..WINDOWS.len() {
            self.open_window(index)?;
            self.attach(index, 0, None)?;
            self.wait_for_callback()?;
        }
        Ok(())
    }

    fn open_window(&mut self, index: usize) -> Result<(), String> {
        let handle = &self.handle;
        let surface = self.compositor.create_surface(handle, ());
        let fractional = self
            .fractional
            .get_fractional_scale(&surface, handle, index);
        let viewport = self.viewporter.get_viewport(&surface, handle, ());
        let xdg_surface = self.wm_base.get_xdg_surface(&surface, handle, index);
        let toplevel = xdg_surface.get_toplevel(handle, index);
        toplevel.set_app_id("llvmpipe-scene".to_owned());
        surface.commit();
        let (width, height) = WINDOWS[index].buffer;
        let stride = width * 4;
        let buffer_bytes = u64::from(stride) * u64::from(height);
        let file = memory_file(c"wl_shm", buffer_bytes * MOST_BUFFERS as u64)?;
        let pool = self.shm.create_pool(
            file.as_fd(),
            i32::try_from(buffer_bytes * MOST_BUFFERS as u64).map_err(|_| "a pool too large")?,
            handle,
            (),
        );
        self.state.windows.push(Window {
            surface,
            _xdg_surface: xdg_surface,
            _toplevel: toplevel,
            viewport,
            _fractional: fractional,
            pending_size: None,
            size: None,
            scale_120ths: None,
            file,
            pool,
            buffers: Vec::new(),
        });
        while self.state.windows[index].size.is_none()
            || self.state.windows[index].scale_120ths.is_none()
        {
            self.dispatch()?;
        }
        let window = &self.state.windows[index];
        let defined = WINDOWS[index];
        let logical = (defined.logical.0 as i32, defined.logical.1 as i32);
        if window.size != Some(logical) || window.scale_120ths != Some(OUTPUT_SCALE_120THS) {
            return Err(format!(
                "window {index} is configured at {:?}, scale {:?}/120; the scene has {logical:?}, \
                 {OUTPUT_SCALE_120THS}/120",
                window.size, window.scale_120ths
            ));
        }
        // Its buffer is its logical size at the scale, to the nearest pixel.
        let at_scale = |logical: u32| {
            (f64::from(logical) * f64::from(OUTPUT_SCALE_120THS) / 120.0).round() as u32
        };
        if (at_scale(defined.logical.0), at_scale(defined.logical.1)) != defined.buffer {
            return Err(format!(
                "window {index}'s buffer, {:?}, is not its logical size at the scale",
                defined.buffer
            ));
        }
        window.viewport.set_destination(logical.0, logical.1);
        Ok(())
    }

    /// A buffer of window `index` that Yuna does not hold, made and filled with frame 0's texels
    /// if every one is held.
    fn free_buffer(&mut self, index: usize) -> Result<usize, String> {
        let window = &mut self.state.windows[index];
        if let Some(free) = window.buffers.iter().position(|(_, held)| !held) {
            return Ok(free);
        }
        let number = window.buffers.len();
        if number == MOST_BUFFERS {
            return Err(format!(
                "Yuna holds all {MOST_BUFFERS} of window {index}'s buffers"
            ));
        }
        let (width, height) = WINDOWS[index].buffer;
        let stride = width * 4;
        let offset = u64::from(stride) * u64::from(height) * number as u64;
        let mut row = vec![0u8; stride as usize];
        for y in 0..height {
            for x in 0..width {
                let at = x as usize * 4;
                row[at..at + 4].copy_from_slice(&texel(index, x, y, 0));
            }
            window
                .file
                .write_all_at(&row, offset + u64::from(y) * u64::from(stride))
                .map_err(|error| format!("cannot fill a buffer: {error}"))?;
        }
        let buffer = window.pool.create_buffer(
            offset as i32,
            width as i32,
            height as i32,
            stride as i32,
            wl_shm::Format::Xrgb8888,
            &self.handle,
            (index, number),
        );
        window.buffers.push((buffer, false));
        Ok(number)
    }

    /// Writes frame `number`'s region into buffer `buffer` of window 0.
    fn write_region(&self, buffer: usize, number: u64) -> Result<(), String> {
        let (width, height) = WINDOWS[0].buffer;
        let stride = u64::from(width) * 4;
        let start = stride * u64::from(height) * buffer as u64;
        let mut row = vec![0u8; REGION.width as usize * 4];
        for y in REGION.y..REGION.y + REGION.height {
            for (column, x) in (REGION.x..REGION.x + REGION.width).enumerate() {
                row[column * 4..column * 4 + 4].copy_from_slice(&texel(0, x, y, number));
            }
            let at = start + u64::from(y) * stride + u64::from(REGION.x) * 4;
            self.state.windows[0]
                .file
                .write_all_at(&row, at)
                .map_err(|error| format!("cannot write the region: {error}"))?;
        }
        Ok(())
    }

    /// Commits a free buffer of window `index`, damaged whole, or in `region` only; window 0's
    /// commit asks a frame callback, as does each window's first.
    fn attach(
        &mut self,
        index: usize,
        number: u64,
        region: Option<crate::scene_definition::Rect>,
    ) -> Result<(), String> {
        let buffer = self.free_buffer(index)?;
        if region.is_some() {
            self.write_region(buffer, number)?;
        }
        let window = &mut self.state.windows[index];
        let (wl_buffer, held) = &mut window.buffers[buffer];
        window.surface.attach(Some(wl_buffer), 0, 0);
        *held = true;
        let (width, height) = WINDOWS[index].buffer;
        match region {
            Some(region) => window.surface.damage_buffer(
                region.x as i32,
                region.y as i32,
                region.width as i32,
                region.height as i32,
            ),
            None => window
                .surface
                .damage_buffer(0, 0, width as i32, height as i32),
        }
        if index == 0 || number == 0 {
            window.surface.frame(&self.handle, ());
        }
        window.surface.commit();
        Ok(())
    }

    fn wait_for_callback(&mut self) -> Result<(), String> {
        self.queue
            .flush()
            .map_err(|error| format!("Wayland: {error}"))?;
        while !self.state.called_back {
            self.dispatch()?;
        }
        self.state.called_back = false;
        Ok(())
    }

    /// Commits frame `number` and waits until Yuna has drawn it: its frame callback.
    pub fn frame(&mut self, number: u64, damage: Damage) -> Result<(), String> {
        match damage {
            Damage::Full => {
                for index in 0..WINDOWS.len() {
                    self.attach(index, number, None)?;
                }
            }
            Damage::Partial => self.attach(0, number, Some(REGION))?,
        }
        self.wait_for_callback()
    }

    /// Each window's buffers made.
    pub fn buffers_made(&self) -> Vec<usize> {
        self.state
            .windows
            .iter()
            .map(|window| window.buffers.len())
            .collect()
    }

    /// The output's newest frame, `B8G8R8A8` rows, as Yuna copies it into a client's `wl_shm`
    /// buffer at the output's physical size.
    pub fn capture(&mut self) -> Result<Vec<u8>, String> {
        let handle = self.handle.clone();
        let sources: ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 =
            self.globals
                .bind(&handle, 1..=1, ())
                .map_err(bind_error("ext_output_image_capture_source_manager_v1"))?;
        let copies: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 = self
            .globals
            .bind(&handle, 1..=1, ())
            .map_err(bind_error("ext_image_copy_capture_manager_v1"))?;
        let source = sources.create_source(&self.output, &handle, ());
        let session = copies.create_session(
            &source,
            ext_image_copy_capture_manager_v1::Options::empty(),
            &handle,
            (),
        );
        self.state.capture = Capture::default();
        while !self.state.capture.constraints_done {
            if self.state.capture.stopped {
                return Err("Yuna stopped the capture's session".to_owned());
            }
            self.dispatch()?;
        }
        let size = self.state.capture.size;
        if size != Some((OUTPUT_WIDTH, OUTPUT_HEIGHT)) {
            return Err(format!("Yuna's capture is {size:?}, not the output's size"));
        }
        let format = [wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888]
            .into_iter()
            .find(|format| self.state.capture.formats.contains(format))
            .ok_or("Yuna's capture takes neither XRGB8888 nor ARGB8888")?;
        let bytes = u64::from(OUTPUT_WIDTH) * u64::from(OUTPUT_HEIGHT) * 4;
        let file = memory_file(c"capture", bytes)?;
        let pool = self
            .shm
            .create_pool(file.as_fd(), bytes as i32, &handle, ());
        let buffer = pool.create_buffer(
            0,
            OUTPUT_WIDTH as i32,
            OUTPUT_HEIGHT as i32,
            OUTPUT_WIDTH as i32 * 4,
            format,
            &handle,
            (usize::MAX, 0),
        );
        let frame = session.create_frame(&handle, ());
        frame.attach_buffer(&buffer);
        frame.damage_buffer(0, 0, OUTPUT_WIDTH as i32, OUTPUT_HEIGHT as i32);
        frame.capture();
        while self.state.capture.outcome.is_none() {
            self.dispatch()?;
        }
        self.state
            .capture
            .outcome
            .take()
            .unwrap_or(Err("no answer".to_owned()))?;
        let mut pixels = vec![0u8; bytes as usize];
        file.read_exact_at(&mut pixels, 0)
            .map_err(|error| format!("cannot read the capture: {error}"))?;
        frame.destroy();
        session.destroy();
        source.destroy();
        buffer.destroy();
        pool.destroy();
        Ok(pixels)
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
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

delegate_noop!(State: wl_compositor::WlCompositor);
delegate_noop!(State: wl_shm_pool::WlShmPool);
delegate_noop!(State: wp_viewporter::WpViewporter);
delegate_noop!(State: wp_viewport::WpViewport);
delegate_noop!(State: wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
delegate_noop!(State: ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(State: ext_image_capture_source_v1::ExtImageCaptureSourceV1);
delegate_noop!(State: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_surface::WlSurface);

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Mode {
            width,
            height,
            refresh,
            ..
        } = event
        {
            state.output_mode = Some((width, height, refresh));
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
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

impl Dispatch<xdg_surface::XdgSurface, usize> for State {
    fn event(
        state: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            if let Some(window) = state.windows.get_mut(*index) {
                window.size = window.pending_size.or(window.size);
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, usize> for State {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event
            && let Some(window) = state.windows.get_mut(*index)
        {
            window.pending_size = Some((width, height));
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event
            && let Some(window) = state.windows.get_mut(*index)
        {
            window.scale_120ths = Some(scale);
        }
    }
}

/// A window's buffer, by its window and its place; the capture's has no window.
impl Dispatch<wl_buffer::WlBuffer, (usize, usize)> for State {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        &(index, number): &(usize, usize),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event
            && let Some((_, held)) = state
                .windows
                .get_mut(index)
                .and_then(|window| window.buffers.get_mut(number))
        {
            *held = false;
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.called_back = true;
        }
    }
}

impl Dispatch<ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event;
        match event {
            Event::BufferSize { width, height } => state.capture.size = Some((width, height)),
            Event::ShmFormat {
                format: WEnum::Value(format),
            } => state.capture.formats.push(format),
            Event::Done => state.capture.constraints_done = true,
            Event::Stopped => state.capture.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event;
        match event {
            Event::Ready => state.capture.outcome = Some(Ok(())),
            Event::Failed { reason } => {
                state.capture.outcome = Some(Err(format!("the capture failed: {reason:?}")))
            }
            _ => {}
        }
    }
}
