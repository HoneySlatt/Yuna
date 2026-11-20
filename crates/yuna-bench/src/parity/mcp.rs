//! The parity script's MCP client: JSON-RPC on `yuna mcp`'s standard input and output, one call
//! at a time, each waiting for its reply.
//!
//! The tools it calls (`seat`, `windows`, `workspaces`, `pointer`, `clock`, `rest`) and the fields
//! it reads are its contract with the builds it compares (ADR-0142): a build whose reply lacks one
//! makes the run fail, and its figures void.

use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, ChildStdout};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The revision of MCP the client speaks, each call stating it (`yuna mcp`'s `protocol.rs`).
const REVISION: &str = "2026-07-28";

/// A call's error.
#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// The clock may not advance that far yet: it never runs ahead of real time. The room it has,
    /// in nanoseconds.
    TooEarly(u64),
    /// Anything else, said.
    Failed(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::TooEarly(room) => write!(f, "the clock may advance by {room} ns now"),
            CallError::Failed(reason) => f.write_str(reason),
        }
    }
}

/// The client of one `yuna mcp`.
pub struct Mcp {
    input: ChildStdin,
    replies: Receiver<Value>,
    next_id: u64,
}

impl Mcp {
    /// A client of the server whose input and output these are. A thread reads the output, line
    /// by line, until it closes.
    pub fn new(input: ChildStdin, output: ChildStdout) -> Mcp {
        let (sender, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                // A line that is not JSON is no reply of this client's: it is left aside.
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && sender.send(value).is_err()
                {
                    break;
                }
            }
        });
        Mcp {
            input,
            replies,
            next_id: 0,
        }
    }

    /// Calls `tool` with `arguments` and gives its structured result, waiting up to `timeout`.
    pub fn call(
        &mut self,
        tool: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, CallError> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": REVISION,
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
                "name": tool,
                "arguments": arguments,
            },
        });
        let mut line = request.to_string();
        line.push('\n');
        self.input
            .write_all(line.as_bytes())
            .and_then(|()| self.input.flush())
            .map_err(|e| CallError::Failed(format!("{tool}: writing to Yuna: {e}")))?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let reply = match self.replies.recv_timeout(left) {
                Ok(reply) => reply,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(CallError::Failed(format!(
                        "{tool}: no reply in {} s",
                        timeout.as_secs()
                    )));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(CallError::Failed(format!("{tool}: Yuna's output closed")));
                }
            };
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            return result_of(tool, &reply);
        }
    }

    /// Closes the server's input, which stops `yuna mcp`.
    pub fn close(self) {
        drop(self.input);
    }
}

/// The structured result of `tool`'s `reply`, or why there is none.
fn result_of(tool: &str, reply: &Value) -> Result<Value, CallError> {
    if let Some(error) = reply.get("error") {
        return Err(CallError::Failed(format!("{tool}: {error}")));
    }
    let result = reply
        .get("result")
        .ok_or_else(|| CallError::Failed(format!("{tool}: a reply without a result")))?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        let text = result
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or("no text")
            .to_owned();
        return Err(match room_of(&text) {
            Some(room) => CallError::TooEarly(room),
            None => CallError::Failed(format!("{tool}: {text}")),
        });
    }
    result
        .get("structuredContent")
        .cloned()
        .ok_or_else(|| CallError::Failed(format!("{tool}: a result without structuredContent")))
}

/// The room in `clock advance`'s refusal: "… it may advance by <n> ns now".
fn room_of(text: &str) -> Option<u64> {
    let (_, rest) = text.split_once("may advance by ")?;
    rest.strip_suffix(" ns now")?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_its_error_and_the_clocks_refusal_are_told_apart() {
        let done = json!({"id": 1, "result": {"isError": false, "structuredContent": {"a": 1}}});
        assert_eq!(result_of("t", &done), Ok(json!({"a": 1})));
        let refused = json!({"id": 1, "result": {"isError": true, "content": [{"type": "text",
            "text": "the clock never runs ahead of real time: it may advance by 1200 ns now"}]}});
        assert_eq!(result_of("clock", &refused), Err(CallError::TooEarly(1200)));
        let failed = json!({"id": 1, "result": {"isError": true, "content": [{"type": "text",
            "text": "no seat 3"}]}});
        assert_eq!(
            result_of("windows", &failed),
            Err(CallError::Failed("windows: no seat 3".to_owned()))
        );
        let protocol = json!({"id": 1, "error": {"code": -32602, "message": "bad"}});
        assert!(matches!(
            result_of("rest", &protocol),
            Err(CallError::Failed(_))
        ));
        let bare = json!({"id": 1, "result": {"isError": false}});
        assert!(matches!(
            result_of("rest", &bare),
            Err(CallError::Failed(_))
        ));
    }
}
