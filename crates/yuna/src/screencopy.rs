//! `ext-image-copy-capture-v1` and `ext-image-capture-source-v1`'s output sources, on Smithay's
//! modules: a thin layer over Yuna's captures (`capture.rs`)
//! ([ADR-0038](../../../docs/adr/0038-captures-coordinates-windows-and-wayland.md)).
//!
//! Every client sees the globals; a client captures only its own seat's outputs: a session of
//! another seat's output is stopped. A frame is copied at an output's refresh, never at once,
//! so that a client asking frames again and again costs one copy a refresh at most. A session's
//! frame is the output's newest, at its next refresh, armed if needed, when the output drew since
//! the session's last frame, or for its first; otherwise it waits for a refresh that draws
//! something, so that a recorder waiting costs nothing at rest. Each frame copies the whole
//! output, at its physical size, into the client's `wl_shm` buffer, and is ready as soon as it
//! is copied, as the protocol asks, with the time of the boundary it is presented at and full
//! damage. No cursor is captured, Yuna drawing none, and no window: a toplevel source needs
//! `ext-foreign-toplevel-list-v1`, which Yuna does not offer.

use std::time::Duration;

use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::utils::{Rectangle, Transform};
use smithay::wayland::image_capture_source::{
    ImageCaptureSource, ImageCaptureSourceHandler, ImageCaptureSourceState,
    OutputCaptureSourceHandler, OutputCaptureSourceState,
};
use smithay::wayland::image_copy_capture::{
    BufferConstraints, CaptureFailureReason, Frame, FrameRef, ImageCopyCaptureHandler,
    ImageCopyCaptureState, Session, SessionRef,
};
use smithay::wayland::shm::with_buffer_contents_mut;
use yuna_layout::OutputId;

use crate::capture::{Destination, Failure};
use crate::output::to_i32;
use crate::seats::seat_of_client;
use crate::state::State;

/// The most capture sessions one client keeps; past it, a new one is stopped at once
/// ([ADR-0028](../../../docs/adr/0028-bounds-on-what-one-client-may-ask.md)). A recorder or
/// grim keeps one.
pub const MAX_CAPTURE_SESSIONS_PER_CLIENT: usize = 4;

/// The most capture sessions all clients keep together: each frame waiting on an output costs
/// a copy of the whole output, about 33 MB at 4K, at each refresh that serves it.
pub const MAX_CAPTURE_SESSIONS: usize = 16;

/// The `wl_shm` formats a client's buffer may have: those every compositor's `wl_shm` takes,
/// and Yuna's offers alone, B, G, R, A in memory; the renderer's R, G, B, A are swapped into it.
const FORMATS: [Format; 2] = [Format::Argb8888, Format::Xrgb8888];

/// The three protocol states, kept by [`State`].
pub(crate) struct ScreencopyState {
    _sources: ImageCaptureSourceState,
    output_sources: OutputCaptureSourceState,
    copy: ImageCopyCaptureState,
    /// Each live session, its output, and how many frames that output had drawn at the
    /// session's last frame; none before its first.
    sessions: Vec<(Session, OutputId, Option<u64>)>,
}

impl ScreencopyState {
    /// Announces the globals to every client.
    pub(crate) fn new(
        display: &smithay::reexports::wayland_server::DisplayHandle,
    ) -> ScreencopyState {
        ScreencopyState {
            _sources: ImageCaptureSourceState::new(),
            output_sources: OutputCaptureSourceState::new::<State>(display),
            copy: ImageCopyCaptureState::new::<State>(display),
            sessions: Vec::new(),
        }
    }

    /// Forgets the sessions their clients destroyed.
    pub(crate) fn cleanup(&mut self) {
        self.copy.cleanup();
    }
}

/// The output a source shows, recorded when the source is made.
struct SourceOutput(OutputId);

fn source_output(source: &ImageCaptureSource) -> Option<OutputId> {
    source
        .user_data()
        .get::<SourceOutput>()
        .map(|output| output.0)
}

/// A client's frame, waiting for its pixels.
pub(crate) struct ClientFrame {
    frame: Frame,
    session: SessionRef,
}

impl ClientFrame {
    /// Tells the client its frame failed: as stopped when its output went, as a failure it may
    /// retry when the frame could not be drawn or read.
    pub(crate) fn fail(self, failure: Failure) {
        self.frame.fail(match failure {
            Failure::OutputGone => CaptureFailureReason::Stopped,
            Failure::ForNow => CaptureFailureReason::Unknown,
        });
    }
}

impl State {
    /// Copies an output's whole frame, `pixels` of R, G, B, A at `width` × `height`, into the
    /// buffer of a client's frame, in the buffer's format, and tells the client it is ready,
    /// presented at `presented_at`; its session records the output's `drawn_frames`. A buffer
    /// that no longer holds the frame fails it.
    pub(crate) fn finish_client_frame(
        &mut self,
        frame: ClientFrame,
        pixels: &[u8],
        (width, height): (u32, u32),
        presented_at: Duration,
        drawn_frames: u64,
    ) {
        let ClientFrame { frame, session } = frame;
        let written = with_buffer_contents_mut(&frame.buffer(), |memory, length, data| {
            let row = width as usize * 4;
            let (offset, stride) = (data.offset.max(0) as usize, data.stride.max(0) as usize);
            let fits = data.width as u32 >= width
                && data.height as u32 >= height
                && stride >= row
                && offset + stride * (height as usize).saturating_sub(1) + row <= length;
            if !fits || !FORMATS.contains(&data.format) {
                return false;
            }
            let mut converted = vec![0u8; row];
            for (y, source) in pixels.chunks_exact(row).take(height as usize).enumerate() {
                converted.copy_from_slice(source);
                converted
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .for_each(|p| p.swap(0, 2));
                // SAFETY: Smithay gives `memory` as the client's pool, `length` bytes long, mapped
                // while this closure runs, and turns a SIGBUS while it is written into an error;
                // the row written, `row` bytes at `offset + y × stride`, lies within it, checked
                // above. No reference into the pool is made, so a client writing it meanwhile
                // spoils only its own buffer.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        converted.as_ptr(),
                        memory.add(offset + y * stride),
                        row,
                    );
                }
            }
            true
        });
        if !matches!(written, Ok(true)) {
            frame.fail(CaptureFailureReason::BufferConstraints);
            return;
        }
        // Smithay sends no damage event for `None`, while the protocol asks full damage of a
        // session's first frame: each frame says it in full.
        let full = Rectangle::from_size((to_i32(width), to_i32(height)).into());
        frame.success(Transform::Normal, Some(vec![full]), presented_at);
        if let Some(entry) = self
            .screencopy
            .sessions
            .iter_mut()
            .find(|(s, _, _)| **s == session)
        {
            entry.2 = Some(drawn_frames);
        }
    }

    /// Stops every session of output `id`, which is being removed: their waiting frames fail.
    pub(crate) fn stop_capture_sessions(&mut self, id: OutputId) {
        // Dropping a session sends `stopped` and fails its frame.
        self.screencopy
            .sessions
            .retain(|(_, output, _)| *output != id);
    }
}

impl ImageCaptureSourceHandler for State {}

impl OutputCaptureSourceHandler for State {
    fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
        &mut self.screencopy.output_sources
    }

    fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
        if let Some(found) = self.outputs.iter().find(|o| o.output == *output) {
            source
                .user_data()
                .insert_if_missing(|| SourceOutput(found.id));
        }
    }
}

impl ImageCopyCaptureHandler for State {
    fn image_copy_capture_state(&mut self) -> &mut ImageCopyCaptureState {
        &mut self.screencopy.copy
    }

    /// The output's physical size and the formats above; none for a source whose output went,
    /// which stops the session.
    fn capture_constraints(&mut self, source: &ImageCaptureSource) -> Option<BufferConstraints> {
        let index = source_output(source).and_then(|id| self.output_index(id))?;
        let mode = self.outputs[index].mode;
        Some(BufferConstraints {
            size: (to_i32(mode.width()), to_i32(mode.height())).into(),
            shm: FORMATS.to_vec(),
        })
    }

    /// Keeps a session of the client's own seat's output, within the bounds; drops any other,
    /// which stops it.
    fn new_session(&mut self, session: Session) {
        let Some(output) = source_output(&session.source()) else {
            return;
        };
        let Some(client) = session.client() else {
            return;
        };
        let seat = seat_of_client(&client);
        let ours = self
            .output_index(output)
            .is_some_and(|index| self.outputs[index].seat == seat);
        if !ours {
            tracing::debug!(
                output = output.0,
                seat = seat.0,
                "a capture of another seat's output is stopped"
            );
            return;
        }
        let kept = self
            .screencopy
            .sessions
            .iter()
            .filter(|(s, _, _)| s.client().is_some_and(|c| c.id() == client.id()))
            .count();
        if kept >= MAX_CAPTURE_SESSIONS_PER_CLIENT
            || self.screencopy.sessions.len() >= MAX_CAPTURE_SESSIONS
        {
            tracing::debug!(
                seat = seat.0,
                "a capture session past the bounds is stopped"
            );
            return;
        }
        self.screencopy.sessions.push((session, output, None));
    }

    fn frame(&mut self, session: &SessionRef, frame: Frame) {
        let Some(&(_, output, last_frame)) = self
            .screencopy
            .sessions
            .iter()
            .find(|(s, _, _)| *s == *session)
        else {
            frame.fail(CaptureFailureReason::Stopped);
            return;
        };
        let frame = ClientFrame {
            frame,
            session: session.clone(),
        };
        // A later frame waits for new content only while the output drew nothing since the
        // session's last: what it drew meanwhile is the next frame.
        let drawn = self
            .output_index(output)
            .map(|index| self.outputs[index].drawn_frames);
        let new_content = last_frame.is_some() && last_frame == drawn;
        self.queue_capture(output, new_content, Destination::Client(frame));
    }

    fn frame_aborted(&mut self, aborted: FrameRef) {
        self.pending_captures.retain(|pending| {
            !matches!(&pending.destination, Destination::Client(waiting) if waiting.frame == aborted)
        });
    }

    fn session_destroyed(&mut self, destroyed: SessionRef) {
        self.screencopy
            .sessions
            .retain(|(session, _, _)| *session != destroyed);
    }
}
