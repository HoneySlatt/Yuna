//! The frame counter, `yuna-bench client frames`: counts a compositor's frames from outside
//! through `ext-image-copy-capture-v1`, on its first output.
//!
//! After its first frame, it asks each next one only when the output's content changed: the
//! protocol lets the compositor wait for new content before it copies, and wlroots does. Each
//! frame the compositor copies is one it drew with damage; at rest, none is asked. A frame callback
//! would make a compositor draw (ADR-0075); this does not, but each copy costs the compositor, so
//! the bench counts frames this way in a pass apart from the one whose costs it reads.
//!
//! It prints `session <width>x<height>` once the compositor gave the buffer's constraints, then a
//! line `frame <ns>` for each frame copied, `<ns>` the CLOCK_MONOTONIC time it was told, flushed
//! at once.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::fd::AsFd;

use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1, ext_output_image_capture_source_manager_v1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1, ext_image_copy_capture_manager_v1,
    ext_image_copy_capture_session_v1,
};

use crate::clock::monotonic_ns;

struct Counter {
    shm: wl_shm::WlShm,
    session: ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
    size: (u32, u32),
    formats: Vec<wl_shm::Format>,
    buffer: Option<(wl_buffer::WlBuffer, (u32, u32))>,
    /// Whether the buffer was captured once: after that, it never changes on this side.
    captured_once: bool,
    stopped: bool,
    error: Option<String>,
}

/// Runs the counter until the compositor stops its session or goes away.
pub fn run() -> Result<(), String> {
    let connection = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<Counter>(&connection).map_err(|e| format!("Wayland: {e}"))?;
    let qh = queue.handle();
    let missing = |name: &'static str| move |e| format!("the compositor offers no {name}: {e}");
    let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).map_err(missing("wl_output"))?;
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(missing("wl_shm"))?;
    let sources: ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 =
        globals
            .bind(&qh, 1..=1, ())
            .map_err(missing("ext_output_image_capture_source_manager_v1"))?;
    let manager: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(missing("ext_image_copy_capture_manager_v1"))?;
    let source = sources.create_source(&output, &qh, ());
    let session = manager.create_session(
        &source,
        ext_image_copy_capture_manager_v1::Options::empty(),
        &qh,
        (),
    );
    let mut counter = Counter {
        shm,
        session,
        size: (0, 0),
        formats: Vec::new(),
        buffer: None,
        captured_once: false,
        stopped: false,
        error: None,
    };
    while !counter.stopped {
        queue
            .blocking_dispatch(&mut counter)
            .map_err(|e| format!("Wayland: {e}"))?;
        if let Some(error) = counter.error.take() {
            return Err(error);
        }
    }
    Ok(())
}

impl Counter {
    /// Asks the next frame, into a buffer that meets the session's constraints.
    fn capture(&mut self, qh: &QueueHandle<Counter>) {
        let fits = self
            .buffer
            .as_ref()
            .is_some_and(|(_, size)| *size == self.size);
        if !fits {
            if let Some((old, _)) = self.buffer.take() {
                old.destroy();
            }
            let format = [wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888]
                .into_iter()
                .find(|format| self.formats.contains(format))
                .or_else(|| self.formats.first().copied());
            let Some(format) = format else {
                self.error = Some("the session offers no shm format".to_owned());
                return;
            };
            match buffer(&self.shm, self.size, format, qh) {
                Ok(buffer) => self.buffer = Some((buffer, self.size)),
                Err(error) => {
                    self.error = Some(error);
                    return;
                }
            }
            self.captured_once = false;
        }
        let Some((buffer, (width, height))) = &self.buffer else {
            return;
        };
        let frame = self.session.create_frame(qh, ());
        frame.attach_buffer(buffer);
        if !self.captured_once {
            frame.damage_buffer(0, 0, *width as i32, *height as i32);
        }
        frame.capture();
    }
}

/// A shm buffer of `size` in `format`, in a memfd of its own.
fn buffer(
    shm: &wl_shm::WlShm,
    size: (u32, u32),
    format: wl_shm::Format,
    qh: &QueueHandle<Counter>,
) -> Result<wl_buffer::WlBuffer, String> {
    let (width, height) = (size.0 as i32, size.1 as i32);
    let stride = width.checked_mul(4).ok_or("an output too wide")?;
    let length = stride.checked_mul(height).ok_or("an output too large")?;
    let fd = memfd_create("yuna-bench-frames", MemfdFlags::CLOEXEC)
        .map_err(|e| format!("memfd: {e}"))?;
    let file = File::from(fd);
    file.set_len(length as u64)
        .map_err(|e| format!("memfd: {e}"))?;
    let pool = shm.create_pool(file.as_fd(), length, qh, ());
    let buffer = pool.create_buffer(0, width, height, stride, format, qh, ());
    pool.destroy();
    Ok(buffer)
}

impl Dispatch<ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1, ()> for Counter {
    fn event(
        counter: &mut Self,
        _: &ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event;
        match event {
            Event::BufferSize { width, height } => counter.size = (width, height),
            Event::ShmFormat {
                format: WEnum::Value(format),
            } => counter.formats.push(format),
            Event::Done => {
                // No buffer yet, or the last one failed the constraints these replace.
                if counter.buffer.is_none() {
                    let mut out = BufWriter::new(std::io::stdout());
                    let _ = writeln!(out, "session {}x{}", counter.size.0, counter.size.1);
                    let _ = out.flush();
                    counter.capture(qh);
                }
            }
            Event::Stopped => counter.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1, ()> for Counter {
    fn event(
        counter: &mut Self,
        frame: &ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::{Event, FailureReason};
        match event {
            Event::Ready => {
                let mut out = std::io::stdout().lock();
                let _ = writeln!(out, "frame {}", monotonic_ns());
                let _ = out.flush();
                frame.destroy();
                counter.captured_once = true;
                counter.capture(qh);
            }
            Event::Failed { reason } => {
                frame.destroy();
                match reason {
                    WEnum::Value(FailureReason::Stopped) => counter.stopped = true,
                    // New constraints come with the session's next `done`; a new buffer then.
                    WEnum::Value(FailureReason::BufferConstraints) => {
                        if let Some((old, _)) = counter.buffer.take() {
                            old.destroy();
                        }
                    }
                    _ => counter.capture(qh),
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Counter {
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

delegate_noop!(Counter: ignore wl_output::WlOutput);
delegate_noop!(Counter: ignore wl_shm::WlShm);
delegate_noop!(Counter: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Counter: ignore wl_buffer::WlBuffer);
delegate_noop!(Counter: ignore ext_image_capture_source_v1::ExtImageCaptureSourceV1);
delegate_noop!(Counter: ignore ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(Counter: ignore ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1);
