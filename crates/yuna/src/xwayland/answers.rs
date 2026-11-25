//! The answers Yuna waits for from X11 windows, each bounded by a deadline on Yuna's clock,
//! armed only while one waits (`clock.rs`): a sync request's, so that a resized window shows its
//! new size whole, and a ping's, so that a frozen program is noticed
//! ([ADR-0066](../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md)).
//!
//! **Sync requests** (EWMH `_NET_WM_SYNC_REQUEST`). A configure that changes the size of a mapped
//! window that lists the protocol and has a sync counter goes with a sync request of the next
//! value, and Xwayland holds the window's commits (`_XWAYLAND_ALLOW_COMMITS` 0,
//! `manager/commands.rs`): Yuna keeps drawing the window's last complete frame. Once the counter
//! reaches the value, or after [`X11_SYNC_TIMEOUT`], the commits go on. One request waits at a
//! time, as KWin learnt clients need: a configure meanwhile goes without one, and the wait keeps
//! its deadline, so that a window resized again and again is never held past it.
//!
//! **Pings** (EWMH `_NET_WM_PING`). Asked to close, a window that lists the protocol is pinged.
//! An answer within [`X11_PING_TIMEOUT`] does nothing more; without one the window is marked
//! unresponsive, which `windows` gives, until it answers a later ping. It is never ended on a
//! timer: its user decides.

use std::time::Duration;

use x11rb::protocol::xproto::Window;

use super::manager::Command;
use super::properties::X11Rect;
use super::windows::X11Toplevel;
use crate::clock::Deadline;
use crate::state::State;

/// How long Xwayland holds a resized window's commits for its sync request: a client draws a
/// new size in a frame or two, 33 ms at 60 Hz; past six frames at 60 Hz a still window reads as
/// stuck. Then its frames show as they come, as without the protocol, logged once per window.
pub const X11_SYNC_TIMEOUT: Duration = Duration::from_millis(100);

/// How long a window asked to close has to answer its ping before it is marked unresponsive:
/// EWMH's suggestion, and KWin's first ping's.
pub const X11_PING_TIMEOUT: Duration = Duration::from_secs(5);

/// What an X11 window is waited for.
#[derive(Debug, Default)]
pub(super) struct Waits {
    /// The value of its last sync request: values count from 1 for each counter it names.
    sync_value: u64,
    /// The value its sync request waits for, and when Yuna stops waiting for it.
    sync: Option<(u64, Duration)>,
    /// Whether a sync request it left unanswered was said: once per window.
    sync_late_said: bool,
    /// When its unanswered ping marks it unresponsive.
    ping: Option<Duration>,
    /// It left a ping unanswered, and has answered none since.
    unresponsive: bool,
}

impl Waits {
    /// Whether a sync request or a ping of it waits: its going changes the deadline.
    pub(super) fn waiting(&self) -> bool {
        self.sync.is_some() || self.ping.is_some()
    }
}

impl State {
    /// Configures `window` to `rect`: with a sync request when it changes the size of a mapped
    /// window that takes them and none waits.
    pub(super) fn x11_configure(&mut self, server: u64, window: Window, rect: X11Rect) {
        let now = self.clock.now();
        let Some(x11_window) = self.x11_window_mut(server, window) else {
            return;
        };
        let resized = x11_window
            .told
            .is_some_and(|told| (told.width, told.height) != (rect.width, rect.height));
        x11_window.told = Some(rect);
        let syncs = x11_window.properties.protocols.sync_request
            && x11_window.properties.sync_counter.is_some()
            && x11_window.is_shown_managed();
        let waits = &mut x11_window.waits;
        let due = now.saturating_add(X11_SYNC_TIMEOUT);
        let sync = (resized && syncs && waits.sync.is_none()).then(|| {
            waits.sync_value += 1;
            waits.sync = Some((waits.sync_value, due));
            waits.sync_value
        });
        self.x11
            .send(server, Command::Configure { window, rect, sync });
        if sync.is_some() {
            // The clock keeps the earlier of two times: a wait added needs no walk of them all.
            self.clock.arm(Deadline::X11Answers, due);
        }
    }

    /// `window`'s sync counter reached `value`: a sync request waiting for it, or for less, is
    /// answered, and Xwayland commits the window again.
    pub(super) fn x11_sync_counter(&mut self, server: u64, window: Window, value: u64) {
        let Some(x11_window) = self.x11_window_mut(server, window) else {
            return;
        };
        if !x11_window
            .waits
            .sync
            .is_some_and(|(waited, _)| value >= waited)
        {
            return;
        }
        x11_window.waits.sync = None;
        self.x11.send(server, Command::AllowCommits(window));
        self.arm_x11_answers();
    }

    /// `window` names another sync counter, or none: its values count from 1 again, and a sync
    /// request waiting is let go.
    pub(super) fn x11_sync_counter_changed(&mut self, server: u64, window: Window) {
        let Some(x11_window) = self.x11_window_mut(server, window) else {
            return;
        };
        x11_window.waits.sync_value = 0;
        if x11_window.waits.sync.take().is_some() {
            self.x11.send(server, Command::AllowCommits(window));
            self.arm_x11_answers();
        }
    }

    /// `window` was withdrawn: nothing it was waited for is waited for now. Its manager gave its
    /// commits back.
    pub(super) fn x11_waits_end(&mut self, server: u64, window: Window) {
        let Some(x11_window) = self.x11_window_mut(server, window) else {
            return;
        };
        let waited =
            x11_window.waits.sync.take().is_some() | x11_window.waits.ping.take().is_some();
        x11_window.waits.unresponsive = false;
        if let Some(id) = self.x11_layout_window(server, window) {
            self.events.window_answers_changed(id);
        }
        if waited {
            self.arm_x11_answers();
        }
    }

    /// Pings `window`, asked to close, if it takes pings and no ping of it waits.
    pub(super) fn x11_ping(&mut self, server: u64, window: Window) {
        let due = self.clock.now().saturating_add(X11_PING_TIMEOUT);
        let Some(x11_window) = self.x11_window_mut(server, window) else {
            return;
        };
        let protocols = x11_window.properties.protocols;
        if !(protocols.ping && protocols.delete_window) || x11_window.waits.ping.is_some() {
            return;
        }
        x11_window.waits.ping = Some(due);
        self.x11.send(server, Command::Ping(window));
        self.clock.arm(Deadline::X11Answers, due);
    }

    /// `window` answered a ping: it responds again.
    pub(super) fn x11_ping_answered(&mut self, server: u64, window: Window) {
        let Some(x11_window) = self.x11_window_mut(server, window) else {
            return;
        };
        let waited = x11_window.waits.ping.take().is_some();
        if std::mem::take(&mut x11_window.waits.unresponsive) {
            tracing::info!(window, "an X11 window answers pings again");
            if let Some(id) = self.x11_layout_window(server, window) {
                self.events.window_answers_changed(id);
            }
        }
        if waited {
            self.arm_x11_answers();
        }
    }

    /// The X11 answers' deadline: each sync request whose time is up is let go, and Xwayland
    /// commits its window again; each ping whose time is up marks its window unresponsive.
    pub(crate) fn x11_answers_due(&mut self) {
        let now = self.clock.now();
        let mut released = Vec::new();
        let mut unanswered = Vec::new();
        for (&server, record) in &mut self.x11.servers {
            for (&window, x11_window) in &mut record.windows {
                let waits = &mut x11_window.waits;
                if waits.sync.is_some_and(|(_, due)| due <= now) {
                    waits.sync = None;
                    released.push((server, window));
                    if !std::mem::replace(&mut waits.sync_late_said, true) {
                        tracing::info!(
                            window,
                            timeout = ?X11_SYNC_TIMEOUT,
                            "an X11 window did not answer its sync request in time: its frames \
                             show as they come; said once per window"
                        );
                    }
                }
                if waits.ping.is_some_and(|due| due <= now) {
                    waits.ping = None;
                    waits.unresponsive = true;
                    unanswered.push((server, window));
                    tracing::info!(
                        window,
                        "an X11 window left its ping unanswered: unresponsive"
                    );
                }
            }
        }
        for (server, window) in unanswered {
            if let Some(id) = self.x11_layout_window(server, window) {
                self.events.window_answers_changed(id);
            }
        }
        for (server, window) in released {
            self.x11.send(server, Command::AllowCommits(window));
        }
        self.arm_x11_answers();
    }

    /// Arms the X11 answers' deadline at the earliest time a sync request or a ping waits until;
    /// disarms it when none waits. Called when a wait goes, its window or its Xwayland with it:
    /// a wait added arms the deadline itself.
    pub(super) fn arm_x11_answers(&mut self) {
        let due = self
            .x11
            .servers
            .values()
            .flat_map(|record| record.windows.values())
            .flat_map(|x11_window| {
                let waits = &x11_window.waits;
                waits.sync.map(|(_, due)| due).into_iter().chain(waits.ping)
            })
            .min();
        self.clock.disarm(Deadline::X11Answers);
        if let Some(at) = due {
            self.clock.arm(Deadline::X11Answers, at);
        }
    }

    /// Whether the X11 window `x11` left a ping unanswered.
    pub(crate) fn x11_unresponsive(&self, x11: &X11Toplevel) -> bool {
        self.x11
            .servers
            .get(&x11.server)
            .and_then(|record| record.windows.get(&x11.window))
            .is_some_and(|x11_window| x11_window.waits.unresponsive)
    }
}
