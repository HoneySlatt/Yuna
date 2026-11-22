//! A seat's accessibility bus: its `dbus-daemon`, started for the seat's first program started
//! with accessibility, and Yuna's connection to it, as the registry.
//!
//! The bus's folder is `seat-<id>.a11y` in the programs' folder: its configuration
//! (`bus.conf`), its socket (`bus`) and its log (`bus.log`), where the daemon's output goes,
//! never to Yuna's standard output, through a pipe Yuna reads, bounded as a program's log is and
//! made anew for each daemon. Its configuration ([`configuration`]) is at-spi's own
//! (`bus/accessibility.conf.in` of at-spi2-core 2.62.0) with its limits lowered and no service
//! folder: no program is started by the bus, and nothing is read again at rest.
//!
//! Its stages, all on Yuna's loop, none waiting:
//!
//! 1. **Starting**: `dbus-daemon --nofork --config-file=bus.conf --print-address=<fd>`, the one
//!    `crate::dbus::find_daemon` finds, with the programs' environment of Yuna's variables (never
//!    Yuna's own whole, never the user's session bus). Its address comes on the pipe once it
//!    listens.
//! 2. **Connecting**: Yuna connects to the address it printed, authenticates, says `Hello`,
//!    asks for `org.a11y.atspi.Registry` and for the bus's `NameOwnerChanged` signals.
//! 3. **Ready** once the name is Yuna's: programs may be given the address.
//!
//! The whole start has [`START_LIMIT`]; past it, or when a stage fails, the daemon is killed.
//! Its exit is watched through its pidfd, whatever the stage: a bus that ends is forgotten, its
//! socket removed, so that a GTK 4 program, which connects without a time limit, never finds a
//! socket with no daemon behind it.

#![expect(
    unsafe_code,
    reason = "the bus's program given the descriptor it writes its address to"
)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use rustix::pipe::{PipeFlags, pipe_with};
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open};

use crate::dbus::MAX_MESSAGE;
use crate::process::{give_descriptors, keep_descriptors_from_the_program};

/// The longest a bus may take from its daemon's start to Yuna owning the registry's name: a
/// daemon on an idle machine takes a few milliseconds.
pub(crate) const START_LIMIT: Duration = Duration::from_secs(5);

/// How long the bus waits for a reply before it answers a call with an error itself: far under
/// at-spi's 300 s, and over a walk's deadline for each application
/// ([`super::APP_DEADLINE`]), so that Yuna's deadline comes first.
pub(crate) const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The most connections the bus accepts: a Yuna runs 64 programs at most, Yuna's own beside.
pub(crate) const MAX_CONNECTIONS: usize = 256;

/// The most bytes the bus queues to or from one connection: twice its largest message, what
/// Yuna itself lets wait to be written (`crate::dbus::MAX_UNSENT`, 8 MiB). A sender past it is
/// refused, its call failed, rather than the daemon growing.
pub(crate) const MAX_QUEUED_BYTES: usize = 2 * MAX_MESSAGE;

/// The most calls one connection may wait on: Yuna's walks have 512 in flight at most
/// ([`super::MAX_IN_FLIGHT`]), beside one process asked for each of 128 applications.
pub(crate) const MAX_REPLIES: usize = 1024;

/// The program the bus runs.
pub(crate) const DAEMON: &str = "dbus-daemon";

/// The files of a seat's bus, in its folder.
#[derive(Debug, Clone)]
pub(crate) struct BusFiles {
    pub folder: PathBuf,
    pub configuration: PathBuf,
    pub socket: PathBuf,
    pub log: PathBuf,
}

impl BusFiles {
    /// The files of the bus whose folder is `folder`.
    pub(crate) fn in_folder(folder: PathBuf) -> BusFiles {
        BusFiles {
            configuration: folder.join("bus.conf"),
            socket: folder.join("bus"),
            log: folder.join("bus.log"),
            folder,
        }
    }
}

/// The longest path a Unix socket's address holds: `sun_path`'s 108 bytes, its last a NUL.
pub(crate) const MAX_SOCKET_PATH: usize = 107;

/// Refuses a socket path a Unix socket cannot hold, before a daemon is started that could not
/// listen on it and would exit at once, and that no program could reach either.
pub(crate) fn check_socket_path(socket: &Path) -> Result<(), String> {
    let length = socket.as_os_str().len();
    if length > MAX_SOCKET_PATH {
        return Err(format!(
            "the accessibility bus's socket, {}, is {length} bytes long, past the {MAX_SOCKET_PATH} \
             a Unix socket's address holds: give Yuna a shorter XDG_RUNTIME_DIR",
            socket.display()
        ));
    }
    Ok(())
}

/// The bus's configuration, listening at `address`, which [`crate::dbus::unix_address`] wrote
/// and which needs no escaping in XML: its bytes are letters, digits, `-_/.\*` and `%`.
pub(crate) fn configuration(address: &str) -> String {
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>accessibility</type>
  <listen>{address}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_type="method_call"/>
    <allow receive_type="method_return"/>
    <allow receive_type="error"/>
    <allow receive_type="signal"/>
    <allow own="*"/>
  </policy>
  <limit name="max_message_size">{MAX_MESSAGE}</limit>
  <limit name="reply_timeout">{reply_timeout}</limit>
  <limit name="max_incoming_bytes">{MAX_QUEUED_BYTES}</limit>
  <limit name="max_outgoing_bytes">{MAX_QUEUED_BYTES}</limit>
  <limit name="max_completed_connections">{MAX_CONNECTIONS}</limit>
  <limit name="max_incomplete_connections">64</limit>
  <limit name="max_connections_per_user">{MAX_CONNECTIONS}</limit>
  <limit name="auth_timeout">5000</limit>
  <limit name="max_names_per_connection">64</limit>
  <limit name="max_match_rules_per_connection">512</limit>
  <limit name="max_replies_per_connection">{MAX_REPLIES}</limit>
</busconfig>
"#,
        reply_timeout = REPLY_TIMEOUT.as_millis(),
    )
}

/// A daemon started, and the read end of the pipe its address comes on.
#[derive(Debug)]
pub(crate) struct Started {
    pub pid: Pid,
    /// Readable once it has ended; reaps it.
    pub pidfd: OwnedFd,
    /// Non-blocking: the address, then a new line.
    pub address: OwnedFd,
}

/// Starts `program`, dbus-daemon, for the bus of `files` listening at `address`, with
/// `environment` alone: its folder and configuration made, a stale socket removed, its output
/// written to what `log` makes of its log's path, a pipe's write end.
pub(crate) fn start(
    program: &Path,
    files: &BusFiles,
    address: &str,
    environment: BTreeMap<OsString, OsString>,
    log: impl FnOnce(&Path) -> io::Result<OwnedFd>,
) -> io::Result<Started> {
    match fs::DirBuilder::new().mode(0o700).create(&files.folder) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    fs::write(&files.configuration, configuration(address))?;
    match fs::remove_file(&files.socket) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let output = log(&files.log)?;
    let errors = output.try_clone()?;
    let (address_read, address_write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
    let mut command = Command::new(program);
    command
        .arg("--nofork")
        .arg(config_argument(&files.configuration))
        .arg(format!("--print-address={}", address_write.as_raw_fd()))
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::from(output))
        .stderr(Stdio::from(errors))
        // Its own group, as each program has, so that a signal to Yuna's group misses it.
        .process_group(0);
    keep_descriptors_from_the_program(&mut command);
    // SAFETY: `address_write` owns the number and is dropped only after `spawn` returns, below.
    unsafe { give_descriptors(&mut command, vec![address_write.as_raw_fd()]) };
    let child = command.spawn()?;
    // Its end closes here: the pipe ends when the daemon's does.
    drop((address_write, command));
    let pid = Pid::from_child(&child);
    // Reaped through its pidfd, or `yuna mcp`'s reaping of every child, not `Child::wait`.
    drop(child);
    let pidfd = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(pidfd) => pidfd,
        Err(error) => {
            // Without a pidfd it would be neither watched nor ended: it goes at once.
            let _ = rustix::process::kill_process(pid, Signal::KILL);
            let _ = rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::empty());
            return Err(error.into());
        }
    };
    Ok(Started {
        pid,
        pidfd,
        address: address_read,
    })
}

/// `--config-file=<path>`, the path's bytes whole.
fn config_argument(path: &Path) -> OsString {
    let mut argument = OsString::from("--config-file=");
    argument.push(path.as_os_str());
    argument
}

/// Reads what the daemon printed of its address on `pipe` into `read`: the address once its
/// line is whole; an error once the pipe ended without one, or a line over 4 KiB. The daemon
/// closes the pipe right after the line, so that its end often comes in the same read.
pub(crate) fn read_address(pipe: &OwnedFd, read: &mut Vec<u8>) -> Result<Option<String>, String> {
    let mut chunk = [0_u8; 512];
    loop {
        match rustix::io::read(pipe, &mut chunk) {
            Ok(0) if read.contains(&b'\n') => break,
            Ok(0) => return Err("dbus-daemon ended its address's pipe without one".to_owned()),
            Ok(count) => {
                read.extend_from_slice(&chunk[..count]);
                if read.len() > 4096 {
                    return Err("dbus-daemon printed an address over 4 KiB".to_owned());
                }
            }
            Err(rustix::io::Errno::AGAIN) => break,
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(format!("dbus-daemon's address cannot be read: {error}")),
        }
    }
    let Some(end) = read.iter().position(|&byte| byte == b'\n') else {
        return Ok(None);
    };
    String::from_utf8(read[..end].to_vec())
        .map(Some)
        .map_err(|_| "dbus-daemon printed an address that is not UTF-8".to_owned())
}

/// Whether the address the daemon printed, `printed`, is the one Yuna asked it to listen at:
/// the same transport and path, its `guid` and other keys aside.
pub(crate) fn is_address(printed: &str, asked: &str) -> bool {
    let path = |address: &str| -> Option<String> {
        let keys = address.strip_prefix("unix:")?;
        keys.split(',')
            .find_map(|key| key.strip_prefix("path="))
            .map(str::to_owned)
    };
    path(printed).is_some() && path(printed) == path(asked)
}

/// Removes the bus's socket, which a daemon killed leaves behind; one already gone is no failure.
pub(crate) fn remove_socket(files: &BusFiles) {
    match fs::remove_file(&files.socket) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, socket = ?files.socket, "an accessibility bus's socket is left")
        }
    }
}

/// Removes the bus's folder, its log with it, when its seat closes.
pub(crate) fn remove_folder(files: &BusFiles) {
    match fs::remove_dir_all(&files.folder) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, folder = ?files.folder, "an accessibility bus's folder is left")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_path_past_a_unix_sockets_address_is_refused() {
        let fits = format!("/{}", "a".repeat(MAX_SOCKET_PATH - 1));
        assert!(check_socket_path(Path::new(&fits)).is_ok());
        let past = format!("/{}", "a".repeat(MAX_SOCKET_PATH));
        let refused = check_socket_path(Path::new(&past)).expect_err("too long");
        assert!(
            refused.contains("108") || refused.contains("107"),
            "{refused}"
        );
    }

    #[test]
    fn the_configuration_lowers_at_spis_limits_and_starts_no_service() {
        let text = configuration("unix:path=/run/user/1000/yuna-mcp-7/seat-2.a11y/bus");
        assert!(text.contains("<type>accessibility</type>"));
        assert!(
            text.contains("<listen>unix:path=/run/user/1000/yuna-mcp-7/seat-2.a11y/bus</listen>")
        );
        assert!(text.contains("<auth>EXTERNAL</auth>"));
        // 4 MiB and 5 s, against at-spi's 1,000,000,000 bytes and 300,000 ms.
        assert!(text.contains(r#"<limit name="max_message_size">4194304</limit>"#));
        assert!(text.contains(r#"<limit name="reply_timeout">5000</limit>"#));
        // 8 MiB queued to or from a connection, against at-spi's 1,000,000,000 bytes.
        assert!(text.contains(r#"<limit name="max_incoming_bytes">8388608</limit>"#));
        assert!(text.contains(r#"<limit name="max_outgoing_bytes">8388608</limit>"#));
        assert!(text.contains(r#"<limit name="max_replies_per_connection">1024</limit>"#));
        // No program is started by the bus, so no folder of services is read.
        assert!(!text.contains("servicedir"));
    }

    #[test]
    fn the_address_printed_is_the_one_asked() {
        let asked = "unix:path=/run/user/1000/seat-2.a11y/bus";
        assert!(is_address(
            "unix:path=/run/user/1000/seat-2.a11y/bus,guid=0123",
            asked
        ));
        assert!(is_address(
            "unix:guid=0123,path=/run/user/1000/seat-2.a11y/bus",
            asked
        ));
        assert!(!is_address("unix:path=/elsewhere,guid=0123", asked));
        assert!(!is_address(
            "unix:abstract=/run/user/1000/seat-2.a11y/bus",
            asked
        ));
        assert!(!is_address("tcp:host=localhost", asked));
    }

    #[test]
    fn an_address_line_is_read_whole_and_its_end_is_an_error() {
        let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK).expect("a pipe");
        let mut buffer = Vec::new();
        assert_eq!(read_address(&read, &mut buffer), Ok(None));
        rustix::io::write(&write, b"unix:path=/a,guid=").expect("a write");
        assert_eq!(read_address(&read, &mut buffer), Ok(None));
        rustix::io::write(&write, b"01\n").expect("a write");
        assert_eq!(
            read_address(&read, &mut buffer),
            Ok(Some("unix:path=/a,guid=01".to_owned()))
        );
        drop(write);
        let mut empty = Vec::new();
        assert!(read_address(&read, &mut empty).is_err());

        // The line and the pipe's end at once, as dbus-daemon writes them.
        let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK).expect("a pipe");
        rustix::io::write(&write, b"unix:path=/b,guid=02\n").expect("a write");
        drop(write);
        let mut buffer = Vec::new();
        assert_eq!(
            read_address(&read, &mut buffer),
            Ok(Some("unix:path=/b,guid=02".to_owned()))
        );
    }
}
