//! The D-Bus code tested alone: its messages' round trips, its refusals of malformed input,
//! random bytes that never make it panic, and its connection against a real `dbus-daemon`, the
//! development shell's.

#![expect(
    clippy::disallowed_methods,
    reason = "a test's threads are not Yuna's tasks (ADR-0152)"
)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use proptest::prelude::*;
use rustix::event::{PollFd, PollFlags, poll};
use zbus::wire::serialized::Context;
use zbus::wire::{Endian, OwnedObjectPath, OwnedVariant, Variant};

use super::connection::READ_BUDGET;
use super::message::{FRAME_PREFIX, MessageError, NO_REPLY_EXPECTED};
use super::*;

fn call_with_children() -> Message {
    let children = vec![
        (
            ":1.4".to_owned(),
            OwnedObjectPath::try_from("/org/a11y/atspi/accessible/1").unwrap(),
        ),
        (
            ":1.4".to_owned(),
            OwnedObjectPath::try_from("/org/a11y/atspi/accessible/2").unwrap(),
        ),
    ];
    let body = Body::new("a(so)", &children).expect("a body");
    let mut call = Message::method_call(
        ":1.4",
        "/org/a11y/atspi/accessible/root",
        "org.a11y.atspi.Accessible",
        "GetChildren",
        body,
    );
    call.sender = Some(":1.9".to_owned());
    call
}

#[test]
fn a_message_comes_back_whole_from_its_bytes() {
    let call = call_with_children();
    let bytes = call.encode(7).expect("encoded");
    assert_eq!(Message::frame_length(&bytes), Ok(Some(bytes.len())));
    let decoded = Message::decode(&bytes).expect("decoded");
    assert_eq!(decoded.kind, Kind::MethodCall);
    assert_eq!(decoded.serial, 7);
    assert_eq!(
        decoded.path.as_deref(),
        Some("/org/a11y/atspi/accessible/root")
    );
    assert_eq!(
        decoded.interface.as_deref(),
        Some("org.a11y.atspi.Accessible")
    );
    assert_eq!(decoded.member.as_deref(), Some("GetChildren"));
    assert_eq!(decoded.destination.as_deref(), Some(":1.4"));
    assert_eq!(decoded.sender.as_deref(), Some(":1.9"));
    let children: Vec<(String, OwnedObjectPath)> = decoded.body("a(so)").expect("its body");
    assert_eq!(children.len(), 2);
    assert_eq!(children[1].1.as_str(), "/org/a11y/atspi/accessible/2");

    // A reply, an error and a signal, each with what it must have.
    let mut received = decoded.clone();
    received.serial = 7;
    let reply = Message::method_return(&received, Body::new("(iiii)", &(1, -2, 30, 40)).unwrap());
    let reply = Message::decode(&reply.encode(8).unwrap()).expect("a reply");
    assert_eq!(reply.kind, Kind::MethodReturn);
    assert_eq!(reply.reply_serial, Some(7));
    assert_eq!(reply.destination.as_deref(), Some(":1.9"));
    assert_eq!(
        reply.body::<(i32, i32, i32, i32)>("(iiii)"),
        Ok((1, -2, 30, 40))
    );
    let error = Message::error(&received, UNKNOWN_METHOD, "no such method");
    let error = Message::decode(&error.encode(9).unwrap()).expect("an error");
    assert_eq!(error.kind, Kind::Error);
    assert_eq!(
        error.error_text(),
        format!("{UNKNOWN_METHOD}: no such method")
    );
    let signal = Message::signal(
        "/org/freedesktop/DBus",
        BUS_INTERFACE,
        "NameOwnerChanged",
        Body::new("sss", &(":1.4", ":1.4", "")).unwrap(),
    );
    let signal = Message::decode(&signal.encode(10).unwrap()).expect("a signal");
    assert_eq!(signal.kind, Kind::Signal);
    assert_eq!(
        signal.body::<(String, String, String)>("sss").unwrap().2,
        ""
    );
    // No argument at all.
    let empty = Message::method_call(BUS_NAME, BUS_PATH, BUS_INTERFACE, "Hello", Body::empty());
    let empty = Message::decode(&empty.encode(1).unwrap()).expect("an empty body");
    assert_eq!(empty.signature(), "");
    assert_eq!(empty.body::<()>(""), Ok(()));
}

#[test]
fn a_bodys_signature_is_sent_exactly_as_written() {
    // One structure and two arguments are the same bytes; a toolkit tells them apart by the
    // header's signature, which GTK 4 checks for `Embed`'s reply.
    let one = Message::method_call(
        ":1.1",
        "/",
        "a.b",
        "c",
        Body::new("(so)", &(":1.2", OwnedObjectPath::try_from("/x").unwrap())).unwrap(),
    );
    let two = Message::method_call(
        ":1.1",
        "/",
        "a.b",
        "c",
        Body::new("so", &(":1.2", OwnedObjectPath::try_from("/x").unwrap())).unwrap(),
    );
    let one = one.encode(1).unwrap();
    let two = two.encode(1).unwrap();
    let holds = |bytes: &[u8], text: &[u8]| bytes.windows(text.len()).any(|window| window == text);
    // A signature in the header: its length, its characters, a NUL.
    assert!(holds(&one, b"\x04(so)\0"));
    assert!(holds(&two, b"\x02so\0"));
    // Read back, either is taken for either: their bytes do not differ.
    let one = Message::decode(&one).unwrap();
    assert!(one.body::<(String, OwnedObjectPath)>("(so)").is_ok());
    assert!(one.body::<(String, OwnedObjectPath)>("so").is_ok());
    assert!(one.body::<String>("s").is_err());
}

#[test]
fn a_big_endian_message_is_read() {
    let header = (
        b'B',
        2_u8,
        0_u8,
        1_u8,
        4_u32,
        3_u32,
        vec![
            (5_u8, Variant::U32(9)),
            (
                8_u8,
                Variant::Signature(zbus::wire::Signature::try_from("u").unwrap()),
            ),
        ],
    );
    let encoded =
        zbus::wire::to_bytes_for_signature(Context::new(Endian::Big, 0), "(yyyyuua(yv))", &header)
            .unwrap();
    let mut bytes = encoded.bytes().to_vec();
    bytes.resize(bytes.len().next_multiple_of(8), 0);
    bytes.extend_from_slice(&42_u32.to_be_bytes());
    let decoded = Message::decode(&bytes).expect("a big-endian reply");
    assert_eq!(decoded.reply_serial, Some(9));
    assert_eq!(decoded.body::<u32>("u"), Ok(42));
}

/// A valid call's bytes, to break.
fn valid() -> Vec<u8> {
    call_with_children().encode(5).unwrap()
}

#[test]
fn malformed_messages_are_refused_not_followed() {
    let bytes = valid();
    // Shorter than its fixed part: not yet a frame.
    assert_eq!(Message::frame_length(&bytes[..FRAME_PREFIX - 1]), Ok(None));
    // Truncated: refused whole, its frame longer than what is given.
    assert!(matches!(
        Message::decode(&bytes[..bytes.len() - 1]),
        Err(MessageError::Malformed(_))
    ));
    // A byte order that is neither `l` nor `B`.
    let mut broken = bytes.clone();
    broken[0] = b'x';
    assert!(matches!(
        Message::frame_length(&broken),
        Err(MessageError::Malformed(_))
    ));
    // A body or fields' length past 4 MiB, or past what a length holds.
    for at in [4, 12] {
        let mut huge = bytes.clone();
        huge[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            matches!(Message::frame_length(&huge), Err(MessageError::TooLong(_))),
            "{at}"
        );
        assert!(Message::decode(&huge).is_err());
    }
    // Another protocol version, a serial of 0, a kind not yet defined.
    let mut version = bytes.clone();
    version[3] = 2;
    assert!(matches!(
        Message::decode(&version),
        Err(MessageError::Malformed(_))
    ));
    let mut serial = bytes.clone();
    serial[8..12].copy_from_slice(&0_u32.to_le_bytes());
    assert!(matches!(
        Message::decode(&serial),
        Err(MessageError::Malformed(_))
    ));
    let mut kind = bytes.clone();
    kind[1] = 9;
    assert_eq!(Message::decode(&kind), Err(MessageError::UnknownKind(9)));
    // The padding before the body, which must be zero.
    let fields = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let header_end = FRAME_PREFIX + fields;
    if !header_end.is_multiple_of(8) {
        let mut padding = bytes.clone();
        padding[header_end] = 1;
        assert!(Message::decode(&padding).is_err());
    }
    // A signature that does not parse: `(so)`'s `)` made a `]`.
    let mut signature = bytes.clone();
    let at = signature
        .windows(6)
        .position(|w| w == b"\x05a(so)")
        .expect("the signature");
    signature[at + 5] = b']';
    assert!(Message::decode(&signature).is_err());
    // A body read for another signature, or with bytes left.
    let decoded = Message::decode(&bytes).unwrap();
    assert!(decoded.body::<Vec<String>>("as").is_err());
}

#[test]
fn a_call_without_its_fields_or_with_file_descriptors_is_refused() {
    // A method call without a member, built as a header by hand.
    let build = |fields: Vec<(u8, Variant<'_>)>| {
        let header = (b'l', 1_u8, 0_u8, 1_u8, 0_u32, 1_u32, fields);
        let encoded = zbus::wire::to_bytes_for_signature(
            Context::new(Endian::Little, 0),
            "(yyyyuua(yv))",
            &header,
        )
        .unwrap();
        let mut bytes = encoded.bytes().to_vec();
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        bytes
    };
    let path = || {
        (
            1_u8,
            Variant::ObjectPath(zbus::wire::ObjectPath::try_from("/a").unwrap()),
        )
    };
    let member = || (3_u8, Variant::from("m"));
    assert!(Message::decode(&build(vec![path(), member()])).is_ok());
    assert!(Message::decode(&build(vec![path()])).is_err());
    // A path that is a string, not an object path.
    assert!(Message::decode(&build(vec![(1, Variant::from("/a")), member()])).is_err());
    // File descriptors, which Yuna never negotiates.
    assert!(Message::decode(&build(vec![path(), member(), (9, Variant::U32(1))])).is_err());
    // A field the specification may add later is ignored.
    assert!(Message::decode(&build(vec![path(), member(), (200, Variant::U32(1))])).is_ok());
}

/// `prefix` filled with `filler` to `length` bytes.
fn name_of_length(prefix: &str, filler: char, length: usize) -> String {
    let mut name = prefix.to_owned();
    while name.len() < length {
        name.push(filler);
    }
    name
}

/// Bus names at the edges of the specification's rules, each valid: a unique name's elements
/// may begin with a digit, a well-known name's may not; `-` and `_` anywhere; 255 bytes.
fn valid_bus_names() -> Vec<String> {
    let mut names: Vec<String> = [
        ":1.4",
        ":1.999",
        ":a-b.c_d",
        ":1.2.3",
        "a.b",
        "_a.-b",
        "A-1.B_2",
        "org.freedesktop.DBus",
    ]
    .map(str::to_owned)
    .to_vec();
    names.push(name_of_length(":1.", '1', 255));
    names.push(name_of_length("a.", 'b', 255));
    names
}

/// Interface names at the edges of the rules, each valid; error names follow the same.
fn valid_interfaces() -> Vec<String> {
    let mut names: Vec<String> = ["a.b", "_a._1", "A9.Z", "org.a11y.atspi.Accessible"]
        .map(str::to_owned)
        .to_vec();
    names.push(name_of_length("a.", 'b', 255));
    names
}

/// Member names at the edges of the rules, each valid.
fn valid_members() -> Vec<String> {
    let mut names: Vec<String> = ["c", "_", "A9_z"].map(str::to_owned).to_vec();
    names.push(name_of_length("m", 'm', 255));
    names
}

/// Asserts that `message` is refused, its error naming `field`.
fn assert_refused(message: Message, field: &str) {
    match message.encode(1) {
        Err(MessageError::Malformed(why)) => assert!(why.contains(field), "{field}: {why}"),
        other => panic!("{field}: {other:?}"),
    }
}

/// What a bus disconnects its sender for is never encoded: it is refused, with an error and never
/// a panic, so that Yuna never sends it. A destination or a sender that is no bus name; an
/// interface, a member or an error name that breaks its rules; a path that is no object path; the
/// path and the interface the bus reserves; a field a kind must have, missing; a reply serial of 0
/// (the specification's *Valid Names*, *Valid Object Paths* and *Header Fields*). Each name at
/// the edge of its rules is encoded.
#[test]
fn a_header_a_bus_would_disconnect_yuna_for_is_never_encoded() {
    let call = |destination: &str, path: &str, interface: &str, member: &str| {
        Message::method_call(destination, path, interface, member, Body::empty())
    };
    let with_sender = |sender: &str| {
        let mut message = call(BUS_NAME, "/a", "a.b", "c");
        message.sender = Some(sender.to_owned());
        message
    };

    for name in valid_bus_names() {
        assert!(call(&name, "/a", "a.b", "c").encode(1).is_ok(), "{name}");
        assert!(with_sender(&name).encode(1).is_ok(), "{name}");
    }
    let mut invalid_bus_names: Vec<String> = [
        "",
        "x",
        "not a bus name",
        ":",
        ":1",
        ":.1",
        ":1.",
        ":1..2",
        ".a.b",
        "a.b.",
        "a..b",
        "1a.b",
        "a.1b",
        "a.b c",
        "a.é",
        "a/b.c",
        "a.b\0",
    ]
    .map(str::to_owned)
    .to_vec();
    invalid_bus_names.push(name_of_length(":1.", '1', 256));
    invalid_bus_names.push(name_of_length("a.", 'b', 256));
    for name in &invalid_bus_names {
        assert_refused(call(name, "/a", "a.b", "c"), "destination");
        assert_refused(with_sender(name), "sender");
    }

    let mut invalid_interfaces: Vec<String> = [
        "", "a", "a.", ".a", "a..b", "1a.b", "a.1b", "a-b.c", "a b.c", "a.é", ":1.4",
    ]
    .map(str::to_owned)
    .to_vec();
    invalid_interfaces.push(name_of_length("a.", 'b', 256));
    let mut received = call(BUS_NAME, "/a", "a.b", "c");
    received.serial = 7;
    received.sender = Some(":1.9".to_owned());
    for name in valid_interfaces() {
        assert!(call(BUS_NAME, "/a", &name, "c").encode(1).is_ok(), "{name}");
        let error = Message::error(&received, &name, "text");
        assert!(error.encode(1).is_ok(), "{name}");
    }
    for name in &invalid_interfaces {
        assert_refused(call(BUS_NAME, "/a", name, "c"), "interface");
        assert_refused(Message::error(&received, name, "text"), "error name");
    }
    // The bus disconnects whoever sends a message on the interface it reserves.
    assert_refused(
        call(BUS_NAME, "/a", "org.freedesktop.DBus.Local", "c"),
        "interface",
    );

    for name in valid_members() {
        assert!(
            call(BUS_NAME, "/a", "a.b", &name).encode(1).is_ok(),
            "{name}"
        );
    }
    let mut invalid_members: Vec<String> = ["", "1a", "a.b", "a-b", "a b", "é"]
        .map(str::to_owned)
        .to_vec();
    invalid_members.push(name_of_length("m", 'm', 256));
    for name in &invalid_members {
        assert_refused(call(BUS_NAME, "/a", "a.b", name), "member");
    }

    for path in ["/", "/a", "/a_9/B", &"/ab".repeat(400)] {
        assert!(call(BUS_NAME, path, "a.b", "c").encode(1).is_ok(), "{path}");
    }
    // The path the bus reserves, with the rest: a toolkit gives the paths Yuna's walks call.
    for path in [
        "",
        "a",
        "/a/",
        "//",
        "/a//b",
        "/a-b",
        "/a.b",
        "/é",
        "/a b",
        "/org/freedesktop/DBus/Local",
    ] {
        assert_refused(call(BUS_NAME, path, "a.b", "c"), "path");
    }

    // A field each kind must have.
    let mut no_path = call(BUS_NAME, "/a", "a.b", "c");
    no_path.path = None;
    assert_refused(no_path, "fields");
    let mut no_member = call(BUS_NAME, "/a", "a.b", "c");
    no_member.member = None;
    assert_refused(no_member, "fields");
    let mut no_interface = Message::signal("/a", "a.b", "c", Body::empty());
    no_interface.interface = None;
    assert_refused(no_interface, "fields");
    let mut no_error_name = Message::error(&received, UNKNOWN_METHOD, "text");
    no_error_name.error_name = None;
    assert_refused(no_error_name, "fields");
    let mut no_reply_serial = Message::method_return(&received, Body::empty());
    no_reply_serial.reply_serial = None;
    assert_refused(no_reply_serial, "fields");
    // The reply to a message never sent, whose serial is still 0.
    let unsent = call(BUS_NAME, "/a", "a.b", "c");
    assert_refused(
        Message::method_return(&unsent, Body::empty()),
        "reply serial",
    );
}

#[test]
fn an_address_escapes_what_its_path_must() {
    assert_eq!(
        unix_address(std::path::Path::new(
            "/run/user/1000/yuna-mcp-7/seat-2.a11y/bus"
        )),
        "unix:path=/run/user/1000/yuna-mcp-7/seat-2.a11y/bus"
    );
    assert_eq!(
        unix_address(std::path::Path::new("/a b,c;d=é")),
        "unix:path=/a%20b%2cc%3bd%3d%c3%a9"
    );
}

proptest! {
    #[test]
    fn random_bytes_never_make_it_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Message::frame_length(&bytes);
        if let Ok(message) = Message::decode(&bytes) {
            let _ = message.body::<Vec<(String, OwnedObjectPath)>>("a(so)");
            let _ = message.body::<OwnedVariant>("v");
        }
    }

    #[test]
    fn a_valid_message_broken_anywhere_never_makes_it_panic(
        at in 0_usize..256,
        byte in any::<u8>(),
        cut in 0_usize..256,
    ) {
        let mut bytes = valid();
        let at = at % bytes.len();
        bytes[at] = byte;
        bytes.truncate(bytes.len() - cut % bytes.len());
        let _ = Message::frame_length(&bytes);
        if let Ok(message) = Message::decode(&bytes) {
            let _ = message.body::<Vec<(String, OwnedObjectPath)>>("a(so)");
        }
    }

    /// Broken anywhere but its two lengths, a message keeps its frame, so that each case reaches
    /// zbus's parser of its header and its body.
    #[test]
    fn a_valid_message_broken_inside_its_frame_reaches_the_parser_and_never_panics(
        changes in proptest::collection::vec((16_usize..4096, any::<u8>()), 1..12),
        first in any::<u8>(),
    ) {
        let mut bytes = valid();
        let length = bytes.len();
        for (at, byte) in changes {
            bytes[16 + at % (length - 16)] = byte;
        }
        // The kind, the flags, the version and the serial, the fixed part's other bytes.
        bytes[1 + usize::from(first % 3)] = first;
        prop_assert_eq!(Message::frame_length(&bytes), Ok(Some(length)));
        if let Ok(message) = Message::decode(&bytes) {
            let _ = message.body::<Vec<(String, OwnedObjectPath)>>("a(so)");
            let _ = message.body::<OwnedVariant>("v");
        }
    }

    /// Random headers and bodies whose lengths agree with their frame: zbus's parser reads
    /// random fields and bodies, and never panics.
    #[test]
    fn random_messages_whose_frame_holds_reach_the_parser_and_never_panic(
        fields in proptest::collection::vec(any::<u8>(), 0..256),
        body in proptest::collection::vec(any::<u8>(), 0..256),
        big_endian in any::<bool>(),
        kind in 0_u8..6,
    ) {
        let word = |value: usize| -> [u8; 4] {
            let value = value as u32;
            if big_endian { value.to_be_bytes() } else { value.to_le_bytes() }
        };
        let mut bytes = vec![if big_endian { b'B' } else { b'l' }, kind, 0, 1];
        bytes.extend_from_slice(&word(body.len()));
        bytes.extend_from_slice(&word(7));
        bytes.extend_from_slice(&word(fields.len()));
        bytes.extend_from_slice(&fields);
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        bytes.extend_from_slice(&body);
        prop_assert_eq!(Message::frame_length(&bytes), Ok(Some(bytes.len())));
        if let Ok(message) = Message::decode(&bytes) {
            let _ = message.body::<Vec<(String, OwnedObjectPath)>>("a(so)");
            let _ = message.body::<OwnedVariant>("v");
            let _ = message.body::<(String, String, String)>("sss");
        }
    }

    /// Any bus name the specification's rules make, as a destination, and any string in a body,
    /// come back whole.
    #[test]
    fn any_names_and_paths_come_back_whole(
        destination in "(:[A-Za-z0-9_-]{1,8}(\\.[A-Za-z0-9_-]{1,8}){1,4}|[A-Za-z_-][A-Za-z0-9_-]{0,7}(\\.[A-Za-z_-][A-Za-z0-9_-]{0,7}){1,4})",
        name in "[A-Za-z0-9_.:-]{1,64}",
        segments in proptest::collection::vec("[A-Za-z0-9_]{1,16}", 0..8),
        text in "\\PC{0,200}",
    ) {
        let path = format!("/{}", segments.join("/"));
        let body = Body::new("(so)s", &((name.as_str(), OwnedObjectPath::try_from(path.as_str()).unwrap()), text.as_str())).unwrap();
        let call = Message::method_call(&destination, &path, "a.b", "c", body);
        let decoded = Message::decode(&call.encode(1).unwrap()).unwrap();
        prop_assert_eq!(decoded.destination.as_deref(), Some(destination.as_str()));
        prop_assert_eq!(decoded.path.as_deref(), Some(path.as_str()));
        let ((got_name, got_path), got_text): ((String, OwnedObjectPath), String) =
            decoded.body("(so)s").unwrap();
        prop_assert_eq!(got_name, name);
        prop_assert_eq!(got_path.as_str(), path.as_str());
        prop_assert_eq!(got_text, text);
    }

    /// A valid name with a character no name may hold put anywhere in it is refused, as each
    /// header field that carries a name, and never makes the encoder panic.
    #[test]
    fn a_name_holding_a_character_no_name_may_hold_is_never_encoded(
        at in any::<usize>(),
        forbidden in "[^A-Za-z0-9_.:-]",
    ) {
        let spoiled = |name: &str| {
            let mut name = name.to_owned();
            name.insert_str(at % (name.len() + 1), &forbidden);
            name
        };
        let mut received = Message::method_call(BUS_NAME, "/a", "a.b", "c", Body::empty());
        received.serial = 7;
        received.sender = Some(":1.9".to_owned());
        let mut with_sender = received.clone();
        with_sender.sender = Some(spoiled(":1.42"));
        let messages = [
            ("destination", Message::method_call(&spoiled("org.a11y.atspi.Registry"), "/a", "a.b", "c", Body::empty())),
            ("sender", with_sender),
            ("interface", Message::method_call(BUS_NAME, "/a", &spoiled("org.a11y.atspi.Accessible"), "c", Body::empty())),
            ("member", Message::method_call(BUS_NAME, "/a", "a.b", &spoiled("GetChildren"), Body::empty())),
            ("error name", Message::error(&received, &spoiled("org.freedesktop.DBus.Error.Failed"), "text")),
        ];
        for (field, message) in messages {
            match message.encode(1) {
                Err(MessageError::Malformed(why)) => prop_assert!(why.contains(field), "{}: {}", field, why),
                other => prop_assert!(false, "{}: {:?}", field, other),
            }
        }
    }
}

/// A `dbus-daemon` of the test's own, on a socket in a folder of its own; killed when dropped.
struct Daemon {
    child: Child,
    folder: PathBuf,
    socket: PathBuf,
}

impl Daemon {
    fn start(name: &str) -> Daemon {
        let program = find_daemon().expect("the development shell gives dbus-daemon");
        let folder = std::env::temp_dir().join(format!("yuna-dbus-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&folder)
            .expect("a folder");
        let socket = folder.join("bus");
        let configuration = folder.join("bus.conf");
        std::fs::write(&configuration, format!(
            "<busconfig><type>session</type><listen>{}</listen><auth>EXTERNAL</auth>\
             <policy context=\"default\"><allow send_destination=\"*\"/><allow eavesdrop=\"false\"/>\
             <allow own=\"*\"/></policy></busconfig>",
            unix_address(&socket)
        )).expect("a configuration");
        let mut child = Command::new(program)
            .arg("--nofork")
            .arg(format!("--config-file={}", configuration.display()))
            .arg("--print-address")
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("dbus-daemon starts");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("its output"))
            .read_line(&mut line)
            .expect("its address");
        assert!(line.starts_with("unix:path="), "{line}");
        Daemon {
            child,
            folder,
            socket,
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// Reads and writes both connections, waiting on their sockets, until `done` gives a value from
/// the messages each received, or 10 s pass.
fn exchange<T>(
    connections: &mut [&mut Connection; 2],
    mut done: impl FnMut(usize, Message, &mut Connection) -> Option<T>,
) -> T {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        for connection in connections.iter_mut() {
            connection.flush().expect("a write");
        }
        let mut fds: Vec<PollFd<'_>> = connections
            .iter()
            .map(|connection| {
                let mut flags = PollFlags::IN;
                if connection.wants_write() {
                    flags |= PollFlags::OUT;
                }
                PollFd::new(connection, flags)
            })
            .collect();
        let timeout = rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 50_000_000,
        };
        poll(&mut fds, Some(&timeout)).expect("a poll");
        drop(fds);
        for (index, connection) in connections.iter_mut().enumerate() {
            for message in connection.read().expect("a read") {
                if let Some(value) = done(index, message, connection) {
                    return value;
                }
            }
        }
        assert!(Instant::now() < until, "the bus did not answer in 10 s");
    }
}

impl std::os::fd::AsFd for Connection {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.fd()
    }
}

/// Two connections to a real bus: each authenticates and is named; one owns a name and serves a
/// call of the other, which waits for no reply in one call and is answered an error for another;
/// a signal reaches the one that asked for it; the bus refuses nothing and drops nothing.
#[test]
fn two_connections_call_answer_and_signal_through_a_real_bus() {
    let daemon = Daemon::start("exchange");
    let mut server = Connection::connect(&daemon.socket).expect("a connection");
    let mut client = Connection::connect(&daemon.socket).expect("a connection");
    let request = Body::new("su", &("org.yuna.Test", 4_u32)).unwrap();
    let asked = server
        .send(Message::method_call(
            BUS_NAME,
            BUS_PATH,
            BUS_INTERFACE,
            "RequestName",
            request,
        ))
        .unwrap();
    let rule = Body::new("s", "type='signal',interface='org.yuna.Test'").unwrap();
    let matched = client
        .send(Message::method_call(
            BUS_NAME,
            BUS_PATH,
            BUS_INTERFACE,
            "AddMatch",
            rule,
        ))
        .unwrap();
    let mut connections = [&mut server, &mut client];
    let mut owned = false;
    let mut subscribed = false;
    exchange(&mut connections, |index, message, _| {
        match (index, message.reply_serial) {
            (0, Some(serial)) if serial == asked => owned = message.body::<u32>("u") == Ok(1),
            (1, Some(serial)) if serial == matched => {
                subscribed = message.kind == Kind::MethodReturn
            }
            _ => {}
        }
        (owned && subscribed).then_some(())
    });
    let [server, client] = connections;
    let server_name = server.unique_name().expect("a unique name").to_owned();
    assert!(server_name.starts_with(':'), "{server_name}");
    assert_ne!(client.unique_name(), Some(server_name.as_str()));

    // A call the client waits for, one it does not, and one the server refuses.
    let ask = |member: &str| {
        Message::method_call(
            "org.yuna.Test",
            "/org/yuna/Test",
            "org.yuna.Test",
            member,
            Body::new("s", "ping").unwrap(),
        )
    };
    let echoed = client.send(ask("Echo")).unwrap();
    let mut quiet = ask("Quiet");
    quiet.flags |= NO_REPLY_EXPECTED;
    client.send(quiet).unwrap();
    let refused = client.send(ask("Missing")).unwrap();
    let mut connections = [server, client];
    let mut seen_quiet = false;
    let mut replies: HashMap<u32, Message> = HashMap::new();
    exchange(&mut connections, |index, message, connection| {
        if index == 0 && message.kind == Kind::MethodCall {
            match message.member.as_deref() {
                Some("Echo") => {
                    let text: String = message.body("s").unwrap();
                    connection
                        .send(Message::method_return(
                            &message,
                            Body::new("s", &format!("{text}, pong")).unwrap(),
                        ))
                        .unwrap();
                }
                Some("Quiet") => {
                    assert!(!message.expects_reply());
                    seen_quiet = true;
                }
                _ => {
                    connection
                        .send(Message::error(&message, UNKNOWN_METHOD, "no such method"))
                        .unwrap();
                }
            }
        }
        if index == 1
            && let Some(serial) = message.reply_serial
        {
            replies.insert(serial, message);
        }
        (seen_quiet && replies.len() == 2).then_some(())
    });
    assert_eq!(replies[&echoed].body::<String>("s").unwrap(), "ping, pong");
    assert_eq!(replies[&refused].kind, Kind::Error);
    assert_eq!(
        replies[&refused].error_name.as_deref(),
        Some(UNKNOWN_METHOD)
    );

    // A signal from the server reaches the client, which asked for it.
    let [server, client] = connections;
    server
        .send(Message::signal(
            "/org/yuna/Test",
            "org.yuna.Test",
            "Changed",
            Body::new("u", &3_u32).unwrap(),
        ))
        .unwrap();
    let mut connections = [server, client];
    let value = exchange(&mut connections, |index, message, _| {
        (index == 1 && message.kind == Kind::Signal).then(|| message.body::<u32>("u").unwrap())
    });
    assert_eq!(value, 3);
    // The server's name, as the bus gives a connection's process, is this test's.
    let [server, client] = connections;
    let pid_call = client
        .send(Message::method_call(
            BUS_NAME,
            BUS_PATH,
            BUS_INTERFACE,
            "GetConnectionUnixProcessID",
            Body::new("s", &server_name).unwrap(),
        ))
        .unwrap();
    let mut connections = [server, client];
    let pid = exchange(&mut connections, |index, message, _| {
        (index == 1 && message.reply_serial == Some(pid_call))
            .then(|| message.body::<u32>("u").unwrap())
    });
    assert_eq!(pid, std::process::id());
}

/// A bus that goes away fails the connection plainly, and every later call says so.
#[test]
fn a_bus_that_ends_fails_the_connection_for_good() {
    let mut daemon = Daemon::start("ends");
    let mut connection = Connection::connect(&daemon.socket).expect("a connection");
    let until = Instant::now() + Duration::from_secs(10);
    while connection.unique_name().is_none() {
        connection.flush().unwrap();
        let _ = connection.read().unwrap();
        assert!(Instant::now() < until, "no Hello reply");
        std::thread::sleep(Duration::from_millis(10));
    }
    daemon.child.kill().unwrap();
    daemon.child.wait().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let failure = loop {
        if let Err(error) = connection.read() {
            break error;
        }
        assert!(Instant::now() < until, "the end was not seen");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(failure, connection::ConnectionError::Closed);
    assert!(connection.failure().is_some());
    assert!(
        connection
            .send(Message::method_call(
                BUS_NAME,
                BUS_PATH,
                BUS_INTERFACE,
                "Hello",
                Body::empty()
            ))
            .is_err()
    );
    // A socket where no bus listens is refused at once.
    assert!(Connection::connect(&daemon.folder.join("nowhere")).is_err());
}

/// Every name at the edge of the specification's rules, which Yuna encodes, a real bus takes:
/// sent in each header field that carries one, it leaves the sender connected. A bus disconnects
/// whoever sends a name it refuses, so that the calls sent after them, answered with each error
/// name at its edge, would never come back.
#[test]
fn a_real_bus_takes_every_name_at_the_edge_of_the_rules() {
    let daemon = Daemon::start("edges");
    let mut server = Connection::connect(&daemon.socket).expect("a connection");
    let mut client = Connection::connect(&daemon.socket).expect("a connection");
    let until = Instant::now() + Duration::from_secs(10);
    while server.unique_name().is_none() || client.unique_name().is_none() {
        for connection in [&mut server, &mut client] {
            connection.flush().unwrap();
            let _ = connection.read().unwrap();
        }
        assert!(Instant::now() < until, "no Hello reply");
        std::thread::sleep(Duration::from_millis(10));
    }
    let server_name = server.unique_name().expect("a name").to_owned();
    let quiet = |destination: &str, path: &str, interface: &str, member: &str| {
        let mut call = Message::method_call(destination, path, interface, member, Body::empty());
        call.flags |= NO_REPLY_EXPECTED;
        call
    };
    let mut sent = Vec::new();
    for name in valid_bus_names() {
        sent.push(quiet(&name, "/a", "a.b", "c"));
        let mut with_sender = quiet(&server_name, "/a", "a.b", "c");
        with_sender.sender = Some(name);
        sent.push(with_sender);
    }
    for name in valid_interfaces() {
        sent.push(quiet(&server_name, "/a", &name, "c"));
    }
    for name in valid_members() {
        sent.push(quiet(&server_name, "/a", "a.b", &name));
    }
    for path in ["/", "/a_9/B", &"/ab".repeat(400)] {
        sent.push(quiet(&server_name, path, "a.b", "c"));
    }
    for message in sent {
        client.send(message).expect("encoded");
    }
    let errors: Vec<u32> = valid_interfaces()
        .into_iter()
        .map(|name| {
            let ask = Message::method_call(
                &server_name,
                "/a",
                "a.b",
                "Fail",
                Body::new("s", &name).unwrap(),
            );
            client.send(ask).expect("encoded")
        })
        .collect();
    let mut connections = [&mut server, &mut client];
    let mut answered: HashMap<u32, Message> = HashMap::new();
    exchange(&mut connections, |index, message, connection| {
        if index == 0 && message.member.as_deref() == Some("Fail") {
            let name: String = message.body("s").unwrap();
            connection
                .send(Message::error(&message, &name, "as asked"))
                .expect("encoded");
        }
        if index == 1
            && let Some(serial) = message.reply_serial
        {
            answered.insert(serial, message);
        }
        (errors.iter().all(|serial| answered.contains_key(serial))).then_some(())
    });
    for (serial, name) in errors.iter().zip(valid_interfaces()) {
        assert_eq!(answered[serial].kind, Kind::Error, "{name}");
        assert_eq!(answered[serial].error_name.as_deref(), Some(name.as_str()));
    }
    assert!(server.failure().is_none() && client.failure().is_none());
}

/// The test's end of a socket pair, which plays the bus.
struct FakeBus {
    stream: std::os::unix::net::UnixStream,
}

impl FakeBus {
    /// A connection and the bus's end of its socket.
    fn pair() -> (Connection, FakeBus) {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().expect("a socket pair");
        theirs
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("a timeout");
        let connection = Connection::over(std::os::fd::OwnedFd::from(ours)).expect("a connection");
        (connection, FakeBus { stream: theirs })
    }

    /// Reads from the connection until `bytes` have come: what it wrote.
    fn read_bytes(&mut self, connection: &mut Connection, count: usize) -> Vec<u8> {
        use std::io::Read;
        let mut bytes = vec![0; count];
        let mut read = 0;
        while read < count {
            connection.flush().expect("a write");
            read += self
                .stream
                .read(&mut bytes[read..])
                .expect("the connection's bytes");
        }
        bytes
    }

    /// Reads the connection's AUTH line, which must name this test's user.
    fn take_auth_line(&mut self, connection: &mut Connection) -> String {
        let mut line = Vec::new();
        while !line.ends_with(b"\r\n") {
            line.extend(self.read_bytes(connection, 1));
        }
        String::from_utf8(line).expect("a text line")
    }

    fn write(&mut self, bytes: &[u8]) {
        use std::io::Write;
        self.stream
            .write_all(bytes)
            .expect("a write to the connection");
    }

    /// Accepts the authentication and answers `Hello`: the connection runs, named `:1.7`.
    fn accept(&mut self, connection: &mut Connection) {
        let line = self.take_auth_line(connection);
        assert!(line.starts_with("\0AUTH EXTERNAL "), "{line:?}");
        self.write(b"OK 0123456789abcdef0123456789abcdef\r\n");
        let _ = connection.read().expect("the authentication's answer");
        assert_eq!(self.read_bytes(connection, 7), b"BEGIN\r\n");
        let hello = self.take_message(connection);
        assert_eq!(hello.member.as_deref(), Some("Hello"));
        let reply = Message::method_return(&hello, Body::new("s", ":1.7").unwrap());
        self.write(&reply.encode(1).unwrap());
        let until = Instant::now() + Duration::from_secs(10);
        while connection.unique_name().is_none() {
            assert!(connection.read().expect("a read").is_empty());
            assert!(Instant::now() < until, "Hello's reply was not taken");
        }
        assert_eq!(connection.unique_name(), Some(":1.7"));
    }

    /// One whole message the connection sent.
    fn take_message(&mut self, connection: &mut Connection) -> Message {
        let mut bytes = self.read_bytes(connection, FRAME_PREFIX);
        let length = Message::frame_length(&bytes).unwrap().unwrap();
        bytes.extend(self.read_bytes(connection, length - FRAME_PREFIX));
        Message::decode(&bytes).expect("a message of the connection's")
    }
}

/// A signal from the fake bus, its serial `serial`, with a body of `size` bytes of text.
fn signal_bytes(serial: u32, size: usize) -> Vec<u8> {
    let text = "x".repeat(size);
    let signal = Message::signal(
        "/a",
        "org.yuna.Test",
        "Changed",
        Body::new("s", &text).unwrap(),
    );
    signal.encode(serial).unwrap()
}

/// Reads until `count` messages came or 10 s passed.
fn read_messages(connection: &mut Connection, count: usize) -> Vec<Message> {
    let mut messages = Vec::new();
    let until = Instant::now() + Duration::from_secs(10);
    while messages.len() < count {
        messages.extend(connection.read().expect("a read"));
        assert!(
            Instant::now() < until,
            "{} of {count} messages came",
            messages.len()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    messages
}

/// A message that comes a few bytes at a time is given once whole, and only then.
#[test]
fn a_message_split_across_reads_is_given_once_whole() {
    let (mut connection, mut bus) = FakeBus::pair();
    bus.accept(&mut connection);
    let bytes = signal_bytes(5, 100);
    for chunk in bytes.chunks(7) {
        assert!(connection.read().expect("a read").is_empty());
        bus.write(chunk);
    }
    let messages = read_messages(&mut connection, 1);
    assert_eq!(messages[0].serial, 5);
    assert_eq!(messages[0].body::<String>("s").unwrap().len(), 100);
}

/// A malformed message whose frame is whole is dropped, and the stream is followed: the next
/// message comes, and the connection still works.
#[test]
fn a_malformed_message_mid_stream_is_dropped_and_the_stream_followed() {
    let (mut connection, mut bus) = FakeBus::pair();
    bus.accept(&mut connection);
    let mut broken = signal_bytes(5, 10);
    // Protocol version 2.
    broken[3] = 2;
    let mut stream = broken;
    stream.extend(signal_bytes(6, 10));
    bus.write(&stream);
    let messages = read_messages(&mut connection, 1);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].serial, 6);
    assert!(connection.failure().is_none());
    // A frame impossible to follow, though, ends it for good.
    let mut lost = signal_bytes(7, 10);
    lost[0] = b'x';
    bus.write(&lost);
    let until = Instant::now() + Duration::from_secs(10);
    while connection.read().is_ok() {
        assert!(Instant::now() < until, "the lost stream was not refused");
    }
    assert!(connection.failure().is_some());
}

/// A message longer than what one read takes is carried over to the next reads and given
/// whole.
#[test]
fn a_message_longer_than_a_read_is_carried_over_whole() {
    let (mut connection, mut bus) = FakeBus::pair();
    bus.accept(&mut connection);
    let size = READ_BUDGET + READ_BUDGET / 2;
    let bytes = signal_bytes(8, size);
    let writer = std::thread::spawn(move || bus.write(&bytes));
    let messages = read_messages(&mut connection, 1);
    writer.join().expect("the writer");
    assert_eq!(messages[0].body::<String>("s").unwrap().len(), size);
}

/// What waits to be written is bounded: while the bus reads nothing, sends past 8 MiB are
/// refused, and the connection goes on.
#[test]
fn what_waits_to_be_written_is_bounded() {
    let (mut connection, bus) = FakeBus::pair();
    let three_mib = "x".repeat(3 << 20);
    let message = || {
        Message::method_call(
            BUS_NAME,
            BUS_PATH,
            BUS_INTERFACE,
            "Ping",
            Body::new("s", &three_mib).unwrap(),
        )
    };
    assert!(connection.send(message()).is_ok());
    assert!(connection.send(message()).is_ok());
    assert_eq!(
        connection.send(message()),
        Err(connection::ConnectionError::Full)
    );
    assert!(connection.failure().is_none());
    drop(bus);
}

/// An authentication the bus refuses, or a line longer than SASL's, fails the connection
/// plainly.
#[test]
fn a_refused_or_overlong_authentication_fails_the_connection() {
    let (mut connection, mut bus) = FakeBus::pair();
    bus.take_auth_line(&mut connection);
    bus.write(b"REJECTED EXTERNAL\r\n");
    let until = Instant::now() + Duration::from_secs(10);
    let failure = loop {
        if let Err(error) = connection.read() {
            break error;
        }
        assert!(Instant::now() < until, "the refusal was not seen");
    };
    assert!(
        matches!(failure, connection::ConnectionError::Authentication(ref line) if line.starts_with("REJECTED")),
        "{failure}"
    );

    let (mut connection, mut bus) = FakeBus::pair();
    bus.take_auth_line(&mut connection);
    bus.write(&[b'x'; connection::MAX_AUTH_LINE + 10]);
    let until = Instant::now() + Duration::from_secs(10);
    let failure = loop {
        if let Err(error) = connection.read() {
            break error;
        }
        assert!(Instant::now() < until, "the long line was not refused");
    };
    assert!(
        matches!(failure, connection::ConnectionError::Authentication(_)),
        "{failure}"
    );
}
