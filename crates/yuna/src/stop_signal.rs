//! Stopping cleanly on SIGTERM and SIGINT: the signal handler writes a byte to a pipe whose read
//! end the compositor's event loop watches, so that the loop wakes and returns, and the Wayland
//! socket and its lock file are removed.
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

/// The pipe's write end, for the handler: set once, before the handler is installed, and never
/// closed while the process runs.
static STOP_PIPE: AtomicI32 = AtomicI32::new(-1);

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

/// Writes one byte to the pipe. Only async-signal-safe work: an atomic load and `write(2)`,
/// which rustix makes as a raw system call, without `errno` or a lock.
extern "C" fn on_stop_signal(_signal: c_int) {
    let fd: RawFd = STOP_PIPE.load(Ordering::Relaxed);
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
    let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
    // Kept open for the life of the process: the handler may run at any time.
    STOP_PIPE.store(write.into_raw_fd(), Ordering::Relaxed);
    let handler: extern "C" fn(c_int) = on_stop_signal;
    for stop in [Signal::TERM, Signal::INT] {
        // SAFETY: `signal` is declared with its C signature, pointer-sized values passed as
        // `usize`; the handler is an `extern "C" fn(c_int)` that does only async-signal-safe
        // work, and the pipe it writes to is open before it is installed.
        let previous = unsafe { signal(stop.as_raw(), handler as usize) };
        if previous == SIG_ERR {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(read)
}
