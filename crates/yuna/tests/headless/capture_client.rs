//! A client that captures outputs through `ext-image-copy-capture-v1`, for tests: it binds every
//! `wl_output` by name, makes a source of one and a session on it, and captures frames into
//! `wl_shm` buffers it reads back, recording every event the protocol sends.
//!
//! A [`CaptureClient`] is a test [`Client`] with these objects on an event queue of their own,
//! over the same connection, as `input_client.rs` does.

use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::time::Duration;

use smithay::reexports::rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1, ext_output_image_capture_source_manager_v1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1, ext_image_copy_capture_manager_v1,
    ext_image_copy_capture_session_v1,
};
use yuna::Compositor;
use yuna::yuna_layout::SeatId;

use crate::client::Client;

pub use ext_image_copy_capture_frame_v1::FailureReason;
pub use wl_shm::Format;

/// What a session was told.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SessionEvents {
    pub buffer_size: Option<(u32, u32)>,
    pub formats: Vec<Format>,
    pub done: u32,
    pub stopped: bool,
}

/// What a frame was told.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FrameEvents {
    pub damage: Vec<(i32, i32, i32, i32)>,
    pub presented: Option<Duration>,
    pub ready: bool,
    pub failed: Option<FailureReason>,
}

/// A frame asked, its buffer's memory beside it.
struct CaptureFrame {
    frame: ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
    file: File,
    stride: u32,
    offset: u32,
    events: FrameEvents,
}

/// A test client that captures outputs.
pub struct CaptureClient {
    /// Its windows' side.
    pub client: Client,
    queue: EventQueue<CaptureState>,
    pub state: CaptureState,
}

/// What the client's capture objects hold and received.
#[derive(Default)]
pub struct CaptureState {
    shm: Option<wl_shm::WlShm>,
    sources:
        Option<ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1>,
    manager: Option<ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1>,
    /// Every `wl_output`, with the name it gave.
    outputs: Vec<(wl_output::WlOutput, Option<String>)>,
    sessions: Vec<(
        ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
        SessionEvents,
    )>,
    frames: Vec<CaptureFrame>,
}

impl CaptureClient {
    /// Connects a client of `seat` and binds the capture globals; panics when one is missing.
    pub fn connect(compositor: &mut Compositor, seat: SeatId) -> CaptureClient {
        let client = Client::connect_to(compositor, seat);
        let queue = client.connection().new_event_queue();
        client
            .connection()
            .display()
            .get_registry(&queue.handle(), ());
        let mut capture = CaptureClient {
            client,
            queue,
            state: CaptureState::default(),
        };
        // The globals, then the outputs' names.
        capture.roundtrip(compositor);
        capture.roundtrip(compositor);
        assert!(
            capture.state.shm.is_some()
                && capture.state.sources.is_some()
                && capture.state.manager.is_some(),
            "a capture global is missing"
        );
        capture
    }

    /// A roundtrip of the client, then the capture events it read.
    pub fn roundtrip(&mut self, compositor: &mut Compositor) {
        self.client.roundtrip(compositor);
        self.queue
            .dispatch_pending(&mut self.state)
            .expect("the capture events are read");
    }

    /// Makes a source of the output named `name` (`HEADLESS-<id>`) and a session on it;
    /// returns the session's index.
    pub fn start_session(&mut self, name: &str) -> usize {
        let qh = self.queue.handle();
        let state = &mut self.state;
        let output = state
            .outputs
            .iter()
            .find(|(_, given)| given.as_deref() == Some(name))
            .map(|(output, _)| output.clone())
            .unwrap_or_else(|| panic!("no wl_output named {name}"));
        let source = state
            .sources
            .as_ref()
            .expect("the source manager")
            .create_source(&output, &qh, ());
        let index = state.sessions.len();
        let session = state
            .manager
            .as_ref()
            .expect("the capture manager")
            .create_session(
                &source,
                ext_image_copy_capture_manager_v1::Options::empty(),
                &qh,
                index,
            );
        state.sessions.push((session, SessionEvents::default()));
        index
    }

    pub fn session(&self, index: usize) -> &SessionEvents {
        &self.state.sessions[index].1
    }

    /// Asks a frame of session `session` into a new buffer of `width` × `height` pixels of
    /// `format`, damaged whole; returns the frame's index.
    pub fn capture(&mut self, session: usize, format: Format, width: u32, height: u32) -> usize {
        self.capture_into(session, format, (width, height), width * 4, 0)
    }

    /// Asks a frame of session `session` into a new buffer of `width` × `height` pixels of
    /// `format`, `stride` bytes a row, starting `offset` bytes into its pool; returns the
    /// frame's index.
    pub fn capture_into(
        &mut self,
        session: usize,
        format: Format,
        (width, height): (u32, u32),
        stride: u32,
        offset: u32,
    ) -> usize {
        let qh = self.queue.handle();
        let state = &mut self.state;
        let size = offset + stride * height;
        let fd = memfd_create("yuna-test-capture", MemfdFlags::CLOEXEC).expect("a memfd");
        let file = File::from(fd);
        file.set_len(u64::from(size))
            .expect("the memfd takes its size");
        let shm = state.shm.as_ref().expect("wl_shm");
        let pool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
        let buffer = pool.create_buffer(
            offset as i32,
            width as i32,
            height as i32,
            stride as i32,
            format,
            &qh,
            (),
        );
        pool.destroy();
        let index = state.frames.len();
        let frame = state.sessions[session].0.create_frame(&qh, index);
        frame.attach_buffer(&buffer);
        frame.damage_buffer(0, 0, width as i32, height as i32);
        frame.capture();
        state.frames.push(CaptureFrame {
            frame,
            file,
            stride,
            offset,
            events: FrameEvents::default(),
        });
        index
    }

    pub fn frame(&self, index: usize) -> &FrameEvents {
        &self.state.frames[index].events
    }

    /// Destroys frame `index`, as a client does once it is ready, before it asks the next.
    pub fn destroy_frame(&mut self, index: usize) {
        self.state.frames[index].frame.destroy();
    }

    /// The four bytes of pixel (`x`, `y`) of frame `index`'s buffer, in memory order.
    pub fn pixel(&self, index: usize, x: u32, y: u32) -> [u8; 4] {
        let frame = &self.state.frames[index];
        let mut bytes = [0; 4];
        frame
            .file
            .read_exact_at(
                &mut bytes,
                u64::from(frame.offset + y * frame.stride + x * 4),
            )
            .expect("the buffer is read");
        bytes
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for CaptureState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &wayland_client::Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
            "wl_output" => {
                let index = state.outputs.len();
                state
                    .outputs
                    .push((registry.bind(name, version.min(4), qh, index), None));
            }
            "ext_output_image_capture_source_manager_v1" => {
                state.sources = Some(registry.bind(name, 1, qh, ()))
            }
            "ext_image_copy_capture_manager_v1" => {
                state.manager = Some(registry.bind(name, 1, qh, ()))
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for CaptureState {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.outputs[*index].1 = Some(name);
        }
    }
}

impl Dispatch<ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1, usize>
    for CaptureState
{
    fn event(
        state: &mut Self,
        _: &ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        index: &usize,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event;
        let events = &mut state.sessions[*index].1;
        match event {
            Event::BufferSize { width, height } => events.buffer_size = Some((width, height)),
            Event::ShmFormat {
                format: WEnum::Value(format),
            } => events.formats.push(format),
            Event::Done => events.done += 1,
            Event::Stopped => events.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1, usize> for CaptureState {
    fn event(
        state: &mut Self,
        _: &ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        index: &usize,
        _: &wayland_client::Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event;
        let events = &mut state.frames[*index].events;
        match event {
            Event::Damage {
                x,
                y,
                width,
                height,
            } => events.damage.push((x, y, width, height)),
            Event::PresentationTime {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                let seconds = u64::from(tv_sec_hi) << 32 | u64::from(tv_sec_lo);
                events.presented = Some(Duration::new(seconds, tv_nsec));
            }
            Event::Ready => events.ready = true,
            Event::Failed {
                reason: WEnum::Value(reason),
            } => events.failed = Some(reason),
            _ => {}
        }
    }
}

delegate_noop!(CaptureState: ignore wl_shm::WlShm);
delegate_noop!(CaptureState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(CaptureState: ignore wl_buffer::WlBuffer);
delegate_noop!(CaptureState: ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(CaptureState: ext_image_capture_source_v1::ExtImageCaptureSourceV1);
delegate_noop!(CaptureState: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1);
