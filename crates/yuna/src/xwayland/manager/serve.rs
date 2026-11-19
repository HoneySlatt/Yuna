//! The manager serving: the loop that carries out Yuna's commands, moves the selections'
//! transfers and the drags, and handles X11's events, Yuna's commands taken between them too; and
//! its writing of the root's lists of windows. Between them it sleeps in `poll` on its
//! connection, its wake-up socket and the pipes its transfers wait on, until the first transfer's
//! or drag's time limit: with neither, until something comes.
//!
//! It is apart from what each wake-up does, X11's events in `x11_events.rs` and Yuna's commands
//! in `commands.rs`, so that the order of one round, and why the thread sleeps where it does,
//! reads in one place. The state it serves with, [`Manager`], is the parent module's.

use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::Instant;

use smithay::reexports::rustix;
use smithay::reexports::rustix::event::{PollFd, PollFlags, Timespec};
use smithay::reexports::rustix::io::Errno;
use smithay::reexports::rustix::net::RecvFlags;
use x11rb::connection::Connection;
use x11rb::errors::ConnectionError;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{AtomEnum, PropMode};
use x11rb::wrapper::ConnectionExt as _;

use super::{Failure, Manager, ManagerConnection};

impl Manager<'_> {
    /// Reads X11's events and Yuna's commands, and moves the transfers of selections and the
    /// drags, until Xwayland closes the connection or Yuna lets the commands go. Sleeps in `poll`
    /// between them, woken by X11, by Yuna, by a transfer's pipe, or by a transfer's or a drag's
    /// time limit: nothing is polled on a timer, and with neither the sleep has no end but those.
    pub(super) fn serve(&mut self, woken: &UnixStream) -> Result<(), Failure> {
        loop {
            if !self.take_commands()? || !self.bridge.run()? || !self.xdnd.run(&self.bridge)? {
                return Ok(());
            }
            let xdnd = &self.xdnd;
            self.bridge.keep_drag_moves(
                |drag| xdnd.follows_x11(drag),
                |drag| xdnd.follows_wayland(drag),
            );
            // Flushed before the events are read: a flush that would block reads X11's packets
            // into x11rb's own queue, where `poll` would not see them, so that an event could
            // wait for unrelated traffic. Handling events sends requests too: flushed again, and
            // the queue read again, until a flush leaves nothing to handle. At the sleep, the
            // queue and the socket are both empty.
            loop {
                self.write_lists();
                self.connection.flush()?;
                let mut handled = false;
                while let Some(event) = next_event(self.connection)? {
                    handled = true;
                    // Yuna's commands are taken between events too: a flood of events, which
                    // may last seconds, never keeps them waiting, nor reads as the stall Yuna's
                    // watchdog ends (`xwayland/mod.rs`).
                    if !self.event(event)? || !self.take_commands()? {
                        return Ok(());
                    }
                }
                if !handled {
                    break;
                }
            }
            let deadline = [
                self.bridge.next_deadline(),
                self.xdnd.next_deadline(&self.bridge),
            ]
            .into_iter()
            .flatten()
            .min();
            let timeout = deadline.and_then(|deadline| {
                Timespec::try_from(deadline.saturating_duration_since(Instant::now())).ok()
            });
            let mut sleep = vec![
                PollFd::new(self.connection.stream(), PollFlags::IN),
                PollFd::new(woken, PollFlags::IN),
            ];
            sleep.extend(
                self.bridge
                    .waits()
                    .into_iter()
                    .map(|(pipe, flags)| PollFd::from_borrowed_fd(pipe, flags)),
            );
            match rustix::event::poll(&mut sleep, timeout.as_ref()) {
                Ok(_) | Err(Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
            drop(sleep);
            if !drain(woken)? {
                return Ok(());
            }
        }
    }

    /// Marks the root's `_NET_CLIENT_LIST` and `_NET_CLIENT_LIST_STACKING` changed: they are set
    /// once the events and commands at hand are handled, so that a thousand windows mapped or
    /// gone together set them once, not a thousand times.
    pub(super) fn publish_lists(&mut self) {
        self.lists_changed = true;
    }

    /// Sets the root's lists of windows, if they changed. Errors go to the event queue: nothing
    /// waits for them.
    fn write_lists(&mut self) {
        if !std::mem::take(&mut self.lists_changed) {
            return;
        }
        for (atom, list) in [
            (self.atoms._NET_CLIENT_LIST, &self.clients),
            (self.atoms._NET_CLIENT_LIST_STACKING, &self.stacking),
        ] {
            if let Err(error) = self.connection.change_property32(
                PropMode::REPLACE,
                self.root,
                atom,
                AtomEnum::WINDOW,
                list,
            ) {
                tracing::debug!(%error, "the root's lists of windows cannot be set");
            }
        }
    }
}

/// The next event read, none when none waits. An event x11rb cannot parse, which any X11 client
/// can make up and send to the root, is logged and skipped: it must not end the manager, and its
/// seat's every X11 program with it.
fn next_event(connection: &ManagerConnection) -> Result<Option<Event>, ConnectionError> {
    loop {
        match connection.poll_for_event() {
            Err(ConnectionError::ParseError(error)) => {
                tracing::debug!(%error, "an X11 event that cannot be read, left");
            }
            read => return read,
        }
    }
}

/// Reads every byte waiting on the wake-up socket; false once Yuna's end has closed.
fn drain(woken: &UnixStream) -> Result<bool, Failure> {
    let mut bytes = [0; 64];
    loop {
        match rustix::net::recv(woken.as_fd(), &mut bytes, RecvFlags::DONTWAIT) {
            Ok((0, _)) => return Ok(false),
            Ok(_) => {}
            Err(Errno::AGAIN) => return Ok(true),
            Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}
