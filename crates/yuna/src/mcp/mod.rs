//! Yuna's MCP server: what `yuna mcp` runs, a headless compositor of one MCP client's own, which
//! speaks MCP on the client's pipes ([ADR-0033](../../../../docs/adr/0033-the-mcp-server.md)).
//!
//! Everything runs in the compositor's event loop, on its thread: the client's input and output,
//! the programs' output and exits, the calls that wait. Their sources only note what came; after
//! each round, [`Server::dispatch`] reads the requests, answers them, and writes the replies. A
//! call that waits is answered from a later round, or when its time is up, on the real clock
//! whatever the compositor's: [`tools::MAX_WAIT`] at most, under the time a client gives a call.
//! `notifications/cancelled` drops a waiting call unanswered.
//!
//! Everything an agent does is on a seat of its own, which `seat create` makes
//! ([ADR-0078](../../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md),
//! [ADR-0095](../../../../docs/adr/0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)): every tool but
//! `clock` names it, `rest` when it is to name only that seat's, and an id of another seat's
//! output, window or program is unknown there.
//!
//! Bounds: a line over [`stdio::MAX_LINE`] ends the connection; while more than
//! [`MAX_QUEUED_OUTPUT`] of replies wait, because the client does not read them, no further
//! request is answered and its input is not read, and the answer of a call that waited is held,
//! as its result, until the client has read enough (`Server::answer_waited`): the replies
//! waiting pass the bound by one reply at most. The calls that wait and the captures being
//! encoded have bounds of their own (`waits.rs`, `captures.rs`).
//!
//! With `--rest-audit`, each round's end logs the rest audit's second that ended, each client's
//! program named.
//!
//! A panic of the server's thread is fatal: the program's panic hook ends the programs and
//! removes their folder ([`Server::programs_folder`], [`end_after_panic`]), then stops Yuna. In a
//! debug build, `PANIC_ON_TOOL` makes a tool's call panic, for the tests of that path, and
//! `PANIC_IN_ENCODERS` the first captures' encoders, whose panic fails their capture alone.
//!
//! This module holds [`Server`]: its state, its life in the loop, the client's lines read and
//! its replies written, and each request routed to its tool. Each group of tools adds its own
//! `impl Server` in a module of its own, so that a tool is read and changed alone:
//!
//! - `seats.rs`: `seat`, and a seat's closing;
//! - `outputs.rs`: `outputs` and `workspaces`;
//! - `input.rs`: `pointer` and `keyboard`;
//! - `spawn.rs`: `spawn` and `accessibility`, and the programs' throttle and end;
//! - `waits.rs`: `windows`, `events` and `rest`, the seats' events recorded, and what every call
//!   that waits shares: its bound, its timer, its time up, its cancellation, its seat closed;
//! - `clipboard.rs`: `clipboard`;
//! - `captures.rs`: `capture`, and the encoders of its images;
//! - `clock.rs`: `clock`.
//!
//! What they share: the protocol in `protocol.rs`, the framing in `stdio.rs`, the tools'
//! schemas, arguments and results in `tools/`, the programs in `programs/`, the seats' events
//! in `events.rs`, a capture's PNG in `image.rs`, the accessibility buses in `accessibility/`.

mod accessibility;
mod captures;
mod clipboard;
mod clock;
mod events;
mod image;
mod input;
mod outputs;
mod programs;
mod protocol;
mod seats;
mod spawn;
mod stdio;
mod tools;
mod waits;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use calloop::channel::{self, SyncSender};
use calloop::generic::Generic;
use calloop::{Interest, Mode, PostAction, RegistrationToken};
use rustix::fs::OFlags;
use rustix::process::Pid;
use serde_json::{Value, json};
use yuna_types::SeatId;

use crate::compositor::Compositor;
use crate::output::OutputMode;
use crate::rest::{Source, Wake, insert_source};
use crate::state::OWNER_SEAT;
use crate::tasks::{Task, TaskThread};
use crate::{CaptureMapping, CapturedFrame, Error};
use accessibility::Accessibility;
use captures::CaptureWait;
use clipboard::ClipboardRead;
use events::{EventLog, Observer};
use image::Encoded;
use programs::{End, Programs};
use protocol::{Call, Message, Revision, RpcError, Session};
use seats::SeatClosing;
use spawn::SpawnWait;
use stdio::{LineReader, LineTooLong, OutputQueue};
use tools::ToolResult;
use waits::{EventWait, RestWait, WindowWait};

pub use accessibility::{
    APP_DEADLINE, MAX_IN_FLIGHT, MAX_NODES, MAX_TEXT, REWALK_AFTER, REWALK_INTERVAL,
};
pub use captures::{MAX_ENCODERS, RECENT_CAPTURES};
pub use events::{MAX_EVENT_TEXT, MAX_EVENTS, MAX_EVENTS_PER_REPLY};
pub use image::{MAX_IMAGE_BYTES, MAX_IMAGE_SIDE};
pub use outputs::{MAX_OUTPUT_PIXELS, MAX_OUTPUT_SIDE};
pub use programs::{
    ACCESSIBILITY_VARIABLES, BUS_VARIABLE, END_GRACE, HOMES_LINK, KEPT_PREFIXES, KEPT_VARIABLES,
    KILL_WAIT, LOCK_FILE, LOG_HEAD, LOG_TAIL, MAX_KEPT, MAX_RUNNING, NO_ACCESSIBILITY_VARIABLES,
    PACE_PERIOD, SEAT_VARIABLE, end_after_panic,
};
pub use protocol::{LEGACY_REVISION, MODERN_REVISION};
pub use stdio::MAX_LINE;
pub use tools::{
    DEFAULT_CLIPBOARD_BYTES, DEFAULT_LOG_READ, MAX_CLIPBOARD_BYTES, MAX_CLIPBOARD_TEXT_BYTES,
    MAX_LOG_READ, MAX_WAIT, MAX_WAITING_CALLS, MAX_WORKSPACE_NAME,
};

/// How many bytes of replies may wait for the client before its input stops being read: 16 MiB,
/// room for a few captures (M1.5's C4) beside the replies to a burst of calls.
pub const MAX_QUEUED_OUTPUT: usize = 16 << 20;

/// The variable that, in a debug build, names a tool whose every call makes the server's thread,
/// Yuna's state thread, panic: what the tests of Yuna's fatal path use
/// ([ADR-0080](../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)). It is
/// read when the server starts. A release build has neither the variable nor the panic.
#[cfg(debug_assertions)]
pub const PANIC_ON_TOOL: &str = "YUNA_PANIC_ON_TOOL";

/// The variable that, in a debug build, gives how many captures' encoders panic, the first ones:
/// what the test of an encoder's contained panic uses
/// ([ADR-0080](../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)). It
/// is read when the server starts; a value that is not a number is none. A release build has
/// neither the variable nor the panic.
#[cfg(debug_assertions)]
pub const PANIC_IN_ENCODERS: &str = "YUNA_PANIC_IN_ENCODERS";

/// How much of the client's input is read at a time.
const READ_SIZE: usize = 64 * 1024;

/// How long Yuna, stopping, still tries to write the replies the client has not read.
const LAST_WRITE_WAIT: Duration = Duration::from_secs(1);

/// The answer of a call that waited, held while [`MAX_QUEUED_OUTPUT`] of replies wait.
#[derive(Debug)]
struct HeldAnswer {
    id: Value,
    revision: Revision,
    result: ToolResult,
}

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
    /// The answers of calls that waited, held, oldest first, while too many replies wait. No
    /// request is answered while one is held, so that no call starts meanwhile: they are those
    /// of the [`MAX_WAITING_CALLS`] calls that wait, of the captures, one a seat, 16 at most
    /// since a seat keeps one of Yuna's [`MAX_OUTPUTS`](crate::MAX_OUTPUTS) outputs, and of the
    /// seats that close, a few bytes a process each. They are bounded in number, not in bytes:
    /// each holds its tool's result. A capture's image takes up to [`MAX_IMAGE_BYTES`], and a
    /// `clipboard get`'s data with its JSON about 3 MiB, so that 64 clipboard reads and 16
    /// captures hold about 280 MB. An `accessibility` call's 256 nodes and 128 applications, their
    /// texts cut, an application's failure among them, take up to about 10 MB; a `windows`
    /// wait's takes every window it matches, up to
    /// [`MAX_WINDOWS`](crate::MAX_WINDOWS).
    held: VecDeque<HeldAnswer>,
    events: Rc<RefCell<Events>>,
    waits: Vec<WindowWait>,
    event_waits: Vec<EventWait>,
    rest_waits: Vec<RestWait>,
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
    /// The highest id of the seats `seat create` made: ids are given in order from the owner's
    /// and never reused, so that a seat up to it exists or existed, which a process's
    /// `YUNA_SEAT` must name to say it is another seat's.
    last_seat: SeatId,
    captures: Vec<CaptureWait>,
    clipboard_reads: Vec<ClipboardRead>,
    /// Where the encoders send the captures' images, by key: bounded by [`MAX_ENCODERS`], since
    /// each encoder sends one and no more run.
    encoder_results: SyncSender<(u64, Result<Encoded, String>)>,
    /// Each seat's last captures, newest last, by id, which `pointer` takes.
    recent_captures: HashMap<SeatId, VecDeque<(u64, CaptureMapping)>>,
    /// The next capture's id: ids increase and are never reused.
    next_capture: u64,
    /// The encoders' threads running, those of captures cancelled included: counted when each
    /// starts, until its image comes back.
    encoders_running: usize,
    /// How many encoders may run at once on this machine ([`captures::encoder_limit`]).
    encoder_limit: usize,
    /// The encoders' threads not yet seen ended, joined within a bound when the server finishes.
    encoders: Vec<TaskThread>,
    /// The frames of captures copied while the limit of encoders ran, by capture key, oldest
    /// first: each starts its encoder when one ends. One a seat at most, as its capture.
    waiting_encoders: VecDeque<(u64, CapturedFrame)>,
    /// The mode of a new seat's output, unless `seat create` says.
    seat_output: OutputMode,
    /// The programs, when the server may start some, with SIGCHLD's pipe.
    programs: Option<(Programs, OwnedFd)>,
    /// The program each connected client belongs to, by the client's number
    /// ([`RestCounts::clients`](crate::rest::RestCounts::clients)), resolved once from the record
    /// made when Yuna accepted it ([`Server::resolve_arrivals`], ADR-0156): what `client_left`,
    /// `rest` and the rest audit's log name it by, even once the program is forgotten. A client
    /// of no program has no entry. Bound: the clients connected, each entry dropped when its
    /// client's departure is recorded.
    client_programs: HashMap<u64, u64>,
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
    /// How many of the next captures' encoders panic, from [`PANIC_IN_ENCODERS`].
    #[cfg(debug_assertions)]
    encoder_panics: u32,
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
fn is_not_pollable(error: &calloop::Error) -> bool {
    matches!(error, calloop::Error::IoError(io)
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
        compositor.watch_client_arrivals();
        compositor.watch_client_departures();
        compositor.watch_client_refusals();
        let failed = |what: &str, error: &dyn std::fmt::Display| {
            Error::EventLoop(format!("the MCP server's {what}: {error}"))
        };
        let output_flags = rustix::fs::fcntl_getfl(&output).map_err(|e| failed("output", &e))?;
        let events = Rc::new(RefCell::new(Events::default()));
        let handle = compositor.loop_handle();
        // A descriptor that cannot be watched, a regular file or `/dev/null`, is `None`.
        let watch = |fd: &OwnedFd, interest: Interest, source: Source, note: fn(&mut Events)| {
            let fd = fd.try_clone().map_err(|e| failed("descriptors", &e))?;
            let events = events.clone();
            let inserted = insert_source(
                &handle,
                Wake::of(source),
                Generic::new(fd, interest, Mode::Level),
                move |_, _, _| {
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
        let input_source = watch(&input, Interest::READ, Source::McpInput, |events| {
            events.input_readable = true
        })?
        .ok_or_else(|| {
            Error::EventLoop(
                "the MCP server reads a pipe or a socket, an MCP client's, not a file".to_owned(),
            )
        })?;
        // Writable only wakes the loop: each round writes what waits. A file is never watched:
        // a write to it does not wait.
        let output_source = watch(&output, Interest::WRITE, Source::McpOutput, |_| {})?;
        if let Some(source) = &output_source {
            handle
                .disable(source)
                .map_err(|e| failed("event loop", &e))?;
        }
        let programs = match children {
            None => None,
            Some(children) => {
                rustix::io::ioctl_fionbio(&children, true).map_err(|e| failed("SIGCHLD", &e))?;
                watch(&children, Interest::READ, Source::ChildEnded, |events| {
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
        let (encoder_results, encoded) = channel::sync_channel(MAX_ENCODERS);
        let noted = events.clone();
        insert_source(
            &handle,
            Wake::of(Source::Encoders),
            encoded,
            move |event, _, _| {
                if let channel::Event::Msg(message) = event {
                    noted.borrow_mut().encoded.push(message);
                }
            },
        )
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
            held: VecDeque::new(),
            events,
            waits: Vec::new(),
            event_waits: Vec::new(),
            rest_waits: Vec::new(),
            event_log: EventLog::default(),
            observer: Observer::default(),
            looked_at: None,
            looks: 0,
            program_exits: Vec::new(),
            next_wait: 1,
            closings: Vec::new(),
            last_seat: OWNER_SEAT,
            captures: Vec::new(),
            clipboard_reads: Vec::new(),
            encoder_results,
            recent_captures: HashMap::new(),
            next_capture: 1,
            encoders_running: 0,
            encoder_limit: captures::encoder_limit(std::thread::available_parallelism().ok()),
            encoders: Vec::new(),
            waiting_encoders: VecDeque::new(),
            seat_output: OutputMode::default(),
            programs,
            client_programs: HashMap::new(),
            throttle_armed: false,
            accessibility: Accessibility::new(handle.clone()),
            spawn_waits: Vec::new(),
            stopping: None,
            #[cfg(debug_assertions)]
            panic_on_tool: std::env::var(PANIC_ON_TOOL).ok(),
            #[cfg(debug_assertions)]
            encoder_panics: std::env::var(PANIC_IN_ENCODERS)
                .ok()
                .and_then(|count| count.parse().ok())
                .unwrap_or(0),
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

    /// How many encoders may run at once on this machine: [`MAX_ENCODERS`], or fewer on a
    /// machine of fewer than five cores. For tests.
    #[doc(hidden)]
    pub fn encoder_limit(&self) -> usize {
        self.encoder_limit
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
    /// and answers the calls waiting for them; then writes the replies, and logs the rest audit's
    /// second that ended, if it logs them.
    pub fn dispatch(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        let wakeups = self.compositor.counters().wakeups;
        self.compositor.dispatch(timeout)?;
        // In the round that accepted them, before a call may forget a program.
        self.resolve_arrivals();
        // Read now: a call answered below may run the clock's deadlines.
        let round_sources = self.compositor.rest_round_sources();
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
            self.answer_timed_out(key, round_sources);
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
        let client_programs = &self.client_programs;
        self.compositor
            .log_ended_rest_second(&|number| client_programs.get(&number).copied());
        Ok(())
    }

    /// Resolves the program of each client the compositor served since the last call, from the
    /// record made when Yuna accepted it, and keeps it by the client's number
    /// ([ADR-0156](../../../../docs/adr/0156-a-clients-pidfd-at-accept-its-life-read-by-poll.md)).
    /// [`Programs::program_of`] gives the same answer at any time until a program is forgotten:
    /// so this runs in the round that accepted the client, before any step that may forget a
    /// program, and before the departures are recorded, so that each finds its arrival resolved.
    fn resolve_arrivals(&mut self) {
        let arrivals = self.compositor.take_client_arrivals();
        let Some((programs, _)) = &self.programs else {
            return;
        };
        for arrival in arrivals {
            if let Some(program) = programs.program_of(arrival.seat, &arrival.process) {
                self.client_programs.insert(arrival.number, program);
            }
        }
    }

    /// Runs until the client closes its input, `stop` (the read end of a pipe SIGTERM and SIGINT
    /// write to) can be read, or the connection breaks; then ends the programs and returns why.
    pub fn run(mut self, stop: OwnedFd) -> Result<Stop, Error> {
        let events = self.events.clone();
        insert_source(
            &self.compositor.loop_handle(),
            Wake::of(Source::StopSignal),
            Generic::new(stop, Interest::READ, Mode::Level),
            move |_, _, _| {
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
    /// its output back its flags, ends every program and removes their folder, and joins the
    /// encoders still running within their bound (`tasks.rs`). Returns why it stopped.
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
        // Stopping, Yuna reads nothing more: what it holds is written with the rest, for a
        // second at most.
        for held in std::mem::take(&mut self.held) {
            self.queue.push(&protocol::response(
                held.id,
                held.result.into_result(held.revision),
            ));
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
            let timeout = rustix::event::Timespec {
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
        let encoders_by = Instant::now() + Task::Encoder.join_bound();
        for encoder in std::mem::take(&mut self.encoders) {
            encoder.join_by(encoders_by);
        }
        let stop = self.stopping.take().unwrap_or(Stop::Signal);
        tracing::info!(reason = ?stop, "the MCP server stops");
        stop
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
    /// replies wait and no answer is held; the rest wait for the client to read.
    fn answer_lines(&mut self) {
        while self.has_room()
            && let Some(line) = self.lines.pop_front()
        {
            self.answer_line(&line);
        }
    }

    /// Whether a reply may be queued now: fewer than [`MAX_QUEUED_OUTPUT`] bytes wait, and no
    /// answer held waits before it.
    fn has_room(&self) -> bool {
        self.held.is_empty() && self.queue.len() < MAX_QUEUED_OUTPUT
    }

    /// Answers request `id`, a call that waited, with `result`: queued now when there is room,
    /// else held as it is until the client has read enough. A burst of answers, 64 waiting
    /// calls answered in one round, never passes the replies that may wait by more than one.
    pub(super) fn answer_waited(&mut self, id: Value, revision: Revision, result: ToolResult) {
        if self.has_room() {
            self.queue
                .push(&protocol::response(id, result.into_result(revision)));
        } else {
            self.held.push_back(HeldAnswer {
                id,
                revision,
                result,
            });
        }
    }

    /// Queues the answers held, oldest first, while fewer than [`MAX_QUEUED_OUTPUT`] bytes
    /// wait.
    fn release_held(&mut self) {
        while self.queue.len() < MAX_QUEUED_OUTPUT
            && let Some(held) = self.held.pop_front()
        {
            self.queue.push(&protocol::response(
                held.id,
                held.result.into_result(held.revision),
            ));
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
                    "rest" => match self.rest(&id, revision, arguments) {
                        Some(result) => result,
                        // It counts until its time is up, and is answered then.
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

    /// Writes what waits, the answers held as room comes, watches the output while some is
    /// left, and stops reading the input while too much is, or an answer is held.
    fn write_output(&mut self) {
        loop {
            self.release_held();
            if let Err(error) = self.queue.write_to(&self.output) {
                self.stop(Stop::Broken(format!("its output failed: {error}")));
                return;
            }
            // A file takes every byte at once and never wakes the loop: what is held goes now.
            if self.held.is_empty() || !self.queue.is_empty() {
                break;
            }
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
        let pause = !self.has_room();
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
