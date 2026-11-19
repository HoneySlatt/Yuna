//! What happens on agents' seats, as the `events` tool tells it: each seat's log of its last
//! [`MAX_EVENTS`] events ([`EventLog`]), and how window and output events are told from what the
//! compositor describes ([`Observer`]).
//!
//! The server keeps no state of a request: the cursor is the agent's, the `seq` of an event or a
//! reply's `next` ([ADR-0120](../../../../docs/adr/0120-the-mcp-server-a-private-yuna-on-stdio.md)). An event's `seq`
//! grows across the whole Yuna and is never reused, so that one cursor reads any seat; between
//! two events of one seat, other seats' take numbers too. Past its bound a seat's log drops its
//! oldest event and counts it, so that a cursor older than the log learns how many it missed.
//!
//! The observer compares what the compositor describes with what it saw last: a window's title
//! changed a thousand times between two looks is one `window_changed`, and its place is told once
//! the layout is arranged, never while an arrangement is pending. The server looks once a round,
//! and before it answers an `events` call, but only when the compositor's description
//! generation has moved since the last look (`Server::record_events`): a round in which clients
//! only drew costs no look. A window that opens and closes between two looks is not seen.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde::Deserialize;
use yuna_layout::geometry::Rect;
use yuna_layout::{OutputId, SeatId, WindowId};

use super::programs::End;
use crate::compositor::{ClientProtocolError, OutputDescription, WindowDescription, WindowShell};

/// How many events a seat's log keeps: its newest. Past it, the oldest is dropped and counted.
pub const MAX_EVENTS: usize = 1024;

/// How many events one reply of `events` gives at most: its `next` reads on from the last.
pub const MAX_EVENTS_PER_REPLY: usize = 256;

/// The longest title, application id or workspace name an event gives, in bytes: a longer one
/// is cut at a character's boundary, so that an event holds at most 1 KiB of text, and a seat's
/// log about 1 MiB. A change past the cut is not told.
pub const MAX_EVENT_TEXT: usize = 512;

/// `text`'s first [`MAX_EVENT_TEXT`] bytes at most, cut at a character's boundary.
fn cut(text: &str) -> &str {
    let mut end = text.len().min(MAX_EVENT_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `text` as an event gives it, [`cut`].
fn bounded(text: Option<&str>) -> Option<String> {
    text.map(|text| cut(text).to_owned())
}

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
    OutputAdded,
    OutputRemoved,
    OutputChanged,
}

impl Kind {
    /// Every kind, in the order the tool's description gives them.
    pub const ALL: [Kind; 11] = [
        Kind::WindowOpened,
        Kind::WindowClosed,
        Kind::WindowChanged,
        Kind::WindowPlaced,
        Kind::WindowUnresponsive,
        Kind::Focus,
        Kind::ProgramExited,
        Kind::ClientLeft,
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
            Kind::OutputAdded => "output_added",
            Kind::OutputRemoved => "output_removed",
            Kind::OutputChanged => "output_changed",
        }
    }
}

/// What happened.
#[derive(Debug, Clone, PartialEq)]
pub enum EventData {
    WindowOpened {
        window: WindowId,
        app_id: Option<String>,
        title: Option<String>,
        pid: Option<i32>,
        shell: WindowShell,
    },
    WindowClosed {
        window: WindowId,
    },
    /// Its title or its application id changed.
    WindowChanged {
        window: WindowId,
        title: Option<String>,
        app_id: Option<String>,
    },
    /// Its output, its workspace or its place in the layout changed.
    WindowPlaced {
        window: WindowId,
        output: Option<OutputId>,
        workspace: Option<String>,
        place: Option<Rect>,
    },
    /// It left a ping unanswered.
    WindowUnresponsive {
        window: WindowId,
    },
    /// The seat's focus moved to this window, or to none.
    Focus {
        window: Option<WindowId>,
    },
    ProgramExited {
        program: u64,
        end: End,
    },
    /// A Wayland connection of the seat ended: closed, or for a protocol error.
    ClientLeft {
        pid: Option<i32>,
        program: Option<u64>,
        error: Option<ClientProtocolError>,
    },
    OutputAdded {
        output: OutputId,
    },
    OutputRemoved {
        output: OutputId,
    },
    /// Its mode or its place changed.
    OutputChanged {
        output: OutputId,
    },
}

impl EventData {
    pub fn kind(&self) -> Kind {
        match self {
            EventData::WindowOpened { .. } => Kind::WindowOpened,
            EventData::WindowClosed { .. } => Kind::WindowClosed,
            EventData::WindowChanged { .. } => Kind::WindowChanged,
            EventData::WindowPlaced { .. } => Kind::WindowPlaced,
            EventData::WindowUnresponsive { .. } => Kind::WindowUnresponsive,
            EventData::Focus { .. } => Kind::Focus,
            EventData::ProgramExited { .. } => Kind::ProgramExited,
            EventData::ClientLeft { .. } => Kind::ClientLeft,
            EventData::OutputAdded { .. } => Kind::OutputAdded,
            EventData::OutputRemoved { .. } => Kind::OutputRemoved,
            EventData::OutputChanged { .. } => Kind::OutputChanged,
        }
    }
}

/// An event of a seat's log.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Its number, from 1, growing across the whole Yuna, never reused.
    pub seq: u64,
    /// When it was recorded, on the clock's scale, in nanoseconds.
    pub time_ns: u64,
    pub data: EventData,
}

/// Consecutive numbers a seat's log dropped. Its `count` is `last - first + 1` while nothing
/// else took a number between; a run merged with an older one past [`MAX_EVENTS`] runs counts
/// fewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dropped {
    first: u64,
    last: u64,
    count: u64,
}

/// A seat's events: the newest kept, the older dropped and counted.
#[derive(Debug, Default)]
struct SeatEvents {
    kept: VecDeque<Event>,
    /// The runs of numbers dropped, oldest first, at most [`MAX_EVENTS`].
    dropped: VecDeque<Dropped>,
}

impl SeatEvents {
    fn note_dropped(&mut self, seq: u64) {
        match self.dropped.back_mut() {
            Some(run) if run.last + 1 == seq => {
                run.last = seq;
                run.count += 1;
            }
            _ => self.dropped.push_back(Dropped {
                first: seq,
                last: seq,
                count: 1,
            }),
        }
        if self.dropped.len() > MAX_EVENTS
            && let Some(oldest) = self.dropped.pop_front()
            && let Some(next) = self.dropped.front_mut()
        {
            next.first = oldest.first;
            next.count += oldest.count;
        }
    }

    /// How many events with a number above `after` were dropped: exact, but for a cursor inside
    /// a merged run, where it is at most that run's count.
    fn missed(&self, after: u64) -> u64 {
        self.dropped
            .iter()
            .map(|run| {
                if run.first > after {
                    run.count
                } else if run.last > after {
                    run.count.min(run.last - after)
                } else {
                    0
                }
            })
            .sum()
    }
}

/// What a read of a seat's log gives.
#[derive(Debug, PartialEq)]
pub struct Reading<'a> {
    /// The events after the cursor that match, oldest first, at most
    /// [`MAX_EVENTS_PER_REPLY`].
    pub events: Vec<&'a Event>,
    /// The cursor to give next: the last event given when more wait, else the newest number
    /// of the whole Yuna.
    pub next: u64,
    /// How many events of the seat after the cursor the log dropped, of any kind.
    pub missed: u64,
}

/// The agents' seats' events.
#[derive(Debug, Default)]
pub struct EventLog {
    /// The newest event's number, 0 before the first.
    last_seq: u64,
    seats: HashMap<SeatId, SeatEvents>,
}

impl EventLog {
    /// The newest event's number in the whole Yuna, 0 before the first.
    pub fn last_seq(&self) -> u64 {
        self.last_seq
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

    /// Records `data` on `seat`'s log at `time_ns`, dropping its oldest past [`MAX_EVENTS`];
    /// nothing for a seat without a log.
    pub fn record(&mut self, seat: SeatId, time_ns: u64, data: EventData) {
        let Some(events) = self.seats.get_mut(&seat) else {
            return;
        };
        self.last_seq += 1;
        events.kept.push_back(Event {
            seq: self.last_seq,
            time_ns,
            data,
        });
        if events.kept.len() > MAX_EVENTS
            && let Some(oldest) = events.kept.pop_front()
        {
            events.note_dropped(oldest.seq);
        }
    }

    /// `seat`'s events numbered above `after` whose kind is among `kinds`, any kind without.
    pub fn read(&self, seat: SeatId, after: u64, kinds: Option<&[Kind]>) -> Reading<'_> {
        let Some(log) = self.seats.get(&seat) else {
            return Reading {
                events: Vec::new(),
                next: self.last_seq,
                missed: 0,
            };
        };
        let start = log.kept.partition_point(|event| event.seq <= after);
        let mut matching = log
            .kept
            .range(start..)
            .filter(|event| kinds.is_none_or(|kinds| kinds.contains(&event.data.kind())));
        let events: Vec<&Event> = matching.by_ref().take(MAX_EVENTS_PER_REPLY).collect();
        let next = match (matching.next(), events.last()) {
            (Some(_), Some(last)) => last.seq,
            _ => self.last_seq,
        };
        Reading {
            events,
            next,
            missed: log.missed(after),
        }
    }
}

/// Where a window rests, as `window_placed` tells it.
#[derive(Debug, Clone, Default, PartialEq)]
struct Placement {
    output: Option<OutputId>,
    workspace: Option<String>,
    place: Option<Rect>,
}

impl Placement {
    fn of(window: &WindowDescription) -> Placement {
        Placement {
            output: window.output,
            workspace: bounded(window.workspace.as_deref()),
            place: window.place,
        }
    }
}

/// What the observer saw of a window.
#[derive(Debug)]
struct SeenWindow {
    seat: SeatId,
    title: Option<String>,
    app_id: Option<String>,
    unresponsive: bool,
    /// Where it was last told to rest: nowhere until its first `window_placed`.
    told: Placement,
    /// The last look that found it: a window an earlier look found and this one did not has
    /// closed.
    last_look: u64,
}

/// What the compositor described at the last look, to tell what changed since. A look costs
/// in proportion to the windows and outputs it is given, and the windows closed sorted.
#[derive(Debug, Default)]
pub struct Observer {
    windows: HashMap<WindowId, SeenWindow>,
    outputs: BTreeMap<OutputId, OutputDescription>,
    /// The window each seat focused, for the seats that focused one.
    focus: HashMap<SeatId, WindowId>,
    /// How many looks it took.
    looks: u64,
}

impl Observer {
    /// The events between the last look and `windows` and `outputs`, all seats together, each
    /// with its seat, in this order: outputs added, changed and removed; then, window by window,
    /// opened, changed, placed and unresponsive; windows closed, by id; focus. A window's place
    /// is told only when `arranged`: no arrangement of the layout is pending.
    pub fn look(
        &mut self,
        windows: &[WindowDescription],
        outputs: &[OutputDescription],
        arranged: bool,
    ) -> Vec<(SeatId, EventData)> {
        self.looks += 1;
        let mut events = Vec::new();
        self.look_at_outputs(outputs, &mut events);
        for window in windows {
            self.look_at_window(window, arranged, &mut events);
        }
        let look = self.looks;
        let mut closed: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, seen)| seen.last_look != look)
            .map(|(&id, _)| id)
            .collect();
        closed.sort_by_key(|id| id.0);
        for window in closed {
            if let Some(seen) = self.windows.remove(&window) {
                events.push((seen.seat, EventData::WindowClosed { window }));
            }
        }
        let focus: HashMap<SeatId, WindowId> = windows
            .iter()
            .filter(|window| window.focused)
            .map(|window| (window.seat, window.id))
            .collect();
        let mut seats: Vec<SeatId> = focus.keys().chain(self.focus.keys()).copied().collect();
        seats.sort_by_key(|seat| seat.0);
        seats.dedup();
        for seat in seats {
            let now = focus.get(&seat).copied();
            if now != self.focus.get(&seat).copied() {
                events.push((seat, EventData::Focus { window: now }));
            }
        }
        self.focus = focus;
        events
    }

    fn look_at_outputs(
        &mut self,
        outputs: &[OutputDescription],
        events: &mut Vec<(SeatId, EventData)>,
    ) {
        for output in outputs {
            let event = match self.outputs.get(&output.id) {
                None => EventData::OutputAdded { output: output.id },
                Some(seen) if seen != output => EventData::OutputChanged { output: output.id },
                Some(_) => continue,
            };
            events.push((output.seat, event));
            self.outputs.insert(output.id, output.clone());
        }
        let current: HashSet<OutputId> = outputs.iter().map(|output| output.id).collect();
        let removed: Vec<OutputId> = self
            .outputs
            .keys()
            .filter(|id| !current.contains(id))
            .copied()
            .collect();
        for output in removed {
            if let Some(seen) = self.outputs.remove(&output) {
                events.push((seen.seat, EventData::OutputRemoved { output }));
            }
        }
    }

    fn look_at_window(
        &mut self,
        window: &WindowDescription,
        arranged: bool,
        events: &mut Vec<(SeatId, EventData)>,
    ) {
        let seat = window.seat;
        let look = self.looks;
        let seen = self.windows.entry(window.id).or_insert_with(|| {
            let (title, app_id) = (
                bounded(window.title.as_deref()),
                bounded(window.app_id.as_deref()),
            );
            events.push((
                seat,
                EventData::WindowOpened {
                    window: window.id,
                    app_id: app_id.clone(),
                    title: title.clone(),
                    pid: window.pid,
                    shell: window.shell,
                },
            ));
            SeenWindow {
                seat,
                title,
                app_id,
                unresponsive: false,
                told: Placement::default(),
                last_look: look,
            }
        });
        seen.last_look = look;
        // Compared as told, cut, without a copy.
        let told = |seen: &Option<String>, now: &Option<String>| {
            seen.as_deref() == now.as_deref().map(cut)
        };
        if !told(&seen.title, &window.title) || !told(&seen.app_id, &window.app_id) {
            seen.title = bounded(window.title.as_deref());
            seen.app_id = bounded(window.app_id.as_deref());
            events.push((
                seat,
                EventData::WindowChanged {
                    window: window.id,
                    title: seen.title.clone(),
                    app_id: seen.app_id.clone(),
                },
            ));
        }
        let placement = Placement::of(window);
        if arranged && placement != seen.told {
            events.push((
                seat,
                EventData::WindowPlaced {
                    window: window.id,
                    output: placement.output,
                    workspace: placement.workspace.clone(),
                    place: placement.place,
                },
            ));
            seen.told = placement;
        }
        if window.unresponsive && !seen.unresponsive {
            events.push((seat, EventData::WindowUnresponsive { window: window.id }));
        }
        seen.unresponsive = window.unresponsive;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputMode;

    const SEAT: SeatId = SeatId(2);

    fn window(id: u64, title: &str) -> WindowDescription {
        WindowDescription {
            id: WindowId(id),
            shell: WindowShell::Xdg,
            app_id: Some("org.example.App".to_owned()),
            title: Some(title.to_owned()),
            pid: Some(40),
            seat: SEAT,
            output: None,
            workspace: None,
            rect: None,
            place: None,
            motion_ends: None,
            focused: false,
            floating: false,
            fullscreen: false,
            unresponsive: false,
        }
    }

    fn placed(mut window: WindowDescription, x: f64) -> WindowDescription {
        window.output = Some(OutputId(1));
        window.place = Some(Rect {
            x,
            y: 16.0,
            w: 400.0,
            h: 300.0,
        });
        window
    }

    fn output(id: u64, width: u32) -> OutputDescription {
        OutputDescription {
            id: OutputId(id),
            seat: SEAT,
            name: format!("HEADLESS-{id}"),
            mode: OutputMode::new(width, 1080, 60.0, 1.0).expect("a mode"),
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: f64::from(width),
                h: 1080.0,
            },
        }
    }

    fn kinds(events: &[(SeatId, EventData)]) -> Vec<Kind> {
        events.iter().map(|(_, data)| data.kind()).collect()
    }

    #[test]
    fn a_window_is_told_opened_placed_once_arranged_changed_and_closed() {
        let mut observer = Observer::default();
        let opened = observer.look(&[window(1, "a")], &[], false);
        assert_eq!(kinds(&opened), [Kind::WindowOpened]);
        // Placed while an arrangement is pending: told once it is arranged.
        assert_eq!(
            observer.look(&[placed(window(1, "a"), 16.0)], &[], false),
            []
        );
        let arranged = observer.look(&[placed(window(1, "b"), 16.0)], &[], true);
        assert_eq!(kinds(&arranged), [Kind::WindowChanged, Kind::WindowPlaced]);
        assert_eq!(
            observer.look(&[placed(window(1, "b"), 16.0)], &[], true),
            []
        );
        let moved = observer.look(&[placed(window(1, "b"), 500.0)], &[], true);
        assert_eq!(kinds(&moved), [Kind::WindowPlaced]);
        let closed = observer.look(&[], &[], true);
        assert_eq!(
            closed,
            [(
                SEAT,
                EventData::WindowClosed {
                    window: WindowId(1)
                }
            )]
        );
    }

    #[test]
    fn a_long_title_is_cut_at_a_characters_boundary() {
        let mut observer = Observer::default();
        // 900 bytes of a character of 3: 512 falls inside the 171st.
        let long = "€".repeat(300);
        let opened = observer.look(&[window(1, &long)], &[], false);
        let EventData::WindowOpened {
            title: Some(title), ..
        } = &opened[0].1
        else {
            panic!("{opened:?}");
        };
        assert_eq!(title.len(), 510);
        assert!(long.starts_with(title.as_str()));
        // A change past the cut is not told; one before it is, cut.
        assert_eq!(
            observer.look(&[window(1, &format!("{long}!"))], &[], false),
            []
        );
        let changed = observer.look(&[window(1, &format!("!{long}"))], &[], false);
        let EventData::WindowChanged {
            title: Some(title), ..
        } = &changed[0].1
        else {
            panic!("{changed:?}");
        };
        assert!(title.len() <= MAX_EVENT_TEXT && title.starts_with('!'));
    }

    #[test]
    fn focus_and_unresponsiveness_are_told_when_they_change() {
        let mut observer = Observer::default();
        let mut focused = window(1, "a");
        focused.focused = true;
        let events = observer.look(&[focused.clone(), window(2, "b")], &[], false);
        assert_eq!(
            events[2..],
            [(
                SEAT,
                EventData::Focus {
                    window: Some(WindowId(1))
                }
            )]
        );
        focused.unresponsive = true;
        let events = observer.look(&[focused.clone()], &[], false);
        assert_eq!(
            kinds(&events),
            [Kind::WindowUnresponsive, Kind::WindowClosed]
        );
        // Still unresponsive: told once.
        assert_eq!(observer.look(&[focused], &[], false), []);
        let events = observer.look(&[], &[], false);
        assert_eq!(kinds(&events), [Kind::WindowClosed, Kind::Focus]);
        assert_eq!(events[1].1, EventData::Focus { window: None });
    }

    #[test]
    fn outputs_are_told_added_changed_and_removed() {
        let mut observer = Observer::default();
        let added = observer.look(&[], &[output(1, 1920), output(2, 640)], true);
        assert_eq!(kinds(&added), [Kind::OutputAdded, Kind::OutputAdded]);
        let changed = observer.look(&[], &[output(1, 1280)], true);
        assert_eq!(kinds(&changed), [Kind::OutputChanged, Kind::OutputRemoved]);
        assert_eq!(
            changed[1].1,
            EventData::OutputRemoved {
                output: OutputId(2)
            }
        );
    }

    fn focus_on(window: u64) -> EventData {
        EventData::Focus {
            window: Some(WindowId(window)),
        }
    }

    #[test]
    fn a_seats_log_keeps_its_newest_and_counts_what_it_dropped() {
        let mut log = EventLog::default();
        log.add_seat(SEAT);
        log.record(SeatId(9), 0, focus_on(1));
        assert_eq!(log.last_seq(), 0, "a seat without a log records nothing");
        for n in 0..(MAX_EVENTS as u64 + 10) {
            log.record(SEAT, n, focus_on(n));
        }
        let read = log.read(SEAT, 0, None);
        assert_eq!(read.missed, 10);
        assert_eq!(read.events.len(), MAX_EVENTS_PER_REPLY);
        assert_eq!(read.events[0].seq, 11);
        // More wait: the cursor is the last event given.
        assert_eq!(read.next, 11 + MAX_EVENTS_PER_REPLY as u64 - 1);
        assert_eq!(log.read(SEAT, 5, None).missed, 5);
        assert_eq!(log.read(SEAT, 10, None).missed, 0);
        let newest = log.read(SEAT, log.last_seq() - 1, None);
        assert_eq!(newest.events.len(), 1);
        assert_eq!(newest.next, log.last_seq());
        let filtered = log.read(SEAT, 0, Some(&[Kind::WindowClosed]));
        assert_eq!((filtered.events.len(), filtered.next), (0, log.last_seq()));
    }

    #[test]
    fn interleaved_seats_count_only_their_own_drops() {
        let mut log = EventLog::default();
        let other = SeatId(3);
        log.add_seat(SEAT);
        log.add_seat(other);
        // Seat 2 takes the odd numbers, seat 3 the even ones.
        for n in 0..(2 * MAX_EVENTS as u64 + 200) {
            let seat = if n % 2 == 0 { SEAT } else { other };
            log.record(seat, n, focus_on(n));
        }
        // 1,124 events each, 100 dropped each: seat 2's 1, 3, …, 199.
        assert_eq!(log.read(SEAT, 0, None).missed, 100);
        assert_eq!(log.read(SEAT, 100, None).missed, 50);
        assert_eq!(log.read(SEAT, 199, None).missed, 0);
        assert_eq!(log.read(other, 100, None).missed, 50);
        // Past MAX_EVENTS runs of drops, the oldest merge: still counted whole.
        for n in 0..(4 * MAX_EVENTS as u64) {
            let seat = if n % 2 == 0 { SEAT } else { other };
            log.record(seat, n, focus_on(n));
        }
        assert_eq!(log.read(SEAT, 0, None).missed, 100 + 2 * MAX_EVENTS as u64);
        log.remove_seat(SEAT);
        assert!(!log.has_seat(SEAT));
        assert_eq!(log.read(SEAT, 0, None).events, Vec::<&Event>::new());
    }
}
