//! Yuna's registry answers each call toolkits make of it, at once, with the signature each
//! toolkit checks, through its messages' bytes as the bus carries them.

use zbus::wire::{OwnedObjectPath, OwnedVariant, Variant};

use super::*;
use crate::dbus::{Body, Kind, Message, NO_REPLY_EXPECTED};

const OWN: &str = ":1.0";

/// A call from `sender`, as it arrives: encoded, then decoded, with a serial.
fn call(sender: &str, path: &str, interface: &str, member: &str, body: Body) -> Message {
    let mut call = Message::method_call(REGISTRY_NAME, path, interface, member, body);
    call.sender = Some(sender.to_owned());
    Message::decode(&call.encode(3).expect("encoded"))
        .map(|mut message| {
            message.sender = Some(sender.to_owned());
            message
        })
        .expect("decoded")
}

/// The reply as the caller reads it.
fn read(reply: Option<Message>) -> Message {
    let reply = reply.expect("a reply");
    Message::decode(&reply.encode(11).expect("encoded")).expect("decoded")
}

fn reference(name: &str, path: &str) -> (String, OwnedObjectPath) {
    (
        name.to_owned(),
        OwnedObjectPath::try_from(path).expect("a path"),
    )
}

fn embed(registry: &mut Registry, name: &str) -> Answer {
    let body = Body::new("(so)", &reference(name, ROOT_PATH)).unwrap();
    registry.answer(
        &call(name, ROOT_PATH, "org.a11y.atspi.Socket", "Embed", body),
        OWN,
    )
}

/// GTK 4 checks `Embed`'s reply as `((so))`, the registry's root, and exports nothing without
/// it; the root then lists the application, and `Unembed` or the connection's end forgets it.
#[test]
fn an_application_embeds_and_is_the_roots_child_until_it_leaves() {
    let mut registry = Registry::default();
    let answer = embed(&mut registry, ":1.4");
    assert_eq!(answer.embedded.as_deref(), Some(":1.4"));
    // The bytes the toolkit reads: one structure, `(so)`, its signature's length 4.
    let bytes = answer
        .reply
        .as_ref()
        .expect("a reply")
        .encode(11)
        .expect("encoded");
    assert!(bytes.windows(6).any(|window| window == b"\x04(so)\0"));
    let reply = read(answer.reply);
    assert_eq!(reply.kind, Kind::MethodReturn);
    assert_eq!(
        reply.body::<(String, OwnedObjectPath)>("(so)").unwrap(),
        reference(OWN, ROOT_PATH)
    );
    assert_eq!(registry.generation(), 1);

    // An empty name is the sender's, as registryd takes it.
    let body = Body::new("(so)", &reference("", "/org/a11y/atspi/accessible/root")).unwrap();
    registry.answer(
        &call(":1.5", ROOT_PATH, "org.a11y.atspi.Socket", "Embed", body),
        OWN,
    );
    let children = read(
        registry
            .answer(
                &call(":1.9", ROOT_PATH, ACCESSIBLE, "GetChildren", Body::empty()),
                OWN,
            )
            .reply,
    );
    assert_eq!(
        children
            .body::<Vec<(String, OwnedObjectPath)>>("a(so)")
            .unwrap(),
        [reference(":1.4", ROOT_PATH), reference(":1.5", ROOT_PATH)]
    );
    let count = read(
        registry
            .answer(
                &call(
                    ":1.9",
                    ROOT_PATH,
                    PROPERTIES,
                    "Get",
                    Body::new("ss", &(ACCESSIBLE, "ChildCount")).unwrap(),
                ),
                OWN,
            )
            .reply,
    );
    let count: OwnedVariant = count.body("v").unwrap();
    assert_eq!(*count, Variant::I32(2));

    let body = Body::new("(so)", &reference(":1.4", ROOT_PATH)).unwrap();
    registry.answer(
        &call(":1.4", ROOT_PATH, "org.a11y.atspi.Socket", "Unembed", body),
        OWN,
    );
    registry.name_lost(":1.5");
    assert!(registry.apps().is_empty());

    registry.set_pid(":1.4", Some(7));
    embed(&mut registry, ":1.6");
    assert!(registry.apps()[0].pid_asked);
    let before = registry.generation();
    registry.set_pid(":1.6", Some(42));
    assert_eq!(registry.apps()[0].pid, Some(42));
    assert!(!registry.apps()[0].pid_asked);
    // A process known is a change a waiting call walks again for.
    assert!(registry.generation() > before);
}

/// No listener is registered: GTK 4, Qt and the ATK bridge then send no event, and Qt installs no
/// key filter; keys sent anyway are answered `false` at once, so that no key waits.
#[test]
fn no_listener_is_told_and_keys_are_answered_at_once() {
    let mut registry = Registry::default();
    let events = read(
        registry
            .answer(
                &call(
                    ":1.4",
                    "/org/a11y/atspi/registry",
                    "org.a11y.atspi.Registry",
                    "GetRegisteredEvents",
                    Body::empty(),
                ),
                OWN,
            )
            .reply,
    );
    assert_eq!(events.signature(), "a(ss)");
    assert!(
        events
            .body::<Vec<(String, String)>>("a(ss)")
            .unwrap()
            .is_empty()
    );

    let controller = "/org/a11y/atspi/registry/deviceeventcontroller";
    let dec = "org.a11y.atspi.DeviceEventController";
    let listeners = read(
        registry
            .answer(
                &call(
                    ":1.4",
                    controller,
                    dec,
                    "GetKeystrokeListeners",
                    Body::empty(),
                ),
                OWN,
            )
            .reply,
    );
    assert_eq!(listeners.signature(), "a(souua(iisi)u(bbb))");
    // The ATK bridge checks only that it starts with `a(s`.
    assert!(listeners.signature().starts_with("a(s"));
    let key = Body::new(
        "(uiuuisb)",
        &(0_u32, 65_i32, 38_u32, 0_u32, 0_i32, "a", true),
    )
    .unwrap();
    let consumed = read(
        registry
            .answer(
                &call(":1.4", controller, dec, "NotifyListenersSync", key),
                OWN,
            )
            .reply,
    );
    assert_eq!(consumed.body::<bool>("b"), Ok(false));
    let generate = read(
        registry
            .answer(
                &call(
                    ":1.4",
                    controller,
                    dec,
                    "GenerateKeyboardEvent",
                    Body::empty(),
                ),
                OWN,
            )
            .reply,
    );
    assert_eq!(generate.kind, Kind::Error);
    let items = read(
        registry
            .answer(
                &call(
                    ":1.4",
                    "/org/a11y/atspi/cache",
                    "org.a11y.atspi.Cache",
                    "GetItems",
                    Body::empty(),
                ),
                OWN,
            )
            .reply,
    );
    assert_eq!(items.signature(), "a((so)(so)(so)iiassusau)");
}

/// The root is a `desktop frame` named `main`, as registryd's; anything else asked is an error,
/// and a call that wants no reply gets none.
#[test]
fn the_root_answers_as_registryds_and_the_rest_is_an_error() {
    let mut registry = Registry::default();
    let role = read(
        registry
            .answer(
                &call(":1.4", ROOT_PATH, ACCESSIBLE, "GetRole", Body::empty()),
                OWN,
            )
            .reply,
    );
    assert_eq!(role.body::<u32>("u"), Ok(14));
    let name = read(
        registry
            .answer(
                &call(
                    ":1.4",
                    ROOT_PATH,
                    PROPERTIES,
                    "Get",
                    Body::new("ss", &(ACCESSIBLE, "Name")).unwrap(),
                ),
                OWN,
            )
            .reply,
    );
    let name: OwnedVariant = name.body("v").unwrap();
    assert_eq!(*name, Variant::from("main"));
    let all = read(
        registry
            .answer(
                &call(
                    ":1.4",
                    ROOT_PATH,
                    PROPERTIES,
                    "GetAll",
                    Body::new("s", APPLICATION).unwrap(),
                ),
                OWN,
            )
            .reply,
    );
    assert_eq!(all.signature(), "a{sv}");
    let unknown = read(
        registry
            .answer(
                &call(":1.4", ROOT_PATH, ACCESSIBLE, "Frobnicate", Body::empty()),
                OWN,
            )
            .reply,
    );
    assert_eq!(unknown.kind, Kind::Error);
    assert_eq!(unknown.error_name.as_deref(), Some(UNKNOWN_METHOD));
    let elsewhere = read(
        registry
            .answer(
                &call(":1.4", "/elsewhere", ACCESSIBLE, "GetRole", Body::empty()),
                OWN,
            )
            .reply,
    );
    assert_eq!(elsewhere.kind, Kind::Error);
    let ping = read(
        registry
            .answer(
                &call(
                    ":1.4",
                    "/",
                    "org.freedesktop.DBus.Peer",
                    "Ping",
                    Body::empty(),
                ),
                OWN,
            )
            .reply,
    );
    assert_eq!(ping.kind, Kind::MethodReturn);

    // The ATK bridge leaves without waiting for a reply.
    embed(&mut registry, ":1.4");
    let mut leaving = call(
        ":1.4",
        "/org/a11y/atspi/registry",
        "org.a11y.atspi.Registry",
        "DeregisterApplication",
        Body::new("s", ":1.4").unwrap(),
    );
    leaving.flags |= NO_REPLY_EXPECTED;
    assert!(registry.answer(&leaving, OWN).reply.is_none());
    assert!(registry.apps().is_empty());
    // A reply or a signal is no call.
    let reply = Message::method_return(&leaving, Body::empty());
    assert!(registry.answer(&reply, OWN).reply.is_none());
}

/// Past 128 applications, `Embed` fails rather than grow without end; an application embedding
/// twice is one; a malformed `Embed` is refused.
#[test]
fn embedding_is_bounded_and_checked() {
    let mut registry = Registry::default();
    for index in 0..MAX_APPS {
        embed(&mut registry, &format!(":1.{index}"));
    }
    // An application embedding again is answered, and neither counted again nor asked its
    // process again: a program looping on `Embed` grows nothing.
    let before = registry.generation();
    let again = embed(&mut registry, ":1.0");
    assert_eq!(read(again.reply).kind, Kind::MethodReturn);
    assert!(again.embedded.is_none());
    assert_eq!(registry.generation(), before);
    assert_eq!(registry.apps().len(), MAX_APPS);
    let refused = read(embed(&mut registry, ":2.0").reply);
    assert_eq!(refused.kind, Kind::Error);
    assert_eq!(registry.apps().len(), MAX_APPS);
    let malformed = call(
        ":3.0",
        ROOT_PATH,
        "org.a11y.atspi.Socket",
        "Embed",
        Body::new("s", "x").unwrap(),
    );
    let answer = registry.answer(&malformed, OWN);
    assert!(answer.embedded.is_none());
    assert_eq!(read(answer.reply).kind, Kind::Error);
}

/// `Socket.<member>((so))` from `sender`, naming `named`'s application at `root`.
fn socket_call(sender: &str, member: &str, named: &str, root: &str) -> Message {
    let body = Body::new("(so)", &reference(named, root)).unwrap();
    call(sender, ROOT_PATH, "org.a11y.atspi.Socket", member, body)
}

/// The bus names of the applications embedded.
fn embedded_names(registry: &Registry) -> Vec<&str> {
    registry
        .apps()
        .iter()
        .map(|app| app.bus_name.as_str())
        .collect()
}

/// An application is the connection that embeds it: `Embed` takes its sender's name, which the
/// bus sets, whether the call names it or leaves the name empty. A name of another connection,
/// or one that is no bus name, is refused with `InvalidArgs`: Yuna's walks call the name
/// recorded, and a bus disconnects whoever sends to a name that is no bus name, which would end
/// the seat's bus. Another connection neither unembeds nor deregisters it.
#[test]
fn an_application_is_the_connection_that_embeds_it() {
    let mut registry = Registry::default();
    let answer = registry.answer(&socket_call(":1.4", "Embed", ":1.4", ROOT_PATH), OWN);
    assert_eq!(answer.embedded.as_deref(), Some(":1.4"));
    assert_eq!(read(answer.reply).kind, Kind::MethodReturn);

    for named in [
        ":1.5",
        "x",
        "not a bus name",
        "org.a11y.atspi.Registry",
        ":1..2",
    ] {
        let answer = registry.answer(&socket_call(":1.6", "Embed", named, "/r"), OWN);
        assert!(answer.embedded.is_none(), "{named:?}");
        let reply = read(answer.reply);
        assert_eq!(reply.kind, Kind::Error, "{named:?}");
        assert_eq!(
            reply.error_name.as_deref(),
            Some(crate::dbus::INVALID_ARGS),
            "{named:?}"
        );
    }
    assert_eq!(embedded_names(&registry), [":1.4"]);

    // A call without a sender, which no bus delivers, embeds nothing.
    let mut anonymous = socket_call(":1.6", "Embed", "", "/r");
    anonymous.sender = None;
    let answer = registry.answer(&anonymous, OWN);
    assert!(answer.embedded.is_none());
    assert_eq!(read(answer.reply).kind, Kind::Error);
    assert_eq!(embedded_names(&registry), [":1.4"]);

    // Another connection's `Unembed` and `DeregisterApplication` forget nothing.
    let unembed = registry.answer(&socket_call(":1.6", "Unembed", ":1.4", ROOT_PATH), OWN);
    assert_eq!(
        read(unembed.reply).error_name.as_deref(),
        Some(crate::dbus::INVALID_ARGS)
    );
    let mut deregister = call(
        ":1.6",
        "/org/a11y/atspi/registry",
        "org.a11y.atspi.Registry",
        "DeregisterApplication",
        Body::new("s", ":1.4").unwrap(),
    );
    deregister.flags |= NO_REPLY_EXPECTED;
    assert!(registry.answer(&deregister, OWN).reply.is_none());
    assert_eq!(embedded_names(&registry), [":1.4"]);

    // Its own connection's `Unembed`, its name left empty, forgets it.
    registry.answer(&socket_call(":1.4", "Unembed", "", ROOT_PATH), OWN);
    assert!(registry.apps().is_empty());
}
