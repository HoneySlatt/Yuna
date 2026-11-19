//! Each tool's arguments, read with serde from a call's `arguments`: an enum tagged by `action`
//! for a tool that does several things, a struct otherwise, each refusing a field it does not
//! know; and the checks a value needs beyond its type.
//!
//! Its neighbours: `schemas.rs` names the same properties to the client; the server's modules
//! act on what is read here (`../mod.rs` names them).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use yuna_layout::{OutputId, WindowId};

use super::{DEFAULT_CLIPBOARD_BYTES, MAX_CLIPBOARD_BYTES, MAX_WORKSPACE_NAME, TEXT_TYPES};
use crate::compositor::WindowDescription;
use crate::mcp::events::Kind;
use crate::xwayland::Selection;
use crate::{CaptureSize, CaptureTarget, LogicalRegion};

/// `seat`'s arguments.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SeatCall {
    Create {
        width: Option<u32>,
        height: Option<u32>,
        rate: Option<f64>,
        scale: Option<f64>,
        vrr_min: Option<f64>,
        hdr: Option<bool>,
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
        vrr_min: Option<f64>,
        hdr: Option<bool>,
    },
    Configure {
        seat: u64,
        output: u64,
        width: Option<u32>,
        height: Option<u32>,
        rate: Option<f64>,
        scale: Option<f64>,
        vrr_min: Option<f64>,
        hdr: Option<bool>,
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

impl WorkspacesCall {
    /// Refuses a workspace's name past [`MAX_WORKSPACE_NAME`] bytes, whichever action names it.
    pub fn check_name(&self) -> Result<(), String> {
        let name = match self {
            WorkspacesCall::List { .. } => return Ok(()),
            WorkspacesCall::Create { name, .. }
            | WorkspacesCall::Remove { name, .. }
            | WorkspacesCall::Focus { name, .. } => name,
            WorkspacesCall::MoveWindow { workspace, .. } => workspace,
        };
        if name.len() > MAX_WORKSPACE_NAME {
            return Err(format!(
                "a workspace's name is {MAX_WORKSPACE_NAME} bytes at most in UTF-8: this one \
                 takes {}",
                name.len()
            ));
        }
        Ok(())
    }
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
    Get {
        seat: u64,
    },
    Press {
        seat: u64,
        key: String,
    },
    Release {
        seat: u64,
        key: String,
    },
    Tap {
        seat: u64,
        keys: String,
    },
    Type {
        seat: u64,
        text: String,
    },
    Preedit {
        seat: u64,
        text: String,
    },
    Layout {
        seat: u64,
        layout: String,
        variant: Option<String>,
        options: Option<String>,
    },
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
        /// Whether it joins its seat's accessibility bus.
        #[serde(default)]
        accessibility: bool,
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
    Log {
        seat: u64,
        program: u64,
        from: Option<u64>,
        max_bytes: Option<u64>,
    },
    Freeze {
        seat: u64,
        program: u64,
    },
    Thaw {
        seat: u64,
        program: u64,
    },
    Slow {
        seat: u64,
        program: u64,
        run_percent: u8,
    },
}

/// `rest`'s arguments.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestCall {
    pub seat: Option<u64>,
    pub wait_ms: Option<u64>,
}

/// `events`' arguments.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventsCall {
    pub seat: u64,
    pub after: Option<u64>,
    pub kinds: Option<Vec<Kind>>,
    #[serde(default)]
    pub wait_ms: u64,
}

/// `windows`' arguments.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsCall {
    pub seat: u64,
    pub app_id: Option<String>,
    pub pid: Option<i32>,
    /// Whether each window also gives its surfaces' colours.
    #[serde(default)]
    pub colour: bool,
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

/// `accessibility`'s arguments.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessibilityCall {
    pub seat: u64,
    pub window: Option<u64>,
    pub pid: Option<i32>,
    pub name: Option<String>,
    pub role: Option<String>,
    #[serde(default)]
    pub states: Vec<String>,
    pub max_nodes: Option<u64>,
    #[serde(default)]
    pub wait_ms: u64,
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

/// `clipboard`'s arguments.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClipboardCall {
    Get {
        seat: u64,
        #[serde(default)]
        selection: SelectionArgument,
        mime: Option<String>,
        max_bytes: Option<u64>,
    },
    Set {
        seat: u64,
        #[serde(default)]
        selection: SelectionArgument,
        text: String,
    },
    Clear {
        seat: u64,
        #[serde(default)]
        selection: SelectionArgument,
    },
}

impl ClipboardCall {
    /// The seat it acts on.
    pub fn seat(&self) -> u64 {
        match self {
            ClipboardCall::Get { seat, .. }
            | ClipboardCall::Set { seat, .. }
            | ClipboardCall::Clear { seat, .. } => *seat,
        }
    }

    /// Which of the seat's selections it acts on.
    pub fn selection(&self) -> Selection {
        let (ClipboardCall::Get { selection, .. }
        | ClipboardCall::Set { selection, .. }
        | ClipboardCall::Clear { selection, .. }) = self;
        match selection {
            SelectionArgument::Clipboard => Selection::Clipboard,
            SelectionArgument::Primary => Selection::Primary,
        }
    }
}

/// One of a seat's selections, by name.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionArgument {
    #[default]
    Clipboard,
    Primary,
}

/// `max_bytes` of `clipboard get`, [`DEFAULT_CLIPBOARD_BYTES`] when left out; refused outside 1
/// to [`MAX_CLIPBOARD_BYTES`].
pub fn clipboard_bytes(max_bytes: Option<u64>) -> Result<usize, String> {
    match max_bytes {
        None => Ok(DEFAULT_CLIPBOARD_BYTES),
        Some(bytes) => usize::try_from(bytes)
            .ok()
            .filter(|bytes| (1..=MAX_CLIPBOARD_BYTES).contains(bytes))
            .ok_or_else(|| format!("max_bytes is from 1 to {MAX_CLIPBOARD_BYTES}")),
    }
}

/// The text type `clipboard get` reads when it names none: the first of [`TEXT_TYPES`] that
/// `mime_types` holds.
pub fn first_text_type(mime_types: &[String]) -> Option<String> {
    TEXT_TYPES
        .into_iter()
        .find(|text| mime_types.iter().any(|mime_type| mime_type == text))
        .map(str::to_owned)
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
