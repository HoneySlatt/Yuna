//! What a call is told: names clipped on a character's boundary, plain words when a program is
//! not on its seat's bus, an application's walk cut by the wait not called complete; and a late
//! reply of a walk never taken by the next.
//!
//! Then a seat's bus over a real `dbus-daemon`, the development shell's, served on a compositor's
//! loop as `yuna mcp` serves it: programs of the test's own on it, none of whose mistakes ends
//! the bus; and Yuna at rest once a bus ended whose connection could be watched only in part.

use std::fs;
use std::os::fd::OwnedFd;

use serde_json::json;
use smithay::reexports::rustix;
use zbus::wire::{OwnedObjectPath, Variant};

use super::*;
use crate::Config;
use crate::dbus::{Body, INVALID_ARGS, UNKNOWN_METHOD};
use crate::mcp::protocol::Revision;

/// A call with no walk yet, its key `key`, on seat 2, filtering nothing.
fn tree_call(key: u64) -> TreeCall {
    TreeCall {
        key,
        id: json!(1),
        revision: Revision::Modern,
        seat: SeatId(2),
        window: None,
        pid: None,
        filter: Filter::default(),
        max_nodes: MAX_NODES,
        wait_until: None,
        walk: None,
        last_end: None,
        seen: (0, 0),
        timer: None,
    }
}

/// An application embedded, its process known.
fn app() -> registry::App {
    registry::App {
        bus_name: ":1.4".to_owned(),
        root: "/root".to_owned(),
        pid: Some(42),
        pid_asked: false,
    }
}

/// The error the bus answers a call with once its reply timeout passes.
fn no_reply() -> Message {
    let mut call = Message::method_call(
        ":1.4",
        "/root",
        "org.a11y.atspi.Accessible",
        "GetChildren",
        Body::empty(),
    );
    call.serial = 9;
    Message::error(
        &call,
        "org.freedesktop.DBus.Error.NoReply",
        "no reply in 5 s",
    )
}

/// A walk cut at its deadline leaves calls in flight; the call walks again; the bus's late
/// error for the first walk's root, 5 s after it was sent, must not fail the second walk's
/// application, which answers.
#[test]
fn a_late_reply_of_a_cut_walk_never_reaches_the_next() {
    let mut calls = vec![tree_call(1)];
    let mut first = Walk::new(vec![app()], Filter::default(), 100);
    while first.next_call(0).is_some() {}
    first.cut_at_deadline();
    calls[0].walk = Some(UnderWay {
        serial: 1,
        walk: first,
        started: Instant::now(),
    });
    calls[0].walk = Some(UnderWay {
        serial: 2,
        walk: Walk::new(vec![app()], Filter::default(), 100),
        started: Instant::now(),
    });
    assert!(!deliver(
        &mut calls,
        (1, 1),
        0,
        0,
        Query::Children,
        &no_reply()
    ));
    let second = &calls[0].walk.as_ref().expect("a walk").walk;
    assert!(
        second.apps[0].failed.is_none(),
        "{:?}",
        second.apps[0].failed
    );
    // Its own replies, and none of another call's, are taken.
    assert!(!deliver(
        &mut calls,
        (2, 2),
        0,
        0,
        Query::Children,
        &no_reply()
    ));
    assert!(deliver(
        &mut calls,
        (1, 2),
        0,
        0,
        Query::Children,
        &no_reply()
    ));
    assert!(
        calls[0].walk.as_ref().expect("a walk").walk.apps[0]
            .failed
            .is_some()
    );
}

/// A walk cut by its call's wait's end gives half a tree: its applications are said to be cut,
/// never complete.
#[test]
fn a_walk_cut_by_the_waits_end_is_not_called_complete() {
    let call = tree_call(1);
    let unfinished = Walk::new(vec![app()], Filter::default(), 100);
    let found = call_result(&call, &unfinished, &[], &[]);
    assert_eq!(
        found.value["apps"][0]["status"],
        json!("cut at the wait's end")
    );
    let mut cut = Walk::new(vec![app()], Filter::default(), 100);
    cut.cut_at_wait();
    assert!(cut.is_done());
    let found = call_result(&call, &cut, &[], &[]);
    assert_eq!(
        found.value["apps"][0]["status"],
        json!("cut at the wait's end")
    );
}

#[test]
fn a_long_name_is_clipped_on_a_characters_boundary() {
    assert_eq!(clipped("Run"), "Run");
    let long = "é".repeat(MAX_TEXT + 5);
    let cut = clipped(&long);
    assert_eq!(cut.chars().count(), MAX_TEXT + 1);
    assert!(cut.ends_with('…'));
    assert_eq!(clipped(&"a".repeat(MAX_TEXT)), "a".repeat(MAX_TEXT));
}

#[test]
fn a_program_not_on_the_bus_is_told_plainly() {
    let seat = SeatId(2);
    let without = no_bus_message(seat, false);
    assert!(
        without.contains("seat 2 has no accessibility bus"),
        "{without}"
    );
    assert!(without.contains("accessibility: true"), "{without}");
    assert!(no_bus_message(seat, true).contains("starting"));
    let process = not_on_bus_message(seat, 4242, None);
    assert!(
        process.starts_with("process 4242 is not on seat 2's accessibility bus"),
        "{process}"
    );
    let window = not_on_bus_message(seat, 4242, Some(WindowId(7)));
    assert!(
        window.starts_with("window 7's process, 4242, is not on"),
        "{window}"
    );
}

/// Only the bus says a connection left: a `NameOwnerChanged` with no new owner from the bus names
/// it; the same signal from a program, or one naming a new owner, names none.
#[test]
fn only_the_bus_says_a_connection_left() {
    let signal = |sender: &str, new: &str| {
        let mut signal = Message::signal(
            BUS_PATH,
            BUS_INTERFACE,
            "NameOwnerChanged",
            Body::new("sss", &(":1.4", ":1.4", new)).expect("a body"),
        );
        signal.sender = Some(sender.to_owned());
        signal
    };
    assert_eq!(name_left(&signal(BUS_NAME, "")).as_deref(), Some(":1.4"));
    assert_eq!(name_left(&signal(":1.6", "")), None);
    assert_eq!(name_left(&signal(BUS_NAME, ":1.4")), None);
    let mut other = signal(BUS_NAME, "");
    other.member = Some("NameAcquired".to_owned());
    assert_eq!(name_left(&other), None);
}

/// The path the bus reserves: a message sent with it makes the bus disconnect its sender.
const LOCAL_PATH: &str = "/org/freedesktop/DBus/Local";

/// How long a step of a test over a real bus may take.
const PATIENCE: Duration = Duration::from_secs(10);

/// A seat's accessibility bus of the test's own, on a compositor of its own whose loop Yuna's
/// accessibility runs on, as `yuna mcp`'s does; its daemon is killed, and its folder removed,
/// when it is dropped.
struct Bus {
    compositor: Compositor,
    accessibility: Accessibility,
    seat: SeatId,
    folder: PathBuf,
    /// The seats whose bus ended, and why.
    ended: Vec<(SeatId, String)>,
    /// The calls answered, as `tools/call` gives them.
    answers: Vec<Value>,
}

/// A folder short enough for a bus's socket, which a Unix socket's 107 bytes bound.
fn short_folder(name: &str) -> PathBuf {
    let temp = std::env::temp_dir();
    let parent = if temp.as_os_str().len() <= 16 {
        temp
    } else {
        PathBuf::from("/tmp")
    };
    parent.join(format!("yuna-a11y-{name}-{}", std::process::id()))
}

impl Bus {
    /// A compositor and a seat, whose bus starts: not ready yet.
    fn starting(name: &str) -> Bus {
        let mut compositor = Compositor::new(Config {
            outputs: Vec::new(),
            listen: false,
            announce_owner_seat: false,
            ..Config::default()
        })
        .expect("the compositor starts");
        let seat = compositor.add_seat().expect("a seat");
        let mut accessibility = Accessibility::new(compositor.loop_handle());
        let folder = short_folder(name);
        let _ = fs::remove_dir_all(&folder);
        accessibility
            .start_bus(seat, folder.clone(), BTreeMap::new(), |log| {
                fs::File::create(log).map(OwnedFd::from)
            })
            .expect("the development shell gives dbus-daemon");
        Bus {
            compositor,
            accessibility,
            seat,
            folder,
            ended: Vec::new(),
            answers: Vec::new(),
        }
    }

    /// A seat whose bus is ready.
    fn ready(name: &str) -> Bus {
        let mut bus = Bus::starting(name);
        bus.run_until("the bus is ready", &mut [], |bus, _| {
            matches!(bus.accessibility.bus_state(bus.seat), BusState::Ready(_))
        });
        bus
    }

    /// The socket programs reach the bus through.
    fn socket(&self) -> PathBuf {
        self.folder.join("bus")
    }

    /// The applications embedded on the bus.
    fn apps(&self) -> Vec<registry::App> {
        self.accessibility
            .buses
            .get(&self.seat)
            .map(|bus| bus.registry.apps().to_vec())
            .unwrap_or_default()
    }

    /// One round: Yuna's loop waits up to 10 ms, its accessibility serves what came, then each
    /// of `programs` writes, reads and answers what it was sent.
    fn round(&mut self, programs: &mut [&mut Program]) {
        self.compositor
            .dispatch(Some(Duration::from_millis(10)))
            .expect("a round");
        if self.accessibility.has_work() {
            let outcome = self.accessibility.service(&self.compositor);
            self.ended.extend(outcome.ended);
            self.answers.extend(
                outcome
                    .answers
                    .into_iter()
                    .map(|(_, revision, result)| result.into_result(revision)),
            );
        }
        for program in programs.iter_mut() {
            program.pump();
        }
    }

    /// Rounds until `done`, or [`PATIENCE`].
    fn run_until(
        &mut self,
        what: &str,
        programs: &mut [&mut Program],
        done: impl Fn(&Bus, &[&mut Program]) -> bool,
    ) {
        let until = Instant::now() + PATIENCE;
        while !done(self, programs) {
            assert!(
                Instant::now() < until,
                "{what}: not in {PATIENCE:?}; the buses ended: {:?}",
                self.ended
            );
            self.round(programs);
        }
    }

    /// Calls `accessibility` for the nodes whose name holds `name`, on the seat, and gives its
    /// answer, as `tools/call` gives it.
    fn find(&mut self, name: &str, programs: &mut [&mut Program]) -> Value {
        let call = AccessibilityCall {
            seat: self.seat.0,
            window: None,
            pid: None,
            name: Some(name.to_owned()),
            role: None,
            states: Vec::new(),
            max_nodes: None,
            wait_ms: 0,
        };
        if let Some(failed) =
            self.accessibility
                .call(&json!(1), Revision::Modern, call, &self.compositor)
        {
            return failed.into_result(Revision::Modern);
        }
        self.run_until("the call is answered", programs, |bus, _| {
            !bus.answers.is_empty()
        });
        self.answers.remove(0)
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        // A daemon still there is the test's child: it is killed and reaped.
        if let Some(pid) = self.accessibility.daemon_pid(self.seat) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            let _ = rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::empty());
        }
        self.accessibility.stop();
        let _ = fs::remove_dir_all(&self.folder);
    }
}

/// A program of the test's own on a seat's bus. Sent a call, it answers as a toolkit whose
/// root, `/root`, a frame named "Fake", has the nodes `children`, each a button named "Press me"
/// with no child; it keeps every other message it reads.
struct Program {
    connection: Connection,
    children: Vec<String>,
    received: Vec<Message>,
    /// Why its connection ended, once it did.
    gone: Option<String>,
}

impl Program {
    fn connect(bus: &mut Bus, children: &[&str]) -> Program {
        let mut program = Program {
            connection: Connection::connect(&bus.socket()).expect("a connection"),
            children: children.iter().map(|path| (*path).to_owned()).collect(),
            received: Vec::new(),
            gone: None,
        };
        bus.run_until(
            "the program is named",
            &mut [&mut program],
            |_, programs| programs[0].connection.unique_name().is_some(),
        );
        program
    }

    /// Its unique name on the bus.
    fn name(&self) -> String {
        self.connection
            .unique_name()
            .expect("named by the bus")
            .to_owned()
    }

    /// Sends `message`, and gives its serial.
    fn send(&mut self, message: Message) -> u32 {
        self.connection.send(message).expect("the program sends")
    }

    /// The reply to its call `serial`, once it came.
    fn reply(&self, serial: u32) -> Option<&Message> {
        self.received
            .iter()
            .find(|message| message.reply_serial == Some(serial))
    }

    /// Writes what waits, reads what came, and answers the calls; its connection's end is kept.
    fn pump(&mut self) {
        if self.gone.is_some() {
            return;
        }
        let read = self
            .connection
            .flush()
            .and_then(|()| self.connection.read());
        match read {
            Ok(messages) => {
                for message in messages {
                    if message.kind == Kind::MethodCall {
                        let reply = self.answer(&message);
                        if let Err(error) = self.connection.send(reply) {
                            self.gone = Some(error.to_string());
                            return;
                        }
                    } else {
                        self.received.push(message);
                    }
                }
                if let Err(error) = self.connection.flush() {
                    self.gone = Some(error.to_string());
                }
            }
            Err(error) => self.gone = Some(error.to_string()),
        }
    }

    /// Its answer to `call`, as a toolkit's.
    fn answer(&self, call: &Message) -> Message {
        let path = call.path.as_deref().unwrap_or_default();
        let root = path == "/root";
        let role = |name: &str| names::ROLES.iter().position(|role| *role == name);
        let body = match call.member.as_deref().unwrap_or_default() {
            "GetChildren" => {
                let own = self.name();
                let children: Vec<(String, OwnedObjectPath)> = if root {
                    self.children
                        .iter()
                        .map(|child| {
                            let path = OwnedObjectPath::try_from(child.as_str()).expect("a path");
                            (own.clone(), path)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                Body::new("a(so)", &children)
            }
            "GetRole" => {
                let role = role(if root { "frame" } else { "button" }).expect("a role");
                Body::new("u", &u32::try_from(role).expect("a small number"))
            }
            "GetState" => Body::new("au", &[0_u32, 0_u32][..]),
            "GetExtents" => Body::new("(iiii)", &(10_i32, 20_i32, 30_i32, 40_i32)),
            "Get" => {
                let (_, property) = call.body::<(String, String)>("ss").expect("a property");
                let text = match (property.as_str(), root) {
                    ("Name", true) => "Fake",
                    ("Name", false) => "Press me",
                    ("ToolkitName", _) => "fake",
                    _ => "",
                };
                Body::new("v", &Variant::from(text))
            }
            _ => return Message::error(call, UNKNOWN_METHOD, "not a fake toolkit's"),
        };
        Message::method_return(call, body.expect("a body"))
    }
}

/// `Socket.<member>((so))` of the registry's root: `named`'s application, at `root`.
fn socket_call(member: &str, named: &str, root: &str) -> Message {
    let reference = (
        named.to_owned(),
        OwnedObjectPath::try_from(root).expect("a path"),
    );
    Message::method_call(
        registry::REGISTRY_NAME,
        registry::ROOT_PATH,
        "org.a11y.atspi.Socket",
        member,
        Body::new("(so)", &reference).expect("a body"),
    )
}

/// Whether every application on the bus has its process asked, so that a walk takes them all.
fn all_walkable(bus: &Bus) -> bool {
    let apps = bus.apps();
    !apps.is_empty() && apps.iter().all(|app| !app.pid_asked)
}

/// A program on a seat's bus speaks for its own connection alone. An `Embed` naming a name that
/// is no bus name, or another program's connection, is refused with `InvalidArgs`, and a
/// `NameOwnerChanged` it sends itself forgets nothing: it can neither make the bus disconnect
/// Yuna, which would end the bus for every program of the seat, nor hide another program. The
/// bus stays, and the other program's tree is walked; the bus's own `NameOwnerChanged` still
/// forgets an application whose connection left.
#[test]
fn a_program_on_the_bus_speaks_for_its_own_connection_alone() {
    let mut bus = Bus::ready("own");
    let daemon = bus.accessibility.daemon_pid(bus.seat);
    let mut toolkit = Program::connect(&mut bus, &["/root/button"]);
    let mut rogue = Program::connect(&mut bus, &[]);
    let toolkit_name = toolkit.name();
    // GTK 4 and Qt name their own connection.
    let embedded = toolkit.send(socket_call("Embed", &toolkit_name, "/root"));
    bus.run_until(
        "the toolkit is embedded",
        &mut [&mut toolkit],
        |_, programs| programs[0].reply(embedded).is_some(),
    );
    let (yuna, _) = toolkit
        .reply(embedded)
        .expect("a reply")
        .body::<(String, OwnedObjectPath)>("(so)")
        .expect("the registry's root");

    let not_a_name = rogue.send(socket_call("Embed", "x", "/r"));
    let another = rogue.send(socket_call("Embed", &toolkit_name, "/r"));
    let mut left = Message::signal(
        crate::dbus::BUS_PATH,
        BUS_INTERFACE,
        "NameOwnerChanged",
        Body::new("sss", &(&toolkit_name, &toolkit_name, "")).expect("a body"),
    );
    left.destination = Some(yuna.clone());
    rogue.send(left);
    // Yuna reads a connection's messages in order: its answer to this follows the rest.
    let ping = rogue.send(Message::method_call(
        &yuna,
        "/",
        "org.freedesktop.DBus.Peer",
        "Ping",
        Body::empty(),
    ));
    bus.run_until(
        "the rogue's calls are answered and every process known",
        &mut [&mut toolkit, &mut rogue],
        |bus, programs| programs[1].reply(ping).is_some() && all_walkable(bus),
    );

    let found = bus.find("press", &mut [&mut toolkit, &mut rogue]);
    assert_eq!(found["isError"], json!(false), "{found}");
    assert_eq!(
        found["structuredContent"]["nodes"][0]["name"],
        json!("Press me"),
        "{found}"
    );
    assert!(bus.ended.is_empty(), "{:?}", bus.ended);
    assert_eq!(bus.accessibility.daemon_pid(bus.seat), daemon);
    for (what, serial) in [("no bus name", not_a_name), ("another's", another)] {
        let reply = rogue.reply(serial).expect("a reply");
        assert_eq!(reply.kind, Kind::Error, "{what}");
        assert_eq!(reply.error_name.as_deref(), Some(INVALID_ARGS), "{what}");
    }
    let names: Vec<String> = bus.apps().into_iter().map(|app| app.bus_name).collect();
    assert_eq!(names, [toolkit_name]);

    // The toolkit leaves: the bus says so, and its application is forgotten.
    drop(toolkit);
    bus.run_until(
        "the application is forgotten",
        &mut [&mut rogue],
        |bus, _| bus.apps().is_empty(),
    );
    assert!(rogue.gone.is_none(), "{:?}", rogue.gone);
}

/// A toolkit that gives the bus's reserved path as a node's child costs that node alone: Yuna
/// never sends a call there, for which the bus would disconnect it and end the seat's bus; the
/// application's other nodes are walked, and the bus stays.
#[test]
fn a_node_at_the_buss_reserved_path_costs_that_node_alone() {
    let mut bus = Bus::ready("local");
    let daemon = bus.accessibility.daemon_pid(bus.seat);
    let mut toolkit = Program::connect(&mut bus, &[LOCAL_PATH, "/root/button"]);
    // An empty name is the sender's, as registryd takes it.
    let embedded = toolkit.send(socket_call("Embed", "", "/root"));
    bus.run_until(
        "the toolkit is embedded and its process known",
        &mut [&mut toolkit],
        |bus, programs| programs[0].reply(embedded).is_some() && all_walkable(bus),
    );

    let found = bus.find("press", &mut [&mut toolkit]);
    assert_eq!(found["isError"], json!(false), "{found}");
    let found = &found["structuredContent"];
    assert_eq!(found["nodes"][0]["name"], json!("Press me"), "{found}");
    assert_eq!(found["apps"][0]["status"], json!("complete"), "{found}");
    assert!(bus.ended.is_empty(), "{:?}", bus.ended);
    assert_eq!(bus.accessibility.daemon_pid(bus.seat), daemon);
    assert!(toolkit.gone.is_none(), "{:?}", toolkit.gone);
}

/// When Yuna's connection to a bus has its first source and its second cannot be made, as when
/// a descriptor cannot be copied past the process's limit, the first is removed with it: the bus
/// ends, and Yuna rests, rather than wake for ever on the socket its daemon's end leaves
/// readable.
#[test]
fn a_connection_whose_second_source_fails_leaves_yuna_at_rest() {
    let mut bus = Bus::starting("watch");
    // The daemon's pidfd and address are watched; the connection's first source is made, its
    // second refused.
    bus.accessibility.watches_before_failure.set(Some(1));
    bus.run_until("the bus ends", &mut [], |bus, _| !bus.ended.is_empty());
    assert!(
        bus.ended[0].1.contains("a source the test refused"),
        "{:?}",
        bus.ended
    );
    for _ in 0..3 {
        let before = bus.compositor.counters().wakeups;
        bus.compositor
            .dispatch(Some(Duration::from_millis(200)))
            .expect("a round");
        assert_eq!(
            bus.compositor.counters().wakeups,
            before,
            "Yuna woke at rest: {:?}",
            bus.compositor.rest_counts()
        );
        assert!(!bus.accessibility.has_work());
    }
}
