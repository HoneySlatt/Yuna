//! A walk fed by hand, its replies built as an application would send them: what it asks, what
//! it keeps, where it stops.

use std::collections::HashMap;

use zbus::wire::{OwnedObjectPath, Variant};

use super::*;
use crate::dbus::Body;

const BUS: &str = ":1.4";

fn app() -> App {
    App {
        bus_name: BUS.to_owned(),
        root: "/root".to_owned(),
        pid: Some(42),
        pid_asked: false,
    }
}

/// An application's tree, by path: children, role, name, states, extents.
#[derive(Clone)]
struct Fake {
    children: Vec<(&'static str, &'static str)>,
    role: u32,
    name: &'static str,
    description: &'static str,
    /// The toolkit's name, which the root alone is asked.
    toolkit: &'static str,
    states: Vec<u32>,
    extents: Option<(i32, i32, i32, i32)>,
}

fn fake(children: &[(&'static str, &'static str)], role: u32, name: &'static str) -> Fake {
    Fake {
        children: children.to_vec(),
        role,
        name,
        description: "",
        toolkit: "GTK",
        states: vec![1 << 25, 0],
        extents: None,
    }
}

/// The reply `tree` gives to `call`, as the application would.
fn answer(tree: &HashMap<&str, Fake>, call: &Message) -> Message {
    let mut call = call.clone();
    call.serial = 5;
    call.sender = Some(":1.0".to_owned());
    let Some(node) = tree.get(call.path.as_deref().unwrap_or_default()) else {
        return Message::error(
            &call,
            "org.freedesktop.DBus.Error.UnknownObject",
            "no object",
        );
    };
    let body = match (call.interface.as_deref(), call.member.as_deref()) {
        (Some(ACCESSIBLE), Some("GetChildren")) => {
            let children: Vec<(String, OwnedObjectPath)> = node
                .children
                .iter()
                .map(|(bus, path)| ((*bus).to_owned(), OwnedObjectPath::try_from(*path).unwrap()))
                .collect();
            Body::new("a(so)", &children)
        }
        (Some(ACCESSIBLE), Some("GetRole")) => Body::new("u", &node.role),
        (Some(ACCESSIBLE), Some("GetState")) => Body::new("au", &node.states),
        (Some(PROPERTIES), Some("Get")) => {
            let (_, property): (String, String) = call.body("ss").unwrap();
            let text = match property.as_str() {
                "Name" => node.name,
                "Description" => node.description,
                "ToolkitName" => node.toolkit,
                _ => "",
            };
            Body::new("v", &Variant::from(text))
        }
        (Some(COMPONENT), Some("GetExtents")) => match node.extents {
            Some(extents) => Body::new("(iiii)", &extents),
            None => {
                return Message::error(
                    &call,
                    "org.freedesktop.DBus.Error.UnknownInterface",
                    "no component",
                );
            }
        },
        _ => return Message::error(&call, "org.freedesktop.DBus.Error.UnknownMethod", "no"),
    };
    let reply = Message::method_return(&call, body.unwrap());
    Message::decode(&reply.encode(9).unwrap()).unwrap()
}

/// Runs `walk` against `tree` until it is done, answering every call at once, in the order
/// sent; returns the number of calls sent.
fn run(walk: &mut Walk, tree: &HashMap<&str, Fake>) -> usize {
    let mut sent = 0;
    loop {
        let mut calls = Vec::new();
        for app in 0..walk.apps.len() {
            while let Some(call) = walk.next_call(app) {
                calls.push((app, call));
            }
        }
        if calls.is_empty() {
            return sent;
        }
        for (app, (node, query, message)) in calls {
            sent += 1;
            walk.take_reply(app, node, query, &answer(tree, &message));
        }
    }
}

/// A window with a box holding two buttons: the walk visits every node once, asks the details of
/// the button that matches and the window's own extents, and places the button in the window
/// from the window node's origin.
#[test]
fn a_walk_finds_a_button_by_name_and_places_it_from_its_window() {
    let mut tree = HashMap::new();
    tree.insert("/root", fake(&[(BUS, "/w")], 75, "demo"));
    let mut window = fake(&[(BUS, "/box")], 23, "Dialogs");
    window.extents = Some((-3, -3, 400, 300));
    tree.insert("/w", window);
    tree.insert("/box", fake(&[(BUS, "/ok"), (BUS, "/cancel")], 39, ""));
    let mut ok = fake(&[], 43, "_Message Dialog");
    ok.extents = Some((17, 47, 120, 34));
    ok.states = vec![(1 << 8) | (1 << 25), 0];
    tree.insert("/ok", ok);
    tree.insert("/cancel", fake(&[], 43, "Cancel"));

    let filter = Filter {
        name: Some("message DIALOG".to_owned()),
        role: Some("button".to_owned()),
        states: vec!["enabled".to_owned()],
    };
    let mut walk = Walk::new(vec![app()], filter, 100);
    run(&mut walk, &tree);
    assert!(walk.is_done());
    assert_eq!(walk.apps[0].toolkit.as_deref(), Some("GTK"));
    assert_eq!(walk.apps[0].nodes.len(), 5);
    let matches = walk.matches();
    assert_eq!(matches.len(), 1);
    let (app_index, node) = matches[0];
    let found = &walk.apps[app_index].nodes[node];
    assert_eq!(found.path, "/ok");
    assert_eq!(found.name.as_deref(), Some("_Message Dialog"));
    // From the window node's corner: 17 + 3, 47 + 3.
    assert_eq!(
        walk.rect_in_window(app_index, node),
        Some((20, 50, 120, 34))
    );
    assert_eq!(
        walk.window_node(app_index, node)
            .and_then(|w| w.name.as_deref()),
        Some("Dialogs")
    );
    // The other button is no candidate: its details were never asked.
    let cancel = walk.apps[0]
        .nodes
        .iter()
        .find(|n| n.path == "/cancel")
        .unwrap();
    assert!(!cancel.candidate && cancel.extents.is_none() && cancel.states.is_none());

    // A state it lacks keeps it out.
    let filter = Filter {
        name: Some("dialog".to_owned()),
        role: None,
        states: vec!["focused".to_owned()],
    };
    let mut walk = Walk::new(vec![app()], filter, 100);
    run(&mut walk, &tree);
    assert!(walk.matches().is_empty());
}

/// A tree that loops is walked once; a child on another connection, an application embedded in
/// this one, is left out; the budget of nodes cuts the walk and says so.
#[test]
fn a_walk_is_bounded_by_loops_other_connections_and_its_budget() {
    let mut tree = HashMap::new();
    tree.insert(
        "/root",
        fake(&[(BUS, "/a"), (":1.9", "/other"), ("", "/b")], 75, "demo"),
    );
    tree.insert(
        "/a",
        fake(
            &[(BUS, "/root"), (BUS, "/a"), (BUS, "/org/a11y/atspi/null")],
            23,
            "a",
        ),
    );
    tree.insert("/b", fake(&[(BUS, "/c")], 23, "b"));
    tree.insert("/c", fake(&[], 43, "c"));
    let mut walk = Walk::new(vec![app()], Filter::default(), 100);
    run(&mut walk, &tree);
    let paths: Vec<&str> = walk.apps[0].nodes.iter().map(|n| n.path.as_str()).collect();
    assert_eq!(paths, ["/root", "/a", "/b", "/c"]);
    assert!(!walk.apps[0].cut);

    let mut walk = Walk::new(vec![app()], Filter::default(), 2);
    run(&mut walk, &tree);
    assert_eq!(walk.apps[0].nodes.len(), 2);
    assert!(walk.apps[0].cut);

    // The depth: a chain of 70 levels stops at 64 below the root.
    let mut deep = HashMap::new();
    let paths: Vec<&'static str> = (0..=70)
        .map(|n| &*Box::leak(format!("/n{n}").into_boxed_str()))
        .collect();
    deep.insert("/root", fake(&[(BUS, paths[0])], 75, "deep"));
    for pair in paths.windows(2) {
        deep.insert(pair[0], fake(&[(BUS, pair[1])], 39, ""));
    }
    let mut walk = Walk::new(vec![app()], Filter::default(), 1000);
    run(&mut walk, &deep);
    assert_eq!(
        walk.apps[0].nodes.iter().map(|n| n.depth).max(),
        Some(MAX_DEPTH)
    );
}

/// An application whose root cannot be read fails alone; one that stops answering is cut at the
/// deadline with what came, and late replies change nothing.
#[test]
fn a_failing_or_silent_application_ends_its_own_walk_alone() {
    let mut gone = app();
    gone.root = "/missing".to_owned();
    let mut silent = app();
    silent.bus_name = ":1.7".to_owned();
    let mut tree = HashMap::new();
    tree.insert("/root", fake(&[], 75, "demo"));
    let mut walk = Walk::new(vec![gone, app(), silent], Filter::default(), 100);
    // Every call is sent; the silent application's are never answered.
    let mut unanswered = Vec::new();
    for app in 0..3 {
        while let Some((node, query, message)) = walk.next_call(app) {
            if app == 2 {
                unanswered.push((node, query));
            } else {
                walk.take_reply(app, node, query, &answer(&tree, &message));
            }
        }
    }
    assert!(walk.apps[0].failed.is_some() && walk.apps[0].is_done());
    assert!(walk.apps[1].is_done() && walk.apps[1].failed.is_none());
    assert!(!walk.apps[2].is_done());
    assert!(unanswered.len() <= IN_FLIGHT_PER_APP);
    walk.cut_at_deadline();
    assert!(walk.is_done() && walk.apps[2].timed_out);
    let (node, query) = unanswered[0];
    let late = answer(&tree, &call_message(":1.7", "/root", query).unwrap());
    walk.take_reply(2, node, query, &late);
    assert!(walk.apps[2].nodes[0].role.is_none());
}

/// At most 32 calls of one application are in flight.
#[test]
fn one_applications_calls_in_flight_are_bounded() {
    let children: Vec<(&'static str, &'static str)> = (0..100)
        .map(|n| (BUS, &*Box::leak(format!("/c{n}").into_boxed_str())))
        .collect();
    let mut tree = HashMap::new();
    tree.insert("/root", fake(&children, 75, "wide"));
    let mut walk = Walk::new(vec![app()], Filter::default(), 1000);
    // The root's own calls answered, its 100 children are queued.
    let mut first = Vec::new();
    while let Some(call) = walk.next_call(0) {
        first.push(call);
    }
    for (node, query, message) in first {
        walk.take_reply(0, node, query, &answer(&tree, &message));
    }
    let mut in_flight = 0;
    while walk.next_call(0).is_some() {
        in_flight += 1;
    }
    assert_eq!(in_flight, IN_FLIGHT_PER_APP);
}

/// What a walk keeps of a node's name is bounded as what it gives,
/// [`super::super::MAX_TEXT`] characters: a program on the seat's bus that names its nodes with
/// texts of megabytes must not make Yuna hold megabytes a node, up to `MAX_NODES` of them
/// (Stage 1's audit, m1.13-audit-mcp-4).
#[test]
fn a_walk_keeps_no_more_of_a_name_than_it_may_give() {
    let long: &'static str = Box::leak("n".repeat(3 << 20).into_boxed_str());
    let children: Vec<(&'static str, &'static str)> = (0..20)
        .map(|n| (BUS, &*Box::leak(format!("/c{n}").into_boxed_str())))
        .collect();
    let mut tree = HashMap::new();
    tree.insert("/root", fake(&children, 75, long));
    for (_, path) in &children {
        tree.insert(*path, fake(&[], 62, long));
    }
    // A filter that matches nothing: the walk visits every node and gives none.
    let filter = Filter {
        name: Some("no such name".to_owned()),
        ..Filter::default()
    };
    let mut walk = Walk::new(vec![app()], filter, 1000);
    run(&mut walk, &tree);
    let nodes = walk.apps[0].nodes.len();
    let kept: usize = walk.apps[0]
        .nodes
        .iter()
        .filter_map(|node| node.name.as_ref())
        .map(String::len)
        .sum();
    eprintln!("{nodes} nodes visited keep {kept} bytes of names");
    // A character takes 4 bytes in UTF-8 at most.
    let given = nodes * super::super::MAX_TEXT * 4;
    assert!(
        kept <= given,
        "{nodes} nodes keep {kept} bytes of names, past the {given} a walk may give of them"
    );
}

/// A description and a toolkit's name are kept as a walk gives them, cut at
/// [`super::super::MAX_TEXT`] characters with the mark of a cut, whatever their length.
#[test]
fn a_walk_keeps_no_more_of_a_description_or_a_toolkit_than_it_may_give() {
    let long: &'static str = Box::leak("é".repeat(1 << 20).into_boxed_str());
    let mut root = fake(&[(BUS, "/w")], 75, "demo");
    root.toolkit = long;
    let mut window = fake(&[], 23, "Dialogs");
    window.description = long;
    let mut tree = HashMap::new();
    tree.insert("/root", root);
    tree.insert("/w", window);
    // Every node is a candidate: its description is asked.
    let mut walk = Walk::new(vec![app()], Filter::default(), 100);
    run(&mut walk, &tree);
    let toolkit = walk.apps[0].toolkit.as_deref().expect("a toolkit");
    let description = walk.apps[0].nodes[1]
        .description
        .as_deref()
        .expect("a description");
    for kept in [toolkit, description] {
        assert_eq!(kept.chars().count(), super::super::MAX_TEXT + 1);
        assert!(kept.ends_with('…'));
    }
}

/// An object path past [`MAX_OBJECT_PATH`] bytes names no node the walk keeps: a child so named
/// is left out, one at the bound walked.
#[test]
fn a_walk_leaves_out_a_child_whose_path_passes_its_bound() {
    let past: &'static str =
        Box::leak(format!("/{}", "p".repeat(MAX_OBJECT_PATH)).into_boxed_str());
    let at: &'static str =
        Box::leak(format!("/{}", "a".repeat(MAX_OBJECT_PATH - 1)).into_boxed_str());
    let mut tree = HashMap::new();
    tree.insert("/root", fake(&[(BUS, past), (BUS, at)], 75, "demo"));
    tree.insert(past, fake(&[], 43, "past"));
    tree.insert(at, fake(&[], 43, "at"));
    let mut walk = Walk::new(vec![app()], Filter::default(), 100);
    run(&mut walk, &tree);
    let paths: Vec<&str> = walk.apps[0].nodes.iter().map(|n| n.path.as_str()).collect();
    assert_eq!(paths, ["/root", at]);
}

/// A node's states are kept as AT-SPI's state set, [`STATE_WORDS`] words, whatever a program
/// sends.
#[test]
fn a_walk_keeps_a_state_sets_words_alone() {
    let mut root = fake(&[], 75, "demo");
    root.states = vec![1 << 8; 1 << 16];
    let mut tree = HashMap::new();
    tree.insert("/root", root);
    let mut walk = Walk::new(vec![app()], Filter::default(), 100);
    run(&mut walk, &tree);
    let states = walk.apps[0].nodes[0].states.as_deref().expect("its states");
    assert_eq!(states.len(), STATE_WORDS);
    assert_eq!(states, [1 << 8; STATE_WORDS]);
}
