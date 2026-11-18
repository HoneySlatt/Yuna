//! One walk of the accessibility trees of a seat's applications, for one `accessibility` call.
//!
//! A walk starts at each application's root and asks, of every node, its children
//! (`GetChildren`, since `Cache.GetItems` is empty in Qt and partial in GTK 4), its role
//! (`GetRole`) and its name (the `Name` property). A node whose role and name pass the call's
//! filter is a candidate: its states (`GetState`), its description and its extents in window
//! coordinates (`Component.GetExtents(1)`) are asked then; a window's own node, a child of the
//! root, has its extents asked too, which place its nodes in the window ([`Walk::rect_in_window`]).
//! Each application's root is also asked its toolkit's name.
//!
//! The walk only makes calls and takes their replies: its owner sends them, at most
//! [`IN_FLIGHT_PER_APP`] in flight for one application, and ends the walk at its deadline.
//! Bounds: [`Walk::new`]'s budget of nodes, all applications together; [`MAX_DEPTH`] levels; a
//! node's path seen once per application, so that a tree that loops is walked once; a child on
//! another connection, an application embedded in this one, is left to its own walk.

use std::collections::{HashSet, VecDeque};

use zbus::wire::{OwnedObjectPath, OwnedVariant, Variant};

use super::names;
use super::registry::{ACCESSIBLE, APPLICATION, App, COMPONENT, NULL_PATH, PROPERTIES};
use crate::dbus::{Body, Kind, Message};

/// How many of one application's calls may be in flight at once.
pub(super) const IN_FLIGHT_PER_APP: usize = 32;

/// How deep a walk goes below an application's root.
pub(super) const MAX_DEPTH: usize = 64;

/// AT-SPI's coordinates of a window: from its top-left corner.
const WINDOW_COORDINATES: u32 = 1;

/// What a call asks of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Query {
    Children,
    Role,
    Name,
    Description,
    States,
    Extents,
    /// The application's toolkit, asked of its root.
    Toolkit,
}

/// What a call's nodes must be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Filter {
    /// A part of the name, compared lowercased.
    pub name: Option<String>,
    /// A role, by AT-SPI's name.
    pub role: Option<String>,
    /// States the node must all have, by AT-SPI's names.
    pub states: Vec<String>,
}

impl Filter {
    /// Whether a node of `role` and `name` may match, its states not yet known.
    fn passes(&self, role: Option<u32>, name: Option<&str>) -> bool {
        let role_passes = self
            .role
            .as_ref()
            .is_none_or(|wanted| role.is_some_and(|role| names::role_name(role) == *wanted));
        let name_passes = self.name.as_ref().is_none_or(|part| {
            name.is_some_and(|name| name.to_lowercase().contains(&part.to_lowercase()))
        });
        role_passes && name_passes
    }

    /// Whether `states` hold every state the filter asks.
    fn has_states(&self, states: &[&str]) -> bool {
        self.states
            .iter()
            .all(|wanted| states.contains(&wanted.as_str()))
    }
}

/// A node of an application's tree, as far as its replies came.
#[derive(Debug, Clone, Default)]
pub(super) struct Node {
    pub path: String,
    pub depth: usize,
    /// The index of the window's node above it, a child of the root; none for the root.
    pub top: Option<usize>,
    pub role: Option<u32>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub states: Option<Vec<u32>>,
    /// `(x, y, width, height)`, in its window's coordinates.
    pub extents: Option<(i32, i32, i32, i32)>,
    /// Its children, role and name not yet answered.
    first_replies_left: u8,
    pub candidate: bool,
}

/// One application's part of a walk.
#[derive(Debug)]
pub(super) struct AppWalk {
    pub app: App,
    pub toolkit: Option<String>,
    pub nodes: Vec<Node>,
    seen: HashSet<String>,
    queue: VecDeque<(usize, Query)>,
    pub in_flight: usize,
    /// Why its walk ended early: its root could not be read.
    pub failed: Option<String>,
    /// Its walk was cut at the deadline, calls still in flight.
    pub timed_out: bool,
    /// The walk's budget of nodes left some of its nodes unvisited.
    pub cut: bool,
    /// Its walk was cut at its call's wait's end, calls still to ask or in flight.
    pub waited_out: bool,
}

impl AppWalk {
    /// Whether nothing is left to ask or wait for.
    pub(super) fn is_done(&self) -> bool {
        self.timed_out
            || self.waited_out
            || self.failed.is_some()
            || (self.queue.is_empty() && self.in_flight == 0)
    }
}

/// A walk of the trees of some applications.
#[derive(Debug)]
pub(super) struct Walk {
    pub apps: Vec<AppWalk>,
    filter: Filter,
    /// Nodes it may still visit.
    budget: usize,
    /// Nodes that passed the filter's role and name: past [`MAX_MATCHES`], they are counted
    /// and not asked more.
    candidates: usize,
}

/// How many nodes a walk asks the states, description and extents of, and gives.
pub(super) const MAX_MATCHES: usize = 256;

impl Walk {
    /// A walk of `apps` for `filter`, visiting `budget` nodes at most.
    pub(super) fn new(apps: Vec<App>, filter: Filter, budget: usize) -> Walk {
        let mut walk = Walk {
            apps: Vec::new(),
            filter,
            budget,
            candidates: 0,
        };
        for app in apps {
            let mut app_walk = AppWalk {
                toolkit: None,
                nodes: Vec::new(),
                seen: HashSet::new(),
                queue: VecDeque::new(),
                in_flight: 0,
                failed: None,
                timed_out: false,
                cut: false,
                waited_out: false,
                app,
            };
            let root = app_walk.app.root.clone();
            if walk.take_budget() {
                app_walk.queue.push_back((0, Query::Toolkit));
                add_node(&mut app_walk, root, 0, None);
            } else {
                app_walk.cut = true;
            }
            walk.apps.push(app_walk);
        }
        walk
    }

    fn take_budget(&mut self) -> bool {
        if self.budget == 0 {
            return false;
        }
        self.budget -= 1;
        true
    }

    /// Whether more nodes passed the filter's role and name than [`MAX_MATCHES`].
    pub(super) fn has_more(&self) -> bool {
        self.candidates > MAX_MATCHES
    }

    /// Whether every application's walk is done.
    pub(super) fn is_done(&self) -> bool {
        self.apps.iter().all(AppWalk::is_done)
    }

    /// Cuts the walk of every application not done: the deadline came.
    pub(super) fn cut_at_deadline(&mut self) {
        for app in self.apps.iter_mut().filter(|app| !app.is_done()) {
            app.timed_out = true;
        }
    }

    /// Cuts the walk of every application not done: its call's wait ended first.
    pub(super) fn cut_at_wait(&mut self) {
        for app in self.apps.iter_mut().filter(|app| !app.is_done()) {
            app.waited_out = true;
        }
    }

    /// The next call of application `app` to send, while it has fewer than
    /// [`IN_FLIGHT_PER_APP`] in flight: its node, its query and its message.
    pub(super) fn next_call(&mut self, app: usize) -> Option<(usize, Query, Message)> {
        let app_walk = self.apps.get_mut(app)?;
        if app_walk.is_done() || app_walk.in_flight >= IN_FLIGHT_PER_APP {
            return None;
        }
        let (node, query) = app_walk.queue.pop_front()?;
        let path = app_walk.nodes.get(node)?.path.clone();
        let message = call_message(&app_walk.app.bus_name, &path, query)?;
        app_walk.in_flight += 1;
        Some((node, query, message))
    }

    /// Takes `reply`, a method return or an error, to `query` of node `node` of application
    /// `app`.
    pub(super) fn take_reply(&mut self, app: usize, node: usize, query: Query, reply: &Message) {
        let answer = match reply.kind {
            Kind::MethodReturn => Ok(reply),
            _ => Err(reply.error_text()),
        };
        self.take_answer(app, node, query, answer);
    }

    /// Takes the failure of a call that could not be sent: as an error's reply.
    pub(super) fn take_unsent(&mut self, app: usize, node: usize, query: Query) {
        let why = "the call could not be sent to the bus".to_owned();
        self.take_answer(app, node, query, Err(why));
    }

    /// Takes the answer to `query` of node `node` of application `app`: its reply, or why it
    /// has none.
    fn take_answer(
        &mut self,
        app: usize,
        node: usize,
        query: Query,
        answer: Result<&Message, String>,
    ) {
        let Some(app_walk) = self.apps.get_mut(app) else {
            return;
        };
        app_walk.in_flight = app_walk.in_flight.saturating_sub(1);
        if app_walk.timed_out || app_walk.waited_out || node >= app_walk.nodes.len() {
            return;
        }
        let reply = match answer {
            Ok(reply) => Some(reply),
            Err(why) if node == 0 && query == Query::Children => {
                // An application whose root cannot be read has nothing to walk: gone, or no
                // accessible object at the path it embedded.
                app_walk.failed = Some(why);
                return;
            }
            Err(_) => None,
        };
        match query {
            Query::Toolkit => app_walk.toolkit = reply.and_then(text_property),
            Query::Children => {
                let children = reply
                    .and_then(|reply| reply.body::<Vec<(String, OwnedObjectPath)>>("a(so)").ok())
                    .unwrap_or_default();
                self.add_children(app, node, children);
            }
            Query::Role => {
                self.apps[app].nodes[node].role =
                    reply.and_then(|reply| reply.body::<u32>("u").ok())
            }
            Query::Name => self.apps[app].nodes[node].name = reply.and_then(text_property),
            Query::Description => {
                self.apps[app].nodes[node].description = reply.and_then(text_property)
            }
            Query::States => {
                self.apps[app].nodes[node].states =
                    reply.and_then(|reply| reply.body::<Vec<u32>>("au").ok())
            }
            Query::Extents => {
                self.apps[app].nodes[node].extents =
                    reply.and_then(|reply| reply.body::<(i32, i32, i32, i32)>("(iiii)").ok())
            }
        }
        if matches!(query, Query::Children | Query::Role | Query::Name) {
            self.first_reply_of(app, node);
        }
    }

    /// Adds the children of node `node` of application `app`, those on its own connection, each
    /// seen once, while the budget and the depth allow.
    fn add_children(&mut self, app: usize, node: usize, children: Vec<(String, OwnedObjectPath)>) {
        let depth = self.apps[app].nodes[node].depth + 1;
        if depth > MAX_DEPTH {
            return;
        }
        // The window's node of every node below a child of the root.
        let top = if depth == 1 {
            None
        } else {
            self.apps[app].nodes[node].top.or(Some(node))
        };
        for (name, path) in children {
            let app_walk = &self.apps[app];
            let path = path.as_str().to_owned();
            let own = name == app_walk.app.bus_name || name.is_empty();
            if !own || path == NULL_PATH || app_walk.seen.contains(&path) {
                continue;
            }
            if !self.take_budget() {
                self.apps[app].cut = true;
                return;
            }
            add_node(&mut self.apps[app], path, depth, top);
        }
    }

    /// Counts one of the first three replies of a node; once all three came, a node that passes
    /// the filter is asked what it is to be told, and a window's node its extents.
    fn first_reply_of(&mut self, app: usize, node: usize) {
        let entry = &mut self.apps[app].nodes[node];
        entry.first_replies_left = entry.first_replies_left.saturating_sub(1);
        if entry.first_replies_left > 0 {
            return;
        }
        let passes = self.filter.passes(entry.role, entry.name.as_deref());
        if passes {
            self.candidates += 1;
        }
        let app_walk = &mut self.apps[app];
        let entry = &mut app_walk.nodes[node];
        entry.candidate = passes && self.candidates <= MAX_MATCHES;
        let window_node = entry.depth == 1;
        if entry.candidate {
            app_walk.queue.push_back((node, Query::States));
            app_walk.queue.push_back((node, Query::Description));
        }
        if entry.candidate || window_node {
            app_walk.queue.push_back((node, Query::Extents));
        }
    }

    /// The nodes that match the filter, states included, by application and node: in each
    /// application, in the order the walk found them.
    pub(super) fn matches(&self) -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        for (app_index, app) in self.apps.iter().enumerate() {
            for (node_index, node) in app.nodes.iter().enumerate() {
                if !node.candidate {
                    continue;
                }
                let states = node
                    .states
                    .as_deref()
                    .map(names::state_names)
                    .unwrap_or_default();
                if self.filter.has_states(&states) {
                    found.push((app_index, node_index));
                }
            }
        }
        found
    }

    /// Node `node`'s rectangle in its window, `(x, y, width, height)`, from the window's own
    /// node: its extents less the window node's, which a toolkit counts from the window's
    /// corner (Qt, `0, 0`) or from its own widget's origin (GTK 4, which sums each widget's
    /// bounds to its window's). None without extents.
    pub(super) fn rect_in_window(&self, app: usize, node: usize) -> Option<(i32, i32, i32, i32)> {
        let app_walk = self.apps.get(app)?;
        let entry = app_walk.nodes.get(node)?;
        let (x, y, width, height) = entry.extents?;
        let window_node = if entry.depth == 1 {
            Some(node)
        } else {
            entry.top
        };
        let (origin_x, origin_y) = window_node
            .and_then(|top| app_walk.nodes.get(top)?.extents)
            .map_or((0, 0), |(x, y, _, _)| (x, y));
        Some((
            x.saturating_sub(origin_x),
            y.saturating_sub(origin_y),
            width,
            height,
        ))
    }

    /// The window's node above node `node`, itself when it is one.
    pub(super) fn window_node(&self, app: usize, node: usize) -> Option<&Node> {
        let app_walk = self.apps.get(app)?;
        let entry = app_walk.nodes.get(node)?;
        let index = if entry.depth == 1 { node } else { entry.top? };
        app_walk.nodes.get(index)
    }
}

/// Adds a node at `path` and asks its children, role and name.
fn add_node(app: &mut AppWalk, path: String, depth: usize, top: Option<usize>) {
    let index = app.nodes.len();
    app.seen.insert(path.clone());
    app.nodes.push(Node {
        path,
        depth,
        top,
        first_replies_left: 3,
        ..Node::default()
    });
    for query in [Query::Children, Query::Role, Query::Name] {
        app.queue.push_back((index, query));
    }
}

/// The call that asks `query` of the object `path` of `bus_name`.
fn call_message(bus_name: &str, path: &str, query: Query) -> Option<Message> {
    let property = |interface: &str, name: &str| {
        let body = Body::new("ss", &(interface, name)).ok()?;
        Some(Message::method_call(
            bus_name, path, PROPERTIES, "Get", body,
        ))
    };
    match query {
        Query::Children => Some(Message::method_call(
            bus_name,
            path,
            ACCESSIBLE,
            "GetChildren",
            Body::empty(),
        )),
        Query::Role => Some(Message::method_call(
            bus_name,
            path,
            ACCESSIBLE,
            "GetRole",
            Body::empty(),
        )),
        Query::States => Some(Message::method_call(
            bus_name,
            path,
            ACCESSIBLE,
            "GetState",
            Body::empty(),
        )),
        Query::Name => property(ACCESSIBLE, "Name"),
        Query::Description => property(ACCESSIBLE, "Description"),
        Query::Toolkit => property(APPLICATION, "ToolkitName"),
        Query::Extents => {
            let body = Body::new("u", &WINDOW_COORDINATES).ok()?;
            Some(Message::method_call(
                bus_name,
                path,
                COMPONENT,
                "GetExtents",
                body,
            ))
        }
    }
}

/// A property's text, from a `Properties.Get` reply: a variant holding a string.
fn text_property(reply: &Message) -> Option<String> {
    let value = reply.body::<OwnedVariant>("v").ok()?;
    match &*value {
        Variant::Str(text) => Some(text.as_str().to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
