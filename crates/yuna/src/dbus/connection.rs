//! A connection to a bus, Yuna's own: a non-blocking Unix socket, SASL's EXTERNAL
//! authentication, `Hello`, then messages both ways.
//!
//! It never waits. Its owner watches [`Connection::fd`] on its event loop, readable always and
//! writable while [`Connection::wants_write`]; then calls [`Connection::read`], which gives the
//! whole messages come, and [`Connection::flush`]. A message sent before the bus has answered
//! `Hello` waits in order behind it: the bus serves a client's messages in the order they come,
//! `Hello` first.
//!
//! Bounds: a message read or sent is [`MAX_MESSAGE`](super::MAX_MESSAGE) at most; at most [`MAX_UNSENT`] bytes wait
//! to be written, past which [`Connection::send`] refuses; [`Connection::read`] reads
//! [`READ_BUDGET`] at most at a time, so that one bus never holds the loop. An authentication
//! line is [`MAX_AUTH_LINE`] at most. A connection that fails stays failed: every later call
//! says so.

use std::collections::VecDeque;
use std::fmt;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;

use smithay::reexports::rustix;
use smithay::reexports::rustix::io::Errno;
use smithay::reexports::rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};

use super::message::{Body, Kind, Message, MessageError};

/// The most bytes waiting to be written: the calls of a walk of an accessibility tree, a few
/// thousand at most, are far under it.
pub(crate) const MAX_UNSENT: usize = 8 << 20;

/// The most bytes [`Connection::read`] reads at a time.
pub(crate) const READ_BUDGET: usize = 1 << 20;

/// The longest line of the authentication.
pub(crate) const MAX_AUTH_LINE: usize = 512;

/// How much is read from the socket at a time.
const CHUNK: usize = 64 * 1024;

/// The bus itself, which answers `Hello` and owns the names.
pub(crate) const BUS_NAME: &str = "org.freedesktop.DBus";
pub(crate) const BUS_PATH: &str = "/org/freedesktop/DBus";
pub(crate) const BUS_INTERFACE: &str = "org.freedesktop.DBus";

/// Why a connection failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectionError {
    /// The socket could not be made, reached, read or written.
    Io(String),
    /// The bus closed the connection.
    Closed,
    /// The bus refused the authentication, or answered it with what is not SASL.
    Authentication(String),
    /// A message that leaves the stream impossible to follow, or one too long to send.
    Message(MessageError),
    /// More than [`MAX_UNSENT`] bytes would wait to be written.
    Full,
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectionError::Io(why) => write!(f, "the bus's socket failed: {why}"),
            ConnectionError::Closed => write!(f, "the bus closed the connection"),
            ConnectionError::Authentication(why) => write!(f, "the bus refused Yuna: {why}"),
            ConnectionError::Message(error) => write!(f, "{error}"),
            ConnectionError::Full => {
                write!(
                    f,
                    "more than {MAX_UNSENT} bytes wait for the bus to read them"
                )
            }
        }
    }
}

/// Where the connection is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    /// The AUTH line is sent, its answer awaited.
    Authenticating,
    /// Messages flow; `Hello` is answered once the unique name is known.
    Running,
    Failed(ConnectionError),
}

/// A connection to a bus.
#[derive(Debug)]
pub(crate) struct Connection {
    socket: OwnedFd,
    stage: Stage,
    /// Bytes read and not yet a whole message or line.
    read: Vec<u8>,
    /// Bytes to write, in order.
    unsent: VecDeque<u8>,
    /// Messages sent before the authentication ended, encoded, to write after `BEGIN`.
    held: Vec<u8>,
    next_serial: u32,
    hello_serial: u32,
    unique_name: Option<String>,
}

impl Connection {
    /// Connects to the bus listening at `path`, without waiting: the AUTH line and `Hello`
    /// go as soon as the socket takes them.
    pub(crate) fn connect(path: &Path) -> Result<Connection, ConnectionError> {
        let io = |error: Errno| ConnectionError::Io(error.to_string());
        let socket = rustix::net::socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
            None,
        )
        .map_err(io)?;
        let address = SocketAddrUnix::new(path).map_err(io)?;
        // A Unix socket connects at once or not at all: a full backlog is EAGAIN, which a bus
        // just started never has.
        rustix::net::connect(&socket, &address).map_err(io)?;
        Connection::over(socket)
    }

    /// A connection over `socket`, a stream connected to a bus, made non-blocking: the AUTH line
    /// and `Hello` go as soon as it takes them.
    pub(crate) fn over(socket: OwnedFd) -> Result<Connection, ConnectionError> {
        rustix::io::ioctl_fionbio(&socket, true)
            .map_err(|error| ConnectionError::Io(error.to_string()))?;
        let mut connection = Connection {
            socket,
            stage: Stage::Authenticating,
            read: Vec::new(),
            unsent: VecDeque::new(),
            held: Vec::new(),
            next_serial: 1,
            hello_serial: 0,
            unique_name: None,
        };
        // SASL's EXTERNAL: the bus reads Yuna's user from the socket's credentials, which the
        // leading NUL byte comes with; the line names the same user, its decimal id in hex.
        let uid = rustix::process::getuid().as_raw().to_string();
        let hex: String = uid.bytes().map(|byte| format!("{byte:02x}")).collect();
        connection
            .unsent
            .extend(format!("\0AUTH EXTERNAL {hex}\r\n").into_bytes());
        let hello = Message::method_call(BUS_NAME, BUS_PATH, BUS_INTERFACE, "Hello", Body::empty());
        connection.hello_serial = connection.send(hello)?;
        Ok(connection)
    }

    /// The socket, for its owner's event loop.
    pub(crate) fn fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }

    /// The name the bus gave Yuna, once it answered `Hello`.
    pub(crate) fn unique_name(&self) -> Option<&str> {
        self.unique_name.as_deref()
    }

    /// Why it failed, if it did.
    pub(crate) fn failure(&self) -> Option<&ConnectionError> {
        match &self.stage {
            Stage::Failed(error) => Some(error),
            _ => None,
        }
    }

    /// Whether bytes wait to be written: the owner watches the socket for writing meanwhile.
    pub(crate) fn wants_write(&self) -> bool {
        !self.unsent.is_empty() && self.failure().is_none()
    }

    /// Sends `message` with the next serial, which it returns: queued, written by
    /// [`Connection::flush`].
    pub(crate) fn send(&mut self, mut message: Message) -> Result<u32, ConnectionError> {
        if let Some(error) = self.failure() {
            return Err(error.clone());
        }
        let serial = self.next_serial;
        let bytes = message.encode(serial).map_err(ConnectionError::Message)?;
        if self.unsent.len() + self.held.len() + bytes.len() > MAX_UNSENT {
            return Err(ConnectionError::Full);
        }
        // Serials go from 1 to u32::MAX and round again past 0, which is never one.
        self.next_serial = self.next_serial.checked_add(1).unwrap_or(1);
        message.serial = serial;
        match self.stage {
            Stage::Authenticating => self.held.extend_from_slice(&bytes),
            _ => self.unsent.extend(bytes),
        }
        Ok(serial)
    }

    /// Writes what waits, as much as the socket takes now.
    pub(crate) fn flush(&mut self) -> Result<(), ConnectionError> {
        if let Some(error) = self.failure() {
            return Err(error.clone());
        }
        while !self.unsent.is_empty() {
            let (front, _) = self.unsent.as_slices();
            match rustix::io::write(&self.socket, front) {
                Ok(0) => return Err(self.fail(ConnectionError::Closed)),
                Ok(written) => {
                    self.unsent.drain(..written);
                }
                Err(Errno::AGAIN) => return Ok(()),
                Err(Errno::INTR) => {}
                Err(error) => return Err(self.fail(ConnectionError::Io(error.to_string()))),
            }
        }
        Ok(())
    }

    /// Reads what came, [`READ_BUDGET`] at most, and gives the whole messages, in order; the
    /// reply to `Hello` is kept. A message of a kind the specification may add later is
    /// dropped, as it asks; one malformed is dropped and logged, the stream still followed.
    pub(crate) fn read(&mut self) -> Result<Vec<Message>, ConnectionError> {
        if let Some(error) = self.failure() {
            return Err(error.clone());
        }
        let mut budget = READ_BUDGET;
        let mut chunk = vec![0; CHUNK];
        let mut closed = false;
        while budget > 0 {
            let room = chunk.len().min(budget);
            match rustix::io::read(&self.socket, &mut chunk[..room]) {
                Ok(0) => {
                    closed = true;
                    break;
                }
                Ok(read) => {
                    self.read.extend_from_slice(&chunk[..read]);
                    budget -= read;
                    if let Err(error) = self.take_lines() {
                        return Err(self.fail(error));
                    }
                }
                Err(Errno::AGAIN) => break,
                Err(Errno::INTR) => {}
                Err(error) => return Err(self.fail(ConnectionError::Io(error.to_string()))),
            }
        }
        let messages = match self.take_messages() {
            Ok(messages) => messages,
            Err(error) => return Err(self.fail(error)),
        };
        if closed {
            // What came before the end is given first; the next read says it failed.
            self.stage = Stage::Failed(ConnectionError::Closed);
            if messages.is_empty() {
                return Err(ConnectionError::Closed);
            }
        }
        Ok(messages)
    }

    /// The authentication's answer, while it is awaited: `OK <guid>` lets the messages go,
    /// after `BEGIN`.
    fn take_lines(&mut self) -> Result<(), ConnectionError> {
        if self.stage != Stage::Authenticating {
            return Ok(());
        }
        let Some(end) = self.read.windows(2).position(|pair| pair == b"\r\n") else {
            if self.read.len() > MAX_AUTH_LINE {
                return Err(ConnectionError::Authentication(
                    "a line longer than SASL's".to_owned(),
                ));
            }
            return Ok(());
        };
        let line = String::from_utf8_lossy(&self.read[..end]).into_owned();
        self.read.drain(..end + 2);
        if !line.starts_with("OK ") {
            return Err(ConnectionError::Authentication(line));
        }
        self.unsent.extend(b"BEGIN\r\n".iter().copied());
        self.unsent.extend(std::mem::take(&mut self.held));
        self.stage = Stage::Running;
        Ok(())
    }

    /// The whole messages read, removed from what was read.
    fn take_messages(&mut self) -> Result<Vec<Message>, ConnectionError> {
        let mut messages = Vec::new();
        if self.stage != Stage::Running {
            return Ok(messages);
        }
        let mut start = 0;
        while let Some(length) =
            Message::frame_length(&self.read[start..]).map_err(ConnectionError::Message)?
        {
            if self.read.len() - start < length {
                break;
            }
            let bytes = &self.read[start..start + length];
            start += length;
            match Message::decode(bytes) {
                Ok(message) => {
                    if message.kind == Kind::MethodReturn
                        && message.reply_serial == Some(self.hello_serial)
                        && self.unique_name.is_none()
                    {
                        self.unique_name = message.body::<String>("s").ok();
                        continue;
                    }
                    messages.push(message);
                }
                Err(MessageError::UnknownKind(_)) => {}
                Err(error) => tracing::warn!(%error, "a message from the bus is dropped"),
            }
        }
        // What is left is the start of one message, which its frame's length bounds.
        self.read.drain(..start);
        Ok(messages)
    }

    /// Records `error` as the connection's end, and gives it back.
    fn fail(&mut self, error: ConnectionError) -> ConnectionError {
        self.stage = Stage::Failed(error.clone());
        error
    }
}
