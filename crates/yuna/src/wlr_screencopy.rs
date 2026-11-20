//! `wlr-screencopy-unstable-v1`, version 3, Yuna's own handler on wayland-server: the capture
//! protocol uwui's `wayland.py` and older tools speak, beside `ext-image-copy-capture-v1`
//! (`screencopy.rs`), on the same captures (`capture.rs`)
//! ([ADR-0089](../../../docs/adr/0089-yuna-for-the-owners-other-projects.md),
//! [ADR-0038](../../../docs/adr/0038-captures-coordinates-windows-and-wayland.md)).
//!
//! Every client sees the global; a client captures only its own seat's outputs: a frame of
//! another seat's output fails. A frame is one copy of an output, whole or a region of it in its
//! logical coordinates, cut to the output, at its physical size, into a `wl_shm` buffer of
//! `XRGB8888` or `ARGB8888`; Yuna offers no dmabuf and draws no cursor, so `overlay_cursor` is
//! ignored. `copy` takes the output's newest frame, at its next refresh, armed if needed, as an
//! `ext-image-copy-capture-v1` session's first frame does. `copy_with_damage` takes a frame the
//! manager object has not copied yet: at once if the output drew since that manager's last copy
//! of it, or for its first; otherwise at the next refresh that draws something, so that a
//! client waiting costs nothing at rest. Its damage is the whole frame. `ready` gives the time
//! the frame is presented at, on `CLOCK_MONOTONIC`, as `wp_presentation` does.
//!
//! A frame waiting counts among the captures a client may keep
//! ([`MAX_CAPTURE_SESSIONS_PER_CLIENT`](crate::MAX_CAPTURE_SESSIONS_PER_CLIENT)) and all clients
//! together ([`MAX_CAPTURE_SESSIONS`](crate::MAX_CAPTURE_SESSIONS)); one past them fails at
//! once. A buffer of the wrong size or format, or a second copy of one frame, is the client's
//! protocol error, as the protocol says.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use smithay::output::Output;
use smithay::utils::{Physical, Rectangle};
use smithay::wayland::shm::with_buffer_contents;
use wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};
use wayland_server::backend::ClientId;
use wayland_server::protocol::wl_buffer::WlBuffer;
use wayland_server::protocol::wl_output::WlOutput;
use wayland_server::protocol::wl_shm::Format;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use yuna_layout::geometry::Rect;
use yuna_types::OutputId;

use crate::capture::{Destination, physical_area};
use crate::screencopy::{FORMATS, copy_into_shm};
use crate::seats::seat_of_client;
use crate::state::State;

/// The version Yuna offers: the protocol's last.
const VERSION: u32 = 3;

/// The format each frame's `buffer` event names; [`FORMATS`] are taken.
const OFFERED_FORMAT: Format = Format::Xrgb8888;

/// The global's data.
pub(crate) struct ManagerGlobal;

/// Creates the global, which every client sees.
pub(crate) fn manager_global(display: &DisplayHandle) {
    display.create_global::<State, ZwlrScreencopyManagerV1, _>(VERSION, ManagerGlobal);
}

/// A manager object's copies: how many frames each output had drawn at its last copy with
/// damage, so that the next one waits for something new.
#[derive(Default)]
pub(crate) struct ManagerCopies(Mutex<HashMap<OutputId, u64>>);

impl ManagerCopies {
    fn last(&self, output: OutputId) -> Option<u64> {
        let copies = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        copies.get(&output).copied()
    }

    fn record(&self, output: OutputId, drawn_frames: u64) {
        let mut copies = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        copies.insert(output, drawn_frames);
    }
}

/// What a frame copies: its output and the physical pixels read, or nothing when it failed at
/// its creation; whether it was copied already; and its manager's copies.
pub(crate) struct FrameData {
    source: Option<(OutputId, Rectangle<i32, Physical>)>,
    used: Mutex<bool>,
    manager: Arc<ManagerCopies>,
}

/// A client's frame, waiting for its pixels.
pub(crate) struct WlrFrame {
    frame: ZwlrScreencopyFrameV1,
    buffer: WlBuffer,
    output: OutputId,
    read: Rectangle<i32, Physical>,
    with_damage: bool,
    manager: Arc<ManagerCopies>,
}

impl WlrFrame {
    /// The physical pixels of its output it reads.
    pub(crate) fn read(&self) -> Rectangle<i32, Physical> {
        self.read
    }

    /// Whether it is the frame `frame`.
    pub(crate) fn is(&self, frame: &ZwlrScreencopyFrameV1) -> bool {
        self.frame == *frame
    }

    /// The client it belongs to.
    pub(crate) fn client(&self) -> Option<ClientId> {
        self.frame.client().map(|client| client.id())
    }

    /// Tells the client its frame failed, however it failed: the protocol has one event.
    pub(crate) fn fail(self) {
        self.frame.failed();
    }
}

impl State {
    /// Copies `pixels`, the frame's region of its output as R, G, B, A rows, into its buffer
    /// and tells the client it is ready, presented at `presented_at`; its manager records the
    /// output's `drawn_frames`. A buffer that no longer holds the frame fails it.
    pub(crate) fn finish_wlr_frame(
        &mut self,
        frame: WlrFrame,
        pixels: &[u8],
        presented_at: Duration,
        drawn_frames: u64,
    ) {
        let size = (frame.read.size.w as u32, frame.read.size.h as u32);
        if !copy_into_shm(&frame.buffer, pixels, size) {
            frame.frame.failed();
            return;
        }
        frame.manager.record(frame.output, drawn_frames);
        frame.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
        if frame.with_damage {
            frame.frame.damage(0, 0, size.0, size.1);
        }
        let seconds = presented_at.as_secs();
        frame.frame.ready(
            (seconds >> 32) as u32,
            seconds as u32,
            presented_at.subsec_nanos(),
        );
    }

    /// What a frame of `output`, a region of it in logical units or all of it, reads, for a
    /// client of `client`'s seat; none for an output that is gone or another seat's, or a
    /// region that holds no whole pixel of it.
    fn frame_source(
        &self,
        client: &Client,
        output: &WlOutput,
        region: Option<(i32, i32, i32, i32)>,
    ) -> Option<(OutputId, Rectangle<i32, Physical>)> {
        let wanted = Output::from_resource(output)?;
        let found = self.outputs.iter().find(|o| o.output == wanted)?;
        if found.seat != seat_of_client(client) {
            tracing::debug!(
                output = found.id.0,
                "a capture of another seat's output fails"
            );
            return None;
        }
        let rect = match region {
            None => Rect::new(0.0, 0.0, found.rect.w, found.rect.h),
            Some((x, y, width, height)) => {
                let (x, y) = (f64::from(x), f64::from(y));
                let (right, bottom) = (x + f64::from(width), y + f64::from(height));
                let (x, y) = (x.max(0.0), y.max(0.0));
                let (right, bottom) = (right.min(found.rect.w), bottom.min(found.rect.h));
                if right <= x || bottom <= y {
                    return None;
                }
                Rect::new(x, y, right - x, bottom - y)
            }
        };
        let read = physical_area(found.id, &found.mode, rect).ok()?;
        Some((found.id, read))
    }
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ManagerGlobal> for State {
    fn bind(
        _state: &mut State,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global: &ManagerGlobal,
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, Arc::new(ManagerCopies::default()));
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, Arc<ManagerCopies>> for State {
    fn request(
        state: &mut State,
        client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        copies: &Arc<ManagerCopies>,
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        let (frame, output, region) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput { frame, output, .. } => {
                (frame, output, None)
            }
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                output,
                x,
                y,
                width,
                height,
                ..
            } => (frame, output, Some((x, y, width, height))),
            // Its frames stay.
            _ => return,
        };
        let source = state.frame_source(client, &output, region);
        let frame = data_init.init(
            frame,
            FrameData {
                source,
                used: Mutex::new(false),
                manager: copies.clone(),
            },
        );
        match source {
            Some((_, read)) => {
                let (width, height) = (read.size.w as u32, read.size.h as u32);
                frame.buffer(OFFERED_FORMAT, width, height, width * 4);
                if frame.version() >= 3 {
                    frame.buffer_done();
                }
            }
            None => frame.failed(),
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for State {
    fn request(
        state: &mut State,
        client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        let (buffer, with_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            // Destroyed: a frame waiting is forgotten in `destroyed`.
            _ => return,
        };
        {
            let mut used = data.used.lock().unwrap_or_else(PoisonError::into_inner);
            if *used {
                frame.post_error(
                    zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                    "the frame was copied already",
                );
                return;
            }
            *used = true;
        }
        let Some((output, read)) = data.source else {
            frame.failed();
            return;
        };
        let (width, height) = (read.size.w, read.size.h);
        let fits = with_buffer_contents(&buffer, |_, _, buffer| {
            buffer.width == width
                && buffer.height == height
                && buffer.stride >= width * 4
                && FORMATS.contains(&buffer.format)
        });
        if !matches!(fits, Ok(true)) {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                format!("the buffer must be wl_shm, {width} × {height}, XRGB8888 or ARGB8888"),
            );
            return;
        }
        if !state.may_hold_another_capture(&client.id()) {
            tracing::debug!(output = output.0, "a frame past the captures' bounds fails");
            frame.failed();
            return;
        }
        let drawn = state
            .output_index(output)
            .map(|index| state.outputs[index].drawn_frames);
        let last = data.manager.last(output);
        let new_content = with_damage && last.is_some() && last == drawn;
        let waiting = WlrFrame {
            frame: frame.clone(),
            buffer,
            output,
            read,
            with_damage,
            manager: data.manager.clone(),
        };
        state.queue_capture(output, new_content, Destination::Wlr(waiting));
    }

    fn destroyed(
        state: &mut State,
        _client: ClientId,
        frame: &ZwlrScreencopyFrameV1,
        _: &FrameData,
    ) {
        state.pending_captures.retain(|pending| {
            !matches!(&pending.destination, Destination::Wlr(waiting) if waiting.is(frame))
        });
    }
}
