//! The `frames` tool: a seat's frames, read from the compositor's frame record
//! ([ADR-0158](../../../../docs/adr/0158-a-frames-identity-each-task-wakes-yuna-through-a-source-of-its-own.md)),
//! each with the inputs and commits it covers and its presentation.
//!
//! As `events`, it keeps nothing between calls: the cursor is the agent's, a frame's `seq` or a
//! reply's `next`, numbered per seat; `missed` counts exactly the frames the seat's ring dropped
//! after it. Unlike `events`, it never waits: a frame is recorded when its refresh runs, and an
//! agent on the manual clock advances it first.

use serde_json::{Map, Value};
use yuna_types::OutputId;

use super::Server;
use super::tools::{self, DEFAULT_FRAMES_PER_REPLY, FramesCall, MAX_FRAMES_PER_REPLY, ToolResult};

impl Server {
    /// `frames`: the seat's frames after the cursor, of one output when asked.
    pub(super) fn frames(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call: FramesCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        let seat = match self.agent_seat(call.seat) {
            Ok(seat) => seat,
            Err(failed) => return failed,
        };
        let limit = call.limit.unwrap_or(DEFAULT_FRAMES_PER_REPLY);
        if limit == 0 || limit > MAX_FRAMES_PER_REPLY {
            return ToolResult::failed(format!("limit is from 1 to {MAX_FRAMES_PER_REPLY}"));
        }
        let output = call.output.map(OutputId);
        if let Some(output) = output
            && !self
                .compositor
                .output_descriptions()
                .iter()
                .any(|described| described.id == output && described.seat == seat)
        {
            return ToolResult::failed(format!("seat {} has no output {}", seat.0, output.0));
        }
        let newest = self.compositor.last_frame(seat);
        let after = match call.after {
            Some(after) if after > newest => {
                return ToolResult::failed(format!(
                    "after {after} is past seat {}'s newest frame, {newest}: give a reply's \
                     next, from this seat",
                    seat.0
                ));
            }
            Some(after) => after,
            None => newest,
        };
        let now = self.compositor.now();
        let priority = self.compositor.gpu_priority();
        match self.compositor.frames(seat, output, after, limit) {
            Some(reading) => ToolResult::done(tools::frames_json(&reading, now, priority)),
            None => ToolResult::failed(format!("seat {} keeps no frames", seat.0)),
        }
    }
}
