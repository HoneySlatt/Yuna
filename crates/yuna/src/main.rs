//! The `yuna` program: reads its command line, and runs the headless compositor until SIGTERM or
//! SIGINT, or, as `yuna mcp`, an MCP server of its own until its client closes its input
//! ([ADR-0120](../../../docs/adr/0120-the-mcp-server-a-private-yuna-on-stdio.md)). A panic of
//! the thread that runs either stops it at once (`fatal.rs`). The desktop session does not exist
//! yet (Stage 2); the work in progress is in `docs/PLAN.md`.
//!
//! Both modes raise their soft limit of open files to the hard one at start
//! ([`yuna::raise_open_files_limit`]), since each client may make Yuna hold descriptors; the
//! programs Yuna starts get back the limit it started with.
//!
//! The log goes to standard error, at `YUNA_LOG`'s level. At `debug`, tests and measures read
//! these lines, by their message and their fields:
//!
//! - `window opened` (`shell.rs`), when a window maps;
//! - `frame drawn` (`frames.rs`), for each frame: `took_us`, its drawing's time; `lead_us`, how
//!   long before the boundary it aimed at its refresh was due; `at_once`, whether its output was
//!   idle; `boundary`, the boundary it aimed at; `presented` and `presented_at_us`, the one it
//!   was presented at;
//! - `counters` (`compositor.rs`), after each round that woke: frames drawn, frame callbacks,
//!   configures, commits and wake-ups so far;
//! - `capture copied` (`capture.rs`) and `capture encoded` (`mcp/captures.rs`), each with its
//!   `took_us`.

mod fatal;
mod stop_signal;

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::process::ExitCode;

use tracing::level_filters::LevelFilter;
use yuna::mcp::{Server, Stop};
use yuna::yuna_motion::Settings;
use yuna::{ClockMode, Compositor, Config, Device, Error, OWNER_SEAT, OutputMode, XwaylandConfig};
use yuna_types::SeatId;

// The command line's contract (ADR-0079), held in `tests/contracts/help.txt`. The environment
// lists every variable a user sets that Yuna's own code reads, `TMPDIR` for `yuna mcp`'s agents'
// throwaway homes among them (`mcp/programs/homes.rs`). It leaves out the switches that exist in debug builds alone, for the tests of failures:
// `YUNA_PANIC_ON_TOOL`, `YUNA_PANIC_IN_ENCODERS` (`mcp/`), `YUNA_PANIC_ON_X11_CLOSE` and
// `YUNA_X11_OWED_BOUND` (`xwayland/manager/`); `YUNA_PARITY_CANARY`, read only in a build with the
// parity script's `parity-canary` feature (`canary.rs`); and the
// variables Yuna's package sets for itself (`flake.nix`): `YUNA_WRAPPED_VARIABLES` and
// `YUNA_CALLER_<name>`, which give mcp's programs the caller's own values back
// (`mcp/programs/environment.rs`). The libraries read their own, xkbcommon's `XKB_*` and Mesa's
// among them, and mcp's programs are given a few of Yuna's (`KEPT_VARIABLES` there).
const HELP: &str = "\
Yuna, a Wayland compositor for Linux.

Usage: yuna --headless [--agent-seat] [--gpu] [--animations | --reduced-motion]
                       [--output MODE]... [--rest-audit]
       yuna mcp [--gpu] [--animations | --reduced-motion] [--output MODE] [--manual-clock]
                [--rest-audit]
       yuna -h | --help
       yuna -V | --version

Without an argument, Yuna would start the desktop session, which does not exist yet.

Commands:
  mcp                An MCP server on standard input and output: a headless Yuna of its own,
                     whose tools create seats, each with its own outputs, Wayland socket,
                     X display, pointer and keyboard, start programs on them, list
                     windows, give input, and read the clock. Logs to standard error;
                     stops when its input closes, or on SIGTERM or SIGINT, and ends its
                     programs then.

Options:
      --headless     Run without a screen, on virtual outputs drawn offscreen. Prints
                     WAYLAND_DISPLAY=<socket> on standard output, then, when Xwayland is
                     found, DISPLAY=:<number> and XAUTHORITY=<file> for X11 programs; logs
                     to standard error, and stops on SIGTERM or SIGINT.
      --agent-seat   With --headless: serve an agent's seat on the socket rather than the
                     owner's, as an agent's seat of mcp is: a us keyboard, a pointer, Yuna
                     as input method, and the virtual keyboard and pointer for its programs
                     (zwp_virtual_keyboard_v1, zwlr_virtual_pointer_v1), which drive them
                     as in a headless sway.
      --gpu          Draw on the GPU's render node rather than on Mesa's llvmpipe.
      --animations   Move windows, the view and workspaces on springs, and fade the special
                     workspace, rather than show each change at once, as without it.
      --reduced-motion
                     Animations under reduced motion: nothing slides, each change shows at
                     once, and the special workspace's fade is kept, shorter.
      --output MODE  Add a virtual output. MODE is WIDTHxHEIGHT@RATE,SCALE[,vrr=MIN][,hdr]:
                     WIDTHxHEIGHT physical pixels, refreshed RATE times a second, at SCALE
                     physical pixels per logical unit; with vrr=MIN, under VRR from MIN to
                     RATE Hz, RATE at least twice MIN; with hdr, announcing the HDR colours
                     of the owner's monitor, a PG32UCDM. Outputs are laid out left to right.
                     Default: one output, 3840x2160@240,1.5. With mcp, given once at most:
                     the mode of each new seat's output.
      --manual-clock With mcp: time moves only when the clock tool advances it, never past
                     real time.
      --rest-audit   Log, for each second in which something woke Yuna, one line at info:
                     what woke it, by source and by client, its process, seat and, with
                     mcp, program, and the frames it drew, by cause. At rest, nothing is
                     logged.
  -h, --help         Print this help
  -V, --version      Print the version

Environment:
  XDG_RUNTIME_DIR    The folder the Wayland socket is created in, and with mcp, the folder of
                     its seats' sockets and its programs' logs; required.
  PATH               Where Xwayland is looked for: the first X11 program to connect to a
                     seat's display starts it. Without it, X11 programs get no display. With
                     mcp, where dbus-daemon is looked for too, unless YUNA_DBUS_DAEMON names
                     one.
  YUNA_DBUS_DAEMON   With mcp: the absolute path of the dbus-daemon that runs each seat's
                     accessibility bus, rather than the first on PATH.
  YUNA_LOG           What is logged: error, warn, info (the default), debug or trace.
  TMPDIR             With mcp: the folder of its agents' throwaway homes, /tmp if unset.

Exit status:
  0                  Success: the help or the version printed, or a stop asked for: SIGTERM
                     or SIGINT, or, with mcp, its input closed.
  1                  A failure: the desktop session, which does not exist yet; a start or a
                     loop that failed; an output that could not be written; with mcp, a
                     client that broke the connection.
  2                  A usage error: an argument, or an output's mode, Yuna does not take.
  101                Yuna's state panicked, a bug: it stopped at once, with mcp after ending
                     its programs.
";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
enum Request {
    Help,
    Version,
    /// No argument: the desktop session, which does not exist yet.
    Session,
    Headless(Headless),
    /// `yuna mcp`.
    Mcp(Headless),
    /// Arguments Yuna does not take; the reason says why.
    Invalid(String),
}

/// What `--headless` and `mcp` run.
#[derive(Debug, PartialEq)]
struct Headless {
    gpu: bool,
    outputs: Vec<OutputMode>,
    /// The manual clock, which `mcp` alone takes.
    manual_clock: bool,
    /// Animations on; off without `--animations` (ADR-0055).
    animations: bool,
    /// Animations on under reduced motion (ADR-0059): `--reduced-motion`.
    reduced_motion: bool,
    /// `--headless`'s socket serves an agent's seat rather than the owner's (ADR-0089).
    agent_seat: bool,
}

/// What the program logs beside what `YUNA_LOG`'s level shows.
#[derive(Debug, Default, PartialEq)]
struct Logging {
    /// `--rest-audit`: for each second in which something woke Yuna, what woke it and what it
    /// drew (ADR-0062).
    rest_audit: bool,
}

/// Reads the arguments that follow the program's name: what to run, and what to log beside.
/// They are `OsString`s, so that an argument that is not UTF-8 is refused like any other unknown
/// one, never a panic.
fn read_command_line(arguments: impl IntoIterator<Item = OsString>) -> (Request, Logging) {
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    let alone = |request| (request, Logging::default());
    match arguments.as_slice() {
        [] => alone(Request::Session),
        [one] if one == "-h" || one == "--help" => alone(Request::Help),
        [one] if one == "-V" || one == "--version" => alone(Request::Version),
        [first, rest @ ..] if first == "mcp" => read_headless(rest, true),
        _ => read_headless(&arguments, false),
    }
}

/// What the command line asks to run, what it logs left aside.
#[cfg(test)]
fn read_request(arguments: impl IntoIterator<Item = OsString>) -> Request {
    read_command_line(arguments).0
}

/// Reads `--headless`'s options, or, for `mcp`, its own: no `--headless`, which it is already,
/// and `--manual-clock`.
fn read_headless(arguments: &[OsString], mcp: bool) -> (Request, Logging) {
    let invalid = |reason: String| (Request::Invalid(reason), Logging::default());
    let mut headless = mcp;
    let mut gpu = false;
    let mut manual_clock = false;
    let mut animations = false;
    let mut reduced_motion = false;
    let mut agent_seat = false;
    let mut rest_audit = false;
    let mut outputs = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--headless") if !headless => headless = true,
            Some("--rest-audit") if !rest_audit => rest_audit = true,
            Some("--gpu") if !gpu => gpu = true,
            Some("--animations") if !animations && !reduced_motion => animations = true,
            Some("--reduced-motion") if !animations && !reduced_motion => reduced_motion = true,
            Some(flag @ "--animations") if animations => {
                return invalid(format!("{flag} is given twice"));
            }
            Some(flag @ "--reduced-motion") if reduced_motion => {
                return invalid(format!("{flag} is given twice"));
            }
            Some("--animations" | "--reduced-motion") => {
                return invalid(
                    "--animations and --reduced-motion are one choice: give one".to_owned(),
                );
            }
            Some("--manual-clock") if mcp && !manual_clock => manual_clock = true,
            Some("--agent-seat") if mcp => {
                return invalid("mcp's seats are agents' already: drop --agent-seat".to_owned());
            }
            Some("--agent-seat") if !agent_seat => agent_seat = true,
            Some("--headless") if mcp => {
                return invalid("mcp is headless already: drop --headless".to_owned());
            }
            Some("--manual-clock") if !mcp => {
                return invalid("--manual-clock is mcp's".to_owned());
            }
            Some(
                flag
                @ ("--headless" | "--gpu" | "--manual-clock" | "--agent-seat" | "--rest-audit"),
            ) => {
                return invalid(format!("{flag} is given twice"));
            }
            Some("--output") => match arguments.next().map(|mode| read_output(mode)) {
                Some(Ok(mode)) => outputs.push(mode),
                Some(Err(reason)) => return invalid(reason),
                None => {
                    return invalid("--output needs a mode, such as 1920x1080@60,1".to_owned());
                }
            },
            Some(flag @ ("-h" | "--help" | "-V" | "--version")) => {
                return invalid(format!("{flag} takes no other argument"));
            }
            // Quoted, so that an empty argument shows, and escaped where it is not UTF-8.
            _ => return invalid(format!("unknown argument {argument:?}")),
        }
    }
    if !headless {
        return invalid(
            "--agent-seat, --gpu, --animations, --reduced-motion, --output and --rest-audit \
             need --headless"
                .to_owned(),
        );
    }
    if mcp && outputs.len() > 1 {
        return invalid(
            "mcp takes one --output, each seat's first output; `outputs create` adds more"
                .to_owned(),
        );
    }
    if outputs.is_empty() {
        outputs.push(OutputMode::default());
    }
    let headless = Headless {
        gpu,
        outputs,
        manual_clock,
        animations,
        reduced_motion,
        agent_seat,
    };
    let logging = Logging { rest_audit };
    if mcp {
        (Request::Mcp(headless), logging)
    } else {
        (Request::Headless(headless), logging)
    }
}

/// Reads an output's mode, `WIDTHxHEIGHT@RATE,SCALE[,vrr=MIN][,hdr]` (ADR-0049), for
/// `--output`.
fn read_output(text: &OsStr) -> Result<OutputMode, String> {
    let text_str = text.to_str().ok_or_else(|| {
        format!(
            "--output {text:?}: expected WIDTHxHEIGHT@RATE,SCALE[,vrr=MIN][,hdr], such as \
             1920x1080@60,1"
        )
    })?;
    text_str
        .parse()
        .map_err(|reason| format!("--output {text_str}: {reason}"))
}

fn main() -> ExitCode {
    let (request, logging) = read_command_line(std::env::args_os().skip(1));
    match request {
        Request::Help => print(HELP),
        Request::Version => print(format!("yuna {}\n", env!("CARGO_PKG_VERSION"))),
        Request::Session => {
            print_error(
                "yuna: the desktop session does not exist yet (Stage 2); run `yuna --headless`\n",
            );
            ExitCode::FAILURE
        }
        Request::Headless(headless) => run_headless(headless, &logging),
        Request::Mcp(headless) => run_mcp(headless, &logging),
        Request::Invalid(reason) => usage_error(&reason),
    }
}

/// 2, as most command-line programs, for a usage error.
fn usage_error(reason: &str) -> ExitCode {
    print_error(&format!("yuna: {reason}\n\n{HELP}"));
    ExitCode::from(2)
}

fn run_headless(headless: Headless, logging: &Logging) -> ExitCode {
    // Nothing of its own to end: a panic of this thread, which runs the compositor, stops it.
    fatal::install();
    start_logging();
    raise_open_files_limit();
    // Before the compositor starts, so that a SIGTERM during its start-up is kept in the pipe and
    // stops it as soon as its loop runs, rather than killing it half started.
    let stop = match stop_signal::install() {
        Ok(stop) => stop,
        Err(error) => {
            print_error(&format!("yuna: cannot handle SIGTERM: {error}\n"));
            return ExitCode::FAILURE;
        }
    };
    let (mut compositor, seat, socket) = match start_compositor(headless) {
        Ok(started) => started,
        Err(exit) => return exit,
    };
    if logging.rest_audit {
        compositor.log_rest_each_second();
    }
    {
        let mut lines = format!("WAYLAND_DISPLAY={}\n", socket.to_string_lossy()).into_bytes();
        if let Some(x11) = compositor.x11_display(seat) {
            lines.extend_from_slice(format!("DISPLAY={}\nXAUTHORITY=", x11.name()).as_bytes());
            // The path as it is, which may not be UTF-8.
            lines.extend_from_slice(x11.xauthority.as_os_str().as_bytes());
            lines.push(b'\n');
        }
        // A reader that left does not stop the compositor: its clients may know the socket.
        let _ = print(lines);
    }
    match compositor.run_until(stop) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "the compositor stopped");
            ExitCode::FAILURE
        }
    }
}

/// Starts the compositor `yuna --headless` asks for, listening on a socket in `XDG_RUNTIME_DIR`
/// for the owner's seat, its outputs on it, or with `--agent-seat` for an agent's seat, its
/// outputs on that seat and the owner's seat without output, socket or `wl_seat`, as in `yuna
/// mcp`. Returns it with the seat it serves and the socket's name; reports why it cannot.
fn start_compositor(headless: Headless) -> Result<(Compositor, SeatId, OsString), ExitCode> {
    let config = Config {
        device: device_of(&headless),
        animations: animations_of(&headless),
        outputs: if headless.agent_seat {
            Vec::new()
        } else {
            headless.outputs.clone()
        },
        clock: ClockMode::Real,
        listen: !headless.agent_seat,
        announce_owner_seat: !headless.agent_seat,
        xwayland: Some(XwaylandConfig::default()),
    };
    let mut compositor = start(config)?;
    if !headless.agent_seat {
        let socket = compositor.socket_name().map(OsStr::to_os_string);
        return match socket {
            Some(socket) => Ok((compositor, OWNER_SEAT, socket)),
            None => {
                print_error("yuna: the compositor has no socket\n");
                Err(ExitCode::FAILURE)
            }
        };
    }
    let serve = |compositor: &mut Compositor| -> Result<(SeatId, OsString), Error> {
        let seat = compositor.add_seat()?;
        for mode in &headless.outputs {
            compositor.add_output(seat, *mode)?;
        }
        let socket = compositor.listen_for_seat_in_runtime_dir(seat)?;
        Ok((seat, socket))
    };
    match serve(&mut compositor) {
        Ok((seat, socket)) => Ok((compositor, seat, socket)),
        Err(Error::InvalidOutput(reason)) => Err(usage_error(&reason.to_string())),
        Err(error) => {
            print_error(&format!("yuna: {error}\n"));
            Err(ExitCode::FAILURE)
        }
    }
}

fn device_of(headless: &Headless) -> Device {
    if headless.gpu {
        Device::Gpu
    } else {
        Device::Software
    }
}

/// The default animations, off unless `--animations` or `--reduced-motion` turned them on.
fn animations_of(headless: &Headless) -> Settings {
    Settings {
        off: !headless.animations && !headless.reduced_motion,
        reduced_motion: headless.reduced_motion,
        ..Settings::default()
    }
}

/// Starts a compositor; reports why it cannot.
fn start(config: Config) -> Result<Compositor, ExitCode> {
    match Compositor::new(config) {
        Ok(compositor) => Ok(compositor),
        Err(Error::InvalidOutput(reason)) => Err(usage_error(&reason.to_string())),
        Err(error) => {
            print_error(&format!("yuna: {error}\n"));
            Err(ExitCode::FAILURE)
        }
    }
}

/// `yuna mcp`: the MCP server on standard input and output, until its input closes or a signal
/// stops it. Yuna becomes its children's subreaper, so that every process its programs start
/// stays under it, to be reaped and ended, on purpose or after a panic of this thread, which
/// runs the server.
fn run_mcp(headless: Headless, logging: &Logging) -> ExitCode {
    let on_panic = fatal::install();
    start_logging();
    raise_open_files_limit();
    let signals = stop_signal::install().and_then(|stop| {
        let children = stop_signal::install_children()?;
        rustix::process::set_child_subreaper(Some(rustix::process::getpid()))?;
        Ok((stop, children))
    });
    let (stop, children) = match signals {
        Ok(signals) => signals,
        Err(error) => {
            print_error(&format!("yuna: cannot handle signals: {error}\n"));
            return ExitCode::FAILURE;
        }
    };
    let streams = io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .and_then(|input| Ok((input, io::stdout().as_fd().try_clone_to_owned()?)));
    let (input, output): (OwnedFd, OwnedFd) = match streams {
        Ok(streams) => streams,
        Err(error) => {
            print_error(&format!(
                "yuna: cannot use standard input and output: {error}\n"
            ));
            return ExitCode::FAILURE;
        }
    };
    // No output, no socket and no `wl_seat` for the owner's seat: everything is on the seats
    // agents create (ADR-0078, ADR-0037).
    let seat_output = headless.outputs.first().copied().unwrap_or_default();
    let config = Config {
        outputs: Vec::new(),
        device: device_of(&headless),
        clock: if headless.manual_clock {
            ClockMode::Manual
        } else {
            ClockMode::Real
        },
        listen: false,
        announce_owner_seat: false,
        animations: animations_of(&headless),
        // Each seat `seat create` makes gets a display with its socket.
        xwayland: Some(XwaylandConfig::default()),
    };
    let mut compositor = match start(config) {
        Ok(compositor) => compositor,
        Err(exit) => return exit,
    };
    if logging.rest_audit {
        compositor.log_rest_each_second();
    }
    let server = match Server::new(compositor, input, output, Some(children)) {
        Ok(mut server) => {
            server.set_seat_output(seat_output);
            // No program can start before the server, so none outlives a panic before this.
            if let Some(folder) = server.programs_folder() {
                on_panic.end_programs_of(folder.to_owned());
            }
            server
        }
        Err(error) => {
            print_error(&format!("yuna: {error}\n"));
            return ExitCode::FAILURE;
        }
    };
    match server.run(stop) {
        Ok(Stop::InputClosed | Stop::Signal) => ExitCode::SUCCESS,
        Ok(Stop::Broken(reason)) => {
            tracing::error!(%reason, "the client broke the connection");
            ExitCode::FAILURE
        }
        Err(error) => {
            tracing::error!(%error, "the compositor stopped");
            ExitCode::FAILURE
        }
    }
}

/// Raises the limit of open files before anything starts, so that one client cannot fill Yuna's
/// table at the usual 1,024 ([`yuna::raise_open_files_limit`]). A failure leaves the limit as it
/// was, and is logged: it never stops Yuna.
fn raise_open_files_limit() {
    match yuna::raise_open_files_limit() {
        Ok(limit) => tracing::debug!(?limit, "the soft limit of open files"),
        Err(error) => tracing::warn!(%error, "the limit of open files could not be raised"),
    }
}

/// Logs to standard error at the level `YUNA_LOG` names, `info` without it. A value that names
/// no level is reported and ignored: a setting never stops Yuna (ADR-0008).
fn start_logging() {
    let (level, unknown) = match std::env::var_os("YUNA_LOG") {
        None => (LevelFilter::INFO, None),
        Some(value) => match value.to_str().map(str::to_ascii_lowercase).as_deref() {
            Some("error") => (LevelFilter::ERROR, None),
            Some("warn") => (LevelFilter::WARN, None),
            Some("info") => (LevelFilter::INFO, None),
            Some("debug") => (LevelFilter::DEBUG, None),
            Some("trace") => (LevelFilter::TRACE, None),
            _ => (LevelFilter::INFO, Some(value)),
        },
    };
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(io::stderr)
        .try_init();
    if let Some(value) = unknown {
        tracing::warn!(
            ?value,
            "YUNA_LOG names no level (error, warn, info, debug, trace); logging at info"
        );
    }
}

/// Writes to standard output. When the reader has left (`yuna --help | true`), the write fails and
/// the program exits with a failure, without the panic of `print!` and without a message.
fn print(text: impl AsRef<[u8]>) -> ExitCode {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_ref())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// Writes to standard error. If that fails too, nothing is left to report it to: the exit status
/// alone tells.
fn print_error(text: &str) {
    let _ = io::stderr().lock().write_all(text.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    use yuna::colour::description::PG32UCDM;

    fn request(arguments: &[&str]) -> Request {
        read_request(arguments.iter().map(OsString::from))
    }

    fn mode(width: u32, height: u32, rate: f64, scale: f64) -> OutputMode {
        OutputMode::new(width, height, rate, scale).expect("a valid mode")
    }

    #[test]
    fn no_argument_asks_for_a_session() {
        assert_eq!(request(&[]), Request::Session);
    }

    #[test]
    fn help_and_version_have_a_short_and_a_long_form() {
        assert_eq!(request(&["-h"]), Request::Help);
        assert_eq!(request(&["--help"]), Request::Help);
        assert_eq!(request(&["-V"]), Request::Version);
        assert_eq!(request(&["--version"]), Request::Version);
    }

    #[test]
    fn unknown_empty_and_extra_arguments_are_refused() {
        assert!(matches!(request(&["--start"]), Request::Invalid(_)));
        assert!(matches!(request(&[""]), Request::Invalid(_)));
        assert!(matches!(
            request(&["--help", "--version"]),
            Request::Invalid(_)
        ));
        assert!(matches!(
            request(&["--headless", "--help"]),
            Request::Invalid(_)
        ));
    }

    #[test]
    fn an_argument_that_is_not_utf8_is_refused_without_a_panic() {
        let not_utf8 = OsString::from_vec(vec![0x66, 0x6f, 0x80]);
        assert!(matches!(read_request([not_utf8]), Request::Invalid(_)));
        let headless = OsString::from("--headless");
        let output = OsString::from("--output");
        let not_utf8 = OsString::from_vec(vec![0x31, 0x80]);
        assert!(matches!(
            read_request([headless, output, not_utf8]),
            Request::Invalid(_)
        ));
    }

    #[test]
    fn headless_alone_has_one_default_output_on_llvmpipe() {
        assert_eq!(
            request(&["--headless"]),
            Request::Headless(Headless {
                gpu: false,
                outputs: vec![mode(3840, 2160, 240.0, 1.5)],
                manual_clock: false,
                animations: false,
                reduced_motion: false,
                agent_seat: false,
            })
        );
    }

    #[test]
    fn outputs_are_kept_in_order_and_the_gpu_asked() {
        assert_eq!(
            request(&[
                "--output",
                "1920x1080@60,1",
                "--headless",
                "--gpu",
                "--output",
                "2560x1440@144,1.25",
            ]),
            Request::Headless(Headless {
                gpu: true,
                outputs: vec![mode(1920, 1080, 60.0, 1.0), mode(2560, 1440, 144.0, 1.25)],
                manual_clock: false,
                animations: false,
                reduced_motion: false,
                agent_seat: false,
            })
        );
    }

    #[test]
    fn the_agent_seat_is_headless_alone() {
        assert_eq!(
            request(&["--headless", "--agent-seat"]),
            Request::Headless(Headless {
                gpu: false,
                outputs: vec![mode(3840, 2160, 240.0, 1.5)],
                manual_clock: false,
                animations: false,
                reduced_motion: false,
                agent_seat: true,
            })
        );
        for bad in [
            &["--agent-seat"][..],
            &["--headless", "--agent-seat", "--agent-seat"],
            &["mcp", "--agent-seat"],
        ] {
            assert!(matches!(request(bad), Request::Invalid(_)), "{bad:?}");
        }
    }

    #[test]
    fn mcp_takes_the_headless_options_and_the_manual_clock() {
        assert_eq!(
            request(&["mcp"]),
            Request::Mcp(Headless {
                gpu: false,
                outputs: vec![mode(3840, 2160, 240.0, 1.5)],
                manual_clock: false,
                animations: false,
                reduced_motion: false,
                agent_seat: false,
            })
        );
        assert_eq!(
            request(&[
                "mcp",
                "--manual-clock",
                "--output",
                "1920x1080@60,1",
                "--gpu"
            ]),
            Request::Mcp(Headless {
                gpu: true,
                outputs: vec![mode(1920, 1080, 60.0, 1.0)],
                manual_clock: true,
                animations: false,
                reduced_motion: false,
                agent_seat: false,
            })
        );
        for bad in [
            &["mcp", "--headless"][..],
            &["mcp", "--manual-clock", "--manual-clock"],
            &["--headless", "--manual-clock"],
            &["--headless", "mcp"],
            &["mcp", "--output"],
            &["mcp", "extra"],
        ] {
            assert!(matches!(request(bad), Request::Invalid(_)), "{bad:?}");
        }
    }

    #[test]
    fn malformed_and_out_of_range_outputs_are_refused() {
        for bad in [
            "1920x1080",
            "1920x1080@60",
            "1920@60,1",
            "x1080@60,1",
            "-1920x1080@60,1",
            "1920x1080@60,1,2",
            "0x1080@60,1",
            "1920x1080@0,1",
            "1920x1080@1001,1",
            "1920x1080@NaN,1",
            "1920x1080@60,0.1",
            "1920x1080@60,inf",
            "99999999999x1080@60,1",
        ] {
            assert!(
                matches!(
                    request(&["--headless", "--output", bad]),
                    Request::Invalid(_)
                ),
                "{bad} was accepted"
            );
        }
        assert!(matches!(
            request(&["--headless", "--output"]),
            Request::Invalid(_)
        ));
    }

    #[test]
    fn an_output_takes_vrr_and_hdr_in_either_order() {
        let fixed = mode(1920, 1080, 240.0, 1.0);
        let vrr = fixed.with_vrr(48.0).expect("a range twice as wide");
        let vrr_hdr = vrr.with_hdr(PG32UCDM);
        for (text, expected) in [
            ("1920x1080@240,1,vrr=48,hdr", vrr_hdr),
            ("1920x1080@240,1,hdr,vrr=48", vrr_hdr),
            ("1920x1080@240,1,vrr=48", vrr),
            ("1920x1080@240,1,hdr", fixed.with_hdr(PG32UCDM)),
            // Exactly twice: the narrowest range accepted.
            (
                "1920x1080@240,1,vrr=120",
                fixed.with_vrr(120.0).expect("a range twice as wide"),
            ),
        ] {
            assert_eq!(
                request(&["--headless", "--output", text]),
                Request::Headless(Headless {
                    gpu: false,
                    outputs: vec![expected],
                    manual_clock: false,
                    animations: false,
                    reduced_motion: false,
                    agent_seat: false,
                }),
                "{text}"
            );
        }
        assert_eq!(
            request(&["mcp", "--output", "1920x1080@240,1,hdr,vrr=48"]),
            Request::Mcp(Headless {
                gpu: false,
                outputs: vec![vrr_hdr],
                manual_clock: false,
                animations: false,
                reduced_motion: false,
                agent_seat: false,
            })
        );
    }

    #[test]
    fn a_repeated_unknown_or_malformed_item_and_a_narrow_range_are_named() {
        for (bad, named) in [
            ("1920x1080@240,1,hdr,hdr", "hdr is given twice"),
            ("1920x1080@240,1,hdr,vrr=48,hdr", "hdr is given twice"),
            ("1920x1080@240,1,vrr=48,vrr=60", "vrr is given twice"),
            ("1920x1080@240,1,sdr", "unknown item \"sdr\""),
            ("1920x1080@240,1,HDR", "unknown item \"HDR\""),
            ("1920x1080@240,1,hdr=1", "unknown item \"hdr=1\""),
            ("1920x1080@240,1,vrr", "unknown item \"vrr\""),
            ("1920x1080@240,1,", "unknown item \"\""),
            ("1920x1080@240,1,vrr=abc", "vrr=abc is not a rate in Hz"),
            ("1920x1080@240,1,vrr=", "vrr= is not a rate in Hz"),
            ("1920x1080@240,1,vrr=200", "VRR range of 200 to 240 Hz"),
            ("1920x1080@240,1,hdr,vrr=121", "VRR range of 121 to 240 Hz"),
            ("1920x1080@240,1,vrr=0", "VRR range of 0 to 240 Hz"),
            ("1920x1080@240,1,vrr=-48", "VRR range of -48 to 240 Hz"),
            ("1920x1080@240,1,vrr=NaN", "VRR range of NaN to 240 Hz"),
        ] {
            for command in [
                &["--headless", "--output", bad][..],
                &["mcp", "--output", bad],
            ] {
                match request(command) {
                    Request::Invalid(reason) => {
                        assert!(reason.contains(named), "{bad}: {reason}");
                        assert!(reason.starts_with("--output 1920x1080@240,1,"), "{reason}");
                    }
                    other => panic!("{bad} was accepted: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn animations_are_off_unless_asked_in_both_modes() {
        for (command, mcp) in [
            (&["--headless", "--animations"][..], false),
            (&["mcp", "--animations"], true),
        ] {
            let expected = Headless {
                gpu: false,
                outputs: vec![mode(3840, 2160, 240.0, 1.5)],
                manual_clock: false,
                animations: true,
                reduced_motion: false,
                agent_seat: false,
            };
            let expected = if mcp {
                Request::Mcp(expected)
            } else {
                Request::Headless(expected)
            };
            assert_eq!(request(command), expected, "{command:?}");
        }
        for bad in [
            &["--headless", "--animations", "--animations"][..],
            &["mcp", "--animations", "--animations"],
            &["--animations"],
        ] {
            match request(bad) {
                Request::Invalid(reason) => assert!(reason.contains("--animations"), "{reason}"),
                other => panic!("{bad:?} was accepted: {other:?}"),
            }
        }
    }

    #[test]
    fn reduced_motion_turns_animations_on_reduced_and_excludes_animations() {
        let Request::Headless(headless) = request(&["--headless", "--reduced-motion"]) else {
            panic!("--headless --reduced-motion was refused");
        };
        assert!(headless.reduced_motion && !headless.animations);
        let settings = animations_of(&headless);
        assert!(!settings.off && settings.reduced_motion);
        assert!(matches!(
            request(&["mcp", "--reduced-motion"]),
            Request::Mcp(Headless {
                reduced_motion: true,
                agent_seat: false,
                ..
            })
        ));
        for bad in [
            &["--headless", "--animations", "--reduced-motion"][..],
            &["--headless", "--reduced-motion", "--animations"],
            &["mcp", "--reduced-motion", "--reduced-motion"],
            &["--reduced-motion"],
        ] {
            match request(bad) {
                Request::Invalid(reason) => {
                    assert!(reason.contains("--reduced-motion"), "{reason}")
                }
                other => panic!("{bad:?} was accepted: {other:?}"),
            }
        }
    }

    #[test]
    fn the_rest_audit_is_asked_once_in_either_mode_and_needs_one() {
        let read = |arguments: &[&str]| read_command_line(arguments.iter().map(OsString::from));
        for command in [
            &["--headless", "--rest-audit"][..],
            &["--rest-audit", "--headless", "--agent-seat"],
            &["mcp", "--manual-clock", "--rest-audit"],
        ] {
            let (request, logging) = read(command);
            assert!(
                matches!(request, Request::Headless(_) | Request::Mcp(_)),
                "{command:?}: {request:?}"
            );
            assert_eq!(logging, Logging { rest_audit: true }, "{command:?}");
        }
        assert_eq!(read(&["--headless"]).1, Logging::default());
        assert_eq!(read(&["mcp"]).1, Logging::default());
        for bad in [
            &["--rest-audit"][..],
            &["--headless", "--rest-audit", "--rest-audit"],
            &["mcp", "--rest-audit", "--rest-audit"],
            &["--rest-audit", "mcp"],
            &["--help", "--rest-audit"],
        ] {
            let (request, logging) = read(bad);
            assert!(
                matches!(request, Request::Invalid(_)),
                "{bad:?}: {request:?}"
            );
            assert_eq!(logging, Logging::default(), "{bad:?}");
        }
    }

    #[test]
    fn flags_given_twice_or_without_headless_are_refused() {
        assert!(matches!(
            request(&["--headless", "--headless"]),
            Request::Invalid(_)
        ));
        assert!(matches!(
            request(&["--headless", "--gpu", "--gpu"]),
            Request::Invalid(_)
        ));
        assert!(matches!(request(&["--gpu"]), Request::Invalid(_)));
        assert!(matches!(
            request(&["--output", "1920x1080@60,1"]),
            Request::Invalid(_)
        ));
    }
}
