//! Yuna's fatal path
//! ([ADR-0080](../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)): a panic
//! of the state thread, the one that runs the event loop, stops Yuna at once, never unwinding
//! through a state the panic may have left half changed.
//!
//! The panic hook keeps the standard library's: its message, the crash report for now, comes
//! first. On any other thread a panic then unwinds that thread alone: an encoder's fails its
//! capture, an X11 window manager's ends its Xwayland. On the state thread, `yuna mcp` first ends
//! every process under it and removes its seats' homes and its folder
//! ([`yuna::mcp::end_after_panic`]), at most `END_GRACE` + `KILL_WAIT`; then Yuna exits with
//! [`PANIC_STATUS`]. `yuna --headless` starts no program and has no such folder: it exits at
//! once. Both first remove their X displays' files in `/tmp`
//! ([`yuna::remove_displays_after_panic`]), which no drop removes then. The hook touches neither
//! the compositor nor the server.
//!
//! The hook runs before any unwinding, so that a panic a dependency catches on the state thread
//! is fatal too: Smithay catches those of its EGL and GL debug callbacks, which only log.
//!
//! Nothing starts Yuna again yet: ADR-0080's keeper, which restarts it within a second, comes
//! with the desktop at Stage 2b.

use std::cell::Cell;
use std::ffi::c_int;
use std::io::{self, Write};
use std::panic;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// The status Yuna exits with after a panic of its state: 101, the status the standard library
/// gives a panic that leaves `main`, so that a test or a keeper reads a panic as before.
pub const PANIC_STATUS: c_int = 101;

thread_local! {
    /// Whether this thread is the state thread: set once, by [`install`]. A constant
    /// initialiser with no destructor, so that the hook reads it on any thread, at any time.
    static STATE_THREAD: Cell<bool> = const { Cell::new(false) };
}

// SAFETY: this declaration matches the C library's own, POSIX's `void _exit(int)`, which the
// standard library links on Linux; its one call is `unsafe`, with its reason.
unsafe extern "C" {
    /// Ends the process with `status` at once: no destructor, no `atexit` handler, no flush.
    fn _exit(status: c_int) -> !;
}

/// What a panic of the state thread ends besides Yuna: `yuna mcp`'s programs and folder, once
/// the server has made it.
#[derive(Debug, Default)]
pub struct OnPanic {
    programs_folder: Arc<OnceLock<PathBuf>>,
}

impl OnPanic {
    /// From now on, a panic of the state thread ends every process under Yuna, then removes
    /// `folder`, `yuna mcp`'s folder of sockets and logs. Set once; a second folder is ignored.
    pub fn end_programs_of(&self, folder: PathBuf) {
        let _ = self.programs_folder.set(folder);
    }
}

/// Installs the panic hook, the calling thread taken as the state thread. Returns what lets
/// `yuna mcp` name its folder once it has one.
pub fn install() -> OnPanic {
    STATE_THREAD.with(|state| state.set(true));
    let on_panic = OnPanic::default();
    let programs_folder = on_panic.programs_folder.clone();
    let standard_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        standard_hook(info);
        if STATE_THREAD.try_with(Cell::get).unwrap_or(false) {
            stop(programs_folder.get());
        }
    }));
    on_panic
}

/// Stops Yuna after a panic of its state: ends its programs and removes `programs_folder` when
/// it has one, then exits with [`PANIC_STATUS`].
fn stop(programs_folder: Option<&PathBuf>) -> ! {
    // Before the programs end: the X displays' files name this process, alive until the exit.
    yuna::remove_displays_after_panic();
    match programs_folder {
        Some(folder) => {
            report("yuna: its state panicked: its programs end, its folder goes, and it stops\n");
            yuna::mcp::end_after_panic(folder);
        }
        None => report("yuna: its state panicked: it stops\n"),
    }
    // SAFETY: `_exit` takes any status and never returns. It runs nothing more: no destructor,
    // so that nothing touches the state the panic left; no `atexit` handler, Mesa's among them,
    // which would tear the renderer down beneath that state while Yuna's other threads run. The
    // kernel closes every descriptor, and Yuna's clients see their connections end.
    unsafe { _exit(PANIC_STATUS) }
}

/// Writes `line` to standard error, as the standard hook writes its message: if that fails,
/// nothing is left to report it to.
fn report(line: &str) {
    let _ = io::stderr().lock().write_all(line.as_bytes());
}
