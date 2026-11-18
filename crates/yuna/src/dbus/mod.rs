//! Yuna's own D-Bus client: what the accessibility bus of an agent's seat needs, and nothing more
//! (`mcp/accessibility/`).
//!
//! - `message.rs`: a message, its header Yuna's own, its header and body encoded through zbus's
//!   `wire` module, the D-Bus wire format alone: no connection, runtime or executor of zbus's.
//! - `names.rs`: the specification's rules for bus names, interfaces, members and error names,
//!   which the encoder checks.
//! - `connection.rs`: a connection on a non-blocking Unix socket, SASL's EXTERNAL authentication,
//!   `Hello`, then calls, replies, errors and signals both ways. Its owner watches its socket on
//!   its event loop: nothing here waits or runs a thread.
//! - [`find_daemon`]: which `dbus-daemon` runs a bus: the one [`DAEMON_VARIABLE`] names, which
//!   Yuna's package and its development shell set to nixpkgs' own, else the first on `PATH`.
//!
//! Promises: bad input from the bus, truncated, misaligned, too long or of a wrong signature, is
//! refused with an error, never a panic, and leaves the connection following its stream when the
//! frame was whole. Yuna never sends what a bus disconnects it for: a message whose header
//! breaks the specification's rules, a name or a path a program gave included, is refused by
//! [`Message::encode`] with an error, and the connection goes on. Bounds: a message is 4 MiB at
//! most ([`MAX_MESSAGE`]), 8 MiB wait to be written at most ([`connection::MAX_UNSENT`]), 1 MiB
//! is read at a time ([`connection::READ_BUDGET`]). Yuna never sends or takes file descriptors.

mod connection;
mod message;
mod names;

#[cfg(test)]
mod tests;

pub(crate) use connection::{BUS_INTERFACE, BUS_NAME, BUS_PATH, Connection};
#[cfg(test)]
pub(crate) use message::NO_REPLY_EXPECTED;
pub(crate) use message::{Body, Kind, MAX_MESSAGE, Message};

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The error a bus gives a call of a method its object does not have.
pub(crate) const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";

/// The error of a call refused for its arguments.
pub(crate) const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";

/// The error of a call Yuna refuses, saying why.
pub(crate) const FAILED: &str = "org.freedesktop.DBus.Error.Failed";

/// The error of a call of something Yuna does not do.
pub(crate) const NOT_SUPPORTED: &str = "org.freedesktop.DBus.Error.NotSupported";

/// The address of a bus listening at `path`: `unix:path=` and the path, each byte outside the
/// specification's unescaped set written `%xx`, so that a path with a space, a comma or a
/// semicolon reaches the program whole.
pub(crate) fn unix_address(path: &Path) -> String {
    let mut address = String::from("unix:path=");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-_/.\\*".contains(&byte) {
            address.push(char::from(byte));
        } else {
            address.push_str(&format!("%{byte:02x}"));
        }
    }
    address
}

/// The variable that names the `dbus-daemon` Yuna runs, by its absolute path: Yuna's package and
/// its development shell set it to nixpkgs' own, so that the host's is never used.
pub(crate) const DAEMON_VARIABLE: &str = "YUNA_DBUS_DAEMON";

/// The `dbus-daemon` to run: the one [`DAEMON_VARIABLE`] names, which must be an absolute path to
/// an executable file; without the variable, the first on `PATH`. The error says what is wrong.
pub(crate) fn find_daemon() -> Result<PathBuf, String> {
    if let Some(named) = std::env::var_os(DAEMON_VARIABLE).filter(|named| !named.is_empty()) {
        let path = PathBuf::from(&named);
        let executable = path.is_absolute()
            && std::fs::metadata(&path)
                .is_ok_and(|file| file.is_file() && file.permissions().mode() & 0o111 != 0);
        return if executable {
            Ok(path)
        } else {
            Err(format!(
                "{DAEMON_VARIABLE} names {}, which is not an absolute path to an executable file",
                path.display()
            ))
        };
    }
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    crate::process::find_program("dbus-daemon", &search_path).ok_or_else(|| {
        format!(
            "accessibility needs dbus-daemon, which neither {DAEMON_VARIABLE} names nor Yuna's PATH \
             holds: Yuna's Nix package and development shell set {DAEMON_VARIABLE}"
        )
    })
}
