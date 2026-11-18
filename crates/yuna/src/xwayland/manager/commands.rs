//! The manager carrying out Yuna's commands, [`Command`], with what ICCCM and EWMH make follow
//! from each; the selections' and the drags' commands go on to `xwayland/bridge.rs` and
//! `xwayland/xdnd.rs`.
//!
//! It is apart from X11's events (`x11_events.rs`), so that what Yuna asks of the manager and
//! what X11 tells it read one at a time. `serve.rs` takes the commands at each wake-up, and
//! `x11_events.rs` before it answers a selection request; the state they change is the parent
//! module's.

use std::sync::mpsc;

use x11rb::connection::Connection;
use x11rb::protocol::sync::{ConnectionExt as _, CreateAlarmAux, Int64, TESTTYPE, VALUETYPE};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConfigureNotifyEvent, ConfigureWindowAux, ConnectionExt as _,
    EventMask, InputFocus, PropMode, StackMode, Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::messages::{Command, ManagerEvent};
use super::{Failure, GoOn, Manager, NORMAL_STATE};
use crate::xwayland::properties::X11Rect;

impl Manager<'_> {
    /// Carries out every command Yuna's loop sent so far; false once Yuna let them go.
    pub(super) fn take_commands(&mut self) -> Result<GoOn, Failure> {
        let commands = self.commands;
        loop {
            match commands.try_recv() {
                Ok(command) => {
                    if !self.command(command)? {
                        return Ok(false);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(true),
                Err(mpsc::TryRecvError::Disconnected) => return Ok(false),
            }
        }
    }

    /// Carries out one of Yuna's commands; false once Yuna's loop is gone. A request on a window
    /// gone meanwhile fails in the event queue, logged there.
    fn command(&mut self, command: Command) -> Result<GoOn, Failure> {
        let connection = self.connection;
        let atoms = &self.atoms;
        match command {
            Command::Map(window) => {
                // Xwayland reads it when it makes the window's surface: a hold left from before
                // would keep the window from ever showing.
                if let Some(tracked) = self.windows.get_mut(&window)
                    && std::mem::take(&mut tracked.commits_held)
                {
                    connection.delete_property(window, atoms._XWAYLAND_ALLOW_COMMITS)?;
                }
                connection.change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms.WM_STATE,
                    atoms.WM_STATE,
                    &[NORMAL_STATE, x11rb::NONE],
                )?;
                // Yuna draws no frame: none is added around the window.
                connection.change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms._NET_FRAME_EXTENTS,
                    AtomEnum::CARDINAL,
                    &[0, 0, 0, 0],
                )?;
                connection.map_window(window)?;
                if let Some(tracked) = self.windows.get_mut(&window) {
                    tracked.managed = true;
                    if !self.clients.contains(&window) {
                        self.clients.push(window);
                        self.stacking.push(window);
                        self.publish_lists();
                    }
                }
            }
            Command::Configure { window, rect, sync } => {
                if let Some(value) = sync {
                    self.request_sync(window, value)?;
                }
                configure(connection, window, rect)?;
            }
            Command::AllowCommits(window) => {
                if let Some(tracked) = self.windows.get_mut(&window)
                    && std::mem::take(&mut tracked.commits_held)
                {
                    connection.change_property32(
                        PropMode::REPLACE,
                        window,
                        atoms._XWAYLAND_ALLOW_COMMITS,
                        AtomEnum::CARDINAL,
                        &[1],
                    )?;
                }
            }
            Command::Ping(window) => {
                let message = ClientMessageEvent::new(
                    32,
                    window,
                    atoms.WM_PROTOCOLS,
                    [atoms._NET_WM_PING, x11rb::CURRENT_TIME, window, 0, 0],
                );
                connection.send_event(false, window, EventMask::NO_EVENT, message)?;
            }
            Command::State { window, state } => {
                // A command sent before Yuna learnt of a withdrawal would set again the states
                // the withdrawal deleted.
                if !self
                    .windows
                    .get(&window)
                    .is_some_and(|tracked| tracked.managed)
                {
                    return Ok(true);
                }
                let mut names = Vec::new();
                if state.fullscreen {
                    names.push(atoms._NET_WM_STATE_FULLSCREEN);
                }
                if state.maximized {
                    names.push(atoms._NET_WM_STATE_MAXIMIZED_HORZ);
                    names.push(atoms._NET_WM_STATE_MAXIMIZED_VERT);
                }
                if state.hidden {
                    names.push(atoms._NET_WM_STATE_HIDDEN);
                }
                connection.change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms._NET_WM_STATE,
                    AtomEnum::ATOM,
                    &names,
                )?;
            }
            Command::Focus(window) => self.focus(window)?,
            Command::Close(window) => {
                // A window destroyed while the command was on its way: its id may already name
                // another program's resource, which `KillClient` would end.
                let Some(tracked) = self.windows.get(&window) else {
                    return Ok(true);
                };
                if tracked.properties.protocols.delete_window {
                    let message = ClientMessageEvent::new(
                        32,
                        window,
                        atoms.WM_PROTOCOLS,
                        [atoms.WM_DELETE_WINDOW, x11rb::CURRENT_TIME, 0, 0, 0],
                    );
                    connection.send_event(false, window, EventMask::NO_EVENT, message)?;
                } else {
                    connection.kill_client(window)?;
                }
            }
            Command::Restack(order) => {
                // From the first window whose place in the order changed: those below it stand
                // as they were, so that one window opening among a thousand restacks a few.
                let kept = self
                    .restacked
                    .iter()
                    .zip(&order)
                    .take_while(|(was, is)| was == is)
                    .count();
                if kept == 0
                    && let Some(&bottom) = order.first()
                {
                    connection.configure_window(
                        bottom,
                        &ConfigureWindowAux::new().stack_mode(StackMode::BELOW),
                    )?;
                }
                for pair in order.windows(2).skip(kept.saturating_sub(1)) {
                    connection.configure_window(
                        pair[1],
                        &ConfigureWindowAux::new()
                            .sibling(pair[0])
                            .stack_mode(StackMode::ABOVE),
                    )?;
                }
                self.restacked.clone_from(&order);
                let listed: Vec<Window> = order
                    .into_iter()
                    .filter(|window| self.clients.contains(window))
                    .collect();
                if listed != self.stacking {
                    self.stacking = listed;
                    self.publish_lists();
                }
            }
            Command::OwnSelection {
                selection,
                serial,
                mime_types,
            } => self.bridge.own(selection, serial, mime_types)?,
            Command::SendToWayland {
                selection,
                owner,
                mime_type,
                pipe,
            } => {
                return self
                    .bridge
                    .send_to_wayland(selection, owner, &mime_type, pipe);
            }
            Command::WaylandData { transfer, source } => {
                return self.bridge.wayland_data(transfer, source);
            }
            Command::ShowDragProxy { drag, shown } => self.xdnd.show_proxy(drag, shown)?,
            Command::AnswerX11Drag {
                drag,
                accepted,
                action,
            } => self.xdnd.answer(drag, accepted, action)?,
            Command::SendX11Drag {
                drag,
                mime_type,
                pipe,
            } => {
                return self
                    .xdnd
                    .send_to_wayland(drag, &mime_type, pipe, &mut self.bridge);
            }
            Command::FinishX11Drag {
                drag,
                accepted,
                action,
            } => self.xdnd.finish(drag, accepted, action, &self.bridge)?,
            Command::ForgetX11Drag(drag) => self.xdnd.forget(drag)?,
            Command::StartWaylandDrag {
                drag,
                mime_types,
                action,
            } => {
                return self
                    .xdnd
                    .start_wayland(drag, &mime_types, action, &mut self.bridge);
            }
            Command::WaylandDragOver { drag, over } => return self.xdnd.wayland_over(drag, over),
            Command::DropWaylandDrag(drag) => return self.xdnd.drop_wayland(drag),
            Command::CancelWaylandDrag(drag) => return self.xdnd.cancel_wayland(drag),
        }
        Ok(true)
    }

    /// Sends `window` a sync request of `value` (`_NET_WM_SYNC_REQUEST`), ahead of a configure,
    /// and has Xwayland hold its commits until Yuna lets them go: then the frame drawn at the
    /// new size shows whole. Its sync counter is watched by an alarm made at its first request,
    /// which reports each change; the counter is set to 0 then, as KWin and Smithay set it, so
    /// that Yuna's values, counted from 1, are ahead of it. Without XSync, without a counter, or
    /// for a window the manager no longer sees mapped, which Yuna asked before it learnt of the
    /// unmapping, the answer is posted at once: nothing waits, and nothing holds the commits of a
    /// window mapped again.
    fn request_sync(&mut self, window: Window, value: u64) -> Result<(), Failure> {
        let connection = self.connection;
        let atoms = &self.atoms;
        let has_sync = self.sync;
        let Some(tracked) = self.windows.get_mut(&window) else {
            return Ok(());
        };
        let counter = tracked
            .properties
            .sync_counter
            .filter(|_| has_sync && tracked.mapped && tracked.managed);
        let Some(counter) = counter else {
            self.post(ManagerEvent::SyncCounter { window, value });
            return Ok(());
        };
        if tracked.alarm.is_none() {
            connection.sync_set_counter(counter, Int64 { hi: 0, lo: 0 })?;
            // Relative 1, delta 1, positive comparison: the alarm fires at each increase of the
            // counter, as Smithay's (`xwayland/xwm/surface.rs`, MIT) does.
            let alarm = connection.generate_id()?;
            let one = Int64 { hi: 0, lo: 1 };
            connection.sync_create_alarm(
                alarm,
                &CreateAlarmAux::new()
                    .counter(counter)
                    .value_type(VALUETYPE::RELATIVE)
                    .value(one)
                    .test_type(TESTTYPE::POSITIVE_COMPARISON)
                    .delta(one)
                    .events(1),
            )?;
            tracked.alarm = Some((alarm, counter));
            self.alarms.insert(alarm, window);
        }
        tracked.commits_held = true;
        connection.change_property32(
            PropMode::REPLACE,
            window,
            atoms._XWAYLAND_ALLOW_COMMITS,
            AtomEnum::CARDINAL,
            &[0],
        )?;
        let message = ClientMessageEvent::new(
            32,
            window,
            atoms.WM_PROTOCOLS,
            [
                atoms._NET_WM_SYNC_REQUEST,
                x11rb::CURRENT_TIME,
                value as u32,
                (value >> 32) as u32,
                0,
            ],
        );
        connection.send_event(false, window, EventMask::NO_EVENT, message)?;
        Ok(())
    }

    /// Gives the X input focus to `window` as its model asks (ICCCM 4.1.7): Passive and Locally
    /// Active windows, which take input, get `SetInputFocus`; Locally and Globally Active ones,
    /// which list `WM_TAKE_FOCUS`, are sent it; a No Input window gets neither. A window without
    /// `WM_HINTS` takes input, as most managers have it. With none, the focus goes to none.
    /// `_NET_ACTIVE_WINDOW` names it. Yuna sends it whenever the seat's keyboard goes to one of
    /// the seat's X11 windows or leaves them: the bridge reads Wayland's selections for X11 only
    /// while one has it.
    fn focus(&mut self, window: Option<Window>) -> Result<(), Failure> {
        self.bridge.focus_moved(window);
        let connection = self.connection;
        let atoms = &self.atoms;
        match window {
            Some(window) => {
                let (input, take_focus) = self.windows.get(&window).map_or((true, false), |w| {
                    (
                        w.properties.accepts_input.unwrap_or(true),
                        w.properties.protocols.take_focus,
                    )
                });
                if input {
                    connection.set_input_focus(InputFocus::NONE, window, x11rb::CURRENT_TIME)?;
                }
                if take_focus {
                    let message = ClientMessageEvent::new(
                        32,
                        window,
                        atoms.WM_PROTOCOLS,
                        [atoms.WM_TAKE_FOCUS, x11rb::CURRENT_TIME, 0, 0, 0],
                    );
                    connection.send_event(false, window, EventMask::NO_EVENT, message)?;
                }
            }
            None => {
                connection.set_input_focus(InputFocus::NONE, x11rb::NONE, x11rb::CURRENT_TIME)?;
            }
        }
        connection.change_property32(
            PropMode::REPLACE,
            self.root,
            atoms._NET_ACTIVE_WINDOW,
            AtomEnum::WINDOW,
            &[window.unwrap_or(x11rb::NONE)],
        )?;
        Ok(())
    }
}

/// Moves and sizes `window` to `rect`, without a border, and tells it with a synthetic
/// `ConfigureNotify`, as ICCCM 4.1.5 asks of a manager that moves a window: the real one does
/// not come when only its place relative to its parent is unchanged. The rectangle is cut to
/// X11's 16-bit coordinates.
fn configure(connection: &RustConnection, window: Window, rect: X11Rect) -> Result<(), Failure> {
    let x = i16::try_from(rect.x).unwrap_or(if rect.x < 0 { i16::MIN } else { i16::MAX });
    let y = i16::try_from(rect.y).unwrap_or(if rect.y < 0 { i16::MIN } else { i16::MAX });
    let side = |value: u32| u16::try_from(value.clamp(1, 32_767)).unwrap_or(u16::MAX);
    let (width, height) = (side(rect.width), side(rect.height));
    connection.configure_window(
        window,
        &ConfigureWindowAux::new()
            .x(i32::from(x))
            .y(i32::from(y))
            .width(u32::from(width))
            .height(u32::from(height))
            .border_width(0),
    )?;
    let notify = ConfigureNotifyEvent {
        response_type: x11rb::protocol::xproto::CONFIGURE_NOTIFY_EVENT,
        sequence: 0,
        event: window,
        window,
        above_sibling: x11rb::NONE,
        x,
        y,
        width,
        height,
        border_width: 0,
        override_redirect: false,
    };
    connection.send_event(false, window, EventMask::STRUCTURE_NOTIFY, notify)?;
    Ok(())
}
