//! Every tool's definition, as `tools/list` gives it: its name, its title, its description and
//! the schema of its arguments, always in the same order. The descriptions are what a model
//! learns the tools from.
//!
//! Its neighbours: `arguments.rs` reads the properties each schema names, and the tests check
//! that it reads every one (the tests in `mod.rs`); the bounds the descriptions state are `mod.rs`'s.

use serde_json::{Value, json};

use super::{
    DEFAULT_CLIPBOARD_BYTES, MAX_CLIPBOARD_BYTES, MAX_CLIPBOARD_TEXT_BYTES, MAX_LOG_READ, MAX_WAIT,
    MAX_WINDOWS_PER_PAGE, MAX_WORKSPACE_NAME, TEXT_TYPES,
};
use crate::mcp::accessibility::MAX_NODES;
use crate::mcp::events::Kind;
use crate::mcp::outputs::{MAX_OUTPUT_PIXELS, MAX_OUTPUT_SIDE};
use crate::mcp::stdio::MAX_LINE;
use crate::rest::{Cause, Source};
use crate::{CLIPBOARD_TIMEOUT, MAX_LAYOUT_NAME};

/// What a seat's id means in a schema: every tool but `clock` and `rest` acts on one seat, and
/// `rest` on one when it is given.
const SEAT_PROPERTY: &str = "the seat's handle, from `seat create`";

/// Every tool's definition, for `tools/list`, always in this order.
pub fn definitions() -> Value {
    let seat = json!({ "type": "integer", "minimum": 2, "description": SEAT_PROPERTY });
    let kind_names: Vec<&str> = Kind::ALL.iter().map(|kind| kind.name()).collect();
    let source_names: Vec<&str> = Source::ALL.iter().map(|source| source.name()).collect();
    let cause_names: Vec<&str> = Cause::ALL.iter().map(|cause| cause.name()).collect();
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
                handle, rest when it is to name only that seat's. Closing it releases what it holds, disconnects its windows, ends its \
                programs (SIGTERM and SIGCONT, then SIGKILL 2 s later) and removes its outputs; it answers \
                once they are gone, in 3 s at most. A Yuna on a manual clock holds one seat.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["create", "list", "close"] },
                    "width": { "type": "integer", "minimum": 1, "maximum": MAX_OUTPUT_SIDE,
                        "description": "create: its output's physical pixels; default 3840" },
                    "height": { "type": "integer", "minimum": 1, "maximum": MAX_OUTPUT_SIDE,
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
            "description": format!("List, create, configure or remove a seat's virtual outputs: \
                screens no monitor shows, drawn at their own size, rate and scale, at a fixed \
                rate or under VRR, announcing SDR or HDR colours. Windows open on the seat's \
                focused output. Coordinates are logical: physical pixels divided by the scale. \
                A seat keeps one output at least. An output's side is {MAX_OUTPUT_SIDE} pixels \
                at most, and Yuna's outputs, every seat's, hold {MAX_OUTPUT_PIXELS} pixels at \
                most together, those of 16 at 3840 × 2160. list gives each output's VRR range \
                in Hz and the \
                monitor whose HDR colours it announces, each null when it has none. configure \
                changes an output as a monitor set to another mode, whole or not at all: it \
                keeps its top-left corner, the seat's outputs on its right move with its right \
                edge so that none overlaps it, and its windows are placed again in it; X11 \
                windows keep the scale their X server started with. create and configure \
                answer with the output as list gives it."),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string",
                        "enum": ["list", "create", "configure", "remove"] },
                    "seat": seat,
                    "width": { "type": "integer", "minimum": 1, "maximum": MAX_OUTPUT_SIDE,
                        "description": format!("create: physical pixels; default 3840{own}") },
                    "height": { "type": "integer", "minimum": 1, "maximum": MAX_OUTPUT_SIDE,
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
                    "name": { "type": "string", "minLength": 1, "maxLength": MAX_WORKSPACE_NAME,
                        "description": format!("create, remove, focus: the workspace's name, \
                            {MAX_WORKSPACE_NAME} bytes at most in UTF-8") },
                    "output": { "type": "integer",
                        "description": "create: the output it lives on; the focused one by default" },
                    "window": { "type": "integer", "description": "move_window: the window's id" },
                    "workspace": { "type": "string", "minLength": 1,
                        "maxLength": MAX_WORKSPACE_NAME,
                        "description": format!("move_window: the workspace's name, \
                            {MAX_WORKSPACE_NAME} bytes at most in UTF-8") }
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
                log of its output and errors; its windows open on its seat. It runs on its \
                seat's throwaway home, never the user's: HOME names a folder the seat's \
                programs share, removed when the seat closes, with XDG_CONFIG_HOME, \
                XDG_DATA_HOME, XDG_CACHE_HOME and XDG_STATE_HOME inside it; a variable named in \
                env wins, to give a program a test profile, or the user's own home on purpose. env \
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
                client_left with reason closed. Logs go when the seat closes. \
                With accessibility, it joins its seat's accessibility bus, which the first such \
                program starts, and the accessibility tool reads its tree: GTK, Qt and the ATK \
                bridge's toolkits turn their accessibility on; Chromium and Electron also need \
                --force-renderer-accessibility in their command for a page's own content. \
                Without it, a program runs with its toolkit's accessibility off.",
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
                    "accessibility": { "type": "boolean",
                        "description": "start: on its seat's accessibility bus, its tree read \
                            by the accessibility tool; false by default" },
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
                max_cll, max_fall and rendering intent. With after, a window's id, or limit, \
                gives a page: the windows whose id is above after, in that order, limit at \
                most (128, its default and its most), and next, the last window given while more \
                match after it, else null: give it as after for the next page. A window opened \
                meanwhile comes on a later page; one closed meanwhile is not listed. Without \
                either, every window, and no next. With wait_ms, waits up to that long for a \
                window that matches the filters, after included, when none does yet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "seat": seat,
                    "app_id": { "type": "string", "description": "only this application id" },
                    "pid": { "type": "integer", "description": "only this client process" },
                    "colour": { "type": "boolean",
                        "description": "also give the colours each window's surfaces declared; \
                            false by default" },
                    "after": { "type": "integer", "minimum": 0,
                        "description": "the page's cursor: a window's id, or a reply's next; \
                            only the windows above it" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_WINDOWS_PER_PAGE,
                        "description": "how many windows the page lists at most; 128 by \
                            default once after is given" },
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
                with each event of its seat and is never reused while the seat lives, so keep \
                one cursor for each seat; time_ns on the clock's scale; kind; and its fields; a \
                title, an app_id or a workspace is cut to 512 bytes. Yuna builds the events of \
                what changed at the end of each round, and at each events call, so that one \
                without after waits only for what comes after it; a window is told at most once \
                a build for each kind, and a window that opens and closes between two builds, \
                between two calls too, is told opened, then closed. Kinds: window_opened \
                (window, app_id, title, pid, shell); window_closed (window); window_changed \
                (window, title, app_id: either changed, however often since the last build); \
                window_placed (window, output, workspace, place: where it rests in the layout \
                changed, told once the layout is arranged); window_unresponsive (window: it \
                left a ping unanswered, as windows' unresponsive tells); focus (window: the \
                seat's focus \
                moved, null for none); program_exited (program, exit_code or signal, the other \
                null); client_left (pid, program, reason, error: a Wayland connection of the \
                seat ended; reason protocol_error, with error's interface, object, code and \
                message, or closed, error null: it left, sent what is not Wayland, or could not \
                take its events; pid as windows gives it; program null for a client of no \
                program, or one whose process had exited before Yuna accepted it); \
                client_refused (bound, refused: the seat's socket closed connections at once, \
                the seat serving bound clients already, its Xwayland apart; refused counts them \
                since the seat's last client_refused; told at the first refusal, then at the \
                first a second or more after the last told: the refusals after the last told \
                wait for such a refusal, and are not told if none comes); \
                output_added, \
                output_removed, output_changed (output). With after, a seq of the seat's or a \
                reply's next, gives the events after it, only kinds when given, at most 256 a \
                reply; an after past the seat's newest event is refused. next is the cursor to \
                give next, the seat's newest event when no more wait, and missed how many \
                events of the seat after the cursor its log, which keeps its last 1024, \
                dropped: read windows again then. Without after, no past event: the reply's \
                next reads what comes. With wait_ms, waits up to that long, on real time \
                whatever the clock, for a first event that matches, and answers once the round \
                that recorded it ends, or at once when the log dropped events after the \
                cursor; timed_out is true when none came. Yuna keeps nothing between calls: \
                the cursor is yours.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "seat": seat,
                    "after": { "type": "integer", "minimum": 0,
                        "description": "the cursor: an event's seq or a reply's next, of this \
                            seat; none for what comes from now" },
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
            "name": "accessibility",
            "title": "Accessibility tree",
            "description": "Find nodes of the accessibility trees of a seat's programs started \
                with spawn start and accessibility: true, a button by its name for one: each \
                node's window, app (its process id), role and name as AT-SPI gives them, \
                description, states, and rect, its rectangle in its output's logical units from \
                the output's top-left corner, the units pointer takes: click its centre to press \
                it. name is a part of the node's name, compared without regard to case; role an \
                AT-SPI role name (button, check box, text, frame, label…); states AT-SPI state \
                names the node must all have (focused, checked, sensitive, showing…). window or \
                pid limit the walk to that program's tree. A walk visits max_nodes nodes at \
                most, 10000 by default, and gives 256 nodes at most, more true past them; each \
                program has 3 s to answer, and one that does not is told so in apps, never \
                holding the others. With wait_ms, waits up to that long for a node that matches, \
                walking again as the programs change. A program started without accessibility \
                is not on the bus.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "seat": seat,
                    "window": { "type": "integer", "description": "only this window's program" },
                    "pid": { "type": "integer", "description": "only this process's tree" },
                    "name": { "type": "string", "minLength": 1,
                        "description": "a part of the name, any case" },
                    "role": { "type": "string", "description": "an AT-SPI role name" },
                    "states": { "type": "array", "items": { "type": "string" },
                        "description": "AT-SPI state names the node must all have" },
                    "max_nodes": { "type": "integer", "minimum": 1, "maximum": MAX_NODES,
                        "description": "the most nodes visited; 10000 by default" },
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
                        "description": format!("layout: XKB's layouts, joined by commas, four \
                            at most: fr, us,ru; {MAX_LAYOUT_NAME} bytes at most in UTF-8") },
                    "variant": { "type": "string", "maxLength": MAX_LAYOUT_NAME,
                        "description": format!("layout: their variants, one for each layout, \
                            joined by commas; each layout's default when left out; \
                            {MAX_LAYOUT_NAME} bytes at most in UTF-8") },
                    "options": { "type": "string", "maxLength": MAX_LAYOUT_NAME,
                        "description": format!("layout: XKB's options, joined by commas, such \
                            as grp:alt_shift_toggle; none when left out; {MAX_LAYOUT_NAME} bytes \
                            at most in UTF-8") }
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
                null when it offers none: as text when it is UTF-8 with no control character \
                but tab, newline and carriage return, else in base64 with base64 true; at \
                most max_bytes, with truncated when there was more. It waits for the \
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
        },
        {
            "name": "rest",
            "title": "Rest audit",
            "description": format!("Name what woke Yuna and what made it draw: at rest, \
                nothing does. Without wait_ms, what was counted since Yuna started; with \
                wait_ms, what is counted in the next wait_ms of real time, whatever the clock, \
                answered at its end, this call's own timer left out, and the wake it caused \
                when nothing else woke Yuna with it; this call's request came before that \
                window and its reply goes after it. The timer of --rest-audit's log is never \
                counted, in a window or since Yuna started. It counts among the calls that wait, \
                and fails when its seat closes. since_ns and now_ns are the counts' start and \
                end on the clock's scale. wakes: the times Yuna woke, every seat's. sources: \
                each source that woke it, with how many times: {}. clients: each client that \
                woke Yuna or made it draw, with pid, program (null for none), seat, xwayland \
                (its seat's Xwayland, the client of every X11 program there), wakes (times it \
                had something to send when the clients woke Yuna), commits and draws (frames \
                drawn with one of its commits among their causes); clients_gone: the same \
                for the clients that left, together, with their count; \
                unnamed_client_wakes: the clients' wakes in which Yuna named none, out of \
                descriptors. programs: \
                each program whose output (output_wakes) or pace (pace_wakes) woke Yuna. \
                frames: the frames drawn; draw_causes: each cause with its frames, a frame \
                counted under each of its causes: {}. With seat, only that seat's clients, \
                programs, sockets, outputs' refreshes, Xwayland, accessibility bus and \
                clipboard reads and writes, beside Yuna's own parts: \
                signals, a test seam's waker, MCP's input and output, encoders, waiting calls, \
                captures' timers, \
                the programs' throttle, the hidden windows' frames, X11 answers and drags' \
                time limits; wakes and \
                unnamed_client_wakes stay every seat's, an unnamed wake's client, and so its \
                seat, being unknown.",
                source_names.join(", "), cause_names.join(", ")),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "seat": { "type": "integer", "minimum": 2,
                        "description": "only this seat's, beside Yuna's own parts; every \
                            seat's by default" },
                    "wait_ms": { "type": "integer", "minimum": 1, "maximum": MAX_WAIT.as_millis(),
                        "description": "count this long from now and answer then; by \
                            default, what was counted since Yuna started" }
                },
                "additionalProperties": false
            }
        }
    ])
}
