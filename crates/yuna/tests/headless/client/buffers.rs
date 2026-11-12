//! The test client's side of how buffers are sized and shared beyond plain shared memory:
//! viewports (`wp_viewporter`), fractional scale (`wp_fractional_scale_v1`) and dmabufs
//! (`linux-dmabuf`); and the protocol errors a misbehaving client gets.

use std::fs::File;
use std::time::Duration;

use smithay::reexports::rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::protocol::wl_buffer;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop, event_created_child};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use yuna::Compositor;

use super::{Client, ClientState};

/// A protocol error the compositor sent: the client's connection ends with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError {
    /// The interface of the object the error is about.
    pub interface: String,
    /// The error's code, in that interface's `error` enum.
    pub code: u32,
}

impl Client {
    /// Creates the window's `wp_fractional_scale_v1`; its preferred scales are recorded.
    pub fn get_fractional_scale(&mut self, window: usize) {
        let qh = self.queue.handle();
        let manager = self
            .state
            .fractional_scale_manager
            .as_ref()
            .expect("wp_fractional_scale_manager_v1");
        let window_state = &mut self.state.windows[window];
        window_state.fractional_scale =
            Some(manager.get_fractional_scale(&window_state.surface, &qh, window));
    }

    /// Creates the window's `wp_viewport`.
    pub fn get_viewport(&mut self, window: usize) {
        let qh = self.queue.handle();
        let viewporter = self.state.viewporter.as_ref().expect("wp_viewporter");
        let window_state = &mut self.state.windows[window];
        window_state.viewport = Some(viewporter.get_viewport(&window_state.surface, &qh, ()));
    }

    /// Sets the destination of the window's viewport, for the next commit.
    pub fn set_viewport_destination(&mut self, window: usize, width: i32, height: i32) {
        self.viewport(window).set_destination(width, height);
    }

    /// Sets the source of the window's viewport, in buffer pixels, for the next commit.
    pub fn set_viewport_source(&mut self, window: usize, (x, y): (f64, f64), (w, h): (f64, f64)) {
        self.viewport(window).set_source(x, y, w, h);
    }

    fn viewport(&self, window: usize) -> &wp_viewport::WpViewport {
        self.state.windows[window]
            .viewport
            .as_ref()
            .expect("the window's viewport")
    }

    /// The version of the `zwp_linux_dmabuf_v1` global the compositor offers; none without one.
    pub fn dmabuf_version(&self) -> Option<u32> {
        self.state.dmabuf.as_ref().map(|(_, version)| *version)
    }

    /// Creates a `zwp_linux_buffer_params_v1`; what its `create` comes to is recorded in
    /// `dmabuf_results`.
    pub fn create_dmabuf_params(&mut self) -> zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1 {
        let qh = self.queue.handle();
        let (dmabuf, _) = self.state.dmabuf.as_ref().expect("zwp_linux_dmabuf_v1");
        dmabuf.create_params(&qh, ())
    }

    /// Lets the compositor handle what the client sent until it answers with a protocol error,
    /// and returns it. Panics if none comes.
    pub fn protocol_error(&mut self, compositor: &mut Compositor) -> ProtocolError {
        for _ in 0..100 {
            // Once the error has come, sending and reading fail: the connection is over, and the
            // error itself says why.
            let _ = self.queue.flush();
            compositor
                .dispatch(Some(Duration::ZERO))
                .expect("the compositor dispatches");
            if let Some(guard) = self.queue.prepare_read() {
                let _ = guard.read();
            }
            let _ = self.queue.dispatch_pending(&mut self.state);
            if let Some(error) = self.connection.protocol_error() {
                return ProtocolError {
                    interface: error.object_interface,
                    code: error.code,
                };
            }
        }
        panic!("the compositor sent no protocol error");
    }
}

/// A file of `size` bytes in memory, as a client hands it to the compositor.
pub fn memory_file(size: u64) -> File {
    let fd = memfd_create("yuna-test-dmabuf", MemfdFlags::CLOEXEC).expect("a memfd");
    let file = File::from(fd);
    file.set_len(size).expect("the memfd takes its size");
    file
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, usize> for ClientState {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.windows[*index].preferred_scales.push(scale);
        }
    }
}

impl Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        event: zwp_linux_dmabuf_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Version 3 offers each format with its modifiers through `modifier` alone; `format` is
        // for versions 1 and 2.
        if let zwp_linux_dmabuf_v1::Event::Modifier {
            format,
            modifier_hi,
            modifier_lo,
        } = event
        {
            let modifier = (u64::from(modifier_hi) << 32) | u64::from(modifier_lo);
            state.dmabuf_formats.push((format, modifier));
        }
    }
}

impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { .. } => state.dmabuf_results.push(true),
            zwp_linux_buffer_params_v1::Event::Failed => state.dmabuf_results.push(false),
            _ => {}
        }
    }

    event_created_child!(ClientState, zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (wl_buffer::WlBuffer, ()),
    ]);
}

delegate_noop!(ClientState: ignore wp_viewporter::WpViewporter);
delegate_noop!(ClientState: ignore wp_viewport::WpViewport);
delegate_noop!(ClientState: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
