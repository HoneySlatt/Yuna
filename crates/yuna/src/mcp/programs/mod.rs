//! The programs an agent starts through `spawn`, in `yuna mcp`
//! ([ADR-0033](../../../../../docs/adr/0033-the-mcp-server.md),
//! [ADR-0078](../../../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)).
//!
//! Each program gets an environment of its own (`environment.rs`) and its seat's throwaway home
//! (`homes.rs`). Its standard input is `/dev/null`; its output and errors go, through a pipe
//! Yuna reads, to a log the agent reads through `spawn log` (`log.rs`). A seat's accessibility
//! bus (`../accessibility/bus.rs`) runs `dbus-daemon` with an environment of Yuna's kept
//! variables alone: its own folder, `seat-<id>.a11y`, is in the programs' folder (`folder.rs`).
//! When a seat closes, its home goes with its programs' logs and runtime folders.
//!
//! Each program leads a process group of its own, which `stop` signals, and which `freeze`,
//! `thaw` and `slow` stop and continue (`pace.rs`). A client's program is the one its process
//! was, or was under, when it connected ([`Programs::program_of`]). Yuna is its programs'
//! subreaper (the program sets it, `main.rs`): a process whose parent dies becomes Yuna's child,
//! so that the whole tree stays Yuna's to reap and to end (`processes.rs`). When a seat closes,
//! the processes that belong to it are ended ([`Programs::seat_processes`]).
//!
//! This module holds [`Programs`] and each [`Program`]. Their other parts are modules of their
//! own, each read alone:
//!
//! - `pace.rs`: a program frozen, thawed or slowed, and its pace ended;
//! - `environment.rs`: the variables a program and a seat's `dbus-daemon` get, and the seat a
//!   process's environment names;
//! - `homes.rs`: the seats' throwaway homes, and their folder in `TMPDIR`;
//! - `log.rs`: a program's log, and the source that reads its output into it;
//! - `folder.rs`: the programs' folder and the homes', their locks, and the sweep of the folders
//!   gone Yunas left;
//! - `processes.rs`: the processes under Yuna, found through `/proc`, signalled, reaped and
//!   ended, by the fatal path too;
//! - `tests.rs`: their tests.

mod environment;
mod folder;
mod homes;
mod log;
mod pace;
mod processes;
#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Instant;

use rustix::pipe::{PipeFlags, pipe_with};
use rustix::process::{Pid, Signal, WaitStatus};
use smithay::reexports::calloop::{LoopHandle, RegistrationToken};

use yuna_layout::SeatId;

use crate::compositor::{ClientProcess, ClientProtocolError};
use crate::process::keep_descriptors_from_the_program;
use crate::rest::{Source, Wake};
use crate::state::{OWNER_SEAT, State};
use crate::xwayland::X11Display;
use environment::{add_accessibility_variables, program_environment, seat_in_environment};
use folder::{PROGRAMS_FOLDER, remove_folder};
use homes::{Homes, add_home_variables, needs_home};
use log::{ProgramLog, Throttled, watch_output};
use pace::unpace;
use processes::{processes_under, reap_children};

pub use environment::{
    ACCESSIBILITY_VARIABLES, BUS_VARIABLE, KEPT_PREFIXES, KEPT_VARIABLES,
    NO_ACCESSIBILITY_VARIABLES, SEAT_VARIABLE, daemon_environment,
};
pub use folder::LOCK_FILE;
pub use homes::HOMES_LINK;
pub use log::{LOG_HEAD, LOG_TAIL, LogRead, THROTTLE};
pub use pace::{PACE_PERIOD, Pace};
pub use processes::{
    END_GRACE, KILL_WAIT, any_alive, drain, end_after_panic, living, send_ending_signal, signal_all,
};

/// How many programs may run at once.
pub const MAX_RUNNING: usize = 64;

/// How many programs are remembered, those that ended included; past it, the oldest that ended
/// is forgotten, and its log deleted.
pub const MAX_KEPT: usize = 128;

/// A program started by `spawn`.
#[derive(Debug)]
pub struct Program {
    /// Its id, from 1, never reused.
    pub id: u64,
    /// The seat it was started on.
    pub seat: SeatId,
    pub pid: Pid,
    pub command: Vec<String>,
    /// Whether it was started with accessibility, on its seat's bus.
    pub accessible: bool,
    /// How it ended; none while it runs.
    pub end: Option<End>,
    /// Its log file: its standard output and errors, in the order they came, its first
    /// [`LOG_HEAD`] bytes, then a ring of its newest [`LOG_TAIL`] once it wrote more.
    pub log: PathBuf,
    /// The last protocol error one of its Wayland clients was ended for.
    pub protocol_error: Option<ClientProtocolError>,
    /// When it started, and when it was reaped: a client connected between was its, when its
    /// process was the program's or under it ([`Programs::program_of`]).
    started: Instant,
    reaped: Option<Instant>,
    log_state: Rc<RefCell<ProgramLog>>,
    pace: Rc<Cell<Pace>>,
    /// The timer that stops and continues it while it is slowed.
    pace_timer: Option<RegistrationToken>,
    /// Times its pace's timer woke Yuna.
    pace_wakes: Rc<Cell<u64>>,
}

/// What woke Yuna for one program: what the `rest` tool names it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramWakes {
    pub seat: SeatId,
    pub program: u64,
    /// Rounds in which it wrote to its output, or closed it.
    pub output: u64,
    /// Times its pace stopped or continued it.
    pub pace: u64,
}

impl Program {
    /// What woke Yuna for it.
    pub fn wakes(&self) -> ProgramWakes {
        ProgramWakes {
            seat: self.seat,
            program: self.id,
            output: self.log_state.borrow().wakes,
            pace: self.pace_wakes.get(),
        }
    }

    /// Bytes of output its log holds.
    pub fn log_kept(&self) -> u64 {
        self.log_state.borrow().kept()
    }

    /// Whether a process of its group still runs: the program, or one it started and did not
    /// move to a group of its own.
    pub fn group_runs(&self) -> bool {
        rustix::process::test_kill_process_group(self.pid).is_ok()
    }

    /// Bytes of output its log dropped: past its first [`LOG_HEAD`] and before its newest
    /// [`LOG_TAIL`].
    pub fn log_dropped(&self) -> u64 {
        self.log_state.borrow().dropped()
    }

    /// How it is paced now: free once its whole group has ended, whatever it was asked.
    pub fn pace(&self) -> Pace {
        match self.pace.get() {
            Pace::Free => Pace::Free,
            paced if self.group_runs() => paced,
            _ => Pace::Free,
        }
    }

    /// Up to `max_bytes` of its log from byte `from` of its output, or its newest without.
    pub fn read_log(&self, from: Option<u64>, max_bytes: u64) -> Result<LogRead, String> {
        self.log_state.borrow().read(from, max_bytes)
    }
}

/// How a program ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// It exited with this status.
    Exited(i32),
    /// A signal ended it.
    Killed(i32),
}

/// What `spawn` asks to start.
#[derive(Debug)]
pub struct Start {
    /// The program and its arguments; the program is looked for in the program's `PATH`.
    pub command: Vec<String>,
    /// Variables added to its environment, or replacing what it would have.
    pub env: BTreeMap<String, String>,
    /// Its working folder; Yuna's own without one.
    pub cwd: Option<PathBuf>,
    /// The address of its seat's accessibility bus, when it is started with accessibility.
    pub accessibility: Option<String>,
}

/// The programs of one `yuna mcp`, the folder of the seats' sockets, the programs' logs and
/// their runtime folders, and the seats' homes.
#[derive(Debug)]
pub struct Programs {
    folder: PathBuf,
    /// The folder's lock file, locked while this Yuna lives ([`LOCK_FILE`]).
    _lock: File,
    homes: Homes,
    list: Vec<Program>,
    next_id: u64,
    /// The output sources that stopped reading once their program wrote more than its log
    /// keeps, until the next [`THROTTLE`] ends.
    throttled: Throttled,
}

impl Programs {
    /// Creates the folder of the seats' sockets, the programs' logs and their runtime folders,
    /// `yuna-mcp-<pid>` in `runtime_dir`, readable by its user alone, and locks its
    /// [`LOCK_FILE`] for as long as this Yuna lives (`folder.rs`). First removes the folders of
    /// Yunas that are gone, in `runtime_dir` and in the temporary folder, where the seats'
    /// homes go (`homes.rs`); those are made at their seats' first programs.
    pub fn new(runtime_dir: &Path) -> io::Result<Programs> {
        PROGRAMS_FOLDER.remove_stale(runtime_dir);
        let homes = Homes::new(homes::temp_dir());
        let (folder, lock) = PROGRAMS_FOLDER.make(runtime_dir)?;
        Ok(Programs {
            folder,
            _lock: lock,
            homes,
            list: Vec::new(),
            next_id: 1,
            throttled: Throttled::default(),
        })
    }

    /// Where seat `seat`'s Wayland socket goes, in the folder.
    pub fn socket_path(&self, seat: SeatId) -> PathBuf {
        self.folder.join(format!("seat-{}", seat.0))
    }

    /// The folder of seat `seat`'s accessibility bus, in the folder.
    pub fn accessibility_folder(&self, seat: SeatId) -> PathBuf {
        self.folder.join(format!("seat-{}.a11y", seat.0))
    }

    /// A log at `path`, made anew, kept as a program's is: the write end of a pipe whose output
    /// a source of `handle`'s loop reads into it, its first [`LOG_HEAD`] bytes and its newest
    /// [`LOG_TAIL`], throttled past them with the programs' own. What `seat`'s dbus-daemon
    /// writes goes there.
    pub fn bounded_log(
        &self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        path: &Path,
    ) -> io::Result<OwnedFd> {
        let log = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        let (output_read, output_write) = pipe_with(PipeFlags::CLOEXEC)?;
        let log_state = Rc::new(RefCell::new(ProgramLog::new(log)));
        let wake = Wake::of_seat(Source::AccessibilityLog, seat);
        watch_output(handle, wake, output_read, log_state, self.throttled.clone())?;
        Ok(output_write)
    }

    /// The folder of the seats' sockets, the programs' logs and their runtime folders.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The programs of `seat` remembered, in the order they started.
    pub fn list(&self, seat: SeatId) -> Vec<&Program> {
        self.list.iter().filter(|p| p.seat == seat).collect()
    }

    /// What woke Yuna for each program remembered, every seat's, in the order they started.
    pub fn wakes(&self) -> Vec<ProgramWakes> {
        self.list.iter().map(Program::wakes).collect()
    }

    /// Program `id` of `seat`, while it is remembered; none for another seat's.
    pub fn program(&self, seat: SeatId, id: u64) -> Option<&Program> {
        self.list
            .iter()
            .find(|program| program.id == id && program.seat == seat)
    }

    /// Starts a program on `seat`, whose clients connect through `socket`, and its X11 clients
    /// to `x11` when the seat has an X display, in its seat's home, its output read by a source
    /// of `handle`'s loop into its log. Refused, with the reason, for an empty command, a
    /// variable that cannot be one, more than [`MAX_RUNNING`] programs, all seats together, a
    /// home that cannot be made, or a program that cannot start.
    pub fn start(
        &mut self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        socket: &Path,
        x11: Option<&X11Display>,
        start: Start,
    ) -> Result<&Program, String> {
        let Some(program) = start.command.first() else {
            return Err("the command is empty: name a program".to_owned());
        };
        if program.is_empty() {
            return Err("the program's name is empty".to_owned());
        }
        if let Some(bad) = start
            .command
            .iter()
            .chain(start.env.values())
            .find(|text| text.contains('\0'))
        {
            return Err(format!("{bad:?} holds a NUL character"));
        }
        if let Some(bad) = start
            .env
            .keys()
            .find(|name| name.is_empty() || name.contains(['=', '\0']))
        {
            return Err(format!("{bad:?} cannot name a variable"));
        }
        if self.list.iter().filter(|p| p.end.is_none()).count() >= MAX_RUNNING {
            return Err(format!(
                "{MAX_RUNNING} programs run already: stop one first"
            ));
        }
        let home = if needs_home(&start.env) {
            let home = self.homes.seat_home(seat, &self.folder);
            Some(home.map_err(|e| format!("its seat's home cannot be made: {e}"))?)
        } else {
            None
        };
        // Taken first, so that a start that fails leaves no folder or log a later one would
        // find in its way.
        let id = self.next_id;
        self.next_id += 1;
        let runtime = self.folder.join("runtime").join(id.to_string());
        let log = self.folder.join("logs").join(format!("{id}.log"));
        let mut environment =
            program_environment(std::env::vars_os(), &runtime, socket, x11, seat, &start.env);
        if let Some(home) = &home {
            add_home_variables(&mut environment, home, &start.env);
        }
        add_accessibility_variables(&mut environment, start.accessibility.as_deref(), &start.env);
        let (pid, log_state) = match self.launch(handle, seat, environment, &start, &runtime, &log)
        {
            Ok(launched) => launched,
            Err(reason) => {
                let _ = fs::remove_file(&log);
                let _ = fs::remove_dir_all(&runtime);
                return Err(reason);
            }
        };
        tracing::info!(program = id, seat = seat.0, pid = pid.as_raw_nonzero().get(), command = ?start.command, "program started");
        self.list.push(Program {
            id,
            seat,
            pid,
            command: start.command,
            accessible: start.accessibility.is_some(),
            end: None,
            log,
            protocol_error: None,
            started: Instant::now(),
            reaped: None,
            log_state,
            pace: Rc::new(Cell::new(Pace::Free)),
            pace_timer: None,
            pace_wakes: Rc::default(),
        });
        self.forget_oldest_ended(handle);
        // Only programs that ended are forgotten: this one is still last.
        self.list
            .last()
            .ok_or_else(|| "the program was forgotten".to_owned())
    }

    /// Makes a program's runtime folder and log, starts it on `seat` with `environment`, and
    /// reads its output into the log.
    fn launch(
        &self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        environment: BTreeMap<OsString, OsString>,
        start: &Start,
        runtime: &Path,
        log_path: &Path,
    ) -> Result<(Pid, Rc<RefCell<ProgramLog>>), String> {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(runtime)
            .map_err(|e| format!("its runtime folder cannot be created: {e}"))?;
        let log = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(log_path)
            .map_err(|e| format!("its log cannot be created: {e}"))?;
        let (output_read, output_write) = pipe_with(PipeFlags::CLOEXEC)
            .map_err(|e| format!("its output's pipe cannot be created: {e}"))?;
        let errors_write = output_write
            .try_clone()
            .map_err(|e| format!("its output's pipe cannot be shared: {e}"))?;
        let program = &start.command[0];
        let mut command = Command::new(program);
        command
            .args(&start.command[1..])
            .env_clear()
            .envs(environment)
            .stdin(Stdio::null())
            .stdout(Stdio::from(output_write))
            .stderr(Stdio::from(errors_write))
            .process_group(0);
        if let Some(cwd) = &start.cwd {
            command.current_dir(cwd);
        }
        keep_descriptors_from_the_program(&mut command);
        let child = command
            .spawn()
            .map_err(|e| format!("{program:?} cannot start: {e}"))?;
        // `Command` holds the write ends until it is dropped: the log sees the end of the output
        // only once the program and its children close theirs.
        drop(command);
        let pid = Pid::from_child(&child);
        // The child is reaped through SIGCHLD (`reap`), not through `Child::wait`.
        drop(child);
        let log_state = Rc::new(RefCell::new(ProgramLog::new(log)));
        if let Err(error) = watch_output(
            handle,
            Wake::of_seat(Source::ProgramOutput, seat),
            output_read,
            log_state.clone(),
            self.throttled.clone(),
        ) {
            tracing::warn!(%error, %program, "a program's output cannot be read");
            log_state.borrow_mut().ended = true;
        }
        Ok((pid, log_state))
    }

    /// Sends program `id` of `seat` its process group SIGTERM, then SIGCONT, or SIGKILL when
    /// `force`: the processes it started go too, even once it has ended itself, stopped ones
    /// included. Its pace ends first. Refused for an unknown program, another seat's, and one
    /// whose whole group has ended.
    pub fn stop(
        &mut self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        id: u64,
        force: bool,
    ) -> Result<(), String> {
        let program = self.program_mut(seat, id)?;
        unpace(handle, program);
        let signal = if force { Signal::KILL } else { Signal::TERM };
        signal_group(program, signal)?;
        if signal == Signal::TERM {
            let _ = signal_group(program, Signal::CONT);
        }
        Ok(())
    }

    /// Program `id` of `seat`; refused for an unknown program and for another seat's.
    fn program_mut(&mut self, seat: SeatId, id: u64) -> Result<&mut Program, String> {
        self.list
            .iter_mut()
            .find(|program| program.id == id && program.seat == seat)
            .ok_or_else(|| format!("no program {id} on seat {}", seat.0))
    }

    /// The program of `seat` a client belongs to, `process` as it connected: the one that was
    /// its process, or above it, when it connected, alive then, so that its pid was its own;
    /// else the latest one started before that whose process group it was in, as a process of
    /// a launcher that exited is. None for a client of no program's, or of a program
    /// forgotten.
    pub fn program_of(&self, seat: SeatId, process: &ClientProcess) -> Option<u64> {
        let at = process.connected_at;
        let before = |program: &&Program| program.seat == seat && program.started <= at;
        let pid = |program: &Program| program.pid.as_raw_nonzero().get();
        self.list
            .iter()
            .filter(before)
            .filter(|program| program.reaped.is_none_or(|reaped| reaped > at))
            .find(|program| process.lineage.contains(&pid(program)))
            .or_else(|| {
                self.list
                    .iter()
                    .filter(before)
                    .rfind(|program| process.group == Some(pid(program)))
            })
            .map(|program| program.id)
    }

    /// Records `error` as the last protocol error of program `id`, a client of its was ended
    /// for.
    pub fn note_protocol_error(&mut self, id: u64, error: ClientProtocolError) {
        if let Some(program) = self.list.iter_mut().find(|program| program.id == id) {
            program.protocol_error = Some(error);
        }
    }

    /// Reaps every child that ended, programs, orphans Yuna adopted and Xwaylands alike, and
    /// records how each program ended; returns those programs, with their seats. Gives
    /// `not_a_program` the pid and status of every other child, so that the compositor learns
    /// how an Xwayland ended, which this reaping took before its own wait on it could. Run when
    /// SIGCHLD has come.
    pub fn reap(
        &mut self,
        mut not_a_program: impl FnMut(Pid, WaitStatus),
    ) -> Vec<(SeatId, u64, End)> {
        let mut ended = Vec::new();
        reap_children(|pid, status| {
            let end = match (status.exit_status(), status.terminating_signal()) {
                (Some(code), _) => End::Exited(code),
                (None, Some(signal)) => End::Killed(signal),
                // Stopped or continued: not an end.
                (None, None) => return,
            };
            match self
                .list
                .iter_mut()
                .find(|p| p.pid == pid && p.end.is_none())
            {
                Some(program) => {
                    tracing::info!(program = program.id, ?end, "program ended");
                    program.end = Some(end);
                    program.reaped = Some(Instant::now());
                    ended.push((program.seat, program.id, end));
                }
                None => not_a_program(pid, status),
            }
        });
        ended
    }

    /// Whether an output source waits for the end of a [`THROTTLE`].
    pub fn has_throttled(&self) -> bool {
        !self.throttled.borrow().is_empty()
    }

    /// Lets the output sources that waited for a [`THROTTLE`] read again.
    pub fn resume_throttled(&self, handle: &LoopHandle<'static, State>) {
        for source in self.throttled.borrow_mut().drain(..) {
            if let Some(token) = source.get() {
                let _ = handle.enable(&token);
            }
        }
    }

    /// The living processes under Yuna that belong to `seat`, whose Wayland clients have the
    /// process ids `client_pids`; `last_seat` is the highest seat id given so far, every seat up
    /// to it existing now or before, since ids are given in order and never reused.
    ///
    /// Lineage decides first: a process that is, or is under, one of the seat's programs still
    /// running is the seat's, whatever its environment names; one that is, or is under, another
    /// seat's running program is that seat's. A running program is not reaped yet: its pid is
    /// its own, never reused. Otherwise a process belongs to the seat when it, or a process above
    /// it under Yuna, is in the process group one of the seat's programs led, running or ended
    /// (an orphan stays in it), is one of its clients, or has the seat's `YUNA_SEAT` in its
    /// environment; and when nothing says it is another seat's: the `YUNA_SEAT` of another seat
    /// that exists or existed, or the group of another seat's running program, so that a pid an
    /// ended program freed and another seat's program took is never ended with this seat. A
    /// process that leaves its group and its parents and empties its environment is not found.
    pub fn seat_processes(&self, seat: SeatId, client_pids: &[i32], last_seat: SeatId) -> Vec<Pid> {
        let groups: Vec<i32> = self
            .list
            .iter()
            .filter(|program| program.seat == seat)
            .map(|program| program.pid.as_raw_nonzero().get())
            .collect();
        let running: Vec<i32> = self
            .list
            .iter()
            .filter(|program| program.seat == seat && program.end.is_none())
            .map(|program| program.pid.as_raw_nonzero().get())
            .collect();
        // Another seat's running programs: a process that is one, or under one, or in one's
        // group, is that seat's, whatever a reused pid says.
        let others: Vec<i32> = self
            .list
            .iter()
            .filter(|program| program.seat != seat && program.end.is_none())
            .map(|program| program.pid.as_raw_nonzero().get())
            .collect();
        let processes = processes_under(rustix::process::getpid());
        // Whether the process is the seat's by its lineage: none when no running program is it
        // or above it.
        let lineage = |pid: i32| -> Option<bool> {
            let mut current = pid;
            while let Some(process) = processes.get(&current) {
                if running.contains(&current) {
                    return Some(true);
                }
                if others.contains(&current) {
                    return Some(false);
                }
                current = process.parent;
            }
            None
        };
        // A seat a process's environment names counts only if it exists or existed: a variable
        // naming any other seat says nothing of the process.
        let named_seat = |pid: i32| {
            seat_in_environment(pid).filter(|named| (OWNER_SEAT.0..=last_seat.0).contains(&named.0))
        };
        let mut memo: HashMap<i32, bool> = HashMap::new();
        let mut member = |pid: i32| -> bool {
            // From the process up to Yuna, the first one found decides for those below it.
            let mut chain = Vec::new();
            let mut current = pid;
            let found = loop {
                if let Some(&known) = memo.get(&current) {
                    break known;
                }
                let Some(process) = processes.get(&current) else {
                    break false;
                };
                chain.push(current);
                let named = named_seat(current);
                if named.is_some_and(|named| named != seat) || others.contains(&process.group) {
                    break false;
                }
                let direct = groups.contains(&process.group)
                    || client_pids.contains(&current)
                    || named == Some(seat);
                if direct {
                    break true;
                }
                current = process.parent;
            };
            for pid in chain {
                memo.insert(pid, found);
            }
            found
        };
        let mut found: Vec<Pid> = processes
            .iter()
            .filter(|(_, process)| !process.zombie)
            .map(|(&pid, _)| pid)
            .filter(|&pid| lineage(pid).unwrap_or_else(|| member(pid)))
            .filter_map(Pid::from_raw)
            .collect();
        found.sort_by_key(|pid| pid.as_raw_nonzero().get());
        found
    }

    /// Forgets the programs of `seat`, which closed, and deletes their logs and runtime
    /// folders, and the seat's home. Their pace has ended already ([`Programs::unpace_seat`]).
    pub fn forget_seat(&mut self, seat: SeatId) {
        self.homes.remove_seat(seat);
        let folder = self.folder.clone();
        self.list.retain(|program| {
            let kept = program.seat != seat;
            if !kept {
                let _ = fs::remove_file(&program.log);
                let _ = fs::remove_dir_all(folder.join("runtime").join(program.id.to_string()));
            }
            kept
        });
    }

    /// Removes the seats' homes and the folder of logs and runtime folders, once every program
    /// has ended.
    pub fn remove_folder(self) {
        self.homes.remove();
        remove_folder(&self.folder);
    }

    /// Forgets the oldest programs that ended while more than [`MAX_KEPT`] are remembered, and
    /// deletes their logs and runtime folders. The group of one still paced, which no call can
    /// name any more, is left to run as it likes.
    fn forget_oldest_ended(&mut self, handle: &LoopHandle<'static, State>) {
        while self.list.len() > MAX_KEPT {
            let Some(index) = self.list.iter().position(|p| p.end.is_some()) else {
                return;
            };
            let mut program = self.list.remove(index);
            if program.pace() != Pace::Free {
                unpace(handle, &mut program);
                let _ = signal_group(&program, Signal::CONT);
            }
            let _ = fs::remove_file(&program.log);
            let _ = fs::remove_dir_all(self.folder.join("runtime").join(program.id.to_string()));
        }
    }
}

/// Sends `signal` to `program`'s process group; refused, with why, when its whole group has
/// ended.
fn signal_group(program: &Program, signal: Signal) -> Result<(), String> {
    match rustix::process::kill_process_group(program.pid, signal) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::SRCH) => Err(format!(
            "program {} has ended, and every process of its group with it",
            program.id
        )),
        Err(error) => Err(format!(
            "program {} cannot be signalled: {error}",
            program.id
        )),
    }
}
