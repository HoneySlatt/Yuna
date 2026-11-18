//! Stopping cleanly on SIGTERM and SIGINT: the signal handler writes a byte to a pipe whose read
//! end the compositor's event loop watches, so that the loop wakes and returns, and the Wayland
//! socket and its lock file are removed. SIGCHLD, for `yuna mcp`, writes to a pipe of its own, so
//! that the loop reaps the programs that ended; only when a child ends, never when one is stopped
//! or continued (`SA_NOCLDSTOP`), so that a program frozen or slowed through MCP wakes nothing.
//!
//! The handlers are set with the C library's `sigaction`, which the standard library links
//! already: calloop's own signal source needs the `nix` crate, a dependency the owner has not
//! accepted. Its structure is declared here as glibc and musl lay it out on x86-64 and AArch64,
//! the targets Yuna builds for; another target refuses to build rather than pass a wrong layout.

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

#[cfg(not(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
compile_error!(
    "stop_signal.rs declares `struct sigaction` as Linux's C libraries lay it out on x86-64 and \
     AArch64: check its layout on this target before adding it"
);

/// Restarts a system call the handler interrupted, as `signal` does on Linux.
const SA_RESTART: c_int = 0x1000_0000;

/// No SIGCHLD when a child stops or continues: only when it ends.
const SA_NOCLDSTOP: c_int = 1;

/// POSIX's `struct sigaction`, as glibc and musl lay it out on x86-64 and AArch64: the handler,
/// a union whose other member Yuna does not use, as a pointer-sized integer; the signals blocked
/// while it runs, a set of 1,024 bits; the flags; and the restorer, which the C library sets
/// itself.
#[repr(C)]
struct SigAction {
    handler: usize,
    mask: [u64; 16],
    flags: c_int,
    restorer: usize,
}

// SAFETY: this declaration matches the C library's own, which the standard library links on
// Linux, with `SigAction` laid out as its `struct sigaction` on the targets allowed above;
// calls to it are `unsafe`, each with its reason.
unsafe extern "C" {
    /// POSIX `sigaction`: `int sigaction(int, const struct sigaction *, struct sigaction *)`.
    fn sigaction(signum: c_int, action: *const SigAction, previous: *mut SigAction) -> c_int;
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
    install_on(&STOP_PIPE, on_stop_signal, &[Signal::TERM, Signal::INT], 0)
}

/// Installs the handler for SIGCHLD, and returns the pipe's read end, which becomes readable
/// when a child ends; a child stopped or continued sends nothing.
pub fn install_children() -> io::Result<OwnedFd> {
    install_on(
        &CHILDREN_PIPE,
        on_child_signal,
        &[Signal::CHILD],
        SA_NOCLDSTOP,
    )
}

/// Makes a pipe whose write end `pipe` keeps, and installs `handler` for `signals`, with
/// `SA_RESTART` and `flags`.
fn install_on(
    pipe: &AtomicI32,
    handler: extern "C" fn(c_int),
    signals: &[Signal],
    flags: c_int,
) -> io::Result<OwnedFd> {
    let (read, write) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
    // Kept open for the life of the process: the handler may run at any time.
    pipe.store(write.into_raw_fd(), Ordering::Relaxed);
    let action = SigAction {
        handler: handler as usize,
        // No other signal is blocked while the handler runs.
        mask: [0; 16],
        flags: SA_RESTART | flags,
        restorer: 0,
    };
    for &signal_number in signals {
        // SAFETY: `sigaction` is declared with its C signature, and `action` is a valid
        // `struct sigaction` for the whole call, laid out as the C library's: its handler is an
        // `extern "C" fn(c_int)` that does only async-signal-safe work, its mask empty, its flags
        // the C library's own values, no `SA_SIGINFO` among them, so that the handler is read as
        // a one-argument function. The previous action is not asked for: a null pointer, which
        // POSIX allows. The pipe the handler writes to is open before it is installed.
        let failed = unsafe { sigaction(signal_number.as_raw(), &action, std::ptr::null_mut()) };
        if failed != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(read)
}
