//! The test's side of `yuna mcp`'s standard input and output, shared by `applications.rs` and
//! `opium.rs`: requests written one a line, replies read on a thread.

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// An MCP connection to a `yuna mcp` process, driven as an agent drives it.
pub struct McpClient {
    input: RefCell<ChildStdin>,
    replies: mpsc::Receiver<Value>,
    next_id: Cell<u64>,
}

impl McpClient {
    /// Takes `child`'s standard input and output, which must be piped.
    pub fn connect(child: &mut Child) -> McpClient {
        let stdout = child.stdout.take().expect("its standard output");
        let input = child.stdin.take().expect("its standard input");
        let (sender, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                    return;
                };
                if sender.send(reply).is_err() {
                    return;
                }
            }
        });
        McpClient {
            input: RefCell::new(input),
            replies,
            next_id: Cell::new(1),
        }
    }

    /// Calls an MCP tool and gives its result, or what it refused, or that it did not reply in
    /// `patience`.
    pub fn call(&self, tool: &str, arguments: Value, patience: Duration) -> Result<Value, String> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let request = json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
                "name": tool,
                "arguments": arguments,
            }
        });
        writeln!(self.input.borrow_mut(), "{request}")
            .map_err(|error| format!("{tool}: {error}"))?;
        let deadline = Instant::now() + patience;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let reply = self
                .replies
                .recv_timeout(left)
                .map_err(|_| format!("no reply to {tool} in {} s", patience.as_secs()))?;
            if reply["id"] == json!(id) {
                let result = reply["result"].clone();
                if result["isError"] == json!(false) {
                    return Ok(result);
                }
                return Err(format!("{tool} {arguments} failed: {reply}"));
            }
        }
    }
}
