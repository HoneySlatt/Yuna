//! Yuna, a Wayland compositor for Linux.
//!
//! No backend exists yet: the program answers `--version` and `--help`, and refuses to start. The
//! work in progress is in `docs/PLAN.md`.

use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

const HELP: &str = "\
Yuna, a Wayland compositor for Linux.

Usage: yuna [--help | --version]

No backend exists yet: Yuna cannot start a session.

Options:
  -h, --help     Print this help
  -V, --version  Print the version
";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
enum Request {
    Help,
    Version,
    /// No argument: a session, which needs a backend that does not exist yet.
    Session,
    /// An argument Yuna does not know, or more than one.
    Invalid(String),
}

/// Reads the arguments that follow the program's name. They are `OsString`s, so that an argument
/// that is not UTF-8 is refused like any other unknown one, never a panic.
fn read_request(arguments: impl IntoIterator<Item = OsString>) -> Request {
    let mut arguments = arguments.into_iter();
    let Some(first) = arguments.next() else {
        return Request::Session;
    };
    if arguments.next().is_some() {
        return Request::Invalid("expected at most one argument".to_owned());
    }
    match first.to_str() {
        Some("-h" | "--help") => Request::Help,
        Some("-V" | "--version") => Request::Version,
        // Quoted, so that an empty argument shows, and escaped where it is not UTF-8.
        _ => Request::Invalid(format!("unknown argument {first:?}")),
    }
}

fn main() -> ExitCode {
    match read_request(std::env::args_os().skip(1)) {
        Request::Help => print(HELP),
        Request::Version => print(&format!("yuna {}\n", env!("CARGO_PKG_VERSION"))),
        Request::Session => {
            print_error("yuna: no backend exists yet; see docs/PLAN.md\n");
            ExitCode::FAILURE
        }
        Request::Invalid(reason) => {
            print_error(&format!("yuna: {reason}\n\n{HELP}"));
            // 2, as most command-line programs, for a usage error.
            ExitCode::from(2)
        }
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
    }

    #[test]
    fn an_argument_that_is_not_utf8_is_refused_without_a_panic() {
        let not_utf8 = OsString::from_vec(vec![0x66, 0x6f, 0x80]);
        assert!(matches!(read_request([not_utf8]), Request::Invalid(_)));
    }
}
