//! Yuna's MCP server: what `yuna mcp` runs, a headless compositor of one MCP client's own, which
//! speaks MCP on the client's pipes ([ADR-0033](../../../../docs/adr/0033-the-mcp-server.md)).
//!
//! Everything runs in the compositor's event loop, on its thread: the client's input and output,
//! the programs' output and exits, the calls that wait. Their sources only note what came; after
//! each round, [`Server::dispatch`] reads the requests, answers them, and writes the replies. A
//! call that waits, for a window or an event, is answered from a later round, once the layout
//! has placed the window or the round that recorded the event has ended, or when its time is
//! up, on the real clock whatever the compositor's:
//! [`tools::MAX_WAIT`] at most, under the time a client gives a call. On a manual clock, the
//! window is placed only once the clock moves to its output's refresh. A `clipboard get` waits
//! for the program that copied to write into a pipe, which each round reads as far as it goes,
//! never waiting, and is answered once the program closes it, past its `max_bytes`, or at
//! [`CLIPBOARD_TIMEOUT`] with what came.
//! `notifications/cancelled` drops a waiting call unanswered.
//!
//! Everything an agent does is on a seat of its own, which `seat create` makes
//! ([ADR-0078](../../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md),
//! [ADR-0095](../../../../docs/adr/0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)): every tool but
//! `clock` names it, and an id of another seat's output, window or program is unknown there.
//! `seat close` releases what the seat holds, removes its socket, ends its clients, its outputs
//! and its place in the layout at once, then ends its processes: SIGTERM and SIGCONT, then
//! SIGKILL [`END_GRACE`] later; it is answered once they are gone, [`KILL_WAIT`] after SIGKILL at
//! most.
//!
//! A program started with `accessibility: true` joins its seat's accessibility bus, which the
//! seat's first such program starts, and on which Yuna is the AT-SPI registry; `accessibility`
//! reads the trees there (`accessibility/`). A `spawn start` with accessibility waits for its
//! seat's bus, and is answered once the program has started, or with the bus's failure.
//!
//! Bounds: a line over [`stdio::MAX_LINE`] ends the connection; while more than
//! [`MAX_QUEUED_OUTPUT`] of replies wait, because the client does not read them, no further
//! request is answered and its input is not read; at most [`tools::MAX_WAITING_CALLS`] calls wait
//! at once, `windows`, `events`, `clipboard get`, `spawn` waiting for its seat's accessibility
//! bus and `accessibility` together; at most [`MAX_ENCODERS`] captures are encoded at once. The
//! protocol is in `protocol.rs`, the framing in `stdio.rs`, the tools' schemas and results in
//! `tools.rs`, the programs in `programs.rs`, the seats' events in `events.rs`.
//!
//! What happens on agents' seats is recorded once a round in each seat's log, `events.rs`: the
//! windows and outputs as the compositor describes them, compared with the last look; the
//! clients the display ended, with the program each belongs to and the protocol error it was
//! ended for, which the program keeps for `spawn list`; the programs reaped. An `events` call
//! reads it from the agent's cursor, or waits, as `windows` does, for a first event that
//! matches, answered at the end of the round that recorded it.
//!
//! A panic of the server's thread is fatal: the program's panic hook ends the programs and
//! removes their folder ([`Server::programs_folder`], [`end_after_panic`]), then stops Yuna. In a
//! debug build, `PANIC_ON_TOOL` makes a tool's call panic, for the tests of that path.

mod accessibility;
mod events;
mod image;
mod programs;
mod protocol;
mod stdio;
mod tools;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use smithay::reexports::calloop::channel::{self, Sender};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, Mode, PostAction, RegistrationToken};
use smithay::reexports::rustix;
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::rustix::process::{Pid, Signal};
use yuna_layout::{OutputId, SeatId, WindowId};

use crate::colour::description::PG32UCDM;
use crate::compositor::{Compositor, WindowDescription};
use crate::output::{InvalidOutput, OutputMode};
use crate::selection::SelectionRead;
use crate::state::OWNER_SEAT;
use crate::xwayland::Selection;
use crate::{
    CLIPBOARD_TIMEOUT, CaptureMapping, CaptureTicket, CapturedFrame, ClockMode, Error,
    KeyboardLayout,
};
use accessibility::{Accessibility, BusState, Outcome};
use events::{EventData, EventLog, Kind, Observer};
use image::Encoded;
use programs::{End, Programs, Start, THROTTLE};
use protocol::{Call, Message, Revision, RpcError, Session};
use stdio::{LineReader, LineTooLong, OutputQueue};
use tools::{
    AccessibilityCall, ButtonCall, CaptureCall, ClipboardCall, ClipboardData, ClockCall,
    CopiedCapture, EventsCall, KeyboardCall, OutputsCall, PointerCall, SeatCall, SpawnCall,
    ToolResult, WindowsCall, WorkspacesCall,
};

pub use accessibility::{
    APP_DEADLINE, MAX_IN_FLIGHT, MAX_NODES, MAX_TEXT, REWALK_AFTER, REWALK_INTERVAL,
};
pub use events::{MAX_EVENT_TEXT, MAX_EVENTS, MAX_EVENTS_PER_REPLY};
pub use image::{MAX_IMAGE_BYTES, MAX_IMAGE_SIDE};
pub use programs::{
    ACCESSIBILITY_VARIABLES, BUS_VARIABLE, END_GRACE, KEPT_PREFIXES, KEPT_VARIABLES, KILL_WAIT,
    LOCK_FILE, LOG_HEAD, LOG_TAIL, MAX_KEPT, MAX_RUNNING, NO_ACCESSIBILITY_VARIABLES, PACE_PERIOD,
    SEAT_VARIABLE, end_after_panic,
};
pub use protocol::{LEGACY_REVISION, MODERN_REVISION};
pub use stdio::MAX_LINE;
pub use tools::{
    DEFAULT_CLIPBOARD_BYTES, DEFAULT_LOG_READ, MAX_CLIPBOARD_BYTES, MAX_CLIPBOARD_TEXT_BYTES,
    MAX_LOG_READ, MAX_WAIT, MAX_WAITING_CALLS,
};

/// How many bytes of replies may wait for the client before its input stops being read: 16 MiB,
/// room for a few captures (M1.5's C4) beside the replies to a burst of calls.
pub const MAX_QUEUED_OUTPUT: usize = 16 << 20;

/// How many of a seat's captures `pointer` may name: its last ones
/// ([ADR-0038](../../../../docs/adr/0038-captures-coordinates-windows-and-wayland.md)).
pub const RECENT_CAPTURES: usize = 16;

/// How many captures' images may be encoded at once, each on a thread of its own with its
/// pixels, about 33 MB at 4K's physical size. An encoder is counted when it starts: a capture
/// copied while this many run waits for one to end, and a capture asked meanwhile is refused.
pub const MAX_ENCODERS: usize = 4;

/// The variable that, in a debug build, names a tool whose every call makes the server's thread,
/// Yuna's state thread, panic: what the tests of Yuna's fatal path use
/// ([ADR-0080](../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)). It is
/// read when the server starts. A release build has neither the variable nor the panic.
#[cfg(debug_assertions)]
pub const PANIC_ON_TOOL: &str = "YUNA_PANIC_ON_TOOL";

/// How much of the client's input is read at a time.
const READ_SIZE: usize = 64 * 1024;

/// How long Yuna, stopping, still tries to write the replies the client has not read.
const LAST_WRITE_WAIT: Duration = Duration::from_secs(1);

/// What the event loop's sources noted during a round, read after it.
#[derive(Debug, Default)]
struct Events {
    input_readable: bool,
    children_ended: bool,
    /// The programs' throttle ended: their output is read again.
    throttle_ended: bool,
    stop: bool,
    /// The waiting calls whose time is up, by key.
    timed_out: Vec<u64>,
    /// The `clipboard get` calls whose pipe may be read, by key.
    clipboards_readable: Vec<u64>,
    /// The seat closings whose stage is up, by key.
    closings_due: Vec<u64>,
    /// The captures' images encoded, by key.
    encoded: Vec<(u64, Result<Encoded, String>)>,
}

/// A `windows` call waiting for a window.
#[derive(Debug)]
struct WindowWait {
    key: u64,
    id: Value,
    revision: Revision,
    call: WindowsCall,
    timer: RegistrationToken,
}

/// An `events` call waiting for a first event of `seat` after `after` that `kinds` take.
#[derive(Debug)]
struct EventWait {
    key: u64,
    id: Value,
    revision: Revision,
    seat: SeatId,
    after: u64,
    kinds: Option<Vec<Kind>>,
    timer: RegistrationToken,
}

/// A `spawn start` with accessibility, waiting for its seat's bus.
#[derive(Debug)]
struct SpawnWait {
    id: Value,
    revision: Revision,
    seat: SeatId,
    start: Start,
}

/// A `capture` call, until it is answered.
#[derive(Debug)]
struct CaptureWait {
    key: u64,
    id: Value,
    revision: Revision,
    seat: SeatId,
    ticket: CaptureTicket,
    stage: CaptureStage,
}

/// Where a capture is: its frame waited for, its image encoded, then held until its frame's
/// boundary ([ADR-0034](../../../../docs/adr/0034-captures.md)).
#[derive(Debug)]
enum CaptureStage {
    /// Waiting for its frame, at most [`MAX_WAIT`]: the timer that fails it.
    Copying(RegistrationToken),
    /// Copied: its image is made on an encoder's thread, or its frame waits for one among the
    /// server's `waiting_encoders`.
    Encoding(CopiedCapture),
    /// Encoded; `woken` once a timer will wake the loop at its boundary.
    Encoded {
        copied: CopiedCapture,
        result: Result<Encoded, String>,
        woken: bool,
    },
}

/// A `clipboard get` reading its selection from the program that copied it, until it is
/// answered.
#[derive(Debug)]
struct ClipboardRead {
    key: u64,
    id: Value,
    revision: Revision,
    seat: SeatId,
    /// The MIME types the selection offered, which the answer gives.
    mime_types: Vec<String>,
    /// The type read.
    mime: String,
    max_bytes: usize,
    /// The read end of the pipe the program writes into, non-blocking.
    pipe: OwnedFd,
    /// What came, up to one byte past `max_bytes`, which tells that the program had more.
    data: Vec<u8>,
    /// The source that wakes the loop when the pipe may be read, on a copy of it.
    source: RegistrationToken,
    /// The timer that answers it [`CLIPBOARD_TIMEOUT`] after it started.
    timer: RegistrationToken,
}

/// How a `clipboard get`'s read ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadEnd {
    /// The program closed its pipe: the data came whole.
    Closed,
    /// It wrote more than `max_bytes`.
    Truncated,
    /// Its time is up, with what came.
    TimedOut,
    /// The pipe failed.
    Failed(rustix::io::Errno),
}

/// A `seat close` waiting for the seat's processes to end.
#[derive(Debug)]
struct SeatClosing {
    key: u64,
    /// The request, unless cancelled: then the seat still closes, unanswered.
    reply: Option<(Value, Revision)>,
    seat: SeatId,
    /// Its clients' process ids, which find its processes.
    client_pids: Vec<i32>,
    /// The processes signalled, in the order they were found.
    signalled: Vec<Pid>,
    /// Whether SIGKILL was sent: the last stage.
    killed: bool,
    timer: RegistrationToken,
}

/// The MCP server around its compositor.
pub struct Server {
    compositor: Compositor,
    session: Session,
    input: OwnedFd,
    input_source: RegistrationToken,
    input_paused: bool,
    output: OwnedFd,
    /// The flags of `output` before Yuna made it non-blocking, given back when it stops.
    output_flags: OFlags,
    /// The source that wakes the loop when `output` can take more; none for a file.
    output_source: Option<RegistrationToken>,
    output_watched: bool,
    reader: LineReader,
    /// Lines read and not yet answered, while too many replies wait.
    lines: VecDeque<Vec<u8>>,
    read_buffer: Vec<u8>,
    queue: OutputQueue,
    events: Rc<RefCell<Events>>,
    waits: Vec<WindowWait>,
    event_waits: Vec<EventWait>,
    /// The agents' seats' events, and what the compositor described at the last look.
    event_log: EventLog,
    observer: Observer,
    /// The compositor's description generation at the last look; none before the first.
    looked_at: Option<u64>,
    /// How many looks described the windows and outputs.
    looks: u64,
    /// The programs reaped since the events were last recorded.
    program_exits: Vec<(SeatId, u64, End)>,
    /// The keys of waiting calls and of closings, never reused.
    next_wait: u64,
    closings: Vec<SeatClosing>,
    captures: Vec<CaptureWait>,
    clipboard_reads: Vec<ClipboardRead>,
    /// Where the encoders send the captures' images, by key.
    encoder_results: Sender<(u64, Result<Encoded, String>)>,
    /// Each seat's last captures, newest last, by id, which `pointer` takes.
    recent_captures: HashMap<SeatId, VecDeque<(u64, CaptureMapping)>>,
    /// The next capture's id: ids increase and are never reused.
    next_capture: u64,
    /// The encoders' threads running, those of captures cancelled included: counted when each
    /// starts, until its image comes back.
    encoders_running: usize,
    /// The frames of captures copied while [`MAX_ENCODERS`] encoders ran, by capture key, oldest
    /// first: each starts its encoder when one ends. One a seat at most, as its capture.
    waiting_encoders: VecDeque<(u64, CapturedFrame)>,
    /// The mode of a new seat's output, unless `seat create` says.
    seat_output: OutputMode,
    /// The programs, when the server may start some, with SIGCHLD's pipe.
    programs: Option<(Programs, OwnedFd)>,
    /// Whether the timer that ends the programs' throttle is armed.
    throttle_armed: bool,
    /// The seats' accessibility buses and the `accessibility` calls in flight.
    accessibility: Accessibility,
    /// The programs waiting for their seat's accessibility bus, in the order they were asked.
    spawn_waits: Vec<SpawnWait>,
    /// Why the server stops, once it does.
    stopping: Option<Stop>,
    /// The tool whose calls panic, from [`PANIC_ON_TOOL`].
    #[cfg(debug_assertions)]
    panic_on_tool: Option<String>,
}

/// Why the server stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The client closed its input: the end MCP's stdio asks for.
    InputClosed,
    /// SIGTERM or SIGINT came.
    Signal,
    /// The client broke the connection: a line too long, its input or output failing.
    Broken(String),
}

/// Whether epoll refused a descriptor because it cannot be watched: a regular file, or a device
/// such as `/dev/null`.
fn is_not_pollable(error: &smithay::reexports::calloop::Error) -> bool {
    matches!(error, smithay::reexports::calloop::Error::IoError(io)
        if io.raw_os_error() == Some(rustix::io::Errno::PERM.raw_os_error()))
}

impl Server {
    /// A server for `compositor`, reading the client's requests from `input` and writing replies
    /// to `output`, which it makes non-blocking until it stops. With `children`, the read end of
    /// a pipe that SIGCHLD writes to, in a process that is its children's subreaper, each seat
    /// gets a Wayland socket and `spawn` starts programs; their folder is made in
    /// `XDG_RUNTIME_DIR`. Without, a seat's clients connect only through
    /// [`Compositor::connect_client_to`], and `spawn` is refused.
    pub fn new(
        mut compositor: Compositor,
        input: OwnedFd,
        output: OwnedFd,
        children: Option<OwnedFd>,
    ) -> Result<Server, Error> {
        compositor.watch_client_departures();
        let failed = |what: &str, error: &dyn std::fmt::Display| {
            Error::EventLoop(format!("the MCP server's {what}: {error}"))
        };
        let output_flags = rustix::fs::fcntl_getfl(&output).map_err(|e| failed("output", &e))?;
        let events = Rc::new(RefCell::new(Events::default()));
        let handle = compositor.loop_handle();
        // A descriptor that cannot be watched, a regular file or `/dev/null`, is `None`.
        let watch = |fd: &OwnedFd, interest: Interest, note: fn(&mut Events)| {
            let fd = fd.try_clone().map_err(|e| failed("descriptors", &e))?;
            let events = events.clone();
            let inserted = handle.insert_source(
                Generic::new(fd, interest, Mode::Level),
                move |_, _, state| {
                    state.woke = true;
                    note(&mut events.borrow_mut());
                    Ok(PostAction::Continue)
                },
            );
            match inserted {
                Ok(token) => Ok(Some(token)),
                Err(error) if is_not_pollable(&error.error) => Ok(None),
                Err(error) => Err(failed("event loop", &error.error)),
            }
        };
        let input_source = watch(&input, Interest::READ, |events| {
            events.input_readable = true
        })?
        .ok_or_else(|| {
            Error::EventLoop(
                "the MCP server reads a pipe or a socket, an MCP client's, not a file".to_owned(),
            )
        })?;
        // Writable only wakes the loop: each round writes what waits. A file is never watched:
        // a write to it does not wait.
        let output_source = watch(&output, Interest::WRITE, |_| {})?;
        if let Some(source) = &output_source {
            handle
                .disable(source)
                .map_err(|e| failed("event loop", &e))?;
        }
        let programs = match children {
            None => None,
            Some(children) => {
                rustix::io::ioctl_fionbio(&children, true).map_err(|e| failed("SIGCHLD", &e))?;
                watch(&children, Interest::READ, |events| {
                    events.children_ended = true
                })?
                .ok_or_else(|| Error::EventLoop("SIGCHLD's pipe cannot be watched".to_owned()))?;
                let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
                    .map(PathBuf::from)
                    .filter(|folder| folder.is_absolute())
                    .ok_or_else(|| {
                        Error::Socket(
                            "XDG_RUNTIME_DIR is not set, or not an absolute path; the seats' \
                             sockets and the programs' logs go there"
                                .to_owned(),
                        )
                    })?;
                let programs =
                    Programs::new(&runtime_dir).map_err(|e| failed("programs' folder", &e))?;
                Some((programs, children))
            }
        };
        let (encoder_results, encoded) = channel::channel();
        let noted = events.clone();
        handle
            .insert_source(encoded, move |event, _, state| {
                state.woke = true;
                if let channel::Event::Msg(message) = event {
                    noted.borrow_mut().encoded.push(message);
                }
            })
            .map_err(|e| failed("event loop", &e.error))?;
        // Last, so that a server that fails to start leaves its caller's output as it was.
        rustix::fs::fcntl_setfl(&output, output_flags | OFlags::NONBLOCK)
            .map_err(|e| failed("output", &e))?;
        Ok(Server {
            compositor,
            session: Session::default(),
            input,
            input_source,
            input_paused: false,
            output,
            output_flags,
            output_source,
            output_watched: false,
            reader: LineReader::default(),
            lines: VecDeque::new(),
            read_buffer: vec![0; READ_SIZE],
            queue: OutputQueue::default(),
            events,
            waits: Vec::new(),
            event_waits: Vec::new(),
            event_log: EventLog::default(),
            observer: Observer::default(),
            looked_at: None,
            looks: 0,
            program_exits: Vec::new(),
            next_wait: 1,
            closings: Vec::new(),
            captures: Vec::new(),
            clipboard_reads: Vec::new(),
            encoder_results,
            recent_captures: HashMap::new(),
            next_capture: 1,
            encoders_running: 0,
            waiting_encoders: VecDeque::new(),
            seat_output: OutputMode::default(),
            programs,
            throttle_armed: false,
            accessibility: Accessibility::new(handle.clone()),
            spawn_waits: Vec::new(),
            stopping: None,
            #[cfg(debug_assertions)]
            panic_on_tool: std::env::var(PANIC_ON_TOOL).ok(),
        })
    }

    /// Sets the mode of a new seat's output when `seat create` gives none: `yuna mcp --output`.
    pub fn set_seat_output(&mut self, mode: OutputMode) {
        self.seat_output = mode;
    }

    /// The compositor it serves.
    pub fn compositor(&mut self) -> &mut Compositor {
        &mut self.compositor
    }

    /// Bytes of replies waiting for the client. For tests.
    #[doc(hidden)]
    pub fn queued_output(&self) -> usize {
        self.queue.len()
    }

    /// Whether the client's input is not read, while its replies wait. For tests.
    #[doc(hidden)]
    pub fn input_paused(&self) -> bool {
        self.input_paused
    }

    /// How many encoders' threads run, as the server counts them. For tests.
    #[doc(hidden)]
    pub fn encoders_running(&self) -> usize {
        self.encoders_running
    }

    /// How many times the server described the compositor's windows and outputs to tell their
    /// events: once for each look after something they show may have changed. For tests.
    #[doc(hidden)]
    pub fn event_looks(&self) -> u64 {
        self.looks
    }

    /// Records the seats' events now, as the end of each round does. For measures.
    #[doc(hidden)]
    pub fn record_events_now(&mut self) {
        self.record_events();
    }

    /// The folder of its seats' sockets and its programs' logs, `yuna-mcp-<pid>` in
    /// `XDG_RUNTIME_DIR`, when it may start programs: what the fatal path removes.
    pub fn programs_folder(&self) -> Option<&Path> {
        self.programs
            .as_ref()
            .map(|(programs, _)| programs.folder())
    }

    /// Why it stops, once it does: nothing more is read, and [`Server::finish`] ends it.
    pub fn stopping(&self) -> Option<&Stop> {
        self.stopping.as_ref()
    }

    /// Runs one round of the compositor's loop, waiting up to `timeout` (`None`: as long as it
    /// takes), then answers what came: requests, cancellations, ended programs, clipboard reads
    /// whose data came, calls whose time is up or whose window opened; records the round's events
    /// and answers the calls waiting for them; then writes the replies.
    pub fn dispatch(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        let wakeups = self.compositor.counters().wakeups;
        self.compositor.dispatch(timeout)?;
        let events = std::mem::take(&mut *self.events.borrow_mut());
        if events.stop {
            self.stop(Stop::Signal);
        }
        if events.children_ended
            && let Some((programs, children)) = &mut self.programs
        {
            programs::drain(children.as_fd());
            let compositor = &mut self.compositor;
            let accessibility = &mut self.accessibility;
            let ended = programs.reap(|pid, status| {
                compositor.child_reaped(pid, status);
                accessibility.child_reaped(pid, status);
            });
            self.program_exits.extend(ended);
        }
        for key in events.closings_due {
            self.closing_stage_up(key);
        }
        self.answer_closings_done();
        self.throttle_programs(events.throttle_ended);
        if events.input_readable && self.stopping.is_none() && self.lines.is_empty() {
            self.read_input();
        }
        self.answer_lines();
        // Before the times up, so that a read whose data came is answered with it.
        self.read_clipboards(events.clipboards_readable);
        for key in events.timed_out {
            self.answer_timed_out(key);
        }
        // The encoders that ended first, so that the captures copied take their places.
        self.note_encoded(events.encoded);
        self.encode_copied_captures();
        self.answer_captures_due();
        // A window is told once it is placed: after the arrangement that follows its mapping.
        if !self.waits.is_empty() && !self.compositor.arrangement_pending() {
            self.answer_waits();
        }
        if self.accessibility.has_work() {
            let outcome = self.accessibility.service(&self.compositor);
            self.take_accessibility(outcome);
        }
        // What the calls changed reaches the compositor's clients now.
        self.compositor.flush();
        // Once the round has ended, the clients it freed included.
        self.record_events();
        self.answer_event_waits();
        self.write_output();
        self.compositor.log_counters_after(wakeups);
        Ok(())
    }

    /// Lets the programs' output be read again when their throttle has ended, and arms the next
    /// throttle while some output waits for one: one timer for all, and none otherwise.
    fn throttle_programs(&mut self, ended: bool) {
        let Some((programs, _)) = &self.programs else {
            return;
        };
        let handle = self.compositor.loop_handle();
        if ended {
            self.throttle_armed = false;
            programs.resume_throttled(&handle);
        }
        if self.throttle_armed || !programs.has_throttled() {
            return;
        }
        let events = self.events.clone();
        let armed = handle.insert_source(Timer::from_duration(THROTTLE), move |_, _, state| {
            state.woke = true;
            events.borrow_mut().throttle_ended = true;
            TimeoutAction::Drop
        });
        match armed {
            Ok(_) => self.throttle_armed = true,
            // Without a timer, the output is read again at once rather than never.
            Err(_) => programs.resume_throttled(&handle),
        }
    }

    /// Runs until the client closes its input, `stop` (the read end of a pipe SIGTERM and SIGINT
    /// write to) can be read, or the connection breaks; then ends the programs and returns why.
    pub fn run(mut self, stop: OwnedFd) -> Result<Stop, Error> {
        let events = self.events.clone();
        self.compositor
            .loop_handle()
            .insert_source(
                Generic::new(stop, Interest::READ, Mode::Level),
                move |_, _, state| {
                    state.woke = true;
                    events.borrow_mut().stop = true;
                    Ok(PostAction::Remove)
                },
            )
            .map_err(|e| Error::EventLoop(e.error.to_string()))?;
        let result = loop {
            if self.stopping.is_some() {
                break Ok(());
            }
            if let Err(error) = self.dispatch(None) {
                self.stop(Stop::Broken(format!("the compositor failed: {error}")));
                break Err(error);
            }
        };
        // Even when the loop failed: the programs end, and their folder goes.
        let stop = self.finish();
        result.map(|()| stop)
    }

    /// Ends the server: writes, for up to a second, the replies the client has not read, gives
    /// its output back its flags, ends every program and removes their folder. Returns why it
    /// stopped.
    pub fn finish(mut self) -> Stop {
        let handle = self.compositor.loop_handle();
        // Nothing more is read; what is left to write is written here.
        let _ = handle.disable(&self.input_source);
        if let Some(source) = &self.output_source {
            let _ = handle.disable(source);
        }
        // A closing still waiting is answered: Yuna's stopping ends what it waited for.
        let living = self.living_under_yuna();
        for closing in std::mem::take(&mut self.closings) {
            handle.remove(closing.timer);
            let left: Vec<Pid> = closing
                .signalled
                .iter()
                .copied()
                .filter(|pid| living.contains(pid))
                .collect();
            self.answer_closing(closing, &left);
        }
        let until = Instant::now() + LAST_WRITE_WAIT;
        while !self.queue.is_empty() && self.queue.write_to(&self.output).is_ok() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let mut fds = [rustix::event::PollFd::new(
                &self.output,
                rustix::event::PollFlags::OUT,
            )];
            let timeout = rustix::time::Timespec {
                tv_sec: left.as_secs() as i64,
                tv_nsec: i64::from(left.subsec_nanos()),
            };
            if !matches!(rustix::event::poll(&mut fds, Some(&timeout)), Ok(1..)) {
                break;
            }
        }
        let _ = rustix::fs::fcntl_setfl(&self.output, self.output_flags);
        // The buses' daemons end among every process under Yuna.
        self.accessibility.stop();
        if let Some((mut programs, children)) = self.programs.take() {
            self.end_programs(&mut programs, &children);
            programs.remove_folder();
        }
        let stop = self.stopping.take().unwrap_or(Stop::Signal);
        tracing::info!(reason = ?stop, "the MCP server stops");
        stop
    }

    /// Ends every process under Yuna: SIGTERM and SIGCONT, then SIGKILL to those left after
    /// [`END_GRACE`], each wait cut short once none is left. No program is paced any more, so
    /// that none is stopped again before it acts on SIGTERM. Meanwhile the compositor still
    /// serves its clients, so that a program answering SIGTERM with a last roundtrip gets its
    /// answer.
    fn end_programs(&mut self, programs: &mut Programs, children: &OwnedFd) {
        programs.unpace_all(&self.compositor.loop_handle());
        for (signal, wait) in [(Signal::TERM, END_GRACE), (Signal::KILL, KILL_WAIT)] {
            if programs::signal_all(signal) == 0 {
                return;
            }
            let until = Instant::now() + wait;
            loop {
                // A process may fork between the walk and its own SIGKILL: each look kills again.
                if signal == Signal::KILL {
                    programs::signal_all(Signal::KILL);
                }
                let compositor = &mut self.compositor;
                programs.reap(|pid, status| compositor.child_reaped(pid, status));
                if !programs::any_alive() {
                    return;
                }
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() || self.compositor.dispatch(Some(left)).is_err() {
                    break;
                }
                if std::mem::take(&mut *self.events.borrow_mut()).children_ended {
                    programs::drain(children.as_fd());
                }
            }
        }
    }

    fn stop(&mut self, reason: Stop) {
        if self.stopping.is_none() {
            self.stopping = Some(reason);
        }
    }

    /// Reads what the client sent, once, and answers each whole line.
    fn read_input(&mut self) {
        let read = match rustix::io::read(&self.input, &mut self.read_buffer) {
            Ok(0) => {
                if self.reader.has_partial_line() {
                    tracing::warn!("the client's input ends inside a line");
                }
                self.compositor.loop_handle().remove(self.input_source);
                self.stop(Stop::InputClosed);
                return;
            }
            Ok(read) => read,
            Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => return,
            Err(error) => {
                self.stop(Stop::Broken(format!("its input failed: {error}")));
                return;
            }
        };
        let mut lines = Vec::new();
        let pushed = self.reader.push(&self.read_buffer[..read], &mut lines);
        self.lines.extend(lines);
        if let Err(LineTooLong) = pushed {
            tracing::error!(limit = MAX_LINE, "a line from the client is too long");
            self.stop(Stop::Broken(format!("a line passed {MAX_LINE} bytes")));
        }
    }

    /// Answers the lines read, in order, while fewer than [`MAX_QUEUED_OUTPUT`] bytes of
    /// replies wait; the rest wait for the client to read.
    fn answer_lines(&mut self) {
        while self.queue.len() < MAX_QUEUED_OUTPUT
            && let Some(line) = self.lines.pop_front()
        {
            self.answer_line(&line);
        }
    }

    fn answer_line(&mut self, line: &[u8]) {
        match protocol::read_message(line) {
            Err(response) => self.queue.push(&response),
            Ok(Message::Response) => {}
            Ok(Message::Notification { method, params }) => {
                if method == "notifications/cancelled"
                    && let Some(id) = protocol::cancelled_request(&params)
                {
                    let id = id.clone();
                    self.cancel(&id);
                }
            }
            Ok(Message::Request { id, method, params }) => {
                match self.session.route(&method, &params) {
                    Ok((revision, call)) => self.answer(id, revision, call),
                    Err(error) => self.queue.push(&protocol::error_response(id, &error)),
                }
            }
        }
    }

    fn answer(&mut self, id: Value, revision: Revision, call: Call) {
        let result = match call {
            Call::Discover => protocol::discover_result(),
            Call::Initialize => protocol::initialize_result(),
            Call::Ping => json!({}),
            Call::ListTools => protocol::tools_list_result(revision, tools::definitions()),
            Call::Tool { name, arguments } => {
                #[cfg(debug_assertions)]
                self.panic_if_asked(&name);
                let result = match name.as_str() {
                    "seat" => match self.seat(&id, revision, arguments) {
                        Some(result) => result,
                        // It waits for the seat's processes, and is answered later.
                        None => return,
                    },
                    "outputs" => self.outputs(arguments),
                    "workspaces" => self.workspaces(arguments),
                    "pointer" => self.pointer(arguments),
                    "keyboard" => self.keyboard(arguments),
                    "spawn" => match self.spawn(&id, revision, arguments) {
                        Some(result) => result,
                        // It waits for its seat's accessibility bus, and is answered later.
                        None => return,
                    },
                    "accessibility" => match self.accessibility_call(&id, revision, arguments) {
                        Some(result) => result,
                        // It walks or waits, and is answered later.
                        None => return,
                    },
                    "windows" => match self.windows(&id, revision, arguments) {
                        Some(result) => result,
                        // It waits, and is answered later.
                        None => return,
                    },
                    "events" => match self.events(&id, revision, arguments) {
                        Some(result) => result,
                        // It waits, and is answered later.
                        None => return,
                    },
                    "capture" => match self.capture(&id, revision, arguments) {
                        Some(result) => result,
                        // It waits for its frame, and is answered later.
                        None => return,
                    },
                    "clipboard" => match self.clipboard(&id, revision, arguments) {
                        Some(result) => result,
                        // It waits for its selection's data, and is answered later.
                        None => return,
                    },
                    "clock" => self.clock(arguments),
                    _ => {
                        let error = RpcError {
                            code: protocol::INVALID_PARAMS,
                            message: format!("Unknown tool: {name}"),
                            data: None,
                        };
                        self.queue.push(&protocol::error_response(id, &error));
                        return;
                    }
                };
                result.into_result(revision)
            }
        };
        self.queue.push(&protocol::response(id, result));
    }

    /// Panics when [`PANIC_ON_TOOL`] names `tool`: the state thread's panic that the tests of
    /// the fatal path ask for, in a debug build alone.
    #[cfg(debug_assertions)]
    fn panic_if_asked(&self, tool: &str) {
        if self.panic_on_tool.as_deref() == Some(tool) {
            panic!("{PANIC_ON_TOOL} asks Yuna's state thread to panic when `{tool}` is called");
        }
    }

    /// The agent seat `seat`, while it exists; a failure that says so otherwise.
    fn agent_seat(&self, seat: u64) -> Result<SeatId, ToolResult> {
        let id = SeatId(seat);
        if id != OWNER_SEAT && self.compositor.seats().contains(&id) {
            Ok(id)
        } else {
            Err(ToolResult::failed(format!(
                "no seat {seat}: create one with `seat create`"
            )))
        }
    }

    /// An output mode from a call's values, each defaulting to the seats' default output's, its
    /// HDR monitor included (ADR-0049). Its VRR range's least rate is the default only when the
    /// call gives no rate either: a range belongs with its rate, and a call that names its own
    /// rate asked for no VRR. `hdr: true` announces the PG32UCDM's colours, the one monitor Yuna
    /// knows; `false` none.
    fn mode_of(
        &self,
        width: Option<u32>,
        height: Option<u32>,
        rate: Option<f64>,
        scale: Option<f64>,
        vrr_min: Option<f64>,
        hdr: Option<bool>,
    ) -> Result<OutputMode, ToolResult> {
        let default = self.seat_output;
        let default_vrr = rate.is_none().then(|| default.vrr_min()).flatten();
        checked_mode(
            OutputMode::new(
                width.unwrap_or(default.width()),
                height.unwrap_or(default.height()),
                rate.unwrap_or(default.rate()),
                scale.unwrap_or(default.scale()),
            ),
            vrr_min.or(default_vrr),
            hdr,
            default,
        )
    }

    /// The mode `outputs configure` gives an output whose mode is `own`: each value the call
    /// leaves out is the output's own, its VRR range included when the call gives another rate,
    /// so that a change of rate alone keeps VRR on; `vrr_min: 0` runs it at a fixed rate; and
    /// `hdr` as [`Server::mode_of`] reads it.
    fn configured_mode(
        own: OutputMode,
        width: Option<u32>,
        height: Option<u32>,
        rate: Option<f64>,
        scale: Option<f64>,
        vrr_min: Option<f64>,
        hdr: Option<bool>,
    ) -> Result<OutputMode, ToolResult> {
        checked_mode(
            OutputMode::new(
                width.unwrap_or(own.width()),
                height.unwrap_or(own.height()),
                rate.unwrap_or(own.rate()),
                scale.unwrap_or(own.scale()),
            ),
            match vrr_min {
                Some(0.0) => None,
                Some(min) => Some(min),
                None => own.vrr_min(),
            },
            hdr,
            own,
        )
    }

    /// The mode of output `output` of `seat`; none for an unknown one or another seat's.
    fn output_mode(&self, seat: SeatId, output: OutputId) -> Option<OutputMode> {
        self.compositor
            .output_descriptions()
            .iter()
            .find(|description| description.id == output && description.seat == seat)
            .map(|description| description.mode)
    }

    /// The output `output` of `seat`, described; none for an unknown one or another seat's.
    fn output_of(&self, seat: SeatId, output: OutputId) -> Option<Value> {
        self.compositor
            .output_descriptions()
            .iter()
            .find(|description| description.id == output && description.seat == seat)
            .map(tools::output_json)
    }

    /// `seat`: the result now, or none when a closing waits for the seat's processes.
    fn seat(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        Some(match call {
            SeatCall::Create {
                width,
                height,
                rate,
                scale,
                vrr_min,
                hdr,
            } => match self.mode_of(width, height, rate, scale, vrr_min, hdr) {
                Ok(mode) => self.create_seat(mode),
                Err(failed) => failed,
            },
            SeatCall::List {} => {
                let seats: Vec<Value> = self
                    .compositor
                    .seats()
                    .into_iter()
                    .filter(|&seat| seat != OWNER_SEAT)
                    .map(|seat| self.seat_json(seat))
                    .collect();
                ToolResult::done(json!({ "seats": seats }))
            }
            SeatCall::Close { seat } => match self.agent_seat(seat) {
                Ok(seat) => return self.close_seat(id, revision, seat),
                Err(failed) => failed,
            },
        })
    }

    /// Creates a seat, its output and, with programs, its socket; nothing is left of it when a
    /// step fails.
    fn create_seat(&mut self, mode: OutputMode) -> ToolResult {
        let has_agent_seat = self
            .compositor
            .seats()
            .iter()
            .any(|&seat| seat != OWNER_SEAT);
        if self.compositor.clock_mode() == ClockMode::Manual && has_agent_seat {
            return ToolResult::failed(
                "a Yuna on a manual clock holds one seat, whose agent steps its clock: close \
                 it, or start another `yuna mcp`",
            );
        }
        let seat = match self.compositor.add_seat() {
            Ok(seat) => seat,
            Err(error) => return ToolResult::failed(error.to_string()),
        };
        let made = self
            .compositor
            .add_output(seat, mode)
            .and_then(|_| match &self.programs {
                Some((programs, _)) => {
                    let path = programs.socket_path(seat);
                    self.compositor.listen_for_seat(seat, path.clone())?;
                    Ok(Some(path))
                }
                None => Ok(None),
            });
        match made {
            Ok(_) => ToolResult::done(self.seat_json(seat)),
            Err(error) => {
                if let Err(undo) = self.compositor.remove_seat(seat) {
                    tracing::error!(%undo, seat = seat.0, "a seat half made is left");
                }
                ToolResult::failed(error.to_string())
            }
        }
    }

    /// A seat, as the tools give it: its handle, its outputs, its socket, its windows' and
    /// running programs' counts.
    fn seat_json(&self, seat: SeatId) -> Value {
        let outputs: Vec<Value> = self
            .compositor
            .output_descriptions()
            .iter()
            .filter(|output| output.seat == seat)
            .map(tools::output_json)
            .collect();
        let windows = self
            .compositor
            .windows()
            .iter()
            .filter(|window| window.seat == seat)
            .count();
        let (socket, running) = match &self.programs {
            Some((programs, _)) => (
                Some(programs.socket_path(seat)),
                programs
                    .list(seat)
                    .iter()
                    .filter(|program| program.end.is_none())
                    .count(),
            ),
            None => (None, 0),
        };
        json!({
            "seat": seat.0,
            "outputs": outputs,
            "socket": socket,
            "windows": windows,
            "programs_running": running,
        })
    }

    /// Closes `seat`: what the compositor holds of it at once, then its processes, SIGTERM
    /// first. Answered now when none is left, else once they end (`answer_closings_done`).
    fn close_seat(&mut self, id: &Value, revision: Revision, seat: SeatId) -> Option<ToolResult> {
        let daemon = self.accessibility.daemon_pid(seat);
        let mut client_pids = match self.compositor.remove_seat(seat) {
            Ok(pids) => pids,
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        // The accessibility bus's daemon is one of the seat's processes.
        client_pids.extend(daemon.map(|pid| pid.as_raw_nonzero().get()));
        self.fail_waits_of(seat);
        let handle = self.compositor.loop_handle();
        let processes = match &mut self.programs {
            Some((programs, _)) => {
                programs.unpace_seat(&handle, seat);
                programs.seat_processes(seat, &client_pids)
            }
            None => Vec::new(),
        };
        if processes.is_empty() {
            return Some(self.seat_closed(seat, &[], &[]));
        }
        for &pid in &processes {
            programs::send_ending_signal(pid, Signal::TERM);
        }
        tracing::info!(
            seat = seat.0,
            processes = processes.len(),
            "ending a seat's processes"
        );
        let key = self.next_wait;
        let timer = match self.arm_closing_stage(key, END_GRACE) {
            Ok(timer) => timer,
            Err(error) => {
                // Without a timer, nothing would send SIGKILL: send it now.
                for &pid in &processes {
                    let _ = rustix::process::kill_process(pid, Signal::KILL);
                }
                tracing::warn!(%error, "a seat's processes are killed at once");
                return Some(self.seat_closed(seat, &processes, &[]));
            }
        };
        self.next_wait += 1;
        self.closings.push(SeatClosing {
            key,
            reply: Some((id.clone(), revision)),
            seat,
            client_pids,
            signalled: processes,
            killed: false,
            timer,
        });
        None
    }

    /// Answers the `windows`, `events`, `capture`, `clipboard`, `accessibility` calls and the
    /// `spawn` starts waiting for its bus that wait on `seat`, which closed, and forgets its
    /// captures, its events and its accessibility bus: a stale handle fails plainly
    /// ([ADR-0078](../../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)).
    fn fail_waits_of(&mut self, seat: SeatId) {
        self.recent_captures.remove(&seat);
        self.event_log.remove_seat(seat);
        let handle = self.compositor.loop_handle();
        let (closed, waiting): (Vec<EventWait>, Vec<EventWait>) =
            std::mem::take(&mut self.event_waits)
                .into_iter()
                .partition(|wait| wait.seat == seat);
        self.event_waits = waiting;
        for wait in closed {
            handle.remove(wait.timer);
            let result = ToolResult::failed(format!("seat {} was closed", seat.0));
            self.queue.push(&protocol::response(
                wait.id,
                result.into_result(wait.revision),
            ));
        }
        while let Some(index) = self
            .clipboard_reads
            .iter()
            .position(|read| read.seat == seat)
        {
            let read = self.drop_clipboard_read(index);
            let result = ToolResult::failed(format!("seat {} was closed", seat.0));
            self.queue.push(&protocol::response(
                read.id,
                result.into_result(read.revision),
            ));
        }
        for (id, revision, result) in self.accessibility.remove_seat(seat) {
            self.queue
                .push(&protocol::response(id, result.into_result(revision)));
        }
        self.fail_spawn_waits(seat, &format!("seat {} was closed", seat.0));
        while let Some(index) = self
            .captures
            .iter()
            .position(|capture| capture.seat == seat)
        {
            let capture = self.drop_capture(index);
            let result = ToolResult::failed(format!("seat {} was closed", seat.0));
            self.queue.push(&protocol::response(
                capture.id,
                result.into_result(capture.revision),
            ));
        }
        let mut index = 0;
        while index < self.waits.len() {
            if self.waits[index].call.seat != seat.0 {
                index += 1;
                continue;
            }
            let wait = self.waits.remove(index);
            handle.remove(wait.timer);
            let result = ToolResult::failed(format!("seat {} was closed", seat.0));
            self.queue.push(&protocol::response(
                wait.id,
                result.into_result(wait.revision),
            ));
        }
    }

    /// Arms the timer that ends a closing's stage `after` from now.
    fn arm_closing_stage(
        &self,
        key: u64,
        after: Duration,
    ) -> Result<RegistrationToken, smithay::reexports::calloop::Error> {
        let events = self.events.clone();
        self.compositor
            .loop_handle()
            .insert_source(Timer::from_duration(after), move |_, _, state| {
                state.woke = true;
                events.borrow_mut().closings_due.push(key);
                TimeoutAction::Drop
            })
            .map_err(|e| e.error)
    }

    /// A closing's stage is up: after SIGTERM, SIGKILL goes to every process of the seat still
    /// there, those it started meanwhile included; after SIGKILL, the closing is answered with
    /// what is left.
    fn closing_stage_up(&mut self, key: u64) {
        let Some(index) = self.closings.iter().position(|closing| closing.key == key) else {
            return;
        };
        let living = self.living_under_yuna();
        if self.closings[index].killed {
            let closing = self.closings.remove(index);
            let left: Vec<Pid> = closing
                .signalled
                .iter()
                .copied()
                .filter(|pid| living.contains(pid))
                .collect();
            self.answer_closing(closing, &left);
            return;
        }
        let closing = &self.closings[index];
        // Only processes still under Yuna: a pid that left it may be another's now.
        let mut targets: Vec<Pid> = closing
            .signalled
            .iter()
            .copied()
            .filter(|pid| living.contains(pid))
            .collect();
        if let Some((programs, _)) = &self.programs {
            for pid in programs.seat_processes(closing.seat, &closing.client_pids) {
                if !targets.contains(&pid) {
                    targets.push(pid);
                }
            }
        }
        for &pid in &targets {
            let _ = rustix::process::kill_process(pid, Signal::KILL);
        }
        let timer = self.arm_closing_stage(key, KILL_WAIT);
        let closing = &mut self.closings[index];
        for pid in targets {
            if !closing.signalled.contains(&pid) {
                closing.signalled.push(pid);
            }
        }
        closing.killed = true;
        match timer {
            Ok(timer) => closing.timer = timer,
            Err(_) => {
                let closing = self.closings.remove(index);
                self.answer_closing(closing, &[]);
            }
        }
    }

    /// The processes living under Yuna; none without programs, in a test's process, whose
    /// children are not the server's.
    fn living_under_yuna(&self) -> Vec<Pid> {
        if self.programs.is_some() {
            programs::living()
        } else {
            Vec::new()
        }
    }

    /// Answers each closing whose processes have all ended, or left Yuna.
    fn answer_closings_done(&mut self) {
        if self.closings.is_empty() {
            return;
        }
        let living = self.living_under_yuna();
        let mut index = 0;
        while index < self.closings.len() {
            if self.closings[index]
                .signalled
                .iter()
                .any(|pid| living.contains(pid))
            {
                index += 1;
                continue;
            }
            let closing = self.closings.remove(index);
            self.compositor.loop_handle().remove(closing.timer);
            self.answer_closing(closing, &[]);
        }
    }

    fn answer_closing(&mut self, closing: SeatClosing, left: &[Pid]) {
        let result = self.seat_closed(closing.seat, &closing.signalled, left);
        if let Some((id, revision)) = closing.reply {
            self.queue
                .push(&protocol::response(id, result.into_result(revision)));
        }
    }

    /// A closed seat's result; its programs are forgotten, their logs deleted.
    fn seat_closed(&mut self, seat: SeatId, ended: &[Pid], left: &[Pid]) -> ToolResult {
        if let Some((programs, _)) = &mut self.programs {
            programs.forget_seat(seat);
        }
        let raw = |pid: &Pid| pid.as_raw_nonzero().get();
        let ended: Vec<i32> = ended
            .iter()
            .filter(|pid| !left.contains(pid))
            .map(raw)
            .collect();
        let left: Vec<i32> = left.iter().map(raw).collect();
        tracing::info!(seat = seat.0, left = left.len(), "seat closed");
        ToolResult::done(json!({
            "closed": seat.0,
            "ended": ended,
            "left_running": left,
        }))
    }

    fn outputs(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        match call {
            OutputsCall::List { seat } => {
                let seat = match self.agent_seat(seat) {
                    Ok(seat) => seat,
                    Err(failed) => return failed,
                };
                let outputs: Vec<Value> = self
                    .compositor
                    .output_descriptions()
                    .iter()
                    .filter(|output| output.seat == seat)
                    .map(tools::output_json)
                    .collect();
                ToolResult::done(json!({ "outputs": outputs }))
            }
            OutputsCall::Create {
                seat,
                width,
                height,
                rate,
                scale,
                vrr_min,
                hdr,
            } => {
                let seat = match self.agent_seat(seat) {
                    Ok(seat) => seat,
                    Err(failed) => return failed,
                };
                let mode = match self.mode_of(width, height, rate, scale, vrr_min, hdr) {
                    Ok(mode) => mode,
                    Err(failed) => return failed,
                };
                match self.compositor.add_output(seat, mode) {
                    Ok(id) => ToolResult::done(json!({ "output": self.output_of(seat, id) })),
                    Err(error) => ToolResult::failed(error.to_string()),
                }
            }
            OutputsCall::Configure {
                seat,
                output,
                width,
                height,
                rate,
                scale,
                vrr_min,
                hdr,
            } => {
                let seat = match self.agent_seat(seat) {
                    Ok(seat) => seat,
                    Err(failed) => return failed,
                };
                let id = OutputId(output);
                let Some(own) = self.output_mode(seat, id) else {
                    return ToolResult::failed(format!("no output {output} on seat {}", seat.0));
                };
                let mode =
                    match Server::configured_mode(own, width, height, rate, scale, vrr_min, hdr) {
                        Ok(mode) => mode,
                        Err(failed) => return failed,
                    };
                match self.compositor.configure_output(id, mode) {
                    Ok(()) => ToolResult::done(json!({ "output": self.output_of(seat, id) })),
                    Err(error) => ToolResult::failed(error.to_string()),
                }
            }
            OutputsCall::Remove { seat, output } => {
                let seat = match self.agent_seat(seat) {
                    Ok(seat) => seat,
                    Err(failed) => return failed,
                };
                if self.output_of(seat, OutputId(output)).is_none() {
                    return ToolResult::failed(format!("no output {output} on seat {}", seat.0));
                }
                match self.compositor.remove_output(OutputId(output)) {
                    Ok(()) => ToolResult::done(json!({ "removed": output })),
                    Err(error) => ToolResult::failed(error.to_string()),
                }
            }
        }
    }

    fn workspaces(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call: WorkspacesCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        let seat = match &call {
            WorkspacesCall::List { seat }
            | WorkspacesCall::Create { seat, .. }
            | WorkspacesCall::Remove { seat, .. }
            | WorkspacesCall::Focus { seat, .. }
            | WorkspacesCall::MoveWindow { seat, .. } => *seat,
        };
        let seat = match self.agent_seat(seat) {
            Ok(seat) => seat,
            Err(failed) => return failed,
        };
        let changed = match call {
            WorkspacesCall::List { .. } => Ok(()),
            WorkspacesCall::Create { name, output, .. } => {
                let output = output.map(OutputId);
                if let Some(output) = output
                    && self.output_of(seat, output).is_none()
                {
                    return ToolResult::failed(format!(
                        "no output {} on seat {}",
                        output.0, seat.0
                    ));
                }
                self.compositor
                    .change_layout(|layout| layout.add_workspace(seat, &name, output))
            }
            WorkspacesCall::Remove { name, .. } => self
                .compositor
                .change_layout(|layout| layout.remove_workspace(seat, &name)),
            WorkspacesCall::Focus { name, .. } => self
                .compositor
                .change_layout(|layout| layout.focus_workspace(seat, &name)),
            WorkspacesCall::MoveWindow {
                window, workspace, ..
            } => {
                let window = WindowId(window);
                let ours = self
                    .compositor
                    .windows()
                    .iter()
                    .any(|description| description.id == window && description.seat == seat);
                if !ours {
                    return ToolResult::failed(format!(
                        "no window {} on seat {}",
                        window.0, seat.0
                    ));
                }
                self.compositor.change_layout(|layout| {
                    layout.focus_window(window)?;
                    layout.move_to_workspace(seat, &workspace)
                })
            }
        };
        if let Err(error) = changed {
            return ToolResult::failed(error.to_string());
        }
        let outputs: Vec<Value> = self
            .compositor
            .output_descriptions()
            .iter()
            .filter(|output| output.seat == seat)
            .map(|output| {
                let workspaces: Vec<Value> = self
                    .compositor
                    .layout()
                    .workspaces(output.id)
                    .unwrap_or_default()
                    .iter()
                    .map(tools::workspace_json)
                    .collect();
                json!({ "output": output.id.0, "workspaces": workspaces })
            })
            .collect();
        ToolResult::done(json!({ "outputs": outputs }))
    }

    fn pointer(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call: PointerCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        let seat = match &call {
            PointerCall::Get { seat }
            | PointerCall::Move { seat, .. }
            | PointerCall::Scroll { seat, .. } => *seat,
            PointerCall::Press(button)
            | PointerCall::Release(button)
            | PointerCall::Click(button) => button.seat,
        };
        let seat = match self.agent_seat(seat) {
            Ok(seat) => seat,
            Err(failed) => return failed,
        };
        let result = match call {
            PointerCall::Get { .. } => Ok(self.compositor.pointer(seat)),
            PointerCall::Move {
                x,
                y,
                output,
                capture,
                ..
            } => self
                .place(seat, x, y, output, capture)
                .and_then(|(output, x, y)| self.compositor.pointer_move(seat, output, x, y)),
            PointerCall::Press(button) => self.move_first(seat, &button).and_then(|()| {
                self.compositor
                    .pointer_button(seat, button.button.code(), true)
            }),
            PointerCall::Release(button) => self.move_first(seat, &button).and_then(|()| {
                self.compositor
                    .pointer_button(seat, button.button.code(), false)
            }),
            PointerCall::Click(button) => self.move_first(seat, &button).and_then(|()| {
                let code = button.button.code();
                self.compositor.pointer_button(seat, code, true)?;
                self.compositor.pointer_button(seat, code, false)
            }),
            PointerCall::Scroll {
                dx,
                dy,
                x,
                y,
                output,
                capture,
                ..
            } => self
                .move_to(seat, x, y, output, capture)
                .and_then(|()| self.compositor.pointer_scroll(seat, dx, dy)),
        };
        match result {
            Ok(pointer) => ToolResult::done(json!({ "pointer": tools::pointer_json(&pointer) })),
            Err(error) => ToolResult::failed(error.to_string()),
        }
    }

    /// Moves `seat`'s pointer where a button call says, when it says.
    fn move_first(&mut self, seat: SeatId, button: &ButtonCall) -> Result<(), Error> {
        self.move_to(seat, button.x, button.y, button.output, button.capture)
    }

    /// Moves `seat`'s pointer to (`x`, `y`), on `output` or in `capture`'s image, when both are
    /// given; refused for one without the other.
    fn move_to(
        &mut self,
        seat: SeatId,
        x: Option<f64>,
        y: Option<f64>,
        output: Option<u64>,
        capture: Option<u64>,
    ) -> Result<(), Error> {
        match (x, y) {
            (Some(x), Some(y)) => {
                let (output, x, y) = self.place(seat, x, y, output, capture)?;
                self.compositor.pointer_move(seat, output, x, y).map(|_| ())
            }
            (None, None) if output.is_none() && capture.is_none() => Ok(()),
            _ => Err(Error::Refused("give x and y together".to_owned())),
        }
    }

    /// The output and logical place a call's (`x`, `y`) name: on `output`, or, with `capture`,
    /// a point of that capture's image, mapped to its output. Refused for both given, a capture
    /// not among the seat's last [`RECENT_CAPTURES`], and a point off its image.
    fn place(
        &self,
        seat: SeatId,
        x: f64,
        y: f64,
        output: Option<u64>,
        capture: Option<u64>,
    ) -> Result<(Option<OutputId>, f64, f64), Error> {
        let Some(capture) = capture else {
            return Ok((output.map(OutputId), x, y));
        };
        if output.is_some() {
            return Err(Error::Refused(
                "give a capture or an output, not both".to_owned(),
            ));
        }
        let mapping = self
            .recent_captures
            .get(&seat)
            .and_then(|recent| recent.iter().find(|(id, _)| *id == capture))
            .map(|(_, mapping)| *mapping)
            .ok_or_else(|| {
                Error::Refused(format!(
                    "no capture {capture} among seat {}'s last {RECENT_CAPTURES}: capture again",
                    seat.0
                ))
            })?;
        let (logical_x, logical_y) = mapping.to_logical(x, y).ok_or_else(|| {
            Error::Refused(format!(
                "({x}, {y}) is off capture {capture}'s {} × {} pixels",
                mapping.width, mapping.height
            ))
        })?;
        Ok((Some(mapping.output), logical_x, logical_y))
    }

    fn keyboard(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call: KeyboardCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        let seat = match &call {
            KeyboardCall::Get { seat }
            | KeyboardCall::Press { seat, .. }
            | KeyboardCall::Release { seat, .. }
            | KeyboardCall::Tap { seat, .. }
            | KeyboardCall::Type { seat, .. }
            | KeyboardCall::Preedit { seat, .. }
            | KeyboardCall::Layout { seat, .. } => *seat,
        };
        let tells_layout = matches!(call, KeyboardCall::Get { .. } | KeyboardCall::Layout { .. });
        let seat = match self.agent_seat(seat) {
            Ok(seat) => seat,
            Err(failed) => return failed,
        };
        let result = match call {
            KeyboardCall::Get { .. } => Ok(()),
            KeyboardCall::Press { key, .. } => self.compositor.key(seat, &key, true),
            KeyboardCall::Release { key, .. } => self.compositor.key(seat, &key, false),
            KeyboardCall::Tap { keys, .. } => self.compositor.tap_keys(seat, &keys),
            KeyboardCall::Type { text, .. } => self.compositor.type_text(seat, &text),
            KeyboardCall::Preedit { text, .. } => self.compositor.compose_text(seat, &text),
            KeyboardCall::Layout {
                layout,
                variant,
                options,
                ..
            } => {
                let layout = KeyboardLayout {
                    layout,
                    variant: variant.unwrap_or_default(),
                    options: options.unwrap_or_default(),
                };
                self.compositor.set_keyboard_layout(seat, layout)
            }
        };
        if let Err(error) = result {
            return ToolResult::failed(error.to_string());
        }
        let mut told = json!({
            "keyboard_window": self.compositor.keyboard_window(seat).map(|window| window.0),
            "text_field": self.compositor.text_field_enabled(seat),
        });
        if tells_layout && let Some(layout) = self.compositor.keyboard_layout(seat) {
            told["layout"] = json!(layout.layout);
            told["variant"] = json!(layout.variant);
            told["options"] = json!(layout.options);
        }
        ToolResult::done(told)
    }

    /// `spawn`: the result now, or none when a start waits for its seat's accessibility bus.
    fn spawn(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        if let SpawnCall::Start {
            seat,
            accessibility: true,
            ..
        } = &call
        {
            let seat = SeatId(*seat);
            if seat != OWNER_SEAT && self.compositor.seats().contains(&seat) {
                return self.start_accessible(id, revision, seat, call);
            }
        }
        Some(self.spawn_now(call))
    }

    /// A `spawn start` with accessibility on `seat`: started now when its bus is ready; else it
    /// waits for the bus, started first when there is none.
    fn start_accessible(
        &mut self,
        id: &Value,
        revision: Revision,
        seat: SeatId,
        call: SpawnCall,
    ) -> Option<ToolResult> {
        let SpawnCall::Start {
            command, env, cwd, ..
        } = call
        else {
            return Some(ToolResult::failed("not a start"));
        };
        let Some((programs, _)) = &self.programs else {
            return Some(ToolResult::failed(
                "this Yuna starts no program: run `yuna mcp`",
            ));
        };
        let mut start = Start {
            command,
            env,
            cwd: cwd.map(PathBuf::from),
            accessibility: None,
        };
        let state = self.accessibility.bus_state(seat);
        if let BusState::Ready(address) = state {
            start.accessibility = Some(address);
            return Some(self.start_program(seat, start));
        }
        if let BusState::Ending(reason) = state {
            return Some(ToolResult::failed(format!(
                "seat {}'s accessibility bus is being ended ({reason}): start the program again \
                 in a moment, and a new bus starts for it",
                seat.0
            )));
        }
        // Refused before a bus starts for it.
        if self.waiting_calls() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        if state == BusState::Absent {
            let folder = programs.accessibility_folder(seat);
            let environment = programs::daemon_environment(std::env::vars_os(), seat);
            let handle = self.compositor.loop_handle();
            let log = |path: &std::path::Path| programs.bounded_log(&handle, path);
            if let Err(reason) = self.accessibility.start_bus(seat, folder, environment, log) {
                return Some(ToolResult::failed(reason));
            }
        }
        self.spawn_waits.push(SpawnWait {
            id: id.clone(),
            revision,
            seat,
            start,
        });
        None
    }

    /// Starts `start` on `seat` and tells it.
    fn start_program(&mut self, seat: SeatId, start: Start) -> ToolResult {
        let Some((programs, _)) = &mut self.programs else {
            return ToolResult::failed("this Yuna starts no program: run `yuna mcp`");
        };
        let handle = self.compositor.loop_handle();
        let socket = programs.socket_path(seat);
        let x11 = self.compositor.x11_display(seat);
        match programs.start(&handle, seat, &socket, x11.as_ref(), start) {
            Ok(program) => ToolResult::done(json!({ "program": tools::program_json(program) })),
            Err(reason) => ToolResult::failed(reason),
        }
    }

    /// Answers what the accessibility buses and calls did this round: the programs waiting for a
    /// bus that became ready are started, those of a bus that ended fail.
    fn take_accessibility(&mut self, outcome: Outcome) {
        for (id, revision, result) in outcome.answers {
            self.queue
                .push(&protocol::response(id, result.into_result(revision)));
        }
        for seat in outcome.ready {
            let BusState::Ready(address) = self.accessibility.bus_state(seat) else {
                continue;
            };
            let mut index = 0;
            while index < self.spawn_waits.len() {
                if self.spawn_waits[index].seat != seat {
                    index += 1;
                    continue;
                }
                let mut wait = self.spawn_waits.remove(index);
                wait.start.accessibility = Some(address.clone());
                let result = self.start_program(seat, wait.start);
                self.queue.push(&protocol::response(
                    wait.id,
                    result.into_result(wait.revision),
                ));
            }
        }
        for (seat, reason) in outcome.ended {
            self.fail_spawn_waits(seat, &reason);
        }
    }

    /// Answers the programs waiting for `seat`'s accessibility bus with `reason`, failed.
    fn fail_spawn_waits(&mut self, seat: SeatId, reason: &str) {
        let mut index = 0;
        while index < self.spawn_waits.len() {
            if self.spawn_waits[index].seat != seat {
                index += 1;
                continue;
            }
            let wait = self.spawn_waits.remove(index);
            let result = ToolResult::failed(reason);
            self.queue.push(&protocol::response(
                wait.id,
                result.into_result(wait.revision),
            ));
        }
    }

    /// `accessibility`: a failure now, or none while it walks or waits.
    fn accessibility_call(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call: AccessibilityCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        if let Err(failed) = self.agent_seat(call.seat) {
            return Some(failed);
        }
        if self.programs.is_none() {
            return Some(ToolResult::failed(
                "this Yuna starts no program: run `yuna mcp`",
            ));
        }
        if self.waiting_calls() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        self.accessibility
            .call(id, revision, call, &self.compositor)
    }

    /// `spawn`'s list, stop, and start without accessibility: answered now.
    fn spawn_now(&mut self, call: SpawnCall) -> ToolResult {
        let Some((programs, _)) = &mut self.programs else {
            return ToolResult::failed("this Yuna starts no program: run `yuna mcp`");
        };
        let seat = match &call {
            SpawnCall::Start { seat, .. }
            | SpawnCall::List { seat }
            | SpawnCall::Stop { seat, .. }
            | SpawnCall::Log { seat, .. }
            | SpawnCall::Freeze { seat, .. }
            | SpawnCall::Thaw { seat, .. }
            | SpawnCall::Slow { seat, .. } => *seat,
        };
        let seat = SeatId(seat);
        if seat == OWNER_SEAT || !self.compositor.seats().contains(&seat) {
            return ToolResult::failed(format!(
                "no seat {}: create one with `seat create`",
                seat.0
            ));
        }
        let handle = self.compositor.loop_handle();
        // A paced program, as its result tells it.
        let paced = |programs: &Programs, program: u64| {
            let listed = programs.program(seat, program).map(tools::program_json);
            ToolResult::done(json!({ "program": listed }))
        };
        match call {
            SpawnCall::Start {
                command, env, cwd, ..
            } => {
                let start = Start {
                    command,
                    env,
                    cwd: cwd.map(PathBuf::from),
                    accessibility: None,
                };
                self.start_program(seat, start)
            }
            SpawnCall::List { .. } => {
                let list: Vec<Value> = programs
                    .list(seat)
                    .into_iter()
                    .map(tools::program_json)
                    .collect();
                ToolResult::done(json!({ "programs": list }))
            }
            SpawnCall::Stop { program, force, .. } => {
                match programs.stop(&handle, seat, program, force) {
                    Ok(()) => ToolResult::done(json!({
                        "stopped": program,
                        "signal": if force { "SIGKILL" } else { "SIGTERM" },
                    })),
                    Err(reason) => ToolResult::failed(reason),
                }
            }
            SpawnCall::Log {
                program,
                from,
                max_bytes,
                ..
            } => {
                let max_bytes = max_bytes.unwrap_or(tools::DEFAULT_LOG_READ);
                if !(1..=tools::MAX_LOG_READ).contains(&max_bytes) {
                    return ToolResult::failed(format!(
                        "max_bytes is from 1 to {}",
                        tools::MAX_LOG_READ
                    ));
                }
                let read = programs
                    .program(seat, program)
                    .ok_or_else(|| format!("no program {program} on seat {}", seat.0))
                    .and_then(|found| found.read_log(from, max_bytes));
                match read {
                    Ok(read) => ToolResult::done(tools::log_json(&read)),
                    Err(reason) => ToolResult::failed(reason),
                }
            }
            SpawnCall::Freeze { program, .. } => match programs.freeze(&handle, seat, program) {
                Ok(()) => paced(programs, program),
                Err(reason) => ToolResult::failed(reason),
            },
            SpawnCall::Thaw { program, .. } => match programs.thaw(&handle, seat, program) {
                Ok(()) => paced(programs, program),
                Err(reason) => ToolResult::failed(reason),
            },
            SpawnCall::Slow {
                program,
                run_percent,
                ..
            } => match programs.slow(&handle, seat, program, run_percent) {
                Ok(()) => paced(programs, program),
                Err(reason) => ToolResult::failed(reason),
            },
        }
    }

    /// `events`: the result now, or none when the call waits. What happened before it is
    /// recorded first, so that a call without `after` waits only for what comes after it.
    fn events(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call: EventsCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        let seat = match self.agent_seat(call.seat) {
            Ok(seat) => seat,
            Err(failed) => return Some(failed),
        };
        let wait = Duration::from_millis(call.wait_ms);
        if wait > MAX_WAIT {
            return Some(ToolResult::failed(format!(
                "wait_ms is at most {}",
                MAX_WAIT.as_millis()
            )));
        }
        if call.kinds.as_ref().is_some_and(Vec::is_empty) {
            return Some(ToolResult::failed(
                "kinds names one kind at least: leave it out for every kind",
            ));
        }
        self.record_events();
        let newest = self.event_log.last_seq();
        let after = match call.after {
            Some(after) if after > newest => {
                return Some(ToolResult::failed(format!(
                    "after {after} is past the newest event, {newest}: give a reply's next"
                )));
            }
            Some(after) => after,
            None => newest,
        };
        let reading = self.event_log.read(seat, after, call.kinds.as_deref());
        // A cursor whose events the log dropped is told at once: what it waits for may be
        // among them.
        if wait.is_zero() || !reading.events.is_empty() || reading.missed > 0 {
            return Some(ToolResult::done(tools::events_json(&reading, false)));
        }
        if self.waiting_calls() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        let key = self.next_wait;
        let timer = match self.arm_wait_timer(key, wait) {
            Ok(timer) => timer,
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        self.next_wait += 1;
        self.event_waits.push(EventWait {
            key,
            id: id.clone(),
            revision,
            seat,
            after,
            kinds: call.kinds,
            timer,
        });
        None
    }

    /// The calls waiting, `windows`', `events`', `clipboard get`'s, the `spawn` starts waiting
    /// for their seat's accessibility bus and `accessibility`'s together, which
    /// [`MAX_WAITING_CALLS`] bounds.
    fn waiting_calls(&self) -> usize {
        self.waits.len()
            + self.event_waits.len()
            + self.clipboard_reads.len()
            + self.spawn_waits.len()
            + self.accessibility.calls_in_flight()
    }

    /// Arms the timer that ends waiting call `key`'s time `after` from now, on real time.
    fn arm_wait_timer(
        &self,
        key: u64,
        after: Duration,
    ) -> Result<RegistrationToken, smithay::reexports::calloop::Error> {
        let events = self.events.clone();
        self.compositor
            .loop_handle()
            .insert_source(Timer::from_duration(after), move |_, _, state| {
                state.woke = true;
                events.borrow_mut().timed_out.push(key);
                TimeoutAction::Drop
            })
            .map_err(|e| e.error)
    }

    /// Records in the seats' logs what happened since the last time: outputs and windows as the
    /// compositor describes them now, the clients the display ended, with the program each
    /// belonged to, which keeps the protocol error it was ended for, and the programs reaped.
    /// The windows and outputs are described only when the compositor's description generation
    /// moved since the last look: a round in which only buffers changed, a game drawing,
    /// describes nothing. Each agent seat has a log from its first look; a closed seat's went
    /// with it.
    fn record_events(&mut self) {
        for seat in self.compositor.seats() {
            if seat != OWNER_SEAT {
                self.event_log.add_seat(seat);
            }
        }
        let time_ns = self.compositor.now().as_nanos() as u64;
        let generation = self.compositor.description_generation();
        if self.looked_at != Some(generation) {
            self.looked_at = Some(generation);
            self.looks += 1;
            let windows = self.compositor.windows();
            let outputs = self.compositor.output_descriptions();
            let arranged = !self.compositor.arrangement_pending();
            for (seat, data) in self.observer.look(&windows, &outputs, arranged) {
                self.event_log.record(seat, time_ns, data);
            }
        }
        for departure in self.compositor.take_client_departures() {
            if !self.event_log.has_seat(departure.seat) {
                continue;
            }
            let program = self.programs.as_mut().and_then(|(programs, _)| {
                let program = programs.program_of(departure.seat, &departure.process)?;
                if let Some(error) = &departure.error {
                    programs.note_protocol_error(program, error.clone());
                }
                Some(program)
            });
            let left = EventData::ClientLeft {
                pid: departure.process.pid,
                program,
                error: departure.error,
            };
            self.event_log.record(departure.seat, time_ns, left);
        }
        for (seat, program, end) in std::mem::take(&mut self.program_exits) {
            self.event_log
                .record(seat, time_ns, EventData::ProgramExited { program, end });
        }
    }

    /// Answers each `events` call waiting whose seat now has an event it takes, or whose
    /// cursor's events the log has begun to drop.
    fn answer_event_waits(&mut self) {
        let mut index = 0;
        while index < self.event_waits.len() {
            let wait = &self.event_waits[index];
            let reading = self
                .event_log
                .read(wait.seat, wait.after, wait.kinds.as_deref());
            if reading.events.is_empty() && reading.missed == 0 {
                index += 1;
                continue;
            }
            let result = ToolResult::done(tools::events_json(&reading, false));
            let wait = self.event_waits.remove(index);
            self.compositor.loop_handle().remove(wait.timer);
            self.queue.push(&protocol::response(
                wait.id,
                result.into_result(wait.revision),
            ));
        }
    }

    /// `windows`: the result now, or none when the call waits.
    fn windows(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call: WindowsCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        if let Err(failed) = self.agent_seat(call.seat) {
            return Some(failed);
        }
        let wait = Duration::from_millis(call.wait_ms);
        if wait > MAX_WAIT {
            return Some(ToolResult::failed(format!(
                "wait_ms is at most {}",
                MAX_WAIT.as_millis()
            )));
        }
        let matching = self.matching_windows(&call);
        // A window that matches but waits for its place is answered once placed, as one that
        // opens later is.
        let placed = !self.compositor.arrangement_pending();
        if wait.is_zero() || (!matching.is_empty() && placed) {
            return Some(ToolResult::done(json!({ "windows": matching })));
        }
        if self.waiting_calls() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        let key = self.next_wait;
        let timer = match self.arm_wait_timer(key, wait) {
            Ok(timer) => timer,
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        self.next_wait += 1;
        self.waits.push(WindowWait {
            key,
            id: id.clone(),
            revision,
            call,
            timer,
        });
        None
    }

    /// `clipboard`: the result now, or none while a `get` waits for its selection's data.
    fn clipboard(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call: ClipboardCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        let seat = match self.agent_seat(call.seat()) {
            Ok(seat) => seat,
            Err(failed) => return Some(failed),
        };
        let selection = call.selection();
        match call {
            ClipboardCall::Get {
                mime, max_bytes, ..
            } => match tools::clipboard_bytes(max_bytes) {
                Ok(max_bytes) => self.clipboard_get(id, revision, seat, selection, mime, max_bytes),
                Err(reason) => Some(ToolResult::failed(reason)),
            },
            ClipboardCall::Set { text, .. } => {
                if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
                    return Some(ToolResult::failed(format!(
                        "the text takes {} bytes, over the {MAX_CLIPBOARD_TEXT_BYTES} set takes",
                        text.len()
                    )));
                }
                let bytes = text.len();
                let text: Arc<[u8]> = Arc::from(text.into_bytes());
                self.compositor.hold_agent_selection(seat, selection, text);
                Some(ToolResult::done(json!({
                    "mime_types": self.compositor.selection_mime_types(seat, selection),
                    "bytes": bytes,
                })))
            }
            ClipboardCall::Clear { .. } => {
                self.compositor.clear_selection(seat, selection);
                Some(ToolResult::done(json!({ "mime_types": [] })))
            }
        }
    }

    /// `clipboard get` of `seat`'s `selection`, as `mime` or its first text type: the answer
    /// now when the selection is empty, offers no text type, or Yuna holds its text; else none,
    /// while its pipe is read, counted among the waiting calls.
    fn clipboard_get(
        &mut self,
        id: &Value,
        revision: Revision,
        seat: SeatId,
        selection: Selection,
        mime: Option<String>,
        max_bytes: usize,
    ) -> Option<ToolResult> {
        let mime_types = self.compositor.selection_mime_types(seat, selection);
        if mime_types.is_empty() {
            return Some(ToolResult::done(tools::clipboard_json(&[], None)));
        }
        let mime = match mime {
            Some(mime) if mime_types.contains(&mime) => mime,
            Some(mime) => {
                return Some(ToolResult::failed(format!(
                    "the selection offers no {mime}: it offers {}",
                    mime_types.join(", ")
                )));
            }
            None => match tools::first_text_type(&mime_types) {
                Some(text) => text,
                None => return Some(ToolResult::done(tools::clipboard_json(&mime_types, None))),
            },
        };
        // Before the source is asked, so that none is asked in vain.
        if self.waiting_calls() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        let pipe = match self
            .compositor
            .read_selection(seat, selection, mime.clone())
        {
            Ok(SelectionRead::Pipe(pipe)) => pipe,
            Ok(SelectionRead::Empty) => {
                return Some(ToolResult::done(tools::clipboard_json(&[], None)));
            }
            Ok(SelectionRead::Held(text)) => {
                let data = ClipboardData {
                    mime,
                    bytes: text[..text.len().min(max_bytes)].to_vec(),
                    truncated: text.len() > max_bytes,
                    timed_out: false,
                };
                let answer = tools::clipboard_json(&mime_types, Some(&data));
                return Some(ToolResult::done(answer));
            }
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        let key = self.next_wait;
        let (source, timer) = match self.watch_clipboard_read(key, &pipe) {
            Ok(tokens) => tokens,
            Err(error) => return Some(ToolResult::failed(error)),
        };
        self.next_wait += 1;
        self.clipboard_reads.push(ClipboardRead {
            key,
            id: id.clone(),
            revision,
            seat,
            mime_types,
            mime,
            max_bytes,
            pipe,
            data: Vec::new(),
            source,
            timer,
        });
        None
    }

    /// The sources of clipboard read `key`: one noting when `pipe` may be read, and the timer of
    /// [`CLIPBOARD_TIMEOUT`]. Neither is left when one cannot be made.
    fn watch_clipboard_read(
        &self,
        key: u64,
        pipe: &OwnedFd,
    ) -> Result<(RegistrationToken, RegistrationToken), String> {
        let handle = self.compositor.loop_handle();
        let watched = pipe.try_clone().map_err(|error| error.to_string())?;
        let events = self.events.clone();
        let source = handle
            .insert_source(
                Generic::new(watched, Interest::READ, Mode::Level),
                move |_, _, state| {
                    state.woke = true;
                    let readable = &mut events.borrow_mut().clipboards_readable;
                    if !readable.contains(&key) {
                        readable.push(key);
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|error| error.error.to_string())?;
        let events = self.events.clone();
        let timer = handle.insert_source(
            Timer::from_duration(CLIPBOARD_TIMEOUT),
            move |_, _, state| {
                state.woke = true;
                events.borrow_mut().timed_out.push(key);
                TimeoutAction::Drop
            },
        );
        match timer {
            Ok(timer) => Ok((source, timer)),
            Err(error) => {
                handle.remove(source);
                Err(error.error.to_string())
            }
        }
    }

    /// Reads what the pipes of the clipboard reads `keys` hold now, and answers each that ended.
    fn read_clipboards(&mut self, keys: Vec<u64>) {
        for key in keys {
            let Some(index) = self.clipboard_reads.iter().position(|read| read.key == key) else {
                continue;
            };
            if let Some(end) = self.drain_clipboard_read(index) {
                self.answer_clipboard_read(index, end);
            }
        }
    }

    /// Reads what the pipe of the clipboard read at `index` holds now, never waiting: how the
    /// read ended, or none while the program may write more.
    fn drain_clipboard_read(&mut self, index: usize) -> Option<ReadEnd> {
        let read = &mut self.clipboard_reads[index];
        let buffer = &mut self.read_buffer;
        loop {
            // One byte past max_bytes tells that the program had more.
            let room = (read.max_bytes + 1 - read.data.len()).min(buffer.len());
            match rustix::io::read(&read.pipe, &mut buffer[..room]) {
                Ok(0) => return Some(ReadEnd::Closed),
                Ok(count) => {
                    read.data.extend_from_slice(&buffer[..count]);
                    if read.data.len() > read.max_bytes {
                        return Some(ReadEnd::Truncated);
                    }
                }
                Err(rustix::io::Errno::AGAIN) => return None,
                Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Some(ReadEnd::Failed(error)),
            }
        }
    }

    /// Answers the clipboard read at `index`, which ended so; its pipe closes, and a program
    /// still writing sees it closed.
    fn answer_clipboard_read(&mut self, index: usize, end: ReadEnd) {
        let read = self.drop_clipboard_read(index);
        let result = match end {
            ReadEnd::Failed(error) => {
                ToolResult::failed(format!("the selection could not be read: {error}"))
            }
            ReadEnd::Closed | ReadEnd::Truncated | ReadEnd::TimedOut => {
                let mut bytes = read.data;
                bytes.truncate(read.max_bytes);
                let data = ClipboardData {
                    mime: read.mime,
                    bytes,
                    truncated: end == ReadEnd::Truncated,
                    timed_out: end == ReadEnd::TimedOut,
                };
                ToolResult::done(tools::clipboard_json(&read.mime_types, Some(&data)))
            }
        };
        self.queue.push(&protocol::response(
            read.id,
            result.into_result(read.revision),
        ));
    }

    /// Takes the clipboard read at `index` out, unanswered, its sources removed.
    fn drop_clipboard_read(&mut self, index: usize) -> ClipboardRead {
        let read = self.clipboard_reads.remove(index);
        let handle = self.compositor.loop_handle();
        handle.remove(read.source);
        // A timer that fired has dropped itself: its token is then stale, and nothing happens.
        handle.remove(read.timer);
        read
    }

    /// `capture`: a failure now, or none while the capture waits for its frame; one capture of
    /// a seat at a time.
    fn capture(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call: CaptureCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        let seat = match self.agent_seat(call.seat) {
            Ok(seat) => seat,
            Err(failed) => return Some(failed),
        };
        // A capture cancelled while encoding leaves its encoder running, out of `captures`.
        if self.encoders_running >= MAX_ENCODERS {
            return Some(ToolResult::failed(format!(
                "{MAX_ENCODERS} captures are being encoded: wait for them"
            )));
        }
        if self.captures.iter().any(|capture| capture.seat == seat) {
            return Some(ToolResult::failed(format!(
                "a capture of seat {} is in flight: wait for its answer",
                seat.0
            )));
        }
        let target = match call.target() {
            Ok(target) => target,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        let ticket = match self.compositor.capture(seat, target, call.size()) {
            Ok(ticket) => ticket,
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        let key = self.next_wait;
        let timer = match self.arm_wait_timer(key, MAX_WAIT) {
            Ok(timer) => timer,
            Err(error) => {
                self.compositor.cancel_capture(ticket);
                return Some(ToolResult::failed(error.to_string()));
            }
        };
        self.next_wait += 1;
        self.captures.push(CaptureWait {
            key,
            id: id.clone(),
            revision,
            seat,
            ticket,
            stage: CaptureStage::Copying(timer),
        });
        None
    }

    /// Starts encoding each capture whose pixels the compositor copied, on a thread of its own,
    /// or queues its frame while [`MAX_ENCODERS`] run; answers those it failed.
    fn encode_copied_captures(&mut self) {
        for (ticket, copied) in self.compositor.take_captures() {
            let Some(index) = self.captures.iter().position(|capture| {
                capture.ticket == ticket && matches!(capture.stage, CaptureStage::Copying(_))
            }) else {
                continue;
            };
            if let CaptureStage::Copying(timer) = self.captures[index].stage {
                self.compositor.loop_handle().remove(timer);
            }
            let frame = match copied {
                Ok(frame) => frame,
                Err(why) => {
                    self.answer_capture(
                        index,
                        ToolResult::failed(format!("the capture failed: {why}")),
                    );
                    continue;
                }
            };
            let (width, height) = (frame.mapping.width, frame.mapping.height);
            if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
                self.answer_capture(
                    index,
                    ToolResult::failed(format!(
                        "the capture's image is {width} × {height} pixels, over the \
                         {MAX_IMAGE_SIDE} an image may have on a side: capture a region, or at \
                         the logical size"
                    )),
                );
                continue;
            }
            self.captures[index].stage = CaptureStage::Encoding(CopiedCapture::of(&frame));
            self.waiting_encoders
                .push_back((self.captures[index].key, frame));
        }
        self.start_encoders();
    }

    /// Starts an encoder for each frame waiting, oldest first, while fewer than
    /// [`MAX_ENCODERS`] run: each is counted as it starts, so that never more run, however many
    /// captures a round copies.
    fn start_encoders(&mut self) {
        while self.encoders_running < MAX_ENCODERS
            && let Some((key, frame)) = self.waiting_encoders.pop_front()
        {
            let results = self.encoder_results.clone();
            let started = std::thread::Builder::new()
                .name("yuna-capture".to_owned())
                .spawn(move || {
                    let began = Instant::now();
                    // A panic here is a bug of Yuna's; it fails this capture, not the seat's
                    // next ones, which would wait on it for ever.
                    let encoded = std::panic::catch_unwind(|| image::encode(&frame))
                        .unwrap_or_else(|_| Err("the capture's encoder failed".to_owned()));
                    tracing::debug!(
                        width = frame.mapping.width,
                        height = frame.mapping.height,
                        took_us = began.elapsed().as_micros(),
                        "capture encoded"
                    );
                    // A capture cancelled meanwhile, or a server gone, takes nothing.
                    let _ = results.send((key, encoded));
                });
            match started {
                Ok(_) => self.encoders_running += 1,
                Err(error) => {
                    if let Some(index) = self.captures.iter().position(|c| c.key == key) {
                        self.answer_capture(
                            index,
                            ToolResult::failed(format!(
                                "the capture could not be encoded: {error}"
                            )),
                        );
                    }
                }
            }
        }
    }

    /// Records the images the encoders finished.
    fn note_encoded(&mut self, encoded: Vec<(u64, Result<Encoded, String>)>) {
        for (key, result) in encoded {
            self.encoders_running = self.encoders_running.saturating_sub(1);
            if let Some(capture) = self.captures.iter_mut().find(|capture| capture.key == key)
                && let CaptureStage::Encoding(copied) = capture.stage
            {
                capture.stage = CaptureStage::Encoded {
                    copied,
                    result,
                    woken: false,
                };
            }
        }
    }

    /// Answers each capture encoded whose frame's boundary has come; on the real clock, arms a
    /// timer to wake the loop at the boundary of one encoded before it.
    fn answer_captures_due(&mut self) {
        let now = self.compositor.now();
        let real = self.compositor.clock_mode() == ClockMode::Real;
        let mut index = 0;
        while index < self.captures.len() {
            let CaptureStage::Encoded { copied, woken, .. } = &mut self.captures[index].stage
            else {
                index += 1;
                continue;
            };
            if copied.presented_at > now {
                if real && !*woken {
                    let wait = copied.presented_at.saturating_sub(now);
                    *woken = self
                        .compositor
                        .loop_handle()
                        .insert_source(Timer::from_duration(wait), |_, _, state| {
                            state.woke = true;
                            TimeoutAction::Drop
                        })
                        .is_ok();
                }
                index += 1;
                continue;
            }
            let wait = self.captures.remove(index);
            let CaptureStage::Encoded { copied, result, .. } = wait.stage else {
                continue;
            };
            let result = match result {
                Ok(encoded) => {
                    let id = self.next_capture;
                    self.next_capture += 1;
                    let recent = self.recent_captures.entry(wait.seat).or_default();
                    if recent.len() >= RECENT_CAPTURES {
                        recent.pop_front();
                    }
                    recent.push_back((id, copied.mapping));
                    ToolResult::done_with_image(
                        tools::capture_json(id, &copied, encoded.png_bytes),
                        encoded.base64,
                    )
                }
                Err(why) => ToolResult::failed(why),
            };
            self.queue.push(&protocol::response(
                wait.id,
                result.into_result(wait.revision),
            ));
        }
    }

    /// Answers the capture at `index` with `result`, now.
    fn answer_capture(&mut self, index: usize, result: ToolResult) {
        let capture = self.captures.remove(index);
        self.queue.push(&protocol::response(
            capture.id,
            result.into_result(capture.revision),
        ));
    }

    /// Drops the capture at `index`, unanswered: its frame no longer waited for, or no longer
    /// waiting for an encoder, its image discarded when it comes.
    fn drop_capture(&mut self, index: usize) -> CaptureWait {
        let capture = self.captures.remove(index);
        match capture.stage {
            CaptureStage::Copying(timer) => {
                self.compositor.loop_handle().remove(timer);
                self.compositor.cancel_capture(capture.ticket);
            }
            CaptureStage::Encoding(_) => self
                .waiting_encoders
                .retain(|(waiting, _)| *waiting != capture.key),
            CaptureStage::Encoded { .. } => {}
        }
        capture
    }

    fn matching_windows(&self, call: &WindowsCall) -> Vec<Value> {
        self.compositor
            .windows()
            .iter()
            .filter(|window| call.matches(window))
            .map(|window| self.window_json(call, window))
            .collect()
    }

    /// `window` as `call` asks for it: with its surfaces' colours when it asks for them.
    fn window_json(&self, call: &WindowsCall, window: &WindowDescription) -> Value {
        let colours = call
            .colour
            .then(|| self.compositor.window_colours(window.id));
        tools::window_json(window, colours.as_deref())
    }

    /// Answers each waiting call a window now matches.
    fn answer_waits(&mut self) {
        let windows = self.compositor.windows();
        let mut index = 0;
        while index < self.waits.len() {
            let call = &self.waits[index].call;
            let matching: Vec<Value> = windows
                .iter()
                .filter(|window| call.matches(window))
                .map(|window| self.window_json(call, window))
                .collect();
            if matching.is_empty() {
                index += 1;
                continue;
            }
            let wait = self.waits.remove(index);
            self.compositor.loop_handle().remove(wait.timer);
            let result = ToolResult::done(json!({ "windows": matching }));
            self.queue.push(&protocol::response(
                wait.id,
                result.into_result(wait.revision),
            ));
        }
    }

    /// Answers the call `key` whose time is up, if it still waits: no window matched, no event
    /// came, no frame came for a capture, or a clipboard read's program did not close its pipe.
    fn answer_timed_out(&mut self, key: u64) {
        if let Some(index) = self.event_waits.iter().position(|wait| wait.key == key) {
            // Its timer removed itself.
            let wait = self.event_waits.remove(index);
            let reading = self
                .event_log
                .read(wait.seat, wait.after, wait.kinds.as_deref());
            // An event recorded in this round, not yet told, comes with it.
            let timed_out = reading.events.is_empty();
            let result = ToolResult::done(tools::events_json(&reading, timed_out));
            self.queue.push(&protocol::response(
                wait.id,
                result.into_result(wait.revision),
            ));
            return;
        }
        if let Some(index) = self.clipboard_reads.iter().position(|read| read.key == key) {
            // What came last is read first: a program that closed its pipe meanwhile is
            // answered whole.
            let end = self
                .drain_clipboard_read(index)
                .unwrap_or(ReadEnd::TimedOut);
            self.answer_clipboard_read(index, end);
            return;
        }
        if let Some(index) = self.captures.iter().position(|capture| {
            capture.key == key && matches!(capture.stage, CaptureStage::Copying(_))
        }) {
            // Its timer removed itself.
            let capture = self.captures.remove(index);
            self.compositor.cancel_capture(capture.ticket);
            let result = ToolResult::failed(format!(
                "no frame came in {} s: nothing changed on the output since a refresh was armed; \
                 on a manual clock, advance it",
                MAX_WAIT.as_secs()
            ));
            self.queue.push(&protocol::response(
                capture.id,
                result.into_result(capture.revision),
            ));
            return;
        }
        let Some(index) = self.waits.iter().position(|wait| wait.key == key) else {
            return;
        };
        // Its timer removed itself.
        let wait = self.waits.remove(index);
        let result = ToolResult::done(json!({ "windows": [], "timed_out": true }));
        self.queue.push(&protocol::response(
            wait.id,
            result.into_result(wait.revision),
        ));
    }

    /// Drops the waiting calls of request `id`, unanswered, as MCP asks. A seat closing goes on,
    /// unanswered.
    fn cancel(&mut self, id: &Value) {
        self.accessibility.cancel(id);
        self.spawn_waits.retain(|wait| &wait.id != id);
        for closing in &mut self.closings {
            if closing
                .reply
                .as_ref()
                .is_some_and(|(request, _)| request == id)
            {
                closing.reply = None;
            }
        }
        while let Some(index) = self.captures.iter().position(|capture| &capture.id == id) {
            self.drop_capture(index);
            tracing::debug!(request = %id, "a capture is cancelled");
        }
        while let Some(index) = self.clipboard_reads.iter().position(|read| &read.id == id) {
            self.drop_clipboard_read(index);
            tracing::debug!(request = %id, "a clipboard read is cancelled");
        }
        let handle = self.compositor.loop_handle();
        self.waits.retain(|wait| {
            let cancelled = &wait.id == id;
            if cancelled {
                handle.remove(wait.timer);
                tracing::debug!(request = %id, "a waiting call is cancelled");
            }
            !cancelled
        });
        self.event_waits.retain(|wait| {
            let cancelled = &wait.id == id;
            if cancelled {
                handle.remove(wait.timer);
                tracing::debug!(request = %id, "a waiting `events` call is cancelled");
            }
            !cancelled
        });
    }

    fn clock(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        if let ClockCall::Advance { by_ns, by_ms } = call {
            if self.compositor.clock_mode() != ClockMode::Manual {
                return ToolResult::failed(
                    "the clock is real time, which moves by itself: start `yuna mcp \
                     --manual-clock` to step it",
                );
            }
            let by = match tools::advance_length(by_ns, by_ms) {
                Ok(by) => by,
                Err(reason) => return ToolResult::failed(reason),
            };
            let room = self
                .compositor
                .real_now()
                .saturating_sub(self.compositor.now());
            if by > room {
                return ToolResult::failed(format!(
                    "the clock never runs ahead of real time: it may advance by {} ns now",
                    room.as_nanos()
                ));
            }
            if let Err(error) = self.compositor.advance(by) {
                return ToolResult::failed(error.to_string());
            }
        }
        ToolResult::done(self.clock_json())
    }

    fn clock_json(&self) -> Value {
        let now = self.compositor.now();
        let manual = self.compositor.clock_mode() == ClockMode::Manual;
        json!({
            "mode": if manual { "manual" } else { "real" },
            "now_ns": now.as_nanos() as u64,
            "real_now_ns": self.compositor.real_now().as_nanos() as u64,
            "next_deadline_ns": self.compositor.next_deadline().map(|at| at.as_nanos() as u64),
        })
    }

    /// Writes what waits, watches the output while some is left, and stops reading the input
    /// while too much is.
    fn write_output(&mut self) {
        if let Err(error) = self.queue.write_to(&self.output) {
            self.stop(Stop::Broken(format!("its output failed: {error}")));
            return;
        }
        let handle = self.compositor.loop_handle();
        let watch = !self.queue.is_empty();
        if watch != self.output_watched
            && let Some(source) = &self.output_source
        {
            let changed = if watch {
                handle.enable(source)
            } else {
                handle.disable(source)
            };
            if changed.is_ok() {
                self.output_watched = watch;
            }
        }
        let pause = self.queue.len() >= MAX_QUEUED_OUTPUT;
        if pause != self.input_paused && self.stopping.is_none() {
            let changed = if pause {
                handle.disable(&self.input_source)
            } else {
                handle.enable(&self.input_source)
            };
            if changed.is_ok() {
                self.input_paused = pause;
                tracing::debug!(paused = pause, "the client's input");
            }
        }
    }
}

/// `mode`, a mode just made, under VRR from `vrr_min` and announcing what `hdr` says: `true`
/// the PG32UCDM's colours, the one monitor Yuna knows, `false` none, and left out
/// `default`'s. A mode refused on the way is the call's failure, with its reason.
fn checked_mode(
    mode: Result<OutputMode, InvalidOutput>,
    vrr_min: Option<f64>,
    hdr: Option<bool>,
    default: OutputMode,
) -> Result<OutputMode, ToolResult> {
    let refused = |reason: InvalidOutput| ToolResult::failed(reason.to_string());
    let mut mode = mode.map_err(refused)?;
    if let Some(min) = vrr_min {
        mode = mode.with_vrr(min).map_err(refused)?;
    }
    let monitor = match hdr {
        Some(true) => Some(PG32UCDM),
        Some(false) => None,
        None => default.hdr(),
    };
    if let Some(monitor) = monitor {
        mode = mode.with_hdr(monitor);
    }
    Ok(mode)
}
