//! Yuna, a Wayland compositor for Linux: its headless mode, as a library that the `yuna`
//! program runs and that tests drive in their own process
//! ([ADR-0002](../../../docs/adr/0002-the-headless-mode-first.md),
//! [ADR-0123](../../../docs/adr/0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md)).
//!
//! A [`Compositor`] holds Smithay's protocol state, the layout model ([`yuna_layout`]), the
//! motion that turns the layout's jumps into frames ([`yuna_motion`]), virtual outputs drawn
//! offscreen by one GLES renderer, and a [`Clock`] that owns the frame loop's deadlines. Frames
//! follow damage: a commit of a shown surface, or a change of the layout, arms its output's next
//! refresh, drawn ahead of its boundary; while a motion runs, each frame arms the next; at rest
//! nothing is armed
//! ([ADR-0118](../../../docs/adr/0118-frames-presentation-times-and-the-clock.md)).
//!
//! **Threads.** Everything the compositor holds runs on one thread, the *state thread*: the
//! thread that made it, which runs its event loop, and in `yuna mcp` the server. It never waits
//! on a client, a program's pipe or an X server: each source only notes what came, and the loop
//! reads it when it is ready. Beside it run one X11 window manager for each running Xwayland, on
//! a thread of its own (`yuna-x11-<seat>`, `xwayland/manager/`), which waits on X11 so that the
//! loop never does; and, in `yuna mcp`, up to `MAX_ENCODERS` threads that encode captures
//! (`yuna-capture`, `mcp/captures.rs`). A panic of the state thread is fatal; one of another
//! thread ends that thread's work alone (`fatal.rs`).
//!
//! Each module has one job; `docs/ARCHITECTURE.md` lists them (*Modules*) and gives the flows
//! across them.

mod activation;
mod capture;
mod clock;
pub mod colour;
mod compositor;
mod dbus;
mod dialog;
mod dmabuf;
mod frames;
mod input;
mod keymap;
pub mod mcp;
mod output;
mod popups;
mod process;
mod render;
pub mod rest;
mod scale;
mod screencopy;
mod seats;
mod selection;
mod shell;
mod state;
mod text_input;
mod virtual_input;
mod vrr;
mod wlr_screencopy;
mod xwayland;

use std::fmt;

pub use activation::{MAX_KEPT_TOKENS_OF_CLIENTS_GONE, TOKEN_LIFETIME};
#[doc(hidden)]
pub use capture::axis_taps;
pub use capture::{
    CaptureMapping, CaptureSize, CaptureTarget, CaptureTicket, CapturedFrame, LogicalRegion,
};
pub use clock::{Clock, ClockMode};
pub use compositor::{
    ClientDeparture, ClientProcess, ClientProtocolError, Compositor, Config, Counters,
    OutputDescription, SOCKET_PAUSE, WindowDescription, WindowShell,
};
pub use frames::HIDDEN_FRAME_INTERVAL;
pub use input::{
    BUTTON_BACK, BUTTON_FORWARD, BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT, MAX_HELD_BUTTONS,
    MAX_HELD_KEYS, MAX_SCROLL_STEPS, MAX_TYPED, PointerDescription,
};
pub use keymap::{KeyboardLayout, MAX_LAYOUT_NAME};
pub use output::{InvalidOutput, MAX_OUTPUTS, OutputMode, RETIRED_GLOBAL_LIFETIME};
pub use popups::{MAX_POPUP_DEPTH, MAX_POPUPS_PER_WINDOW};
pub use process::raise_open_files_limit;
pub use render::Device;
pub use screencopy::{MAX_CAPTURE_SESSIONS, MAX_CAPTURE_SESSIONS_PER_CLIENT};
pub use seats::AGENT_KEYBOARD_LAYOUT;
pub use selection::{CLIPBOARD_TIMEOUT, MAX_CLIPBOARD_WRITES};
pub use shell::{MAX_WINDOWS, MAX_WINDOWS_PER_CLIENT};
pub use state::OWNER_SEAT;
pub use text_input::MAX_TEXT_BYTES;
pub use virtual_input::MAX_VIRTUAL_DEVICES_PER_CLIENT;
pub use xwayland::{
    DrawnX11Menu, FAILURE_MEMORY, FIRST_PAUSE, IDLE_SECONDS, LONGEST_PAUSE, MANAGER_GRACE,
    MANAGER_SLICE, MAX_DISPLAYS, MAX_MENUS_PER_CLIENT, MAX_MENUS_PER_XWAYLAND,
    MAX_X11_TRANSFER_BYTES, MAX_X11_TRANSFERS, START_TIMEOUT, X11_PING_TIMEOUT, X11_SYNC_TIMEOUT,
    X11_TRANSFER_TIMEOUT, X11Display, XwaylandConfig, remove_displays_after_panic,
};
pub use xwayland::{MANAGER_STALL, STALL_RECHECK};
#[cfg(debug_assertions)]
pub use xwayland::{PANIC_ON_X11_CLOSE, X11_OWED_BOUND};
pub use yuna_layout;
pub use yuna_motion;

/// Why the compositor could not start, or refused a request.
#[derive(Debug)]
pub enum Error {
    /// An output's mode was refused; the reason says why.
    InvalidOutput(InvalidOutput),
    /// The renderer could not start, allocate or draw.
    Renderer(String),
    /// The layout model refused its setup.
    Layout(String),
    /// The keyboard's keymap could not be compiled.
    Keyboard(String),
    /// The Wayland socket could not be created, most often because `XDG_RUNTIME_DIR` is unset.
    Socket(String),
    /// The event loop or the display failed.
    EventLoop(String),
    /// Only the manual clock can be advanced.
    NotManual,
    /// A request named something that does not exist, or asked something impossible.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidOutput(reason) => write!(f, "invalid output: {reason}"),
            Error::Renderer(reason) => write!(f, "renderer: {reason}"),
            Error::Layout(reason) => write!(f, "layout: {reason}"),
            Error::Keyboard(reason) => write!(f, "keyboard: {reason}"),
            Error::Socket(reason) => write!(f, "Wayland socket: {reason}"),
            Error::EventLoop(reason) => write!(f, "event loop: {reason}"),
            Error::NotManual => f.write_str("only the manual clock can be advanced"),
            Error::Refused(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for Error {}
