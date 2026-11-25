//! What the compositor tells has happened: [`Event`], one shape for every reader
//! ([ADR-0153](../../../docs/adr/0153-one-event-stream-marks-at-the-change-mcps-log-per-seat-the-bound.md)).

use yuna_types::{OutputId, Rect, SeatId, WindowId, WindowShell};

/// The longest title, application id or workspace name an event holds, in bytes: a longer one is
/// cut at a character's boundary, so that an event holds at most about 1 KiB of text. A change
/// past the cut is not told.
pub const MAX_EVENT_TEXT: usize = 512;

/// Something that happened on a seat, as the compositor tells it. Each event names its seat
/// ([`Event::seat`]): a reader that serves one seat keeps that seat's alone.
///
/// The compositor marks what changes where it changes, and builds events from its marks once a
/// round, and when a reader asks. A build compares each fact marked with what it last told, so
/// that a fact changed and changed back tells nothing, and one changed a thousand times tells one
/// event with its last value: a window is told at most once a build for each kind. A window
/// opened and closed between two builds is told opened, then closed.
///
/// A build gives its events in this order: outputs added and changed, in the order they were
/// created, then outputs removed, by id; then window by window, by id: opened, changed, placed,
/// unresponsive, and, for a window opened since the last build and closed already, closed; then
/// the windows closed that an earlier build told opened, by id; then the focus, by seat; then the
/// clients that left and the refusals of connections, in the order they came.
///
/// Plain data, with no wire format: Hollow's is decided with its interface (Stage 2c). It is
/// `#[non_exhaustive]`, so that a kind added is not a break for the programs that match on it:
/// such a program passes over a kind it does not know.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// A window opened: its client mapped it and the layout holds it.
    WindowOpened {
        /// The seat whose workspaces hold it: its client's.
        seat: SeatId,
        /// Its id, never reused; ids grow in the order windows open.
        window: WindowId,
        /// The application id its client set, or an X11 window's `WM_CLASS` class, if any, cut
        /// to [`MAX_EVENT_TEXT`].
        app_id: Option<String>,
        /// Its title, if any, cut to [`MAX_EVENT_TEXT`].
        title: Option<String>,
        /// The process id of its client, as the kernel gave it for the client's socket; for an
        /// X11 window, as X11 records its client's connection, else as the window says, never
        /// Xwayland's.
        pid: Option<i32>,
        /// The shell behind it.
        shell: WindowShell,
    },
    /// Its title or its application id changed, however often since the last build: both as
    /// they are now.
    WindowChanged {
        /// Its seat.
        seat: SeatId,
        /// The window.
        window: WindowId,
        /// Its title now, if any, cut to [`MAX_EVENT_TEXT`].
        title: Option<String>,
        /// Its application id now, if any, cut to [`MAX_EVENT_TEXT`].
        app_id: Option<String>,
    },
    /// Where it rests changed: its output, its workspace or its place in the layout. Told once
    /// the layout is arranged, never while an arrangement is pending.
    WindowPlaced {
        /// Its seat.
        seat: SeatId,
        /// The window.
        window: WindowId,
        /// The output that shows it; none when it is hidden.
        output: Option<OutputId>,
        /// Its workspace's name, cut to [`MAX_EVENT_TEXT`]; none for an unnamed workspace.
        workspace: Option<String>,
        /// Its place in the layout, in logical coordinates, where it rests once its motion ends;
        /// none when it is hidden.
        place: Option<Rect>,
    },
    /// It left a ping unanswered: an X11 window asked to close, its program frozen. Told again
    /// only once it has answered one since.
    WindowUnresponsive {
        /// Its seat.
        seat: SeatId,
        /// The window.
        window: WindowId,
    },
    /// The window closed: its client unmapped or destroyed it, or left.
    WindowClosed {
        /// Its seat.
        seat: SeatId,
        /// The window.
        window: WindowId,
    },
    /// The seat's focus moved to this window, or to none.
    Focus {
        /// The seat.
        seat: SeatId,
        /// The window it focuses now; none when it focuses none.
        window: Option<WindowId>,
    },
    /// An output was added to the seat.
    OutputAdded {
        /// The seat it belongs to.
        seat: SeatId,
        /// Its id.
        output: OutputId,
    },
    /// An output's mode or its place changed.
    OutputChanged {
        /// The seat it belongs to.
        seat: SeatId,
        /// Its id.
        output: OutputId,
    },
    /// An output was removed.
    OutputRemoved {
        /// The seat it belonged to.
        seat: SeatId,
        /// Its id.
        output: OutputId,
    },
    /// A Wayland connection of the seat ended: closed, or for a protocol error.
    ClientLeft {
        /// The seat it belonged to.
        seat: SeatId,
        /// The client's number, which the compositor gave it when it arrived; never reused.
        client: u64,
        /// The process id of the client, as the kernel gave it for its socket.
        pid: Option<i32>,
        /// The protocol error it was ended for; none when its connection closed: it left, sent
        /// what is not Wayland, or could not take the events written to it.
        error: Option<ProtocolError>,
    },
    /// The seat's socket refused connections, the seat serving `bound` clients already, its
    /// Xwayland apart. Told at the seat's first refusal, then at its first a second or more, on
    /// real time, after the last told; the refusals after the last told wait for such a
    /// refusal.
    ClientRefused {
        /// The seat.
        seat: SeatId,
        /// The most clients the seat serves.
        bound: usize,
        /// How many connections it refused since its last such event, this one's included.
        refused: u64,
    },
}

impl Event {
    /// The seat it happened on. The match names every kind, so that a kind added says its
    /// seat here.
    pub fn seat(&self) -> SeatId {
        match *self {
            Event::WindowOpened { seat, .. }
            | Event::WindowChanged { seat, .. }
            | Event::WindowPlaced { seat, .. }
            | Event::WindowUnresponsive { seat, .. }
            | Event::WindowClosed { seat, .. }
            | Event::Focus { seat, .. }
            | Event::OutputAdded { seat, .. }
            | Event::OutputChanged { seat, .. }
            | Event::OutputRemoved { seat, .. }
            | Event::ClientLeft { seat, .. }
            | Event::ClientRefused { seat, .. } => seat,
        }
    }
}

/// A protocol error a client was ended for: `wl_display.error`'s arguments, as Yuna sent them.
/// `#[non_exhaustive]`, so that a field added is not a break: a caller outside this crate reads
/// it, and makes one with [`ProtocolError::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProtocolError {
    /// The interface of the object it named.
    pub interface: String,
    /// That object's id.
    pub object: u32,
    /// The error's code, in that interface's `error` enum.
    pub code: u32,
    /// What Yuna, or the library under it, wrote of it.
    pub message: String,
}

impl ProtocolError {
    /// The error `code` of the object `object`, of interface `interface`, with `message`.
    pub fn new(interface: String, object: u32, code: u32, message: String) -> ProtocolError {
        ProtocolError {
            interface,
            object,
            code,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind names its seat.
    #[test]
    fn every_event_names_its_seat() {
        let seat = SeatId(4);
        let window = WindowId(1);
        let output = OutputId(2);
        let events = [
            Event::WindowOpened {
                seat,
                window,
                app_id: None,
                title: None,
                pid: None,
                shell: WindowShell::Xdg,
            },
            Event::WindowChanged {
                seat,
                window,
                title: None,
                app_id: None,
            },
            Event::WindowPlaced {
                seat,
                window,
                output: None,
                workspace: None,
                place: None,
            },
            Event::WindowUnresponsive { seat, window },
            Event::WindowClosed { seat, window },
            Event::Focus { seat, window: None },
            Event::OutputAdded { seat, output },
            Event::OutputChanged { seat, output },
            Event::OutputRemoved { seat, output },
            Event::ClientLeft {
                seat,
                client: 3,
                pid: None,
                error: None,
            },
            Event::ClientRefused {
                seat,
                bound: 256,
                refused: 1,
            },
        ];
        for event in events {
            assert_eq!(event.seat(), seat, "{event:?}");
        }
    }
}
