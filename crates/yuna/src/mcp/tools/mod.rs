//! The tools `yuna mcp` offers, their schemas, their arguments and how their results are told
//! ([ADR-0033](../../../../../docs/adr/0033-the-mcp-server.md)). What each does is in the
//! server's modules (`../mod.rs` names them).
//!
//! Each tool that does several things takes an `action`. A result is a JSON object, given both
//! as `structuredContent` and as text, which MCP asks for clients that read text alone; a failure
//! is a text with `isError`, which the model reads and may correct.
//!
//! The tools' contract with the client is in three layers, a module each:
//!
//! - `schemas.rs`: every tool's definition, as `tools/list` gives it;
//! - `arguments.rs`: each tool's arguments, read with serde;
//! - `results.rs`: a tool's result, and how each thing a tool tells is given as JSON.
//!
//! They are cut by layer rather than by tool: the definitions are one list in a fixed order,
//! and the results share their parts across tools, a rectangle or a program's end. This module
//! holds the bounds the layers name, and the tests of all three.

mod arguments;
mod results;
mod schemas;

use std::time::Duration;

pub use arguments::{
    AccessibilityCall, ButtonCall, CaptureCall, ClipboardCall, ClockCall, EventsCall, KeyboardCall,
    OutputsCall, PointerCall, RestCall, SeatCall, SpawnCall, WindowsCall, WorkspacesCall,
    advance_length, arguments, clipboard_bytes, first_text_type,
};
pub use results::{
    ClipboardData, CopiedCapture, ToolResult, capture_json, clipboard_json, events_json, log_json,
    output_json, pointer_json, program_json, rest_json, window_json, workspace_json,
};
pub use schemas::definitions;

/// The longest a call may wait: under the 60 s at which Codex gives up on a call by default
/// ([survey](../../../../../docs/survey.md#agents-and-mcp)).
pub const MAX_WAIT: Duration = Duration::from_secs(50);

/// How many calls may wait at once, `windows`', `events`', `clipboard get`'s, `rest`'s and
/// `accessibility`'s, and programs waiting for their seat's bus, together; past it, a call that
/// would wait is refused.
pub const MAX_WAITING_CALLS: usize = 64;

/// How many bytes of a log `spawn log` gives by default.
pub const DEFAULT_LOG_READ: u64 = 64 << 10;

/// How many bytes of a log `spawn log` gives at most.
pub const MAX_LOG_READ: u64 = 1 << 20;

/// The most bytes of data `clipboard get` gives: 1 MiB. Its reply gives it twice, in its
/// structured content and in its text, the structured content's JSON escaped again: data in
/// base64 takes 4/3 of its bytes each time, about 2.7 MiB in all; text holds no control
/// character JSON writes in six bytes, `\u00XX`, which goes in base64 (`clipboard_json`), so
/// that a byte takes 6 at most, a quote or a backslash escaped once then twice: 6 MiB, under
/// the replies that may wait ([`MAX_QUEUED_OUTPUT`](super::MAX_QUEUED_OUTPUT)), 16 MiB.
pub const MAX_CLIPBOARD_BYTES: usize = 1 << 20;

/// The most bytes of text `clipboard set` takes: 256 KiB, a quarter of a request's line
/// ([`MAX_LINE`](super::stdio::MAX_LINE)), so that any text but control characters fits it once
/// escaped as JSON. A client that escapes all but ASCII writes a character of 2 or 3 bytes as
/// `\uXXXX`, 6 bytes, and one of 4 bytes as two of them, 12: three times its length at most.
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 256 << 10;

/// The most bytes of a workspace's name an agent gives, in UTF-8: 256, as a keyboard layout's
/// name ([`MAX_LAYOUT_NAME`](crate::MAX_LAYOUT_NAME)), more than a person reads at a glance.
/// `windows`, `workspaces` and `seat list` repeat a workspace's name for each window or
/// workspace that has it, up to thousands in one reply: a name bounded by a request's line
/// alone, 1 MiB, would make one reply gigabytes.
pub const MAX_WORKSPACE_NAME: usize = 256;

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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Map, Value, json};

    use super::arguments::Button;
    use super::results::event_json;
    use super::*;
    use crate::colour::description::{
        Chromaticities, Description, Luminances, PG32UCDM, TransferFunction,
    };
    use crate::colour::{SurfaceColourDescription, SurfaceRole};
    use crate::compositor::{
        ClientProcess, ClientProtocolError, OutputDescription, WindowDescription,
    };
    use crate::mcp::events::{Event, EventData, Kind};
    use crate::mcp::programs::{End, ProgramWakes};
    use crate::mcp::protocol::Revision;
    use crate::rest::{RestCounts, Source};
    use crate::xwayland::Selection;
    use crate::{OutputMode, WindowShell};
    use yuna_layout::SeatId;
    use yuna_layout::geometry::Rect;
    use yuna_layout::{OutputId, WindowId};

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
                "accessibility",
                "capture",
                "pointer",
                "keyboard",
                "clipboard",
                "clock",
                "rest"
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
        // Every tool but `clock` and `rest` names its seat; `rest` may.
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
    fn rest_reads_its_arguments_and_lists_a_seats_own_beside_yunas_parts() {
        let call: RestCall = arguments(args(json!({"seat": 2, "wait_ms": 10}))).expect("read");
        assert_eq!((call.seat, call.wait_ms), (Some(2), Some(10)));
        let call: RestCall = arguments(args(json!({}))).expect("read");
        assert_eq!((call.seat, call.wait_ms), (None, None));
        assert!(arguments::<RestCall>(args(json!({"wait": 10}))).is_err());
        assert!(arguments::<RestCall>(args(json!({"seat": "2"}))).is_err());

        let (ours, theirs) = (SeatId(2), SeatId(3));
        let mut counts = RestCounts::default();
        let mut whole = crate::rest::Tally::default();
        let mut our_tally = crate::rest::Tally::default();
        let mut their_tally = crate::rest::Tally::default();
        let slot = |source: Source| Source::ALL.iter().position(|s| *s == source).unwrap();
        whole.wakes[slot(Source::McpInput)] = 4;
        whole.wakes[slot(Source::ClockRefresh)] = 7;
        whole.wakes[slot(Source::Clients)] = 9;
        our_tally.wakes[slot(Source::ClockRefresh)] = 2;
        our_tally.wakes[slot(Source::Clients)] = 3;
        their_tally.wakes[slot(Source::ClockRefresh)] = 5;
        their_tally.wakes[slot(Source::Clients)] = 6;
        our_tally.frames = 2;
        our_tally.causes[0] = 2;
        whole.frames = 7;
        whole.causes[0] = 7;
        counts.whole = whole;
        counts.seats.insert(ours, our_tally);
        counts.seats.insert(theirs, their_tally);
        let client = |seat: SeatId, wakes: u64| crate::rest::ClientCount {
            seat,
            process: ClientProcess::connecting(Some(40 + seat.0 as i32), false),
            xwayland: false,
            tally: crate::rest::ClientTally {
                wakes,
                commits: wakes,
                draws: wakes,
            },
        };
        counts.clients.insert(1, client(ours, 3));
        counts.clients.insert(2, client(theirs, 6));
        counts.clients.insert(
            3,
            crate::rest::ClientCount {
                tally: crate::rest::ClientTally::default(),
                ..client(ours, 0)
            },
        );
        let programs = [
            ProgramWakes {
                seat: ours,
                program: 1,
                output: 2,
                pace: 0,
            },
            ProgramWakes {
                seat: theirs,
                program: 2,
                output: 0,
                pace: 0,
            },
        ];
        let no_program = |_: SeatId, _: &ClientProcess| None;

        let every = rest_json(&counts, None, None, &programs, &no_program);
        assert_eq!(
            every["sources"],
            json!([{"source": "clients", "wakes": 9}, {"source": "clock_refresh", "wakes": 7},
                   {"source": "mcp_input", "wakes": 4}])
        );
        assert_eq!(
            every["clients"].as_array().map(Vec::len),
            Some(2),
            "{every}"
        );
        assert_eq!(
            every["programs"].as_array().map(Vec::len),
            Some(1),
            "{every}"
        );
        assert_eq!(every["frames"], json!(7));

        let our = rest_json(&counts, Some(ours), Some(10), &programs, &no_program);
        assert_eq!(our["seat"], json!(2));
        assert_eq!(our["wait_ms"], json!(10));
        assert_eq!(
            our["sources"],
            json!([{"source": "clients", "wakes": 3}, {"source": "clock_refresh", "wakes": 2},
                   {"source": "mcp_input", "wakes": 4}])
        );
        assert_eq!(
            our["clients"],
            json!([{"pid": 42, "program": null, "seat": 2, "xwayland": false,
                    "wakes": 3, "commits": 3, "draws": 3}])
        );
        assert_eq!(
            our["programs"],
            json!([{"program": 1, "seat": 2, "output_wakes": 2, "pace_wakes": 0}])
        );
        assert_eq!(our["frames"], json!(2));
        assert_eq!(
            our["draw_causes"],
            json!([{"cause": "commit", "frames": 2}])
        );
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

    /// `MAX_CLIPBOARD_BYTES` says that 1 MiB of data, "its base64 or its escaped text", stays
    /// well under the replies that may wait, `MAX_QUEUED_OUTPUT`, 16 MiB. Measured here: the
    /// reply to a `clipboard get` of 1 MiB of control characters, which are UTF-8 (Stage 1's
    /// audit, m1.13-audit-mcp-6).
    #[test]
    fn a_clipboard_reply_stays_well_under_the_replies_that_may_wait() {
        let data = ClipboardData {
            mime: "text/plain;charset=utf-8".to_owned(),
            bytes: vec![1; MAX_CLIPBOARD_BYTES],
            truncated: false,
            timed_out: false,
        };
        let answer = clipboard_json(&["text/plain;charset=utf-8".to_owned()], Some(&data));
        let reply = crate::mcp::protocol::response(
            json!(1),
            ToolResult::done(answer).into_result(Revision::Modern),
        );
        let bytes = serde_json::to_vec(&reply)
            .expect("a reply serialises")
            .len();
        eprintln!(
            "a reply to 1 MiB of control characters takes {bytes} bytes, against {} that may wait",
            super::super::MAX_QUEUED_OUTPUT
        );
        assert!(
            bytes <= super::super::MAX_QUEUED_OUTPUT / 4,
            "{bytes} bytes for 1 MiB of data"
        );
    }

    /// Text holding a control character JSON writes in six bytes, `\u00XX`, is given in base64;
    /// tabs, newlines and carriage returns, which JSON writes in two, leave text as text.
    #[test]
    fn clipboard_text_with_control_characters_is_base64_but_for_tabs_and_line_ends() {
        use crate::mcp::image::base64;
        let read = |text: &str| {
            let data = ClipboardData {
                mime: "text/plain".to_owned(),
                bytes: text.as_bytes().to_vec(),
                truncated: false,
                timed_out: false,
            };
            clipboard_json(&["text/plain".to_owned()], Some(&data))
        };
        let lines = read("a\tb\r\nc\n");
        assert_eq!(
            (&lines["data"], &lines["base64"]),
            (&json!("a\tb\r\nc\n"), &json!(false))
        );
        for control in ["\u{0}", "\u{1}", "\u{8}", "\u{c}", "\u{1b}", "\u{1f}"] {
            let text = format!("a{control}b");
            let given = read(&text);
            assert_eq!(given["base64"], json!(true), "{text:?}");
            assert_eq!(given["data"], json!(base64(text.as_bytes())), "{text:?}");
            assert_eq!(given["bytes"], json!(3));
        }
    }
}
