//! Yuna, a Wayland compositor for Linux: its headless mode, as a library that the `yuna`
//! program runs and that tests drive in their own process
//! ([ADR-0002](../../../docs/adr/0002-the-headless-mode-first.md),
//! [ADR-0022](../../../docs/adr/0022-the-headless-backend-in-yuna.md)).
//!
//! A [`Compositor`] holds Smithay's protocol state, the layout model ([`yuna_layout`]), virtual
//! outputs drawn offscreen by one GLES renderer, and a [`Clock`] that owns the frame loop's
//! deadlines. Frames follow damage: a commit of a shown surface, or a change of the layout, arms
//! one refresh boundary; at rest nothing is armed
//! ([ADR-0023](../../../docs/adr/0023-frames-presentation-and-the-clock.md)).
//!
//! The parts, each in its module: the clock (`clock.rs`); the virtual outputs and their refresh
//! boundaries (`output.rs`); the renderer, the only part that knows OpenGL (`render.rs`); the
//! surfaces, the seat and the other protocol handlers (`state.rs`); `xdg-shell` and
//! `xdg-decoration` in front of the layout (`shell.rs`); the frame loop (`frames.rs`); the
//! public face (`compositor.rs`). `docs/ARCHITECTURE.md` gives the flow across them.

mod clock;
mod compositor;
mod frames;
mod output;
mod render;
mod shell;
mod state;

use std::fmt;

pub use clock::{Clock, ClockMode};
pub use compositor::{Compositor, Config, Counters};
pub use frames::HIDDEN_FRAME_INTERVAL;
pub use output::{InvalidOutput, OutputMode};
pub use render::Device;
pub use state::OWNER_SEAT;
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
