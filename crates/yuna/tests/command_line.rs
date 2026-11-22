//! The program run as a user runs it: its output and its exit status.

#![allow(
    clippy::disallowed_methods,
    reason = "a test's threads are not Yuna's tasks (ADR-0152)"
)]

#[path = "common/folders.rs"]
mod folders;

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::os::fd::AsFd;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use folders::fresh_folder;
use rustix::process::{Pid, Resource, Rlimit, Signal, getrlimit, kill_process, setrlimit};
use wayland_client::protocol::{wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};

fn run_yuna(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_yuna"))
        .args(arguments)
        .output()
        .expect("the yuna binary runs")
}

#[test]
fn version_prints_the_crate_version() {
    let output = run_yuna(&["--version"]);
    assert!(output.status.success());
    let expected = format!("yuna {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn help_succeeds_and_names_the_options() {
    let output = run_yuna(&["--help"]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for option in [
        "--help",
        "--version",
        "--headless",
        "--gpu",
        "--output",
        "--rest-audit",
        "XDG_RUNTIME_DIR",
        "PATH",
        "YUNA_LOG",
        "YUNA_DBUS_DAEMON",
    ] {
        assert!(help.contains(option), "the help does not name {option}");
    }
    let (_, statuses) = help
        .split_once("\nExit status:\n")
        .expect("the help lists the exit statuses");
    for status in ["0", "1", "2", "101"] {
        assert!(
            statuses
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("{status} "))),
            "the help does not list exit status {status}"
        );
    }
}

#[test]
fn a_session_fails_while_the_desktop_does_not_exist() {
    let output = run_yuna(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("desktop session does not exist yet"));
}

#[test]
fn a_bad_output_is_a_usage_error() {
    let output = run_yuna(&["--headless", "--output", "1920x1080@5000,1"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("refresh rate"));
}

#[test]
fn headless_without_a_runtime_folder_is_refused_clearly() {
    for arguments in [&["--headless"][..], &["--headless", "--agent-seat"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_yuna"))
            .args(arguments)
            .env_remove("XDG_RUNTIME_DIR")
            .output()
            .expect("the yuna binary runs");
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("XDG_RUNTIME_DIR is not set"),
            "{arguments:?}"
        );
    }
}

/// A `yuna --headless` process, killed if the test fails before it stops.
struct Running {
    child: Child,
    runtime_dir: PathBuf,
    /// The lines it printed after `WAYLAND_DISPLAY`, read as they come.
    printed: Option<BufReader<ChildStdout>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.runtime_dir);
    }
}

/// The globals a client sees, and the first output's mode.
#[derive(Default)]
struct Globals {
    interfaces: BTreeSet<String>,
    output_mode: Option<(i32, i32, i32)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Globals {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            if interface == "wl_output" {
                registry.bind::<wl_output::WlOutput, _, _>(name, 1, qh, ());
            }
            state.interfaces.insert(interface);
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Globals {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Mode {
            flags: WEnum::Value(flags),
            width,
            height,
            refresh,
        } = event
            && flags.contains(wl_output::Mode::Current)
        {
            state.output_mode = Some((width, height, refresh));
        }
    }
}

/// `yuna --headless` with `arguments` started in a runtime folder of its own named after `name`,
/// its log read on a thread, and the socket it printed: the process, its log's reader and the
/// socket's path.
fn start_headless(
    name: &str,
    arguments: &[&str],
) -> (
    Running,
    std::thread::JoinHandle<std::io::Result<String>>,
    PathBuf,
) {
    start_headless_with(name, arguments, |_| {})
}

/// As [`start_headless`], its command changed by `change` first.
fn start_headless_with(
    name: &str,
    arguments: &[&str],
    change: impl FnOnce(&mut Command),
) -> (
    Running,
    std::thread::JoinHandle<std::io::Result<String>>,
    PathBuf,
) {
    let runtime_dir = fresh_folder(&format!("test-{name}"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_yuna"));
    command
        .arg("--headless")
        .args(arguments)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        // Info, for the line that says whether linux-dmabuf is offered: a few lines in all.
        .env("YUNA_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    change(&mut command);
    let child = command.spawn().expect("the yuna binary runs");
    let mut running = Running {
        child,
        runtime_dir,
        printed: None,
    };

    // Read on a thread from the start: a pipe may hold as little as one page when the user's
    // pipes are many, and Yuna, blocked writing its log, would never print its socket.
    let mut stderr = running.child.stderr.take().expect("its standard error");
    let log_reader = std::thread::spawn(move || {
        let mut log = String::new();
        stderr.read_to_string(&mut log).map(|_| log)
    });
    let stdout = running.child.stdout.take().expect("its standard output");
    let mut printed = BufReader::new(stdout);
    let mut line = String::new();
    printed
        .read_line(&mut line)
        .expect("a line on standard output");
    running.printed = Some(printed);
    let socket = line
        .trim_end()
        .strip_prefix("WAYLAND_DISPLAY=")
        .unwrap_or_else(|| panic!("no socket printed: {line:?}"))
        .to_owned();
    let socket_path = running.runtime_dir.join(&socket);
    (running, log_reader, socket_path)
}

/// The number of the X display `running` printed, `DISPLAY=:<number>`, the line after its
/// socket's: printed when Xwayland is found, which the development shell gives.
fn x11_display_number(running: &mut Running) -> u32 {
    let mut line = String::new();
    running
        .printed
        .as_mut()
        .expect("its standard output")
        .read_line(&mut line)
        .expect("a second line on standard output");
    line.trim_end()
        .strip_prefix("DISPLAY=:")
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("no X display printed: {line:?}"))
}

/// The globals a client of the socket at `path` sees, with the first output's mode.
fn globals_at(path: &std::path::Path) -> Globals {
    let stream = UnixStream::connect(path).expect("the socket accepts a client");
    let connection = Connection::from_socket(stream).expect("a Wayland connection");
    let mut queue = connection.new_event_queue();
    connection.display().get_registry(&queue.handle(), ());
    let mut globals = Globals::default();
    queue.roundtrip(&mut globals).expect("the globals come");
    queue
        .roundtrip(&mut globals)
        .expect("the output's mode comes");
    globals
}

/// Sends SIGTERM and waits for a clean exit, the socket at `socket_path` removed.
fn stop_cleanly(running: &mut Running, socket_path: &std::path::Path) {
    kill_process(Pid::from_child(&running.child), Signal::TERM).expect("SIGTERM is sent");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = running.child.try_wait().expect("the process is waited for") {
            break status;
        }
        assert!(Instant::now() < deadline, "yuna did not stop on SIGTERM");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
    assert!(!socket_path.exists(), "the socket was left behind");
}

/// `yuna --headless` as an agent runs it: it prints its socket, a client sees the globals of
/// M1.4 and the output asked, and SIGTERM stops it cleanly, its socket removed. `linux-dmabuf` is
/// offered only when the renderer, llvmpipe here, imports a format: the program's log says which,
/// and the globals must agree with it. Its socket serves the owner's seat, which is offered no
/// virtual keyboard or pointer (ADR-0058).
#[test]
fn headless_serves_its_globals_and_stops_on_sigterm() {
    let (mut running, log_reader, socket_path) =
        start_headless("owner", &["--output", "1920x1080@60,1"]);
    let globals = globals_at(&socket_path);
    for interface in [
        "wl_compositor",
        "wl_subcompositor",
        "wl_shm",
        "wl_seat",
        "wl_output",
        "zxdg_output_manager_v1",
        "xdg_wm_base",
        "zxdg_decoration_manager_v1",
        "wp_presentation",
        "wl_data_device_manager",
        "zwp_primary_selection_device_manager_v1",
        "ext_data_control_manager_v1",
        "xdg_activation_v1",
        "xdg_wm_dialog_v1",
        "wp_viewporter",
        "wp_fractional_scale_manager_v1",
        "wp_cursor_shape_manager_v1",
        "zwp_text_input_manager_v3",
    ] {
        assert!(
            globals.interfaces.contains(interface),
            "{interface} is missing from {:?}",
            globals.interfaces
        );
    }
    assert_eq!(globals.output_mode, Some((1920, 1080, 60_000)));
    for interface in VIRTUAL_INPUT {
        assert!(
            !globals.interfaces.contains(interface),
            "{interface} is offered"
        );
    }
    stop_cleanly(&mut running, &socket_path);

    let log = log_reader
        .join()
        .expect("the log's reader does not panic")
        .expect("its log reads");
    let dmabuf_offered = log.contains("linux-dmabuf offered");
    assert!(
        dmabuf_offered || log.contains("the renderer imports no dmabuf"),
        "the log does not say whether linux-dmabuf is offered: {log}"
    );
    eprintln!("linux-dmabuf offered on the software device: {dmabuf_offered}");
    assert_eq!(
        globals.interfaces.contains("zwp_linux_dmabuf_v1"),
        dmabuf_offered,
        "the globals {:?} disagree with the log: {log}",
        globals.interfaces
    );
}

/// The virtual keyboard's and pointer's globals.
const VIRTUAL_INPUT: [&str; 2] = [
    "zwp_virtual_keyboard_manager_v1",
    "zwlr_virtual_pointer_manager_v1",
];

/// `yuna --headless --agent-seat` serves an agent's seat on its socket, its outputs on it: the
/// virtual keyboard and pointer are offered there, and SIGTERM stops it cleanly, its socket
/// removed (ADR-0089).
#[test]
fn headless_with_an_agent_seat_offers_virtual_input() {
    let (mut running, _log, socket_path) =
        start_headless("agent", &["--agent-seat", "--output", "1920x1080@60,1"]);
    let globals = globals_at(&socket_path);
    for interface in VIRTUAL_INPUT
        .iter()
        .chain(&["wl_seat", "xdg_wm_base", "wl_output"])
    {
        assert!(
            globals.interfaces.contains(*interface),
            "{interface} is missing from {:?}",
            globals.interfaces
        );
    }
    assert_eq!(globals.output_mode, Some((1920, 1080, 60_000)));
    stop_cleanly(&mut running, &socket_path);
}

/// `yuna --headless --rest-audit` logs the seconds that woke it, a client named by its process
/// and its seat, the test's own process here, and nothing while it rests: over 4.5 s with a
/// client connected and silent, at most the second of its start and the client's, when they are
/// apart, are logged.
#[test]
fn headless_with_the_rest_audit_names_a_client_then_rests() {
    let (mut running, log_reader, socket_path) =
        start_headless("rest-audit", &["--output", "640x480@60,1", "--rest-audit"]);
    let stream = UnixStream::connect(&socket_path).expect("the socket accepts a client");
    let connection = Connection::from_socket(stream).expect("a Wayland connection");
    let mut queue = connection.new_event_queue();
    connection.display().get_registry(&queue.handle(), ());
    let mut globals = Globals::default();
    queue.roundtrip(&mut globals).expect("the globals come");
    std::thread::sleep(Duration::from_millis(4500));
    stop_cleanly(&mut running, &socket_path);
    drop(connection);
    let log = log_reader
        .join()
        .expect("the log's reader does not panic")
        .expect("its log reads");
    let lines: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("rest audit: what woke Yuna in one second"))
        .collect();
    let named = format!("pid {} seat 1: ", std::process::id());
    assert!(
        lines.iter().any(|line| line.contains(&named)),
        "the client is not named\n{log}"
    );
    assert!((1..=2).contains(&lines.len()), "Yuna logged at rest\n{log}");
}

/// Starts `command`'s process with `limit` as its limit of open files.
#[expect(
    unsafe_code,
    reason = "the closure between fork and exec that sets the child's limit of open files"
)]
fn limit_open_files(command: &mut Command, limit: Rlimit) {
    // SAFETY: the closure runs in the child between fork and exec, where only async-signal-safe
    // calls are sound: it makes one system call, `prlimit64` through rustix, and allocates
    // nothing, an `Errno` turning into an `io::Error` without a box.
    unsafe {
        command.pre_exec(move || Ok(setrlimit(Resource::Nofile, limit)?));
    }
}

/// A client that makes `wl_shm_pool`s and destroys none: each keeps a descriptor in Yuna.
#[derive(Default)]
struct Pools {
    shm: Option<wl_shm::WlShm>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Pools {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
            && interface == "wl_shm"
        {
            state.shm = Some(registry.bind(name, 1, qh, ()));
        }
    }
}

delegate_noop!(Pools: ignore wl_shm::WlShm);
delegate_noop!(Pools: ignore wl_shm_pool::WlShmPool);

/// How many descriptors process `pid` holds.
fn descriptors_of(pid: u32) -> usize {
    std::fs::read_dir(format!("/proc/{pid}/fd")).map_or(0, Iterator::count)
}

/// Connects a client to the socket at `path` on a thread, which asks for its globals: the
/// receiver gets them once Yuna serves it, or why it was not served.
fn connect_on_a_thread(path: &std::path::Path) -> mpsc::Receiver<Result<BTreeSet<String>, String>> {
    let path = path.to_owned();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let served = || -> Result<BTreeSet<String>, String> {
            let stream = UnixStream::connect(&path).map_err(|e| e.to_string())?;
            let connection = Connection::from_socket(stream).map_err(|e| e.to_string())?;
            let mut queue = connection.new_event_queue();
            connection.display().get_registry(&queue.handle(), ());
            let mut globals = Globals::default();
            queue.roundtrip(&mut globals).map_err(|e| e.to_string())?;
            Ok(globals.interfaces)
        };
        let _ = sender.send(served());
    });
    receiver
}

/// The audit's story (m1.13-audit-core-4): `yuna --headless` under a limit of 256 open files,
/// soft and hard, so that it cannot raise it; one client of the owner's seat makes
/// `wl_shm_pool`s from one file, each of which keeps a descriptor in Yuna, until Yuna's table is
/// full. Another client connecting then made the owner's socket's `accept` fail, and Yuna
/// stopped with status 1, every client lost. The socket must pause instead, as an agent's seat's
/// does, Yuna keep running, and the waiting client be served once the flooding client leaves.
#[test]
fn a_client_filling_yunas_descriptors_does_not_stop_it() {
    const LIMIT: u64 = 256;
    let limit = Rlimit {
        current: Some(LIMIT),
        maximum: Some(LIMIT),
    };
    let (mut running, log_reader, socket_path) =
        start_headless_with("fd-flood", &["--output", "640x360@60,1"], |command| {
            limit_open_files(command, limit)
        });
    let pid = running.child.id();

    let stream = UnixStream::connect(&socket_path).expect("the socket accepts the flooding client");
    let connection = Connection::from_socket(stream).expect("a Wayland connection");
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    connection.display().get_registry(&handle, ());
    let mut flooder = Pools::default();
    queue.roundtrip(&mut flooder).expect("the globals come");
    let shm = flooder.shm.clone().expect("wl_shm is offered");
    let file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(running.runtime_dir.join("pool"))
        .expect("a file for the pools");
    file.set_len(4096).expect("the pools' size");
    // More pools than Yuna may hold descriptors: the last ones find its table full.
    let mut pools = Vec::new();
    for made in 1..=LIMIT + 64 {
        pools.push(shm.create_pool(file.as_fd(), 4096, &handle, ()));
        if made % 16 == 0 {
            let _ = connection.flush();
        }
    }
    let _ = connection.flush();
    let deadline = Instant::now() + Duration::from_secs(20);
    while descriptors_of(pid) < LIMIT as usize {
        assert!(
            running
                .child
                .try_wait()
                .expect("Yuna is waited for")
                .is_none(),
            "Yuna stopped during the flood"
        );
        assert!(
            Instant::now() < deadline,
            "Yuna holds {} descriptors, never {LIMIT}",
            descriptors_of(pid)
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // Its connection waits to be accepted, which fails while Yuna's table is full.
    let waiting = connect_on_a_thread(&socket_path);
    let watched_until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < watched_until {
        if let Some(status) = running.child.try_wait().expect("Yuna is waited for") {
            panic!("Yuna stopped when another client connected: {status}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    drop((pools, shm, flooder, queue, connection));
    let globals = waiting
        .recv_timeout(Duration::from_secs(30))
        .expect("the waiting client is served once the flooding client left")
        .expect("the waiting client is served");
    assert!(globals.contains("wl_compositor"), "{globals:?}");
    stop_cleanly(&mut running, &socket_path);
    let log = log_reader
        .join()
        .expect("the log's reader does not panic")
        .expect("its log reads");
    assert!(
        log.contains("a seat's socket pauses"),
        "the socket never paused, so the table was never full: {log}"
    );
}

/// Process `pid`'s limit of open files, soft and hard, as `/proc` gives it.
fn open_files_limit(pid: u32) -> (u64, u64) {
    let limits = std::fs::read_to_string(format!("/proc/{pid}/limits")).expect("its limits");
    let line = limits
        .lines()
        .find_map(|line| line.strip_prefix("Max open files"))
        .expect("a line of open files");
    let mut figures = line
        .split_whitespace()
        .map(|figure| figure.parse().expect("a figure"));
    (
        figures.next().expect("a soft limit"),
        figures.next().expect("a hard limit"),
    )
}

/// The first folder of the test's `PATH` that holds `program`.
fn on_path(program: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("a PATH"))
        .map(|folder| folder.join(program))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("no {program} on PATH"))
}

/// `yuna --headless` raises its soft limit of open files to its hard limit, as KWin and Mutter
/// do, since each client may make it hold many; the programs it starts get back the limit it
/// started with, which a program that still waits with `select` needs. The one `yuna
/// --headless` starts is Xwayland, at the first connection to its X display. The real one
/// raises its own limit as it starts (Xwayland 24.1 calls `setrlimit`), so a stand-in named
/// `Xwayland`, alone on Yuna's `PATH`, writes down the limit it was given, then exits.
#[test]
fn headless_raises_its_open_files_limit_but_not_its_programs() {
    const STARTED: u64 = 256;
    let hard = getrlimit(Resource::Nofile)
        .maximum
        .expect("a finite hard limit of open files");
    assert!(
        hard > STARTED,
        "the test needs a hard limit above {STARTED}: {hard}"
    );
    let started = Rlimit {
        current: Some(STARTED),
        maximum: Some(hard),
    };
    let stand_in = fresh_folder("test-open-files-path");
    let written = stand_in.join("limits");
    std::fs::write(
        stand_in.join("Xwayland"),
        format!(
            "#!{}\nulimit -Sn > '{}'\nulimit -Hn >> '{}'\n",
            on_path("sh").display(),
            written.display(),
            written.display()
        ),
    )
    .expect("the stand-in is written");
    std::fs::set_permissions(
        stand_in.join("Xwayland"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .expect("the stand-in runs");

    let (mut running, _log, socket_path) =
        start_headless_with("open-files", &["--output", "640x360@60,1"], |command| {
            command.env("PATH", &stand_in);
            limit_open_files(command, started);
        });
    let pid = running.child.id();
    assert_eq!(open_files_limit(pid), (hard, hard));

    let display = x11_display_number(&mut running);
    let address = SocketAddr::from_abstract_name(format!("/tmp/.X11-unix/X{display}"))
        .expect("an abstract address");
    let _x11 = UnixStream::connect_addr(&address).expect("the X display takes a connection");
    let deadline = Instant::now() + Duration::from_secs(20);
    let given = loop {
        let text = std::fs::read_to_string(&written).unwrap_or_default();
        let figures: Vec<u64> = text.lines().filter_map(|line| line.parse().ok()).collect();
        if figures.len() == 2 {
            break (figures[0], figures[1]);
        }
        assert!(Instant::now() < deadline, "Xwayland's stand-in never ran");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(given, (STARTED, hard));
    stop_cleanly(&mut running, &socket_path);
}

#[test]
fn an_unknown_argument_is_a_usage_error() {
    let output = run_yuna(&["--start"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains(r#"unknown argument "--start""#));
}

#[test]
fn a_reader_that_left_early_is_no_panic() {
    let (reader, writer) = std::io::pipe().expect("a pipe opens");
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_yuna"))
        .arg("--help")
        .stdout(writer)
        .output()
        .expect("the yuna binary runs");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
