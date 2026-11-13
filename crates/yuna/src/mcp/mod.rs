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
use smithay::reexports::rustix::process::Signal;
use yuna_layout::OutputId;

use crate::compositor::Compositor;
use crate::output::OutputMode;
use crate::{ClockMode, Error};
use programs::{Programs, Start, THROTTLE};
use protocol::{Call, Message, Revision, RpcError, Session};
use stdio::{LineReader, LineTooLong, OutputQueue};
use tools::{ClockCall, OutputsCall, SpawnCall, ToolResult, WindowsCall};

pub use programs::{
    END_GRACE, KEPT_PREFIXES, KEPT_VARIABLES, KILL_WAIT, LOG_LIMIT, MAX_KEPT, MAX_RUNNING,
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
    next_wait: u64,
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
    /// a pipe that SIGCHLD writes to, in a process that is its children's subreaper, `spawn`
    /// starts programs; their folder is made in `XDG_RUNTIME_DIR`, which the compositor's socket
    /// must be in. Without, `spawn` is refused.
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
                let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
                let (Some(runtime_dir), Some(socket)) = (runtime_dir, compositor.socket_name())
                else {
                    return Err(Error::Socket(
                        "programs need Yuna's socket in XDG_RUNTIME_DIR".to_owned(),
                    ));
                };
                let socket = runtime_dir.join(socket);
                let programs = Programs::new(&runtime_dir, socket)
                    .map_err(|e| failed("programs' folder", &e))?;
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
            programs,
            throttle_armed: false,
            stopping: None,
        })
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
                    "outputs" => self.outputs(arguments),
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

    fn outputs(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        match call {
            OutputsCall::List {} => {
                let outputs: Vec<Value> = self
                    .compositor
                    .output_descriptions()
                    .iter()
                    .map(tools::output_json)
                    .collect();
                ToolResult::done(json!({ "outputs": outputs }))
            }
            OutputsCall::Create {
                width,
                height,
                rate,
                scale,
            } => {
                let default = OutputMode::default();
                let mode = match OutputMode::new(
                    width.unwrap_or(default.width()),
                    height.unwrap_or(default.height()),
                    rate.unwrap_or(f64::from(default.refresh_millihertz()) / 1_000.0),
                    scale.unwrap_or(default.scale()),
                ) {
                    Ok(mode) => mode,
                    Err(reason) => return ToolResult::failed(reason.to_string()),
                };
                match self.compositor.add_output(mode) {
                    Ok(id) => {
                        let output = self
                            .compositor
                            .output_descriptions()
                            .into_iter()
                            .find(|output| output.id == id)
                            .map(|output| tools::output_json(&output));
                        ToolResult::done(json!({ "output": output }))
                    }
                    Err(error) => ToolResult::failed(error.to_string()),
                }
            }
            OutputsCall::Remove { output } => {
                match self.compositor.remove_output(OutputId(output)) {
                    Ok(()) => ToolResult::done(json!({ "removed": output })),
                    Err(error) => ToolResult::failed(error.to_string()),
                }
            }
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
        match call {
            SpawnCall::Start { command, env, cwd } => {
                let handle = self.compositor.loop_handle();
                let start = Start {
                    command,
                    env,
                    cwd: cwd.map(PathBuf::from),
                };
                match programs.start(&handle, start) {
                    Ok(program) => ToolResult::done(json!({
                        "program": tools::program_json(program)
                    })),
                    Err(reason) => ToolResult::failed(reason),
                }
            }
            SpawnCall::List {} => {
                let list: Vec<Value> = programs.list().iter().map(tools::program_json).collect();
                ToolResult::done(json!({ "programs": list }))
            }
            SpawnCall::Stop { program, force } => match programs.stop(program, force) {
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

    /// Drops the waiting calls of request `id`, unanswered, as MCP asks.
    fn cancel(&mut self, id: &Value) {
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
