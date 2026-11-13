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
use crate::input::PointerDescription;
use crate::{CaptureMapping, CaptureSize, CaptureTarget, CapturedFrame, LogicalRegion};
use yuna_layout::{OutputId, WindowId, WorkspaceInfo};

/// The longest a call may wait: under the 60 s at which Codex gives up on a call by default
/// ([survey](../../../../docs/survey.md#agents-and-mcp)).
pub const MAX_WAIT: Duration = Duration::from_secs(50);

/// How many calls may wait at once; past it, a call that would wait is refused.
pub const MAX_WAITING_CALLS: usize = 64;

/// What a seat's id means in a schema: every tool but `clock` acts on one seat.
const SEAT_PROPERTY: &str = "the seat's handle, from `seat create`";

/// Every tool's definition, for `tools/list`, always in this order.
pub fn definitions() -> Value {
    let seat = json!({ "type": "integer", "minimum": 2, "description": SEAT_PROPERTY });
    json!([
        {
            "name": "seat",
            "title": "Seats",
            "description": "Create a seat of your own, list the seats, or close one. A seat has \
                its own pointer, keyboard, focus and clipboard, its own outputs and workspaces, \
                and its own Wayland socket, which the programs it starts use; no input, focus \
                or clipboard of one seat reaches another. Create one first: every other tool but clock takes its \
                handle. Closing it releases what it holds, disconnects its windows, ends its \
                programs (SIGTERM, then SIGKILL 2 s later) and removes its outputs; it answers \
                once they are gone, in 3 s at most. A Yuna on a manual clock holds one seat.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["create", "list", "close"] },
                    "width": { "type": "integer", "minimum": 1,
                        "description": "create: its output's physical pixels; default 3840" },
                    "height": { "type": "integer", "minimum": 1,
                        "description": "create: physical pixels; default 2160" },
                    "rate": { "type": "number", "minimum": 1, "maximum": 1000,
                        "description": "create: refreshes a second; default 240" },
                    "scale": { "type": "number", "minimum": 0.25, "maximum": 8,
                        "description": "create: physical pixels per logical unit; default 1.5" },
                    "seat": { "type": "integer", "minimum": 2, "description": "close: its handle" }
                },
                "required": ["action"],
                "additionalProperties": false
            }
        },
        {
            "name": "outputs",
            "title": "Virtual outputs",
            "description": "List, create or remove a seat's virtual outputs: screens no monitor \
                shows, drawn at their own size, rate and scale. Windows open on the seat's \
                focused output. Coordinates are logical: physical pixels divided by the scale. \
                A seat keeps one output at least.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "create", "remove"] },
                    "seat": seat,
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
                "required": ["action", "seat"],
                "additionalProperties": false
            }
        },
        {
            "name": "workspaces",
            "title": "Workspaces",
            "description": "List a seat's workspaces by output, with their windows; create or \
                remove a named one; show one; or move a window to one, created if needed. A \
                removed workspace keeps its windows, unnamed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string",
                        "enum": ["list", "create", "remove", "focus", "move_window"] },
                    "seat": seat,
                    "name": { "type": "string", "minLength": 1,
                        "description": "create, remove, focus: the workspace's name" },
                    "output": { "type": "integer",
                        "description": "create: the output it lives on; the focused one by default" },
                    "window": { "type": "integer", "description": "move_window: the window's id" },
                    "workspace": { "type": "string", "minLength": 1,
                        "description": "move_window: the workspace's name" }
                },
                "required": ["action", "seat"],
                "additionalProperties": false
            }
        },
        {
            "name": "spawn",
            "title": "Programs",
            "description": "Start a program on a seat, list a seat's programs, or stop one. A \
                program gets its seat's Wayland socket, an environment without DISPLAY or \
                D-Bus, and a log file of its output and errors, whose path the result gives; \
                its windows open on its seat. Logs go when the seat closes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["start", "list", "stop"] },
                    "seat": seat,
                    "command": { "type": "array", "items": { "type": "string" }, "minItems": 1,
                        "description": "start: the program, found in PATH, and its arguments" },
                    "env": { "type": "object", "additionalProperties": { "type": "string" },
                        "description": "start: variables added to its environment" },
                    "cwd": { "type": "string", "description": "start: its working folder" },
                    "program": { "type": "integer", "description": "stop: the program's id" },
                    "force": { "type": "boolean",
                        "description": "stop: SIGKILL rather than SIGTERM" }
                },
                "required": ["action", "seat"],
                "additionalProperties": false
            }
        },
        {
            "name": "windows",
            "title": "Windows",
            "description": "List a seat's windows, in the order they opened: id, application \
                id, title, process id, output, workspace, logical rectangle (none when hidden), \
                focus. With wait_ms, waits up to that long for a window that matches the \
                filters, when none does yet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "seat": seat,
                    "app_id": { "type": "string", "description": "only this application id" },
                    "pid": { "type": "integer", "description": "only this client process" },
                    "wait_ms": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT.as_millis(),
                        "description": "how long to wait for a match; 0 by default" }
                },
                "required": ["seat"],
                "additionalProperties": false
            }
        },
        {
            "name": "capture",
            "title": "Capture",
            "description": "Capture what one of a seat's outputs shows, as a PNG image of its \
                newest frame: the one being drawn when a change is on its way, else the last \
                one drawn. All of the output, its first by default, a region of it, or a \
                window: what the output shows where the window is, what covers it included; \
                a hidden window is refused. By default at the logical size, where a pixel is \
                a logical unit, the units pointer takes; size physical gives every physical \
                pixel. The result tells how the image maps to its output, logical = x + pixel \
                × units_per_pixel, and gives a capture id: pointer takes it, with x and y in \
                that image's pixels. One capture of a seat at a time. It comes once the clock \
                reaches the boundary its frame is presented at: on a manual clock, advance it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "seat": seat,
                    "output": { "type": "integer",
                        "description": "the output; the seat's first by default" },
                    "window": { "type": "integer",
                        "description": "a window's id, instead of an output or a region" },
                    "region": {
                        "type": "object",
                        "description": "a rectangle of the output, in whole logical units from \
                            its top-left corner",
                        "properties": {
                            "x": { "type": "integer", "minimum": 0 },
                            "y": { "type": "integer", "minimum": 0 },
                            "width": { "type": "integer", "minimum": 1 },
                            "height": { "type": "integer", "minimum": 1 }
                        },
                        "required": ["x", "y", "width", "height"],
                        "additionalProperties": false
                    },
                    "size": { "type": "string", "enum": ["logical", "physical"],
                        "description": "logical by default" }
                },
                "required": ["seat"],
                "additionalProperties": false
            }
        },
        {
            "name": "pointer",
            "title": "Pointer",
            "description": "Move, press, release, click or scroll a seat's pointer, or read \
                where it is. x and y are logical units from the top-left corner of one of the \
                seat's outputs, its first by default, which are a whole output's capture's \
                pixels at the logical size; with capture, they are pixels of that capture's \
                image, which Yuna maps to its output. press, release, click and scroll move \
                first when x and y are given. A press focuses the window under it. Drag in \
                several calls: press, move, release. The result tells the window and the \
                surface position under the pointer, and the cursor its client asked for.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string",
                        "enum": ["get", "move", "press", "release", "click", "scroll"] },
                    "seat": seat,
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "output": { "type": "integer", "description": "the output x and y are on" },
                    "capture": { "type": "integer",
                        "description": "a capture's id, among the seat's last 16: x and y are \
                            pixels of its image, instead of an output's units" },
                    "button": { "type": "string",
                        "enum": ["left", "right", "middle", "back", "forward"],
                        "description": "press, release, click: left by default" },
                    "dx": { "type": "number", "description": "scroll: wheel steps, right positive" },
                    "dy": { "type": "number", "description": "scroll: wheel steps, down positive" }
                },
                "required": ["action", "seat"],
                "additionalProperties": false
            }
        },
        {
            "name": "keyboard",
            "title": "Keyboard",
            "description": "Press, release or tap keys on a seat's keyboard, type text, or show \
                text being composed, to the window its keyboard is on, or read where it is. Keys \
                are named as XKB names them: Return, a, F5, BackSpace, Left. tap takes keys \
                joined by +, the last pressed while the others are held: ctrl+shift+t; ctrl, \
                shift, alt, super and altgr name modifiers. type types text through the seat's \
                us layout, any character included: one outside it is committed to the text \
                field there when it speaks text-input-v3. preedit shows text being composed in \
                that text field, as an input method does, until the next type replaces it; \
                empty text clears it. The result tells the window the keyboard is on and \
                whether a text field is enabled there. A click focuses a window; the keyboard \
                follows it at the output's next refresh.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string",
                        "enum": ["get", "press", "release", "tap", "type", "preedit"] },
                    "seat": seat,
                    "key": { "type": "string", "minLength": 1,
                        "description": "press, release: the key's name" },
                    "keys": { "type": "string", "minLength": 1,
                        "description": "tap: keys joined by +" },
                    "text": { "type": "string",
                        "description": "type: the text; preedit: the text being composed" }
                },
                "required": ["action", "seat"],
                "additionalProperties": false
            }
        },
        {
            "name": "clock",
            "title": "Clock",
            "description": "Read Yuna's clock, on CLOCK_MONOTONIC's scale, in nanoseconds; or \
                advance it, when Yuna was started with --manual-clock, running every frame due \
                on the way. It never moves past real time. It is the whole Yuna's.",
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

/// `seat`'s arguments.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SeatCall {
    Create {
        width: Option<u32>,
        height: Option<u32>,
        rate: Option<f64>,
        scale: Option<f64>,
    },
    List {},
    Close {
        seat: u64,
    },
}

/// `outputs`' arguments.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputsCall {
    // Braces, so that `deny_unknown_fields` reaches it: serde ignores the fields of a unit
    // variant.
    List {
        seat: u64,
    },
    Create {
        seat: u64,
        width: Option<u32>,
        height: Option<u32>,
        rate: Option<f64>,
        scale: Option<f64>,
    },
    Remove {
        seat: u64,
        output: u64,
    },
}

/// `workspaces`' arguments.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspacesCall {
    List {
        seat: u64,
    },
    Create {
        seat: u64,
        name: String,
        output: Option<u64>,
    },
    Remove {
        seat: u64,
        name: String,
    },
    Focus {
        seat: u64,
        name: String,
    },
    MoveWindow {
        seat: u64,
        window: u64,
        workspace: String,
    },
}

/// `pointer`'s arguments.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum PointerCall {
    Get {
        seat: u64,
    },
    Move {
        seat: u64,
        x: f64,
        y: f64,
        output: Option<u64>,
        capture: Option<u64>,
    },
    Press(ButtonCall),
    Release(ButtonCall),
    Click(ButtonCall),
    Scroll {
        seat: u64,
        #[serde(default)]
        dx: f64,
        #[serde(default)]
        dy: f64,
        x: Option<f64>,
        y: Option<f64>,
        output: Option<u64>,
        capture: Option<u64>,
    },
}

/// A pointer's button call: press, release or click.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ButtonCall {
    pub seat: u64,
    #[serde(default)]
    pub button: Button,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub output: Option<u64>,
    pub capture: Option<u64>,
}

/// A pointer button, by name.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl Button {
    /// Its code, as Linux's `input-event-codes.h` numbers it.
    pub fn code(self) -> u32 {
        match self {
            Button::Left => crate::BUTTON_LEFT,
            Button::Right => crate::BUTTON_RIGHT,
            Button::Middle => crate::BUTTON_MIDDLE,
            Button::Back => crate::BUTTON_BACK,
            Button::Forward => crate::BUTTON_FORWARD,
        }
    }

    /// Its name, as the tools give it.
    pub fn name_of(code: u32) -> Option<&'static str> {
        [
            Button::Left,
            Button::Right,
            Button::Middle,
            Button::Back,
            Button::Forward,
        ]
        .into_iter()
        .find(|button| button.code() == code)
        .map(|button| match button {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
            Button::Back => "back",
            Button::Forward => "forward",
        })
    }
}

/// `keyboard`'s arguments.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum KeyboardCall {
    Get { seat: u64 },
    Press { seat: u64, key: String },
    Release { seat: u64, key: String },
    Tap { seat: u64, keys: String },
    Type { seat: u64, text: String },
    Preedit { seat: u64, text: String },
}

/// `spawn`'s arguments.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpawnCall {
    Start {
        seat: u64,
        command: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    },
    List {
        seat: u64,
    },
    Stop {
        seat: u64,
        program: u64,
        #[serde(default)]
        force: bool,
    },
}

/// `windows`' arguments.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsCall {
    pub seat: u64,
    pub app_id: Option<String>,
    pub pid: Option<i32>,
    #[serde(default)]
    pub wait_ms: u64,
}

impl WindowsCall {
    /// Whether `window` is of the call's seat and passes the filters.
    pub fn matches(&self, window: &WindowDescription) -> bool {
        window.seat.0 == self.seat
            && self
                .app_id
                .as_ref()
                .is_none_or(|app_id| window.app_id.as_ref() == Some(app_id))
            && self.pid.is_none_or(|pid| window.pid == Some(pid))
    }
}

/// `capture`'s arguments.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureCall {
    pub seat: u64,
    pub output: Option<u64>,
    pub window: Option<u64>,
    pub region: Option<RegionArgument>,
    #[serde(default)]
    pub size: SizeArgument,
}

/// A capture's region, in whole logical units.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionArgument {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A capture's size, by name.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeArgument {
    #[default]
    Logical,
    Physical,
}

impl CaptureCall {
    /// What the call captures; refused for a window given with an output or a region.
    pub fn target(&self) -> Result<CaptureTarget, String> {
        match (self.window, self.output, self.region) {
            (Some(window), None, None) => Ok(CaptureTarget::Window(WindowId(window))),
            (Some(_), _, _) => Err("give a window, or an output and a region, not both".to_owned()),
            (None, output, region) => Ok(CaptureTarget::Output {
                output: output.map(OutputId),
                region: region.map(|r| LogicalRegion {
                    x: r.x,
                    y: r.y,
                    width: r.width,
                    height: r.height,
                }),
            }),
        }
    }

    /// The image's size.
    pub fn size(&self) -> CaptureSize {
        match self.size {
            SizeArgument::Logical => CaptureSize::Logical,
            SizeArgument::Physical => CaptureSize::Physical,
        }
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
    /// A PNG image in base64, given after the text.
    image: Option<String>,
}

impl ToolResult {
    /// A result.
    pub fn done(structured: Value) -> ToolResult {
        ToolResult {
            text: structured.to_string(),
            structured: Some(structured),
            image: None,
        }
    }

    /// A result with a PNG image, in base64.
    pub fn done_with_image(structured: Value, png_base64: String) -> ToolResult {
        ToolResult {
            image: Some(png_base64),
            ..ToolResult::done(structured)
        }
    }

    /// A failure, with what went wrong.
    pub fn failed(reason: impl Into<String>) -> ToolResult {
        ToolResult {
            structured: None,
            text: reason.into(),
            image: None,
        }
    }

    /// `tools/call`'s result, shaped for `revision`.
    pub fn into_result(self, revision: Revision) -> Value {
        let mut body = Map::new();
        let mut content = vec![json!({ "type": "text", "text": self.text })];
        if let Some(data) = self.image {
            content.push(json!({ "type": "image", "data": data, "mimeType": "image/png" }));
        }
        body.insert("content".to_owned(), Value::Array(content));
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
        "seat": output.seat.0,
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
        "seat": window.seat.0,
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

/// A pointer, as the tools give it.
pub fn pointer_json(pointer: &PointerDescription) -> Value {
    json!({
        "output": pointer.output.map(|output| output.0),
        "x": pointer.x,
        "y": pointer.y,
        "window": pointer.window.map(|window| window.0),
        "surface_position": pointer.surface_position.map(|(x, y)| json!({ "x": x, "y": y })),
        "cursor": pointer.cursor,
        "buttons": pointer
            .buttons
            .iter()
            .map(|&code| Button::name_of(code).map_or_else(|| json!(code), |name| json!(name)))
            .collect::<Vec<Value>>(),
    })
}

/// What a capture's result tells of its frame, kept while its image is encoded.
#[derive(Debug, Clone, Copy)]
pub struct CopiedCapture {
    pub mapping: CaptureMapping,
    pub window: Option<WindowId>,
    pub boundary: u64,
    pub presented_at: Duration,
}

impl CopiedCapture {
    pub fn of(frame: &CapturedFrame) -> CopiedCapture {
        CopiedCapture {
            mapping: frame.mapping,
            window: frame.window,
            boundary: frame.boundary,
            presented_at: frame.presented_at,
        }
    }
}

/// A capture, as `capture` gives it: its id, and how its image maps to its output.
pub fn capture_json(id: u64, frame: &CopiedCapture, png_bytes: usize) -> Value {
    let mapping = &frame.mapping;
    json!({
        "capture": id,
        "output": mapping.output.0,
        "window": frame.window.map(|window| window.0),
        "width": mapping.width,
        "height": mapping.height,
        "x": mapping.x,
        "y": mapping.y,
        "units_per_pixel": mapping.units_per_pixel,
        "presented_ns": frame.presented_at.as_nanos() as u64,
        "boundary": frame.boundary,
        "png_bytes": png_bytes,
    })
}

/// A workspace, as the tools give it.
pub fn workspace_json(workspace: &WorkspaceInfo) -> Value {
    json!({
        "name": workspace.name,
        "shown": workspace.shown,
        "windows": workspace.windows.iter().map(|window| window.0).collect::<Vec<u64>>(),
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
        "seat": program.seat.0,
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
        assert_eq!(
            names,
            [
                "seat",
                "outputs",
                "workspaces",
                "spawn",
                "windows",
                "capture",
                "pointer",
                "keyboard",
                "clock"
            ]
        );
        for tool in tools.as_array().into_iter().flatten() {
            assert_eq!(tool["inputSchema"]["type"], json!("object"), "{tool}");
            assert_eq!(tool["inputSchema"]["additionalProperties"], json!(false));
        }
    }

    #[test]
    fn each_property_a_schema_names_is_read_by_its_arguments() {
        // Every property of each schema, given alone with a valid action, is accepted or
        // refused for its action, never as an unknown field.
        assert!(matches!(
            arguments(args(json!({
                "action": "create", "width": 1920, "height": 1080, "rate": 60, "scale": 1
            }))),
            Ok(SeatCall::Create {
                width: Some(1920),
                ..
            })
        ));
        assert!(matches!(
            arguments(args(json!({"action": "close", "seat": 2}))),
            Ok(SeatCall::Close { seat: 2 })
        ));
        let outputs: Result<OutputsCall, _> = arguments(args(json!({
            "action": "create", "seat": 2, "width": 1920, "height": 1080, "rate": 60, "scale": 1
        })));
        assert!(matches!(
            outputs,
            Ok(OutputsCall::Create {
                width: Some(1920),
                ..
            })
        ));
        assert!(matches!(
            arguments(args(json!({"action": "remove", "seat": 2, "output": 2}))),
            Ok(OutputsCall::Remove { output: 2, .. })
        ));
        assert!(matches!(
            arguments(args(
                json!({"action": "create", "seat": 2, "name": "web", "output": 3})
            )),
            Ok(WorkspacesCall::Create {
                output: Some(3),
                ..
            })
        ));
        assert!(matches!(
            arguments(args(json!({
                "action": "move_window", "seat": 2, "window": 1, "workspace": "b"
            }))),
            Ok(WorkspacesCall::MoveWindow { window: 1, .. })
        ));
        let spawn: Result<SpawnCall, _> = arguments(args(json!({
            "action": "start", "seat": 2, "command": ["vkcube"], "env": {"A": "b"}, "cwd": "/tmp"
        })));
        assert!(matches!(spawn, Ok(SpawnCall::Start { .. })));
        assert!(matches!(
            arguments(args(
                json!({"action": "stop", "seat": 2, "program": 1, "force": true})
            )),
            Ok(SpawnCall::Stop {
                program: 1,
                force: true,
                ..
            })
        ));
        let windows: Result<WindowsCall, _> = arguments(args(
            json!({"seat": 2, "app_id": "a", "pid": 3, "wait_ms": 10}),
        ));
        assert!(windows.is_ok());
        assert!(matches!(
            arguments(args(json!({
                "action": "click", "seat": 2, "button": "right", "x": 1, "y": 2, "output": 3
            }))),
            Ok(PointerCall::Click(ButtonCall {
                button: Button::Right,
                x: Some(_),
                ..
            }))
        ));
        assert!(matches!(
            arguments(args(
                json!({"action": "scroll", "seat": 2, "dx": 1, "dy": -2})
            )),
            Ok(PointerCall::Scroll { .. })
        ));
        assert!(matches!(
            arguments(args(json!({"action": "press", "seat": 2}))),
            Ok(PointerCall::Press(ButtonCall {
                button: Button::Left,
                ..
            }))
        ));
        assert!(matches!(
            arguments(args(json!({"action": "tap", "seat": 2, "keys": "ctrl+t"}))),
            Ok(KeyboardCall::Tap { .. })
        ));
        assert!(matches!(
            arguments(args(json!({"action": "type", "seat": 2, "text": "é"}))),
            Ok(KeyboardCall::Type { .. })
        ));
        assert!(matches!(
            arguments(args(
                json!({"action": "preedit", "seat": 2, "text": "にほ"})
            )),
            Ok(KeyboardCall::Preedit { .. })
        ));
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
        assert!(
            arguments::<OutputsCall>(args(json!({"action": "list", "seat": 2, "x": 1}))).is_err()
        );
        // Every tool but `clock` names its seat.
        assert!(arguments::<OutputsCall>(args(json!({"action": "list"}))).is_err());
        assert!(arguments::<WindowsCall>(args(json!({}))).is_err());
        assert!(
            arguments::<PointerCall>(args(json!({"action": "click", "seat": 2, "z": 1}))).is_err()
        );
        assert!(
            arguments::<PointerCall>(args(json!({"action": "click", "seat": 2, "button": "x"})))
                .is_err()
        );
        assert!(arguments::<KeyboardCall>(args(json!({"action": "tap", "seat": 2}))).is_err());
        assert!(arguments::<KeyboardCall>(args(json!({"action": "preedit", "seat": 2}))).is_err());
        assert!(arguments::<OutputsCall>(args(json!({"action": "rotate"}))).is_err());
        assert!(arguments::<OutputsCall>(args(json!({}))).is_err());
        assert!(arguments::<OutputsCall>(args(json!({"action": "create", "width": -1}))).is_err());
        assert!(arguments::<WindowsCall>(args(json!({"seat": 2, "wait": 1}))).is_err());
        assert!(arguments::<SpawnCall>(args(json!({"action": "start", "seat": 2}))).is_err());
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
