//! The manager's handling of X11's events: the windows it follows, from their creation to their
//! destruction, what they ask, their properties' changes, X11's stacking of the root's children,
//! and the client messages it takes; the selections' and the drags' events go on to
//! `xwayland/bridge.rs` and `xwayland/xdnd.rs`.
//!
//! It is apart from Yuna's commands (`commands.rs`), so that what X11 tells the manager and what
//! Yuna asks of it read one at a time. `serve.rs` reads the events and hands each to
//! [`Manager::event`]; `reading.rs` reads the properties they call for; the state they change is
//! the parent module's.

use x11rb::protocol::sync::{Alarm, ConnectionExt as _};
use x11rb::protocol::xproto::{
    Atom, ChangeWindowAttributesAux, ClientMessageEvent, ConfigWindow, ConfigureNotifyEvent,
    ConfigureRequestEvent, ConnectionExt as _, EventMask, Place, PropMode, Window,
};
use x11rb::protocol::{ErrorKind, Event};
use x11rb::wrapper::ConnectionExt as _;

use super::messages::{ManagerEvent, StateAction, StateChange};
use super::reading::Read;
use super::{
    Failure, GoOn, ICONIC_STATE, MAX_PROPERTIES_PER_CLIENT, Manager, Tracked, WITHDRAWN_STATE,
};
use crate::xwayland::properties::{Properties, X11Rect};

/// The bit of an event's type that says a client sent it with `SendEvent` (X11's protocol,
/// section 10).
const SENT_EVENT: u8 = 0x80;

/// `_NET_WM_STATE`'s actions (EWMH, `_NET_WM_STATE`).
const STATE_REMOVE: u32 = 0;
const STATE_ADD: u32 = 1;
const STATE_TOGGLE: u32 = 2;

impl Manager<'_> {
    /// Handles one X11 event.
    pub(super) fn event(&mut self, event: Event) -> Result<GoOn, Failure> {
        // Any X11 client may send any event to any window (`SendEvent`): the server marks it as
        // sent. Only client messages and a selection owner's answers are meant to come so;
        // another sent event, a made-up `DestroyNotify` or `MapRequest` of another program's
        // window, is taken as nothing.
        if event.sent_event()
            && !matches!(event, Event::ClientMessage(_) | Event::SelectionNotify(_))
        {
            tracing::trace!(?event, "an event another X11 client made up, left");
            return Ok(true);
        }
        let go_on = match event {
            Event::CreateNotify(created) if created.parent == self.root => self.track(
                created.window,
                X11Rect {
                    x: i32::from(created.x),
                    y: i32::from(created.y),
                    width: u32::from(created.width),
                    height: u32::from(created.height),
                },
                created.override_redirect,
            )?,
            Event::ReparentNotify(moved) if moved.window != self.own => {
                if moved.parent == self.root {
                    let geometry = X11Rect {
                        x: i32::from(moved.x),
                        y: i32::from(moved.y),
                        ..X11Rect::default()
                    };
                    self.track(moved.window, geometry, moved.override_redirect)?
                } else {
                    self.forget(moved.window)
                }
            }
            Event::DestroyNotify(destroyed) => {
                self.bridge.window_destroyed(destroyed.window);
                self.xdnd.window_destroyed(destroyed.window)? && self.forget(destroyed.window)
            }
            Event::MapRequest(request) => {
                let past_limit = self
                    .windows
                    .get_mut(&request.window)
                    .is_some_and(|tracked| {
                        tracked.override_redirect = false;
                        tracked.past_limit
                    });
                if past_limit {
                    self.map(request.window)?;
                    true
                } else {
                    self.post(ManagerEvent::MapRequest(request.window))
                }
            }
            Event::MapNotify(mapped) if !self.xdnd.mapped(mapped.window)? => false,
            Event::MapNotify(mapped) => match self.windows.get_mut(&mapped.window) {
                Some(tracked) if !tracked.mapped => {
                    tracked.mapped = true;
                    tracked.override_redirect = mapped.override_redirect;
                    let window = mapped.window;
                    !self.told(window)
                        || self.post(ManagerEvent::Mapped {
                            window,
                            override_redirect: mapped.override_redirect,
                        }) && self.post_place_in_stacking(window)
                }
                _ => true,
            },
            Event::UnmapNotify(unmapped) if unmapped.window == self.xdnd.proxy() => {
                self.xdnd.proxy_unmapped()
            }
            // A client withdrawing its window sends a synthetic one too (ICCCM 4.1.4): either
            // comes first, the other finds it unmapped.
            Event::UnmapNotify(unmapped) => self.unmapped(unmapped.window)?,
            Event::ConfigureNotify(configured) => self.configured(&configured),
            Event::CirculateNotify(circulated) => {
                let window = circulated.window;
                let above = match circulated.place {
                    Place::ON_TOP => self.order.iter().rev().find(|&&w| w != window).copied(),
                    _ => None,
                };
                self.restacked(window, above)
            }
            Event::ConfigureRequest(request) => self.configure_request(&request),
            Event::PropertyNotify(changed) => {
                self.bridge.property(&changed)?
                    && self.property_changed(changed.window, changed.atom)?
            }
            Event::ClientMessage(message) if self.xdnd.takes(&message) => {
                self.xdnd.client_message(&message, &mut self.bridge)?
            }
            Event::ClientMessage(message) => self.client_message(&message),
            Event::XfixesSelectionNotify(changed)
                if changed.selection == self.atoms.XdndSelection =>
            {
                self.xdnd.owner_changed(&changed, &mut self.bridge)?
            }
            Event::XfixesSelectionNotify(changed) => self.bridge.owner_changed(&changed)?,
            Event::SelectionNotify(answer) => {
                if self.xdnd.converted(&answer)? {
                    let xdnd = &self.xdnd;
                    self.bridge
                        .converted(&answer, |drag: u64| xdnd.drag_time(drag))?
                } else {
                    false
                }
            }
            // The keyboard may have come to one of the seat's X11 windows just before the
            // request that follows from it, or a drag moved: the commands sent so far are
            // carried out first.
            Event::SelectionRequest(request) if request.selection == self.atoms.XdndSelection => {
                self.take_commands()? && self.xdnd.requested(&request, &mut self.bridge)?
            }
            Event::SelectionRequest(request) => {
                self.take_commands()? && self.bridge.requested(&request)?
            }
            Event::SyncAlarmNotify(alarm) => match self.alarms.get(&alarm.alarm) {
                Some(&window) => {
                    let value = alarm.counter_value;
                    // A counter below 0 has reached no value Yuna sends.
                    let value = (i64::from(value.hi) << 32 | i64::from(value.lo)).max(0) as u64;
                    self.post(ManagerEvent::SyncCounter { window, value })
                }
                None => true,
            },
            Event::Error(error) => {
                // A request on a window its client destroyed meanwhile, most often.
                tracing::debug!(?error, "an X11 request of the manager failed");
                if error.error_kind == ErrorKind::Window {
                    self.bridge.window_failed(error.bad_value)?
                        && self.xdnd.window_destroyed(error.bad_value)?
                } else {
                    true
                }
            }
            event => {
                tracing::trace!(?event, "an X11 event the manager leaves");
                true
            }
        };
        Ok(go_on)
    }

    /// Follows a new top-level window: its properties' changes, its properties now and its
    /// process, posted together. A window past its client's [`MAX_PROPERTIES_PER_CLIENT`] is
    /// followed without them, and never posted.
    fn track(
        &mut self,
        window: Window,
        geometry: X11Rect,
        override_redirect: bool,
    ) -> Result<GoOn, Failure> {
        if window == self.own || window == self.xdnd.proxy() || self.windows.contains_key(&window) {
            return Ok(true);
        }
        let client = window & !self.resource_mask;
        let with_properties = self.with_properties.entry(client).or_default();
        let past_limit = *with_properties >= MAX_PROPERTIES_PER_CLIENT;
        let mut tracked = Tracked {
            properties: Properties::default(),
            geometry,
            override_redirect,
            mapped: false,
            managed: false,
            logged: false,
            alarm: None,
            commits_held: false,
            past_limit,
        };
        // A window made or reparented to the root goes on top of its siblings.
        self.order.push(window);
        if past_limit {
            tracing::debug!(
                window,
                "an X11 window past its client's limit is followed without its properties"
            );
            self.windows.insert(window, tracked);
            return Ok(true);
        }
        *with_properties += 1;
        // Selected before the properties are read, so that no change between falls through.
        self.connection.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        let pid = self.pid_of(window)?;
        for (read, raw) in self.read_all(window)? {
            self.apply(window, &mut tracked, read, raw.as_ref());
        }
        let properties = Box::new(tracked.properties.clone());
        self.windows.insert(window, tracked);
        Ok(self.post(ManagerEvent::Created {
            window,
            client,
            override_redirect,
            geometry,
            pid,
            properties,
        }))
    }

    /// Forgets a window gone from the root, and tells Yuna, if Yuna was told of it.
    fn forget(&mut self, window: Window) -> GoOn {
        self.bridge.window_left(window);
        let Some(tracked) = self.windows.remove(&window) else {
            return true;
        };
        self.order.retain(|&stacked| stacked != window);
        self.unlist(window);
        if tracked.past_limit {
            return true;
        }
        let client = window & !self.resource_mask;
        if let Some(count) = self.with_properties.get_mut(&client) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.with_properties.remove(&client);
            }
        }
        if let Some((alarm, _)) = tracked.alarm {
            self.destroy_alarm(alarm);
        }
        self.post(ManagerEvent::Destroyed(window))
    }

    /// A window moved, changed size or was restacked. An override-redirect window's new
    /// rectangle goes to Yuna, which draws it where its client puts it; so does its new place in
    /// the stacking once it is mapped. A managed window's place is Yuna's own already.
    fn configured(&mut self, configured: &ConfigureNotifyEvent) -> GoOn {
        let window = configured.window;
        let above = Some(configured.above_sibling).filter(|&sibling| sibling != x11rb::NONE);
        let Some(tracked) = self.windows.get_mut(&window) else {
            // The proxy, raised: its place counts for the menus mapped above it.
            if window == self.xdnd.proxy() {
                return self.restacked(window, above);
            }
            return true;
        };
        let rect = X11Rect {
            x: i32::from(configured.x),
            y: i32::from(configured.y),
            width: u32::from(configured.width),
            height: u32::from(configured.height),
        };
        let moved = std::mem::replace(&mut tracked.geometry, rect) != rect;
        tracked.override_redirect = configured.override_redirect;
        let posted = tracked.override_redirect && moved && !tracked.past_limit;
        if posted && !self.post(ManagerEvent::Configured { window, rect }) {
            return false;
        }
        self.restacked(window, above)
    }

    /// `window` now lies just above `above` in X11's stacking, or at the bottom with none; a
    /// sibling the manager does not follow puts it at the bottom. Yuna is told when it is a mapped
    /// override-redirect window whose place changed.
    fn restacked(&mut self, window: Window, above: Option<Window>) -> GoOn {
        let Some(from) = self.order.iter().position(|&stacked| stacked == window) else {
            return true;
        };
        self.order.remove(from);
        let to = above
            .and_then(|above| self.order.iter().position(|&stacked| stacked == above))
            .map_or(0, |below| below + 1);
        self.order.insert(to, window);
        if from == to {
            return true;
        }
        self.post_place_in_stacking(window)
    }

    /// Tells Yuna where `window`, if it is a mapped override-redirect window it was told of, lies
    /// among the others: just above the first one mapped below it.
    fn post_place_in_stacking(&self, window: Window) -> GoOn {
        let is_menu = |window: &Window| {
            self.windows.get(window).is_some_and(|tracked| {
                tracked.mapped && tracked.override_redirect && !tracked.past_limit
            })
        };
        if !is_menu(&window) {
            return true;
        }
        let above = self
            .order
            .iter()
            .take_while(|&&stacked| stacked != window)
            .filter(|stacked| is_menu(stacked))
            .last()
            .copied();
        self.post(ManagerEvent::Restacked { window, above })
    }

    /// A window was unmapped: a managed one is withdrawn (ICCCM 4.1.4) and leaves the lists. Its
    /// `_NET_WM_STATE` goes, as EWMH asks of a manager at withdrawal: mapped again, the window
    /// starts from what its client sets then, not from the states it had.
    fn unmapped(&mut self, window: Window) -> Result<GoOn, Failure> {
        self.bridge.window_left(window);
        let Some(tracked) = self.windows.get_mut(&window) else {
            return Ok(true);
        };
        if !tracked.mapped {
            return Ok(true);
        }
        tracked.mapped = false;
        let told = !tracked.past_limit;
        if std::mem::take(&mut tracked.managed) {
            self.connection.change_property32(
                PropMode::REPLACE,
                window,
                self.atoms.WM_STATE,
                self.atoms.WM_STATE,
                &[WITHDRAWN_STATE, x11rb::NONE],
            )?;
            self.connection
                .delete_property(window, self.atoms._NET_WM_STATE)?;
            // Held for a sync request that will not be answered now: Xwayland reads it again
            // when the window is mapped, and would never commit it.
            if std::mem::take(&mut tracked.commits_held) {
                self.connection
                    .delete_property(window, self.atoms._XWAYLAND_ALLOW_COMMITS)?;
            }
            self.unlist(window);
        }
        Ok(!told || self.post(ManagerEvent::Unmapped(window)))
    }

    /// Takes `window` out of `_NET_CLIENT_LIST` and `_NET_CLIENT_LIST_STACKING`, if it is there.
    fn unlist(&mut self, window: Window) {
        let listed = self.clients.contains(&window);
        self.clients.retain(|&listed| listed != window);
        self.stacking.retain(|&listed| listed != window);
        if listed {
            self.publish_lists();
        }
    }

    /// Passes a client's `ConfigureRequest` to Yuna: what it asks, its current place on the axes
    /// it does not name. Its stacking is left: Yuna stacks windows. A window Yuna was not told of
    /// keeps its size.
    fn configure_request(&self, request: &ConfigureRequestEvent) -> GoOn {
        if !self.told(request.window) {
            return true;
        }
        let current = self
            .windows
            .get(&request.window)
            .map_or_else(X11Rect::default, |tracked| tracked.geometry);
        let mask = request.value_mask;
        let asked = X11Rect {
            x: if mask.contains(ConfigWindow::X) {
                i32::from(request.x)
            } else {
                current.x
            },
            y: if mask.contains(ConfigWindow::Y) {
                i32::from(request.y)
            } else {
                current.y
            },
            width: if mask.contains(ConfigWindow::WIDTH) {
                u32::from(request.width)
            } else {
                current.width
            },
            height: if mask.contains(ConfigWindow::HEIGHT) {
                u32::from(request.height)
            } else {
                current.height
            },
        };
        self.post(ManagerEvent::ConfigureRequest {
            window: request.window,
            asked,
        })
    }

    /// A property changed: when Yuna reads it, read again, and every property posted.
    fn property_changed(&mut self, window: Window, atom: Atom) -> Result<GoOn, Failure> {
        let Some(read) = Read::ALL
            .into_iter()
            .find(|read| read.atom(&self.atoms) == atom)
        else {
            return Ok(true);
        };
        if !self.told(window) {
            return Ok(true);
        }
        let Some(mut tracked) = self.windows.remove(&window) else {
            return Ok(true);
        };
        let raw = self.read_one(window, read)?;
        self.apply(window, &mut tracked, read, raw.as_ref());
        // Another counter: the next sync request watches it from 0, as Yuna counts again.
        if let Some((alarm, counter)) = tracked.alarm
            && tracked.properties.sync_counter != Some(counter)
        {
            tracked.alarm = None;
            self.destroy_alarm(alarm);
        }
        let properties = Box::new(tracked.properties.clone());
        self.windows.insert(window, tracked);
        Ok(self.post(ManagerEvent::Properties { window, properties }))
    }

    /// The client messages the manager takes; others are left.
    fn client_message(&self, message: &ClientMessageEvent) -> GoOn {
        let atoms = &self.atoms;
        let data = message.data.as_data32();
        let window = message.window;
        if message.format != 32 {
            return true;
        }
        if message.type_ == atoms.WL_SURFACE_SERIAL {
            // Xwayland delivers its own from within the server; a copy with SendEvent's bit came
            // from a client, which could pair its window with another's surface.
            if message.response_type & SENT_EVENT != 0 {
                tracing::debug!(window, "a WL_SURFACE_SERIAL sent by a client is ignored");
                return true;
            }
            if !self.told(window) {
                return true;
            }
            let serial = u64::from(data[0]) | (u64::from(data[1]) << 32);
            return self.post(ManagerEvent::SurfaceSerial { window, serial });
        }
        // A ping's answer comes back on the root, naming its window (EWMH, `_NET_WM_PING`).
        if window == self.root
            && message.type_ == atoms.WM_PROTOCOLS
            && data[0] == atoms._NET_WM_PING
        {
            let pinged = data[2];
            if self.told(pinged) {
                return self.post(ManagerEvent::PingAnswered(pinged));
            }
            return true;
        }
        if !self.told(window) {
            return true;
        }
        if message.type_ == atoms._NET_WM_STATE {
            let action = match data[0] {
                STATE_REMOVE => StateAction::Remove,
                STATE_ADD => StateAction::Add,
                STATE_TOGGLE => StateAction::Toggle,
                _ => return true,
            };
            let names = |atom: Atom| data[1] == atom || data[2] == atom;
            let change = StateChange {
                action,
                fullscreen: names(atoms._NET_WM_STATE_FULLSCREEN),
                maximized: names(atoms._NET_WM_STATE_MAXIMIZED_HORZ),
            };
            if names(atoms._NET_WM_STATE_HIDDEN) {
                tracing::debug!(
                    window,
                    "an X11 window asks to be hidden: Yuna minimises nothing"
                );
            }
            if change.fullscreen || change.maximized {
                return self.post(ManagerEvent::StateRequest { window, change });
            }
        } else if message.type_ == atoms._NET_ACTIVE_WINDOW {
            return self.post(ManagerEvent::ActivateRequest(window));
        } else if message.type_ == atoms._NET_WM_MOVERESIZE {
            tracing::debug!(
                window,
                "an X11 window asks to be moved: Yuna has no interactive move"
            );
        } else if message.type_ == atoms.WM_CHANGE_STATE && data[0] == ICONIC_STATE {
            tracing::debug!(
                window,
                "an X11 window asks to be iconified: Yuna minimises nothing"
            );
        }
        true
    }

    /// Destroys a sync alarm; an error, for a client gone, goes to the event queue.
    fn destroy_alarm(&mut self, alarm: Alarm) {
        self.alarms.remove(&alarm);
        if let Err(error) = self.connection.sync_destroy_alarm(alarm) {
            tracing::debug!(%error, "a sync alarm cannot be destroyed");
        }
    }
}
