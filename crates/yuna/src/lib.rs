//! Yuna, a Wayland compositor for Linux: its headless mode, as a library that the `yuna`
//! program runs and that tests drive in their own process
//! ([ADR-0002](../../../docs/adr/0002-the-headless-mode-first.md),
//! [ADR-0022](../../../docs/adr/0022-the-headless-backend-in-yuna.md)).
//!
//! A [`Compositor`] holds Smithay's protocol state, the layout model ([`yuna_layout`]), virtual
//! outputs drawn offscreen by one GLES renderer, and a [`Clock`] that owns the frame loop's
//! deadlines. Frames follow damage: a commit of a shown surface, or a change of the layout, arms
//! its output's next refresh, drawn ahead of its boundary; at rest nothing is armed
//! ([ADR-0023](../../../docs/adr/0023-frames-presentation-and-the-clock.md),
//! [ADR-0031](../../../docs/adr/0031-frames-drawn-ahead-of-their-boundary.md)).
//!
//! The parts, each in its module: the clock (`clock.rs`); the virtual outputs and their refresh
//! boundaries (`output.rs`); the renderer, the only part that knows OpenGL (`render.rs`); the
//! surfaces and the other protocol handlers (`state.rs`); the seats (`seats.rs`) and their
//! pointer and keyboard input (`input.rs`); `xdg-shell` and
//! `xdg-decoration` in front of the layout (`shell.rs`), with popups (`popups.rs`),
//! `xdg-activation` (`activation.rs`) and `xdg-dialog` (`dialog.rs`); the clipboard and the
//! primary selection (`selection.rs`); the frame loop (`frames.rs`); captures (`capture.rs`),
//! and `ext-image-copy-capture-v1` on them (`screencopy.rs`); clients' dmabufs (`dmabuf.rs`);
//! the scale each surface is told (`scale.rs`); the public face (`compositor.rs`). `docs/ARCHITECTURE.md` gives the flow across them.

mod activation;
mod capture;
mod clock;
mod compositor;
mod dialog;
mod dmabuf;
mod frames;
mod input;
pub mod mcp;
mod output;
mod popups;
mod render;
mod scale;
mod screencopy;
mod seats;
mod selection;
mod shell;
mod state;
mod text_input;

use std::fmt;

pub use activation::TOKEN_LIFETIME;
pub use capture::{
    CaptureMapping, CaptureSize, CaptureTarget, CaptureTicket, CapturedFrame, LogicalRegion,
};
pub use clock::{Clock, ClockMode};
pub use compositor::{
    Compositor, Config, Counters, OutputDescription, SOCKET_PAUSE, WindowDescription,
};
pub use frames::HIDDEN_FRAME_INTERVAL;
pub use input::{
    BUTTON_BACK, BUTTON_FORWARD, BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT, MAX_SCROLL_STEPS,
    MAX_TYPED, PointerDescription,
};
pub use output::{InvalidOutput, MAX_OUTPUTS, OutputMode, RETIRED_GLOBAL_LIFETIME};
pub use popups::{MAX_POPUP_DEPTH, MAX_POPUPS_PER_WINDOW};
pub use render::Device;
pub use screencopy::{MAX_CAPTURE_SESSIONS, MAX_CAPTURE_SESSIONS_PER_CLIENT};
pub use seats::AGENT_KEYBOARD_LAYOUT;
pub use shell::{MAX_WINDOWS, MAX_WINDOWS_PER_CLIENT};
pub use state::OWNER_SEAT;
pub use text_input::MAX_TEXT_BYTES;
pub use yuna_layout;

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
