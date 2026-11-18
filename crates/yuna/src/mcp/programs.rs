//! The programs an agent starts through `spawn`, in `yuna mcp`
//! ([ADR-0033](../../../../docs/adr/0033-the-mcp-server.md),
//! [ADR-0078](../../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)).
//!
//! Each program gets an environment of its own: the few variables of Yuna's own that a program
//! needs to find itself, its libraries, its locale and its drivers ([`KEPT_VARIABLES`],
//! [`KEPT_PREFIXES`]), with the caller's own values of those Yuna's package set for Yuna alone
//! ([`WRAPPED_VARIABLES`]); `WAYLAND_DISPLAY`, the full path of its seat's socket; `DISPLAY` and
//! `XAUTHORITY`, its seat's X display and cookie, when the seat has one
//! ([ADR-0067](../../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md));
//! and an `XDG_RUNTIME_DIR` of its own, empty, so that it finds neither the user's D-Bus nor
//! their sound server there; with a `DISPLAY`, `DBUS_SESSION_BUS_ADDRESS=disabled:`, so that
//! D-Bus's library does not launch a bus of its own through X11, which would hold the seat's
//! Xwayland. Nothing else: not Yuna's own `DISPLAY`, not the user's bus. The agent's own
//! variables come next and win, but for `YUNA_SEAT`, its seat's id, which comes last. Its
//! standard input is `/dev/null`; its output and errors go, through a pipe Yuna reads, to a log
//! file the agent reads through `spawn log`, which keeps its first [`LOG_HEAD`] bytes and its
//! newest [`LOG_TAIL`] ([`LogRead`]).
//!
//! Each program leads a process group of its own, which `stop` signals, and which `freeze`,
//! `thaw` and `slow` stop and continue ([`Pace`]). Every path that ends processes with SIGTERM
//! sends SIGCONT right after it, as systemd does, so that a stopped process ends at once rather
//! than at SIGKILL. A client's program is the one its process was, or was under, when it
//! connected ([`Programs::program_of`]). Yuna is its programs'
//! subreaper (the program sets it, `main.rs`): a process whose parent dies becomes Yuna's
//! child, so that the whole tree stays Yuna's to reap and to end. When Yuna stops, every process
//! under it gets SIGTERM, then SIGKILL [`END_GRACE`] later if it is still there, while Yuna still
//! serves its Wayland clients (`mod.rs`). When a seat closes, the processes that belong to it
//! are ended the same way ([`Programs::seat_processes`]). Exits are read when SIGCHLD arrives,
//! never polled: every child is reaped there, and the status of one that is not a program, an
//! Xwayland's, goes back to the compositor, whose own wait on it then finds it reaped. The folder
//! that holds the seats' sockets, the logs and the programs' runtime folders is removed when Yuna
//! stops; one that a Yuna killed by SIGKILL left is removed when the next `yuna mcp` starts, its
//! lock free: Yuna holds its folder's [`LOCK_FILE`] locked while it lives ([`Programs::new`]).
//!
//! A panic of Yuna's state stops it at once
//! ([ADR-0080](../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)): its
//! panic hook, which cannot run the event loop, ends the programs and removes the folder through
//! [`end_after_panic`], the same signals and the same walk through `/proc`, its waits slept.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use smithay::reexports::rustix;
use smithay::reexports::rustix::pipe::{PipeFlags, pipe_with};
use smithay::reexports::rustix::process::{Pid, Signal, WaitOptions, WaitStatus};

use yuna_layout::SeatId;

use crate::compositor::{ClientProcess, ClientProtocolError};
use crate::process::{ProcessStat, keep_descriptors_from_the_program, read_stat};
use crate::state::State;
use crate::xwayland::X11Display;

/// The variables of Yuna's environment a program keeps.
pub const KEPT_VARIABLES: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TZDIR",
    "LOCALE_ARCHIVE",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_DIRS",
    "LD_LIBRARY_PATH",
    "XKB_CONFIG_ROOT",
    "FONTCONFIG_FILE",
    "FONTCONFIG_PATH",
    "LIBGL_DRIVERS_PATH",
    "LIBGL_ALWAYS_SOFTWARE",
    "GBM_BACKENDS_PATH",
    "__EGL_VENDOR_LIBRARY_FILENAMES",
    "__EGL_VENDOR_LIBRARY_DIRS",
];

/// The variable in which Yuna's package names, separated by spaces, the variables it set for
/// Yuna alone: its own libraries and Xwayland (`flake.nix`).
pub const WRAPPED_VARIABLES: &str = "YUNA_WRAPPED_VARIABLES";

/// The prefix of the variables in which Yuna's package keeps the caller's own value of each
/// variable [`WRAPPED_VARIABLES`] names, when the caller had one: a program gets it back.
pub const CALLER_PREFIX: &str = "YUNA_CALLER_";

/// The prefixes of the variables a program keeps: the locale's categories, and the settings of
/// Vulkan's loader and of Mesa.
pub const KEPT_PREFIXES: &[&str] = &["LC_", "VK_", "MESA_"];

/// How many programs may run at once.
pub const MAX_RUNNING: usize = 64;

/// How many programs are remembered, those that ended included; past it, the oldest that ended
/// is forgotten, and its log deleted.
pub const MAX_KEPT: usize = 128;

/// How much of the start of a program's output its log keeps: 256 KiB, where a failure to start
/// shows.
pub const LOG_HEAD: u64 = 256 << 10;

/// How much of the newest of a program's output its log keeps, after its first [`LOG_HEAD`]:
/// 1 MiB. What comes between is counted and dropped, so that a program that never stops writing
/// fills neither the disk nor memory: the logs are in `XDG_RUNTIME_DIR`, most often in memory,
/// [`MAX_KEPT`] of them at most, 160 MiB together at worst.
pub const LOG_TAIL: u64 = 1 << 20;

/// How long a slowed program's period is: it runs its share of each, and is stopped the rest.
pub const PACE_PERIOD: Duration = Duration::from_millis(100);

/// How long the programs have, once Yuna stops, between SIGTERM and SIGKILL.
pub const END_GRACE: Duration = Duration::from_secs(2);

/// How long Yuna waits, after SIGKILL, for the killed to be reaped.
pub const KILL_WAIT: Duration = Duration::from_secs(1);

/// How often the fatal path, which has no event loop to wake it, looks again whether the
/// processes it signalled have ended: each look reads `/proc`.
const ENDING_POLL: Duration = Duration::from_millis(10);

/// How much of a program's output is read at a time.
const READ_SIZE: usize = 64 * 1024;

/// How often a program's output is read once it has written more than its log keeps.
pub const THROTTLE: Duration = Duration::from_millis(100);

/// A program started by `spawn`.
#[derive(Debug)]
pub struct Program {
    /// Its id, from 1, never reused.
    pub id: u64,
    /// The seat it was started on.
    pub seat: SeatId,
    pub pid: Pid,
    pub command: Vec<String>,
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
}

impl Program {
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

/// How much of the time a program's process group may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    /// As it likes.
    Free,
    /// Stopped (SIGSTOP), until thawed.
    Frozen,
    /// Running this many hundredths of each [`PACE_PERIOD`], from 1 to 99, stopped the rest.
    Slow(u8),
}

impl Pace {
    /// The share of the time it may run, in hundredths: 100 free, 0 frozen.
    pub fn run_percent(self) -> u8 {
        match self {
            Pace::Free => 100,
            Pace::Frozen => 0,
            Pace::Slow(percent) => percent,
        }
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

/// A read of a program's log, as `spawn log` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRead {
    /// Bytes `from` to `to` of its output, as UTF-8, any invalid sequence replaced.
    pub text: String,
    /// Where the bytes start and end, in bytes since the program started.
    pub from: u64,
    pub to: u64,
    /// Every byte it wrote.
    pub size: u64,
    /// The bytes its log did not keep.
    pub dropped: u64,
    /// Whether its output has ended: every process that held its pipe has closed it.
    pub ended: bool,
}

/// A program's log: its file, which holds its first [`LOG_HEAD`] bytes, then its newest
/// [`LOG_TAIL`] in a ring, byte `n` past the head at `LOG_HEAD + (n - LOG_HEAD) % LOG_TAIL`.
#[derive(Debug)]
struct ProgramLog {
    file: File,
    /// Every byte the program wrote.
    size: u64,
    /// The bytes put in the file: `size`, unless a write failed, after which none is.
    written: u64,
    failed: bool,
    ended: bool,
}

impl ProgramLog {
    fn new(file: File) -> ProgramLog {
        ProgramLog {
            file,
            size: 0,
            written: 0,
            failed: false,
            ended: false,
        }
    }

    /// Where byte `offset` of the output is in the file.
    fn position(offset: u64) -> u64 {
        if offset < LOG_HEAD {
            offset
        } else {
            LOG_HEAD + (offset - LOG_HEAD) % LOG_TAIL
        }
    }

    /// How many bytes from byte `offset` lie in the file before its head's end or the ring's
    /// end, whichever comes first.
    fn contiguous_from(offset: u64) -> u64 {
        if offset < LOG_HEAD {
            LOG_HEAD - offset
        } else {
            LOG_HEAD + LOG_TAIL - Self::position(offset)
        }
    }

    /// The end of the head kept, and the start of the tail kept: equal when nothing was
    /// dropped between.
    fn kept_bounds(&self) -> (u64, u64) {
        let head_end = self.written.min(LOG_HEAD);
        let tail_start = head_end.max(self.written.saturating_sub(LOG_TAIL));
        (head_end, tail_start)
    }

    fn kept(&self) -> u64 {
        let (head_end, tail_start) = self.kept_bounds();
        head_end + (self.written - tail_start)
    }

    fn dropped(&self) -> u64 {
        self.size - self.kept()
    }

    /// Adds `bytes` the program wrote. A write that fails, the disk full most often, is said
    /// once, and nothing more goes into the file: what came after is dropped.
    fn append(&mut self, bytes: &[u8]) {
        self.size += bytes.len() as u64;
        let mut rest = bytes;
        while !rest.is_empty() && !self.failed {
            let room = usize::try_from(Self::contiguous_from(self.written)).unwrap_or(usize::MAX);
            let length = room.min(rest.len());
            match self
                .file
                .write_all_at(&rest[..length], Self::position(self.written))
            {
                Ok(()) => {
                    self.written += length as u64;
                    rest = &rest[length..];
                }
                Err(error) => {
                    tracing::warn!(%error, "a program's log cannot be written: the rest is dropped");
                    self.failed = true;
                }
            }
        }
    }

    /// Up to `max_bytes` of the bytes kept from byte `from`, or the newest without. A start in
    /// the bytes dropped moves to the first kept after; the bytes given never cross what was
    /// dropped, so that they are whole: the next read starts at their end.
    fn read(&self, from: Option<u64>, max_bytes: u64) -> Result<LogRead, String> {
        let (head_end, tail_start) = self.kept_bounds();
        let mut start = from
            .unwrap_or_else(|| self.written.saturating_sub(max_bytes))
            .min(self.written);
        if (head_end..tail_start).contains(&start) {
            start = tail_start;
        }
        let limit = if start < head_end && head_end < tail_start {
            head_end
        } else {
            self.written
        };
        let end = start.saturating_add(max_bytes).min(limit);
        let mut bytes = vec![0; usize::try_from(end - start).unwrap_or(0)];
        let mut offset = start;
        let mut filled = 0;
        while offset < end {
            let length = Self::contiguous_from(offset).min(end - offset) as usize;
            self.file
                .read_exact_at(&mut bytes[filled..filled + length], Self::position(offset))
                .map_err(|error| format!("its log cannot be read: {error}"))?;
            filled += length;
            offset += length as u64;
        }
        Ok(LogRead {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            from: start,
            to: end,
            size: self.size,
            dropped: self.dropped(),
            ended: self.ended,
        })
    }
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
}

/// The programs of one `yuna mcp`, and the folder of the seats' sockets, the programs' logs and
/// their runtime folders.
#[derive(Debug)]
pub struct Programs {
    folder: PathBuf,
    /// The folder's lock file, locked while this Yuna lives ([`LOCK_FILE`]).
    _lock: File,
    list: Vec<Program>,
    next_id: u64,
    /// The output sources that stopped reading once their program wrote more than its log
    /// keeps, until the next [`THROTTLE`] ends.
    throttled: Throttled,
}

/// The tokens of the output sources waiting for the end of a [`THROTTLE`]. A source holds this
/// list, never the loop's handle: a source holding the handle would keep the loop, and Yuna's
/// socket with it, alive for ever.
type Throttled = Rc<RefCell<Vec<Rc<Cell<Option<RegistrationToken>>>>>>;

impl Programs {
    /// Creates the folder of the seats' sockets, the programs' logs and their runtime folders,
    /// `yuna-mcp-<pid>` in `runtime_dir`, readable by its user alone, and locks its
    /// [`LOCK_FILE`] for as long as this Yuna lives. First removes the folders of Yunas that are
    /// gone ([`remove_stale_folders`]). The folder is made and locked under a name of its own,
    /// which no sweep takes, then given its name: another Yuna's sweep never finds it unlocked.
    pub fn new(runtime_dir: &Path) -> io::Result<Programs> {
        remove_stale_folders(runtime_dir);
        let pid = std::process::id();
        let staging = runtime_dir.join(format!(".{FOLDER_PREFIX}{pid}-starting"));
        match prepare_folder(&staging) {
            Ok(lock) => match name_folder(runtime_dir, pid, &staging) {
                Ok(folder) => Ok(Programs {
                    folder,
                    _lock: lock,
                    list: Vec::new(),
                    next_id: 1,
                    throttled: Throttled::default(),
                }),
                Err(error) => {
                    let _ = fs::remove_dir_all(&staging);
                    Err(error)
                }
            },
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                Err(error)
            }
        }
    }

    /// Where seat `seat`'s Wayland socket goes, in the folder.
    pub fn socket_path(&self, seat: SeatId) -> PathBuf {
        self.folder.join(format!("seat-{}", seat.0))
    }

    /// The folder of the seats' sockets, the programs' logs and their runtime folders.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The programs of `seat` remembered, in the order they started.
    pub fn list(&self, seat: SeatId) -> Vec<&Program> {
        self.list.iter().filter(|p| p.seat == seat).collect()
    }

    /// Program `id` of `seat`, while it is remembered; none for another seat's.
    pub fn program(&self, seat: SeatId, id: u64) -> Option<&Program> {
        self.list
            .iter()
            .find(|program| program.id == id && program.seat == seat)
    }

    /// Starts a program on `seat`, whose clients connect through `socket`, and its X11 clients
    /// to `x11` when the seat has an X display, its output read by a source of `handle`'s loop
    /// into its log. Refused, with the reason, for an empty command, a variable that cannot be
    /// one, more than [`MAX_RUNNING`] programs, all seats together, or a program that cannot
    /// start.
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
        // Taken first, so that a start that fails leaves no folder or log a later one would
        // find in its way.
        let id = self.next_id;
        self.next_id += 1;
        let runtime = self.folder.join("runtime").join(id.to_string());
        let log = self.folder.join("logs").join(format!("{id}.log"));
        let environment =
            program_environment(std::env::vars_os(), &runtime, socket, x11, seat, &start.env);
        let (pid, log_state) = match self.launch(handle, environment, &start, &runtime, &log) {
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
            end: None,
            log,
            protocol_error: None,
            started: Instant::now(),
            reaped: None,
            log_state,
            pace: Rc::new(Cell::new(Pace::Free)),
            pace_timer: None,
        });
        self.forget_oldest_ended(handle);
        // Only programs that ended are forgotten: this one is still last.
        self.list
            .last()
            .ok_or_else(|| "the program was forgotten".to_owned())
    }

    /// Makes a program's runtime folder and log, starts it with `environment`, and reads its
    /// output into the log.
    fn launch(
        &self,
        handle: &LoopHandle<'static, State>,
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

    /// Stops program `id` of `seat`, its whole process group, until it is thawed: SIGSTOP.
    /// Refused as [`Programs::stop`] is.
    pub fn freeze(
        &mut self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        id: u64,
    ) -> Result<(), String> {
        let program = self.program_mut(seat, id)?;
        unpace(handle, program);
        signal_group(program, Signal::STOP)?;
        program.pace.set(Pace::Frozen);
        Ok(())
    }

    /// Lets program `id` of `seat` run as it likes again: its pace ends, and its whole group
    /// gets SIGCONT. Refused as [`Programs::stop`] is.
    pub fn thaw(
        &mut self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        id: u64,
    ) -> Result<(), String> {
        let program = self.program_mut(seat, id)?;
        unpace(handle, program);
        signal_group(program, Signal::CONT)
    }

    /// Slows program `id` of `seat`: its whole group runs `run_percent` hundredths of each
    /// [`PACE_PERIOD`], continued then stopped by a timer on real time, whatever the
    /// compositor's clock, until it is thawed, frozen, stopped, or its group has ended. Refused
    /// for a share outside 1 to 99, and as [`Programs::stop`] is.
    pub fn slow(
        &mut self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        id: u64,
        run_percent: u8,
    ) -> Result<(), String> {
        if !(1..=99).contains(&run_percent) {
            return Err("run_percent is from 1 to 99: freeze or thaw for 0 or 100".to_owned());
        }
        let program = self.program_mut(seat, id)?;
        unpace(handle, program);
        signal_group(program, Signal::CONT)?;
        let running_for = PACE_PERIOD * u32::from(run_percent) / 100;
        let stopped_for = PACE_PERIOD - running_for;
        let group = program.pid;
        let pace = program.pace.clone();
        let mut running = true;
        let timer = handle
            .insert_source(Timer::from_duration(running_for), move |_, _, state| {
                state.woke = true;
                running = !running;
                let signal = if running { Signal::CONT } else { Signal::STOP };
                match rustix::process::kill_process_group(group, signal) {
                    // Its whole group has ended: nothing is left to pace.
                    Err(rustix::io::Errno::SRCH) => {
                        pace.set(Pace::Free);
                        TimeoutAction::Drop
                    }
                    _ if running => TimeoutAction::ToDuration(running_for),
                    _ => TimeoutAction::ToDuration(stopped_for),
                }
            })
            .map_err(|error| format!("program {id} cannot be paced: {}", error.error))?;
        program.pace_timer = Some(timer);
        program.pace.set(Pace::Slow(run_percent));
        Ok(())
    }

    /// Ends the pace of every program, without signalling them: Yuna is about to end them all,
    /// and no timer must stop one again between its SIGTERM and its SIGCONT.
    pub fn unpace_all(&mut self, handle: &LoopHandle<'static, State>) {
        for program in &mut self.list {
            unpace(handle, program);
        }
    }

    /// Ends the pace of `seat`'s programs, without signalling them, as [`Programs::unpace_all`].
    pub fn unpace_seat(&mut self, handle: &LoopHandle<'static, State>, seat: SeatId) {
        for program in self.list.iter_mut().filter(|program| program.seat == seat) {
            unpace(handle, program);
        }
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
    /// process ids `client_pids`. A process belongs to it when it, or a process above it under
    /// Yuna, is one of the seat's programs still running, is in the process group one of its
    /// programs led, running or ended (an orphan stays in it), is one of its clients, or has the
    /// seat's `YUNA_SEAT` in its environment; and when nothing says it is another seat's: another
    /// seat's `YUNA_SEAT`, or another seat's running program or its group, so that a pid an
    /// ended program freed and another seat's program took is never ended with this seat. A
    /// process that leaves its group and its parents and empties its environment is not found.
    pub fn seat_processes(&self, seat: SeatId, client_pids: &[i32]) -> Vec<Pid> {
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
        // Another seat's running programs: a process that is one, or in one's group, is that
        // seat's, whatever a reused pid says.
        let others: Vec<i32> = self
            .list
            .iter()
            .filter(|program| program.seat != seat && program.end.is_none())
            .map(|program| program.pid.as_raw_nonzero().get())
            .collect();
        let processes = processes_under(rustix::process::getpid());
        let mut memo: HashMap<i32, bool> = HashMap::new();
        let mut belongs = |pid: i32| -> bool {
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
                let named = seat_in_environment(current);
                let foreign = named.is_some_and(|named| named != seat)
                    || others.contains(&current)
                    || others.contains(&process.group);
                if foreign {
                    break false;
                }
                let direct = running.contains(&current)
                    || groups.contains(&process.group)
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
            .filter(|&pid| belongs(pid))
            .filter_map(Pid::from_raw)
            .collect();
        found.sort_by_key(|pid| pid.as_raw_nonzero().get());
        found
    }

    /// Forgets the programs of `seat`, which closed, and deletes their logs and runtime
    /// folders. Their pace has ended already ([`Programs::unpace_seat`]).
    pub fn forget_seat(&mut self, seat: SeatId) {
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

    /// Removes the folder of logs and runtime folders, once every program has ended.
    pub fn remove_folder(self) {
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

/// Sends `signal` to every living process under Yuna, found through `/proc`, as
/// [`send_ending_signal`] does, and returns how many there were. A child of Yuna's that ended
/// since it was read is not reaped yet, so its pid is not reused; a deeper process may be reaped
/// by its own parent in between, its pid then free for another
/// ([debt](../../../../docs/debt.md#the-mcp-server)).
pub fn signal_all(signal: Signal) -> usize {
    let living = living();
    if !living.is_empty() {
        tracing::info!(processes = living.len(), ?signal, "ending the programs");
    }
    for &pid in &living {
        send_ending_signal(pid, signal);
    }
    living.len()
}

/// Sends `signal` to process `pid`; after SIGTERM, SIGCONT too, as systemd does: a stopped
/// process, a program frozen or slowed, acts on SIGTERM only once continued, and would otherwise
/// wait for SIGKILL.
pub fn send_ending_signal(pid: Pid, signal: Signal) {
    let _ = rustix::process::kill_process(pid, signal);
    if signal == Signal::TERM {
        let _ = rustix::process::kill_process(pid, Signal::CONT);
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

/// Ends `program`'s pace, without signalling it: its timer goes while it runs.
fn unpace(handle: &LoopHandle<'static, State>, program: &mut Program) {
    // A timer that found the group ended has removed itself, and set the pace free: its token
    // is not removed again.
    if let (Some(timer), Pace::Slow(_)) = (program.pace_timer.take(), program.pace.get()) {
        handle.remove(timer);
    }
    program.pace.set(Pace::Free);
}

/// The prefix of the folders `yuna mcp` makes in `XDG_RUNTIME_DIR`, before its pid.
const FOLDER_PREFIX: &str = "yuna-mcp-";

/// The file in a `yuna mcp`'s folder whose lock it holds while it lives (`flock`, through
/// [`File::try_lock`]): the kernel releases it at any end, SIGKILL included, and in any pid
/// namespace, so that a lock another Yuna can take tells a folder whose Yuna is gone.
pub const LOCK_FILE: &str = "lock";

/// Makes the folder `staging`, its lock file locked, and its `logs` and `runtime` folders;
/// returns the lock. A folder of that name is a gone Yuna's that had this pid, and goes first.
fn prepare_folder(staging: &Path) -> io::Result<File> {
    match fs::DirBuilder::new().mode(0o700).create(staging) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            fs::remove_dir_all(staging)?;
            fs::DirBuilder::new().mode(0o700).create(staging)?;
        }
        made => made?,
    }
    let lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(staging.join(LOCK_FILE))?;
    lock.try_lock().map_err(io::Error::from)?;
    for inner in ["logs", "runtime"] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(staging.join(inner))?;
    }
    Ok(lock)
}

/// Gives `staging` its name in `runtime_dir`, `yuna-mcp-<pid>`, or `yuna-mcp-<pid>-<n>` when
/// a folder of that name is there, a gone Yuna's whose lock is not free yet; never in place of
/// another.
fn name_folder(runtime_dir: &Path, pid: u32, staging: &Path) -> io::Result<PathBuf> {
    for attempt in 0..100 {
        let folder = runtime_dir.join(match attempt {
            0 => format!("{FOLDER_PREFIX}{pid}"),
            n => format!("{FOLDER_PREFIX}{pid}-{n}"),
        });
        let renamed = rustix::fs::renameat_with(
            rustix::fs::CWD,
            staging,
            rustix::fs::CWD,
            &folder,
            rustix::fs::RenameFlags::NOREPLACE,
        );
        match renamed {
            Ok(()) => return Ok(folder),
            Err(rustix::io::Errno::EXIST) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::other(format!(
        "every name of {FOLDER_PREFIX}{pid}'s is taken"
    )))
}

/// Whether `name` is a `yuna mcp`'s folder's: `yuna-mcp-<pid>` or `yuna-mcp-<pid>-<n>`. A
/// folder still being made, `.yuna-mcp-<pid>-starting`, is not.
fn is_folder_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(FOLDER_PREFIX) else {
        return false;
    };
    let (pid, attempt) = rest.split_once('-').unwrap_or((rest, "0"));
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    digits(pid) && digits(attempt)
}

/// Removes each `yuna mcp`'s folder in `runtime_dir` whose [`LOCK_FILE`]'s lock can be taken:
/// the folder of sockets, logs and runtime folders a Yuna killed by SIGKILL left, which nothing
/// else removes. It is removed with its lock held, so that no other sweep takes it meanwhile;
/// never by its pid alone. A link is not followed, and a folder with no lock file, a Yuna's
/// older than this one, is kept: nothing tells whether it lives.
fn remove_stale_folders(runtime_dir: &Path) {
    let Ok(entries) = fs::read_dir(runtime_dir) else {
        return;
    };
    for entry in entries.flatten() {
        // The entry's own type: a link to a folder is not followed.
        let is_folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if !is_folder || !entry.file_name().to_str().is_some_and(is_folder_name) {
            continue;
        }
        let folder = entry.path();
        let Ok(lock) = File::open(folder.join(LOCK_FILE)) else {
            continue;
        };
        match lock.try_lock() {
            Ok(()) => match fs::remove_dir_all(&folder) {
                Ok(()) => tracing::info!(?folder, "a gone Yuna's folder is removed"),
                Err(error) => {
                    tracing::warn!(%error, ?folder, "a gone Yuna's folder cannot be removed");
                }
            },
            // Its Yuna lives.
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => {
                tracing::warn!(%error, ?folder, "a Yuna's folder's lock cannot be read");
            }
        }
    }
}

/// The processes living under Yuna now.
pub fn living() -> Vec<Pid> {
    living_descendants(rustix::process::getpid())
}

/// Whether a process still runs under Yuna.
pub fn any_alive() -> bool {
    !living().is_empty()
}

/// Reaps every child of Yuna's that has ended, without waiting, and gives `ended` each one's pid
/// and status.
fn reap_children(mut ended: impl FnMut(Pid, WaitStatus)) {
    loop {
        // `wait`, POSIX's `waitpid(-1)`: any child. rustix's `waitpid(None)` is `waitpid(0)`, the
        // children of Yuna's own process group alone, and each program leads its own.
        match rustix::process::wait(WaitOptions::NOHANG) {
            Ok(Some((pid, status))) => ended(pid, status),
            Ok(None) => return,
            Err(rustix::io::Errno::INTR) => continue,
            // ECHILD: no child left.
            Err(_) => return,
        }
    }
}

/// Removes `folder`, the programs' folder, warning when a process under Yuna outlives it. A
/// folder already gone, by a panic after Yuna's stopping removed it, is no failure.
fn remove_folder(folder: &Path) {
    if any_alive() {
        tracing::warn!("programs outlive Yuna");
    }
    match fs::remove_dir_all(folder) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(%error, ?folder, "the programs' folder cannot be removed"),
    }
}

/// Ends every process under Yuna, then removes `folder`, the programs' folder: what `yuna mcp`
/// does when it stops on purpose, for its fatal path, on the thread whose panic stops Yuna
/// ([ADR-0080](../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)).
/// It touches neither the compositor nor the server, which the panic may have left half
/// changed: with no event loop to serve the programs' clients or to wake it, it sleeps 10 ms
/// (`ENDING_POLL`) between looks. SIGTERM first, then SIGKILL [`END_GRACE`] later to those left, sent again at each look,
/// each reaped as it ends; it returns once none is left, [`KILL_WAIT`] after SIGKILL at most, at
/// once when none ran.
pub fn end_after_panic(folder: &Path) {
    'stages: for (signal, wait) in [(Signal::TERM, END_GRACE), (Signal::KILL, KILL_WAIT)] {
        if signal_all(signal) == 0 {
            break;
        }
        let until = Instant::now() + wait;
        loop {
            // A process may fork between the walk and its own SIGKILL: each look kills again.
            if signal == Signal::KILL {
                signal_all(Signal::KILL);
            }
            reap_children(|_, _| {});
            if !any_alive() {
                break 'stages;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(ENDING_POLL));
        }
    }
    remove_folder(folder);
}

/// A program's environment: the variables of `inherited` that [`KEPT_VARIABLES`] and
/// [`KEPT_PREFIXES`] name, then its own runtime folder, its seat's socket and its seat's X
/// display, `x11`, when it has one, then the agent's `added`, which win, then `YUNA_SEAT`, which
/// names its seat for its closing.
pub fn program_environment(
    inherited: impl Iterator<Item = (OsString, OsString)>,
    runtime_dir: &Path,
    wayland_display: &Path,
    x11: Option<&X11Display>,
    seat: SeatId,
    added: &BTreeMap<String, String>,
) -> BTreeMap<OsString, OsString> {
    let kept = |name: &OsStr| {
        name.to_str().is_some_and(|name| {
            KEPT_VARIABLES.contains(&name) || KEPT_PREFIXES.iter().any(|p| name.starts_with(p))
        })
    };
    let inherited: BTreeMap<OsString, OsString> = inherited.collect();
    let mut environment: BTreeMap<OsString, OsString> = inherited
        .iter()
        .filter(|(name, _)| kept(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    // The variables Yuna's package set for Yuna alone (flake.nix) go back to the caller's own.
    let wrapped = inherited
        .get(OsStr::new(WRAPPED_VARIABLES))
        .and_then(|names| names.to_str())
        .unwrap_or_default();
    for name in wrapped.split_whitespace() {
        match inherited.get(OsStr::new(&format!("{CALLER_PREFIX}{name}"))) {
            Some(value) => environment.insert(name.into(), value.clone()),
            None => environment.remove(OsStr::new(name)),
        };
    }
    environment.insert("XDG_RUNTIME_DIR".into(), runtime_dir.into());
    environment.insert("WAYLAND_DISPLAY".into(), wayland_display.into());
    environment.insert("XDG_SESSION_TYPE".into(), "wayland".into());
    if let Some(x11) = x11 {
        environment.insert("DISPLAY".into(), x11.name().into());
        environment.insert("XAUTHORITY".into(), x11.xauthority.clone().into());
        // Without a bus's address and with a DISPLAY, D-Bus's library launches a bus of its own
        // through X11 (`dbus-launch --autolaunch`), the machine's, which stays connected to the
        // seat's Xwayland and keeps it from ever stopping. `disabled:` says there is no bus.
        environment.insert("DBUS_SESSION_BUS_ADDRESS".into(), "disabled:".into());
    }
    for (name, value) in added {
        environment.insert(name.into(), value.into());
    }
    environment.insert(SEAT_VARIABLE.into(), seat.0.to_string().into());
    environment
}

/// The variable that names a program's seat, which its processes inherit.
pub const SEAT_VARIABLE: &str = "YUNA_SEAT";

/// The seat process `pid`'s environment names, as it was when the process started: its
/// `YUNA_SEAT`.
fn seat_in_environment(pid: i32) -> Option<SeatId> {
    let environment = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let prefix = format!("{SEAT_VARIABLE}=");
    environment
        .split(|&byte| byte == 0)
        .find_map(|entry| entry.strip_prefix(prefix.as_bytes()))
        .and_then(|value| std::str::from_utf8(value).ok()?.parse().ok())
        .map(SeatId)
}

/// Reads a program's output as it comes, into `log`; the source goes when the output ends.
/// Once the program has written more than its log keeps, [`LOG_HEAD`] and [`LOG_TAIL`], it is
/// read at most [`READ_SIZE`] every [`THROTTLE`]: the source stops reading and joins
/// `throttled`, which the server lets read again at the end of the throttle. A program that
/// writes faster waits on its pipe, rather than keep Yuna reading for what it drops.
fn watch_output(
    handle: &LoopHandle<'static, State>,
    output: OwnedFd,
    log: Rc<RefCell<ProgramLog>>,
    throttled: Throttled,
) -> io::Result<()> {
    rustix::io::ioctl_fionbio(&output, true)?;
    let mut buffer = vec![0; READ_SIZE];
    let source: Rc<Cell<Option<RegistrationToken>>> = Rc::default();
    let own_source = source.clone();
    let token = handle
        .insert_source(
            Generic::new(output, Interest::READ, Mode::Level),
            move |_, output, state| {
                state.woke = true;
                match rustix::io::read(output.as_fd(), &mut buffer) {
                    Ok(0) => {
                        log.borrow_mut().ended = true;
                        Ok(PostAction::Remove)
                    }
                    Ok(read) => {
                        let mut log = log.borrow_mut();
                        log.append(&buffer[..read]);
                        if log.size < LOG_HEAD + LOG_TAIL {
                            return Ok(PostAction::Continue);
                        }
                        throttled.borrow_mut().push(own_source.clone());
                        Ok(PostAction::Disable)
                    }
                    Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {
                        Ok(PostAction::Continue)
                    }
                    Err(error) => {
                        tracing::warn!(%error, "a program's output cannot be read");
                        log.borrow_mut().ended = true;
                        Ok(PostAction::Remove)
                    }
                }
            },
        )
        .map_err(|e| io::Error::other(e.error.to_string()))?;
    source.set(Some(token));
    Ok(())
}

/// Reads every byte waiting in a signal's pipe, which does not block.
pub fn drain(pipe: BorrowedFd<'_>) {
    let mut bytes = [0; 64];
    while let Ok(read) = rustix::io::read(pipe, &mut bytes) {
        if read < bytes.len() {
            return;
        }
    }
}

/// Every process under `root`, its parents followed through `/proc`, by pid.
fn processes_under(root: Pid) -> HashMap<i32, ProcessStat> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return HashMap::new();
    };
    let mut all: HashMap<i32, ProcessStat> = HashMap::new();
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        if let Some(process) = read_stat(&stat) {
            children.entry(process.parent).or_default().push(pid);
            all.insert(pid, process);
        }
    }
    let mut under = HashMap::new();
    let mut to_visit = vec![root.as_raw_nonzero().get()];
    while let Some(parent) = to_visit.pop() {
        for &pid in children.get(&parent).into_iter().flatten() {
            to_visit.push(pid);
            if let Some(&process) = all.get(&pid) {
                under.insert(pid, process);
            }
        }
    }
    under
}

/// The living processes under `root`, zombies left out: those are dead, and waiting for their
/// parent.
fn living_descendants(root: Pid) -> Vec<Pid> {
    processes_under(root)
        .into_iter()
        .filter(|(_, process)| !process.zombie)
        .filter_map(|(pid, _)| Pid::from_raw(pid))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Yuna's package sets its own libraries and Xwayland for Yuna alone, and names in
    /// `YUNA_WRAPPED_VARIABLES` what it changed, with the caller's own value of each it found:
    /// a program gets the caller's values back, and none where the caller had none.
    #[test]
    fn a_program_gets_back_what_the_package_changed_for_yuna() {
        let inherited = [
            ("PATH", "/package/xwayland/bin:/usr/bin"),
            ("LD_LIBRARY_PATH", "/package/libglvnd/lib"),
            (
                "__EGL_VENDOR_LIBRARY_FILENAMES",
                "/package/mesa/50_mesa.json",
            ),
            (
                "YUNA_WRAPPED_VARIABLES",
                "PATH LD_LIBRARY_PATH __EGL_VENDOR_LIBRARY_FILENAMES",
            ),
            ("YUNA_CALLER_PATH", "/usr/bin"),
        ]
        .into_iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let environment = program_environment(
            inherited,
            Path::new("/run/user/1000/yuna-mcp-7/runtime/1"),
            Path::new("/run/user/1000/yuna-mcp-7/seat-3"),
            None,
            SeatId(3),
            &BTreeMap::new(),
        );
        assert_eq!(environment[OsStr::new("PATH")], "/usr/bin");
        for gone in [
            "LD_LIBRARY_PATH",
            "__EGL_VENDOR_LIBRARY_FILENAMES",
            "YUNA_WRAPPED_VARIABLES",
            "YUNA_CALLER_PATH",
        ] {
            assert!(!environment.contains_key(OsStr::new(gone)), "{gone}");
        }
    }

    #[test]
    fn a_program_keeps_only_the_variables_it_needs_and_gets_its_own() {
        let inherited = [
            ("PATH", "/bin"),
            ("DISPLAY", ":0"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("LC_TIME", "fr_FR.UTF-8"),
            ("VK_ICD_FILENAMES", "/icd.json"),
            ("SSH_AUTH_SOCK", "/run/agent"),
            ("TERM", "xterm"),
        ]
        .into_iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let added = BTreeMap::from([("GDK_BACKEND".to_owned(), "wayland".to_owned())]);
        let environment = program_environment(
            inherited,
            Path::new("/run/user/1000/yuna-mcp-7/runtime/1"),
            Path::new("/run/user/1000/yuna-mcp-7/seat-3"),
            None,
            SeatId(3),
            &added,
        );
        let names: Vec<&str> = environment
            .keys()
            .map(|name| name.to_str().expect("UTF-8"))
            .collect();
        assert_eq!(
            names,
            [
                "GDK_BACKEND",
                "LC_TIME",
                "PATH",
                "VK_ICD_FILENAMES",
                "WAYLAND_DISPLAY",
                "XDG_RUNTIME_DIR",
                "XDG_SESSION_TYPE",
                "YUNA_SEAT"
            ]
        );
        assert_eq!(
            environment[OsStr::new("WAYLAND_DISPLAY")],
            "/run/user/1000/yuna-mcp-7/seat-3"
        );
        assert_eq!(environment[OsStr::new("YUNA_SEAT")], "3");
        assert_eq!(
            environment[OsStr::new("XDG_RUNTIME_DIR")],
            "/run/user/1000/yuna-mcp-7/runtime/1"
        );
    }

    #[test]
    fn the_agents_variables_win_but_the_seats() {
        let inherited = [(OsString::from("PATH"), OsString::from("/bin"))].into_iter();
        let added = BTreeMap::from([
            ("PATH".to_owned(), "/opt/bin".to_owned()),
            ("WAYLAND_DISPLAY".to_owned(), "elsewhere".to_owned()),
            ("YUNA_SEAT".to_owned(), "9".to_owned()),
        ]);
        let environment = program_environment(
            inherited,
            Path::new("/r"),
            Path::new("/w"),
            None,
            SeatId(2),
            &added,
        );
        assert_eq!(environment[OsStr::new("PATH")], "/opt/bin");
        assert_eq!(environment[OsStr::new("WAYLAND_DISPLAY")], "elsewhere");
        // Its seat's, whatever the agent says: it decides which seat's closing ends it.
        assert_eq!(environment[OsStr::new("YUNA_SEAT")], "2");
    }

    #[test]
    fn a_program_gets_its_seats_x_display_and_cookie_not_yunas() {
        let inherited = [
            (OsString::from("DISPLAY"), OsString::from(":0")),
            (
                OsString::from("XAUTHORITY"),
                OsString::from("/home/user/.Xauthority"),
            ),
        ]
        .into_iter();
        let x11 = X11Display {
            number: 12,
            xauthority: PathBuf::from("/run/user/1000/yuna-mcp-7/seat-2.x11/Xauthority"),
            server: None,
        };
        let environment = program_environment(
            inherited,
            Path::new("/r"),
            Path::new("/w"),
            Some(&x11),
            SeatId(2),
            &BTreeMap::new(),
        );
        assert_eq!(environment[OsStr::new("DISPLAY")], ":12");
        assert_eq!(
            environment[OsStr::new("XAUTHORITY")],
            "/run/user/1000/yuna-mcp-7/seat-2.x11/Xauthority"
        );
        // With a DISPLAY and no bus, D-Bus's library would launch a bus of its own through X11,
        // which would hold the seat's Xwayland for ever.
        assert_eq!(
            environment[OsStr::new("DBUS_SESSION_BUS_ADDRESS")],
            "disabled:"
        );
        // The agent's win, as for WAYLAND_DISPLAY.
        let added = BTreeMap::from([("DISPLAY".to_owned(), ":5".to_owned())]);
        let environment = program_environment(
            std::iter::empty(),
            Path::new("/r"),
            Path::new("/w"),
            Some(&x11),
            SeatId(2),
            &added,
        );
        assert_eq!(environment[OsStr::new("DISPLAY")], ":5");
    }

    #[test]
    fn a_stat_line_is_read_past_a_name_with_spaces_and_parentheses() {
        let stat = |parent, group, zombie| {
            Some(ProcessStat {
                parent,
                group,
                zombie,
            })
        };
        assert_eq!(read_stat("42 (a b) c)) S 7 40 42 0"), stat(7, 40, false));
        assert_eq!(read_stat("43 (z) Z 1 43"), stat(1, 43, true));
        assert_eq!(read_stat("44 no parenthesis"), None);
        assert_eq!(read_stat("45 (x) S 1"), None);
    }

    /// A log in a file of its own, in the test's temporary folder.
    fn log_in(name: &str) -> (ProgramLog, PathBuf) {
        let path =
            std::env::temp_dir().join(format!("yuna-program-log-{name}-{}", std::process::id()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .expect("a log file");
        (ProgramLog::new(file), path)
    }

    /// The bytes `start..end` of an output where byte `n` is `n % 251`: a prime, so that bytes
    /// read at a wrong place in the ring differ.
    fn output(start: u64, end: u64) -> Vec<u8> {
        (start..end).map(|n| (n % 251) as u8).collect()
    }

    #[test]
    fn a_log_keeps_its_head_and_its_newest_bytes_by_offset() {
        let (mut log, path) = log_in("ring");
        let size = LOG_HEAD + 3 * LOG_TAIL + 12_345;
        // Written in uneven pieces, so that pieces cross the head's end and the ring's.
        let mut written = 0;
        for piece in [100, LOG_HEAD - 50, 70_001, LOG_TAIL, 2 * LOG_TAIL] {
            let end = (written + piece).min(size);
            log.append(&output(written, end));
            written = end;
        }
        log.append(&output(written, size));
        let _ = fs::remove_file(&path);
        assert_eq!(log.size, size);
        assert_eq!(log.kept(), LOG_HEAD + LOG_TAIL);
        assert_eq!(log.dropped(), size - LOG_HEAD - LOG_TAIL);
        // The text is the bytes as UTF-8, the invalid ones replaced.
        let text = |start, end| String::from_utf8_lossy(&output(start, end)).into_owned();
        let head = log.read(Some(0), 1_000).expect("a read");
        assert_eq!((head.from, head.to), (0, 1_000));
        assert_eq!(head.text, text(0, 1_000));
        let edge = log.read(Some(LOG_HEAD - 10), 1_000).expect("a read");
        assert_eq!((edge.from, edge.to), (LOG_HEAD - 10, LOG_HEAD));
        let tail_start = size - LOG_TAIL;
        let gap = log.read(Some(LOG_HEAD + 5), 4_000).expect("a read");
        assert_eq!((gap.from, gap.to), (tail_start, tail_start + 4_000));
        assert_eq!(gap.text, text(tail_start, tail_start + 4_000));
        let newest = log.read(None, LOG_TAIL).expect("a read");
        assert_eq!((newest.from, newest.to), (tail_start, size));
        assert_eq!(newest.text, text(tail_start, size));
        let past = log.read(Some(size * 2), 10).expect("a read");
        assert_eq!((past.from, past.to, past.text.as_str()), (size, size, ""));
    }

    #[test]
    fn a_short_log_keeps_everything_and_reads_across_head_and_tail() {
        let (mut log, path) = log_in("short");
        let size = LOG_HEAD + 1_000;
        log.append(&vec![b'a'; size as usize]);
        let whole = log.read(Some(0), LOG_HEAD + LOG_TAIL).expect("a read");
        let _ = fs::remove_file(&path);
        assert_eq!((whole.from, whole.to, whole.dropped), (0, size, 0));
        assert_eq!(whole.text.len(), size as usize);
        assert!(!whole.ended);
        let newest = log.read(None, 2_000).expect("a read");
        assert_eq!((newest.from, newest.to), (size - 2_000, size));
    }

    #[test]
    fn only_yuna_mcps_folders_are_swept() {
        assert!(is_folder_name("yuna-mcp-123"));
        assert!(is_folder_name("yuna-mcp-123-4"));
        for other in [
            "yuna-mcp-",
            "yuna-mcp-12-",
            "yuna-mcp-+12",
            "yuna-mcp-test-tree-12",
            "yuna-mcp-12-a",
            ".yuna-mcp-12-starting",
            "wayland-1",
        ] {
            assert!(!is_folder_name(other), "{other}");
        }
    }

    #[test]
    fn this_process_is_not_its_own_descendant() {
        let me = rustix::process::getpid();
        assert!(!living_descendants(me).contains(&me));
    }
}
