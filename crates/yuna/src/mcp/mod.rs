//! Yuna's MCP server: what `yuna mcp` runs, a headless compositor of one MCP client's own, which
//! speaks MCP on the client's pipes ([ADR-0033](../../../../docs/adr/0033-the-mcp-server.md)).
//!
//! Everything runs in the compositor's event loop, on its thread: the client's input and output,
//! the programs' output and exits, the calls that wait. Their sources only note what came; after
//! each round, [`Server::dispatch`] reads the requests, answers them, and writes the replies. A
//! call that waits, for a window, is answered from a later round, once the layout has placed the
//! window, or when its time is up, on the real clock whatever the compositor's:
//! [`tools::MAX_WAIT`] at most, under the time a client gives a call. On a manual clock, the
//! window is placed only once the clock moves to its output's refresh.
//! `notifications/cancelled` drops a waiting call unanswered.
//!
//! Everything an agent does is on a seat of its own, which `seat create` makes
//! ([ADR-0035](../../../../docs/adr/0035-agents-seats.md),
//! [ADR-0037](../../../../docs/adr/0037-seats-tools-input-and-closing.md)): every tool but
//! `clock` names it, and an id of another seat's output, window or program is unknown there.
//! `seat close` releases what the seat holds, removes its socket, ends its clients, its outputs
//! and its place in the layout at once, then ends its processes: SIGTERM, then SIGKILL
//! [`END_GRACE`] later; it is answered once they are gone, [`KILL_WAIT`] after SIGKILL at most.
//!
//! Bounds: a line over [`stdio::MAX_LINE`] ends the connection; while more than
//! [`MAX_QUEUED_OUTPUT`] of replies wait, because the client does not read them, no further
//! request is answered and its input is not read; at most [`tools::MAX_WAITING_CALLS`] calls wait
//! at once. The protocol is in
//! `protocol.rs`, the framing in `stdio.rs`, the tools' schemas and results in `tools.rs`, the
//! programs in `programs.rs`.

mod programs;
mod protocol;
mod stdio;
mod tools;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, Mode, PostAction, RegistrationToken};
use smithay::reexports::rustix;
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::rustix::process::{Pid, Signal};
use yuna_layout::{OutputId, SeatId, WindowId};

use crate::compositor::Compositor;
use crate::output::OutputMode;
use crate::state::OWNER_SEAT;
use crate::{ClockMode, Error};
use programs::{Programs, Start, THROTTLE};
use protocol::{Call, Message, Revision, RpcError, Session};
use stdio::{LineReader, LineTooLong, OutputQueue};
use tools::{
    ButtonCall, ClockCall, KeyboardCall, OutputsCall, PointerCall, SeatCall, SpawnCall, ToolResult,
    WindowsCall, WorkspacesCall,
};

pub use programs::{
    END_GRACE, KEPT_PREFIXES, KEPT_VARIABLES, KILL_WAIT, LOG_LIMIT, MAX_KEPT, MAX_RUNNING,
    SEAT_VARIABLE,
};
pub use protocol::{LEGACY_REVISION, MODERN_REVISION};
pub use stdio::MAX_LINE;
pub use tools::{MAX_WAIT, MAX_WAITING_CALLS};

/// How many bytes of replies may wait for the client before its input stops being read: 16 MiB,
/// room for a few captures (M1.5's C4) beside the replies to a burst of calls.
pub const MAX_QUEUED_OUTPUT: usize = 16 << 20;

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
    /// The seat closings whose stage is up, by key.
    closings_due: Vec<u64>,
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
    /// The keys of waiting calls and of closings, never reused.
    next_wait: u64,
    closings: Vec<SeatClosing>,
    /// The mode of a new seat's output, unless `seat create` says.
    seat_output: OutputMode,
    /// The programs, when the server may start some, with SIGCHLD's pipe.
    programs: Option<(Programs, OwnedFd)>,
    /// Whether the timer that ends the programs' throttle is armed.
    throttle_armed: bool,
    /// Why the server stops, once it does.
    stopping: Option<Stop>,
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
        compositor: Compositor,
        input: OwnedFd,
        output: OwnedFd,
        children: Option<OwnedFd>,
    ) -> Result<Server, Error> {
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
            next_wait: 1,
            closings: Vec::new(),
            seat_output: OutputMode::default(),
            programs,
            throttle_armed: false,
            stopping: None,
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

    /// Why it stops, once it does: nothing more is read, and [`Server::finish`] ends it.
    pub fn stopping(&self) -> Option<&Stop> {
        self.stopping.as_ref()
    }

    /// Runs one round of the compositor's loop, waiting up to `timeout` (`None`: as long as it
    /// takes), then answers what came: requests, cancellations, ended programs, calls whose time
    /// is up or whose window opened; then writes the replies.
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
            programs.reap();
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
        for key in events.timed_out {
            self.answer_timed_out(key);
        }
        // A window is told once it is placed: after the arrangement that follows its mapping.
        if !self.waits.is_empty() && !self.compositor.arrangement_pending() {
            self.answer_waits();
        }
        // What the calls changed reaches the compositor's clients now.
        self.compositor.flush();
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
        if let Some((mut programs, children)) = self.programs.take() {
            self.end_programs(&mut programs, &children);
            programs.remove_folder();
        }
        let stop = self.stopping.take().unwrap_or(Stop::Signal);
        tracing::info!(reason = ?stop, "the MCP server stops");
        stop
    }

    /// Ends every process under Yuna: SIGTERM, then SIGKILL to those left after [`END_GRACE`],
    /// each wait cut short once none is left. Meanwhile the compositor still serves its clients,
    /// so that a program answering SIGTERM with a last roundtrip gets its answer.
    fn end_programs(&mut self, programs: &mut Programs, children: &OwnedFd) {
        for (signal, wait) in [(Signal::TERM, END_GRACE), (Signal::KILL, KILL_WAIT)] {
            if programs.signal_all(signal) == 0 {
                return;
            }
            let until = Instant::now() + wait;
            loop {
                programs.reap();
                if !programs.any_alive() {
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
                    "spawn" => self.spawn(arguments),
                    "windows" => match self.windows(&id, revision, arguments) {
                        Some(result) => result,
                        // It waits, and is answered later.
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

    /// An output mode from a call's values, each defaulting to the seats' default output's.
    fn mode_of(
        &self,
        width: Option<u32>,
        height: Option<u32>,
        rate: Option<f64>,
        scale: Option<f64>,
    ) -> Result<OutputMode, ToolResult> {
        let default = self.seat_output;
        OutputMode::new(
            width.unwrap_or(default.width()),
            height.unwrap_or(default.height()),
            rate.unwrap_or(f64::from(default.refresh_millihertz()) / 1_000.0),
            scale.unwrap_or(default.scale()),
        )
        .map_err(|reason| ToolResult::failed(reason.to_string()))
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
            } => match self.mode_of(width, height, rate, scale) {
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
        let client_pids = match self.compositor.remove_seat(seat) {
            Ok(pids) => pids,
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        let processes = match &self.programs {
            Some((programs, _)) => programs.seat_processes(seat, &client_pids),
            None => Vec::new(),
        };
        if processes.is_empty() {
            return Some(self.seat_closed(seat, &[], &[]));
        }
        for &pid in &processes {
            let _ = rustix::process::kill_process(pid, Signal::TERM);
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
        if self.closings[index].killed {
            let closing = self.closings.remove(index);
            let left: Vec<Pid> = closing
                .signalled
                .iter()
                .copied()
                .filter(|&pid| programs::is_alive(pid))
                .collect();
            self.answer_closing(closing, &left);
            return;
        }
        let closing = &self.closings[index];
        let mut targets: Vec<Pid> = closing
            .signalled
            .iter()
            .copied()
            .filter(|&pid| programs::is_alive(pid))
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

    /// Answers each closing whose processes have all ended.
    fn answer_closings_done(&mut self) {
        let mut index = 0;
        while index < self.closings.len() {
            if self.closings[index]
                .signalled
                .iter()
                .any(|&pid| programs::is_alive(pid))
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
            } => {
                let seat = match self.agent_seat(seat) {
                    Ok(seat) => seat,
                    Err(failed) => return failed,
                };
                let mode = match self.mode_of(width, height, rate, scale) {
                    Ok(mode) => mode,
                    Err(failed) => return failed,
                };
                match self.compositor.add_output(seat, mode) {
                    Ok(id) => ToolResult::done(json!({ "output": self.output_of(seat, id) })),
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
            PointerCall::Move { x, y, output, .. } => {
                self.compositor
                    .pointer_move(seat, output.map(OutputId), x, y)
            }
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
                ..
            } => self
                .move_to(seat, x, y, output)
                .and_then(|()| self.compositor.pointer_scroll(seat, dx, dy)),
        };
        match result {
            Ok(pointer) => ToolResult::done(json!({ "pointer": tools::pointer_json(&pointer) })),
            Err(error) => ToolResult::failed(error.to_string()),
        }
    }

    /// Moves `seat`'s pointer where a button call says, when it says.
    fn move_first(&mut self, seat: SeatId, button: &ButtonCall) -> Result<(), Error> {
        self.move_to(seat, button.x, button.y, button.output)
    }

    /// Moves `seat`'s pointer to (`x`, `y`) on `output` when both are given; refused for one
    /// without the other.
    fn move_to(
        &mut self,
        seat: SeatId,
        x: Option<f64>,
        y: Option<f64>,
        output: Option<u64>,
    ) -> Result<(), Error> {
        match (x, y) {
            (Some(x), Some(y)) => self
                .compositor
                .pointer_move(seat, output.map(OutputId), x, y)
                .map(|_| ()),
            (None, None) if output.is_none() => Ok(()),
            _ => Err(Error::Refused("give x and y together".to_owned())),
        }
    }

    fn keyboard(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call: KeyboardCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        let seat = match &call {
            KeyboardCall::Press { seat, .. }
            | KeyboardCall::Release { seat, .. }
            | KeyboardCall::Tap { seat, .. }
            | KeyboardCall::Type { seat, .. } => *seat,
        };
        let seat = match self.agent_seat(seat) {
            Ok(seat) => seat,
            Err(failed) => return failed,
        };
        let result = match call {
            KeyboardCall::Press { key, .. } => self.compositor.key(seat, &key, true),
            KeyboardCall::Release { key, .. } => self.compositor.key(seat, &key, false),
            KeyboardCall::Tap { keys, .. } => self.compositor.tap_keys(seat, &keys),
            KeyboardCall::Type { text, .. } => self.compositor.type_text(seat, &text),
        };
        match result {
            Ok(()) => ToolResult::done(json!({
                "keyboard_window": self.compositor.keyboard_window(seat).map(|window| window.0),
            })),
            Err(error) => ToolResult::failed(error.to_string()),
        }
    }

    fn spawn(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        let Some((programs, _)) = &mut self.programs else {
            return ToolResult::failed("this Yuna starts no program: run `yuna mcp`");
        };
        let seat = match &call {
            SpawnCall::Start { seat, .. }
            | SpawnCall::List { seat }
            | SpawnCall::Stop { seat, .. } => *seat,
        };
        let seat = SeatId(seat);
        if seat == OWNER_SEAT || !self.compositor.seats().contains(&seat) {
            return ToolResult::failed(format!(
                "no seat {}: create one with `seat create`",
                seat.0
            ));
        }
        match call {
            SpawnCall::Start {
                command, env, cwd, ..
            } => {
                let handle = self.compositor.loop_handle();
                let start = Start {
                    command,
                    env,
                    cwd: cwd.map(PathBuf::from),
                };
                let socket = programs.socket_path(seat);
                match programs.start(&handle, seat, &socket, start) {
                    Ok(program) => ToolResult::done(json!({
                        "program": tools::program_json(program)
                    })),
                    Err(reason) => ToolResult::failed(reason),
                }
            }
            SpawnCall::List { .. } => {
                let list: Vec<Value> = programs
                    .list(seat)
                    .into_iter()
                    .map(tools::program_json)
                    .collect();
                ToolResult::done(json!({ "programs": list }))
            }
            SpawnCall::Stop { program, force, .. } => match programs.stop(seat, program, force) {
                Ok(()) => ToolResult::done(json!({
                    "stopped": program,
                    "signal": if force { "SIGKILL" } else { "SIGTERM" },
                })),
                Err(reason) => ToolResult::failed(reason),
            },
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
        if self.waits.len() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        let key = self.next_wait;
        let events = self.events.clone();
        let timer = self.compositor.loop_handle().insert_source(
            Timer::from_duration(wait),
            move |_, _, state| {
                state.woke = true;
                events.borrow_mut().timed_out.push(key);
                TimeoutAction::Drop
            },
        );
        let timer = match timer {
            Ok(timer) => timer,
            Err(error) => return Some(ToolResult::failed(error.error.to_string())),
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

    fn matching_windows(&self, call: &WindowsCall) -> Vec<Value> {
        self.compositor
            .windows()
            .iter()
            .filter(|window| call.matches(window))
            .map(tools::window_json)
            .collect()
    }

    /// Answers each waiting call a window now matches.
    fn answer_waits(&mut self) {
        let windows = self.compositor.windows();
        let mut index = 0;
        while index < self.waits.len() {
            let matching: Vec<Value> = windows
                .iter()
                .filter(|window| self.waits[index].call.matches(window))
                .map(tools::window_json)
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

    /// Answers the call `key` whose time is up, if it still waits: no window matched.
    fn answer_timed_out(&mut self, key: u64) {
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
        for closing in &mut self.closings {
            if closing
                .reply
                .as_ref()
                .is_some_and(|(request, _)| request == id)
            {
                closing.reply = None;
            }
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
