//! Stopping cleanly on SIGTERM and SIGINT: the signal handler writes a byte to a pipe whose read
//! end the compositor's event loop watches, so that the loop wakes and returns, and the Wayland
//! socket and its lock file are removed. SIGCHLD, for `yuna mcp`, writes to a pipe of its own, so
//! that the loop reaps the programs that ended.
//!
//! The handler is set with the C library's `signal`, which the standard library links already:
//! calloop's own signal source needs the `nix` crate, a dependency the owner has not accepted.

use std::ffi::c_int;
use std::io;
use std::os::fd::{BorrowedFd, IntoRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicI32, Ordering};

use rustix::pipe::{PipeFlags, pipe_with};
use rustix::process::Signal;
use smithay::reexports::rustix;

/// The pipes' write ends, for the handlers: each set once, before its handler is installed, and
/// never closed while the process runs.
static STOP_PIPE: AtomicI32 = AtomicI32::new(-1);
static CHILDREN_PIPE: AtomicI32 = AtomicI32::new(-1);

/// `SIG_ERR`, `(void (*)(int)) -1`, as `signal` returns it.
const SIG_ERR: usize = usize::MAX;

// SAFETY: this declaration matches the C library's own, which the standard library links on
// Linux; calls to it are `unsafe`, each with its reason.
unsafe extern "C" {
    /// POSIX `signal`: `void (*signal(int, void (*)(int)))(int)`. The handler and the value
    /// returned are pointer-sized, passed as integers so that `SIG_ERR` is not taken for a
    /// function.
    fn signal(signum: c_int, handler: usize) -> usize;
}

/// Writes one byte to the stop pipe.
extern "C" fn on_stop_signal(_signal: c_int) {
    write_byte(&STOP_PIPE);
}

/// Writes one byte to the children's pipe.
extern "C" fn on_child_signal(_signal: c_int) {
    write_byte(&CHILDREN_PIPE);
}

/// Writes one byte to the pipe whose write end `pipe` holds. Only async-signal-safe work: an
/// atomic load and `write(2)`, which rustix makes as a raw system call, without `errno` or a
/// lock.
fn write_byte(pipe: &AtomicI32) {
    let fd: RawFd = pipe.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    // SAFETY: `fd` is the pipe's write end, stored by `install` before any handler could run and
    // never closed while the process lives, so it stays open for this borrow.
    let pipe = unsafe { BorrowedFd::borrow_raw(fd) };
    // The pipe never blocks; a full pipe already holds the byte that stops the loop.
    let _ = rustix::io::write(pipe, &[1]);
}

/// Installs the handler for SIGTERM and SIGINT, and returns the pipe's read end, which becomes
/// readable when either arrives.
pub fn install() -> io::Result<OwnedFd> {
    install_on(&STOP_PIPE, on_stop_signal, &[Signal::TERM, Signal::INT])
}

/// Installs the handler for SIGCHLD, and returns the pipe's read end, which becomes readable
/// when a child ends.
pub fn install_children() -> io::Result<OwnedFd> {
    install_on(&CHILDREN_PIPE, on_child_signal, &[Signal::CHILD])
}

/// Makes a pipe whose write end `pipe` keeps, and installs `handler` for `signals`.
fn install_on(
    pipe: &AtomicI32,
    handler: extern "C" fn(c_int),
    signals: &[Signal],
) -> io::Result<OwnedFd> {
    let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
    // Kept open for the life of the process: the handler may run at any time.
    pipe.store(write.into_raw_fd(), Ordering::Relaxed);
    for &signal_number in signals {
        // SAFETY: `signal` is declared with its C signature, pointer-sized values passed as
        // `usize`; the handler is an `extern "C" fn(c_int)` that does only async-signal-safe
        // work, and the pipe it writes to is open before it is installed.
        let previous = unsafe { signal(signal_number.as_raw(), handler as usize) };
        if previous == SIG_ERR {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(read)
}
