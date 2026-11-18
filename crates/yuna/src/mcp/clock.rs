//! The `clock` tool: Yuna's clock read, and a manual clock advanced, never past real time.
//!
//! The clock itself is the compositor's; this module reads the call and tells the time.

use serde_json::{Map, Value, json};

use super::Server;
use super::tools::{self, ClockCall, ToolResult};
use crate::ClockMode;

impl Server {
    pub(super) fn clock(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        if let ClockCall::Advance { by_ns, by_ms } = call {
            if self.compositor.clock_mode() != ClockMode::Manual {
                return ToolResult::failed(
                    "the clock is real time, which moves by itself: start `yuna mcp \
                     --manual-clock` to step it",
                );
            }
            let by = match tools::advance_length(by_ns, by_ms) {
                Ok(by) => by,
                Err(reason) => return ToolResult::failed(reason),
            };
            let room = self
                .compositor
                .real_now()
                .saturating_sub(self.compositor.now());
            if by > room {
                return ToolResult::failed(format!(
                    "the clock never runs ahead of real time: it may advance by {} ns now",
                    room.as_nanos()
                ));
            }
            if let Err(error) = self.compositor.advance(by) {
                return ToolResult::failed(error.to_string());
            }
        }
        ToolResult::done(self.clock_json())
    }

    fn clock_json(&self) -> Value {
        let now = self.compositor.now();
        let manual = self.compositor.clock_mode() == ClockMode::Manual;
        json!({
            "mode": if manual { "manual" } else { "real" },
            "now_ns": now.as_nanos() as u64,
            "real_now_ns": self.compositor.real_now().as_nanos() as u64,
            "next_deadline_ns": self.compositor.next_deadline().map(|at| at.as_nanos() as u64),
        })
    }
}
