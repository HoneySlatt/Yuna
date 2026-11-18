//! Yuna as the AT-SPI registry of a seat's bus: it owns `org.a11y.atspi.Registry` and answers
//! what toolkits ask of the registry, at once, so that no program waits on it. There is no
//! `at-spi2-registryd` and no ping.
//!
//! What each toolkit asks, read in at-spi2-core 2.62.0, GTK 4.24.0 and Qt 6.12.0:
//!
//! | Call | Who asks | Yuna's answer |
//! |---|---|---|
//! | `Socket.Embed((so))` on the root | GTK 4 (`gtkatspiroot.c`, `root_register`, which exports nothing before the reply), Qt (`atspiadaptor.cpp`, `registerApplication`, waiting for the reply), the ATK bridge (`bridge.c`, `register_application`) | the application is recorded as its sender's connection, the name given empty or the sender's, another refused with `InvalidArgs` ([`caller_name`]); the reply is the registry's root, `(so)`, Yuna's unique name and `/org/a11y/atspi/accessible/root` |
//! | `Socket.Unembed((so))` | toolkits leaving | the sender's application is forgotten |
//! | `Registry.GetRegisteredEvents()` | GTK 4 after `Embed` (a critical error without it), Qt at start and on each listener's change, the ATK bridge after `Embed` | an empty `a(ss)`: no listener, so that no toolkit sends events, and Qt installs no key filter |
//! | `Registry.RegisterEvent`, `DeregisterEvent` | assistive programs | an empty reply; Yuna records no listener, being the only reader |
//! | `Registry.DeregisterApplication(s)` | the ATK bridge leaving, no reply asked | the sender's applications are forgotten |
//! | `DeviceEventController.GetKeystrokeListeners()`, `GetDeviceEventListeners()` | the ATK bridge after `Embed` | empty lists |
//! | `DeviceEventController.NotifyListenersSync(…)` | the ATK bridge for each key, Qt for each key while a listener is registered (never, here) | `false` at once: no listener consumed it |
//! | `DeviceEventController.NotifyListenersAsync(…)` | toolkits | an empty reply |
//! | `DeviceEventController.RegisterKeystrokeListener`, `DeregisterKeystrokeListener` | assistive programs | `false`, an empty reply: Yuna delivers no key |
//! | `DeviceEventController.GenerateKeyboardEvent`, `GenerateMouseEvent` | assistive programs | `NotSupported`: input comes through MCP's `keyboard` and `pointer` |
//! | the root's `Accessible` (`GetChildren`, `GetChildAtIndex`, `GetIndexInParent`, `GetRole`, `GetRoleName`, `GetLocalizedRoleName`, `GetState`, `GetAttributes`, `GetRelationSet`, `GetApplication`, `GetInterfaces`), its properties (`Name`, `Description`, `Parent`, `ChildCount`; `ToolkitName`, `Version`) | assistive programs | a `desktop frame` named `main` whose children are the applications embedded, as registryd's |
//! | `Cache.GetItems()` on `/org/a11y/atspi/cache` | assistive programs | an empty list |
//! | `Peer.Ping()` | anyone | an empty reply |
//!
//! Any other call that waits for a reply gets `UnknownMethod`. Unlike registryd, Yuna sets no
//! application's `Id`, which no toolkit needs, and sends no `ChildrenChanged` and no
//! `Available` signals, which no program here listens to: the bus stays quiet. At most
//! [`MAX_APPS`] applications are embedded; past it, `Embed` fails.

use std::collections::HashMap;

use serde::Serialize;
use zbus::wire::{ObjectPath, OwnedObjectPath, Variant};

use super::names::DESKTOP_FRAME;
use crate::dbus::{Body, FAILED, Kind, Message, NOT_SUPPORTED, UNKNOWN_METHOD};

/// The name Yuna owns on each seat's bus.
pub(super) const REGISTRY_NAME: &str = "org.a11y.atspi.Registry";
/// Every application's root, and the registry's.
pub(super) const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
/// The path of no object.
pub(super) const NULL_PATH: &str = "/org/a11y/atspi/null";
const REGISTRY_PATH: &str = "/org/a11y/atspi/registry";
const CONTROLLER_PATH: &str = "/org/a11y/atspi/registry/deviceeventcontroller";
const CACHE_PATH: &str = "/org/a11y/atspi/cache";

pub(super) const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
pub(super) const APPLICATION: &str = "org.a11y.atspi.Application";
pub(super) const COMPONENT: &str = "org.a11y.atspi.Component";
pub(super) const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const SOCKET: &str = "org.a11y.atspi.Socket";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const CONTROLLER: &str = "org.a11y.atspi.DeviceEventController";
const CACHE: &str = "org.a11y.atspi.Cache";
const PEER: &str = "org.freedesktop.DBus.Peer";

/// How many applications may be embedded on one seat's bus: twice the programs a Yuna runs.
pub(super) const MAX_APPS: usize = 128;

/// An application embedded: its connection's name on the bus and its root's path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct App {
    pub bus_name: String,
    pub root: String,
    /// Its process, as the bus knows its connection's, once asked.
    pub pid: Option<i32>,
    /// Whether its process is still being asked of the bus: a walk leaves it out meanwhile,
    /// since its nodes could not be given a window.
    pub pid_asked: bool,
}

/// What answering a call did.
#[derive(Debug, Default)]
pub(super) struct Answer {
    /// The reply, unless the caller asked for none.
    pub reply: Option<Message>,
    /// The bus name of an application embedded now, for the first time, whose process is to be
    /// asked.
    pub embedded: Option<String>,
}

/// The applications embedded on one seat's bus.
#[derive(Debug, Default)]
pub(super) struct Registry {
    apps: Vec<App>,
    /// Counts the applications embedded, so that a wait knows a new tree may be there.
    generation: u64,
}

/// The empty reference AT-SPI gives for no object.
fn null_reference(name: &str) -> (String, OwnedObjectPath) {
    (
        name.to_owned(),
        OwnedObjectPath::from(ObjectPath::from_static_str_unchecked(NULL_PATH)),
    )
}

/// The bus name of the application `call` speaks for, which it names `named`: its sender's, which
/// the bus sets and guarantees. An empty name is the sender's too, as registryd takes it; GTK 4
/// and Qt name their own (`tests/mcp.rs`). Another name is refused, so that no program has Yuna
/// call, or forget, another connection, nor a name that is no bus name, for which the bus would
/// disconnect Yuna and end the seat's bus.
fn caller_name(call: &Message, named: &str) -> Result<String, (&'static str, String)> {
    let Some(sender) = call.sender.as_deref() else {
        return Err((
            FAILED,
            "a call without a sender, which a bus always sets".to_owned(),
        ));
    };
    if named.is_empty() || named == sender {
        Ok(sender.to_owned())
    } else {
        Err((
            crate::dbus::INVALID_ARGS,
            format!("an application is its own connection's, {sender}, never another name"),
        ))
    }
}

impl Registry {
    /// The applications embedded, in the order they came.
    pub(super) fn apps(&self) -> &[App] {
        &self.apps
    }

    /// A number that grows each time an application is embedded.
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// Records the process of the connection `bus_name`, or that the bus could not tell it:
    /// either lets a walk take the application, a change a waiting call walks again for.
    pub(super) fn set_pid(&mut self, bus_name: &str, pid: Option<i32>) {
        for app in self.apps.iter_mut().filter(|app| app.bus_name == bus_name) {
            app.pid = pid;
            app.pid_asked = false;
        }
        self.generation += 1;
    }

    /// Forgets the applications of the connection `name`, which left the bus.
    pub(super) fn name_lost(&mut self, name: &str) {
        self.apps.retain(|app| app.bus_name != name);
    }

    /// Answers `call`, a method call to Yuna, whose unique name on the bus is `own_name`.
    pub(super) fn answer(&mut self, call: &Message, own_name: &str) -> Answer {
        if call.kind != Kind::MethodCall {
            return Answer::default();
        }
        let path = call.path.as_deref().unwrap_or_default();
        let interface = call.interface.as_deref().unwrap_or_default();
        let member = call.member.as_deref().unwrap_or_default();
        let mut embedded = None;
        let reply = match (path, interface, member) {
            (_, PEER, "Ping") => Ok(Body::empty()),
            (ROOT_PATH, SOCKET, "Embed") => match self.embed(call) {
                Ok(new) => {
                    embedded = new;
                    let root = (own_name, ObjectPath::from_static_str_unchecked(ROOT_PATH));
                    Body::new("(so)", &root).map_err(|e| (FAILED, e.to_string()))
                }
                Err(failure) => Err(failure),
            },
            (ROOT_PATH, SOCKET, "Unembed") => self.unembed(call),
            (ROOT_PATH, ACCESSIBLE, member) => self.root_accessible(call, member, own_name),
            (ROOT_PATH, PROPERTIES, "Get") => self.root_property(call),
            (ROOT_PATH, PROPERTIES, "GetAll") => self.root_properties(call),
            (REGISTRY_PATH, REGISTRY, "GetRegisteredEvents") => {
                encoded("a(ss)", &Vec::<(String, String)>::new())
            }
            (REGISTRY_PATH, REGISTRY, "RegisterEvent" | "DeregisterEvent") => Ok(Body::empty()),
            (REGISTRY_PATH, REGISTRY, "DeregisterApplication") => self.deregister(call),
            (CONTROLLER_PATH, CONTROLLER, "NotifyListenersSync") => encoded("b", &false),
            (
                CONTROLLER_PATH,
                CONTROLLER,
                "NotifyListenersAsync" | "DeregisterKeystrokeListener",
            ) => Ok(Body::empty()),
            (CONTROLLER_PATH, CONTROLLER, "RegisterKeystrokeListener") => encoded("b", &false),
            (CONTROLLER_PATH, CONTROLLER, "GetKeystrokeListeners") => {
                type Listener = (
                    String,
                    OwnedObjectPath,
                    u32,
                    u32,
                    Vec<(i32, i32, String, i32)>,
                    u32,
                    (bool, bool, bool),
                );
                encoded("a(souua(iisi)u(bbb))", &Vec::<Listener>::new())
            }
            (CONTROLLER_PATH, CONTROLLER, "GetDeviceEventListeners") => {
                encoded("a(sou)", &Vec::<(String, OwnedObjectPath, u32)>::new())
            }
            (CONTROLLER_PATH, CONTROLLER, "GenerateKeyboardEvent" | "GenerateMouseEvent") => Err((
                NOT_SUPPORTED,
                "Yuna takes input through MCP's keyboard and pointer tools".to_owned(),
            )),
            (CACHE_PATH, CACHE, "GetItems") => {
                type Reference = (String, OwnedObjectPath);
                type Item = (
                    Reference,
                    Reference,
                    Reference,
                    i32,
                    i32,
                    Vec<String>,
                    String,
                    u32,
                    String,
                    Vec<u32>,
                );
                encoded("a((so)(so)(so)iiassusau)", &Vec::<Item>::new())
            }
            _ => Err((
                UNKNOWN_METHOD,
                format!("Yuna's registry has no {interface}.{member} at {path}"),
            )),
        };
        let reply = call.expects_reply().then(|| match reply {
            Ok(body) => Message::method_return(call, body),
            Err((name, text)) => Message::error(call, name, &text),
        });
        Answer { reply, embedded }
    }

    /// Records the application `call` embeds, its sender's ([`caller_name`]): its name when it is
    /// new, none when it was embedded already, which is answered the same and neither counted
    /// again nor asked its process again.
    fn embed(&mut self, call: &Message) -> Result<Option<String>, (&'static str, String)> {
        let (named, root) = call
            .body::<(String, OwnedObjectPath)>("(so)")
            .map_err(|e| (crate::dbus::INVALID_ARGS, e.to_string()))?;
        let name = caller_name(call, &named)?;
        let app = App {
            bus_name: name.clone(),
            root: root.as_str().to_owned(),
            pid: None,
            pid_asked: true,
        };
        if self
            .apps
            .iter()
            .any(|known| known.bus_name == app.bus_name && known.root == app.root)
        {
            return Ok(None);
        }
        if self.apps.len() >= MAX_APPS {
            return Err((
                FAILED,
                format!("{MAX_APPS} applications are embedded already"),
            ));
        }
        self.apps.push(app);
        self.generation += 1;
        Ok(Some(name))
    }

    /// Forgets the application `call` unembeds, its sender's ([`caller_name`]) at the root it
    /// names. A malformed body forgets nothing and is answered all the same.
    fn unembed(&mut self, call: &Message) -> Result<Body, (&'static str, String)> {
        let Ok((named, root)) = call.body::<(String, OwnedObjectPath)>("(so)") else {
            return Ok(Body::empty());
        };
        let name = caller_name(call, &named)?;
        self.apps
            .retain(|app| !(app.bus_name == name && app.root == root.as_str()));
        Ok(Body::empty())
    }

    /// Forgets the applications of the connection `call` deregisters, its sender's
    /// ([`caller_name`]), as the ATK bridge leaves. A malformed body forgets nothing.
    fn deregister(&mut self, call: &Message) -> Result<Body, (&'static str, String)> {
        let Ok(named) = call.body::<String>("s") else {
            return Ok(Body::empty());
        };
        let name = caller_name(call, &named)?;
        self.name_lost(&name);
        Ok(Body::empty())
    }

    /// The references of the applications embedded, as the root's children.
    fn children(&self) -> Vec<(String, OwnedObjectPath)> {
        self.apps
            .iter()
            .filter_map(|app| {
                let path = OwnedObjectPath::try_from(app.root.clone()).ok()?;
                Some((app.bus_name.clone(), path))
            })
            .collect()
    }

    /// The root's `Accessible` methods, as registryd answers them.
    fn root_accessible(
        &self,
        call: &Message,
        member: &str,
        own_name: &str,
    ) -> Result<Body, (&'static str, String)> {
        match member {
            "GetChildren" => encoded("a(so)", &self.children()),
            "GetChildAtIndex" => {
                let index = call
                    .body::<i32>("i")
                    .map_err(|e| (crate::dbus::INVALID_ARGS, e.to_string()))?;
                let child = usize::try_from(index)
                    .ok()
                    .and_then(|index| self.children().into_iter().nth(index))
                    .unwrap_or_else(|| null_reference(REGISTRY_NAME));
                encoded("(so)", &child)
            }
            "GetIndexInParent" => encoded("i", &0_i32),
            "GetRelationSet" => encoded(
                "a(ua(so))",
                &Vec::<(u32, Vec<(String, OwnedObjectPath)>)>::new(),
            ),
            "GetRole" => encoded("u", &DESKTOP_FRAME),
            "GetRoleName" | "GetLocalizedRoleName" => encoded("s", "desktop frame"),
            "GetState" => encoded("au", &[0_u32, 0_u32][..]),
            "GetAttributes" => encoded("a{ss}", &HashMap::<String, String>::new()),
            "GetApplication" => encoded("(so)", &null_reference(own_name)),
            "GetInterfaces" => encoded("as", &[ACCESSIBLE, APPLICATION, SOCKET][..]),
            _ => Err((
                UNKNOWN_METHOD,
                format!("Yuna's registry has no {ACCESSIBLE}.{member}"),
            )),
        }
    }

    /// One property of the root, as `Properties.Get(ss)` asks.
    fn root_property(&self, call: &Message) -> Result<Body, (&'static str, String)> {
        let (interface, name) = call
            .body::<(String, String)>("ss")
            .map_err(|e| (crate::dbus::INVALID_ARGS, e.to_string()))?;
        let value = self
            .property(&interface, &name)
            .ok_or_else(|| (FAILED, "Property unavailable".to_owned()))?;
        encoded("v", &value)
    }

    /// Every property of one of the root's interfaces, as `Properties.GetAll(s)` asks.
    fn root_properties(&self, call: &Message) -> Result<Body, (&'static str, String)> {
        let interface = call
            .body::<String>("s")
            .map_err(|e| (crate::dbus::INVALID_ARGS, e.to_string()))?;
        let names: &[&str] = match interface.as_str() {
            ACCESSIBLE => &["Name", "Description", "Parent", "ChildCount"],
            APPLICATION => &["ToolkitName", "Version"],
            _ => &[],
        };
        let properties: HashMap<&str, Variant<'_>> = names
            .iter()
            .filter_map(|name| Some((*name, self.property(&interface, name)?)))
            .collect();
        encoded("a{sv}", &properties)
    }

    fn property(&self, interface: &str, name: &str) -> Option<Variant<'static>> {
        Some(match (interface, name) {
            (ACCESSIBLE, "Name") => Variant::from("main"),
            (ACCESSIBLE, "Description") => Variant::from(""),
            (ACCESSIBLE, "Parent") => {
                let (name, path) = null_reference("");
                Variant::from(zbus::wire::Structure::from((name, path)))
            }
            (ACCESSIBLE, "ChildCount") => {
                Variant::from(i32::try_from(self.apps.len()).unwrap_or(i32::MAX))
            }
            (APPLICATION, "ToolkitName") => Variant::from("Yuna"),
            (APPLICATION, "Version") => Variant::from(env!("CARGO_PKG_VERSION")),
            _ => return None,
        })
    }
}

/// `value` as a body of `signature`; an encoding that fails is the call's failure.
fn encoded<T: Serialize + ?Sized>(
    signature: &str,
    value: &T,
) -> Result<Body, (&'static str, String)> {
    Body::new(signature, value).map_err(|e| (FAILED, e.to_string()))
}

#[cfg(test)]
mod tests;
