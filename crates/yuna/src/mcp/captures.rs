//! The `capture` tool: a PNG of a seat's output, a region of it or a window
//! ([ADR-0034](../../../../docs/adr/0034-captures.md)), and the encoders that make its image on
//! threads of their own, at most [`MAX_ENCODERS`] at once.
//!
//! Apart because a capture spans rounds: it waits for its frame, then for its encoder, then for
//! its frame's boundary. Its neighbours: `image.rs` makes the PNG; `input.rs` maps a point of a
//! capture's image to its output, for `pointer`; `waits.rs` ends a capture whose frame did not
//! come, that is cancelled, or whose seat closes.

use std::time::Instant;

use calloop::RegistrationToken;
use calloop::timer::{TimeoutAction, Timer};
use serde_json::{Map, Value};
use yuna_layout::SeatId;

use super::Server;
use super::image::{self, Encoded, MAX_IMAGE_SIDE};
use super::protocol::Revision;
use super::tools::{self, CaptureCall, CopiedCapture, MAX_WAIT, ToolResult};
use crate::rest::{Source, Wake, insert_source};
use crate::{CaptureTicket, ClockMode};

/// How many of a seat's captures `pointer` may name: its last ones
/// ([ADR-0038](../../../../docs/adr/0038-captures-coordinates-windows-and-wayland.md)).
pub const RECENT_CAPTURES: usize = 16;

/// How many captures' images may be encoded at once, each on a thread of its own with its
/// pixels, about 33 MB at 4K's physical size. An encoder is counted when it starts: a capture
/// copied while this many run waits for one to end, and a capture asked meanwhile is refused.
pub const MAX_ENCODERS: usize = 4;

/// A `capture` call, until it is answered.
#[derive(Debug)]
pub(super) struct CaptureWait {
    pub(super) key: u64,
    pub(super) id: Value,
    pub(super) revision: Revision,
    pub(super) seat: SeatId,
    pub(super) ticket: CaptureTicket,
    pub(super) stage: CaptureStage,
}

/// Where a capture is: its frame waited for, its image encoded, then held until its frame's
/// boundary ([ADR-0034](../../../../docs/adr/0034-captures.md)).
#[derive(Debug)]
pub(super) enum CaptureStage {
    /// Waiting for its frame, at most [`MAX_WAIT`]: the timer that fails it.
    Copying(RegistrationToken),
    /// Copied: its image is made on an encoder's thread, or its frame waits for one among the
    /// server's `waiting_encoders`.
    Encoding(CopiedCapture),
    /// Encoded; `woken` once a timer will wake the loop at its boundary.
    Encoded {
        copied: CopiedCapture,
        result: Result<Encoded, String>,
        woken: bool,
    },
}

impl Server {
    /// `capture`: a failure now, or none while the capture waits for its frame; one capture of
    /// a seat at a time.
    pub(super) fn capture(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call: CaptureCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        let seat = match self.agent_seat(call.seat) {
            Ok(seat) => seat,
            Err(failed) => return Some(failed),
        };
        // A capture cancelled while encoding leaves its encoder running, out of `captures`.
        if self.encoders_running >= MAX_ENCODERS {
            return Some(ToolResult::failed(format!(
                "{MAX_ENCODERS} captures are being encoded: wait for them"
            )));
        }
        if self.captures.iter().any(|capture| capture.seat == seat) {
            return Some(ToolResult::failed(format!(
                "a capture of seat {} is in flight: wait for its answer",
                seat.0
            )));
        }
        let target = match call.target() {
            Ok(target) => target,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        let ticket = match self.compositor.capture(seat, target, call.size()) {
            Ok(ticket) => ticket,
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        let key = self.next_wait;
        let timer = match self.arm_wait_timer(key, MAX_WAIT, Source::CaptureTimeout) {
            Ok(timer) => timer,
            Err(error) => {
                self.compositor.cancel_capture(ticket);
                return Some(ToolResult::failed(error.to_string()));
            }
        };
        self.next_wait += 1;
        self.captures.push(CaptureWait {
            key,
            id: id.clone(),
            revision,
            seat,
            ticket,
            stage: CaptureStage::Copying(timer),
        });
        None
    }

    /// Starts encoding each capture whose pixels the compositor copied, on a thread of its own,
    /// or queues its frame while [`MAX_ENCODERS`] run; answers those it failed.
    pub(super) fn encode_copied_captures(&mut self) {
        for (ticket, copied) in self.compositor.take_captures() {
            let Some(index) = self.captures.iter().position(|capture| {
                capture.ticket == ticket && matches!(capture.stage, CaptureStage::Copying(_))
            }) else {
                continue;
            };
            if let CaptureStage::Copying(timer) = self.captures[index].stage {
                self.compositor.loop_handle().remove(timer);
            }
            let frame = match copied {
                Ok(frame) => frame,
                Err(why) => {
                    self.answer_capture(
                        index,
                        ToolResult::failed(format!("the capture failed: {why}")),
                    );
                    continue;
                }
            };
            let (width, height) = (frame.mapping.width, frame.mapping.height);
            if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
                self.answer_capture(
                    index,
                    ToolResult::failed(format!(
                        "the capture's image is {width} × {height} pixels, over the \
                         {MAX_IMAGE_SIDE} an image may have on a side: capture a region, or at \
                         the logical size"
                    )),
                );
                continue;
            }
            self.captures[index].stage = CaptureStage::Encoding(CopiedCapture::of(&frame));
            self.waiting_encoders
                .push_back((self.captures[index].key, frame));
        }
        self.start_encoders();
    }

    /// Starts an encoder for each frame waiting, oldest first, while fewer than
    /// [`MAX_ENCODERS`] run: each is counted as it starts, so that never more run, however many
    /// captures a round copies.
    fn start_encoders(&mut self) {
        while self.encoders_running < MAX_ENCODERS
            && let Some((key, frame)) = self.waiting_encoders.pop_front()
        {
            let results = self.encoder_results.clone();
            let panic_asked = self.encoder_panic_asked();
            let started = std::thread::Builder::new()
                .name("yuna-capture".to_owned())
                .spawn(move || {
                    let began = Instant::now();
                    // A panic here is a bug of Yuna's; it fails this capture, not the seat's
                    // next ones, which would wait on it for ever.
                    let encoded = std::panic::catch_unwind(|| {
                        if panic_asked {
                            panic!("a test asks this capture's encoder to panic");
                        }
                        image::encode(&frame)
                    })
                    .unwrap_or_else(|_| Err("the capture's encoder failed".to_owned()));
                    tracing::debug!(
                        width = frame.mapping.width,
                        height = frame.mapping.height,
                        took_us = began.elapsed().as_micros(),
                        "capture encoded"
                    );
                    // A capture cancelled meanwhile, or a server gone, takes nothing.
                    let _ = results.send((key, encoded));
                });
            match started {
                Ok(_) => self.encoders_running += 1,
                Err(error) => {
                    if let Some(index) = self.captures.iter().position(|c| c.key == key) {
                        self.answer_capture(
                            index,
                            ToolResult::failed(format!(
                                "the capture could not be encoded: {error}"
                            )),
                        );
                    }
                }
            }
        }
    }

    /// Whether the encoder about to start is to panic: one of the first ones
    /// [`PANIC_IN_ENCODERS`](super::PANIC_IN_ENCODERS) names, in a debug build alone.
    #[cfg(debug_assertions)]
    fn encoder_panic_asked(&mut self) -> bool {
        let asked = self.encoder_panics > 0;
        self.encoder_panics = self.encoder_panics.saturating_sub(1);
        asked
    }

    /// Never, in a release build.
    #[cfg(not(debug_assertions))]
    fn encoder_panic_asked(&mut self) -> bool {
        false
    }

    /// Records the images the encoders finished.
    pub(super) fn note_encoded(&mut self, encoded: Vec<(u64, Result<Encoded, String>)>) {
        for (key, result) in encoded {
            self.encoders_running = self.encoders_running.saturating_sub(1);
            if let Some(capture) = self.captures.iter_mut().find(|capture| capture.key == key)
                && let CaptureStage::Encoding(copied) = capture.stage
            {
                capture.stage = CaptureStage::Encoded {
                    copied,
                    result,
                    woken: false,
                };
            }
        }
    }

    /// Answers each capture encoded whose frame's boundary has come; on the real clock, arms a
    /// timer to wake the loop at the boundary of one encoded before it.
    pub(super) fn answer_captures_due(&mut self) {
        let now = self.compositor.now();
        let real = self.compositor.clock_mode() == ClockMode::Real;
        let mut index = 0;
        while index < self.captures.len() {
            let CaptureStage::Encoded { copied, woken, .. } = &mut self.captures[index].stage
            else {
                index += 1;
                continue;
            };
            if copied.presented_at > now {
                if real && !*woken {
                    let wait = copied.presented_at.saturating_sub(now);
                    *woken = insert_source(
                        &self.compositor.loop_handle(),
                        Wake::of(Source::CaptureBoundary),
                        Timer::from_duration(wait),
                        |_, _, _| TimeoutAction::Drop,
                    )
                    .is_ok();
                }
                index += 1;
                continue;
            }
            let wait = self.captures.remove(index);
            let CaptureStage::Encoded { copied, result, .. } = wait.stage else {
                continue;
            };
            let result = match result {
                Ok(encoded) => {
                    let id = self.next_capture;
                    self.next_capture += 1;
                    let recent = self.recent_captures.entry(wait.seat).or_default();
                    if recent.len() >= RECENT_CAPTURES {
                        recent.pop_front();
                    }
                    recent.push_back((id, copied.mapping));
                    ToolResult::done_with_image(
                        tools::capture_json(id, &copied, encoded.png_bytes),
                        encoded.base64,
                    )
                }
                Err(why) => ToolResult::failed(why),
            };
            self.answer_waited(wait.id, wait.revision, result);
        }
    }

    /// Answers the capture at `index` with `result`, now.
    fn answer_capture(&mut self, index: usize, result: ToolResult) {
        let capture = self.captures.remove(index);
        self.answer_waited(capture.id, capture.revision, result);
    }

    /// Drops the capture at `index`, unanswered: its frame no longer waited for, or no longer
    /// waiting for an encoder, its image discarded when it comes.
    pub(super) fn drop_capture(&mut self, index: usize) -> CaptureWait {
        let capture = self.captures.remove(index);
        match capture.stage {
            CaptureStage::Copying(timer) => {
                self.compositor.loop_handle().remove(timer);
                self.compositor.cancel_capture(capture.ticket);
            }
            CaptureStage::Encoding(_) => self
                .waiting_encoders
                .retain(|(waiting, _)| *waiting != capture.key),
            CaptureStage::Encoded { .. } => {}
        }
        capture
    }
}
