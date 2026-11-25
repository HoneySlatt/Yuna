//! A tool's result, [`ToolResult`], and how each thing a tool tells is given as JSON: an output,
//! a window and its surfaces' colours, a pointer, a capture, a workspace, a program and its log,
//! an event, what the rest audit counted, a selection's data.
//!
//! Its neighbours: the server's modules make the results (`../mod.rs` names them);
//! `protocol.rs` shapes each for its revision.

use std::time::Duration;

use serde_json::{Map, Value, json};
use yuna_ipc::ProtocolError;
use yuna_layout::WorkspaceInfo;
use yuna_types::{Rect, SeatId, WindowId};

use super::arguments::Button;
use crate::colour::SurfaceColourDescription;
use crate::colour::description::Chromaticities;
use crate::compositor::{OutputDescription, WindowDescription};
use crate::frame_record::{
    Boundary, Commit, Frame, FrameOutcome, Input, InputDevice, Reading as FramesReading,
};
use crate::input::PointerDescription;
use crate::mcp::events::{Event, EventData, Reading};
use crate::mcp::image::base64;
use crate::mcp::programs::{End, LogRead, Program, ProgramWakes};
use crate::mcp::protocol::{self, Revision};
use crate::rest::{Cause, RestCounts, Source};
use crate::{CaptureMapping, CapturedFrame, GpuPriority};

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
    pub frame: Option<crate::FrameId>,
}

impl CopiedCapture {
    pub fn of(frame: &CapturedFrame) -> CopiedCapture {
        CopiedCapture {
            mapping: frame.mapping,
            window: frame.window,
            boundary: frame.boundary,
            presented_at: frame.presented_at,
            frame: frame.frame,
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
        "frame": frame.frame.map(|id| id.get()),
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
fn protocol_error_json(error: &ProtocolError) -> Value {
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
        EventData::Compositor { event, program } => compositor_event_json(event, *program),
        EventData::ProgramExited { program, end } => {
            let (exit_code, signal) = end_json(*end);
            json!({ "program": program, "exit_code": exit_code, "signal": signal })
        }
    };
    described["seq"] = json!(event.seq);
    described["time_ns"] = json!(event.time_ns);
    described["kind"] = json!(event.kind.name());
    described
}

/// The fields of a compositor's event; `program`, a `client_left`'s client's. A kind MCP does
/// not know is never recorded (`Kind::of`): it has no field here.
fn compositor_event_json(event: &yuna_ipc::Event, program: Option<u64>) -> Value {
    use yuna_ipc::Event;
    match event {
        Event::WindowOpened {
            window,
            app_id,
            title,
            pid,
            shell,
            ..
        } => json!({
            "window": window.0, "app_id": app_id, "title": title, "pid": pid,
            "shell": shell.name(),
        }),
        Event::WindowClosed { window, .. } | Event::WindowUnresponsive { window, .. } => {
            json!({ "window": window.0 })
        }
        Event::WindowChanged {
            window,
            title,
            app_id,
            ..
        } => json!({ "window": window.0, "title": title, "app_id": app_id }),
        Event::WindowPlaced {
            window,
            output,
            workspace,
            place,
            ..
        } => json!({
            "window": window.0,
            "output": output.map(|output| output.0),
            "workspace": workspace,
            "place": place.map(rect_json),
        }),
        Event::Focus { window, .. } => json!({ "window": window.map(|window| window.0) }),
        Event::ClientLeft { pid, error, .. } => json!({
            "pid": pid,
            "program": program,
            "reason": if error.is_some() { "protocol_error" } else { "closed" },
            "error": error.as_ref().map(protocol_error_json),
        }),
        Event::ClientRefused { bound, refused, .. } => {
            json!({ "bound": bound, "refused": refused })
        }
        Event::OutputAdded { output, .. }
        | Event::OutputRemoved { output, .. }
        | Event::OutputChanged { output, .. } => json!({ "output": output.0 }),
        _ => json!({}),
    }
}

/// What `counts` holds, as `rest` answers: of `seat` and Yuna's own parts when given, else of
/// every seat; each program of `programs` that woke Yuna, and each client's program, as
/// `program_of` names it from the client's number. Only what woke Yuna or drew is listed.
pub fn rest_json(
    counts: &RestCounts,
    seat: Option<SeatId>,
    wait_ms: Option<u64>,
    programs: &[ProgramWakes],
    program_of: &dyn Fn(u64) -> Option<u64>,
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
        .iter()
        .filter(|(_, client)| ours(client.seat) && !client.tally.is_empty())
        .map(|(&number, client)| {
            json!({
                "pid": client.process.pid,
                "program": program_of(number),
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

/// What `frames` answers: the frames read, oldest first, each with the inputs and commits it
/// covers, [`MAX_LISTED_PER_REPLY`](super::MAX_LISTED_PER_REPLY) of them at most in all; the
/// cursor to give next; the frames the seat's ring dropped after the cursor; and the GPU priority
/// granted. A frame past the listing's room waits for the next reply, its cursor the last frame
/// given; the first is given always, its lists cut. `now` is the clock's: a presentation is
/// told once it has passed.
pub fn frames_json(reading: &FramesReading<'_>, now: Duration, priority: GpuPriority) -> Value {
    let mut room = super::MAX_LISTED_PER_REPLY;
    let mut frames = Vec::new();
    let mut next = reading.next;
    for (given, (seq, frame)) in reading.frames.iter().enumerate() {
        if room == 0 && given > 0 {
            next = reading.frames[given - 1].0;
            break;
        }
        frames.push(frame_json(reading, *seq, frame, now, &mut room));
    }
    json!({
        "frames": frames,
        "next": next,
        "missed": reading.missed,
        "gpu_priority": priority.name(),
    })
}

/// A frame, as `frames` tells it, its inputs and commits listed within `room`, which it lessens.
fn frame_json(
    reading: &FramesReading<'_>,
    seq: u64,
    frame: &Frame,
    now: Duration,
    room: &mut usize,
) -> Value {
    let nanos = |at: Duration| u64::try_from(at.as_nanos()).unwrap_or(u64::MAX);
    let boundary = |at: Boundary| json!({ "boundary": at.seq, "at_ns": nanos(at.at) });
    let (outcome, damaged) = match frame.outcome {
        FrameOutcome::Drawn { damaged } => ("drawn", Some(damaged)),
        FrameOutcome::Unchanged => ("unchanged", None),
        FrameOutcome::Failed => ("failed", None),
    };
    let inputs = listed(reading.inputs(frame), room, |(number, input)| {
        input_json(number, input, nanos)
    });
    // Its own output's commits, and those shown on none, a window's first among them; those
    // another output showed are counted.
    let on_frame = |commit: &&Commit| commit.output.is_none_or(|output| output == frame.output);
    let elsewhere = reading
        .commits(frame)
        .filter(|(_, commit)| !on_frame(commit))
        .count();
    let commits = listed(
        reading
            .commits(frame)
            .filter(|(_, commit)| on_frame(commit)),
        room,
        |(number, commit)| commit_json(number, commit, nanos),
    );
    json!({
        "seq": seq,
        "id": frame.id.get(),
        "output": frame.output.0,
        "outcome": outcome,
        "damaged": damaged,
        "causes": frame.causes.iter().map(Cause::name).collect::<Vec<&str>>(),
        "armed_ns": nanos(frame.armed_at),
        "due_ns": nanos(frame.due_at),
        "ran_ns": nanos(frame.ran_at),
        "ended_ns": nanos(frame.ended_at),
        "took_us": u64::try_from(frame.took.as_micros()).unwrap_or(u64::MAX),
        "sampled_ns": nanos(frame.sampled_at),
        "aimed": boundary(frame.aimed),
        "presented": frame
            .presented
            .filter(|presented| presented.at <= now)
            .map(boundary),
        "late": frame.late(),
        "discarded": frame.discarded,
        "inputs": inputs.listed,
        "inputs_more": inputs.more,
        "inputs_dropped": frame.inputs.count().saturating_sub(inputs.kept),
        "commits": commits.listed,
        "commits_more": commits.more,
        "commits_dropped": frame.commits.count().saturating_sub(commits.kept + elsewhere as u64),
        "commits_elsewhere": elsewhere,
    })
}

/// Entries of a frame's list: those given, within the reply's room, how many more there are,
/// and how many the ring still keeps.
struct Listed {
    listed: Vec<Value>,
    more: u64,
    kept: u64,
}

/// Lists `entries` within `room`, each made by `json`, lessening `room` by those given.
fn listed<T>(
    entries: impl Iterator<Item = T>,
    room: &mut usize,
    json: impl Fn(T) -> Value,
) -> Listed {
    let mut list = Listed {
        listed: Vec::new(),
        more: 0,
        kept: 0,
    };
    for entry in entries {
        list.kept += 1;
        if *room == 0 {
            list.more += 1;
            continue;
        }
        *room -= 1;
        list.listed.push(json(entry));
    }
    list
}

/// An input, as `frames` tells it.
fn input_json(number: u64, input: &Input, nanos: impl Fn(Duration) -> u64) -> Value {
    let (device, request, client) = match input.device {
        InputDevice::Interface => ("interface", None, None),
        InputDevice::Yuna => ("yuna", None, None),
        InputDevice::Agent { request } => ("agent", request, None),
        InputDevice::VirtualKeyboard { client } => ("virtual_keyboard", None, Some(client)),
        InputDevice::VirtualPointer { client } => ("virtual_pointer", None, Some(client)),
    };
    let mut entry = json!({
        "seq": number,
        "time_ns": nanos(input.time),
        "kind": input.kind.name(),
        "device": device,
    });
    if matches!(input.device, InputDevice::Agent { .. }) {
        entry["request"] = json!(request);
    }
    if let Some(client) = client {
        entry["client"] = json!(client);
    }
    if input.count > 1 {
        entry["count"] = json!(input.count);
        entry["last_ns"] = json!(nanos(input.last));
    }
    entry
}

/// A commit, as `frames` tells it.
fn commit_json(number: u64, commit: &Commit, nanos: impl Fn(Duration) -> u64) -> Value {
    json!({
        "seq": number,
        "time_ns": nanos(commit.time),
        "client": commit.client,
        "surface": commit.surface,
        "window": commit.window.map(|window| window.0),
        "output": commit.output.map(|output| output.0),
        "new_content": commit.new_content,
    })
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
