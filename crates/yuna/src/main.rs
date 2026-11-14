//! The `yuna` program: reads its command line, and runs the headless compositor until SIGTERM or
//! SIGINT, or, as `yuna mcp`, an MCP server of its own until its client closes its input
//! ([ADR-0033](../../../docs/adr/0033-the-mcp-server.md)). The desktop session does not exist yet
//! (Stage 2); the work in progress is in `docs/PLAN.md`.

mod stop_signal;

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::process::ExitCode;

use smithay::reexports::rustix;
use tracing::level_filters::LevelFilter;
use yuna::colour::description::PG32UCDM;
use yuna::mcp::{Server, Stop};
use yuna::yuna_motion::Settings;
use yuna::{ClockMode, Compositor, Config, Device, Error, OutputMode};

const HELP: &str = "\
Yuna, a Wayland compositor for Linux.

Usage: yuna --headless [--gpu] [--animations | --reduced-motion] [--output MODE]...
       yuna mcp [--gpu] [--animations | --reduced-motion] [--output MODE] [--manual-clock]
       yuna -h | --help
       yuna -V | --version

Without an argument, Yuna would start the desktop session, which does not exist yet.

Commands:
  mcp                An MCP server on standard input and output: a headless Yuna of its own,
                     whose tools create seats, each with its own outputs, Wayland socket,
                     pointer and keyboard, start programs on them, list windows, give
                     input, and read the clock. Logs to standard error; stops when its
                     input closes, or on SIGTERM or SIGINT, and ends its programs then.

Options:
      --headless     Run without a screen, on virtual outputs drawn offscreen. Prints
                     WAYLAND_DISPLAY=<socket> on standard output, logs to standard error,
                     and stops on SIGTERM or SIGINT.
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
  -h, --help         Print this help
  -V, --version      Print the version

Environment:
  XDG_RUNTIME_DIR    The folder the Wayland socket is created in; required.
  YUNA_LOG           What is logged: error, warn, info (the default), debug or trace.
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
}

/// Reads the arguments that follow the program's name. They are `OsString`s, so that an argument
/// that is not UTF-8 is refused like any other unknown one, never a panic.
fn read_request(arguments: impl IntoIterator<Item = OsString>) -> Request {
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    match arguments.as_slice() {
        [] => Request::Session,
        [one] if one == "-h" || one == "--help" => Request::Help,
        [one] if one == "-V" || one == "--version" => Request::Version,
        [first, rest @ ..] if first == "mcp" => read_headless(rest, true),
        _ => read_headless(&arguments, false),
    }
}

/// Reads `--headless`'s options, or, for `mcp`, its own: no `--headless`, which it is already,
/// and `--manual-clock`.
fn read_headless(arguments: &[OsString], mcp: bool) -> Request {
    let mut headless = mcp;
    let mut gpu = false;
    let mut manual_clock = false;
    let mut animations = false;
    let mut reduced_motion = false;
    let mut outputs = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--headless") if !headless => headless = true,
            Some("--gpu") if !gpu => gpu = true,
            Some("--animations") if !animations && !reduced_motion => animations = true,
            Some("--reduced-motion") if !animations && !reduced_motion => reduced_motion = true,
            Some("--animations" | "--reduced-motion") if animations != reduced_motion => {
                return Request::Invalid(
                    "--animations and --reduced-motion are one choice: give one, once".to_owned(),
                );
            }
            Some("--manual-clock") if mcp && !manual_clock => manual_clock = true,
            Some("--headless") if mcp => {
                return Request::Invalid("mcp is headless already: drop --headless".to_owned());
            }
            Some("--manual-clock") if !mcp => {
                return Request::Invalid("--manual-clock is mcp's".to_owned());
            }
            Some(flag @ ("--headless" | "--gpu" | "--animations" | "--manual-clock")) => {
                return Request::Invalid(format!("{flag} is given twice"));
            }
            Some("--output") => match arguments.next().map(|mode| read_output(mode)) {
                Some(Ok(mode)) => outputs.push(mode),
                Some(Err(reason)) => return Request::Invalid(reason),
                None => {
                    return Request::Invalid(
                        "--output needs a mode, such as 1920x1080@60,1".to_owned(),
                    );
                }
            },
            Some(flag @ ("-h" | "--help" | "-V" | "--version")) => {
                return Request::Invalid(format!("{flag} takes no other argument"));
            }
            // Quoted, so that an empty argument shows, and escaped where it is not UTF-8.
            _ => return Request::Invalid(format!("unknown argument {argument:?}")),
        }
    }
    if !headless {
        return Request::Invalid(
            "--gpu, --animations, --reduced-motion and --output need --headless".to_owned(),
        );
    }
    if mcp && outputs.len() > 1 {
        return Request::Invalid(
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
    };
    if mcp {
        Request::Mcp(headless)
    } else {
        Request::Headless(headless)
    }
}

/// Reads `WIDTHxHEIGHT@RATE,SCALE[,vrr=MIN][,hdr]`, the two items in any order, each once, and
/// checks it as an output's mode (ADR-0049).
fn read_output(text: &OsStr) -> Result<OutputMode, String> {
    let bad = || {
        format!(
            "--output {text:?}: expected WIDTHxHEIGHT@RATE,SCALE[,vrr=MIN][,hdr], such as \
             1920x1080@60,1"
        )
    };
    let text_str = text.to_str().ok_or_else(bad)?;
    let refused = |reason: String| format!("--output {text_str}: {reason}");
    let (size, rest) = text_str.split_once('@').ok_or_else(bad)?;
    let (width, height) = size.split_once('x').ok_or_else(bad)?;
    let mut items = rest.split(',');
    let (Some(rate), Some(scale)) = (items.next(), items.next()) else {
        return Err(bad());
    };
    let width: u32 = width.parse().map_err(|_| bad())?;
    let height: u32 = height.parse().map_err(|_| bad())?;
    let rate: f64 = rate.parse().map_err(|_| bad())?;
    let scale: f64 = scale.parse().map_err(|_| bad())?;
    let mut mode =
        OutputMode::new(width, height, rate, scale).map_err(|r| refused(r.to_string()))?;
    let mut vrr_min = None;
    let mut hdr = false;
    for item in items {
        if item == "hdr" {
            if hdr {
                return Err(refused("hdr is given twice".to_owned()));
            }
            hdr = true;
        } else if let Some(min) = item.strip_prefix("vrr=") {
            if vrr_min.is_some() {
                return Err(refused("vrr is given twice".to_owned()));
            }
            let min: f64 = min
                .parse()
                .map_err(|_| refused(format!("vrr={min} is not a rate in Hz")))?;
            vrr_min = Some(min);
        } else {
            return Err(refused(format!(
                "unknown item {item:?}: expected vrr=MIN or hdr"
            )));
        }
    }
    if let Some(min) = vrr_min {
        mode = mode.with_vrr(min).map_err(|r| refused(r.to_string()))?;
    }
    if hdr {
        mode = mode.with_hdr(PG32UCDM);
    }
    Ok(mode)
}

fn main() -> ExitCode {
    match read_request(std::env::args_os().skip(1)) {
        Request::Help => print(HELP),
        Request::Version => print(&format!("yuna {}\n", env!("CARGO_PKG_VERSION"))),
        Request::Session => {
            print_error(
                "yuna: the desktop session does not exist yet (Stage 2); run `yuna --headless`\n",
            );
            ExitCode::FAILURE
        }
        Request::Headless(headless) => run_headless(headless),
        Request::Mcp(headless) => run_mcp(headless),
        Request::Invalid(reason) => usage_error(&reason),
    }
}

/// 2, as most command-line programs, for a usage error.
fn usage_error(reason: &str) -> ExitCode {
    print_error(&format!("yuna: {reason}\n\n{HELP}"));
    ExitCode::from(2)
}

fn run_headless(headless: Headless) -> ExitCode {
    start_logging();
    // Before the compositor starts, so that a SIGTERM during its start-up is kept in the pipe and
    // stops it as soon as its loop runs, rather than killing it half started.
    let stop = match stop_signal::install() {
        Ok(stop) => stop,
        Err(error) => {
            print_error(&format!("yuna: cannot handle SIGTERM: {error}\n"));
            return ExitCode::FAILURE;
        }
    };
    let mut compositor = match start_compositor(headless) {
        Ok(compositor) => compositor,
        Err(exit) => return exit,
    };
    if let Some(name) = compositor.socket_name() {
        // A reader that left does not stop the compositor: its clients may know the socket.
        let _ = print(&format!("WAYLAND_DISPLAY={}\n", name.to_string_lossy()));
    }
    match compositor.run_until(stop) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "the compositor stopped");
            ExitCode::FAILURE
        }
    }
}

/// Starts the compositor `yuna --headless` asks for, its outputs on the owner's seat, listening on
/// a socket; reports why it cannot.
fn start_compositor(headless: Headless) -> Result<Compositor, ExitCode> {
    let config = Config {
        device: device_of(&headless),
        animations: animations_of(&headless),
        outputs: headless.outputs,
        clock: ClockMode::Real,
        listen: true,
        announce_owner_seat: true,
    };
    start(config)
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
/// stays under it, to be reaped and ended.
fn run_mcp(headless: Headless) -> ExitCode {
    start_logging();
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
    // agents create (ADR-0035, ADR-0037).
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
    };
    let compositor = match start(config) {
        Ok(compositor) => compositor,
        Err(exit) => return exit,
    };
    let server = match Server::new(compositor, input, output, Some(children)) {
        Ok(mut server) => {
            server.set_seat_output(seat_output);
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
fn print(text: &str) -> ExitCode {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
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
            })
        );
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
