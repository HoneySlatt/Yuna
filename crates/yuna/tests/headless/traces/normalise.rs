//! A trace's normalisation: what `wayland-backend`'s client side printed of a scenario's messages
//! under `WAYLAND_DEBUG=client`, read into typed messages and written again in the traces' own
//! format, independent of when and where the scenario ran and of how the server is written.
//!
//! `wayland-backend`'s text is a debugging aid with no promise: [`normalise`] reads it with each
//! message's description from the generated code, typed argument by typed argument, and refuses
//! what it cannot read, so that a change of that text breaks the reading, never the files held.
//!
//! A trace holds, for each client, each object's own messages in order, and nothing of the
//! orders the server chose where the protocol gives none: the interleaving of unrelated objects,
//! the order globals are announced in, the server's numbers for serials and globals. Stage 2a's
//! rewrite may change those without changing a behaviour; a trace changes only where one does.
//! The orders between objects that the protocol requires are held beside each object's messages
//! ([`applied_by`]): an event that another object's event applies, a toplevel's `configure` by its
//! `xdg_surface.configure`, says which one followed it. In the trace's own format:
//!
//! - objects are listed by interface, then rank, each introduced by the message that made it.
//!   An object's rank among its client's objects of its interface, `wl_surface#2`, follows from
//!   how it was made ([`RankKey`]): its maker and the message, the objects that message named,
//!   and for an object bound from a global, the version bound and then its own messages, so
//!   that the order globals came in shows nowhere; last, the order the client made it in;
//! - `wl_display.delete_id` is held on the object it deletes, as `<- delete_id`;
//! - the harness's own roundtrips, `wl_display.sync` and the callbacks it makes, are left out;
//! - a serial ([`SERIALS`]) is named by the event of its client that first carried it, in the
//!   trace's order of objects, `serial of xdg_surface#1.configure[2]`, the second `configure` of
//!   that object; that event itself says `serial`. A serial one more changes its own lines alone;
//! - a registry name is named by the object its client bound from it, `name of wl_output#2`, or
//!   by its interface when the client bound none;
//! - a file descriptor is `<fd>`; a keymap's size, which follows the XKB library, is
//!   `<keymap size>`; a protocol error's text is `<message>`, its object and code kept
//!   ([`MASKED`]); a run of `linux-dmabuf`'s formats is one line ([`COLLAPSED`]);
//! - a time ([`TIMES`]) is kept on the manual clock, in milliseconds from the scenario's earliest
//!   frame callback, and is `<time>` on the real clock, as is a refresh counter (`<count>`);
//! - a set the protocol sends in no order is sorted: an array of enum values ([`SET_ARRAYS`]), and
//!   a run of events or requests that each name one member, formats or globals
//!   ([`SET_MESSAGES`]). Smithay sends a toplevel's capabilities and `wl_shm`'s formats in an
//!   order that changes from one process to the next.
//!
//! The scenario itself writes one kind of line, through [`mark_client`]: the client whose
//! messages follow, when a scenario has several, each with its own objects. Every other line that
//! is not a message, Mesa's or the test harness's on the same standard error, is ignored; a line
//! that starts as a message does but cannot be read is refused.
//!
//! The tables cite the protocols' XML as the crates `Cargo.lock` pins ship it: `wayland.xml` of
//! `wayland-client` 0.31.15, `virtual-keyboard-unstable-v1.xml` of `wayland-protocols-misc`
//! 0.3.12, the others of `wayland-protocols` 0.32.13. A unit test checks each entry against the
//! generated code: the argument it names has the type it is read as.

use std::collections::HashMap;
use std::fmt::Write as _;

use wayland_client::Proxy;
use wayland_client::backend::protocol::{ArgumentType, Interface, MessageDesc};
use wayland_client::protocol::{
    wl_compositor, wl_data_device_manager, wl_display, wl_output, wl_seat, wl_shm, wl_subcompositor,
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_manager_v1;
use wayland_protocols::ext::image_capture_source::v1::client::ext_output_image_capture_source_manager_v1;
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1;
use wayland_protocols::wp::presentation_time::client::wp_presentation;
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_manager_v1;
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3;
use wayland_protocols::wp::viewporter::client::wp_viewporter;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_decoration_manager_v1;
use wayland_protocols::xdg::dialog::v1::client::xdg_wm_dialog_v1;
use wayland_protocols::xdg::shell::client::xdg_wm_base;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1;

/// The clock a trace was recorded on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    /// The in-process compositor's manual clock: times are kept, from the first frame.
    Manual,
    /// `yuna --headless`'s real clock: times are masked.
    Real,
}

impl Clock {
    /// Its name in a trace file's name, `<scenario>.<clock>.trace`.
    pub fn name(self) -> &'static str {
        match self {
            Clock::Manual => "manual",
            Clock::Real => "real",
        }
    }
}

/// The line a scenario writes before another client speaks: its name.
const CLIENT_MARK: &str = "yuna-trace: client ";

/// Marks that the client `name` speaks next: its messages follow, until the next mark.
pub fn mark_client(name: &str) {
    eprintln!("{CLIENT_MARK}{name}");
}

/// Whether a message is a request the client sent or an event it dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Request,
    Event,
}

/// The arguments that are serials: by direction, interface, message and position from 0, with
/// the line of the protocol's XML that names them. Every serial of the protocols the scenarios
/// speak is listed, the core's, xdg-shell's and the selections', whether or not a scenario sends
/// it yet. `zwp_text_input_v3.done`'s serial is not one: it counts the client's own commits.
const SERIALS: &[(Direction, &str, &str, usize, &str)] = {
    use Direction::{Event, Request};
    &[
        (Request, "wl_data_offer", "accept", 0, "wayland.xml:609"),
        (
            Request,
            "wl_data_device",
            "start_drag",
            3,
            "wayland.xml:968",
        ),
        (
            Request,
            "wl_data_device",
            "set_selection",
            1,
            "wayland.xml:983",
        ),
        (Event, "wl_data_device", "enter", 0, "wayland.xml:1006"),
        (Request, "wl_pointer", "set_cursor", 0, "wayland.xml:2190"),
        (Event, "wl_pointer", "enter", 0, "wayland.xml:2206"),
        (Event, "wl_pointer", "leave", 0, "wayland.xml:2220"),
        (Event, "wl_pointer", "button", 0, "wayland.xml:2261"),
        (Event, "wl_keyboard", "enter", 0, "wayland.xml:2630"),
        (Event, "wl_keyboard", "leave", 0, "wayland.xml:2648"),
        (Event, "wl_keyboard", "key", 0, "wayland.xml:2695"),
        (Event, "wl_keyboard", "modifiers", 0, "wayland.xml:2717"),
        (Event, "wl_touch", "down", 0, "wayland.xml:2773"),
        (Event, "wl_touch", "up", 0, "wayland.xml:2787"),
        (Request, "xdg_wm_base", "pong", 0, "xdg-shell.xml:101"),
        (Event, "xdg_wm_base", "ping", 0, "xdg-shell.xml:120"),
        (
            Request,
            "xdg_positioner",
            "set_parent_configure",
            0,
            "xdg-shell.xml:404",
        ),
        (
            Request,
            "xdg_surface",
            "ack_configure",
            0,
            "xdg-shell.xml:595",
        ),
        (Event, "xdg_surface", "configure", 0, "xdg-shell.xml:617"),
        (
            Request,
            "xdg_toplevel",
            "show_window_menu",
            1,
            "xdg-shell.xml:747",
        ),
        (Request, "xdg_toplevel", "move", 1, "xdg-shell.xml:772"),
        (Request, "xdg_toplevel", "resize", 1, "xdg-shell.xml:826"),
        (Request, "xdg_popup", "grab", 1, "xdg-shell.xml:1332"),
        (
            Request,
            "zwp_primary_selection_device_v1",
            "set_selection",
            1,
            "primary-selection-unstable-v1.xml:109",
        ),
        (
            Request,
            "wp_cursor_shape_device_v1",
            "set_shape",
            0,
            "cursor-shape-v1.xml:158",
        ),
        (
            Request,
            "xdg_activation_token_v1",
            "set_serial",
            0,
            "xdg-activation-v1.xml:147",
        ),
    ]
};

/// How a time is carried in an event's arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeArguments {
    /// Milliseconds of `CLOCK_MONOTONIC` in 32 bits, which wrap, at this position.
    Millis(usize),
    /// A timestamp in three arguments from this position: the seconds' high and low 32 bits, then
    /// the nanoseconds. Held as one.
    Timestamp(usize),
    /// A refresh counter in two arguments from this position, its high and low 32 bits: it counts
    /// the output's refreshes since its creation, the scenario's start on the manual clock. Held
    /// as one.
    RefreshCount(usize),
}

impl TimeArguments {
    /// The positions the time takes.
    fn positions(self) -> std::ops::Range<usize> {
        match self {
            TimeArguments::Millis(index) => index..index + 1,
            TimeArguments::Timestamp(index) => index..index + 3,
            TimeArguments::RefreshCount(index) => index..index + 2,
        }
    }
}

/// The times the compositor sends, by interface and event, with the line of the protocol's XML
/// that names them. Times a client sends, a virtual pointer's or keyboard's, are the client's own:
/// the test drivers send 0, kept as it is. A `wl_callback`'s `done` is a time only for a callback
/// made by `wl_surface.frame`; a sync's is undefined, 0 from `wayland-backend`, and kept.
const TIMES: &[(&str, &str, TimeArguments, &str)] = {
    use TimeArguments::{Millis, RefreshCount, Timestamp};
    &[
        (
            "wl_callback",
            "done",
            Millis(0),
            "wayland.xml:1660, wl_surface.frame",
        ),
        ("wl_data_device", "motion", Millis(0), "wayland.xml:1029"),
        ("wl_pointer", "motion", Millis(0), "wayland.xml:2230"),
        ("wl_pointer", "button", Millis(1), "wayland.xml:2262"),
        ("wl_pointer", "axis", Millis(0), "wayland.xml:2294"),
        ("wl_pointer", "axis_stop", Millis(0), "wayland.xml:2425"),
        ("wl_keyboard", "key", Millis(1), "wayland.xml:2696"),
        ("wl_touch", "down", Millis(1), "wayland.xml:2774"),
        ("wl_touch", "up", Millis(1), "wayland.xml:2788"),
        ("wl_touch", "motion", Millis(0), "wayland.xml:2796"),
        (
            "wp_presentation_feedback",
            "presented",
            Timestamp(0),
            "presentation-time.xml:247",
        ),
        (
            "wp_presentation_feedback",
            "presented",
            RefreshCount(4),
            "presentation-time.xml:254",
        ),
        (
            "ext_image_copy_capture_frame_v1",
            "presentation_time",
            Timestamp(0),
            "ext-image-copy-capture-v1.xml:324",
        ),
    ]
};

/// The arguments masked, with what replaces them: what follows a library rather than Yuna, and a
/// protocol error's text, which says the same in other words from one implementation to the next.
/// By direction, interface, message and position, with the line of the protocol's XML that names
/// them.
const MASKED: &[(Direction, &str, &str, usize, &str, &str)] = {
    use Direction::{Event, Request};
    &[
        (
            Event,
            "wl_display",
            "error",
            2,
            "<message>",
            "wayland.xml:83",
        ),
        (
            Event,
            "wl_keyboard",
            "keymap",
            2,
            "<keymap size>",
            "wayland.xml:2611",
        ),
        (
            Request,
            "zwp_virtual_keyboard_v1",
            "keymap",
            2,
            "<keymap size>",
            "virtual-keyboard-unstable-v1.xml:47",
        ),
    ]
};

/// The arrays of 32-bit enum values whose order the protocol gives no meaning, by interface,
/// event and position, with the line of the protocol's XML that names them: kept sorted.
const SET_ARRAYS: &[(&str, &str, usize, &str)] = &[
    ("xdg_toplevel", "configure", 2, "xdg-shell.xml:1171"),
    ("xdg_toplevel", "wm_capabilities", 0, "xdg-shell.xml:1243"),
];

/// The messages that each name one member of a set, sent one after the other in no order the
/// protocol gives, by direction, interface and message, with the line of the protocol's XML that
/// describes them: a run of them on one object is kept sorted. The registry's globals come in the
/// server's order, and a client binds them as they come.
const SET_MESSAGES: &[(Direction, &str, &str, &str)] = {
    use Direction::{Event, Request};
    &[
        (Event, "wl_registry", "global", "wayland.xml:150"),
        (Request, "wl_registry", "bind", "wayland.xml:141"),
        (Event, "wl_shm", "format", "wayland.xml:498"),
        (
            Event,
            "ext_image_copy_capture_session_v1",
            "shm_format",
            "ext-image-copy-capture-v1.xml:121",
        ),
        (
            Event,
            "ext_image_copy_capture_session_v1",
            "dmabuf_format",
            "ext-image-copy-capture-v1.xml:144",
        ),
    ]
};

/// The messages a run of which on one object is held as one line, its arguments replaced, by
/// direction, interface and message, with what replaces them and why. `linux-dmabuf`'s formats
/// (linux-dmabuf-v1.xml:111, 128) are those the renderer imports, Mesa's list on llvmpipe, some
/// fifty a client: they change with Mesa and with the renderer, not with Yuna's protocol code,
/// and no scenario sends a dmabuf. That the formats are offered, and in which events, is kept.
const COLLAPSED: &[(Direction, &str, &str, &str)] = {
    use Direction::Event;
    &[
        (
            Event,
            "zwp_linux_dmabuf_v1",
            "format",
            "<the renderer's formats>",
        ),
        (
            Event,
            "zwp_linux_dmabuf_v1",
            "modifier",
            "<the renderer's formats>",
        ),
    ]
};

/// The registry names, by direction, message and position on `wl_registry` (wayland.xml:141, 150,
/// 163). A name is the server's own number: the trace names it by the object its client bound
/// from it, `name of wl_output#2`, or by its interface alone when the client bound none.
const GLOBAL_NAMES: &[(Direction, &str, usize)] = &[
    (Direction::Event, "global", 0),
    (Direction::Request, "bind", 0),
    (Direction::Event, "global_remove", 0),
];

/// The globals the test clients bind. Every interface their messages name is one of these, or is
/// made by a request or an event of one ([`known_interfaces`]); a message of another is refused,
/// for its arguments could not be read.
fn bound_globals() -> [&'static Interface; 24] {
    [
        wl_display::WlDisplay::interface(),
        wl_compositor::WlCompositor::interface(),
        wl_subcompositor::WlSubcompositor::interface(),
        wl_shm::WlShm::interface(),
        wl_output::WlOutput::interface(),
        wl_seat::WlSeat::interface(),
        wl_data_device_manager::WlDataDeviceManager::interface(),
        xdg_wm_base::XdgWmBase::interface(),
        zxdg_decoration_manager_v1::ZxdgDecorationManagerV1::interface(),
        zxdg_output_manager_v1::ZxdgOutputManagerV1::interface(),
        wp_presentation::WpPresentation::interface(),
        wp_viewporter::WpViewporter::interface(),
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1::interface(),
        zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1::interface(),
        xdg_activation_v1::XdgActivationV1::interface(),
        xdg_wm_dialog_v1::XdgWmDialogV1::interface(),
        wp_cursor_shape_manager_v1::WpCursorShapeManagerV1::interface(),
        zwp_text_input_manager_v3::ZwpTextInputManagerV3::interface(),
        zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1::interface(),
        ext_data_control_manager_v1::ExtDataControlManagerV1::interface(),
        ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1::interface(
        ),
        ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1::interface(),
        zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1::interface(),
        zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1::interface(),
    ]
}

/// The interfaces the test clients may speak, by name: [`bound_globals`], and every interface
/// their messages make or name, found by walking the generated code's descriptions.
fn known_interfaces() -> HashMap<&'static str, &'static Interface> {
    let mut known = HashMap::new();
    let mut to_visit: Vec<&'static Interface> = bound_globals().to_vec();
    while let Some(interface) = to_visit.pop() {
        if known.insert(interface.name, interface).is_some() {
            continue;
        }
        for message in interface.requests.iter().chain(interface.events) {
            to_visit.extend(message.child_interface);
            to_visit.extend(message.arg_interfaces.iter().copied());
        }
    }
    known
}

/// One message as `wayland-backend` printed it, its arguments still text.
#[derive(Debug, PartialEq)]
struct Printed<'a> {
    /// A request sent on an object the server had already destroyed.
    discarded: bool,
    direction: Direction,
    interface: &'a str,
    object: u32,
    name: &'a str,
    arguments: Vec<&'a str>,
}

/// One message, its arguments read by their types.
#[derive(Debug)]
struct Message {
    discarded: bool,
    direction: Direction,
    interface: &'static str,
    object: u32,
    name: &'static str,
    arguments: Vec<Value>,
}

/// An argument, by its type in the message's description.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Int(i32),
    Uint(u32),
    /// In 256ths.
    Fixed(i32),
    Str(Option<Vec<u8>>),
    /// The object's id, none for a null object.
    Object(Option<u32>),
    /// A new object: its interface and id.
    NewId(&'static str, u32),
    Array(Vec<u8>),
    Fd,
}

/// Turns `raw`, what a scenario's process wrote on standard error, into its trace, as the
/// module's documentation says. Refused, with the line and why: a line that starts as a message
/// does but cannot be read; a message of an interface the clients do not bind, of an object the
/// client never made, or whose arguments do not match its description; and, on the manual clock,
/// times without a frame callback to count them from.
pub fn normalise(raw: &str, clock: Clock) -> Result<String, String> {
    let interfaces = known_interfaces();
    let mut trace = Trace::default();
    for (number, line) in raw.lines().enumerate() {
        let refused = |why: String| format!("line {}: {why}: {line:?}", number + 1);
        if let Some(client) = line.strip_prefix(CLIENT_MARK) {
            trace.switch_to(client);
        } else if let Some(printed) = parse_line(line) {
            let printed = printed.map_err(refused)?;
            let message = read_types(&printed, &interfaces).map_err(refused)?;
            trace.add(&message).map_err(refused)?;
        }
    }
    trace.render(clock)
}

/// Reads a line `wayland-backend` printed (`debug.rs`): `[<seconds>.<millis>][rs]`, then
/// `[discarded]` for a request on a dead object, then ` -> interface@id.request(arguments)` or
/// ` <- interface@id.event, (arguments)`. None for a line without that timestamp, which is not
/// a message; an error for one with it that cannot be read.
fn parse_line(line: &str) -> Option<Result<Printed<'_>, String>> {
    let (stamp, rest) = line.strip_prefix('[')?.split_once("][rs]")?;
    if stamp.is_empty()
        || !stamp
            .chars()
            .all(|c| c.is_ascii_digit() || c == ' ' || c == '.')
    {
        return None;
    }
    Some(parse_after_stamp(rest))
}

fn parse_after_stamp(rest: &str) -> Result<Printed<'_>, String> {
    let (discarded, rest) = match rest.strip_prefix("[discarded]") {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    let (direction, rest) = if let Some(rest) = rest.strip_prefix(" -> ") {
        (Direction::Request, rest)
    } else if let Some(rest) = rest.strip_prefix(" <- ") {
        (Direction::Event, rest)
    } else {
        return Err("no direction".to_owned());
    };
    let open = rest
        .find('(')
        .ok_or_else(|| "no arguments' parenthesis".to_owned())?;
    let head = &rest[..open];
    // An event's name is followed by ", " before its arguments.
    let head = match direction {
        Direction::Event => head
            .strip_suffix(", ")
            .ok_or_else(|| "no comma after an event's name".to_owned())?,
        Direction::Request => head,
    };
    let arguments = rest[open + 1..]
        .strip_suffix(')')
        .ok_or_else(|| "no closing parenthesis".to_owned())?;
    let (interface, object_and_name) =
        head.split_once('@').ok_or_else(|| "no object".to_owned())?;
    let (object, name) = object_and_name
        .split_once('.')
        .ok_or_else(|| "no message's name".to_owned())?;
    let object = object
        .parse()
        .map_err(|_| "an object id that is not a number".to_owned())?;
    Ok(Printed {
        discarded,
        direction,
        interface,
        object,
        name,
        arguments: split_arguments(arguments)?,
    })
}

/// Splits printed arguments at the commas between them, not those inside a string (`Some("a,
/// b")`), an array (`[1, 2]`) or an option's parentheses.
fn split_arguments(text: &str) -> Result<Vec<&str>, String> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut arguments = Vec::new();
    let (mut depth, mut in_string, mut escaped, mut from) = (0_i32, false, false, 0);
    for (index, c) in text.char_indices() {
        if in_string {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                arguments.push(&text[from..index]);
                from = index + 2;
            }
            _ => {}
        }
        if depth < 0 {
            return Err("unbalanced arguments".to_owned());
        }
    }
    if in_string || depth != 0 {
        return Err("unbalanced arguments".to_owned());
    }
    arguments.push(&text[from.min(text.len())..]);
    Ok(arguments)
}

/// Reads `printed`'s arguments by the types its description in the generated code gives.
fn read_types(
    printed: &Printed<'_>,
    interfaces: &HashMap<&'static str, &'static Interface>,
) -> Result<Message, String> {
    let interface = known_interface(printed.interface, interfaces)?;
    let messages = match printed.direction {
        Direction::Request => interface.requests,
        Direction::Event => interface.events,
    };
    let description: &'static MessageDesc = messages
        .iter()
        .find(|description| description.name == printed.name)
        .ok_or_else(|| format!("{} has no message {}", printed.interface, printed.name))?;
    if description.signature.len() != printed.arguments.len() {
        return Err(format!(
            "{} arguments where its description has {}",
            printed.arguments.len(),
            description.signature.len()
        ));
    }
    let arguments = description
        .signature
        .iter()
        .zip(&printed.arguments)
        .map(|(kind, text)| read_value(*kind, text, interfaces))
        .collect::<Result<_, _>>()?;
    Ok(Message {
        discarded: printed.discarded,
        direction: printed.direction,
        interface: interface.name,
        object: printed.object,
        name: description.name,
        arguments,
    })
}

fn known_interface(
    name: &str,
    interfaces: &HashMap<&'static str, &'static Interface>,
) -> Result<&'static Interface, String> {
    interfaces.get(name).copied().ok_or_else(|| {
        format!(
            "{name} is not an interface the test clients bind: add its global to `bound_globals`"
        )
    })
}

/// Reads one argument of type `kind` from its text: an integer as a number; a fixed number with
/// four decimals, from which its 256ths come back exactly; a string as Rust's `Debug` writes an
/// option of one; an object or a new object as `interface@id`, a null object as `<anonymous>@0`;
/// an array as its bytes.
fn read_value(
    kind: ArgumentType,
    text: &str,
    interfaces: &HashMap<&'static str, &'static Interface>,
) -> Result<Value, String> {
    let wrong = || format!("{text:?} is not a {kind:?}");
    Ok(match kind {
        ArgumentType::Int => Value::Int(text.parse().map_err(|_| wrong())?),
        ArgumentType::Uint => Value::Uint(text.parse().map_err(|_| wrong())?),
        ArgumentType::Fixed => {
            let value: f64 = text.parse().map_err(|_| wrong())?;
            // Four decimals are closer than half a 256th: the rounding finds it back.
            Value::Fixed((value * 256.0).round() as i32)
        }
        ArgumentType::Str(_) => match text {
            "None" => Value::Str(None),
            _ => {
                let quoted = text
                    .strip_prefix("Some(")
                    .and_then(|rest| rest.strip_suffix(')'))
                    .ok_or_else(wrong)?;
                Value::Str(Some(unescape(quoted).ok_or_else(wrong)?))
            }
        },
        ArgumentType::Object(_) => {
            let (interface, id) = text.rsplit_once('@').ok_or_else(wrong)?;
            let id: u32 = id.parse().map_err(|_| wrong())?;
            if id == 0 {
                Value::Object(None)
            } else {
                known_interface(interface, interfaces)?;
                Value::Object(Some(id))
            }
        }
        ArgumentType::NewId => {
            let (interface, id) = text.rsplit_once('@').ok_or_else(wrong)?;
            let interface = known_interface(interface, interfaces)?.name;
            Value::NewId(interface, id.parse().map_err(|_| wrong())?)
        }
        ArgumentType::Array => {
            let inside = text
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
                .ok_or_else(wrong)?;
            let bytes = if inside.is_empty() {
                Vec::new()
            } else {
                inside
                    .split(", ")
                    .map(|byte| byte.parse().map_err(|_| wrong()))
                    .collect::<Result<_, _>>()?
            };
            Value::Array(bytes)
        }
        ArgumentType::Fd => {
            text.parse::<i32>().map_err(|_| wrong())?;
            Value::Fd
        }
    })
}

/// The bytes of a string as `Debug` writes a `CStr` (`core::bstr`): quoted, a byte of ASCII
/// escaped as `escape_ascii` does, `\0`, a character past ASCII as itself or `\u{…}`, and a byte
/// that is no UTF-8 as `\x..`. None for a text it could not have written.
fn unescape(quoted: &str) -> Option<Vec<u8>> {
    let inside = quoted.strip_prefix('"')?.strip_suffix('"')?;
    let mut bytes = Vec::with_capacity(inside.len());
    let mut chars = inside.chars();
    let push_char = |bytes: &mut Vec<u8>, c: char| {
        let mut buffer = [0; 4];
        bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
    };
    while let Some(c) = chars.next() {
        if c != '\\' {
            push_char(&mut bytes, c);
            continue;
        }
        match chars.next()? {
            '0' => bytes.push(0),
            't' => bytes.push(b'\t'),
            'r' => bytes.push(b'\r'),
            'n' => bytes.push(b'\n'),
            c @ ('\\' | '\'' | '"') => bytes.push(c as u8),
            'x' => {
                let hex: String = chars.by_ref().take(2).collect();
                bytes.push(u8::from_str_radix(&hex, 16).ok()?);
            }
            'u' => {
                if chars.next()? != '{' {
                    return None;
                }
                let code: String = chars.by_ref().take_while(|c| *c != '}').collect();
                push_char(
                    &mut bytes,
                    char::from_u32(u32::from_str_radix(&code, 16).ok()?)?,
                );
            }
            _ => return None,
        }
    }
    Some(bytes)
}

/// An argument as the trace holds it, but for what is named only once the whole trace is read:
/// objects by their rank, serials by the event that carried them, registry names by the object
/// bound from them, and times by the first frame they count from.
#[derive(Debug, Clone)]
enum Held {
    Text(String),
    /// An object, by its index among its client's.
    Object(usize),
    /// An object the message made.
    NewObject(usize),
    Serial(u32),
    /// A registry name, as the `wl_registry` object `registry` carried it.
    GlobalName {
        name: u32,
        registry: usize,
    },
    /// Milliseconds of the 32-bit clock.
    Millis(u32),
    /// A timestamp, in nanoseconds.
    Timestamp(u128),
    /// A refresh counter.
    RefreshCount(u64),
}

/// One message of an object, or its deletion.
#[derive(Debug)]
struct Entry {
    direction: Direction,
    discarded: bool,
    name: &'static str,
    arguments: Vec<Held>,
    /// The event of another object that followed it, as the protocol requires: that object, the
    /// event, and its rank among that object's events of that name.
    before: Option<(usize, &'static str, usize)>,
}

/// One object of a client, and its messages in order.
#[derive(Debug)]
struct Object {
    interface: &'static str,
    /// The object and message that made it; none for `wl_display`.
    made_by: Option<(usize, &'static str)>,
    /// The other objects the message that made it named.
    named_with: Vec<usize>,
    /// The version it was bound at, or the one of the object that made it.
    version: u32,
    /// For an object bound from a global: the version it was bound at, which with its interface
    /// says what it is, whatever the order the globals came in.
    bound_at: Option<u32>,
    /// Made by `wl_display.sync`: one of the harness's own roundtrips, which the trace leaves out.
    roundtrip: bool,
    entries: Vec<Entry>,
}

/// The entries waiting for the event of another object that applies them: the waiting object
/// and entry, and the events that apply them.
type Waiting = Vec<(usize, usize, &'static [&'static str])>;

/// One client's objects, in the order they were made.
#[derive(Debug)]
struct ClientTrace {
    name: String,
    objects: Vec<Object>,
    /// The object each live id is, by id.
    ids: HashMap<u32, usize>,
    /// The object bound from each registry name, by the registry that bound it and the name.
    binds: HashMap<(usize, u32), usize>,
    /// The entries waiting, by the object whose event applies them.
    waiting: HashMap<usize, Waiting>,
}

impl ClientTrace {
    fn new(name: &str) -> ClientTrace {
        let mut client = ClientTrace {
            name: name.to_owned(),
            objects: Vec::new(),
            ids: HashMap::new(),
            binds: HashMap::new(),
            waiting: HashMap::new(),
        };
        client.objects.push(Object {
            interface: "wl_display",
            made_by: None,
            named_with: Vec::new(),
            version: 1,
            bound_at: None,
            roundtrip: false,
            entries: Vec::new(),
        });
        client.ids.insert(1, 0);
        client
    }

    /// The object the live id `id` is.
    fn object(&self, id: u32) -> Result<usize, String> {
        self.ids
            .get(&id)
            .copied()
            .ok_or_else(|| format!("object {id} was never made, or is deleted"))
    }
}

/// What normalising a trace gathers, before it is written.
#[derive(Debug, Default)]
struct Trace {
    clients: Vec<ClientTrace>,
    /// The client that speaks: an index in `clients`.
    current: usize,
    /// Each registry name's interface, as its `global` announced it.
    globals: HashMap<u32, String>,
    /// The earliest frame callback's time, from which the manual clock's times count.
    first_frame: Option<u32>,
    /// Whether the trace holds a time.
    has_times: bool,
}

impl Trace {
    fn switch_to(&mut self, name: &str) {
        self.current = match self.clients.iter().position(|client| client.name == name) {
            Some(index) => index,
            None => {
                self.clients.push(ClientTrace::new(name));
                self.clients.len() - 1
            }
        };
    }

    /// Adds `message` to its object's messages, making the objects it makes. A roundtrip's
    /// messages make their callback, but are left out.
    fn add(&mut self, message: &Message) -> Result<(), String> {
        if self.clients.is_empty() {
            self.switch_to("");
        }
        let client = &mut self.clients[self.current];
        let object = client.object(message.object)?;
        if client.objects[object].interface != message.interface {
            return Err(format!(
                "object {} is a {}",
                message.object, client.objects[object].interface
            ));
        }
        if (message.interface, message.name) == ("wl_display", "delete_id") {
            let [Value::Uint(id)] = message.arguments[..] else {
                return Err("delete_id without an id".to_owned());
            };
            let deleted = client.object(id)?;
            client.ids.remove(&id);
            if !client.objects[deleted].roundtrip {
                client.objects[deleted].entries.push(Entry {
                    direction: Direction::Event,
                    discarded: false,
                    name: "delete_id",
                    arguments: Vec::new(),
                    before: None,
                });
            }
            return Ok(());
        }
        let arguments = self.hold(message, object)?;
        let client = &mut self.clients[self.current];
        if client.objects[object].roundtrip {
            return Ok(());
        }
        if (message.direction, message.interface, message.name)
            == (Direction::Request, "wl_display", "sync")
        {
            for argument in &arguments {
                if let Held::NewObject(callback) = argument {
                    client.objects[*callback].roundtrip = true;
                }
            }
            return Ok(());
        }
        let entry = client.objects[object].entries.len();
        client.objects[object].entries.push(Entry {
            direction: message.direction,
            discarded: message.discarded,
            name: message.name,
            arguments,
            before: None,
        });
        if message.direction == Direction::Event {
            apply_waiting(client, object, message.name);
            if let Some((applier, events)) = applied_by(client, object, message.name) {
                client
                    .waiting
                    .entry(applier)
                    .or_default()
                    .push((object, entry, events));
            }
        }
        Ok(())
    }

    /// `message`'s arguments as the trace holds them, sent to or by `object`: new objects made,
    /// serials, registry names, masks, sets and times.
    fn hold(&mut self, message: &Message, object: usize) -> Result<Vec<Held>, String> {
        let key = (message.direction, message.interface, message.name);
        let is_bind = key == (Direction::Request, "wl_registry", "bind");
        let client = &mut self.clients[self.current];
        let named_with: Vec<usize> = message
            .arguments
            .iter()
            .filter_map(|value| match value {
                Value::Object(Some(id)) => client.ids.get(id).copied(),
                _ => None,
            })
            .collect();
        // A bound object has the version bound; another, its maker's.
        let version = match (is_bind, message.arguments.get(2)) {
            (true, Some(Value::Uint(version))) => *version,
            (true, _) => return Err("a bind without its version".to_owned()),
            (false, _) => client.objects[object].version,
        };
        let mut held: Vec<Option<Held>> = Vec::with_capacity(message.arguments.len());
        for value in &message.arguments {
            held.push(Some(match value {
                Value::Int(value) => Held::Text(value.to_string()),
                Value::Uint(value) => Held::Text(value.to_string()),
                Value::Fixed(value) => Held::Text((f64::from(*value) / 256.0).to_string()),
                Value::Str(None) | Value::Object(None) => Held::Text("null".to_owned()),
                Value::Str(Some(bytes)) => Held::Text(format!("\"{}\"", bytes.escape_ascii())),
                Value::Object(Some(id)) => Held::Object(client.object(*id)?),
                Value::NewId(interface, id) => {
                    client.objects.push(Object {
                        interface,
                        made_by: Some((object, message.name)),
                        named_with: named_with.clone(),
                        version,
                        bound_at: is_bind.then_some(version),
                        roundtrip: false,
                        entries: Vec::new(),
                    });
                    let made = client.objects.len() - 1;
                    client.ids.insert(*id, made);
                    Held::NewObject(made)
                }
                Value::Array(bytes) => Held::Text(array_text(bytes)),
                Value::Fd => Held::Text("<fd>".to_owned()),
            }));
        }
        for &(direction, interface, name, index, _) in SERIALS {
            if (direction, interface, name) == key {
                let Value::Uint(serial) = message.arguments[index] else {
                    return Err("a serial that is not a uint".to_owned());
                };
                held[index] = Some(Held::Serial(serial));
            }
        }
        if message.interface == "wl_registry" {
            self.hold_global_names(message, object, &mut held)?;
        }
        for &(direction, interface, name, index, mask, _) in MASKED {
            if (direction, interface, name) == key {
                held[index] = Some(Held::Text(mask.to_owned()));
            }
        }
        for &(interface, name, index, _) in SET_ARRAYS {
            if (Direction::Event, interface, name) == key {
                let Value::Array(bytes) = &message.arguments[index] else {
                    return Err("a set that is not an array".to_owned());
                };
                held[index] = Some(Held::Text(array_text(&sorted_values(bytes)?)));
            }
        }
        for &(direction, interface, name, mask) in COLLAPSED {
            if (direction, interface, name) == key {
                return Ok(vec![Held::Text(mask.to_owned())]);
            }
        }
        if message.direction == Direction::Event && self.is_timed(message, object) {
            self.hold_times(message, &mut held)?;
        }
        Ok(held.into_iter().flatten().collect())
    }

    /// Whether `message`, an event of `object`, carries times: always for those of [`TIMES`],
    /// but a `wl_callback`'s `done` only for a callback made by `wl_surface.frame`.
    fn is_timed(&self, message: &Message, object: usize) -> bool {
        let client = &self.clients[self.current];
        let made_by_a_frame = || {
            client.objects[object]
                .made_by
                .is_some_and(|(maker, request)| {
                    client.objects[maker].interface == "wl_surface" && request == "frame"
                })
        };
        TIMES
            .iter()
            .any(|&(interface, name, ..)| (interface, name) == (message.interface, message.name))
            && (message.interface != "wl_callback" || made_by_a_frame())
    }

    /// Holds the times of `message`'s arguments, each as one argument where it takes several.
    fn hold_times(&mut self, message: &Message, held: &mut [Option<Held>]) -> Result<(), String> {
        let uint = |index: usize| match message.arguments[index] {
            Value::Uint(value) => Ok(value),
            _ => Err("a time that is not a uint".to_owned()),
        };
        for &(interface, name, at, _) in TIMES {
            if (interface, name) != (message.interface, message.name) {
                continue;
            }
            self.has_times = true;
            let time = match at {
                TimeArguments::Millis(index) => {
                    let millis = uint(index)?;
                    if message.interface == "wl_callback" {
                        // The earliest, on a 32-bit clock that may wrap during a scenario.
                        let earlier = |first: u32| (millis.wrapping_sub(first) as i32) < 0;
                        if self.first_frame.is_none_or(earlier) {
                            self.first_frame = Some(millis);
                        }
                    }
                    Held::Millis(millis)
                }
                TimeArguments::Timestamp(index) => {
                    let seconds = (u64::from(uint(index)?) << 32) | u64::from(uint(index + 1)?);
                    Held::Timestamp(
                        u128::from(seconds) * 1_000_000_000 + u128::from(uint(index + 2)?),
                    )
                }
                TimeArguments::RefreshCount(index) => Held::RefreshCount(
                    (u64::from(uint(index)?) << 32) | u64::from(uint(index + 1)?),
                ),
            };
            let positions = at.positions();
            held[positions.start] = Some(time);
            for index in positions.skip(1) {
                held[index] = None;
            }
        }
        Ok(())
    }

    /// Holds the registry's names, read when the trace is written, and remembers what each
    /// names: its interface, from its `global`, and the object a `bind` made of it.
    fn hold_global_names(
        &mut self,
        message: &Message,
        registry: usize,
        held: &mut [Option<Held>],
    ) -> Result<(), String> {
        for &(direction, name, index) in GLOBAL_NAMES {
            if (direction, name) != (message.direction, message.name) {
                continue;
            }
            let Value::Uint(global) = message.arguments[index] else {
                return Err("a registry name that is not a uint".to_owned());
            };
            if name == "global" {
                let Value::Str(Some(interface)) = &message.arguments[1] else {
                    return Err("a global without its interface".to_owned());
                };
                let interface = String::from_utf8_lossy(interface).into_owned();
                self.globals.insert(global, interface);
            }
            if name == "bind"
                && let Some(Some(Held::NewObject(bound))) = held.get(3)
            {
                self.clients[self.current]
                    .binds
                    .insert((registry, global), *bound);
            }
            held[index] = Some(Held::GlobalName {
                name: global,
                registry,
            });
        }
        Ok(())
    }

    /// The trace's text: for each client, its objects ranked and listed by interface, each with
    /// its messages in order.
    fn render(self, clock: Clock) -> Result<String, String> {
        if clock == Clock::Manual && self.has_times && self.first_frame.is_none() {
            return Err("times, but no frame callback to count them from".to_owned());
        }
        let mut text = String::new();
        for client in &self.clients {
            if !client.name.is_empty() {
                writeln!(text, "client {}", client.name).expect("a string takes text");
            }
            let names = Names::of(client, &self.globals, clock)?;
            let writer = Writer {
                client,
                globals: &self.globals,
                clock,
                first_frame: self.first_frame.unwrap_or(0),
                names: Some(&names),
            };
            for &index in &names.order {
                let object = &client.objects[index];
                write!(text, "{}", names.label(client, index)).expect("a string takes text");
                if let Some((maker, message)) = object.made_by {
                    write!(text, " = {}.{message}", names.label(client, maker))
                        .expect("a string takes text");
                }
                text.push('\n');
                for line in writer.object_lines(index) {
                    writeln!(text, "  {line}").expect("a string takes text");
                }
            }
        }
        Ok(text)
    }
}

/// How the objects of one client are named in its trace: each by its interface and its rank,
/// in an order that holds nothing of the order the server announced its globals in, and each
/// serial by the event that first carried it.
struct Names {
    /// Each object's rank among the client's objects of its interface, from 1; 0 for the
    /// roundtrips left out.
    ranks: Vec<usize>,
    /// The objects as the trace lists them: by interface, then rank, the roundtrips left out.
    order: Vec<usize>,
    /// The event that first carried each serial, in the order the trace lists objects: its object
    /// and entry.
    serials: HashMap<u32, (usize, usize)>,
}

/// What an object's rank is sorted by: its maker and the message that made it, the objects that
/// message named, for an object bound from a global the version and then its own messages, as
/// two such objects of one registry are told apart, and last the order it was made in.
type RankKey = Vec<(String, usize)>;

impl Names {
    fn of(
        client: &ClientTrace,
        globals: &HashMap<u32, String>,
        clock: Clock,
    ) -> Result<Names, String> {
        let mut ranking = Ranking {
            client,
            globals,
            clock,
            ranks: vec![None; client.objects.len()],
            ranking: Vec::new(),
        };
        for (index, object) in client.objects.iter().enumerate() {
            if !object.roundtrip {
                ranking.rank(index)?;
            }
        }
        let ranks: Vec<usize> = ranking.ranks.iter().map(|rank| rank.unwrap_or(0)).collect();
        let mut order: Vec<usize> = (0..client.objects.len())
            .filter(|&index| !client.objects[index].roundtrip)
            .collect();
        order.sort_by_key(|&index| (client.objects[index].interface, ranks[index]));
        let mut serials = HashMap::new();
        for &index in &order {
            for (entry_index, entry) in client.objects[index].entries.iter().enumerate() {
                if entry.direction != Direction::Event {
                    continue;
                }
                for argument in &entry.arguments {
                    if let Held::Serial(serial) = argument {
                        serials.entry(*serial).or_insert((index, entry_index));
                    }
                }
            }
        }
        Ok(Names {
            ranks,
            order,
            serials,
        })
    }

    fn label(&self, client: &ClientTrace, object: usize) -> String {
        format!(
            "{}#{}",
            client.objects[object].interface, self.ranks[object]
        )
    }
}

/// The ranks of one client's objects, found interface by interface.
struct Ranking<'a> {
    client: &'a ClientTrace,
    globals: &'a HashMap<u32, String>,
    clock: Clock,
    ranks: Vec<Option<usize>>,
    /// The interfaces whose ranks are being found, to refuse ranks that depend on themselves.
    ranking: Vec<&'static str>,
}

impl Ranking<'_> {
    /// The rank of `object`, found with those of every object of its interface.
    fn rank(&mut self, object: usize) -> Result<usize, String> {
        if let Some(rank) = self.ranks[object] {
            return Ok(rank);
        }
        let client = self.client;
        let objects = &client.objects;
        let interface = objects[object].interface;
        if self.ranking.contains(&interface) {
            return Err(format!("the ranks of {interface} depend on themselves"));
        }
        self.ranking.push(interface);
        let mut keyed: Vec<(RankKey, usize)> = Vec::new();
        for (index, member) in objects.iter().enumerate() {
            if member.interface == interface && !member.roundtrip {
                keyed.push((self.key(index)?, index));
            }
        }
        keyed.sort();
        for (rank, (_, index)) in keyed.into_iter().enumerate() {
            self.ranks[index] = Some(rank + 1);
        }
        self.ranking.pop();
        self.ranks[object].ok_or_else(|| format!("{interface} has no rank"))
    }

    fn key(&mut self, object: usize) -> Result<RankKey, String> {
        let client = self.client;
        let objects = &client.objects;
        let this = &objects[object];
        let mut key = RankKey::new();
        if let Some((maker, message)) = this.made_by {
            key.push((objects[maker].interface.to_owned(), self.rank(maker)?));
            key.push((message.to_owned(), 0));
        }
        for &named in &this.named_with {
            key.push((objects[named].interface.to_owned(), self.rank(named)?));
        }
        if let Some(version) = this.bound_at {
            key.push((String::new(), version as usize));
            let writer = Writer {
                client: self.client,
                globals: self.globals,
                clock: self.clock,
                first_frame: 0,
                names: None,
            };
            key.push((writer.object_lines(object).join("\n"), 0));
        }
        key.push((String::new(), object));
        Ok(key)
    }
}

/// Writes one client's entries, with the names of [`Names`], or with interfaces alone when they
/// are not known yet: what tells apart objects that are the same in every other way.
struct Writer<'a> {
    client: &'a ClientTrace,
    globals: &'a HashMap<u32, String>,
    clock: Clock,
    first_frame: u32,
    names: Option<&'a Names>,
}

impl Writer<'_> {
    /// `object`'s lines: each entry's, sets sorted and collapsed runs held as one.
    fn object_lines(&self, object: usize) -> Vec<String> {
        let this = &self.client.objects[object];
        let mut lines: Vec<(Option<(Direction, &str)>, String)> = this
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let key = (entry.direction, this.interface, entry.name);
                let member = SET_MESSAGES
                    .iter()
                    .any(|&(direction, interface, name, _)| (direction, interface, name) == key)
                    .then_some((entry.direction, entry.name));
                (member, self.entry_text(object, index))
            })
            .collect();
        sort_runs_of_set_members(&mut lines);
        let mut kept: Vec<String> = Vec::with_capacity(lines.len());
        for (index, (_, line)) in lines.into_iter().enumerate() {
            let entry = &this.entries[index];
            let collapsed = COLLAPSED.iter().any(|&(direction, interface, name, _)| {
                (direction, interface, name) == (entry.direction, this.interface, entry.name)
            });
            if !(collapsed && kept.last() == Some(&line)) {
                kept.push(line);
            }
        }
        kept
    }

    fn label(&self, object: usize) -> String {
        match self.names {
            Some(names) => names.label(self.client, object),
            None => self.client.objects[object].interface.to_owned(),
        }
    }

    /// The rank, from 1, of entry `entry` of `object` among that object's entries of its
    /// direction and name.
    fn entry_rank(&self, object: usize, entry: usize) -> usize {
        let entries = &self.client.objects[object].entries;
        let this = &entries[entry];
        1 + entries[..entry]
            .iter()
            .filter(|earlier| (earlier.direction, earlier.name) == (this.direction, this.name))
            .count()
    }

    /// An entry's line: its direction, name and arguments, and the event that followed it where
    /// the protocol requires one to.
    fn entry_text(&self, object: usize, entry: usize) -> String {
        let this = &self.client.objects[object].entries[entry];
        let arguments: Vec<String> = this
            .arguments
            .iter()
            .map(|argument| self.argument_text(argument, object, entry))
            .collect();
        let mut line = format!(
            "{} {}{}",
            match this.direction {
                Direction::Request => "->",
                Direction::Event => "<-",
            },
            if this.discarded { "(discarded) " } else { "" },
            this.name,
        );
        if this.name != "delete_id" {
            line.push_str(&format!("({})", arguments.join(", ")));
        }
        if let Some((applier, event, rank)) = this.before {
            line.push_str(&format!(", before {}.{event}[{rank}]", self.label(applier)));
        }
        line
    }

    fn argument_text(&self, argument: &Held, object: usize, entry: usize) -> String {
        let timed = |time: String| match self.clock {
            Clock::Real => "<time>".to_owned(),
            Clock::Manual if self.names.is_none() => "<time>".to_owned(),
            Clock::Manual => time,
        };
        match argument {
            Held::Text(text) => text.clone(),
            Held::Object(index) => self.label(*index),
            Held::NewObject(index) => format!("new {}", self.label(*index)),
            Held::Serial(serial) => match self.names.map(|names| names.serials.get(serial)) {
                None => "serial".to_owned(),
                Some(Some(&first)) if first == (object, entry) => "serial".to_owned(),
                Some(Some(&(carrier, carried))) => {
                    let event = self.client.objects[carrier].entries[carried].name;
                    format!(
                        "serial of {}.{event}[{}]",
                        self.label(carrier),
                        self.entry_rank(carrier, carried)
                    )
                }
                Some(None) => "a serial this client was never sent".to_owned(),
            },
            Held::GlobalName { name, registry } => {
                match self.client.binds.get(&(*registry, *name)) {
                    Some(&bound) => format!("name of {}", self.label(bound)),
                    None => format!(
                        "name of {}",
                        self.globals
                            .get(name)
                            .map_or("an unknown global", String::as_str)
                    ),
                }
            }
            Held::RefreshCount(count) => match self.clock {
                Clock::Real => "<count>".to_owned(),
                Clock::Manual => count.to_string(),
            },
            // 32 bits that wrap: the difference is read as signed.
            Held::Millis(millis) => timed(format!(
                "t{:+}ms",
                millis.wrapping_sub(self.first_frame) as i32
            )),
            Held::Timestamp(nanos) => {
                let from = i128::try_from(*nanos).unwrap_or(i128::MAX)
                    - i128::from(self.first_frame) * 1_000_000;
                let sign = if from < 0 { '-' } else { '+' };
                let from = from.unsigned_abs();
                timed(format!(
                    "t{sign}{}.{:06}ms",
                    from / 1_000_000,
                    from % 1_000_000
                ))
            }
        }
    }
}

/// Marks the entries waiting for the event `event` of `object`, which has come: each was followed
/// by it, as the protocol requires.
fn apply_waiting(client: &mut ClientTrace, object: usize, event: &'static str) {
    let Some(waiting) = client.waiting.remove(&object) else {
        return;
    };
    let rank = client.objects[object]
        .entries
        .iter()
        .filter(|entry| entry.direction == Direction::Event && entry.name == event)
        .count();
    let mut still: Waiting = Vec::new();
    for (waiter, entry, events) in waiting {
        if events.contains(&event) {
            client.objects[waiter].entries[entry].before = Some((object, event, rank));
        } else {
            still.push((waiter, entry, events));
        }
    }
    if !still.is_empty() {
        client.waiting.insert(object, still);
    }
}

/// The object whose event applies `event` of `object`, and the events of it that do, where the
/// protocol requires that event to follow: the orders between objects a trace holds.
///
/// - A toplevel's `configure`, `configure_bounds` and `wm_capabilities`, a popup's `configure`
///   and `repositioned`, and a toplevel decoration's `configure` are applied by their
///   `xdg_surface`'s next `configure` (xdg-shell.xml:617, xdg-decoration-unstable-v1.xml:163).
/// - An `xdg-output`'s events are applied by its `wl_output`'s next `done` from version 3
///   (xdg-output-unstable-v1.xml:87); before, by its own `done`.
/// - An offer's mime types come before the device's `selection` or `enter` that names it
///   (wayland.xml:986, primary-selection-unstable-v1.xml:112, ext-data-control-v1.xml:99).
fn applied_by(
    client: &ClientTrace,
    object: usize,
    event: &str,
) -> Option<(usize, &'static [&'static str])> {
    let this = &client.objects[object];
    let maker = this.made_by.map(|(maker, _)| maker);
    let named = |interface: &str| {
        this.named_with
            .iter()
            .copied()
            .find(|&named| client.objects[named].interface == interface)
    };
    match (this.interface, event) {
        ("xdg_toplevel", "configure" | "configure_bounds" | "wm_capabilities")
        | ("xdg_popup", "configure" | "repositioned") => Some((maker?, &["configure"])),
        ("zxdg_toplevel_decoration_v1", "configure") => {
            let toplevel = named("xdg_toplevel")?;
            let (xdg_surface, _) = client.objects[toplevel].made_by?;
            Some((xdg_surface, &["configure"]))
        }
        ("zxdg_output_v1", _) if this.version >= 3 => Some((named("wl_output")?, &["done"])),
        ("wl_data_offer", "offer") => Some((maker?, &["selection", "enter"])),
        ("zwp_primary_selection_offer_v1", "offer") => Some((maker?, &["selection"])),
        ("ext_data_control_offer_v1", "offer") => {
            Some((maker?, &["selection", "primary_selection"]))
        }
        _ => None,
    }
}

/// An array as the trace holds it: its 32-bit values in the machine's order when it holds whole
/// ones, as Wayland's arrays of enums and keys do, else its bytes.
fn array_text(bytes: &[u8]) -> String {
    let (values, rest) = bytes.as_chunks::<4>();
    if rest.is_empty() {
        let values: Vec<String> = values
            .iter()
            .map(|value| u32::from_ne_bytes(*value).to_string())
            .collect();
        format!("[{}]", values.join(", "))
    } else {
        format!("bytes{bytes:?}")
    }
}

/// `bytes`, an array of 32-bit values, its values sorted.
fn sorted_values(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let (values, rest) = bytes.as_chunks::<4>();
    if !rest.is_empty() {
        return Err(format!("not an array of 32-bit values: {bytes:?}"));
    }
    let mut values: Vec<u32> = values
        .iter()
        .map(|value| u32::from_ne_bytes(*value))
        .collect();
    values.sort_unstable();
    Ok(values
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect())
}

/// Sorts each run of lines that name members of one set, by their arguments: as numbers where
/// they are, as text elsewhere.
fn sort_runs_of_set_members(lines: &mut [(Option<(Direction, &str)>, String)]) {
    let mut from = 0;
    while from < lines.len() {
        let mut to = from + 1;
        if lines[from].0.is_some() {
            while to < lines.len() && lines[to].0 == lines[from].0 {
                to += 1;
            }
            lines[from..to].sort_by_cached_key(|(_, line)| arguments_order(line));
        }
        from = to;
    }
}

/// What a set's member is sorted by: its arguments, each a number or a text.
fn arguments_order(line: &str) -> Vec<(u64, String)> {
    let arguments = line
        .split_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap_or_default();
    arguments
        .split(", ")
        .map(|argument| match argument.parse() {
            Ok(number) => (number, String::new()),
            Err(_) => (u64::MAX, argument.to_owned()),
        })
        .collect()
}

#[cfg(test)]
mod tests;
