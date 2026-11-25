//! What happens on agents' seats, as the `events` tool tells it: each seat's log of its last
//! [`MAX_EVENTS`] events ([`EventLog`]), the compositor's own (`yuna_ipc::Event`, from
//! `Compositor::take_events`) and the programs' exits, which are MCP's.
//!
//! The server keeps no state of a request: the cursor is the agent's, the `seq` of an event or a
//! reply's `next` ([ADR-0120](../../../../docs/adr/0120-the-mcp-server-a-private-yuna-on-stdio.md)).
//! Each seat's log numbers its own events from 1, and never reuses a number while the seat lives
//! ([ADR-0153](../../../../docs/adr/0153-one-event-stream-marks-at-the-change-mcps-log-per-seat-the-bound.md)):
//! an agent keeps one cursor for each seat. Past its bound a seat's log drops its oldest event
//! and counts it; its numbers having no gap, the count of those dropped after a cursor, `missed`,
//! is exact. The bound is the resync ADR-0082 asks for: a reader told it missed events reads
//! `windows` again.
//!
//! The server records the compositor's events once a round, and before it answers an `events`
//! call (`Server::record_events`); the compositor builds them from what changed alone, so that a
//! round in which clients only drew builds nothing.

use std::collections::{HashMap, VecDeque};

use serde::Deserialize;
use yuna_types::SeatId;

use super::programs::End;

/// How many events a seat's log keeps: its newest. Past it, the oldest is dropped and counted.
pub const MAX_EVENTS: usize = 1024;

/// How many events one reply of `events` gives at most: its `next` reads on from the last.
pub const MAX_EVENTS_PER_REPLY: usize = 256;

/// An event's kind, as `events` names it and filters on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    WindowOpened,
    WindowClosed,
    WindowChanged,
    WindowPlaced,
    WindowUnresponsive,
    Focus,
    ProgramExited,
    ClientLeft,
    ClientRefused,
    OutputAdded,
    OutputRemoved,
    OutputChanged,
}

impl Kind {
    /// Every kind, in the order the tool's description gives them.
    pub const ALL: [Kind; 12] = [
        Kind::WindowOpened,
        Kind::WindowClosed,
        Kind::WindowChanged,
        Kind::WindowPlaced,
        Kind::WindowUnresponsive,
        Kind::Focus,
        Kind::ProgramExited,
        Kind::ClientLeft,
        Kind::ClientRefused,
        Kind::OutputAdded,
        Kind::OutputRemoved,
        Kind::OutputChanged,
    ];

    /// Its name, in snake case, as serde reads it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::WindowOpened => "window_opened",
            Kind::WindowClosed => "window_closed",
            Kind::WindowChanged => "window_changed",
            Kind::WindowPlaced => "window_placed",
            Kind::WindowUnresponsive => "window_unresponsive",
            Kind::Focus => "focus",
            Kind::ProgramExited => "program_exited",
            Kind::ClientLeft => "client_left",
            Kind::ClientRefused => "client_refused",
            Kind::OutputAdded => "output_added",
            Kind::OutputRemoved => "output_removed",
            Kind::OutputChanged => "output_changed",
        }
    }

    /// The kind of a compositor's event; none for a kind this server does not know, which
    /// `yuna_ipc::Event`, `#[non_exhaustive]`, may add: such an event is not told.
    pub fn of(event: &yuna_ipc::Event) -> Option<Kind> {
        use yuna_ipc::Event;
        Some(match event {
            Event::WindowOpened { .. } => Kind::WindowOpened,
            Event::WindowChanged { .. } => Kind::WindowChanged,
            Event::WindowPlaced { .. } => Kind::WindowPlaced,
            Event::WindowUnresponsive { .. } => Kind::WindowUnresponsive,
            Event::WindowClosed { .. } => Kind::WindowClosed,
            Event::Focus { .. } => Kind::Focus,
            Event::OutputAdded { .. } => Kind::OutputAdded,
            Event::OutputChanged { .. } => Kind::OutputChanged,
            Event::OutputRemoved { .. } => Kind::OutputRemoved,
            Event::ClientLeft { .. } => Kind::ClientLeft,
            Event::ClientRefused { .. } => Kind::ClientRefused,
            _ => return None,
        })
    }
}

/// What happened, as a seat's log keeps it.
#[derive(Debug, Clone, PartialEq)]
pub enum EventData {
    /// What the compositor told; for a `ClientLeft`, with the program its client belonged to,
    /// as the server resolved it when Yuna accepted the client (`Server::resolve_arrivals`):
    /// none for a client of no program, and for every other kind.
    Compositor {
        event: yuna_ipc::Event,
        program: Option<u64>,
    },
    /// One of the seat's programs exited.
    ProgramExited { program: u64, end: End },
}

/// An event of a seat's log.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Its number in its seat's log, from 1, never reused while the seat lives.
    pub seq: u64,
    /// When it was recorded, on the clock's scale, in nanoseconds.
    pub time_ns: u64,
    /// Its kind, read when it was recorded.
    pub kind: Kind,
    pub data: EventData,
}

/// A seat's events: the newest kept, the older dropped and counted.
#[derive(Debug, Default)]
struct SeatEvents {
    /// Its newest event's number, 0 before its first.
    last_seq: u64,
    kept: VecDeque<Event>,
    /// How many of its oldest events were dropped: those numbered 1 to `dropped`.
    dropped: u64,
}

impl SeatEvents {
    /// How many events with a number above `after` were dropped: exact, since the numbers
    /// dropped are the first ones, with no gap.
    fn missed(&self, after: u64) -> u64 {
        self.dropped.saturating_sub(after)
    }
}

/// What a read of a seat's log gives.
#[derive(Debug, PartialEq)]
pub struct Reading<'a> {
    /// The events after the cursor that match, oldest first, at most
    /// [`MAX_EVENTS_PER_REPLY`].
    pub events: Vec<&'a Event>,
    /// The cursor to give next: the last event given when more wait, else the seat's newest
    /// number.
    pub next: u64,
    /// How many events of the seat after the cursor the log dropped, of any kind.
    pub missed: u64,
}

/// The agents' seats' events.
#[derive(Debug, Default)]
pub struct EventLog {
    seats: HashMap<SeatId, SeatEvents>,
}

impl EventLog {
    /// `seat`'s newest event's number, 0 before its first and for a seat without a log.
    pub fn last_seq(&self, seat: SeatId) -> u64 {
        self.seats.get(&seat).map_or(0, |log| log.last_seq)
    }

    /// Gives `seat` a log, if it has none.
    pub fn add_seat(&mut self, seat: SeatId) {
        self.seats.entry(seat).or_default();
    }

    /// Forgets `seat`'s log: its seat closed.
    pub fn remove_seat(&mut self, seat: SeatId) {
        self.seats.remove(&seat);
    }

    /// Whether `seat` has a log.
    pub fn has_seat(&self, seat: SeatId) -> bool {
        self.seats.contains_key(&seat)
    }

    /// Records `data`, of `kind`, on `seat`'s log at `time_ns`, with the seat's next number,
    /// dropping its oldest past [`MAX_EVENTS`]; nothing for a seat without a log.
    pub fn record(&mut self, seat: SeatId, time_ns: u64, kind: Kind, data: EventData) {
        let Some(log) = self.seats.get_mut(&seat) else {
            return;
        };
        log.last_seq += 1;
        log.kept.push_back(Event {
            seq: log.last_seq,
            time_ns,
            kind,
            data,
        });
        if log.kept.len() > MAX_EVENTS && log.kept.pop_front().is_some() {
            log.dropped += 1;
        }
    }

    /// `seat`'s events numbered above `after` whose kind is among `kinds`, any kind without.
    pub fn read(&self, seat: SeatId, after: u64, kinds: Option<&[Kind]>) -> Reading<'_> {
        let Some(log) = self.seats.get(&seat) else {
            return Reading {
                events: Vec::new(),
                next: 0,
                missed: 0,
            };
        };
        let start = log.kept.partition_point(|event| event.seq <= after);
        let mut matching = log
            .kept
            .range(start..)
            .filter(|event| kinds.is_none_or(|kinds| kinds.contains(&event.kind)));
        let events: Vec<&Event> = matching.by_ref().take(MAX_EVENTS_PER_REPLY).collect();
        let next = match (matching.next(), events.last()) {
            (Some(_), Some(last)) => last.seq,
            _ => log.last_seq,
        };
        Reading {
            events,
            next,
            missed: log.missed(after),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yuna_types::WindowId;

    const SEAT: SeatId = SeatId(2);

    fn focus_on(seat: SeatId, window: u64) -> EventData {
        EventData::Compositor {
            event: yuna_ipc::Event::Focus {
                seat,
                window: Some(WindowId(window)),
            },
            program: None,
        }
    }

    fn record_focus(log: &mut EventLog, seat: SeatId, n: u64) {
        log.record(seat, n, Kind::Focus, focus_on(seat, n));
    }

    #[test]
    fn a_seats_log_keeps_its_newest_and_counts_what_it_dropped() {
        let mut log = EventLog::default();
        log.add_seat(SEAT);
        record_focus(&mut log, SeatId(9), 0);
        assert_eq!(
            log.last_seq(SeatId(9)),
            0,
            "a seat without a log records nothing"
        );
        for n in 0..(MAX_EVENTS as u64 + 10) {
            record_focus(&mut log, SEAT, n);
        }
        let newest = log.last_seq(SEAT);
        assert_eq!(newest, MAX_EVENTS as u64 + 10);
        let read = log.read(SEAT, 0, None);
        assert_eq!(read.missed, 10);
        assert_eq!(read.events.len(), MAX_EVENTS_PER_REPLY);
        assert_eq!(read.events[0].seq, 11);
        // More wait: the cursor is the last event given.
        assert_eq!(read.next, 11 + MAX_EVENTS_PER_REPLY as u64 - 1);
        assert_eq!(log.read(SEAT, 5, None).missed, 5);
        assert_eq!(log.read(SEAT, 10, None).missed, 0);
        let last = log.read(SEAT, newest - 1, None);
        assert_eq!(last.events.len(), 1);
        assert_eq!(last.next, newest);
        let filtered = log.read(SEAT, 0, Some(&[Kind::WindowClosed]));
        assert_eq!((filtered.events.len(), filtered.next), (0, newest));
    }

    /// Two seats' events interleaved, each seat past [`MAX_EVENTS`] and then past
    /// [`MAX_EVENTS`] times more: each numbers its own from 1, and a cursor anywhere in the part
    /// its log dropped is told exactly how many it missed.
    #[test]
    fn interleaved_seats_number_their_own_and_miss_exactly() {
        let mut log = EventLog::default();
        let other = SeatId(3);
        log.add_seat(SEAT);
        log.add_seat(other);
        // 1,124 events each, alternating.
        for n in 0..(2 * MAX_EVENTS as u64 + 200) {
            let seat = if n % 2 == 0 { SEAT } else { other };
            record_focus(&mut log, seat, n);
        }
        assert_eq!(log.last_seq(SEAT), MAX_EVENTS as u64 + 100);
        assert_eq!(log.last_seq(other), MAX_EVENTS as u64 + 100);
        // Each dropped its first 100.
        assert_eq!(log.read(SEAT, 0, None).missed, 100);
        assert_eq!(log.read(SEAT, 37, None).missed, 63);
        assert_eq!(log.read(SEAT, 99, None).missed, 1);
        assert_eq!(log.read(SEAT, 100, None).missed, 0);
        assert_eq!(log.read(other, 50, None).missed, 50);
        let read = log.read(SEAT, 37, None);
        assert_eq!(
            read.events[0].seq, 101,
            "the oldest kept follows the dropped"
        );
        assert!(
            read.events.iter().all(|event| matches!(
                &event.data,
                EventData::Compositor { event, .. } if event.seat() == SEAT
            )),
            "a seat's log holds its own events alone"
        );
        // Far past the bound, more than MAX_EVENTS times: still exact, at any cursor.
        for n in 0..(4 * MAX_EVENTS as u64 * MAX_EVENTS as u64 / 256) {
            let seat = if n % 3 == 0 { other } else { SEAT };
            record_focus(&mut log, seat, n);
        }
        let newest = log.last_seq(SEAT);
        let dropped = newest - MAX_EVENTS as u64;
        assert_eq!(log.read(SEAT, 0, None).missed, dropped);
        assert_eq!(log.read(SEAT, 1, None).missed, dropped - 1);
        assert_eq!(
            log.read(SEAT, dropped / 2, None).missed,
            dropped - dropped / 2
        );
        assert_eq!(log.read(SEAT, dropped, None).missed, 0);
        assert_eq!(log.read(SEAT, dropped, None).events[0].seq, dropped + 1);
        let other_newest = log.last_seq(other);
        assert_eq!(
            log.read(other, 0, None).missed,
            other_newest - MAX_EVENTS as u64
        );
        log.remove_seat(SEAT);
        assert!(!log.has_seat(SEAT));
        assert_eq!(log.read(SEAT, 0, None).events, Vec::<&Event>::new());
        assert_eq!(log.last_seq(SEAT), 0);
    }

    /// Each of the compositor's kinds has MCP's: the list names every kind
    /// `yuna_ipc::Event` has, eleven, MCP's own `program_exited` apart.
    #[test]
    fn every_kind_of_the_compositors_is_known() {
        use yuna_ipc::Event;
        let seat = SEAT;
        let window = WindowId(1);
        let output = yuna_types::OutputId(1);
        let told = [
            (
                Event::WindowOpened {
                    seat,
                    window,
                    app_id: None,
                    title: None,
                    pid: None,
                    shell: yuna_types::WindowShell::Xdg,
                },
                Kind::WindowOpened,
            ),
            (
                Event::WindowChanged {
                    seat,
                    window,
                    title: None,
                    app_id: None,
                },
                Kind::WindowChanged,
            ),
            (
                Event::WindowPlaced {
                    seat,
                    window,
                    output: None,
                    workspace: None,
                    place: None,
                },
                Kind::WindowPlaced,
            ),
            (
                Event::WindowUnresponsive { seat, window },
                Kind::WindowUnresponsive,
            ),
            (Event::WindowClosed { seat, window }, Kind::WindowClosed),
            (Event::Focus { seat, window: None }, Kind::Focus),
            (Event::OutputAdded { seat, output }, Kind::OutputAdded),
            (Event::OutputChanged { seat, output }, Kind::OutputChanged),
            (Event::OutputRemoved { seat, output }, Kind::OutputRemoved),
            (
                Event::ClientLeft {
                    seat,
                    client: 1,
                    pid: None,
                    error: None,
                },
                Kind::ClientLeft,
            ),
            (
                Event::ClientRefused {
                    seat,
                    bound: 1,
                    refused: 1,
                },
                Kind::ClientRefused,
            ),
        ];
        assert_eq!(
            told.len() + 1,
            Kind::ALL.len(),
            "program_exited is MCP's own"
        );
        for (event, kind) in told {
            assert_eq!(Kind::of(&event), Some(kind));
        }
    }
}
