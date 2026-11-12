//! The frame loop ([ADR-0023](../../../docs/adr/0023-frames-presentation-and-the-clock.md)).
//!
//! A commit of a surface shown on an output, or a change of the layout, arms that output's next
//! refresh boundary; nothing is armed otherwise. At the boundary, in order: the layout is
//! arranged once if it changed, and the configures that changed are sent; the output is drawn
//! if anything on it is damaged; its shown surfaces get their frame callbacks and their
//! presentation feedback. A frame is presented at the first boundary at or after its drawing
//! ended, and never at the boundary of an earlier frame.
//!
//! A window shown on no output is hidden: it is told `suspended`, its content updates are
//! discarded, and its frame callbacks go out at most once a second, through one deadline armed
//! only while such a window waits for one. A window the layout refused to open is treated the
//! same way, so that its client never waits for ever.
//!
//! A window's popups go with it: drawn just above it, called back, reported presented or
//! discarded, and entering or leaving outputs with its own surfaces.

use std::collections::HashMap;
use std::time::Duration;

use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
use smithay::desktop::utils::{
    OutputPresentationFeedback, SurfacePresentationFeedback, take_presentation_feedback_surface_tree,
    with_surfaces_surface_tree,
};
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Monotonic, SERIAL_COUNTER};
use smithay::wayland::compositor::{
    SurfaceAttributes, SurfaceData, TraversalAction, with_surface_tree_downward,
};
use smithay::wayland::presentation::Refresh;
use yuna_layout::{OutputId, Placed, WindowId};

use crate::clock::Deadline;
use crate::popups::window_surface_trees;
use crate::render;
use crate::state::{OWNER_SEAT, State};

/// The shortest time between two frame callbacks of a hidden window.
pub const HIDDEN_FRAME_INTERVAL: Duration = Duration::from_secs(1);

impl State {
    /// Marks the layout changed: every output arranges it again at its next boundary.
    pub(crate) fn mark_layout_changed(&mut self) {
        self.layout_dirty = true;
        let ids: Vec<OutputId> = self.outputs.iter().map(|o| o.id).collect();
        for id in ids {
            self.schedule_refresh(id);
        }
    }

    /// Arms `output`'s next refresh boundary, unless it is armed already.
    pub(crate) fn schedule_refresh(&mut self, output: OutputId) {
        let deadline = Deadline::Refresh(output);
        if self.clock.is_armed(deadline) {
            return;
        }
        let now = self.clock.now();
        let Some(index) = self.output_index(output) else {
            return;
        };
        let output = &mut self.outputs[index];
        let boundary = output.boundaries.first_after(now);
        output.armed_boundary = boundary;
        let at = output.boundaries.at(boundary);
        self.clock.arm(deadline, at);
    }

    /// What a commit of `surface`, in the toplevel of `root`, arms: its output's next boundary
    /// when the window is shown; when no output shows it, its content update is discarded and
    /// its frame callbacks wait for the hidden windows' deadline. While the layout waits to be
    /// arranged, every output is armed already, and the arrangement decides for mapped windows.
    pub(crate) fn schedule_for_commit(&mut self, root: &WlSurface, surface: &WlSurface) {
        let Some(toplevel) = self.toplevels.get(&root.id()) else {
            return;
        };
        let (mapped, shown_on, unseen) = (
            toplevel.window.is_some(),
            toplevel.shown_on,
            toplevel.is_unseen(),
        );
        if let Some(index) = shown_on.and_then(|output| self.output_index(output)) {
            // A subsurface committed for the first time enters its window's output, even when
            // the layout waits: an arrangement that keeps the window there sends no `enter`.
            self.outputs[index].output.enter(surface);
        }
        if mapped && self.layout_dirty {
            return;
        }
        if let Some(output) = shown_on {
            self.schedule_refresh(output);
        } else if unseen {
            discard_presentation_feedback(root);
            if waits_for_frame(root) {
                self.arm_hidden_frames();
            }
        }
    }

    /// Runs a deadline that came due.
    pub(crate) fn fire(&mut self, deadline: Deadline) {
        match deadline {
            Deadline::Refresh(output) => self.refresh(output),
            Deadline::HiddenFrames => self.send_hidden_frames(),
        }
    }

    /// An output's refresh boundary. See the module's documentation.
    fn refresh(&mut self, output: OutputId) {
        if self.layout_dirty {
            self.arrange();
        }
        let Some(index) = self.output_index(output) else {
            return;
        };
        let shown = self.shown(index);
        let drawing = self.draw(index, &shown);
        let now = self.clock.now();
        for placed in &shown {
            if let Some(root) = self.root_of(placed.window) {
                self.send_frames(&root, now);
            }
        }
        let output = &mut self.outputs[index];
        let boundary = output.armed_boundary;
        let seq = match drawing {
            Drawing::Drawn => {
                // When the drawing could first show, and never on an earlier frame's boundary.
                let next_free = output.last_presented.map_or(0, |seq| seq + 1);
                let seq = output
                    .boundaries
                    .first_at_or_after(now)
                    .max(boundary)
                    .max(next_free);
                output.last_presented = Some(seq);
                seq
            }
            // Nothing new to show: content updates show with the frame already on its way.
            Drawing::Unchanged => boundary.max(output.last_presented.unwrap_or(0)),
            Drawing::Failed => {
                for placed in &shown {
                    if let Some(root) = self.root_of(placed.window) {
                        discard_presentation_feedback(&root);
                    }
                }
                return;
            }
        };
        let presented_at = output.boundaries.at(seq);
        let refresh = Refresh::Fixed(output.mode.period());
        let smithay_output = output.output.clone();
        let mut feedback = OutputPresentationFeedback::new(&smithay_output);
        for placed in &shown {
            if let Some(root) = self.root_of(placed.window) {
                for tree in window_surface_trees(&root) {
                    take_presentation_feedback_surface_tree(
                        &tree,
                        &mut feedback,
                        |_, _| Some(smithay_output.clone()),
                        |_, _| Kind::empty(),
                    );
                }
            }
        }
        feedback.presented::<Duration, Monotonic>(presented_at, refresh, seq, Kind::Vsync);
    }

    /// Draws output `index`, which shows `shown`, if anything on it is damaged.
    fn draw(&mut self, index: usize, shown: &[Placed]) -> Drawing {
        let output = &self.outputs[index];
        let mut elements = Vec::new();
        // The layout gives windows bottom to top; the renderer takes them front to back.
        for placed in shown.iter().rev() {
            let Some(toplevel) = self
                .windows
                .get(&placed.window)
                .and_then(|id| self.toplevels.get(id))
            else {
                continue;
            };
            // A window's popups are in front of it, behind the windows over it.
            elements.extend(render::popup_elements(
                &mut self.renderer,
                toplevel.wl_surface(),
                placed.rect,
                output,
            ));
            elements.extend(render::window_elements(
                &mut self.renderer,
                toplevel.wl_surface(),
                toplevel.geometry,
                placed.rect,
                output,
            ));
        }
        let output = &mut self.outputs[index];
        match render::draw(&mut self.renderer, output, &elements) {
            Ok(true) => {
                self.counters.frames_drawn += 1;
                if let Some(hook) = &mut self.draw_hook {
                    hook(&mut self.clock);
                }
                Drawing::Drawn
            }
            Ok(false) => Drawing::Unchanged,
            Err(error) => {
                tracing::warn!(%error, "a frame could not be drawn");
                Drawing::Failed
            }
        }
    }

    /// The windows output `index` shows, bottom to top, from its last arrangement: those on it,
    /// not those scrolled out of view.
    fn shown(&self, index: usize) -> Vec<Placed> {
        let output = &self.outputs[index];
        output
            .placed
            .iter()
            .filter(|placed| placed.rect.overlaps(&output.rect))
            .copied()
            .collect()
    }

    fn root_of(&self, window: WindowId) -> Option<WlSurface> {
        self.windows
            .get(&window)
            .and_then(|id| self.toplevels.get(id))
            .map(|toplevel| toplevel.wl_surface().clone())
    }

    /// Sends the frame callbacks the surfaces of the toplevel of `root` wait for, stamped `now`.
    fn send_frames(&mut self, root: &WlSurface, now: Duration) {
        let sent = send_frame_callbacks(root, now);
        if sent > 0 {
            self.counters.frame_callbacks += sent;
            if let Some(toplevel) = self.toplevels.get_mut(&root.id()) {
                toplevel.last_frame_sent = Some(now);
            }
        }
    }

    /// Arranges every output, tells each window its size and states, and moves each between
    /// shown and hidden.
    pub(crate) fn arrange(&mut self) {
        self.layout_dirty = false;
        let mut shown: HashMap<WindowId, (OutputId, Placed)> = HashMap::new();
        let mut scrolled_out: HashMap<WindowId, Placed> = HashMap::new();
        for output in &mut self.outputs {
            output.placed = self.layout.arrange(output.id).unwrap_or_else(|error| {
                tracing::warn!(%error, output = output.id.0, "the layout has no such output");
                Vec::new()
            });
            for placed in &output.placed {
                if placed.rect.overlaps(&output.rect) {
                    shown.insert(placed.window, (output.id, *placed));
                } else {
                    scrolled_out.insert(placed.window, *placed);
                }
            }
        }
        let mut windows: Vec<(WindowId, WlSurface)> = self
            .windows
            .keys()
            .filter_map(|&window| self.root_of(window).map(|root| (window, root)))
            .collect();
        // In the order they opened, so that a test reads configures in a stable order.
        windows.sort_by_key(|(window, _)| *window);
        for (window, root) in windows {
            match shown.get(&window) {
                Some(&(output, placed)) => {
                    self.configure(
                        &root,
                        Some(placed.configure),
                        placed.fullscreen,
                        placed.activated,
                        false,
                    );
                    self.move_to_output(&root, Some(output));
                }
                None => {
                    let size = scrolled_out.get(&window).map(|placed| placed.configure);
                    let fullscreen = self
                        .layout
                        .window(window)
                        .is_some_and(|info| info.fullscreen || info.fake_fullscreen);
                    self.configure(&root, size, fullscreen, false, true);
                    self.move_to_output(&root, None);
                    discard_presentation_feedback(&root);
                }
            }
        }
        self.focus_keyboard();
        self.clear_urgency_of_focused();
        self.arm_hidden_frames();
    }

    /// Records the output a toplevel is shown on, and sends its surfaces `wl_surface.enter` and
    /// `leave` when it changes.
    fn move_to_output(&mut self, root: &WlSurface, output: Option<OutputId>) {
        let Some(toplevel) = self.toplevels.get_mut(&root.id()) else {
            return;
        };
        let before = std::mem::replace(&mut toplevel.shown_on, output);
        if before == output {
            return;
        }
        if let Some(before) = before {
            self.leave_output(root, before);
        }
        if let Some(index) = output.and_then(|output| self.output_index(output)) {
            let smithay_output = &self.outputs[index].output;
            for tree in window_surface_trees(root) {
                with_surfaces_surface_tree(&tree, |surface, _| smithay_output.enter(surface));
            }
        }
    }

    /// Sends `wl_surface.leave` for `output` to every surface of the toplevel of `root` and of
    /// its popups.
    pub(crate) fn leave_output(&mut self, root: &WlSurface, output: OutputId) {
        if let Some(index) = self.output_index(output) {
            let smithay_output = &self.outputs[index].output;
            for tree in window_surface_trees(root) {
                with_surfaces_surface_tree(&tree, |surface, _| smithay_output.leave(surface));
            }
        }
    }

    /// Gives the keyboard to the window the owner's seat focuses, or to the topmost popup of a
    /// grab on it.
    pub(crate) fn focus_keyboard(&mut self) {
        let window = self
            .layout
            .focused(OWNER_SEAT)
            .and_then(|window| self.root_of(window));
        let focus = self.keyboard_focus(window);
        if self.keyboard.current_focus() != focus {
            let keyboard = self.keyboard.clone();
            keyboard.set_focus(self, focus, SERIAL_COUNTER.next_serial());
        }
    }

    /// Arms the hidden windows' deadline at the earliest time one of them may next get its frame
    /// callbacks; disarms it when none waits. A window the layout refused counts as hidden, so
    /// that its client does not wait for ever.
    pub(crate) fn arm_hidden_frames(&mut self) {
        let now = self.clock.now();
        let due = self
            .toplevels
            .values()
            .filter(|t| t.is_unseen())
            .filter(|t| waits_for_frame(t.wl_surface()))
            .map(|t| hidden_frame_due(t.last_frame_sent, now))
            .min();
        self.clock.disarm(Deadline::HiddenFrames);
        if let Some(at) = due {
            self.clock.arm(Deadline::HiddenFrames, at);
        }
    }

    /// The hidden windows' deadline: each hidden window whose last callbacks are a second old
    /// gets the frame callbacks it waits for. Its second starts again even if it had none to
    /// get, so that this deadline is never armed twice at one time.
    fn send_hidden_frames(&mut self) {
        let now = self.clock.now();
        let due: Vec<WlSurface> = self
            .toplevels
            .values()
            .filter(|t| t.is_unseen() && hidden_frame_due(t.last_frame_sent, now) <= now)
            .map(|t| t.wl_surface().clone())
            .collect();
        for root in due {
            self.send_frames(&root, now);
            if let Some(toplevel) = self.toplevels.get_mut(&root.id()) {
                toplevel.last_frame_sent = Some(now);
            }
        }
        self.arm_hidden_frames();
    }
}

/// What a refresh boundary's drawing did.
enum Drawing {
    Drawn,
    /// Nothing was damaged.
    Unchanged,
    /// The renderer failed: logged, and the content updates are not shown.
    Failed,
}

/// When a hidden window whose last frame callbacks went out at `last` may get its next ones.
fn hidden_frame_due(last: Option<Duration>, now: Duration) -> Duration {
    last.map_or(now, |last| {
        last.saturating_add(HIDDEN_FRAME_INTERVAL).max(now)
    })
}

/// Whether any drawn surface of the window whose toplevel surface is `root`, its popups included,
/// waits for a frame callback.
fn waits_for_frame(root: &WlSurface) -> bool {
    let mut waits = false;
    for_each_drawn_surface(root, |attributes| {
        waits |= !attributes.frame_callbacks.is_empty()
    });
    waits
}

/// Sends every frame callback the drawn surfaces of the window whose toplevel surface is `root`
/// wait for, its popups' included, with `time` in milliseconds as `wl_callback.done` carries it,
/// and returns how many were sent.
fn send_frame_callbacks(root: &WlSurface, time: Duration) -> u64 {
    // wl_callback's time is milliseconds in 32 bits, which wrap, as Wayland intends.
    let millis = time.as_millis() as u32;
    let mut sent = 0;
    for_each_drawn_surface(root, |attributes| {
        for callback in attributes.frame_callbacks.drain(..) {
            callback.done(millis);
            sent += 1;
        }
    });
    sent
}

/// Runs `f` on the current state of each surface that has a buffer, in the tree under `root`
/// and in its popups' trees. A surface without one is not drawn, nor are the surfaces under it:
/// their frame callbacks wait, as Smithay's own helpers have them wait.
fn for_each_drawn_surface(root: &WlSurface, mut f: impl FnMut(&mut SurfaceAttributes)) {
    for tree in window_surface_trees(root) {
        // The walk holds each surface's state while it visits it: the callbacks read the state
        // they are given, never through `with_states` again, which would wait for itself.
        with_surface_tree_downward(
            &tree,
            (),
            |_, states, ()| {
                if is_drawn(states) {
                    TraversalAction::DoChildren(())
                } else {
                    TraversalAction::SkipChildren
                }
            },
            |_, states, ()| {
                if is_drawn(states) {
                    f(states.cached_state.get::<SurfaceAttributes>().current());
                }
            },
            |_, _, ()| true,
        );
    }
}

/// Whether a surface has something to draw: a buffer, its size known.
fn is_drawn(states: &SurfaceData) -> bool {
    states
        .data_map
        .get::<RendererSurfaceStateUserData>()
        .and_then(|state| state.lock().ok().map(|state| state.view().is_some()))
        .unwrap_or(false)
}

/// Tells the clients of the window whose toplevel surface is `root`, its popups included, that
/// their content updates were not shown.
fn discard_presentation_feedback(root: &WlSurface) {
    for tree in window_surface_trees(root) {
        with_surfaces_surface_tree(&tree, |_, states| {
            if let Some(mut feedback) =
                SurfacePresentationFeedback::from_states(states, Kind::empty())
            {
                feedback.discarded();
            }
        });
    }
}
