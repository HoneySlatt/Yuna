//! The programs an agent starts through `spawn`, in `yuna mcp`
//! ([ADR-0033](../../../../docs/adr/0033-the-mcp-server.md),
//! [ADR-0035](../../../../docs/adr/0035-agents-seats.md)).
//!
//! Each program gets an environment of its own: the few variables of Yuna's own that a program
//! needs to find itself, its libraries, its locale and its drivers ([`KEPT_VARIABLES`],
//! [`KEPT_PREFIXES`]); `WAYLAND_DISPLAY`, the full path of its seat's socket; and an
//! `XDG_RUNTIME_DIR` of its own, empty, so that it finds neither the user's D-Bus nor their
//! sound server there. Nothing else: no `DISPLAY`, no `DBUS_SESSION_BUS_ADDRESS`. The agent's
//! own variables come next and win, but for `YUNA_SEAT`, its seat's id, which comes last. Its standard input is `/dev/null`; its output and errors go,
//! through a pipe Yuna reads, to a log file the agent reads, bounded to [`LOG_LIMIT`].
//!
//! Each program leads a process group of its own, which `stop` signals. Yuna is its programs'
//! subreaper (the program sets it, `main.rs`): a process whose parent dies becomes Yuna's
//! child, so that the whole tree stays Yuna's to reap and to end. When Yuna stops, every process
//! under it gets SIGTERM, then SIGKILL [`END_GRACE`] later if it is still there, while Yuna still
//! serves its Wayland clients (`mod.rs`). When a seat closes, the processes that belong to it
//! are ended the same way ([`Programs::seat_processes`]). Exits are read when SIGCHLD arrives,
//! never polled. The folder that holds the seats' sockets, the logs and the programs' runtime
//! folders is removed when Yuna stops.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use smithay::reexports::rustix;
use smithay::reexports::rustix::pipe::{PipeFlags, pipe_with};
use smithay::reexports::rustix::process::{Pid, Signal, WaitOptions};

use yuna_layout::SeatId;

use crate::state::State;

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

/// The prefixes of the variables a program keeps: the locale's categories, and the settings of
/// Vulkan's loader and of Mesa.
pub const KEPT_PREFIXES: &[&str] = &["LC_", "VK_", "MESA_"];

/// How many programs may run at once.
pub const MAX_RUNNING: usize = 64;

/// How many programs are remembered, those that ended included; past it, the oldest that ended
/// is forgotten, and its log deleted.
pub const MAX_KEPT: usize = 128;

/// How much of a program's output its log keeps: 1 MiB. What comes after is counted and
/// dropped, so that a program that never stops writing fills neither the disk nor memory: the
/// logs are in `XDG_RUNTIME_DIR`, most often in memory, [`MAX_KEPT`] of them at most.
pub const LOG_LIMIT: u64 = 1 << 20;

/// How long the programs have, once Yuna stops, between SIGTERM and SIGKILL.
pub const END_GRACE: Duration = Duration::from_secs(2);

/// How long Yuna waits, after SIGKILL, for the killed to be reaped.
pub const KILL_WAIT: Duration = Duration::from_secs(1);

/// How much of a program's output is read at a time.
const READ_SIZE: usize = 64 * 1024;

/// How often a program's output is read once its log is full.
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
    /// Its log file: its standard output and errors, in the order they came.
    pub log: PathBuf,
    log_count: Rc<Cell<LogCount>>,
}

impl Program {
    /// Bytes of output its log holds.
    pub fn log_written(&self) -> u64 {
        self.log_count.get().written
    }

    /// Whether a process of its group still runs: the program, or one it started and did not
    /// move to a group of its own.
    pub fn group_runs(&self) -> bool {
        rustix::process::test_kill_process_group(self.pid).is_ok()
    }

    /// Bytes of output dropped past [`LOG_LIMIT`].
    pub fn log_dropped(&self) -> u64 {
        self.log_count.get().dropped
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

#[derive(Debug, Clone, Copy, Default)]
struct LogCount {
    written: u64,
    dropped: u64,
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
    list: Vec<Program>,
    next_id: u64,
    /// The output sources that stopped reading past [`LOG_LIMIT`], until the next
    /// [`THROTTLE`] ends.
    throttled: Throttled,
}

/// The tokens of the output sources waiting for the end of a [`THROTTLE`]. A source holds this
/// list, never the loop's handle: a source holding the handle would keep the loop, and Yuna's
/// socket with it, alive for ever.
type Throttled = Rc<RefCell<Vec<Rc<Cell<Option<RegistrationToken>>>>>>;

impl Programs {
    /// Creates the folder of the seats' sockets, the programs' logs and their runtime folders,
    /// `yuna-mcp-<pid>` in `runtime_dir`, readable by its user alone.
    pub fn new(runtime_dir: &Path) -> io::Result<Programs> {
        let pid = std::process::id();
        let mut attempt = 0_u32;
        let folder = loop {
            // A folder of the same name is a stopped Yuna's, with this pid then: kept, and
            // another name taken.
            let name = match attempt {
                0 => format!("yuna-mcp-{pid}"),
                n => format!("yuna-mcp-{pid}-{n}"),
            };
            let folder = runtime_dir.join(name);
            match fs::DirBuilder::new().mode(0o700).create(&folder) {
                Ok(()) => break folder,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        };
        fs::DirBuilder::new()
            .mode(0o700)
            .create(folder.join("logs"))?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(folder.join("runtime"))?;
        Ok(Programs {
            folder,
            list: Vec::new(),
            next_id: 1,
            throttled: Throttled::default(),
        })
    }

    /// Where seat `seat`'s Wayland socket goes, in the folder.
    pub fn socket_path(&self, seat: SeatId) -> PathBuf {
        self.folder.join(format!("seat-{}", seat.0))
    }

    /// The programs of `seat` remembered, in the order they started.
    pub fn list(&self, seat: SeatId) -> Vec<&Program> {
        self.list.iter().filter(|p| p.seat == seat).collect()
    }

    /// Starts a program on `seat`, whose clients connect through `socket`, its output read by a
    /// source of `handle`'s loop into its log. Refused, with the reason, for an empty command, a
    /// variable that cannot be one, more than [`MAX_RUNNING`] programs, all seats together, or a
    /// program that cannot start.
    pub fn start(
        &mut self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        socket: &Path,
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
        let (pid, log_count) = match self.launch(handle, seat, socket, &start, &runtime, &log) {
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
            log_count,
        });
        self.forget_oldest_ended();
        // Only programs that ended are forgotten: this one is still last.
        self.list
            .last()
            .ok_or_else(|| "the program was forgotten".to_owned())
    }

    /// Makes a program's runtime folder and log, starts it, and reads its output into the log.
    fn launch(
        &self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        socket: &Path,
        start: &Start,
        runtime: &Path,
        log_path: &Path,
    ) -> Result<(Pid, Rc<Cell<LogCount>>), String> {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(runtime)
            .map_err(|e| format!("its runtime folder cannot be created: {e}"))?;
        let log = OpenOptions::new()
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
            .envs(program_environment(
                std::env::vars_os(),
                runtime,
                socket,
                seat,
                &start.env,
            ))
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
        let log_count = Rc::new(Cell::new(LogCount::default()));
        if let Err(error) = watch_output(
            handle,
            output_read,
            log,
            log_count.clone(),
            self.throttled.clone(),
        ) {
            tracing::warn!(%error, %program, "a program's output cannot be read");
        }
        Ok((pid, log_count))
    }

    /// Sends program `id` of `seat` its process group SIGTERM, or SIGKILL when `force`: the
    /// processes it started go too, even once it has ended itself. Refused for an unknown
    /// program, another seat's, and one whose whole group has ended.
    pub fn stop(&mut self, seat: SeatId, id: u64, force: bool) -> Result<(), String> {
        let program = self
            .list
            .iter()
            .find(|program| program.id == id && program.seat == seat)
            .ok_or_else(|| format!("no program {id} on seat {}", seat.0))?;
        let signal = if force { Signal::KILL } else { Signal::TERM };
        match rustix::process::kill_process_group(program.pid, signal) {
            Ok(()) => Ok(()),
            Err(rustix::io::Errno::SRCH) => Err(format!(
                "program {id} has ended, and every process of its group with it"
            )),
            Err(error) => Err(format!("program {id} cannot be signalled: {error}")),
        }
    }

    /// Reaps every child that ended, programs and orphans Yuna adopted, and records how each
    /// program ended. Run when SIGCHLD has come.
    pub fn reap(&mut self) {
        loop {
            // `wait`, POSIX's `waitpid(-1)`: any child. rustix's `waitpid(None)` is `waitpid(0)`,
            // the children of Yuna's own process group alone, and each program leads its own.
            match rustix::process::wait(WaitOptions::NOHANG) {
                Ok(Some((pid, status))) => {
                    let end = match (status.exit_status(), status.terminating_signal()) {
                        (Some(code), _) => End::Exited(code),
                        (None, Some(signal)) => End::Killed(signal),
                        // Stopped or continued: not an end.
                        (None, None) => continue,
                    };
                    if let Some(program) = self
                        .list
                        .iter_mut()
                        .find(|p| p.pid == pid && p.end.is_none())
                    {
                        tracing::info!(program = program.id, ?end, "program ended");
                        program.end = Some(end);
                    }
                }
                Ok(None) => return,
                Err(rustix::io::Errno::INTR) => continue,
                // ECHILD: no child left.
                Err(_) => return,
            }
        }
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

    /// Sends `signal` to every living process under Yuna, found through `/proc`, and returns how
    /// many there were. A process that ended since its parent was read is not reaped yet: its
    /// pid is not reused.
    pub fn signal_all(&self, signal: Signal) -> usize {
        let living = living_descendants(rustix::process::getpid());
        if !living.is_empty() {
            tracing::info!(processes = living.len(), ?signal, "ending the programs");
        }
        for &pid in &living {
            let _ = rustix::process::kill_process(pid, signal);
        }
        living.len()
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
    /// folders.
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

    /// The processes living under Yuna now.
    pub fn living(&self) -> Vec<Pid> {
        living_descendants(rustix::process::getpid())
    }

    /// Whether a process still runs under Yuna.
    pub fn any_alive(&self) -> bool {
        !living_descendants(rustix::process::getpid()).is_empty()
    }

    /// Removes the folder of logs and runtime folders, once every program has ended.
    pub fn remove_folder(self) {
        if self.any_alive() {
            tracing::warn!("programs outlive Yuna");
        }
        if let Err(error) = fs::remove_dir_all(&self.folder) {
            tracing::warn!(%error, folder = ?self.folder, "the programs' folder cannot be removed");
        }
    }

    /// Forgets the oldest programs that ended while more than [`MAX_KEPT`] are remembered, and
    /// deletes their logs and runtime folders.
    fn forget_oldest_ended(&mut self) {
        while self.list.len() > MAX_KEPT {
            let Some(index) = self.list.iter().position(|p| p.end.is_some()) else {
                return;
            };
            let program = self.list.remove(index);
            let _ = fs::remove_file(&program.log);
            let _ = fs::remove_dir_all(self.folder.join("runtime").join(program.id.to_string()));
        }
    }
}

/// A program's environment: the variables of `inherited` that [`KEPT_VARIABLES`] and
/// [`KEPT_PREFIXES`] name, then its own runtime folder and its seat's socket, then the agent's
/// `added`, which win, then `YUNA_SEAT`, which names its seat for its closing.
pub fn program_environment(
    inherited: impl Iterator<Item = (OsString, OsString)>,
    runtime_dir: &Path,
    wayland_display: &Path,
    seat: SeatId,
    added: &BTreeMap<String, String>,
) -> BTreeMap<OsString, OsString> {
    let kept = |name: &OsStr| {
        name.to_str().is_some_and(|name| {
            KEPT_VARIABLES.contains(&name) || KEPT_PREFIXES.iter().any(|p| name.starts_with(p))
        })
    };
    let mut environment: BTreeMap<OsString, OsString> =
        inherited.filter(|(name, _)| kept(name)).collect();
    environment.insert("XDG_RUNTIME_DIR".into(), runtime_dir.into());
    environment.insert("WAYLAND_DISPLAY".into(), wayland_display.into());
    environment.insert("XDG_SESSION_TYPE".into(), "wayland".into());
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

/// Reads a program's output as it comes, into `log`, up to [`LOG_LIMIT`]; the source goes when
/// the output ends. Past the limit, what comes is counted and dropped, read at most
/// [`READ_SIZE`] every [`THROTTLE`]: the source stops reading and joins `throttled`, which the
/// server lets read again at the end of the throttle. A program that writes faster waits on its
/// pipe, rather than keep Yuna reading for nothing.
fn watch_output(
    handle: &LoopHandle<'static, State>,
    output: OwnedFd,
    mut log: File,
    count: Rc<Cell<LogCount>>,
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
                    Ok(0) => Ok(PostAction::Remove),
                    Ok(read) => {
                        let mut now = count.get();
                        let room = usize::try_from(LOG_LIMIT.saturating_sub(now.written))
                            .unwrap_or(usize::MAX)
                            .min(read);
                        if room > 0 && log.write_all(&buffer[..room]).is_ok() {
                            now.written += room as u64;
                        } else {
                            now.dropped += room as u64;
                        }
                        now.dropped += (read - room) as u64;
                        count.set(now);
                        if now.written < LOG_LIMIT {
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
                        Ok(PostAction::Remove)
                    }
                }
            },
        )
        .map_err(|e| io::Error::other(e.error.to_string()))?;
    source.set(Some(token));
    Ok(())
}

/// Marks every descriptor of the program above 2 close-on-exec, in the child just before it runs
/// the program, so that it inherits none of Yuna's: llvmpipe opens some without
/// `O_CLOEXEC`, `/dev/udmabuf` and the memory of its buffers among them. Marked, not closed:
/// the pipe through which the standard library reports a failed `exec` must stay open.
fn keep_descriptors_from_the_program(command: &mut Command) {
    // SAFETY: the closure runs in the child between fork and exec, where only async-signal-safe
    // calls are sound: it makes one system call, `close_range`, and allocates nothing, an errno
    // turning into an `io::Error` without a box.
    unsafe {
        command.pre_exec(|| {
            if close_range(3, u32::MAX, CLOSE_RANGE_CLOEXEC) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
}

/// `close_range`'s flag that marks the descriptors close-on-exec rather than closing them.
const CLOSE_RANGE_CLOEXEC: u32 = 1 << 2;

// SAFETY: this declaration matches the C library's own, glibc's since 2.34, which the standard
// library links on Linux; its one call is `unsafe`, with its reason.
unsafe extern "C" {
    /// Linux's `close_range(2)`: `int close_range(unsigned int first, unsigned int last, int
    /// flags)`, its flags passed as the `unsigned int` they are built from.
    fn close_range(first: u32, last: u32, flags: u32) -> i32;
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

/// A process as `/proc/<pid>/stat` gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcessStat {
    parent: i32,
    group: i32,
    zombie: bool,
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

/// A `/proc/<pid>/stat` line: `pid (name) state ppid pgrp …`, the name possibly holding spaces
/// and parentheses, so read from its last `)`.
fn read_stat(stat: &str) -> Option<ProcessStat> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    let mut fields = after_name.split_whitespace();
    let state = fields.next()?;
    let parent = fields.next()?.parse().ok()?;
    let group = fields.next()?.parse().ok()?;
    Some(ProcessStat {
        parent,
        group,
        zombie: state == "Z",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            SeatId(2),
            &added,
        );
        assert_eq!(environment[OsStr::new("PATH")], "/opt/bin");
        assert_eq!(environment[OsStr::new("WAYLAND_DISPLAY")], "elsewhere");
        // Its seat's, whatever the agent says: it decides which seat's closing ends it.
        assert_eq!(environment[OsStr::new("YUNA_SEAT")], "2");
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

    #[test]
    fn this_process_is_not_its_own_descendant() {
        let me = rustix::process::getpid();
        assert!(!living_descendants(me).contains(&me));
    }
}
