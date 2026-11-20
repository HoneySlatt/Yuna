//! A seat's X display: its number, its two listening sockets, its lock file and its cookie
//! ([ADR-0067](../../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)).
//!
//! The number is the first free one from 0, below [`MAX_DISPLAYS`]. Its abstract socket,
//! `@/tmp/.X11-unix/X<n>`, is bound first: that is the reservation, which the kernel frees when
//! Yuna dies, and a name already bound sends Yuna to the next number. Then its lock,
//! `/tmp/.X<n>-lock`, which holds Yuna's process id: a lock of a process that no longer exists is
//! replaced, a live one sends Yuna to the next number. Only then its file socket,
//! `/tmp/.X11-unix/X<n>`, replacing a stale one: libxcb tries the abstract socket first, so a
//! `/tmp` that cannot hold the lock or the file socket leaves the display served by the abstract
//! one alone, which is said once. Both sockets listen with a backlog of [`BACKLOG`], so that X11
//! programs started together wait for Xwayland rather than be refused.
//!
//! The cookie is 16 bytes of `/dev/urandom`, in an Xauthority file of the seat's own: one
//! `FamilyWild` entry for the display, `MIT-MAGIC-COOKIE-1`, mode 0600, written to a temporary
//! file then renamed, in a folder of the seat's own, mode 0700. Everything is removed when the
//! [`Display`] drops: with its seat, or with the compositor.

use std::ffi::OsString;
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::mem::DropGuard;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::io::Errno;
use rustix::net::{
    AddressFamily, SocketAddrUnix, SocketFlags, SocketType, bind, listen, socket_with,
};
use rustix::process::Pid;

/// How many display numbers Yuna tries, from 0: a thousand. Each test of Yuna's, and each seat
/// of each `yuna mcp`, holds one while it lives, so that tests running many compositors at once
/// take a few dozen at most; a number taken costs one `bind` to skip, a search through all of them
/// about a millisecond.
pub const MAX_DISPLAYS: u32 = 1000;

/// How many connections each socket holds while Xwayland starts: a burst of X11 programs
/// started at once waits for it rather than be refused. The kernel caps it at `somaxconn`.
pub(crate) const BACKLOG: i32 = 128;

/// The folder of X11's file sockets, the same on every system.
const SOCKET_FOLDER: &str = "/tmp/.X11-unix";

/// The authorisation protocol of the cookie: X11's only one in common use, which Xwayland and
/// every client library speak.
pub(crate) const COOKIE_NAME: &[u8] = b"MIT-MAGIC-COOKIE-1";

/// An Xauthority entry's family that matches every address: the entry is for the display number
/// alone, whatever host name a client looks for.
const FAMILY_WILD: u16 = 0xFFFF;

/// The name of the Xauthority file in its seat's folder.
const XAUTHORITY: &str = "Xauthority";

/// A seat's X display, held from its reservation until it drops.
#[derive(Debug)]
pub(crate) struct Display {
    pub number: u32,
    /// The abstract socket, whose name is the reservation.
    pub abstract_socket: OwnedFd,
    /// The file socket in `/tmp/.X11-unix`, when it could be made.
    pub file_socket: Option<FileSocket>,
    /// The lock, when `/tmp` could hold it.
    _lock: Option<Lock>,
    pub xauthority: Xauthority,
}

/// Why a display could not be reserved.
#[derive(Debug)]
pub(crate) enum ReserveError {
    /// Every number below [`MAX_DISPLAYS`] is taken.
    NoFreeNumber,
    /// A socket, the cookie or the Xauthority file could not be made.
    Io(io::Error),
}

impl std::fmt::Display for ReserveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReserveError::NoFreeNumber => {
                write!(f, "every X display number below {MAX_DISPLAYS} is taken")
            }
            ReserveError::Io(error) => error.fmt(f),
        }
    }
}

impl From<io::Error> for ReserveError {
    fn from(error: io::Error) -> ReserveError {
        ReserveError::Io(error)
    }
}

impl From<Errno> for ReserveError {
    fn from(error: Errno) -> ReserveError {
        ReserveError::Io(error.into())
    }
}

/// What [`Display::reserve`] could not make, which a display goes without.
#[derive(Debug, Default)]
pub(crate) struct Shortfall {
    /// Why `/tmp` holds neither the lock nor the file socket, when it does not.
    pub tmp: Option<io::Error>,
}

impl Display {
    /// Reserves the first free display number, its sockets and its lock, and writes a new cookie
    /// into `folder`'s Xauthority file, `folder` made with mode 0700. Returns it, with what
    /// `/tmp` could not hold.
    pub(crate) fn reserve(folder: &Path) -> Result<(Display, Shortfall), ReserveError> {
        let mut cookie = [0; 16];
        File::open("/dev/urandom")?.read_exact(&mut cookie)?;
        let mut shortfall = Shortfall::default();
        for number in 0..MAX_DISPLAYS {
            let Some(abstract_socket) = bind_abstract(number)? else {
                continue;
            };
            let lock = match take_lock(number) {
                Ok(Some(lock)) => Some(lock),
                Ok(None) => continue,
                Err(error) => {
                    shortfall.tmp = Some(error);
                    None
                }
            };
            // A file socket is made only under a lock of Yuna's: never in another server's way.
            let file_socket = match &lock {
                Some(_) => match FileSocket::bind(number) {
                    Ok(socket) => Some(socket),
                    Err(error) => {
                        shortfall.tmp = Some(error);
                        None
                    }
                },
                None => None,
            };
            let xauthority = Xauthority::write(folder, number, &cookie)?;
            let display = Display {
                number,
                abstract_socket,
                file_socket,
                _lock: lock,
                xauthority,
            };
            return Ok((display, shortfall));
        }
        Err(ReserveError::NoFreeNumber)
    }

    /// The listening sockets Xwayland takes: the abstract one, then the file one.
    pub(crate) fn sockets(&self) -> Vec<&OwnedFd> {
        let mut sockets = vec![&self.abstract_socket];
        if let Some(file) = &self.file_socket {
            sockets.push(&file.socket);
        }
        sockets
    }

    /// Accepts and closes every connection waiting on its sockets, without waiting for one. A
    /// connection no Xwayland could serve stays in the kernel's queue even after its client gave
    /// up, and would start Xwayland again at each watch, for ever.
    pub(crate) fn refuse_waiting(&self) {
        for socket in self.sockets() {
            refuse_waiting_on(socket);
        }
    }
}

/// Accepts and closes the connections waiting on the listening `socket`, at most a full queue:
/// what comes meanwhile is for the next watch.
fn refuse_waiting_on(socket: &OwnedFd) {
    for _ in 0..BACKLOG {
        let mut waiting = [PollFd::new(socket, PollFlags::IN)];
        let ready = rustix::event::poll(&mut waiting, Some(&Timespec::default()));
        if !matches!(ready, Ok(n) if n > 0) {
            break;
        }
        if rustix::net::accept_with(socket, SocketFlags::CLOEXEC).is_err() {
            break;
        }
    }
}

/// The path both sockets of display `number` are named after.
fn socket_path(number: u32) -> String {
    format!("{SOCKET_FOLDER}/X{number}")
}

/// A listening Unix socket, close-on-exec: Xwayland gets it through `-listenfd`, which clears
/// the flag in its own process only.
fn listening_socket(address: &SocketAddrUnix) -> Result<OwnedFd, Errno> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )?;
    bind(&socket, address)?;
    listen(&socket, BACKLOG)?;
    Ok(socket)
}

/// Binds the abstract socket of display `number`; none when another process holds its name.
fn bind_abstract(number: u32) -> Result<Option<OwnedFd>, ReserveError> {
    let address = SocketAddrUnix::new_abstract_name(socket_path(number).as_bytes())?;
    match listening_socket(&address) {
        Ok(socket) => Ok(Some(socket)),
        Err(Errno::ADDRINUSE) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The file socket of a display, removed when it drops.
#[derive(Debug)]
pub(crate) struct FileSocket {
    pub socket: OwnedFd,
    path: PathBuf,
}

impl FileSocket {
    /// Binds `/tmp/.X11-unix/X<number>`, making the folder, sticky and writable by every user as
    /// X11 has it, if it is missing, and replacing a stale socket: the lock is Yuna's.
    fn bind(number: u32) -> io::Result<FileSocket> {
        match DirBuilder::new().create(SOCKET_FOLDER) {
            Ok(()) => fs::set_permissions(SOCKET_FOLDER, Permissions::from_mode(0o1777))?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        // Another user may have made the folder, or a link in its place, to catch X11's
        // clients: it is used only as X's own transport would take it.
        let folder = fs::symlink_metadata(SOCKET_FOLDER)?;
        if !is_trusted_socket_folder(&folder, rustix::process::getuid().as_raw()) {
            return Err(io::Error::other(format!(
                "{SOCKET_FOLDER} is not a folder of root or of this user, sticky"
            )));
        }
        let path = PathBuf::from(socket_path(number));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let socket = listening_socket(&SocketAddrUnix::new(&path)?)?;
        Ok(FileSocket { socket, path })
    }
}

/// Whether a folder of sockets, read without following a link, can hold this user's: a real
/// folder, the user's own, or root's and sticky, so that no other user can replace a socket in it.
fn is_trusted_socket_folder(folder: &fs::Metadata, uid: u32) -> bool {
    let sticky = folder.mode() & 0o1000 != 0;
    folder.file_type().is_dir() && (folder.uid() == uid || (folder.uid() == 0 && sticky))
}

impl Drop for FileSocket {
    fn drop(&mut self) {
        remove_quietly(&self.path);
    }
}

/// The lock of a display, `/tmp/.X<n>-lock`, which holds its server's process id as X servers
/// write it, ten characters and a newline: its path, removed when the guard drops.
type Lock = DropGuard<PathBuf, fn(PathBuf)>;

/// Takes the lock of display `number`; none when a live process holds it, or one that cannot be
/// read, which may be a server writing it. A lock of a process that no longer exists is replaced.
/// An error when `/tmp` cannot hold one.
fn take_lock(number: u32) -> io::Result<Option<Lock>> {
    let path = PathBuf::from(format!("/tmp/.X{number}-lock"));
    let pid = rustix::process::getpid().as_raw_nonzero().get();
    // Written whole beside it, then linked to its name, which fails if it exists: a reader
    // never finds it half written, as X servers write theirs.
    let mut written = OsString::from(format!("/tmp/.tX{number}-lock-"));
    written.push(pid.to_string());
    let written = PathBuf::from(written);
    remove_quietly(&written);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o444)
        .open(&written)?;
    let linked = file
        .write_all(format!("{pid:>10}\n").as_bytes())
        .and_then(|()| fs::hard_link(&written, &path));
    drop(file);
    let linked = match linked {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if lock_is_stale(&path) && fs::remove_file(&path).is_ok() {
                fs::hard_link(&written, &path).map(|()| true)
            } else {
                Ok(false)
            }
        }
        linked => linked.map(|()| true),
    };
    remove_quietly(&written);
    match linked {
        Ok(true) => Ok(Some(DropGuard::new(path, |path| remove_quietly(&path)))),
        // Taken by another server between the two tries.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Ok(false) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether the lock at `path` names a process that no longer exists. A lock that cannot be read
/// or does not name a process is not stale: it may be another server's, half written.
fn lock_is_stale(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    // Read as a positive number first: rustix's `Pid` takes no negative one.
    let Some(pid) = text
        .trim()
        .parse::<u32>()
        .ok()
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(Pid::from_raw)
    else {
        return false;
    };
    rustix::process::test_kill_process(pid) == Err(Errno::SRCH)
}

/// A seat's Xauthority file and the folder that holds it, removed when it drops.
#[derive(Debug)]
pub(crate) struct Xauthority {
    pub path: PathBuf,
    folder: PathBuf,
}

impl Xauthority {
    /// Makes `folder`, the user's alone, and writes the entry of display `number` there,
    /// through a temporary file renamed over it. A folder already there is taken if it is a
    /// folder of the user's, a stale one of a Yuna that died.
    fn write(folder: &Path, number: u32, cookie: &[u8; 16]) -> io::Result<Xauthority> {
        match DirBuilder::new().mode(0o700).create(folder) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(folder)?;
                if !metadata.is_dir() || metadata.uid() != rustix::process::getuid().as_raw() {
                    return Err(error);
                }
                fs::set_permissions(folder, Permissions::from_mode(0o700))?;
            }
            Err(error) => return Err(error),
        }
        let xauthority = Xauthority {
            path: folder.join(XAUTHORITY),
            folder: folder.to_owned(),
        };
        let temporary = folder.join(format!("{XAUTHORITY}.new"));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&xauthority_entry(number, cookie))?;
        file.sync_all()?;
        fs::rename(&temporary, &xauthority.path)?;
        Ok(xauthority)
    }
}

impl Drop for Xauthority {
    fn drop(&mut self) {
        remove_quietly(&self.path);
        // Only if it is empty: nothing else of the user's goes with it.
        if let Err(error) = fs::remove_dir(&self.folder)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::debug!(%error, folder = ?self.folder, "an X11 folder is left");
        }
    }
}

/// One Xauthority entry: `FamilyWild`, no address, the display's number in decimal, the
/// cookie's protocol and its bytes, each field after its length, all big-endian as libXau reads
/// them.
pub(crate) fn xauthority_entry(number: u32, cookie: &[u8; 16]) -> Vec<u8> {
    let number = number.to_string();
    let mut entry = Vec::new();
    entry.extend_from_slice(&FAMILY_WILD.to_be_bytes());
    for field in [&[][..], number.as_bytes(), COOKIE_NAME, cookie] {
        // Every field is far below 65,536 bytes.
        let length = u16::try_from(field.len()).unwrap_or(u16::MAX);
        entry.extend_from_slice(&length.to_be_bytes());
        entry.extend_from_slice(field);
    }
    entry
}

/// Removes the files of every X display this process holds, `/tmp/.X11-unix/X<n>` then
/// `/tmp/.X<n>-lock`, read from the locks that name this process: what the displays' drops
/// would remove, for a Yuna that stops after a panic of its state and runs no drop
/// ([ADR-0080](../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)). It
/// reads no state and logs nothing: an error leaves the file, which the next server takes as
/// stale once this process is gone.
pub fn remove_displays_after_panic() {
    let Ok(entries) = fs::read_dir("/tmp") else {
        return;
    };
    let own = rustix::process::getpid().as_raw_nonzero().get().to_string();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(number) = name
            .to_str()
            .and_then(|name| name.strip_prefix(".X")?.strip_suffix("-lock"))
            .and_then(|number| number.parse::<u32>().ok())
        else {
            continue;
        };
        let lock = entry.path();
        if fs::read_to_string(&lock).is_ok_and(|text| text.trim() == own) {
            // The socket first: while the lock stays, no other server takes the number.
            let _ = fs::remove_file(socket_path(number));
            let _ = fs::remove_file(&lock);
        }
    }
}

/// Removes a file Yuna made; one already gone is no error, any other is logged.
fn remove_quietly(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::debug!(%error, ?path, "an X11 file could not be removed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_connections_are_refused_and_the_socket_rests() {
        let name = format!("yuna-refuse-test-{}", std::process::id());
        let address = SocketAddrUnix::new_abstract_name(name.as_bytes()).expect("an address");
        let socket = listening_socket(&address).expect("a listening socket");
        let clients: Vec<OwnedFd> = (0..3)
            .map(|_| {
                let client = socket_with(
                    AddressFamily::UNIX,
                    SocketType::STREAM,
                    SocketFlags::CLOEXEC,
                    None,
                )
                .expect("a socket");
                rustix::net::connect(&client, &address).expect("a connection");
                client
            })
            .collect();
        refuse_waiting_on(&socket);
        let mut waiting = [PollFd::new(&socket, PollFlags::IN)];
        let ready = rustix::event::poll(&mut waiting, Some(&Timespec::default()));
        assert_eq!(ready.ok(), Some(0), "a connection still waits");
        for client in clients {
            let mut byte = [0];
            let read = rustix::io::read(&client, &mut byte).expect("an end");
            assert_eq!(read, 0, "the connection is closed");
        }
        // With nothing waiting, it returns at once.
        refuse_waiting_on(&socket);
    }

    #[test]
    fn a_socket_folder_is_trusted_only_as_x_takes_it() {
        let base = std::env::temp_dir().join(format!("yuna-socket-folder-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("a folder");
        let uid = rustix::process::getuid().as_raw();
        let own = fs::symlink_metadata(&base).expect("its metadata");
        assert!(is_trusted_socket_folder(&own, uid));
        // The same folder, as another user's: refused.
        assert!(!is_trusted_socket_folder(&own, uid.wrapping_add(1)));
        let link = base.join("link");
        std::os::unix::fs::symlink(&base, &link).expect("a link");
        let linked = fs::symlink_metadata(&link).expect("its metadata");
        assert!(!is_trusted_socket_folder(&linked, uid));
        fs::remove_dir_all(&base).expect("the folder goes");
    }

    #[test]
    fn the_xauthority_entry_is_family_wild_for_the_display() {
        let cookie: [u8; 16] = std::array::from_fn(|i| i as u8);
        let mut expected = vec![0xFF, 0xFF, 0, 0, 0, 2, b'4', b'2', 0, 18];
        expected.extend_from_slice(b"MIT-MAGIC-COOKIE-1");
        expected.extend_from_slice(&[0, 16]);
        expected.extend_from_slice(&cookie);
        assert_eq!(xauthority_entry(42, &cookie), expected);
    }

    #[test]
    fn a_lock_naming_no_process_is_stale_and_others_are_not() {
        let folder = std::env::temp_dir().join(format!("yuna-lock-test-{}", std::process::id()));
        fs::create_dir_all(&folder).expect("a folder");
        let lock = folder.join("lock");
        let stale = |text: &str| {
            fs::write(&lock, text).expect("a lock");
            lock_is_stale(&lock)
        };
        // Above Linux's largest process id, 2^22: no process has it.
        assert!(stale(&format!("{:>10}\n", 1 << 23)));
        assert!(!stale(&format!("{:>10}\n", std::process::id())));
        assert!(!stale(""));
        assert!(!stale("not a pid\n"));
        assert!(!stale("-5\n"));
        fs::remove_dir_all(&folder).expect("the folder goes");
    }
}
