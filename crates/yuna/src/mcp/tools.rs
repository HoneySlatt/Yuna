//! The tools `yuna mcp` offers, their schemas, their arguments and how their results are told
//! ([ADR-0033](../../../../docs/adr/0033-the-mcp-server.md)). What each does is in `mod.rs`,
//! which holds the compositor.
//!
//! Each tool that does several things takes an `action`. A result is a JSON object, given both
//! as `structuredContent` and as text, which MCP asks for clients that read text alone; a failure
//! is a text with `isError`, which the model reads and may correct.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use super::programs::{End, Program};
use super::protocol::{self, Revision};
use crate::compositor::{OutputDescription, WindowDescription};

/// The longest a call may wait: under the 60 s at which Codex gives up on a call by default
/// ([survey](../../../../docs/survey.md#agents-and-mcp)).
pub const MAX_WAIT: Duration = Duration::from_secs(50);

/// How many calls may wait at once; past it, a call that would wait is refused.
pub const MAX_WAITING_CALLS: usize = 64;

/// Every tool's definition, for `tools/list`, always in this order.
pub fn definitions() -> Value {
    json!([
        {
            "name": "outputs",
            "title": "Virtual outputs",
            "description": "List, create or remove Yuna's virtual outputs: screens no monitor \
                shows, drawn at their own size, rate and scale. Windows open on the focused \
                output. Coordinates are logical: physical pixels divided by the scale.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "create", "remove"] },
                    "width": { "type": "integer", "minimum": 1,
                        "description": "create: physical pixels; default 3840" },
                    "height": { "type": "integer", "minimum": 1,
                        "description": "create: physical pixels; default 2160" },
                    "rate": { "type": "number", "minimum": 1, "maximum": 1000,
                        "description": "create: refreshes a second; default 240" },
                    "scale": { "type": "number", "minimum": 0.25, "maximum": 8,
                        "description": "create: physical pixels per logical unit; default 1.5" },
                    "output": { "type": "integer", "description": "remove: the output's id" }
                },
                "required": ["action"],
                "additionalProperties": false
            }
        },
        {
            "name": "spawn",
            "title": "Programs",
            "description": "Start a program in Yuna, list the programs started, or stop one. A \
                program gets Yuna's Wayland socket, an environment without DISPLAY or D-Bus, \
                and a log file of its output and errors, whose path the result gives.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["start", "list", "stop"] },
                    "command": { "type": "array", "items": { "type": "string" }, "minItems": 1,
                        "description": "start: the program, found in PATH, and its arguments" },
                    "env": { "type": "object", "additionalProperties": { "type": "string" },
                        "description": "start: variables added to its environment" },
                    "cwd": { "type": "string", "description": "start: its working folder" },
                    "program": { "type": "integer", "description": "stop: the program's id" },
                    "force": { "type": "boolean",
                        "description": "stop: SIGKILL rather than SIGTERM" }
                },
                "required": ["action"],
                "additionalProperties": false
            }
        },
        {
            "name": "windows",
            "title": "Windows",
            "description": "List the windows open in Yuna, in the order they opened: id, \
                application id, title, process id, output, workspace, logical rectangle \
                (none when hidden), focus. With wait_ms, waits up to that long for a window \
                that matches the filters, when none does yet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app_id": { "type": "string", "description": "only this application id" },
                    "pid": { "type": "integer", "description": "only this client process" },
                    "wait_ms": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT.as_millis(),
                        "description": "how long to wait for a match; 0 by default" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "clock",
            "title": "Clock",
            "description": "Read Yuna's clock, on CLOCK_MONOTONIC's scale, in nanoseconds; or \
                advance it, when Yuna was started with --manual-clock, running every frame due \
                on the way. It never moves past real time.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["now", "advance"] },
                    "by_ns": { "type": "integer", "minimum": 1,
                        "description": "advance: nanoseconds" },
                    "by_ms": { "type": "number", "exclusiveMinimum": 0,
                        "description": "advance: milliseconds, instead of by_ns" }
                },
                "required": ["action"],
                "additionalProperties": false
            }
        }
    ])
}

/// `outputs`' arguments.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputsCall {
    // Braces, so that `deny_unknown_fields` reaches it: serde ignores the fields of a unit
    // variant.
    List {},
    Create {
        width: Option<u32>,
        height: Option<u32>,
        rate: Option<f64>,
        scale: Option<f64>,
    },
    Remove {
        output: u64,
    },
}

/// `spawn`'s arguments.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpawnCall {
    Start {
        command: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    },
    List {},
    Stop {
        program: u64,
        #[serde(default)]
        force: bool,
    },
}

/// `windows`' arguments.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsCall {
    pub app_id: Option<String>,
    pub pid: Option<i32>,
    #[serde(default)]
    pub wait_ms: u64,
}

impl WindowsCall {
    /// Whether `window` passes the filters.
    pub fn matches(&self, window: &WindowDescription) -> bool {
        self.app_id
            .as_ref()
            .is_none_or(|app_id| window.app_id.as_ref() == Some(app_id))
            && self.pid.is_none_or(|pid| window.pid == Some(pid))
    }
}

/// `clock`'s arguments.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockCall {
    Now {},
    Advance {
        by_ns: Option<u64>,
        by_ms: Option<f64>,
    },
}

/// Reads a tool's arguments; the error says what is wrong, for the model to correct.
pub fn arguments<T: DeserializeOwned>(arguments: Map<String, Value>) -> Result<T, String> {
    serde_json::from_value(Value::Object(arguments))
        .map_err(|error| format!("invalid arguments: {error}"))
}

/// A tool's result.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    /// The result, or none for a failure.
    structured: Option<Value>,
    /// What the model reads: the result as JSON, or the failure.
    text: String,
}

impl ToolResult {
    /// A result.
    pub fn done(structured: Value) -> ToolResult {
        ToolResult {
            text: structured.to_string(),
            structured: Some(structured),
        }
    }

    /// A failure, with what went wrong.
    pub fn failed(reason: impl Into<String>) -> ToolResult {
        ToolResult {
            structured: None,
            text: reason.into(),
        }
    }

    /// `tools/call`'s result, shaped for `revision`.
    pub fn into_result(self, revision: Revision) -> Value {
        let mut body = Map::new();
        body.insert(
            "content".to_owned(),
            json!([{ "type": "text", "text": self.text }]),
        );
        body.insert("isError".to_owned(), json!(self.structured.is_none()));
        if let Some(structured) = self.structured {
            body.insert("structuredContent".to_owned(), structured);
        }
        protocol::result(revision, body)
    }
}

/// An output, as the tools give it.
pub fn output_json(output: &OutputDescription) -> Value {
    json!({
        "id": output.id.0,
        "name": output.name,
        "width": output.mode.width(),
        "height": output.mode.height(),
        "refresh_mhz": output.mode.refresh_millihertz(),
        "scale": output.mode.scale(),
        "logical": {
            "x": output.rect.x, "y": output.rect.y,
            "width": output.rect.w, "height": output.rect.h,
        },
    })
}

/// A window, as the tools give it.
pub fn window_json(window: &WindowDescription) -> Value {
    json!({
        "id": window.id.0,
        "app_id": window.app_id,
        "title": window.title,
        "pid": window.pid,
        "output": window.output.map(|output| output.0),
        "workspace": window.workspace,
        "rect": window.rect.map(|rect| json!({
            "x": rect.x, "y": rect.y, "width": rect.w, "height": rect.h,
        })),
        "focused": window.focused,
        "floating": window.floating,
        "fullscreen": window.fullscreen,
    })
}

/// A program, as the tools give it.
pub fn program_json(program: &Program) -> Value {
    let (state, exit_code, signal) = match program.end {
        None => ("running", None, None),
        Some(End::Exited(code)) => ("exited", Some(code), None),
        Some(End::Killed(signal)) => ("killed", None, Some(signal)),
    };
    json!({
        "id": program.id,
        "pid": program.pid.as_raw_nonzero().get(),
        "command": program.command,
        "state": state,
        "group_running": program.group_runs(),
        "exit_code": exit_code,
        "signal": signal,
        "log": program.log,
        "log_bytes": program.log_written(),
        "log_dropped": program.log_dropped(),
    })
}

/// An advance's length from its arguments: exactly one of nanoseconds or milliseconds, more
/// than zero.
pub fn advance_length(by_ns: Option<u64>, by_ms: Option<f64>) -> Result<Duration, String> {
    match (by_ns, by_ms) {
        (Some(ns), None) if ns > 0 => Ok(Duration::from_nanos(ns)),
        (None, Some(ms)) if ms.is_finite() && ms > 0.0 => {
            Duration::try_from_secs_f64(ms / 1_000.0).map_err(|e| format!("by_ms: {e}"))
        }
        (Some(_), Some(_)) => Err("give by_ns or by_ms, not both".to_owned()),
        (None, None) => Err("advance needs by_ns or by_ms".to_owned()),
        _ => Err("an advance is longer than zero".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn tools_are_listed_in_a_fixed_order_each_with_an_object_schema() {
        let tools = definitions();
        let names: Vec<&str> = tools
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(names, ["outputs", "spawn", "windows", "clock"]);
        for tool in tools.as_array().into_iter().flatten() {
            assert_eq!(tool["inputSchema"]["type"], json!("object"), "{tool}");
            assert_eq!(tool["inputSchema"]["additionalProperties"], json!(false));
        }
    }

    #[test]
    fn each_property_a_schema_names_is_read_by_its_arguments() {
        // Every property of each schema, given alone with a valid action, is accepted or
        // refused for its action, never as an unknown field.
        let outputs: Result<OutputsCall, _> = arguments(args(json!({
            "action": "create", "width": 1920, "height": 1080, "rate": 60, "scale": 1
        })));
        assert!(matches!(
            outputs,
            Ok(OutputsCall::Create {
                width: Some(1920),
                ..
            })
        ));
        assert!(matches!(
            arguments(args(json!({"action": "remove", "output": 2}))),
            Ok(OutputsCall::Remove { output: 2 })
        ));
        let spawn: Result<SpawnCall, _> = arguments(args(json!({
            "action": "start", "command": ["vkcube"], "env": {"A": "b"}, "cwd": "/tmp"
        })));
        assert!(matches!(spawn, Ok(SpawnCall::Start { .. })));
        assert!(matches!(
            arguments(args(json!({"action": "stop", "program": 1, "force": true}))),
            Ok(SpawnCall::Stop {
                program: 1,
                force: true
            })
        ));
        let windows: Result<WindowsCall, _> =
            arguments(args(json!({"app_id": "a", "pid": 3, "wait_ms": 10})));
        assert!(windows.is_ok());
        assert!(matches!(
            arguments(args(json!({"action": "advance", "by_ms": 4.5}))),
            Ok(ClockCall::Advance {
                by_ms: Some(_),
                by_ns: None
            })
        ));
    }

    #[test]
    fn unknown_fields_actions_and_wrong_types_are_refused() {
        assert!(arguments::<OutputsCall>(args(json!({"action": "list", "x": 1}))).is_err());
        assert!(arguments::<OutputsCall>(args(json!({"action": "rotate"}))).is_err());
        assert!(arguments::<OutputsCall>(args(json!({}))).is_err());
        assert!(arguments::<OutputsCall>(args(json!({"action": "create", "width": -1}))).is_err());
        assert!(arguments::<WindowsCall>(args(json!({"wait": 1}))).is_err());
        assert!(arguments::<SpawnCall>(args(json!({"action": "start"}))).is_err());
    }

    #[test]
    fn an_advance_takes_one_positive_length() {
        assert_eq!(advance_length(Some(5), None), Ok(Duration::from_nanos(5)));
        assert_eq!(
            advance_length(None, Some(4.5)),
            Ok(Duration::from_micros(4_500))
        );
        assert!(advance_length(None, None).is_err());
        assert!(advance_length(Some(1), Some(1.0)).is_err());
        assert!(advance_length(Some(0), None).is_err());
        assert!(advance_length(None, Some(-1.0)).is_err());
        assert!(advance_length(None, Some(f64::NAN)).is_err());
        assert!(advance_length(None, Some(1e300)).is_err());
    }

    #[test]
    fn a_failure_has_no_structured_content() {
        let failed = ToolResult::failed("no").into_result(Revision::Legacy);
        assert_eq!(failed["isError"], json!(true));
        assert!(failed.get("structuredContent").is_none());
        let done = ToolResult::done(json!({"a": 1})).into_result(Revision::Modern);
        assert_eq!(done["isError"], json!(false));
        assert_eq!(done["structuredContent"], json!({"a": 1}));
        assert_eq!(done["content"][0]["text"], json!("{\"a\":1}"));
        assert_eq!(done["resultType"], json!("complete"));
    }
}
