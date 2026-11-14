//! Xwayland's process: how Yuna starts it for a seat's display, ends it and reaps it.
//!
//! Xwayland gets four kinds of descriptors, each cleared of close-on-exec in its own process
//! alone, after every other descriptor of Yuna's above 2 was marked close-on-exec there
//! ([`keep_descriptors_from_the_program`]): its Wayland connection (`WAYLAND_SOCKET`), one end of
//! a socket pair Yuna inserted as a client of the seat; the manager's X11 connection (`-wm`); a
//! pipe on which it writes its display number once ready (`-displayfd`); and the display's
//! listening sockets (`-listenfd`), on which it accepts once a client owns `WM_S0`. It reads the
//! seat's cookie (`-auth`) and listens on no TCP port.
//!
//! When Yuna draws on llvmpipe, Xwayland touches no GPU either: it draws in shared memory
//! (`-shm`), since Yuna names no device to share GPU buffers on, and it has no GLX
//! (`-extension GLX`). Mesa's GLX for Xwayland, `libdril`, loads the machine's own drivers and
//! opens its display device, `/dev/dri/card*`, even to draw on the CPU, whatever Mesa's variables
//! say (Mesa 26.2, read with `strace`); a display device opened while no compositor holds it
//! makes its opener its master. X11 programs that need OpenGL get it with `--gpu` alone.
//!
//! Its exit is read through a pidfd, which the event loop watches and which reaps it, in both
//! headless modes and in tests alike: Yuna handles SIGCHLD only in `yuna mcp`, whose reaping of
//! every child may take Xwayland first, and then its status is unknown. A pidfd names this one
//! process, so that a signal sent through it never reaches a process that took its pid.

use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use smithay::reexports::rustix;
use smithay::reexports::rustix::io::{Errno, FdFlags};
use smithay::reexports::rustix::pipe::{PipeFlags, pipe_with};
use smithay::reexports::rustix::process::{
    Pid, PidfdFlags, Signal, WaitId, WaitIdOptions, pidfd_open,
};

use super::display::Display;
use crate::process::keep_descriptors_from_the_program;

/// The variables of Yuna's environment Xwayland does not get: those that name a display, which
/// Xwayland is, or a Wayland connection, which it gets through `WAYLAND_SOCKET`.
const DROPPED_VARIABLES: &[&str] = &["DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "XAUTHORITY"];

/// An Xwayland just started, and Yuna's ends of what it was given.
#[derive(Debug)]
pub(crate) struct Spawned {
    pub pid: Pid,
    /// Readable once it has ended; reaps it.
    pub pidfd: OwnedFd,
    /// Yuna's end of its Wayland connection, to insert as a client.
    pub wayland: UnixStream,
    /// Yuna's end of the manager's X11 connection.
    pub wm: UnixStream,
    /// The read end of `-displayfd`, non-blocking.
    pub displayfd: OwnedFd,
}

/// Starts `program`, Xwayland, for `display`, rootless, ending `idle_seconds` after its last
/// client leaves; on the CPU alone, without GLX, when `software`.
pub(crate) fn spawn(
    program: &Path,
    display: &Display,
    idle_seconds: u32,
    software: bool,
) -> io::Result<Spawned> {
    let (wayland, wayland_theirs) = UnixStream::pair()?;
    let (wm, wm_theirs) = UnixStream::pair()?;
    let (displayfd, displayfd_theirs) = pipe_with(PipeFlags::CLOEXEC | PipeFlags::NONBLOCK)?;
    let sockets = display.sockets();
    let mut command = Command::new(program);
    command
        .arg(format!(":{}", display.number))
        .arg("-rootless")
        .arg("-terminate")
        .arg(idle_seconds.to_string())
        .arg("-wm")
        .arg(wm_theirs.as_raw_fd().to_string())
        .arg("-displayfd")
        .arg(displayfd_theirs.as_raw_fd().to_string());
    for socket in &sockets {
        command.arg("-listenfd").arg(socket.as_raw_fd().to_string());
    }
    command
        .arg("-auth")
        .arg(&display.xauthority.path)
        .args(["-nolisten", "tcp"]);
    if software {
        command.arg("-shm").args(["-extension", "GLX"]);
    }
    for variable in DROPPED_VARIABLES {
        command.env_remove(variable);
    }
    command
        .env("WAYLAND_SOCKET", wayland_theirs.as_raw_fd().to_string())
        .stdin(Stdio::null())
        // Never on Yuna's output, which is MCP's in `yuna mcp`; its errors join Yuna's log.
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    keep_descriptors_from_the_program(&mut command);
    let given: Vec<RawFd> = [
        wayland_theirs.as_raw_fd(),
        wm_theirs.as_raw_fd(),
        displayfd_theirs.as_raw_fd(),
    ]
    .into_iter()
    .chain(sockets.iter().map(|socket| socket.as_raw_fd()))
    .collect();
    // SAFETY: the closure runs in the child between fork and exec, after the one that marks
    // every descriptor close-on-exec (`pre_exec` closures run in the order they were added),
    // where only async-signal-safe calls are sound: it makes one `fcntl` system call per
    // descriptor through rustix, which allocates nothing, on numbers it was given, owned by
    // values that live in the parent until `spawn` returns, so that they are open in the child.
    unsafe {
        command.pre_exec(move || {
            for &fd in &given {
                // SAFETY: `fd` is open in the child for the closure's whole run, as above.
                let fd = BorrowedFd::borrow_raw(fd);
                rustix::io::fcntl_setfd(fd, FdFlags::empty())?;
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    // Its ends close here: Yuna sees `-displayfd` close, and the manager's connection end, when
    // Xwayland's do.
    drop((wayland_theirs, wm_theirs, displayfd_theirs, command));
    let pid = Pid::from_child(&child);
    // Reaped through its pidfd (`reap`), not through `Child::wait`.
    drop(child);
    let pidfd = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(pidfd) => pidfd,
        Err(error) => {
            // Without a pidfd it would be neither reaped nor ended: it goes at once.
            let _ = rustix::process::kill_process(pid, Signal::KILL);
            let _ = rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::empty());
            return Err(error.into());
        }
    };
    Ok(Spawned {
        pid,
        pidfd,
        wayland,
        wm,
        displayfd,
    })
}

/// How an Xwayland ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    /// It exited with this status: 0 when `-terminate` ended it, idle.
    Exited(i32),
    /// This signal ended it.
    Killed(i32),
    /// Another reaped it first: `yuna mcp`'s reaping of every child.
    Unknown,
}

/// Reaps the process of `pidfd` if it has ended: none while it runs.
pub(crate) fn reap(pidfd: BorrowedFd<'_>) -> Option<End> {
    loop {
        match rustix::process::waitid(
            WaitId::PidFd(pidfd),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG,
        ) {
            Ok(Some(status)) => {
                return Some(match (status.exit_status(), status.terminating_signal()) {
                    (Some(code), _) => End::Exited(code),
                    (None, Some(signal)) => End::Killed(signal),
                    (None, None) => End::Unknown,
                });
            }
            Ok(None) => return None,
            Err(Errno::INTR) => {}
            // ECHILD: reaped already.
            Err(_) => return Some(End::Unknown),
        }
    }
}

/// Ends the process of `pidfd` at once. A process that has ended is not signalled again, and
/// never another that took its pid.
pub(crate) fn kill(pidfd: impl AsFd) {
    if let Err(error) = rustix::process::pidfd_send_signal(pidfd, Signal::KILL)
        && error != Errno::SRCH
    {
        tracing::warn!(%error, "Xwayland could not be ended");
    }
}

/// Ends the process of `pidfd` and waits until it is reaped: when the compositor drops, with no
/// loop left to watch it. SIGKILL cannot be ignored: the wait is the kernel's tearing down.
pub(crate) fn kill_and_reap(pidfd: &OwnedFd) {
    kill(pidfd);
    loop {
        match rustix::process::waitid(WaitId::PidFd(pidfd.as_fd()), WaitIdOptions::EXITED) {
            Err(Errno::INTR) => continue,
            _ => return,
        }
    }
}
