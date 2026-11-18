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

use super::events::{Event, EventData, Kind, Reading};
use super::image::base64;
use super::programs::{End, LogRead, Program};
use super::protocol::{self, Revision};
use super::stdio::MAX_LINE;
use crate::colour::SurfaceColourDescription;
use crate::colour::description::Chromaticities;
use crate::compositor::{ClientProtocolError, OutputDescription, WindowDescription};
use crate::input::PointerDescription;
use crate::xwayland::Selection;
use crate::{
    CLIPBOARD_TIMEOUT, CaptureMapping, CaptureSize, CaptureTarget, CapturedFrame, LogicalRegion,
    MAX_LAYOUT_NAME,
};
use yuna_layout::geometry::Rect;
use yuna_layout::{OutputId, WindowId, WorkspaceInfo};

/// The longest a call may wait: under the 60 s at which Codex gives up on a call by default
/// ([survey](../../../../docs/survey.md#agents-and-mcp)).
pub const MAX_WAIT: Duration = Duration::from_secs(50);

/// How many calls may wait at once, `windows`', `events`' and `clipboard get`'s together; past it,
/// a call that would wait is refused.
pub const MAX_WAITING_CALLS: usize = 64;

/// How many bytes of a log `spawn log` gives by default.
pub const DEFAULT_LOG_READ: u64 = 64 << 10;

/// How many bytes of a log `spawn log` gives at most.
pub const MAX_LOG_READ: u64 = 1 << 20;

/// The most bytes of data `clipboard get` gives: 1 MiB, its base64 or its escaped text well
/// under the replies that may wait ([`MAX_QUEUED_OUTPUT`](super::MAX_QUEUED_OUTPUT)).
pub const MAX_CLIPBOARD_BYTES: usize = 1 << 20;

/// The most bytes of text `clipboard set` takes: 256 KiB, a quarter of a request's line
/// ([`MAX_LINE`]), so that any text but control characters fits it once escaped as JSON. A
/// client that escapes all but ASCII writes a character of 2 or 3 bytes as `\uXXXX`, 6 bytes, and
/// one of 4 bytes as two of them, 12: three times its length at most.
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 256 << 10;

/// How many bytes of data `clipboard get` gives by default: 64 KiB, more than a text copied by
/// hand, small beside a model's context.
pub const DEFAULT_CLIPBOARD_BYTES: usize = 64 << 10;

/// The text types `clipboard get` reads by default, the first a selection offers: UTF-8 first,
/// in Wayland's name, then X11's, then the others.
const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
    "TEXT",
];

/// What a seat's id means in a schema: every tool but `clock` acts on one seat.
const SEAT_PROPERTY: &str = "the seat's handle, from `seat create`";

/// Every tool's definition, for `tools/list`, always in this order.
pub fn definitions() -> Value {
    let seat = json!({ "type": "integer", "minimum": 2, "description": SEAT_PROPERTY });
    let kind_names: Vec<&str> = Kind::ALL.iter().map(|kind| kind.name()).collect();
    // `seat create`'s output and `outputs create`'s take the same two (ADR-0049); `outputs
    // configure` takes them too, the output's own by default, and 0 for no VRR range.
    let vrr_min_create = "create: run under VRR, from this many refreshes a second up to rate, \
        which must be at least twice it; by default a fixed rate, or the server's default \
        output's range when rate is left out too";
    let hdr_create = "create: true announces the HDR colours of the owner's monitor, a \
        PG32UCDM; false, SDR; by default as the server's default output";
    let vrr_min = |minimum: u32, more: &str| {
        json!({ "type": "number", "minimum": minimum, "maximum": 500,
            "description": format!("{vrr_min_create}{more}") })
    };
    let hdr =
        |more: &str| json!({ "type": "boolean", "description": format!("{hdr_create}{more}") });
    let own = "; configure: the output's own by default";
    json!([
        {
            "name": "seat",
            "title": "Seats",
            "description": "Create a seat of your own, list the seats, or close one. A seat has \
                its own pointer, keyboard, focus and clipboard, its own outputs and workspaces, \
                and its own Wayland socket, which the programs it starts use; no input, focus \
                or clipboard of one seat reaches another. Create one first: every other tool but clock takes its \
                handle. Closing it releases what it holds, disconnects its windows, ends its \
                programs (SIGTERM and SIGCONT, then SIGKILL 2 s later) and removes its outputs; it answers \
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
                    "vrr_min": vrr_min(1, ""),
                    "hdr": hdr(""),
                    "seat": { "type": "integer", "minimum": 2, "description": "close: its handle" }
                },
                "required": ["action"],
                "additionalProperties": false
            }
        },
        {
            "name": "outputs",
            "title": "Virtual outputs",
            "description": "List, create, configure or remove a seat's virtual outputs: screens \
                no monitor shows, drawn at their own size, rate and scale, at a fixed rate or \
                under VRR, announcing SDR or HDR colours. Windows open on the seat's focused \
                output. Coordinates are logical: physical pixels divided by the scale. A seat \
                keeps one output at least. list gives each output's VRR range in Hz and the \
                monitor whose HDR colours it announces, each null when it has none. configure \
                changes an output as a monitor set to another mode, whole or not at all: it \
                keeps its top-left corner, the seat's outputs on its right move with its right \
                edge so that none overlaps it, and its windows are placed again in it; X11 \
                windows keep the scale their X server started with. create and configure \
                answer with the output as list gives it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string",
                        "enum": ["list", "create", "configure", "remove"] },
                    "seat": seat,
                    "width": { "type": "integer", "minimum": 1,
                        "description": format!("create: physical pixels; default 3840{own}") },
                    "height": { "type": "integer", "minimum": 1,
                        "description": format!("create: physical pixels; default 2160{own}") },
                    "rate": { "type": "number", "minimum": 1, "maximum": 1000,
                        "description": format!("create: refreshes a second; default 240{own}") },
                    "scale": { "type": "number", "minimum": 0.25, "maximum": 8,
                        "description": format!(
                            "create: physical pixels per logical unit; default 1.5{own}") },
                    "vrr_min": vrr_min(0, &format!(
                        "{own}, kept when rate changes; 0 runs it at a fixed rate"
                    )),
                    "hdr": hdr(own),
                    "output": { "type": "integer",
                        "description": "remove, configure: the output's id" }
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
            "description": "Start a program on a seat, list a seat's programs, stop one, read \
                its log, or make it stuck or slow. A program gets its seat's Wayland socket and \
                X display, an environment without Yuna's own DISPLAY or the user's D-Bus, and a \
                log of its output and errors; its windows open on its seat. env \
                {\"WAYLAND_DEBUG\": \"client\"} writes its Wayland trace to its log. list gives \
                each program's state, its log's size, its run_percent (100 free, 0 frozen) and \
                protocol_error: the last Wayland protocol error one of its connections was \
                ended for (interface, object, code, message), null for none. stop sends its \
                process group SIGTERM then SIGCONT, or SIGKILL with force. log gives text, the \
                bytes from to to of its output as UTF-8, invalid bytes replaced: max_bytes of \
                them, 64 KiB by default, 1 MiB at most, from the byte offset from since it \
                started, or its newest without from; size is every byte it wrote, dropped \
                those its log did not keep: a log keeps its first 256 KiB and its newest 1 MiB; \
                ended is true once its output has closed. freeze stops its process group \
                (SIGSTOP) until thaw continues it (SIGCONT); slow lets it run run_percent of \
                each 100 ms and stops it the rest, on real time whatever the clock, until thaw, \
                freeze, stop or the end of its group. A frozen program reads no event: once its \
                Wayland socket's buffer fills, Yuna closes its connection, and events tells \
                client_left with reason closed. Logs go when the seat closes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string",
                        "enum": ["start", "list", "stop", "log", "freeze", "thaw", "slow"] },
                    "seat": seat,
                    "command": { "type": "array", "items": { "type": "string" }, "minItems": 1,
                        "description": "start: the program, found in PATH, and its arguments" },
                    "env": { "type": "object", "additionalProperties": { "type": "string" },
                        "description": "start: variables added to its environment" },
                    "cwd": { "type": "string", "description": "start: its working folder" },
                    "program": { "type": "integer",
                        "description": "stop, log, freeze, thaw, slow: the program's id" },
                    "force": { "type": "boolean",
                        "description": "stop: SIGKILL rather than SIGTERM" },
                    "from": { "type": "integer", "minimum": 0,
                        "description": "log: the first byte, an offset since it started; its \
                            newest bytes by default" },
                    "max_bytes": { "type": "integer", "minimum": 1, "maximum": MAX_LOG_READ,
                        "description": "log: how many bytes at most; 65536 by default" },
                    "run_percent": { "type": "integer", "minimum": 1, "maximum": 99,
                        "description": "slow: the share of each 100 ms it runs" }
                },
                "required": ["action", "seat"],
                "additionalProperties": false
            }
        },
        {
            "name": "windows",
            "title": "Windows",
            "description": "List a seat's windows, in the order they opened: id, seat, shell \
                (xdg for a Wayland program's window, x11 for an X11 program's), application id \
                (an X11 window's WM_CLASS class; null when its client gave none), title, process \
                id, output, \
                workspace (null for an unnamed one), rect: the logical rectangle it is drawn at \
                now, or where its output's last frame drew it when that frame was sampled \
                later (null when its output's frame does not draw it), place: its rectangle in the layout, where it rests \
                once its motion ends (null when hidden), motion_ends_ns: when its output's \
                motion ends, on the clock's scale (null at rest), focus, floating, fullscreen, \
                unresponsive: an X11 window asked to close that left its ping unanswered 5 s. \
                With neither --animations nor --reduced-motion, rect is place and \
                motion_ends_ns null. With colour, each window also gives colours: one \
                entry for each of its surfaces that declared its colours through \
                color-management-v1, its own, then its subsurfaces' and its popups': the \
                surface's role, transfer function, primaries (a protocol name, else their \
                chromaticities), luminances in cd/m², target primaries and luminance, \
                max_cll, max_fall and rendering intent. With wait_ms, waits up to that long for a window that matches \
                the filters, when none does yet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "seat": seat,
                    "app_id": { "type": "string", "description": "only this application id" },
                    "pid": { "type": "integer", "description": "only this client process" },
                    "colour": { "type": "boolean",
                        "description": "also give the colours each window's surfaces declared; \
                            false by default" },
                    "wait_ms": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT.as_millis(),
                        "description": "how long to wait for a match; 0 by default" }
                },
                "required": ["seat"],
                "additionalProperties": false
            }
        },
        {
            "name": "events",
            "title": "Events",
            "description": "Wait for what happens on a seat, or read what happened since a \
                cursor, rather than asking in a loop. Each event gives seq, a number that grows \
                across the whole Yuna and is never reused, time_ns on the clock's scale, kind, \
                and its fields; a title, an app_id or a workspace is cut to 512 bytes. Yuna \
                looks for events at the end of each round, and at each events call, so that \
                one without after waits only for what comes after it; a window is told at most \
                once a look. Kinds: window_opened (window, app_id, title, pid, shell); \
                window_closed (window); window_changed (window, title, app_id: either changed, \
                however often since the last look); window_placed (window, output, \
                workspace, place: where it rests in the layout changed, told once the layout \
                is arranged); window_unresponsive (window: it left a ping \
                unanswered, as windows' unresponsive tells); focus (window: the seat's focus \
                moved, null for none); program_exited (program, exit_code or signal, the other \
                null); client_left (pid, program, reason, error: a Wayland connection of the \
                seat ended; reason protocol_error, with error's interface, object, code and \
                message, or closed, error null: it left, sent what is not Wayland, or could not \
                take its events; pid as windows gives it; program null for a client of no \
                program, or one whose process had exited before Yuna accepted it); \
                output_added, \
                output_removed, output_changed (output). With after, a seq or a reply's next, \
                gives the events after it, only kinds when given, at most 256 a reply; next is \
                the cursor to give next, and missed how many events of the seat after the \
                cursor its log, which keeps its last 1024, dropped. Without after, no past \
                event: the reply's next reads what comes. With wait_ms, waits up to that long, \
                on real time whatever the clock, for a first event that matches, and answers \
                once the round that recorded it ends, or at once when the log dropped events \
                after the cursor; timed_out is true when none came. A window that opens and \
                closes between two looks is not seen. Yuna keeps nothing \
                between calls: the cursor is yours.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "seat": seat,
                    "after": { "type": "integer", "minimum": 0,
                        "description": "the cursor: an event's seq or a reply's next; none \
                            for what comes from now" },
                    "kinds": { "type": "array", "minItems": 1,
                        "items": { "type": "string", "enum": kind_names },
                        "description": "only these kinds; every kind by default" },
                    "wait_ms": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT.as_millis(),
                        "description": "how long to wait for a first event; 0 by default" }
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
                several calls: press, move, release. The result tells where the pointer is, \
                in the output's logical units whatever x and y were given in, the window under \
                it and the position on its surface, and the cursor its client last asked for, \
                before it handled this call's input.",
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
            "description": "Press, release or tap keys on a seat's keyboard, type text, show \
                text being composed, or change its layout, to the window its keyboard is on, or \
                read where it is. Keys are named as XKB names them: Return, a, F5, BackSpace, \
                Left; a name presses a key that types that keysym, alone where one does, and \
                adds no modifier. tap takes keys joined by +, the last pressed while the others \
                are held: ctrl+shift+t; ctrl, shift, alt, super and altgr name modifiers. type \
                types text through the seat's layout, us unless changed, with Shift, AltGr and \
                the layout's own switch between its layouts, any character included: one its \
                keys do not type is committed to the text field there when it speaks \
                text-input-v3. preedit shows text being composed in that text field, as an input \
                method does, until the next type replaces it; empty text clears it. layout sets \
                the seat's XKB layout, variant and options, on the evdev rules and a pc105 \
                keyboard: fr, or us,ru with the option grp:alt_shift_toggle; every program of \
                the seat gets the new keymap; it is refused while a key is held, and for a \
                layout, variant or option XKB's rules do not know. The result tells the window \
                the keyboard is on and whether a text field is enabled there; get's and \
                layout's tell the seat's layout, variant and options too. A click focuses a \
                window; the keyboard follows it at the output's next refresh.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string",
                        "enum": ["get", "press", "release", "tap", "type", "preedit", "layout"] },
                    "seat": seat,
                    "key": { "type": "string", "minLength": 1,
                        "description": "press, release: the key's name" },
                    "keys": { "type": "string", "minLength": 1,
                        "description": "tap: keys joined by +" },
                    "text": { "type": "string",
                        "description": "type: the text; preedit: the text being composed" },
                    "layout": { "type": "string", "minLength": 1, "maxLength": MAX_LAYOUT_NAME,
                        "description": "layout: XKB's layouts, joined by commas, four at most: \
                            fr, us,ru" },
                    "variant": { "type": "string", "maxLength": MAX_LAYOUT_NAME,
                        "description": "layout: their variants, one for each layout, joined by \
                            commas; each layout's default when left out" },
                    "options": { "type": "string", "maxLength": MAX_LAYOUT_NAME,
                        "description": "layout: XKB's options, joined by commas, such as \
                            grp:alt_shift_toggle; none when left out" }
                },
                "required": ["action", "seat"],
                "additionalProperties": false
            }
        },
        {
            "name": "clipboard",
            "title": "Clipboard",
            "description": format!("Read, set or clear a seat's clipboard, or its primary \
                selection, which its X11 and Wayland programs copy to and paste from alike. get \
                gives the MIME types the selection offers, none when it is empty, and the data of \
                one of them, mime, by default the first text type it offers of {text_types}, \
                null when it offers none: as text when it is UTF-8, else in base64 with base64 \
                true; at most max_bytes, with truncated when there was more. It waits for the \
                program that copied to write the data, {timeout} s at most, then answers with \
                what came and timed_out. set holds the selection for you, offering text as \
                {agent_types}, until a program copies something else or clear; a program pastes \
                it while its window has the seat's keyboard, as any paste. clear empties the \
                selection, whoever copied it.",
                text_types = TEXT_TYPES.join(", "),
                timeout = CLIPBOARD_TIMEOUT.as_secs(),
                agent_types = crate::selection::AGENT_MIME_TYPES.join(", ")),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["get", "set", "clear"] },
                    "seat": seat,
                    "selection": { "type": "string", "enum": ["clipboard", "primary"],
                        "description": "clipboard by default; primary is the selection a middle \
                            click pastes" },
                    "mime": { "type": "string", "minLength": 1,
                        "description": "get: the MIME type to read, one the selection offers" },
                    "max_bytes": { "type": "integer", "minimum": 1,
                        "maximum": MAX_CLIPBOARD_BYTES,
                        "description": format!(
                            "get: the most bytes of data given; {DEFAULT_CLIPBOARD_BYTES} by \
                             default") },
                    "text": { "type": "string",
                        "description": format!(
                            "set: the text, {MAX_CLIPBOARD_TEXT_BYTES} bytes at most in UTF-8; \
                             the request's line, the text escaped as JSON included, is \
                             {MAX_LINE} bytes at most") }
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

/// What `clipboard get` read of its selection, as one of its types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardData {
    pub mime: String,
    pub bytes: Vec<u8>,
    /// Whether the selection's source had more than was read.
    pub truncated: bool,
    /// Whether its source had not closed its pipe within [`CLIPBOARD_TIMEOUT`].
    pub timed_out: bool,
}

/// `clipboard get`'s result: the MIME types the selection offers and, when one was read, its
/// data, as text when it is UTF-8, else in base64 with `base64` true, and its length in bytes.
/// Data cut inside a character, by `max_bytes` or the time limit, is text up to that character.
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
        EventData::OutputAdded { output }
        | EventData::OutputRemoved { output }
        | EventData::OutputChanged { output } => json!({ "output": output.0 }),
    };
    described["seq"] = json!(event.seq);
    described["time_ns"] = json!(event.time_ns);
    described["kind"] = json!(event.data.kind().name());
    described
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
    use crate::colour::SurfaceRole;
    use crate::colour::description::{Description, Luminances, PG32UCDM, TransferFunction};
    use crate::{OutputMode, WindowShell};
    use yuna_layout::SeatId;
    use yuna_layout::geometry::Rect;

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
                "events",
                "capture",
                "pointer",
                "keyboard",
                "clipboard",
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
            arguments(args(json!({
                "action": "layout", "seat": 2, "layout": "us,ru", "variant": ",phonetic",
                "options": "grp:alt_shift_toggle"
            }))),
            Ok(KeyboardCall::Layout {
                variant: Some(_),
                options: Some(_),
                ..
            })
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
    fn configure_reads_every_value_of_a_mode_and_needs_its_output() {
        let call: Result<OutputsCall, _> = arguments(args(json!({
            "action": "configure", "seat": 2, "output": 3, "width": 1280, "height": 720,
            "rate": 120, "scale": 2, "vrr_min": 48, "hdr": true
        })));
        assert!(
            matches!(
                call,
                Ok(OutputsCall::Configure {
                    seat: 2,
                    output: 3,
                    width: Some(1280),
                    height: Some(720),
                    vrr_min: Some(_),
                    hdr: Some(true),
                    ..
                })
            ),
            "{call:?}"
        );
        assert!(matches!(
            arguments(args(json!({"action": "configure", "seat": 2, "output": 1}))),
            Ok(OutputsCall::Configure {
                width: None,
                rate: None,
                hdr: None,
                ..
            })
        ));
        assert!(
            arguments::<OutputsCall>(args(json!({"action": "configure", "seat": 2, "scale": 1})))
                .is_err()
        );
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
        assert!(arguments::<KeyboardCall>(args(json!({"action": "layout", "seat": 2}))).is_err());
        assert!(
            arguments::<KeyboardCall>(args(
                json!({"action": "layout", "seat": 2, "layout": "fr", "model": "pc104"})
            ))
            .is_err()
        );
        assert!(arguments::<OutputsCall>(args(json!({"action": "rotate"}))).is_err());
        assert!(arguments::<OutputsCall>(args(json!({}))).is_err());
        assert!(arguments::<OutputsCall>(args(json!({"action": "create", "width": -1}))).is_err());
        assert!(arguments::<WindowsCall>(args(json!({"seat": 2, "wait": 1}))).is_err());
        assert!(arguments::<SpawnCall>(args(json!({"action": "start", "seat": 2}))).is_err());
    }

    #[test]
    fn vrr_hdr_and_colour_are_named_read_and_their_wrong_types_refused() {
        let tools = definitions();
        let property = |tool: usize, name: &str| {
            tools[tool]["inputSchema"]["properties"][name]["type"].clone()
        };
        for tool in [0, 1] {
            assert_eq!(
                property(tool, "vrr_min"),
                json!("number"),
                "{}",
                tools[tool]["name"]
            );
            assert_eq!(
                property(tool, "hdr"),
                json!("boolean"),
                "{}",
                tools[tool]["name"]
            );
        }
        assert_eq!(tools[4]["name"], json!("windows"));
        assert_eq!(property(4, "colour"), json!("boolean"));

        let seat: Result<SeatCall, _> = arguments(args(
            json!({"action": "create", "vrr_min": 48, "hdr": true}),
        ));
        assert!(
            matches!(seat, Ok(SeatCall::Create { vrr_min: Some(min), hdr: Some(true), .. })
                if min == 48.0),
            "{seat:?}"
        );
        let outputs: Result<OutputsCall, _> = arguments(args(json!({
            "action": "create", "seat": 2, "vrr_min": 47.952, "hdr": false
        })));
        assert!(
            matches!(outputs, Ok(OutputsCall::Create { vrr_min: Some(min), hdr: Some(false), .. })
                if min == 47.952),
            "{outputs:?}"
        );
        // Absent, they leave the mode as it was.
        assert!(matches!(
            arguments(args(json!({"action": "create"}))),
            Ok(SeatCall::Create {
                vrr_min: None,
                hdr: None,
                ..
            })
        ));
        let windows: WindowsCall =
            arguments(args(json!({"seat": 2, "colour": true}))).expect("windows' arguments");
        assert!(windows.colour);
        let windows: WindowsCall = arguments(args(json!({"seat": 2}))).expect("windows' arguments");
        assert!(!windows.colour);

        for wrong in [json!("48"), json!(true), json!([48])] {
            let call = json!({"action": "create", "vrr_min": wrong});
            assert!(
                arguments::<SeatCall>(args(call)).is_err(),
                "vrr_min {wrong}"
            );
            let call = json!({"action": "create", "seat": 2, "vrr_min": wrong});
            assert!(
                arguments::<OutputsCall>(args(call)).is_err(),
                "vrr_min {wrong}"
            );
        }
        for wrong in [json!("true"), json!(1), json!({})] {
            let call = json!({"action": "create", "hdr": wrong});
            assert!(arguments::<SeatCall>(args(call)).is_err(), "hdr {wrong}");
            let call = json!({"action": "create", "seat": 2, "hdr": wrong});
            assert!(arguments::<OutputsCall>(args(call)).is_err(), "hdr {wrong}");
            let call = json!({"seat": 2, "colour": wrong});
            assert!(
                arguments::<WindowsCall>(args(call)).is_err(),
                "colour {wrong}"
            );
        }
        // Only `create` takes them.
        assert!(
            arguments::<OutputsCall>(args(json!({"action": "list", "seat": 2, "hdr": true})))
                .is_err()
        );
        assert!(arguments::<SeatCall>(args(json!({"action": "list", "vrr_min": 48}))).is_err());
    }

    #[test]
    fn an_outputs_vrr_range_and_hdr_monitor_are_listed_null_when_it_has_none() {
        let fixed = OutputMode::new(1920, 1080, 240.0, 1.0).expect("a mode");
        let description = |mode: OutputMode| OutputDescription {
            id: OutputId(3),
            seat: SeatId(2),
            name: "HEADLESS-3".to_owned(),
            mode,
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: 1920.0,
                h: 1080.0,
            },
        };
        let plain = output_json(&description(fixed));
        assert_eq!(plain["vrr"], Value::Null);
        assert_eq!(plain["hdr"], Value::Null);
        let vrr_hdr = fixed
            .with_vrr(47.952)
            .expect("a range twice as wide")
            .with_hdr(PG32UCDM);
        let listed = output_json(&description(vrr_hdr));
        assert_eq!(listed["vrr"], json!({"min": 47.952, "max": 240.0}));
        assert_eq!(listed["hdr"], json!("PG32UCDM"));
    }

    #[test]
    fn a_window_names_its_shell() {
        let mut window = WindowDescription {
            id: WindowId(4),
            shell: WindowShell::X11,
            app_id: Some("XTerm".to_owned()),
            title: Some("xterm".to_owned()),
            pid: Some(4242),
            seat: SeatId(2),
            output: None,
            workspace: None,
            rect: None,
            place: None,
            motion_ends: None,
            focused: true,
            floating: false,
            fullscreen: false,
            unresponsive: false,
        };
        let listed = window_json(&window, None);
        assert_eq!(listed["shell"], json!("x11"));
        assert_eq!(listed["app_id"], json!("XTerm"));
        assert_eq!(listed["pid"], json!(4242));
        window.shell = WindowShell::Xdg;
        assert_eq!(window_json(&window, None)["shell"], json!("xdg"));
    }

    #[test]
    fn a_windows_colours_come_only_when_asked_with_unnamed_primaries_as_numbers() {
        let window = WindowDescription {
            id: WindowId(1),
            shell: WindowShell::Xdg,
            app_id: None,
            title: None,
            pid: None,
            seat: SeatId(2),
            output: None,
            workspace: None,
            rect: None,
            place: None,
            motion_ends: None,
            focused: false,
            floating: false,
            fullscreen: false,
            unresponsive: false,
        };
        assert_eq!(window_json(&window, None).get("colours"), None);
        assert_eq!(window_json(&window, Some(&[]))["colours"], json!([]));

        let primaries = Chromaticities {
            red: (680_000, 320_000),
            green: (265_000, 690_000),
            blue: (150_000, 60_000),
            white: (312_700, 329_000),
        };
        let declared = SurfaceColourDescription {
            role: SurfaceRole::Popup,
            description: Description {
                transfer_function: TransferFunction::Gamma22,
                primaries,
                named_primaries: None,
                luminances: Luminances {
                    min: 2_000,
                    max: 80,
                    reference: 80,
                },
                target_primaries: primaries,
                target_luminance: (5, 400),
                max_cll: Some(400),
                max_fall: None,
            },
        };
        let p3 = json!({
            "red": [0.68, 0.32], "green": [0.265, 0.69], "blue": [0.15, 0.06],
            "white": [0.3127, 0.329],
        });
        assert_eq!(
            window_json(&window, Some(&[declared]))["colours"],
            json!([{
                "role": "popup",
                "transfer_function": "gamma22",
                "primaries": p3,
                "luminances": {"min": 0.2, "max": 80, "reference": 80},
                "target_primaries": p3,
                "target_luminance": {"min": 0.0005, "max": 400},
                "max_cll": 400,
                "max_fall": null,
                "intent": "perceptual",
            }])
        );
    }

    #[test]
    fn events_and_spawns_new_actions_read_their_arguments() {
        let call: EventsCall = arguments(args(json!({
            "seat": 2, "after": 7, "kinds": ["window_opened", "client_left"], "wait_ms": 10
        })))
        .expect("events' arguments");
        assert_eq!(call.after, Some(7));
        assert_eq!(call.kinds, Some(vec![Kind::WindowOpened, Kind::ClientLeft]));
        let call: EventsCall = arguments(args(json!({"seat": 2}))).expect("events' arguments");
        assert_eq!((call.after, call.kinds, call.wait_ms), (None, None, 0));
        assert!(arguments::<EventsCall>(args(json!({"seat": 2, "kinds": ["nope"]}))).is_err());
        assert!(arguments::<EventsCall>(args(json!({"seat": 2, "from": 1}))).is_err());
        // The schema names every kind serde reads, and nothing else.
        let tools = definitions();
        let events = tools
            .as_array()
            .into_iter()
            .flatten()
            .find(|tool| tool["name"] == json!("events"))
            .expect("the events tool");
        let names = &events["inputSchema"]["properties"]["kinds"]["items"]["enum"];
        for name in names.as_array().expect("the kinds") {
            let call = json!({"seat": 2, "kinds": [name]});
            assert!(arguments::<EventsCall>(args(call)).is_ok(), "{name}");
        }
        assert_eq!(names.as_array().map(Vec::len), Some(Kind::ALL.len()));

        assert!(matches!(
            arguments(args(
                json!({"action": "log", "seat": 2, "program": 1, "from": 5,
                                  "max_bytes": 9})
            )),
            Ok(SpawnCall::Log {
                from: Some(5),
                max_bytes: Some(9),
                ..
            })
        ));
        assert!(matches!(
            arguments(args(json!({"action": "slow", "seat": 2, "program": 1,
                                  "run_percent": 50}))),
            Ok(SpawnCall::Slow {
                run_percent: 50,
                ..
            })
        ));
        assert!(matches!(
            arguments(args(json!({"action": "freeze", "seat": 2, "program": 1}))),
            Ok(SpawnCall::Freeze { program: 1, .. })
        ));
        assert!(matches!(
            arguments(args(json!({"action": "thaw", "seat": 2, "program": 1}))),
            Ok(SpawnCall::Thaw { program: 1, .. })
        ));
        assert!(
            arguments::<SpawnCall>(args(json!({"action": "slow", "seat": 2, "program": 1})))
                .is_err()
        );
        assert!(
            arguments::<SpawnCall>(args(
                json!({"action": "slow", "seat": 2, "program": 1, "run_percent": 300})
            ))
            .is_err()
        );
        assert!(
            arguments::<SpawnCall>(args(
                json!({"action": "freeze", "seat": 2, "program": 1, "force": true})
            ))
            .is_err()
        );
    }

    #[test]
    fn an_event_gives_its_number_time_kind_and_fields() {
        let event = |data| Event {
            seq: 3,
            time_ns: 40,
            data,
        };
        let left = event(EventData::ClientLeft {
            pid: Some(77),
            program: Some(2),
            error: Some(ClientProtocolError {
                interface: "wl_subcompositor".to_owned(),
                object: 5,
                code: 0,
                message: "role".to_owned(),
            }),
        });
        assert_eq!(
            event_json(&left),
            json!({"seq": 3, "time_ns": 40, "kind": "client_left", "pid": 77, "program": 2,
                   "reason": "protocol_error", "error": {"interface": "wl_subcompositor",
                   "object": 5, "code": 0, "message": "role"}})
        );
        let closed = event(EventData::ClientLeft {
            pid: None,
            program: None,
            error: None,
        });
        assert_eq!(event_json(&closed)["reason"], json!("closed"));
        assert_eq!(event_json(&closed)["error"], Value::Null);
        let exited = event(EventData::ProgramExited {
            program: 4,
            end: End::Killed(9),
        });
        assert_eq!(
            event_json(&exited),
            json!({"seq": 3, "time_ns": 40, "kind": "program_exited", "program": 4,
                   "exit_code": null, "signal": 9})
        );
        let placed = event(EventData::WindowPlaced {
            window: WindowId(1),
            output: None,
            workspace: Some("web".to_owned()),
            place: None,
        });
        assert_eq!(
            event_json(&placed),
            json!({"seq": 3, "time_ns": 40, "kind": "window_placed", "window": 1,
                   "output": null, "workspace": "web", "place": null})
        );
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

    #[test]
    fn clipboard_reads_its_properties_and_refuses_the_others() {
        let get: Result<ClipboardCall, _> = arguments(args(json!({
            "action": "get", "seat": 2, "selection": "primary", "mime": "image/png",
            "max_bytes": 10
        })));
        let get = get.expect("a get");
        assert_eq!(get.seat(), 2);
        assert_eq!(get.selection(), Selection::Primary);
        assert!(matches!(
            get,
            ClipboardCall::Get {
                mime: Some(_),
                max_bytes: Some(10),
                ..
            }
        ));
        let set: ClipboardCall =
            arguments(args(json!({"action": "set", "seat": 3, "text": "ユナ"}))).expect("a set");
        assert_eq!(set.selection(), Selection::Clipboard, "by default");
        assert!(matches!(set, ClipboardCall::Set { seat: 3, .. }));
        assert!(matches!(
            arguments(args(json!({"action": "clear", "seat": 2}))),
            Ok(ClipboardCall::Clear { seat: 2, .. })
        ));
        for refused in [
            json!({"action": "set", "seat": 2}),
            json!({"action": "get"}),
            json!({"action": "get", "seat": 2, "selection": "secondary"}),
            json!({"action": "clear", "seat": 2, "text": "a"}),
            json!({"action": "get", "seat": 2, "max_bytes": -1}),
            json!({"action": "copy", "seat": 2}),
        ] {
            assert!(
                arguments::<ClipboardCall>(args(refused.clone())).is_err(),
                "{refused}"
            );
        }
        assert_eq!(clipboard_bytes(None), Ok(DEFAULT_CLIPBOARD_BYTES));
        assert_eq!(
            clipboard_bytes(Some(MAX_CLIPBOARD_BYTES as u64)),
            Ok(MAX_CLIPBOARD_BYTES)
        );
        assert!(clipboard_bytes(Some(0)).is_err());
        assert!(clipboard_bytes(Some(MAX_CLIPBOARD_BYTES as u64 + 1)).is_err());
    }

    #[test]
    fn a_text_type_is_read_by_default_utf_8_first() {
        let types = |names: &[&str]| -> Vec<String> {
            names.iter().map(|name| (*name).to_owned()).collect()
        };
        let first = |names: &[&str]| first_text_type(&types(names));
        assert_eq!(
            first(&["TEXT", "text/plain", "text/plain;charset=utf-8"]),
            Some("text/plain;charset=utf-8".to_owned())
        );
        assert_eq!(
            first(&["STRING", "UTF8_STRING", "text/plain"]),
            Some("UTF8_STRING".to_owned())
        );
        assert_eq!(first(&["text/html", "TEXT"]), Some("TEXT".to_owned()));
        assert_eq!(first(&["image/png", "text/html"]), None);
    }

    #[test]
    fn clipboard_data_is_text_when_utf_8_else_base64() {
        let read = |bytes: &[u8], truncated: bool, timed_out: bool| {
            let data = ClipboardData {
                mime: "text/plain".to_owned(),
                bytes: bytes.to_vec(),
                truncated,
                timed_out,
            };
            clipboard_json(&["text/plain".to_owned()], Some(&data))
        };
        let text = read("ユナ".as_bytes(), false, false);
        assert_eq!(
            (&text["data"], &text["base64"], &text["bytes"]),
            (&json!("ユナ"), &json!(false), &json!(6))
        );
        // Cut inside its last character by max_bytes or the time limit: text up to it.
        for (truncated, timed_out) in [(true, false), (false, true)] {
            let cut = read(&"ユナ".as_bytes()[..5], truncated, timed_out);
            assert_eq!((&cut["data"], &cut["bytes"]), (&json!("ユ"), &json!(3)));
            assert_eq!(cut["base64"], json!(false));
        }
        // Whole yet ending inside a character: not text.
        let whole = read(&"ユナ".as_bytes()[..5], false, false);
        assert_eq!(whole["base64"], json!(true));
        assert_eq!(whole["bytes"], json!(5));
        let binary = read(&[0xFF, 0x00, b'a'], true, false);
        assert_eq!(
            (&binary["data"], &binary["base64"]),
            (&json!("/wBh"), &json!(true))
        );
        assert_eq!(
            clipboard_json(&[], None),
            json!({"mime_types": [], "mime": null, "data": null, "base64": false, "bytes": 0,
                   "truncated": false, "timed_out": false})
        );
    }
}
