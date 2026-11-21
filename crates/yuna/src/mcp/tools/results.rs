//! A tool's result, [`ToolResult`], and how each thing a tool tells is given as JSON: an output,
//! a window and its surfaces' colours, a pointer, a capture, a workspace, a program and its log,
//! an event, what the rest audit counted, a selection's data.
//!
//! Its neighbours: the server's modules make the results (`../mod.rs` names them);
//! `protocol.rs` shapes each for its revision.

use std::time::Duration;

use serde_json::{Map, Value, json};
use yuna_layout::WorkspaceInfo;
use yuna_layout::geometry::Rect;
use yuna_types::{SeatId, WindowId};

use super::arguments::Button;
use crate::colour::SurfaceColourDescription;
use crate::colour::description::Chromaticities;
use crate::compositor::{ClientProcess, ClientProtocolError, OutputDescription, WindowDescription};
use crate::input::PointerDescription;
use crate::mcp::events::{Event, EventData, Reading};
use crate::mcp::image::base64;
use crate::mcp::programs::{End, LogRead, Program, ProgramWakes};
use crate::mcp::protocol::{self, Revision};
use crate::rest::{Cause, RestCounts, Source};
use crate::{CaptureMapping, CapturedFrame};

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

/// An output, as the tools give it: its VRR range in Hz and the name of the monitor whose HDR
/// colours it announces, each null when it has none.
pub fn output_json(output: &OutputDescription) -> Value {
    let mode = &output.mode;
    json!({
        "id": output.id.0,
        "seat": output.seat.0,
        "name": output.name,
        "width": mode.width(),
        "height": mode.height(),
        "refresh_mhz": mode.refresh_millihertz(),
        "scale": mode.scale(),
        "vrr": mode.vrr_min().map(|min| json!({
            "min": min, "max": mode.rate(),
        })),
        "hdr": mode.hdr().map(|monitor| monitor.name),
        "logical": {
            "x": output.rect.x, "y": output.rect.y,
            "width": output.rect.w, "height": output.rect.h,
        },
    })
}

/// A window, as the tools give it; with `colours`, those its surfaces declared, which `windows`
/// gives only when asked, so that its reply does not grow for agents that do not ask.
pub fn window_json(
    window: &WindowDescription,
    colours: Option<&[SurfaceColourDescription]>,
) -> Value {
    let mut described = json!({
        "id": window.id.0,
        "seat": window.seat.0,
        "shell": window.shell.name(),
        "app_id": window.app_id,
        "title": window.title,
        "pid": window.pid,
        "output": window.output.map(|output| output.0),
        "workspace": window.workspace,
        "rect": window.rect.map(rect_json),
        "place": window.place.map(rect_json),
        "motion_ends_ns": window.motion_ends.map(|at| at.as_nanos() as u64),
        "focused": window.focused,
        "floating": window.floating,
        "fullscreen": window.fullscreen,
        "unresponsive": window.unresponsive,
    });
    if let Some(colours) = colours {
        described["colours"] = colours.iter().map(surface_colour_json).collect();
    }
    described
}

/// A rectangle in logical units.
fn rect_json(rect: Rect) -> Value {
    json!({ "x": rect.x, "y": rect.y, "width": rect.w, "height": rect.h })
}

/// A surface's declared colours, in the protocol's names, its luminances in cd/m² and its
/// chromaticities as numbers (ADR-0049).
fn surface_colour_json(surface: &SurfaceColourDescription) -> Value {
    let description = &surface.description;
    let (target_min, target_max) = description.target_luminance;
    json!({
        "role": surface.role.name(),
        "transfer_function": description.transfer_function.name(),
        "primaries": description.named_primaries.map_or_else(
            || chromaticities_json(&description.primaries),
            |named| json!(named.name()),
        ),
        "luminances": {
            "min": candelas_of_min(description.luminances.min),
            "max": description.luminances.max,
            "reference": description.luminances.reference,
        },
        "target_primaries": chromaticities_json(&description.target_primaries),
        "target_luminance": { "min": candelas_of_min(target_min), "max": target_max },
        "max_cll": description.max_cll,
        "max_fall": description.max_fall,
        // The one intent Yuna lists (ADR-0047): every description is set with it.
        "intent": "perceptual",
    })
}

/// A minimum luminance in cd/m², from the protocol's ten-thousandths.
fn candelas_of_min(ten_thousandths: u32) -> f64 {
    f64::from(ten_thousandths) / 10_000.0
}

/// Chromaticities as `{"red": [x, y], …}`, from the protocol's millionths.
fn chromaticities_json(chromaticities: &Chromaticities) -> Value {
    let xy = |(x, y): (i32, i32)| json!([f64::from(x) / 1e6, f64::from(y) / 1e6]);
    json!({
        "red": xy(chromaticities.red),
        "green": xy(chromaticities.green),
        "blue": xy(chromaticities.blue),
        "white": xy(chromaticities.white),
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

/// How a program ended, as `exit_code` and `signal`, one of them null.
fn end_json(end: End) -> (Option<i32>, Option<i32>) {
    match end {
        End::Exited(code) => (Some(code), None),
        End::Killed(signal) => (None, Some(signal)),
    }
}

/// A program, as the tools give it.
pub fn program_json(program: &Program) -> Value {
    let state = match program.end {
        None => "running",
        Some(End::Exited(_)) => "exited",
        Some(End::Killed(_)) => "killed",
    };
    let (exit_code, signal) = program.end.map_or((None, None), end_json);
    json!({
        "id": program.id,
        "seat": program.seat.0,
        "pid": program.pid.as_raw_nonzero().get(),
        "command": program.command,
        "accessibility": program.accessible,
        "state": state,
        "group_running": program.group_runs(),
        "exit_code": exit_code,
        "signal": signal,
        "run_percent": program.pace().run_percent(),
        "protocol_error": program.protocol_error.as_ref().map(protocol_error_json),
        "log": program.log,
        "log_bytes": program.log_kept(),
        "log_dropped": program.log_dropped(),
    })
}

/// A protocol error a client was ended for.
fn protocol_error_json(error: &ClientProtocolError) -> Value {
    json!({
        "interface": error.interface,
        "object": error.object,
        "code": error.code,
        "message": error.message,
    })
}

/// A read of a program's log, as `spawn log` gives it.
pub fn log_json(read: &LogRead) -> Value {
    json!({
        "text": read.text,
        "from": read.from,
        "to": read.to,
        "size": read.size,
        "dropped": read.dropped,
        "ended": read.ended,
    })
}

/// An event, as `events` gives it: its number, time and kind, and its fields.
pub fn event_json(event: &Event) -> Value {
    let mut described = match &event.data {
        EventData::WindowOpened {
            window,
            app_id,
            title,
            pid,
            shell,
        } => json!({
            "window": window.0, "app_id": app_id, "title": title, "pid": pid,
            "shell": shell.name(),
        }),
        EventData::WindowClosed { window } | EventData::WindowUnresponsive { window } => {
            json!({ "window": window.0 })
        }
        EventData::WindowChanged {
            window,
            title,
            app_id,
        } => json!({ "window": window.0, "title": title, "app_id": app_id }),
        EventData::WindowPlaced {
            window,
            output,
            workspace,
            place,
        } => json!({
            "window": window.0,
            "output": output.map(|output| output.0),
            "workspace": workspace,
            "place": place.map(rect_json),
        }),
        EventData::Focus { window } => json!({ "window": window.map(|window| window.0) }),
        EventData::ProgramExited { program, end } => {
            let (exit_code, signal) = end_json(*end);
            json!({ "program": program, "exit_code": exit_code, "signal": signal })
        }
        EventData::ClientLeft {
            pid,
            program,
            error,
        } => json!({
            "pid": pid,
            "program": program,
            "reason": if error.is_some() { "protocol_error" } else { "closed" },
            "error": error.as_ref().map(protocol_error_json),
        }),
        EventData::ClientRefused { bound, refused } => {
            json!({ "bound": bound, "refused": refused })
        }
        EventData::OutputAdded { output }
        | EventData::OutputRemoved { output }
        | EventData::OutputChanged { output } => json!({ "output": output.0 }),
    };
    described["seq"] = json!(event.seq);
    described["time_ns"] = json!(event.time_ns);
    described["kind"] = json!(event.data.kind().name());
    described
}

/// What `counts` holds, as `rest` answers: of `seat` and Yuna's own parts when given, else of
/// every seat; each program of `programs` that woke Yuna, and each client's program, as
/// `program_of` names it. Only what woke Yuna or drew is listed.
pub fn rest_json(
    counts: &RestCounts,
    seat: Option<SeatId>,
    wait_ms: Option<u64>,
    programs: &[ProgramWakes],
    program_of: &dyn Fn(SeatId, &ClientProcess) -> Option<u64>,
) -> Value {
    let seats_tally = seat.map(|seat| counts.seats.get(&seat).cloned().unwrap_or_default());
    let scope = seats_tally.as_ref().unwrap_or(&counts.whole);
    let wakes_by = |source: Source| {
        if source.serves_a_seat() {
            scope.wakes_by(source)
        } else {
            counts.whole.wakes_by(source)
        }
    };
    let ours = |of: SeatId| seat.is_none_or(|seat| seat == of);
    let sources: Vec<Value> = Source::ALL
        .iter()
        .filter_map(|&source| {
            let wakes = wakes_by(source);
            (wakes > 0).then(|| json!({ "source": source.name(), "wakes": wakes }))
        })
        .collect();
    let clients: Vec<Value> = counts
        .clients
        .values()
        .filter(|client| ours(client.seat) && !client.tally.is_empty())
        .map(|client| {
            json!({
                "pid": client.process.pid,
                "program": program_of(client.seat, &client.process),
                "seat": client.seat.0,
                "xwayland": client.xwayland,
                "wakes": client.tally.wakes,
                "commits": client.tally.commits,
                "draws": client.tally.draws,
            })
        })
        .collect();
    let programs: Vec<Value> = programs
        .iter()
        .filter(|program| ours(program.seat) && (program.output > 0 || program.pace > 0))
        .map(|program| {
            json!({
                "program": program.program,
                "seat": program.seat.0,
                "output_wakes": program.output,
                "pace_wakes": program.pace,
            })
        })
        .collect();
    let draw_causes: Vec<Value> = Cause::ALL
        .iter()
        .filter_map(|&cause| {
            let frames = scope.frames_by(cause);
            (frames > 0).then(|| json!({ "cause": cause.name(), "frames": frames }))
        })
        .collect();
    json!({
        "seat": seat.map(|seat| seat.0),
        "since_ns": counts.since.as_nanos() as u64,
        "now_ns": counts.at.as_nanos() as u64,
        "wait_ms": wait_ms,
        "wakes": counts.woke,
        "sources": sources,
        "clients": clients,
        "clients_gone": {
            "clients": scope.gone_clients,
            "wakes": scope.gone.wakes,
            "commits": scope.gone.commits,
            "draws": scope.gone.draws,
        },
        "unnamed_client_wakes": counts.unnamed_client_wakes,
        "programs": programs,
        "frames": scope.frames,
        "draw_causes": draw_causes,
    })
}

/// What `events` answers: the events read, the cursor to give next, the events the log dropped
/// after the cursor, and, when its wait ran out, `timed_out`.
pub fn events_json(reading: &Reading<'_>, timed_out: bool) -> Value {
    let mut answer = json!({
        "events": reading.events.iter().map(|event| event_json(event)).collect::<Vec<Value>>(),
        "next": reading.next,
        "missed": reading.missed,
    });
    if timed_out {
        answer["timed_out"] = json!(true);
    }
    answer
}

/// What `clipboard get` read of its selection, as one of its types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardData {
    pub mime: String,
    pub bytes: Vec<u8>,
    /// Whether the selection's source had more than was read.
    pub truncated: bool,
    /// Whether its source had not closed its pipe within
    /// [`CLIPBOARD_TIMEOUT`](crate::CLIPBOARD_TIMEOUT).
    pub timed_out: bool,
}

/// `clipboard get`'s result: the MIME types the selection offers and, when one was read, its
/// data, as text when it is UTF-8 with no control character but tab, newline and carriage
/// return, else in base64 with `base64` true, and its length in bytes. Data cut inside a
/// character, by `max_bytes` or the time limit, is text up to that character. JSON writes each
/// other control character in six bytes, `\u00XX`, and the reply gives the data twice, the
/// second escaped again: as text, a byte of them would take 13 in the reply, against 8/3 in
/// base64 ([`MAX_CLIPBOARD_BYTES`](super::MAX_CLIPBOARD_BYTES)).
pub fn clipboard_json(mime_types: &[String], data: Option<&ClipboardData>) -> Value {
    let Some(data) = data else {
        return json!({
            "mime_types": mime_types,
            "mime": null,
            "data": null,
            "base64": false,
            "bytes": 0,
            "truncated": false,
            "timed_out": false,
        });
    };
    let cut = data.truncated || data.timed_out;
    let text = match std::str::from_utf8(&data.bytes) {
        Ok(text) => Some(text),
        // An incomplete character at the end alone: the rest is whole text.
        Err(error) if cut && error.error_len().is_none() => {
            std::str::from_utf8(&data.bytes[..error.valid_up_to()]).ok()
        }
        Err(_) => None,
    };
    let text = text.filter(|text| {
        !text
            .bytes()
            .any(|byte| byte < 0x20 && !matches!(byte, b'\t' | b'\n' | b'\r'))
    });
    let (given, bytes, in_base64) = match text {
        Some(text) => (text.to_owned(), text.len(), false),
        None => (base64(&data.bytes), data.bytes.len(), true),
    };
    json!({
        "mime_types": mime_types,
        "mime": data.mime,
        "data": given,
        "base64": in_base64,
        "bytes": bytes,
        "truncated": data.truncated,
        "timed_out": data.timed_out,
    })
}
