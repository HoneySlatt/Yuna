//! A client that captures outputs through `wlr-screencopy-unstable-v1`, as uwui's `wayland.py`
//! does, for tests: it binds every `wl_output` by name, asks frames of one, whole or a region,
//! and copies them into `wl_shm` buffers it reads back, recording every event a frame gets.
//!
//! A [`WlrCaptureClient`] is a test [`Client`] with these objects on an event queue of their
//! own, over the same connection, as `capture_client.rs` does.

use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::time::Duration;

use smithay::reexports::rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};
use yuna::Compositor;
use yuna::yuna_layout::SeatId;

use crate::client::Client;

pub use wl_shm::Format;

/// What a frame was told.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct WlrFrameEvents {
    /// Its `buffer` event: format, width, height, stride.
    pub buffer: Option<(WEnum<Format>, u32, u32, u32)>,
    pub buffer_done: bool,
    pub flags: Option<u32>,
    pub damage: Vec<(u32, u32, u32, u32)>,
    pub presented: Option<Duration>,
    pub failed: bool,
}

impl WlrFrameEvents {
    pub fn ready(&self) -> bool {
        self.presented.is_some()
    }
}

/// A frame asked, its buffer's memory once copied.
struct WlrFrame {
    frame: zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    file: Option<(File, u32)>,
    events: WlrFrameEvents,
}

/// A test client that captures outputs through wlr-screencopy.
pub struct WlrCaptureClient {
    pub client: Client,
    queue: EventQueue<WlrCaptureState>,
    pub state: WlrCaptureState,
}

/// What the client's capture objects hold and received.
#[derive(Default)]
pub struct WlrCaptureState {
    shm: Option<wl_shm::WlShm>,
    manager: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    /// Every `wl_output`, with the name it gave.
    outputs: Vec<(wl_output::WlOutput, Option<String>)>,
    frames: Vec<WlrFrame>,
}

impl WlrCaptureClient {
    /// Connects a client of `seat` and binds the globals.
    pub fn connect(compositor: &mut Compositor, seat: SeatId) -> WlrCaptureClient {
        WlrCaptureClient::bind(Client::connect_to(compositor, seat), compositor)
    }

    /// As [`WlrCaptureClient::connect`], for a client shown every output's `wl_output`.
    pub fn connect_spy(compositor: &mut Compositor, seat: SeatId) -> WlrCaptureClient {
        WlrCaptureClient::bind(
            Client::connect_seeing_every_seat(compositor, seat),
            compositor,
        )
    }

    /// Binds the globals on `client`'s connection, a client that already captures through
    /// `ext-image-copy-capture-v1` for instance.
    pub fn bind_to(client: Client, compositor: &mut Compositor) -> WlrCaptureClient {
        WlrCaptureClient::bind(client, compositor)
    }

    fn bind(client: Client, compositor: &mut Compositor) -> WlrCaptureClient {
        let queue = client.connection().new_event_queue();
        client
            .connection()
            .display()
            .get_registry(&queue.handle(), ());
        let mut capture = WlrCaptureClient {
            client,
            queue,
            state: WlrCaptureState::default(),
        };
        capture.roundtrip(compositor);
        capture.roundtrip(compositor);
        assert!(
            capture.state.shm.is_some() && capture.state.manager.is_some(),
            "a global is missing"
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

    fn output(&self, name: &str) -> wl_output::WlOutput {
        self.state
            .outputs
            .iter()
            .find(|(_, given)| given.as_deref() == Some(name))
            .map(|(output, _)| output.clone())
            .unwrap_or_else(|| panic!("no wl_output named {name}"))
    }

    /// Asks a frame of the output named `name`, its region (`x`, `y`, `width`, `height`) in
    /// logical units when given; returns the frame's index.
    pub fn frame(&mut self, name: &str, region: Option<(i32, i32, i32, i32)>) -> usize {
        let output = self.output(name);
        let qh = self.queue.handle();
        let index = self.state.frames.len();
        let manager = self.state.manager.as_ref().expect("the manager");
        let frame = match region {
            None => manager.capture_output(0, &output, &qh, index),
            Some((x, y, width, height)) => {
                manager.capture_output_region(0, &output, x, y, width, height, &qh, index)
            }
        };
        self.state.frames.push(WlrFrame {
            frame,
            file: None,
            events: WlrFrameEvents::default(),
        });
        index
    }

    /// Copies frame `index` into a new buffer of `format`, `width` × `height` at 4 bytes a
    /// pixel, with or without waiting for damage.
    pub fn copy(
        &mut self,
        index: usize,
        format: Format,
        (width, height): (u32, u32),
        with_damage: bool,
    ) {
        let qh = self.queue.handle();
        let stride = width * 4;
        let size = stride * height;
        let fd = memfd_create("yuna-test-wlr-capture", MemfdFlags::CLOEXEC).expect("a memfd");
        let file = File::from(fd);
        file.set_len(u64::from(size.max(1)))
            .expect("the memfd takes its size");
        let shm = self.state.shm.as_ref().expect("wl_shm");
        let pool = shm.create_pool(file.as_fd(), size.max(1) as i32, &qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            format,
            &qh,
            (),
        );
        pool.destroy();
        let frame = &mut self.state.frames[index];
        if with_damage {
            frame.frame.copy_with_damage(&buffer);
        } else {
            frame.frame.copy(&buffer);
        }
        frame.file = Some((file, stride));
    }

    pub fn events(&self, index: usize) -> &WlrFrameEvents {
        &self.state.frames[index].events
    }

    /// Destroys frame `index`, as a client does once it is ready or failed.
    pub fn destroy(&mut self, index: usize) {
        self.state.frames[index].frame.destroy();
    }

    /// The four bytes of pixel (`x`, `y`) of frame `index`'s buffer, in memory order.
    pub fn pixel(&self, index: usize, x: u32, y: u32) -> [u8; 4] {
        let (file, stride) = self.state.frames[index]
            .file
            .as_ref()
            .expect("a frame copied");
        let mut bytes = [0; 4];
        file.read_exact_at(&mut bytes, u64::from(y * stride + x * 4))
            .expect("the buffer is read");
        bytes
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for WlrCaptureState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
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
            "zwlr_screencopy_manager_v1" => {
                state.manager = Some(registry.bind(name, version.min(3), qh, ()));
            }
            "wl_output" => {
                let index = state.outputs.len();
                let output = registry.bind(name, version.min(4), qh, index);
                state.outputs.push((output, None));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for WlrCaptureState {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.outputs[*index].1 = Some(name);
        }
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, usize> for WlrCaptureState {
    fn event(
        state: &mut Self,
        _: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let events = &mut state.frames[*index].events;
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => events.buffer = Some((format, width, height, stride)),
            zwlr_screencopy_frame_v1::Event::BufferDone => events.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                events.flags = Some(match flags {
                    WEnum::Value(flags) => flags.bits(),
                    WEnum::Unknown(bits) => bits,
                });
            }
            zwlr_screencopy_frame_v1::Event::Damage {
                x,
                y,
                width,
                height,
            } => events.damage.push((x, y, width, height)),
            zwlr_screencopy_frame_v1::Event::Ready {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                events.presented = Some(Duration::new(seconds, tv_nsec));
            }
            zwlr_screencopy_frame_v1::Event::Failed => events.failed = true,
            _ => {}
        }
    }
}

delegate_noop!(WlrCaptureState: ignore wl_shm::WlShm);
delegate_noop!(WlrCaptureState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(WlrCaptureState: ignore wl_buffer::WlBuffer);
delegate_noop!(WlrCaptureState: zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1);
