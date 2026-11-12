//! What each surface is told of its output's scale: `wp_fractional_scale_v1`'s preferred scale,
//! and `wl_surface.preferred_buffer_scale` and `preferred_buffer_transform` (`wl_surface`
//! version 6). A client that follows the fractional scale draws its buffer at its output's
//! physical size and sets its viewport's destination (`wp_viewporter`) to its logical size: one
//! buffer pixel then covers one physical pixel, and the window sits exactly on its pixels.
//!
//! A surface is told the scale of the output its window is shown on: when its fractional scale
//! object is created, at its commits (in practice its first: a new surface, a new subsurface),
//! and when its window enters an output. Only a change sends anything. A surface whose window is
//! on no output keeps the last value it was told. One never told anything learns what the root
//! of its tree was last told, so that a subsurface added to a hidden window matches its window;
//! else, as a new window, the scale of the owner seat's active output, where a new window opens,
//! else the first output's, so that a window's first buffer is drawn at the right scale.

use std::cell::Cell;

use smithay::desktop::utils::with_surfaces_surface_tree;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Transform;
use smithay::wayland::compositor::{SurfaceData, send_surface_state, with_states};
use smithay::wayland::fractional_scale::{FractionalScaleHandler, with_fractional_scale};
use yuna_layout::OutputId;

use crate::state::{OWNER_SEAT, State, tree_root};

/// What a surface is told of its output.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Preferred {
    /// The output's scale, as `wp_fractional_scale_v1` carries it.
    scale: f64,
    transform: Transform,
}

/// The last `Preferred` a surface was told, kept in its state's data map.
#[derive(Debug, Default)]
struct LastTold(Cell<Option<Preferred>>);

impl State {
    /// Tells every surface of the tree under `root` the scale of `output`, the output the tree is
    /// shown on. It takes any tree's root, so that a popup's surfaces can be told the output of
    /// their toplevel.
    pub(crate) fn tell_preferred_scale(&self, root: &WlSurface, output: OutputId) {
        let Some(preferred) = self.preferred_on(output) else {
            return;
        };
        with_surfaces_surface_tree(root, |surface, states| tell(surface, states, preferred));
    }

    /// Tells `surface`, in the tree under `root`, the scale of the output the tree is shown on.
    /// Shown nowhere, the surface keeps what it was told, or, never told anything, learns
    /// `preferred_never_told`. Called at each commit: a surface never told its scale, a new
    /// subsurface most often, learns it then; for the others nothing changed, and nothing is sent.
    pub(crate) fn tell_surface_preferred_scale(&self, root: &WlSurface, surface: &WlSurface) {
        let preferred = match self.output_showing(root) {
            Some(output) => self.preferred_on(output),
            None if with_states(surface, last_told).is_some() => return,
            None => self.preferred_never_told(root),
        };
        if let Some(preferred) = preferred {
            with_states(surface, |states| tell(surface, states, preferred));
        }
    }

    /// The output the tree under `root` is shown on: its toplevel's, from the last arrangement.
    fn output_showing(&self, root: &WlSurface) -> Option<OutputId> {
        self.toplevels
            .get(&root.id())
            .and_then(|toplevel| toplevel.shown_on)
    }

    fn preferred_on(&self, output: OutputId) -> Option<Preferred> {
        let output = &self.outputs[self.output_index(output)?];
        Some(Preferred {
            scale: output.mode.scale(),
            transform: output.output.current_transform(),
        })
    }

    /// What a surface of the tree under `root`, shown nowhere and never told anything, learns:
    /// what `root` was last told; else, as a new window, the owner seat's active output, where
    /// new windows open; else the first output.
    fn preferred_never_told(&self, root: &WlSurface) -> Option<Preferred> {
        with_states(root, last_told)
            .or_else(|| {
                self.layout
                    .active_output(OWNER_SEAT)
                    .and_then(|output| self.preferred_on(output))
            })
            .or_else(|| {
                let first = self.outputs.first()?;
                self.preferred_on(first.id)
            })
    }
}

/// What the surface whose state is `states` was last told; none if it was never told anything.
fn last_told(states: &SurfaceData) -> Option<Preferred> {
    states
        .data_map
        .get::<LastTold>()
        .and_then(|last| last.0.get())
}

/// Tells one surface `preferred`. Each event goes out only when its value changes.
fn tell(surface: &WlSurface, states: &SurfaceData, preferred: Preferred) {
    states
        .data_map
        .get_or_insert(LastTold::default)
        .0
        .set(Some(preferred));
    with_fractional_scale(states, |state| state.set_preferred_scale(preferred.scale));
    send_surface_state(
        surface,
        states,
        buffer_scale(preferred.scale),
        preferred.transform,
    );
}

/// The buffer scale `wl_surface` prefers for an output's scale: its integer ceiling.
fn buffer_scale(scale: f64) -> i32 {
    // An output's scale is checked to lie within 0.25–8 when it is created.
    (scale.ceil() as i32).max(1)
}

impl FractionalScaleHandler for State {
    /// A client asks the preferred scale of `surface`: it learns it at once, even if it was told
    /// it before through `wl_surface` alone.
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let root = tree_root(&surface);
        self.tell_surface_preferred_scale(&root, &surface);
    }
}

#[cfg(test)]
mod tests {
    use super::buffer_scale;

    #[test]
    fn the_buffer_scale_is_the_ceiling_of_the_outputs() {
        assert_eq!(buffer_scale(0.25), 1);
        assert_eq!(buffer_scale(1.0), 1);
        assert_eq!(buffer_scale(1.25), 2);
        assert_eq!(buffer_scale(1.5), 2);
        assert_eq!(buffer_scale(2.0), 2);
        assert_eq!(buffer_scale(8.0), 8);
    }
}
