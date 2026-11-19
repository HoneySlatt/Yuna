//! The rest auditor ([ADR-0062](../../../docs/adr/0062-the-compositors-comfort-and-look.md),
//! point 7): what woke Yuna and what made it draw, named and counted, so that an agent, or a log
//! line, names each program or part of Yuna that broke its rest.
//!
//! **Wakes.** Every source of the event loop is inserted through `insert_source`, with what it
//! is: a `Wake`, one of a closed list of [`Source`]s, with the seat it serves when it serves
//! one. Clippy refuses calloop's own `insert_source` anywhere else (`clippy.toml`). A source
//! counts once a round in which it had something, however many events it gave in that round;
//! with the manual clock, each distinct time at which an advance runs deadlines counts as the
//! clock's timer would have, named by the deadline due first.
//!
//! **Clients.** Every client shares one source, the Wayland display's descriptor. Yuna keeps a
//! second descriptor of each client's socket, made when it connects, in an epoll of the
//! auditor's own; after a wake of the display, before the display reads anything, a wait of no
//! time on that epoll gives the clients that had something to send or hung up. Each is counted,
//! by its process, its seat, and in `yuna mcp` the program it belongs to. A display's wake with
//! no such client, one whose second descriptor could not be made, is counted unnamed.
//!
//! **Frames.** Arming an output's refresh takes its cause (`DrawCause`); each output keeps the
//! causes noted since its last refresh (`PendingCauses`), and a frame drawn is counted once
//! under each: a client's commit, with the client, a layout change, a motion, a capture, the
//! output's first frame, its change of mode, or a popup or an X11 menu that came, went or moved
//! without a commit of its own. A refresh that drew nothing counts nothing.
//!
//! **Bounds.** Counters only: an array per [`Source`] and per [`Cause`], a map of the clients
//! connected and one of the seats that exist, each entry dropped when its client leaves or its
//! seat is removed; a client that leaves is folded into one entry of clients gone, its seat's and
//! Yuna's. Nothing is allocated at a wake but room for an entry in one of these, bounded by the
//! clients connected or the seats that exist: the clients' map, the seats', an output's list of
//! the clients whose commits its next frame shows, and, with the log on, the second's map of its
//! clients. No text is made unless asked: by the `rest` tool, or by the log of each second,
//! `--rest-audit`.
//!
//! **The log of each second**, when asked: the first wake after a quiet second opens a second
//! and arms a timer of 1 s, on real time; at its end, one line at `info` names what woke Yuna in
//! it and what it drew. That timer is the auditor's own (`Wake::of_the_auditor`): its wake is
//! never counted, among Yuna's wakes nor in any count, and opens no second, so that at rest
//! nothing runs, nothing is logged, and a `rest` call's window counts no wake of the log.

use std::collections::{BTreeMap, HashMap};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;

use rustix::event::epoll;
use smithay::reexports::calloop::{EventSource, InsertError, LoopHandle, RegistrationToken};
use yuna_layout::SeatId;

use crate::compositor::ClientProcess;
use crate::state::State;

/// How long a second of the log of each second lasts, on real time.
pub const SECOND: std::time::Duration = std::time::Duration::from_secs(1);

/// What woke Yuna: each kind of source its event loop watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    /// A client sent something or hung up: the Wayland display's descriptor, which every client
    /// shares; [`RestCounts::clients`] names them.
    Clients,
    /// A client connected through a seat's socket.
    SeatSocket,
    /// A seat's socket listens again, after an `accept` that failed paused it.
    SocketPause,
    /// The clock's timer, due for an output's refresh.
    ClockRefresh,
    /// The clock's timer, due for the boundary an output's held presentation feedback waits for.
    ClockPresentation,
    /// The clock's timer, due for the frame callbacks of hidden windows.
    ClockHiddenFrames,
    /// The clock's timer, due when an X11 window's sync request or ping goes unanswered.
    ClockX11Answers,
    /// The clock's timer, due when an X11 program's drag released goes without its source's
    /// drop or leave.
    ClockDrags,
    /// SIGTERM or SIGINT, through its pipe.
    StopSignal,
    /// SIGCHLD, through its pipe: a child of `yuna mcp` ended.
    ChildEnded,
    /// The MCP client sent something.
    McpInput,
    /// The MCP client can take more of the replies waiting for it.
    McpOutput,
    /// A capture's image came back from its encoder's thread.
    Encoders,
    /// A program wrote to its output, or closed it.
    ProgramOutput,
    /// The programs' throttle ended: their output is read again.
    ProgramsThrottle,
    /// A slowed program's pace stopped or continued it.
    ProgramPace,
    /// A seat's closing reached its next stage.
    SeatClosing,
    /// A waiting call's time is up: `windows`, `events` or `rest`.
    WaitingCall,
    /// A capture's frame did not come in time.
    CaptureTimeout,
    /// An encoded capture's frame reached its boundary.
    CaptureBoundary,
    /// An Xwayland did not start, or did not end, in time; or its manager took none of Yuna's
    /// commands in time, its watchdog.
    XwaylandTimeout,
    /// A seat's Xwayland may start again, after a pause that followed its failures.
    XwaylandPause,
    /// An X11 program connected to a seat's X display.
    X11Connection,
    /// Xwayland gave its display's number: it is ready.
    XwaylandDisplay,
    /// Xwayland exited: stopped idle, crashed or ended.
    XwaylandExited,
    /// A seat's X11 window manager sent an event, took the commands a watchdog waited on, or
    /// ended.
    X11Manager,
    /// Yuna's connection to a seat's accessibility bus can be read, or written again.
    AccessibilityBus,
    /// A seat's accessibility bus daemon exited.
    AccessibilityDaemon,
    /// A starting accessibility bus daemon wrote its address, or closed its pipe.
    AccessibilityAddress,
    /// A seat's accessibility bus daemon wrote to its log.
    AccessibilityLog,
    /// An accessibility bus did not start in time.
    AccessibilityStart,
    /// An `accessibility` call's walk is due again, overdue, or its wait is up.
    AccessibilityCall,
    /// The program that holds a seat's selection wrote to the pipe `clipboard get` reads.
    ClipboardRead,
    /// A `clipboard get`'s program did not close its pipe in time.
    ClipboardReadTimeout,
    /// A reader of an agent's selection can take more of it.
    SelectionWrite,
    /// A reader of an agent's selection took nothing more in time.
    SelectionWriteTimeout,
}

impl Source {
    /// How many there are.
    pub const COUNT: usize = 36;

    /// Every source, in the order results give them.
    pub const ALL: [Source; Source::COUNT] = [
        Source::Clients,
        Source::SeatSocket,
        Source::SocketPause,
        Source::ClockRefresh,
        Source::ClockPresentation,
        Source::ClockHiddenFrames,
        Source::ClockX11Answers,
        Source::ClockDrags,
        Source::StopSignal,
        Source::ChildEnded,
        Source::McpInput,
        Source::McpOutput,
        Source::Encoders,
        Source::ProgramOutput,
        Source::ProgramsThrottle,
        Source::ProgramPace,
        Source::SeatClosing,
        Source::WaitingCall,
        Source::CaptureTimeout,
        Source::CaptureBoundary,
        Source::XwaylandTimeout,
        Source::XwaylandPause,
        Source::X11Connection,
        Source::XwaylandDisplay,
        Source::XwaylandExited,
        Source::X11Manager,
        Source::AccessibilityBus,
        Source::AccessibilityDaemon,
        Source::AccessibilityAddress,
        Source::AccessibilityLog,
        Source::AccessibilityStart,
        Source::AccessibilityCall,
        Source::ClipboardRead,
        Source::ClipboardReadTimeout,
        Source::SelectionWrite,
        Source::SelectionWriteTimeout,
    ];

    /// Its name, as the `rest` tool and the log give it.
    pub fn name(self) -> &'static str {
        match self {
            Source::Clients => "clients",
            Source::SeatSocket => "seat_socket",
            Source::SocketPause => "socket_pause",
            Source::ClockRefresh => "clock_refresh",
            Source::ClockPresentation => "clock_presentation",
            Source::ClockHiddenFrames => "clock_hidden_frames",
            Source::ClockX11Answers => "clock_x11_answers",
            Source::ClockDrags => "clock_drags",
            Source::StopSignal => "stop_signal",
            Source::ChildEnded => "child_ended",
            Source::McpInput => "mcp_input",
            Source::McpOutput => "mcp_output",
            Source::Encoders => "encoders",
            Source::ProgramOutput => "program_output",
            Source::ProgramsThrottle => "programs_throttle",
            Source::ProgramPace => "program_pace",
            Source::SeatClosing => "seat_closing",
            Source::WaitingCall => "waiting_call",
            Source::CaptureTimeout => "capture_timeout",
            Source::CaptureBoundary => "capture_boundary",
            Source::XwaylandTimeout => "xwayland_timeout",
            Source::XwaylandPause => "xwayland_pause",
            Source::X11Connection => "x11_connection",
            Source::XwaylandDisplay => "xwayland_display",
            Source::XwaylandExited => "xwayland_exited",
            Source::X11Manager => "x11_manager",
            Source::AccessibilityBus => "accessibility_bus",
            Source::AccessibilityDaemon => "accessibility_daemon",
            Source::AccessibilityAddress => "accessibility_address",
            Source::AccessibilityLog => "accessibility_log",
            Source::AccessibilityStart => "accessibility_start",
            Source::AccessibilityCall => "accessibility_call",
            Source::ClipboardRead => "clipboard_read",
            Source::ClipboardReadTimeout => "clipboard_read_timeout",
            Source::SelectionWrite => "selection_write",
            Source::SelectionWriteTimeout => "selection_write_timeout",
        }
    }

    /// Whether it serves one seat, and is counted for that seat too: a seat's sockets, its
    /// outputs' refreshes and presentations, its programs, its closing, its Xwayland, its
    /// accessibility bus and calls, its clipboard's reads and writes, and the display's wakes in
    /// which one of its clients had something. The others are Yuna's own parts: the signals,
    /// MCP's input and output, its encoders, its waiting calls and its captures' timers, the
    /// throttle every program shares, the hidden windows', the X11 answers' and the drags'
    /// deadlines.
    pub fn serves_a_seat(self) -> bool {
        matches!(
            self,
            Source::Clients
                | Source::SeatSocket
                | Source::SocketPause
                | Source::ClockRefresh
                | Source::ClockPresentation
                | Source::ProgramOutput
                | Source::ProgramPace
                | Source::SeatClosing
                | Source::XwaylandTimeout
                | Source::XwaylandPause
                | Source::X11Connection
                | Source::XwaylandDisplay
                | Source::XwaylandExited
                | Source::X11Manager
                | Source::AccessibilityBus
                | Source::AccessibilityDaemon
                | Source::AccessibilityAddress
                | Source::AccessibilityLog
                | Source::AccessibilityStart
                | Source::AccessibilityCall
                | Source::ClipboardRead
                | Source::ClipboardReadTimeout
                | Source::SelectionWrite
                | Source::SelectionWriteTimeout
        )
    }

    fn index(self) -> usize {
        self as usize
    }
}

// Each source's count is at its index in arrays of `COUNT`: a source added must come before
// `SelectionWriteTimeout`, the last, and in `ALL`, whose length is `COUNT`.
const _: () = assert!(Source::SelectionWriteTimeout as usize + 1 == Source::COUNT);

/// Why a frame was drawn: what armed its output's refresh, or joined it, since that output's
/// last refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cause {
    /// A client's commit to a surface the output shows: a window's, a popup's, an X11 menu's.
    Commit,
    /// The layout changed: a window opened, closed or moved, an output came or went.
    LayoutChange,
    /// The output's motion runs: each of its frames arms the next.
    Motion,
    /// A capture asked for the output's newest frame.
    Capture,
    /// The output was created.
    FirstFrame,
    /// The output was given another mode.
    OutputChange,
    /// A popup or an X11 menu came, went or moved without a commit of its own.
    Popups,
}

impl Cause {
    /// How many there are.
    pub const COUNT: usize = 7;

    /// Every cause, in the order results give them.
    pub const ALL: [Cause; Cause::COUNT] = [
        Cause::Commit,
        Cause::LayoutChange,
        Cause::Motion,
        Cause::Capture,
        Cause::FirstFrame,
        Cause::OutputChange,
        Cause::Popups,
    ];

    /// Its name, as the `rest` tool and the log give it.
    pub fn name(self) -> &'static str {
        match self {
            Cause::Commit => "commit",
            Cause::LayoutChange => "layout_change",
            Cause::Motion => "motion",
            Cause::Capture => "capture",
            Cause::FirstFrame => "first_frame",
            Cause::OutputChange => "output_change",
            Cause::Popups => "popups",
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    fn bit(self) -> u8 {
        1 << self.index()
    }
}

// As for `Source`, with `Popups` last; a frame's causes are bits of a `u8`.
const _: () = assert!(Cause::Popups as usize + 1 == Cause::COUNT && Cause::COUNT <= 8);

/// What a source is, given when it is inserted: its kind, and the seat it serves, if any; none
/// for the auditor's own, which is never counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Wake {
    source: Option<Source>,
    seat: Option<SeatId>,
}

impl Wake {
    /// A source of Yuna's own, which serves no seat.
    pub(crate) fn of(source: Source) -> Wake {
        Wake {
            source: Some(source),
            seat: None,
        }
    }

    /// A source that serves `seat`.
    pub(crate) fn of_seat(source: Source, seat: SeatId) -> Wake {
        Wake {
            source: Some(source),
            seat: Some(seat),
        }
    }

    /// The auditor's own source, the timer of the log of each second: what observes Yuna's rest
    /// is not counted as breaking it, as a `rest` call's own timer is left out of its window.
    pub(crate) fn of_the_auditor() -> Wake {
        Wake {
            source: None,
            seat: None,
        }
    }
}

/// Why an output's refresh is armed, as the frame loop notes it ([`PendingCauses`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrawCause {
    /// A commit, by the client of this number, when it is known.
    Commit(Option<u64>),
    LayoutChange,
    Motion,
    Capture,
    FirstFrame,
    OutputChange,
    Popups,
}

impl DrawCause {
    fn cause(self) -> Cause {
        match self {
            DrawCause::Commit(_) => Cause::Commit,
            DrawCause::LayoutChange => Cause::LayoutChange,
            DrawCause::Motion => Cause::Motion,
            DrawCause::Capture => Cause::Capture,
            DrawCause::FirstFrame => Cause::FirstFrame,
            DrawCause::OutputChange => Cause::OutputChange,
            DrawCause::Popups => Cause::Popups,
        }
    }
}

/// The causes an output's next frame has so far, and the clients whose commits it shows: one
/// entry each, so at most the clients connected.
#[derive(Debug, Default)]
pub(crate) struct PendingCauses {
    causes: u8,
    committers: Vec<u64>,
}

impl PendingCauses {
    /// Notes `cause`, and its client, once.
    pub(crate) fn note(&mut self, cause: DrawCause) {
        self.causes |= cause.cause().bit();
        if let DrawCause::Commit(Some(client)) = cause
            && !self.committers.contains(&client)
        {
            self.committers.push(client);
        }
    }

    /// Forgets the causes noted: the refresh they were for will not run.
    pub(crate) fn clear(&mut self) {
        self.causes = 0;
        self.committers.clear();
    }

    /// The causes noted, leaving none.
    pub(crate) fn take(&mut self) -> PendingCauses {
        std::mem::take(self)
    }

    /// Takes back the clients' list of `used`, emptied, unless clients were noted meanwhile: the
    /// next frame's clients then need no new room.
    pub(crate) fn recycle(&mut self, used: PendingCauses) {
        if self.committers.is_empty() && self.committers.capacity() < used.committers.capacity() {
            let mut list = used.committers;
            list.clear();
            self.committers = list;
        }
    }

    fn has(&self, cause: Cause) -> bool {
        self.causes & cause.bit() != 0
    }
}

/// What a client, or the clients gone, did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientTally {
    /// Times the display woke while it had something to send, or had hung up.
    pub wakes: u64,
    /// Its `wl_surface.commit` requests.
    pub commits: u64,
    /// Frames drawn with one of its commits among their causes.
    pub draws: u64,
}

impl ClientTally {
    fn add(&mut self, other: &ClientTally) {
        self.wakes += other.wakes;
        self.commits += other.commits;
        self.draws += other.draws;
    }

    fn minus(&self, earlier: &ClientTally) -> ClientTally {
        ClientTally {
            wakes: self.wakes.saturating_sub(earlier.wakes),
            commits: self.commits.saturating_sub(earlier.commits),
            draws: self.draws.saturating_sub(earlier.draws),
        }
    }

    /// Whether it did nothing.
    pub fn is_empty(&self) -> bool {
        *self == ClientTally::default()
    }
}

/// What woke Yuna and what it drew, for the whole of Yuna or for one seat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tally {
    /// Wakes by source, [`Source::ALL`]'s order; a seat's holds only the sources that
    /// [serve a seat](Source::serves_a_seat).
    pub wakes: [u64; Source::COUNT],
    /// Frames drawn.
    pub frames: u64,
    /// Frames drawn by cause, [`Cause::ALL`]'s order: a frame counts once under each of its
    /// causes.
    pub causes: [u64; Cause::COUNT],
    /// Clients that left.
    pub gone_clients: u64,
    /// What they did, together.
    pub gone: ClientTally,
}

// Arrays past 32 have no derived `Default`.
impl Default for Tally {
    fn default() -> Tally {
        Tally {
            wakes: [0; Source::COUNT],
            frames: 0,
            causes: [0; Cause::COUNT],
            gone_clients: 0,
            gone: ClientTally::default(),
        }
    }
}

impl Tally {
    /// Wakes by `source`.
    pub fn wakes_by(&self, source: Source) -> u64 {
        self.wakes[source.index()]
    }

    /// Frames drawn with `cause` among their causes.
    pub fn frames_by(&self, cause: Cause) -> u64 {
        self.causes[cause.index()]
    }

    fn count_frame(&mut self, causes: &PendingCauses) {
        self.frames += 1;
        for cause in Cause::ALL {
            if causes.has(cause) {
                self.causes[cause.index()] += 1;
            }
        }
    }

    fn fold_gone(&mut self, tally: &ClientTally) {
        self.gone_clients += 1;
        self.gone.add(tally);
    }

    fn minus(&self, earlier: &Tally) -> Tally {
        Tally {
            wakes: std::array::from_fn(|i| self.wakes[i].saturating_sub(earlier.wakes[i])),
            frames: self.frames.saturating_sub(earlier.frames),
            causes: std::array::from_fn(|i| self.causes[i].saturating_sub(earlier.causes[i])),
            gone_clients: self.gone_clients.saturating_sub(earlier.gone_clients),
            gone: self.gone.minus(&earlier.gone),
        }
    }
}

/// A client connected, as the auditor knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCount {
    /// Its seat.
    pub seat: SeatId,
    /// Its process, read when it connected, which names its program in `yuna mcp`.
    pub process: ClientProcess,
    /// Whether it is its seat's Xwayland: the client of every X11 program of the seat.
    pub xwayland: bool,
    /// What it did.
    pub tally: ClientTally,
}

/// What woke Yuna and what it drew, from `since` to `at`, as
/// [`Compositor::rest_counts`](crate::Compositor::rest_counts) gives it since Yuna started,
/// and [`RestCounts::since`] between two of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestCounts {
    /// When they begin, on the clock's scale.
    pub since: std::time::Duration,
    /// When they were read, on the clock's scale.
    pub at: std::time::Duration,
    /// Times Yuna woke, every seat's together, as [`Counters::wakeups`](crate::Counters).
    pub woke: u64,
    /// The display's wakes in which no client Yuna follows had something: a client whose second
    /// descriptor could not be made, out of descriptors.
    pub unnamed_client_wakes: u64,
    /// The whole of Yuna's.
    pub whole: Tally,
    /// Each seat's that exists.
    pub seats: BTreeMap<SeatId, Tally>,
    /// Each client connected, by its number: clients are numbered from 1 as they connect, and a
    /// number is never reused.
    pub clients: BTreeMap<u64, ClientCount>,
}

impl RestCounts {
    /// Leaves out one wake of `source`, a source of Yuna's own, and the time Yuna woke for it
    /// when it woke `alone`: what a call that counts a window leaves out of it, its own timer.
    pub(crate) fn leave_out_wake(&mut self, source: Source, alone: bool) {
        let wakes = &mut self.whole.wakes[source.index()];
        *wakes = wakes.saturating_sub(1);
        if alone {
            self.woke = self.woke.saturating_sub(1);
        }
    }

    /// What happened between `earlier` and these, both read from one compositor. A client that
    /// left meanwhile is among the clients gone with what it did after `earlier` alone; a seat
    /// removed meanwhile is not listed.
    pub fn since(&self, earlier: &RestCounts) -> RestCounts {
        let mut window = RestCounts {
            since: earlier.at,
            at: self.at,
            woke: self.woke.saturating_sub(earlier.woke),
            unnamed_client_wakes: self
                .unnamed_client_wakes
                .saturating_sub(earlier.unnamed_client_wakes),
            whole: self.whole.minus(&earlier.whole),
            seats: self
                .seats
                .iter()
                .map(|(seat, tally)| {
                    let window = match earlier.seats.get(seat) {
                        Some(earlier) => tally.minus(earlier),
                        None => tally.clone(),
                    };
                    (*seat, window)
                })
                .collect(),
            clients: BTreeMap::new(),
        };
        for (number, client) in &self.clients {
            let mut client = client.clone();
            if let Some(earlier) = earlier.clients.get(number) {
                client.tally = client.tally.minus(&earlier.tally);
            }
            window.clients.insert(*number, client);
        }
        // A client that left was folded into the clients gone whole: what it had done before
        // the window is not the window's.
        for (number, client) in &earlier.clients {
            if self.clients.contains_key(number) {
                continue;
            }
            window.whole.gone = window.whole.gone.minus(&client.tally);
            if let Some(seat) = window.seats.get_mut(&client.seat) {
                seat.gone = seat.gone.minus(&client.tally);
            }
        }
        window
    }
}

/// What one second of the log counted.
#[derive(Debug)]
pub(crate) struct Second {
    /// The rounds that woke Yuna in it, and the last of them, counted once.
    woke: u64,
    last_round: Option<u64>,
    wakes: [u64; Source::COUNT],
    frames: u64,
    causes: [u64; Cause::COUNT],
    unnamed_client_wakes: u64,
    /// By client number, while each is connected.
    clients: HashMap<u64, ClientTally>,
    /// What the clients that left in it did in it.
    gone: ClientTally,
}

// Arrays past 32 have no derived `Default`.
impl Default for Second {
    fn default() -> Second {
        Second {
            woke: 0,
            last_round: None,
            wakes: [0; Source::COUNT],
            frames: 0,
            causes: [0; Cause::COUNT],
            unnamed_client_wakes: 0,
            clients: HashMap::new(),
            gone: ClientTally::default(),
        }
    }
}

/// The auditor: its counts and what it needs to make them. The compositor's state owns it, and
/// changes it only through its methods.
pub(crate) struct RestAudit {
    counts: RestCounts,
    /// The rounds of the event loop, so that a source counts once a round.
    round: u64,
    /// How many sources had something in the current round.
    round_sources: u32,
    next_client: u64,
    /// Where the clients' second descriptors are watched; none if it could not be made.
    epoll: Option<OwnedFd>,
    /// Each client's second descriptor, by number.
    sockets: HashMap<u64, OwnedFd>,
    /// The clients that had something, at a wait: room for every client watched.
    ready: Vec<epoll::Event>,
    /// The seats counted at a wait, so that each counts once.
    seats_counted: Vec<SeatId>,
    log_seconds: bool,
    second: Option<Second>,
    second_needs_timer: bool,
    ended_second: Option<Second>,
}

impl RestAudit {
    /// An auditor whose counts begin at `now`, on the clock's scale.
    pub(crate) fn new(now: std::time::Duration) -> RestAudit {
        let epoll = epoll::create(epoll::CreateFlags::CLOEXEC)
            .inspect_err(|error| {
                tracing::warn!(%error, "the rest auditor cannot name clients: no epoll");
            })
            .ok();
        RestAudit {
            counts: RestCounts {
                since: now,
                ..RestCounts::default()
            },
            round: 0,
            round_sources: 0,
            next_client: 1,
            epoll,
            sockets: HashMap::new(),
            ready: Vec::new(),
            seats_counted: Vec::new(),
            log_seconds: false,
            second: None,
            second_needs_timer: false,
            ended_second: None,
        }
    }

    /// The counts so far, but for [`RestCounts::woke`] and [`RestCounts::at`], which the
    /// compositor holds.
    pub(crate) fn counts(&self) -> &RestCounts {
        &self.counts
    }

    /// A round of the event loop begins.
    pub(crate) fn begin_round(&mut self) {
        self.round += 1;
        self.round_sources = 0;
    }

    /// How many sources had something in the last round, the auditor's own left out.
    pub(crate) fn round_sources(&self) -> u32 {
        self.round_sources
    }

    /// `source` had something, once in a round: from [`insert_source`]'s callbacks.
    fn source_woke(&mut self, source: Source, seat: Option<SeatId>) {
        self.round_sources += 1;
        self.count_wake(source, seat);
    }

    /// The manual clock's advance ran deadlines at a time of their own, as the clock's timer
    /// would have woken Yuna for `wake`: a round of its own.
    pub(crate) fn clock_woke(&mut self, wake: Wake) {
        self.begin_round();
        if let Some(source) = wake.source {
            self.source_woke(source, wake.seat);
        }
    }

    fn count_wake(&mut self, source: Source, seat: Option<SeatId>) {
        let index = source.index();
        self.counts.whole.wakes[index] += 1;
        // A seat removed is counted no more: an Xwayland may end after its seat.
        if let Some(tally) = seat.and_then(|seat| self.counts.seats.get_mut(&seat)) {
            tally.wakes[index] += 1;
        }
        if self.log_seconds && self.second.is_none() {
            self.second = Some(Second::default());
            self.second_needs_timer = true;
        }
        if let Some(second) = &mut self.second {
            second.wakes[index] += 1;
            // Counted in the second that holds its first wake, as a second that ends inside
            // a round leaves that round's later wakes to the next.
            if second.last_round != Some(self.round) {
                second.last_round = Some(self.round);
                second.woke += 1;
            }
        }
    }

    /// The next client's number.
    pub(crate) fn next_client_number(&mut self) -> u64 {
        let number = self.next_client;
        self.next_client += 1;
        number
    }

    /// Client `number` connected to `seat`, with its process; `socket`, a second descriptor of
    /// its socket, is watched to name it when it wakes Yuna.
    pub(crate) fn client_connected(
        &mut self,
        number: u64,
        seat: SeatId,
        process: ClientProcess,
        xwayland: bool,
        socket: Option<UnixStream>,
    ) {
        self.counts.clients.insert(
            number,
            ClientCount {
                seat,
                process,
                xwayland,
                tally: ClientTally::default(),
            },
        );
        let (Some(epoll), Some(socket)) = (&self.epoll, socket) else {
            return;
        };
        let socket = OwnedFd::from(socket);
        let watched = epoll::add(
            epoll,
            &socket,
            epoll::EventData::new_u64(number),
            epoll::EventFlags::IN,
        );
        match watched {
            Ok(()) => {
                self.sockets.insert(number, socket);
                // Room for every client watched to be ready at once, made now rather than at a
                // wake; what it holds is a wait's alone.
                self.ready.clear();
                self.ready.reserve_exact(self.sockets.len());
            }
            Err(error) => tracing::debug!(%error, "a client is not named when it wakes Yuna"),
        }
    }

    /// Client `number` left: its descriptor is closed, and what it did folded into the clients
    /// gone, its seat's and Yuna's.
    pub(crate) fn client_left(&mut self, number: u64) {
        if let Some(socket) = self.sockets.remove(&number)
            && let Some(epoll) = &self.epoll
        {
            let _ = epoll::delete(epoll, &socket);
        }
        if let Some(second) = &mut self.second
            && let Some(tally) = second.clients.remove(&number)
        {
            second.gone.add(&tally);
        }
        let Some(client) = self.counts.clients.remove(&number) else {
            return;
        };
        self.counts.whole.fold_gone(&client.tally);
        if let Some(seat) = self.counts.seats.get_mut(&client.seat) {
            seat.fold_gone(&client.tally);
        }
    }

    /// The display woke: counts each client that had something, before the display reads it.
    pub(crate) fn clients_readable(&mut self) {
        let RestAudit {
            counts,
            epoll,
            ready,
            seats_counted,
            second,
            sockets,
            ..
        } = self;
        ready.clear();
        if let Some(epoll) = epoll
            && !sockets.is_empty()
        {
            let no_time = rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            if let Err(error) = epoll::wait(
                epoll,
                rustix::buffer::spare_capacity(&mut *ready),
                Some(&no_time),
            ) {
                tracing::debug!(%error, "the clients that woke Yuna are not named");
            }
        }
        if ready.is_empty() {
            counts.unnamed_client_wakes += 1;
            if let Some(second) = second {
                second.unnamed_client_wakes += 1;
            }
            return;
        }
        seats_counted.clear();
        for event in ready.iter() {
            let number = event.data.u64();
            let Some(client) = counts.clients.get_mut(&number) else {
                continue;
            };
            client.tally.wakes += 1;
            if let Some(second) = second {
                second.clients.entry(number).or_default().wakes += 1;
            }
            if !seats_counted.contains(&client.seat) {
                seats_counted.push(client.seat);
                if let Some(tally) = counts.seats.get_mut(&client.seat) {
                    tally.wakes[Source::Clients.index()] += 1;
                }
            }
        }
    }

    /// Client `number` committed a surface.
    pub(crate) fn client_committed(&mut self, number: u64) {
        let Some(client) = self.counts.clients.get_mut(&number) else {
            return;
        };
        client.tally.commits += 1;
        if let Some(second) = &mut self.second {
            second.clients.entry(number).or_default().commits += 1;
        }
    }

    /// An output of `seat` drew a frame, for `causes`.
    pub(crate) fn frame_drawn(&mut self, seat: SeatId, causes: &PendingCauses) {
        self.counts.whole.count_frame(causes);
        if let Some(tally) = self.counts.seats.get_mut(&seat) {
            tally.count_frame(causes);
        }
        for number in &causes.committers {
            if let Some(client) = self.counts.clients.get_mut(number) {
                client.tally.draws += 1;
            }
        }
        if let Some(second) = &mut self.second {
            second.frames += 1;
            for cause in Cause::ALL {
                if causes.has(cause) {
                    second.causes[cause.index()] += 1;
                }
            }
            for number in &causes.committers {
                if self.counts.clients.contains_key(number) {
                    second.clients.entry(*number).or_default().draws += 1;
                }
            }
        }
    }

    /// `seat` was made: it is counted from now on.
    pub(crate) fn add_seat(&mut self, seat: SeatId) {
        self.counts.seats.entry(seat).or_default();
        // Room for every seat to be counted at a wake, made now rather than then.
        self.seats_counted.reserve(self.counts.seats.len());
    }

    /// `seat` was removed: its counts go, Yuna's keep them.
    pub(crate) fn remove_seat(&mut self, seat: SeatId) {
        self.counts.seats.remove(&seat);
    }

    /// Logs each second in which something woke Yuna, from now on.
    pub(crate) fn log_each_second(&mut self) {
        self.log_seconds = true;
    }

    /// Whether a second has begun whose timer is not armed yet; asked once.
    pub(crate) fn take_second_needing_timer(&mut self) -> bool {
        std::mem::take(&mut self.second_needs_timer)
    }

    /// The second's timer is up: its counts wait to be logged.
    pub(crate) fn end_second(&mut self) {
        self.ended_second = self.second.take();
    }

    /// The second that ended, to log.
    pub(crate) fn take_ended_second(&mut self) -> Option<Second> {
        self.ended_second.take()
    }
}

/// Inserts `source` into the event loop, as calloop's `LoopHandle::insert_source` does, with
/// what it is: each round in which it has something, it is counted once, and Yuna is noted as
/// woken, before `callback` runs; the auditor's own source is neither. The one way Yuna inserts
/// a source (`clippy.toml`).
#[expect(
    clippy::disallowed_methods,
    reason = "the one insertion of a source, which names and counts its wakes"
)]
pub(crate) fn insert_source<S, F>(
    handle: &LoopHandle<'static, State>,
    wake: Wake,
    source: S,
    mut callback: F,
) -> Result<RegistrationToken, InsertError<S>>
where
    S: EventSource + 'static,
    F: FnMut(S::Event, &mut S::Metadata, &mut State) -> S::Ret + 'static,
{
    let mut counted_in = None;
    handle.insert_source(source, move |event, metadata, state: &mut State| {
        if let Some(kind) = wake.source
            && counted_in != Some(state.rest.round)
        {
            counted_in = Some(state.rest.round);
            state.woke = true;
            state.rest.source_woke(kind, wake.seat);
        }
        callback(event, metadata, state)
    })
}

/// Logs `second` in one line at `info`: what woke Yuna in it, by source, each client by its
/// process, its seat and the program `program_of` names, and the frames drawn, by cause.
pub(crate) fn log_second(
    second: &Second,
    live: &RestCounts,
    program_of: &dyn Fn(SeatId, &ClientProcess) -> Option<u64>,
) {
    let mut sources = String::new();
    for source in Source::ALL {
        let count = second.wakes[source.index()];
        if count > 0 {
            append_item(&mut sources, &format!("{} {count}", source.name()));
        }
    }
    let mut numbers: Vec<&u64> = second.clients.keys().collect();
    numbers.sort();
    let mut clients = String::new();
    let mut gone = second.gone;
    for number in numbers {
        let tally = &second.clients[number];
        let Some(client) = live.clients.get(number) else {
            gone.add(tally);
            continue;
        };
        let mut name = match client.process.pid {
            Some(pid) => format!("pid {pid}"),
            None => "pid unknown".to_owned(),
        };
        if let Some(program) = program_of(client.seat, &client.process) {
            name.push_str(&format!(" program {program}"));
        }
        name.push_str(&format!(" seat {}", client.seat.0));
        if client.xwayland {
            name.push_str(" xwayland");
        }
        append_item(&mut clients, &format!("{name}: {}", tally_text(tally)));
    }
    if !gone.is_empty() {
        append_item(&mut clients, &format!("gone: {}", tally_text(&gone)));
    }
    if second.unnamed_client_wakes > 0 {
        append_item(
            &mut clients,
            &format!("unnamed: {} wakes", second.unnamed_client_wakes),
        );
    }
    let mut causes = String::new();
    for cause in Cause::ALL {
        let count = second.causes[cause.index()];
        if count > 0 {
            append_item(&mut causes, &format!("{} {count}", cause.name()));
        }
    }
    tracing::info!(
        woke = second.woke,
        sources = %sources,
        clients = %clients,
        frames = second.frames,
        causes = %causes,
        "rest audit: what woke Yuna in one second"
    );
}

fn tally_text(tally: &ClientTally) -> String {
    format!(
        "{} wakes {} commits {} draws",
        tally.wakes, tally.commits, tally.draws
    )
}

fn append_item(list: &mut String, item: &str) {
    if !list.is_empty() {
        list.push_str(", ");
    }
    list.push_str(item);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_and_cause_is_listed_once_in_its_place_with_a_name_of_its_own() {
        for (index, source) in Source::ALL.iter().enumerate() {
            assert_eq!(source.index(), index);
        }
        for (index, cause) in Cause::ALL.iter().enumerate() {
            assert_eq!(cause.index(), index);
        }
        let mut names: Vec<&str> = Source::ALL.iter().map(|source| source.name()).collect();
        names.extend(Cause::ALL.iter().map(|cause| cause.name()));
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count);
    }

    #[test]
    fn a_frame_counts_once_under_each_cause_and_for_each_committer_once() {
        let mut audit = RestAudit::new(std::time::Duration::ZERO);
        let seat = SeatId(2);
        audit.add_seat(seat);
        let process = ClientProcess::connecting(None, false);
        for number in [1, 2] {
            audit.client_connected(number, seat, process.clone(), false, None);
        }
        let mut causes = PendingCauses::default();
        causes.note(DrawCause::Commit(Some(1)));
        causes.note(DrawCause::Commit(Some(1)));
        causes.note(DrawCause::Commit(Some(2)));
        causes.note(DrawCause::LayoutChange);
        let taken = causes.take();
        audit.frame_drawn(seat, &taken);
        causes.recycle(taken);
        assert!(causes.committers.is_empty() && causes.committers.capacity() >= 2);
        let counts = audit.counts();
        assert_eq!(counts.whole.frames, 1);
        assert_eq!(counts.whole.frames_by(Cause::Commit), 1);
        assert_eq!(counts.whole.frames_by(Cause::LayoutChange), 1);
        assert_eq!(counts.whole.frames_by(Cause::Motion), 0);
        assert_eq!(counts.seats[&seat].frames, 1);
        assert_eq!(counts.clients[&1].tally.draws, 1);
        assert_eq!(counts.clients[&2].tally.draws, 1);
    }

    #[test]
    fn a_client_gone_in_a_window_counts_what_it_did_in_it_alone() {
        let mut audit = RestAudit::new(std::time::Duration::ZERO);
        let seat = SeatId(2);
        audit.add_seat(seat);
        audit.client_connected(1, seat, ClientProcess::connecting(None, false), false, None);
        for _ in 0..5 {
            audit.client_committed(1);
        }
        let earlier = audit.counts().clone();
        for _ in 0..3 {
            audit.client_committed(1);
        }
        audit.client_left(1);
        audit.client_connected(2, seat, ClientProcess::connecting(None, false), false, None);
        audit.client_committed(2);
        audit.client_left(2);
        let window = audit.counts().since(&earlier);
        assert!(window.clients.is_empty());
        assert_eq!(window.whole.gone_clients, 2);
        assert_eq!(window.whole.gone.commits, 4);
        assert_eq!(window.seats[&seat].gone.commits, 4);
        assert_eq!(audit.counts().whole.gone.commits, 9);
    }

    #[test]
    fn a_second_opens_at_a_wake_and_its_own_timer_opens_none() {
        let mut audit = RestAudit::new(std::time::Duration::ZERO);
        audit.add_seat(SeatId(2));
        audit.clock_woke(Wake::of(Source::ClockHiddenFrames));
        assert!(!audit.take_second_needing_timer(), "no second unless asked");
        audit.log_each_second();
        audit.clock_woke(Wake::of_the_auditor());
        assert!(
            !audit.take_second_needing_timer(),
            "the log's own wake opened a second"
        );
        audit.clock_woke(Wake::of_seat(Source::ClockRefresh, SeatId(2)));
        assert!(audit.take_second_needing_timer());
        audit.clock_woke(Wake::of_seat(Source::ClockRefresh, SeatId(2)));
        assert!(!audit.take_second_needing_timer(), "one timer a second");
        audit.clock_woke(Wake::of_the_auditor());
        audit.end_second();
        let second = audit.take_ended_second().expect("a second ended");
        assert_eq!(second.woke, 2);
        assert_eq!(second.wakes[Source::ClockRefresh.index()], 2);
        assert_eq!(second.wakes.iter().sum::<u64>(), 2);
        assert_eq!(audit.counts().whole.wakes.iter().sum::<u64>(), 3);
        assert_eq!(
            audit.counts().seats[&SeatId(2)].wakes_by(Source::ClockRefresh),
            2
        );
    }

    #[test]
    fn a_round_is_counted_in_the_second_of_its_first_wake_and_its_later_ones_in_the_next() {
        let mut audit = RestAudit::new(std::time::Duration::ZERO);
        audit.add_seat(SeatId(2));
        audit.log_each_second();
        audit.begin_round();
        audit.source_woke(Source::McpInput, None);
        // The second ends inside the round, its timer among its sources.
        audit.end_second();
        audit.source_woke(Source::Clients, Some(SeatId(2)));
        let ended = audit.take_ended_second().expect("a second ended");
        assert_eq!((ended.woke, ended.wakes[Source::McpInput.index()]), (1, 1));
        audit.end_second();
        let next = audit.take_ended_second().expect("the next second");
        assert_eq!((next.woke, next.wakes[Source::Clients.index()]), (1, 1));
        assert_eq!(audit.round_sources(), 2);
    }

    #[test]
    fn a_client_that_leaves_in_a_second_is_kept_among_its_clients_gone_alone() {
        let mut audit = RestAudit::new(std::time::Duration::ZERO);
        let seat = SeatId(2);
        audit.add_seat(seat);
        audit.log_each_second();
        audit.client_connected(1, seat, ClientProcess::connecting(None, false), false, None);
        audit.clock_woke(Wake::of_seat(Source::ClockRefresh, seat));
        audit.client_committed(1);
        audit.client_left(1);
        audit.client_committed(1);
        audit.end_second();
        let second = audit.take_ended_second().expect("a second ended");
        assert!(second.clients.is_empty());
        assert_eq!(second.gone.commits, 1);
    }

    #[test]
    fn a_seat_removed_is_counted_no_more() {
        let mut audit = RestAudit::new(std::time::Duration::ZERO);
        let seat = SeatId(3);
        audit.add_seat(seat);
        audit.clock_woke(Wake::of_seat(Source::XwaylandExited, seat));
        audit.remove_seat(seat);
        audit.clock_woke(Wake::of_seat(Source::XwaylandExited, seat));
        audit.frame_drawn(seat, &PendingCauses::default());
        assert!(audit.counts().seats.is_empty());
        assert_eq!(audit.counts().whole.wakes_by(Source::XwaylandExited), 2);
        assert_eq!(audit.counts().whole.frames, 1);
    }
}
