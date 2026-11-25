//! The frame record
//! ([ADR-0158](../../../../docs/adr/0158-a-frames-identity-each-task-wakes-yuna-through-a-source-of-its-own.md)):
//! each seat's last inputs, commits and frames, numbered, so that an agent reads which frame
//! showed what it did, and when that frame was presented.
//!
//! **A frame's identity.** An output's refresh takes a [`FrameId`] when it is first armed, and
//! keeps it when it is aimed again, sooner under VRR or for new content: the boundary aimed at,
//! the time sampled and the boundary presented are the frame's fields, not its identity. The id
//! is released when the refresh runs, which records the frame, or when it is disarmed before it
//! runs (its output removed or given another mode), which records nothing.
//!
//! **Three rings a seat**, each of a fixed size made with the seat, written in place
//! ([`Ring`]): no allocation once the seat is made. Each numbers its entries from 1 and counts
//! those it dropped, so that a reader told how many it missed after a cursor is told exactly.
//!
//! - **Inputs**: each event Yuna delivers on the seat, pointer motion, button, axis, key or text,
//!   with its time and its device: the compositor's interface (tests, WLCS), an agent's MCP call
//!   by its number, or a client's virtual keyboard or pointer.
//! - **Commits**: each `wl_surface.commit` of the seat's clients, with the client, the surface,
//!   its window and the output it was shown on, if any, and whether it brought something new.
//! - **Frames**: each refresh that ran, with its id, its output, its outcome, its causes, the
//!   inputs and commits it covers, when it was armed, ran and ended, the boundary it aimed at and
//!   the one it was presented at.
//!
//! **What a frame covers** is told in order, not as causality: the seat's inputs and commits
//! recorded after the output's previous frame, up to its refresh. Which input made a client
//! commit is not knowable: Wayland carries no input's identity to the commit that follows it.
//!
//! Plain numbers only: this module names no Wayland or Smithay type.

mod ring;

use std::time::Duration;

use yuna_types::{OutputId, SeatId, WindowId};

use crate::rest::Causes;
pub(crate) use ring::Ring;

/// How many inputs a seat keeps. A pointer's motions between two frames merge into one entry, so
/// that a fast pointer takes one a frame: eight seconds of it at 240 Hz. A long `keyboard type`
/// may pass it, its oldest keys dropped and counted.
pub const MAX_INPUTS: usize = 2048;

/// How many commits a seat keeps: two clients drawing at 240 Hz for four seconds.
pub const MAX_COMMITS: usize = 2048;

/// How many frames a seat keeps: four seconds of one output at 240 Hz.
pub const MAX_FRAMES: usize = 1024;

// What a seat's rings take at most, made when the seat is and resident as they fill: 464 KiB
// on x86_64 and aarch64. An entry grown is a choice, made here.
const _: () = assert!(size_of::<Input>() <= 56 && size_of::<Commit>() <= 72);
const _: () = assert!(size_of::<Frame>() <= 208);
const _: () = assert!(
    MAX_INPUTS * size_of::<Input>()
        + MAX_COMMITS * size_of::<Commit>()
        + MAX_FRAMES * size_of::<Frame>()
        <= 464 * 1024
);

/// A frame's identity: an opaque number, taken when its output's refresh is first armed and
/// never reused while Yuna runs. Its value says nothing of the output, the boundary or the
/// order in which frames were drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameId(u64);

impl FrameId {
    /// The number that names it, as MCP gives it.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// What kind of input an event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// The pointer moved.
    Motion,
    /// A pointer button was pressed or released.
    Button,
    /// The pointer scrolled.
    Axis,
    /// A key was pressed or released.
    Key,
    /// Text was given to a text field, committed or as a preedit.
    Text,
}

impl InputKind {
    /// Its name, as MCP gives it.
    pub fn name(self) -> &'static str {
        match self {
            InputKind::Motion => "motion",
            InputKind::Button => "button",
            InputKind::Axis => "axis",
            InputKind::Key => "key",
            InputKind::Text => "text",
        }
    }
}

/// Where an input came from. Every delivery names it: the compiler asks each caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputDevice {
    /// The compositor's interface, called by a test or WLCS's shim.
    Interface,
    /// Yuna itself: a pointer put on what is under it after windows moved, a drag's motion
    /// repeated, what a seat held released when it closes.
    Yuna,
    /// An agent's MCP call, by the JSON-RPC id of its request when that id is an integer.
    Agent {
        /// The request's id; none for a string.
        request: Option<i64>,
    },
    /// A client's virtual keyboard.
    VirtualKeyboard {
        /// The client's number in the rest auditor, as `rest` names it.
        client: u64,
    },
    /// A client's virtual pointer.
    VirtualPointer {
        /// The client's number in the rest auditor, as `rest` names it.
        client: u64,
    },
}

/// An input delivered on a seat: one event, or a run of motions of one device that no frame
/// covered between them, merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Input {
    /// When it was delivered, on the clock's scale: the time the event carries; a run's first.
    pub time: Duration,
    /// A run's last; its time for one event.
    pub last: Duration,
    /// How many events it holds: more than one only for a run of motions.
    pub count: u32,
    /// What it is.
    pub kind: InputKind,
    /// Where it came from.
    pub device: InputDevice,
}

/// A commit of one of a seat's clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Commit {
    /// When it was handled, on the clock's scale.
    pub time: Duration,
    /// Its client's number in the rest auditor; none for a client Yuna does not know.
    pub client: Option<u64>,
    /// The surface it committed, by its protocol id, its client's own.
    pub surface: u32,
    /// The window the surface belongs to, if it is a window's and that window is open.
    pub window: Option<WindowId>,
    /// The output that showed that window when it committed; none when no output did.
    pub output: Option<OutputId>,
    /// Whether it attached or removed a buffer: only then is it a cause of the frame that shows
    /// it.
    pub new_content: bool,
}

/// What a refresh's drawing did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    /// It drew.
    Drawn {
        /// The rectangles it found damaged.
        damaged: u32,
    },
    /// Nothing on the output was damaged.
    Unchanged,
    /// The renderer failed.
    Failed,
}

/// A refresh boundary: its number, as presentation feedback carries it, and its time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boundary {
    /// Its number, counted from the output's creation.
    pub seq: u64,
    /// Its time, on the clock's scale.
    pub at: Duration,
}

/// Entries of a ring numbered `first` to `last`, both included: none when `first > last`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Numbers {
    /// The first number.
    pub first: u64,
    /// The last number.
    pub last: u64,
}

impl Numbers {
    /// How many numbers it holds.
    pub fn count(self) -> u64 {
        (self.last + 1).saturating_sub(self.first)
    }
}

/// A refresh that ran, on one output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// Its identity.
    pub id: FrameId,
    /// The output it was drawn for.
    pub output: OutputId,
    /// What its drawing did.
    pub outcome: FrameOutcome,
    /// What armed the refresh or joined it ([`rest::Cause`](crate::rest::Cause)).
    pub causes: Causes,
    /// The seat's inputs recorded after the output's previous frame, up to this one's refresh.
    pub inputs: Numbers,
    /// The seat's commits recorded after the output's previous frame, up to this one's refresh.
    pub commits: Numbers,
    /// When its refresh was first armed, on the clock's scale.
    pub armed_at: Duration,
    /// When its refresh was due, as last armed.
    pub due_at: Duration,
    /// When its refresh ran.
    pub ran_at: Duration,
    /// When its drawing ended, on the clock's scale: a draw hook may move a manual clock.
    pub ended_at: Duration,
    /// How long its drawing took on real time, from building its elements; zero when nothing
    /// was drawn.
    pub took: Duration,
    /// The time its motion was sampled at.
    pub sampled_at: Duration,
    /// The boundary its refresh aimed at.
    pub aimed: Boundary,
    /// The boundary it is presented at; none when its drawing failed, or when its output was
    /// removed before then. One past `aimed` is a late frame.
    pub presented: Option<Boundary>,
    /// Whether its output was removed before its boundary: its presentation feedback was
    /// discarded.
    pub discarded: bool,
}

impl Frame {
    /// Whether it was presented after the boundary it aimed at.
    pub fn late(&self) -> bool {
        self.presented
            .is_some_and(|presented| presented.seq > self.aimed.seq)
    }
}

/// One seat's rings.
#[derive(Debug)]
struct SeatRecord {
    inputs: Ring<Input>,
    commits: Ring<Commit>,
    frames: Ring<Frame>,
    /// The newest input any of its frames covers: a motion after it starts a new entry.
    covered_input: u64,
}

impl SeatRecord {
    fn new() -> SeatRecord {
        SeatRecord {
            inputs: Ring::new(MAX_INPUTS),
            commits: Ring::new(MAX_COMMITS),
            frames: Ring::new(MAX_FRAMES),
            covered_input: 0,
        }
    }
}

/// An output's refresh while it is armed: its id, and when it was first armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArmedFrame {
    pub id: FrameId,
    /// When the refresh was first armed, on the clock's scale.
    pub armed_at: Duration,
}

/// Every seat's rings, and the frames' ids. The compositor's state owns it.
#[derive(Debug, Default)]
pub(crate) struct FrameRecord {
    /// By seat: a handful, so a scan is cheaper than a hash on the frame path.
    seats: Vec<(SeatId, SeatRecord)>,
    /// The last frame id taken.
    last_frame: u64,
}

impl FrameRecord {
    /// Gives `seat` its rings, if it has none.
    pub(crate) fn add_seat(&mut self, seat: SeatId) {
        if self.seat(seat).is_none() {
            self.seats.push((seat, SeatRecord::new()));
        }
    }

    /// Forgets `seat`'s rings: the seat was removed.
    pub(crate) fn remove_seat(&mut self, seat: SeatId) {
        self.seats.retain(|(id, _)| *id != seat);
    }

    fn seat(&self, seat: SeatId) -> Option<&SeatRecord> {
        self.seats
            .iter()
            .find(|(id, _)| *id == seat)
            .map(|(_, record)| record)
    }

    fn seat_mut(&mut self, seat: SeatId) -> Option<&mut SeatRecord> {
        self.seats
            .iter_mut()
            .find(|(id, _)| *id == seat)
            .map(|(_, record)| record)
    }

    /// A new frame's identity, its refresh first armed at `now`.
    pub(crate) fn arm_frame(&mut self, now: Duration) -> ArmedFrame {
        self.last_frame += 1;
        ArmedFrame {
            id: FrameId(self.last_frame),
            armed_at: now,
        }
    }

    /// Records an input of `kind` from `device`, delivered at `time`, on `seat`; nothing for a
    /// seat without rings. A motion that follows a motion of the same device, no frame having
    /// covered it, joins its entry, so that a fast pointer fills no ring between two frames.
    pub(crate) fn input(
        &mut self,
        seat: SeatId,
        kind: InputKind,
        device: InputDevice,
        time: Duration,
    ) {
        let Some(record) = self.seat_mut(seat) else {
            return;
        };
        let uncovered = record.inputs.last() > record.covered_input;
        if kind == InputKind::Motion
            && uncovered
            && let Some(newest) = record.inputs.newest_mut()
            && newest.kind == InputKind::Motion
            && newest.device == device
        {
            newest.last = time;
            newest.count = newest.count.saturating_add(1);
            return;
        }
        record.inputs.push(Input {
            time,
            last: time,
            count: 1,
            kind,
            device,
        });
    }

    /// Records `commit` on `seat`; nothing for a seat without rings.
    pub(crate) fn commit(&mut self, seat: SeatId, commit: Commit) {
        if let Some(record) = self.seat_mut(seat) {
            record.commits.push(commit);
        }
    }

    /// The numbers of `seat`'s newest input and commit: where an output's next frame's coverage
    /// begins after.
    pub(crate) fn newest(&self, seat: SeatId) -> (u64, u64) {
        self.seat(seat).map_or((0, 0), |record| {
            (record.inputs.last(), record.commits.last())
        })
    }

    /// Records `frame` on `seat`; nothing for a seat without rings.
    pub(crate) fn frame(&mut self, seat: SeatId, frame: Frame) {
        if let Some(record) = self.seat_mut(seat) {
            record.covered_input = record.covered_input.max(frame.inputs.last);
            record.frames.push(frame);
        }
    }

    /// `seat`'s frames numbered above `after`, of `output` alone when given, oldest first, at
    /// most `limit`; none for a seat without rings.
    pub(crate) fn read(
        &self,
        seat: SeatId,
        output: Option<OutputId>,
        after: u64,
        limit: usize,
    ) -> Option<Reading<'_>> {
        let record = self.seat(seat)?;
        let mut matching = record
            .frames
            .after(after)
            .filter(|(_, frame)| output.is_none_or(|output| frame.output == output));
        let frames: Vec<(u64, &Frame)> = matching.by_ref().take(limit).collect();
        let next = match (matching.next(), frames.last()) {
            (Some(_), Some((last, _))) => *last,
            _ => record.frames.last(),
        };
        Some(Reading {
            record,
            frames,
            next,
            missed: record.frames.missed(after),
        })
    }

    /// `output`, of `seat`, was removed at `now`: its frames not yet presented never will be,
    /// their feedback discarded. Those are its last few: a frame is presented within a period or
    /// two of its refresh, so the walk stops at the first frame of the seat older than a second.
    pub(crate) fn output_removed(&mut self, seat: SeatId, output: OutputId, now: Duration) {
        let Some(record) = self.seat_mut(seat) else {
            return;
        };
        for frame in record.frames.newest_first_mut() {
            if frame.ran_at.saturating_add(Duration::from_secs(1)) < now {
                break;
            }
            if frame.output == output
                && let Some(presented) = frame.presented
                && presented.at > now
            {
                frame.presented = None;
                frame.discarded = true;
            }
        }
    }

    /// The number of `seat`'s newest frame; 0 before its first and for a seat without rings.
    pub(crate) fn last_frame(&self, seat: SeatId) -> u64 {
        self.seat(seat).map_or(0, |record| record.frames.last())
    }
}

/// What a read of a seat's frames gives.
#[derive(Debug)]
pub struct Reading<'a> {
    record: &'a SeatRecord,
    /// The frames after the cursor that match, oldest first, each with its number.
    pub frames: Vec<(u64, &'a Frame)>,
    /// The cursor to give next: the last frame given when more wait, else the seat's newest.
    pub next: u64,
    /// How many frames of the seat after the cursor its ring dropped, of any output.
    pub missed: u64,
}

impl<'a> Reading<'a> {
    /// The inputs `frame` covers that the seat's ring still keeps, oldest first, with their
    /// numbers.
    pub fn inputs(&self, frame: &Frame) -> impl Iterator<Item = (u64, &'a Input)> + use<'a> {
        self.record
            .inputs
            .range(frame.inputs.first, frame.inputs.last)
    }

    /// The commits `frame` covers that the seat's ring still keeps, oldest first, with their
    /// numbers.
    pub fn commits(&self, frame: &Frame) -> impl Iterator<Item = (u64, &'a Commit)> + use<'a> {
        self.record
            .commits
            .range(frame.commits.first, frame.commits.last)
    }
}
