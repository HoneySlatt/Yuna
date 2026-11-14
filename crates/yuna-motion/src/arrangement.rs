//! The motion of an arrangement: [`Motion`] follows the layout from one update to the next and
//! gives each output's [`Frame`] at any time. The [crate's documentation](crate#the-motion-of-an-arrangement)
//! explains how.

mod output;
mod special;
mod switch;

use std::collections::HashMap;
use std::time::Duration;

use yuna_layout::geometry::Rect;
use yuna_layout::{Layout, OutputId, Placed, WindowId, WorkspaceId};

use crate::{CurveParams, Easing, SpringParams};
use output::OutputMotion;

/// How each motion of an arrangement moves
/// ([ADR-0055](../../../docs/adr/0055-what-moves-first-and-animations-off-headless.md)).
///
/// [`Settings::default`] gives the defaults: critical springs of stiffness 800 for the camera and
/// the windows, as niri's ([survey](../../../docs/survey.md#animations)), 1,000 for the
/// workspace switch, and the special workspace's fade along [`Easing::EaseOutCubic`] over 200 ms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Every change instant: no motion starts, as with [`Change::Instant`]. For game mode and
    /// agents, and the headless modes' default.
    pub off: bool,
    /// Reduced motion: nothing crosses the screen, the camera, windows and the workspace switch
    /// showing their result at once; fades stay, over half their duration
    /// ([ADR-0059](../../../docs/adr/0059-reduced-motion-nothing-slides-fades-stay.md)).
    pub reduced_motion: bool,
    /// The scrolling camera's spring: the view sliding along a strip of columns.
    pub camera: SpringParams,
    /// The spring of each window's offset, from where it was drawn to its new place.
    pub windows: SpringParams,
    /// The spring of an output's slide from one workspace to another.
    pub workspace_switch: SpringParams,
    /// The special workspace's fade in and out.
    pub special_fade: CurveParams,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            off: false,
            reduced_motion: false,
            camera: SpringParams::known_valid(1.0, 800.0),
            windows: SpringParams::known_valid(1.0, 800.0),
            workspace_switch: SpringParams::known_valid(1.0, 1000.0),
            special_fade: CurveParams::known_valid(
                Easing::EaseOutCubic,
                Duration::from_millis(200),
            ),
        }
    }
}

/// How the arrangement changed since the last update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// By an operation whose jumps are turned into motion: a focus, a move, a new or closed
    /// window, a workspace switch.
    Animated,
    /// By a change that must show at once: outputs and seats added or removed, a drag that
    /// follows the pointer. Every motion of every output ends.
    Instant,
}

/// The motion of every output's arrangement: what each output draws at any time, between the
/// layout's arrangement before an operation and after it.
///
/// It keeps, per output, what the layout arranged at the last [`Motion::update`] and the motions
/// running over it: an offset per window that moved, a camera per scrolling workspace, a
/// workspace switch, the special workspace's fade. The [crate's
/// documentation](crate#the-motion-of-an-arrangement) explains each. It reads no clock: every
/// time is a [`Duration`] from the caller's origin, and a time earlier than the last update's
/// counts as that update's.
///
/// Its cost: [`Motion::update`] is linear in the windows of the workspaces each output draws,
/// plus, while a special workspace fades out, a [`Layout::window`] for each of its windows;
/// [`Motion::frame`] is linear in the output's drawn windows and allocates only the list it
/// returns.
#[derive(Debug, Clone)]
pub struct Motion {
    settings: Settings,
    outputs: HashMap<OutputId, OutputMotion>,
}

impl Motion {
    /// A motion with no output yet: every output is unknown until an update reads it.
    pub fn new(settings: Settings) -> Motion {
        Motion {
            settings,
            outputs: HashMap::new(),
        }
    }

    /// Changes the settings. Every running motion ends at once, so that none goes on with
    /// parameters it no longer has: each output's next frame is its last arrangement.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
        for output in self.outputs.values_mut() {
            output.settle();
        }
    }

    /// Reads every output of every seat from `layout` after a change at time `now`, and turns
    /// what moved into motion, unless the change is [`Change::Instant`] or the settings are
    /// [`off`](Settings::off). An output the layout no longer knows is forgotten.
    ///
    /// An output whose rectangle or scale changed starts again at rest, as do new outputs: their
    /// workspaces moved or were snapped again ([ADR-0052](../../../docs/adr/0052-motion-in-a-pure-crate.md)).
    /// A `now` earlier than an output's last update counts as that update's time.
    pub fn update(&mut self, layout: &Layout, now: Duration, change: Change) {
        let animate = change == Change::Animated && !self.settings.off;
        let mut previous = std::mem::take(&mut self.outputs);
        for seat in layout.seats() {
            for output in layout.outputs(seat).unwrap_or_default() {
                let motion = OutputMotion::update(
                    previous.remove(&output),
                    layout,
                    output,
                    now,
                    animate,
                    &self.settings,
                );
                if let Some(motion) = motion {
                    self.outputs.insert(output, motion);
                }
            }
        }
    }

    /// What `output` draws at `at`, bottom to top; empty for an output the last update did not
    /// read.
    ///
    /// At rest, from [`Motion::end`] on, it is exactly the layout's arrangement of the last
    /// update ([`Layout::arrange`]), in its order, each window at its place with alpha 1. While
    /// motions run: the main workspace's windows, or while a switch is drawn, those of the
    /// workspaces it slides that show on the output, each workspace's in its order, from the top
    /// of the switch's stack; then the special workspace's, shown or fading out; then the pinned
    /// windows.
    pub fn frame(&self, output: OutputId, at: Duration) -> Frame {
        self.outputs
            .get(&output)
            .map_or_else(Frame::default, |motion| motion.frame(at))
    }

    /// When the motions of `output` end: from then on its frame is the arrangement and nothing
    /// more moves. `None` when it is at rest at `at`, and for an unknown output.
    pub fn end(&self, output: OutputId, at: Duration) -> Option<Duration> {
        self.outputs.get(&output)?.end(at)
    }

    /// The windows `output` tells they are visible at `at`, by id, in increasing order
    /// ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)):
    /// those whose place overlaps it, and while its motions run, every window told since they
    /// began that it still draws. Empty for an unknown output.
    pub fn told_visible(&self, output: OutputId, at: Duration) -> Vec<WindowId> {
        self.outputs
            .get(&output)
            .map_or_else(Vec::new, |motion| motion.told_visible(at).to_vec())
    }
}

/// What an output draws at a time, bottom to top.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frame {
    /// The windows drawn, bottom to top; each window once.
    pub windows: Vec<Drawn>,
}

/// A window as a frame draws it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drawn {
    /// The window.
    pub window: WindowId,
    /// Where it is drawn, in logical coordinates: its place moved by its motions, the same size,
    /// its corner rounded once to the output's physical grid. Exactly `placed.rect` at rest.
    pub rect: Rect,
    /// Its opacity, from 0 to 1: below 1 only while the special workspace fades.
    pub alpha: f64,
    /// The workspace it is drawn with: `placed.workspace`.
    pub workspace: WorkspaceId,
    /// Its place in the layout: as [`Layout::arrange`] gave it at the last update; for a
    /// workspace sliding out of a switch, as [`Layout::arrange_workspace`] gave it; for the
    /// special workspace fading out, its last place before it was hidden.
    pub placed: Placed,
}
