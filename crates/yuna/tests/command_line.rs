//! The program run as a user runs it: its output and its exit status.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use smithay::reexports::rustix::process::{Pid, Signal, kill_process};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

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
        "YUNA_LOG",
    ] {
        assert!(help.contains(option), "the help does not name {option}");
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
    let runtime_dir = std::env::temp_dir().join(format!("yuna-test-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&runtime_dir).expect("a runtime folder");
    let child = Command::new(env!("CARGO_BIN_EXE_yuna"))
        .arg("--headless")
        .args(arguments)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        // Info, for the line that says whether linux-dmabuf is offered: a few lines in all.
        .env("YUNA_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the yuna binary runs");
    let mut running = Running { child, runtime_dir };

    // Read on a thread from the start: a pipe may hold as little as one page when the user's
    // pipes are many, and Yuna, blocked writing its log, would never print its socket.
    let mut stderr = running.child.stderr.take().expect("its standard error");
    let log_reader = std::thread::spawn(move || {
        let mut log = String::new();
        stderr.read_to_string(&mut log).map(|_| log)
    });
    let stdout = running.child.stdout.take().expect("its standard output");
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("a line on standard output");
    let socket = line
        .trim_end()
        .strip_prefix("WAYLAND_DISPLAY=")
        .unwrap_or_else(|| panic!("no socket printed: {line:?}"))
        .to_owned();
    let socket_path = running.runtime_dir.join(&socket);
    (running, log_reader, socket_path)
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
