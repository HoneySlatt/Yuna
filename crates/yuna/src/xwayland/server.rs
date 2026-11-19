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
//! Its environment is Yuna's kept variables alone ([`KEPT_VARIABLES`], [`KEPT_PREFIXES`]), with
//! Yuna's own values, and `WAYLAND_SOCKET`: nothing else of the program that started Yuna, an
//! agent's harness in `yuna mcp`, reaches Xwayland, nor the `xkbcomp` it runs.
//!
//! When Yuna draws on llvmpipe, Xwayland touches no GPU either: it draws in shared memory
//! (`-shm`), since Yuna names no device to share GPU buffers on, and it has no GLX
//! (`-extension GLX`). Mesa's GLX for Xwayland, `libdril`, loads the machine's own drivers and
//! opens its display device, `/dev/dri/card*`, even to draw on the CPU, whatever Mesa's variables
//! say (Mesa 26.2, read with `strace`); a display device opened while no compositor holds it
//! makes its opener its master. X11 programs that need OpenGL get it with `--gpu` alone.
//!
//! Its exit is read through a pidfd, which the event loop watches and which reaps it, in both
//! headless modes and in tests alike, through what every process Yuna starts shares
//! (`process.rs`). Yuna handles SIGCHLD only in `yuna mcp`, whose reaping of every child may take
//! Xwayland first: that reaping then gives the compositor its status
//! ([`Compositor::child_reaped`](crate::Compositor::child_reaped)), and the pidfd finds it
//! reaped, its end then unknown and come after.

use std::ffi::OsStr;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};

use rustix::pipe::{PipeFlags, pipe_with};
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open};

use super::display::Display;
use crate::process::{give_descriptors, keep_descriptors_from_the_program};

/// The variables of Yuna's environment Xwayland keeps, with Yuna's values: what it, the drivers
/// glamor loads with `--gpu` and the `xkbcomp` it runs read. Its own list, not the one
/// `yuna mcp` gives its programs (`mcp/programs/environment.rs`): XWayland is part of Yuna's
/// core, which does not reach into the MCP server, and Xwayland takes Yuna's own values of the
/// variables Yuna's package sets for Yuna alone (`flake.nix`), its drivers', where a program gets
/// back its caller's. No `DISPLAY`, `XAUTHORITY` or `WAYLAND_DISPLAY`: Xwayland is the display,
/// reads its cookie from `-auth`, and gets its Wayland connection through `WAYLAND_SOCKET`.
const KEPT_VARIABLES: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "LANGUAGE",
    "LOCALE_ARCHIVE",
    "XDG_CACHE_HOME",
    "LD_LIBRARY_PATH",
    "XKB_CONFIG_ROOT",
    "LIBGL_DRIVERS_PATH",
    "LIBGL_ALWAYS_SOFTWARE",
    "GBM_BACKENDS_PATH",
    "__EGL_VENDOR_LIBRARY_FILENAMES",
    "__EGL_VENDOR_LIBRARY_DIRS",
];

/// The prefixes of the variables Xwayland keeps: the locale's categories, and the settings of
/// Mesa and of Vulkan's loader, which its drivers read.
const KEPT_PREFIXES: &[&str] = &["LC_", "VK_", "MESA_"];

/// Whether Xwayland keeps Yuna's variable `name`.
fn kept(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        KEPT_VARIABLES.contains(&name) || KEPT_PREFIXES.iter().any(|p| name.starts_with(p))
    })
}

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
    command
        .env_clear()
        .envs(std::env::vars_os().filter(|(name, _)| kept(name)))
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
    // SAFETY: each number is owned by a value above, `wayland_theirs`, `wm_theirs`,
    // `displayfd_theirs` and `display`'s sockets, none of which is dropped or moved before
    // `spawn` returns.
    unsafe { give_descriptors(&mut command, given) };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xwayland_keeps_what_it_and_its_drivers_read_and_nothing_of_another_display() {
        for name in [
            "PATH",
            "LD_LIBRARY_PATH",
            "__EGL_VENDOR_LIBRARY_FILENAMES",
            "MESA_LOADER_DRIVER_OVERRIDE",
            "VK_ICD_FILENAMES",
            "LC_ALL",
        ] {
            assert!(kept(OsStr::new(name)), "{name} is not kept");
        }
        for name in [
            "DISPLAY",
            "XAUTHORITY",
            "WAYLAND_DISPLAY",
            "WAYLAND_SOCKET",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_RUNTIME_DIR",
            "SSH_AUTH_SOCK",
            "CARGO_HOME",
            "MESA",
        ] {
            assert!(!kept(OsStr::new(name)), "{name} is kept");
        }
    }
}
