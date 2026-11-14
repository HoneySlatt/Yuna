//! The frame loop ([ADR-0023](../../../docs/adr/0023-frames-presentation-and-the-clock.md),
//! [ADR-0031](../../../docs/adr/0031-frames-drawn-ahead-of-their-boundary.md)).
//!
//! A commit of a surface shown on an output, or a change of the layout, arms that output's next
//! refresh; nothing is armed otherwise. The refresh is due at once when the output is idle: no
//! frame waits for its boundary, and the boundary after its last refresh has passed. Otherwise
//! it is due at a boundary minus the output's *lead*, so that its drawing ends before the
//! boundary: the first boundary after its last refresh whose time minus the lead is not yet
//! past. The lead is the longest drawing of the last [`DRAWING_WINDOW`], each at least
//! [`DRAWING_FLOOR`], plus [`LEAD_MARGIN`], at most one period; a drawing is timed from its
//! deadline to its end.
//!
//! At the refresh, in order: the layout is arranged once if it changed, and the configures that
//! changed are sent; the output is drawn if anything on it is damaged; its shown surfaces get
//! their frame callbacks. Their presentation feedback is taken then and held until the boundary
//! the frame is presented at: the one it aimed at if its drawing ended by then, else the first
//! after. It is sent at that boundary, through a deadline of its own. The captures that waited
//! for the refresh are copied after its drawing (`capture.rs`).
//!
//! A window shown on no output is hidden: it is told `suspended`, its content updates are
//! discarded, and its frame callbacks go out at most once a second, through one deadline armed
//! only while such a window waits for one. A window the layout refused to open is treated the
//! same way, so that its client never waits for ever.
//!
//! A window's popups go with it: drawn just above it, called back, reported presented or
//! discarded, and entering or leaving outputs with its own surfaces.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

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
use smithay::wayland::presentation::PresentationFeedbackCachedState;
use yuna_layout::{OutputId, Placed, SeatId, WindowId};

use crate::capture::Failure;
use crate::clock::Deadline;
use crate::output::{RefreshAt, VirtualOutput};
use crate::popups::window_surface_trees;
use crate::render;
use crate::seats::seat_of_surface;
use crate::state::State;

/// The later of two refreshes, by number.
fn later(a: RefreshAt, b: RefreshAt) -> RefreshAt {
    if b.seq > a.seq { b } else { a }
}

/// The shortest time between two frame callbacks of a hidden window.
pub const HIDDEN_FRAME_INTERVAL: Duration = Duration::from_secs(1);

/// How long a drawing counts in its output's lead after it ended.
pub const DRAWING_WINDOW: Duration = Duration::from_millis(500);

/// The least a drawing counts in its output's lead, KWin's: power states of a CPU or a GPU may
/// slow the next drawing ([ADR-0031](../../../docs/adr/0031-frames-drawn-ahead-of-their-boundary.md)).
pub const DRAWING_FLOOR: Duration = Duration::from_millis(2);

/// What the lead keeps beyond the longest recent drawing, KWin's.
pub const LEAD_MARGIN: Duration = Duration::from_millis(1);

impl State {
    /// Marks the layout changed: every output arranges it again at its next refresh.
    pub(crate) fn mark_layout_changed(&mut self) {
        self.layout_dirty = true;
        let ids: Vec<OutputId> = self.outputs.iter().map(|o| o.id).collect();
        for id in ids {
            self.schedule_refresh(id);
        }
    }

    /// Arms `output`'s next refresh, unless it is armed already: at once when the output is
    /// idle, else at the boundary it aims at minus its lead. See the module's documentation.
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
        // At a fixed rate, the boundary after its last refresh has passed; under VRR, the panel
        // may start a refresh now: no frame waits for its own.
        let idle = output.timing.is_idle(now, output.last_refresh);
        let (at, aimed) = if idle {
            (now, output.timing.reachable(now, output.last_refresh))
        } else {
            let lead = output.drawings.lead(now, output.mode.period());
            let aimed = output
                .timing
                .reachable(now.saturating_add(lead), output.last_refresh);
            (aimed.at.saturating_sub(lead), aimed)
        };
        output.armed_refresh = aimed;
        output.armed_at = at;
        output.armed_at_once = idle;
        self.clock.arm(deadline, at);
    }

    /// What a commit of `surface`, in the toplevel of `root`, arms: its output's next refresh
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
            Deadline::Presentation(output) => self.send_presentations(output),
            Deadline::HiddenFrames => self.send_hidden_frames(),
        }
    }

    /// An output's refresh. See the module's documentation.
    fn refresh(&mut self, output: OutputId) {
        let output_id = output;
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
        let drew = matches!(drawing, Drawing::Drawn { .. });
        let output = &mut self.outputs[index];
        let aimed = output.armed_refresh;
        let presented = match drawing {
            Drawing::Drawn { started, damaged } => {
                let took = now.saturating_sub(output.armed_at);
                output.drawings.record(now, took);
                // When the drawing could first show, and never before the refresh it aimed at.
                let presented = output.timing.present(now, aimed, output.last_refresh);
                tracing::debug!(
                    output = output.id.0,
                    elements = shown.len(),
                    damaged_rectangles = damaged,
                    took_us = started.elapsed().as_micros(),
                    lead_us = aimed.at.saturating_sub(output.armed_at).as_micros(),
                    at_once = output.armed_at_once,
                    boundary = aimed.seq,
                    presented = presented.seq,
                    "frame drawn"
                );
                presented
            }
            // Nothing new to show: content updates show with the frame this refresh aimed at.
            Drawing::Unchanged => aimed,
            Drawing::Failed => {
                output.last_refresh = later(output.last_refresh, aimed);
                for placed in &shown {
                    if let Some(root) = self.root_of(placed.window) {
                        discard_presentation_feedback(&root);
                    }
                }
                self.fail_captures(output_id, "its frame could not be drawn", Failure::ForNow);
                return;
            }
        };
        output.last_refresh = later(output.last_refresh, presented);
        // The frame shown from `presented` on is in the buffer now: the captures that waited.
        self.copy_captures(output_id, drew, presented);
        let output = &mut self.outputs[index];
        let smithay_output = output.output.clone();
        let mut feedback = OutputPresentationFeedback::new(&smithay_output);
        let mut any = false;
        for placed in &shown {
            if let Some(root) = self.root_of(placed.window) {
                for tree in window_surface_trees(&root) {
                    take_presentation_feedback_surface_tree(
                        &tree,
                        &mut feedback,
                        |_, _| Some(smithay_output.clone()),
                        |_, states| {
                            any |= has_presentation_feedback(states);
                            Kind::empty()
                        },
                    );
                }
            }
        }
        // Nothing to tell: no deadline, so that a frame nobody asked feedback for wakes nothing.
        if !any {
            return;
        }
        let output = &mut self.outputs[index];
        output.held_feedback.push((presented, feedback));
        self.clock
            .arm(Deadline::Presentation(output.id), presented.at);
    }

    /// An output's presentation deadline: the feedback held for each boundary now reached is
    /// sent, presented at that boundary; the deadline is armed again for the next one held.
    fn send_presentations(&mut self, output: OutputId) {
        let now = self.clock.now();
        let Some(index) = self.output_index(output) else {
            return;
        };
        let VirtualOutput {
            timing,
            held_feedback,
            mode,
            ..
        } = &mut self.outputs[index];
        let refresh = timing.refresh(mode.period());
        let mut next = None;
        held_feedback.retain_mut(|(presented, feedback)| {
            let at = presented.at;
            if at > now {
                next = Some(next.map_or(at, |next: Duration| next.min(at)));
                return true;
            }
            feedback.presented::<Duration, Monotonic>(at, refresh, presented.seq, Kind::Vsync);
            false
        });
        if let Some(at) = next {
            self.clock.arm(Deadline::Presentation(output), at);
        }
    }

    /// Draws output `index`, which shows `shown`, if anything on it is damaged.
    fn draw(&mut self, index: usize, shown: &[Placed]) -> Drawing {
        // From here, so that a frame's time includes the import of its clients' buffers, which
        // Smithay does while it builds their elements: a `wl_shm` buffer is copied then.
        let started = Instant::now();
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
            Ok(Some(damaged)) => {
                self.counters.frames_drawn += 1;
                self.outputs[index].drawn_frames += 1;
                if let Some(hook) = &mut self.draw_hook {
                    hook(&mut self.clock);
                }
                Drawing::Drawn { started, damaged }
            }
            Ok(None) => Drawing::Unchanged,
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
                    // The output, and with it its scale, before the configure: the client then
                    // draws the buffer that acknowledges it at the right scale.
                    self.move_to_output(&root, Some(output));
                    self.configure(
                        &root,
                        Some(placed.configure),
                        placed.fullscreen,
                        placed.activated,
                        false,
                    );
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
        // Windows may have moved to another output, with other colours.
        self.update_preferred_descriptions();
    }

    /// Records the output a toplevel is shown on, and sends its surfaces `wl_surface.enter` and
    /// `leave` when it changes, and the new output's scale (`scale.rs`).
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
        if let Some(output) = output
            && let Some(index) = self.output_index(output)
        {
            let smithay_output = &self.outputs[index].output;
            for tree in window_surface_trees(root) {
                with_surfaces_surface_tree(&tree, |surface, _| smithay_output.enter(surface));
                self.tell_preferred_scale(&tree, output);
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

    /// At a refresh boundary, gives each seat's keyboard to the window the seat focuses, or to
    /// the topmost popup of a grab on it; and puts each seat's pointer on what is under it now
    /// that windows moved. Only here does a keyboard change window: between boundaries,
    /// `popups.rs` moves it within the window it is on, and a click moves the seat's focus.
    fn focus_keyboard(&mut self) {
        for seat in self.seat_ids() {
            let window = self
                .layout
                .focused(seat)
                .and_then(|window| self.root_of(window));
            self.give_keyboard_to(seat, window);
            self.refresh_pointer_focus(seat);
        }
    }

    /// Gives `seat`'s keyboard to the toplevel surface `window`, or to the topmost popup of a
    /// grab on it, and the seat's selections with it: the selections first, for
    /// `wl_data_device` asks that a client be told the selection just before its keyboard
    /// `enter`. A window of another seat's client is never given it. Every change of a keyboard
    /// focus goes through here.
    pub(crate) fn give_keyboard_to(&mut self, seat: SeatId, window: Option<WlSurface>) {
        let window = window.filter(|window| seat_of_surface(window) == seat);
        let focus = self.keyboard_focus(seat, window);
        let Some(keyboard) = self.seat(seat).map(|data| data.keyboard.clone()) else {
            return;
        };
        if keyboard.current_focus() != focus {
            self.give_selections_to(seat, focus.as_ref());
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

/// What a refresh's drawing did.
enum Drawing {
    /// Drawn, from `started`, when its elements began to be built, which imports its clients'
    /// new buffers; `damaged` rectangles.
    Drawn { started: Instant, damaged: usize },
    /// Nothing was damaged.
    Unchanged,
    /// The renderer failed: logged, and the content updates are not shown.
    Failed,
}

/// An output's recent drawings, from which its lead is predicted.
#[derive(Debug, Default)]
pub(crate) struct Drawings {
    /// When each drawing ended and how long it took from its deadline, oldest first, none
    /// ended [`DRAWING_WINDOW`] or longer ago: at most one a boundary, so a scan stays short.
    recent: VecDeque<(Duration, Duration)>,
}

impl Drawings {
    /// Records a drawing that ended at `ended` and took `took` from its deadline.
    pub(crate) fn record(&mut self, ended: Duration, took: Duration) {
        self.forget_old(ended);
        self.recent.push_back((ended, took));
    }

    /// The lead at `now` of an output of refresh period `period`: the longest drawing of the
    /// last [`DRAWING_WINDOW`], at least [`DRAWING_FLOOR`], plus [`LEAD_MARGIN`], at most
    /// `period`.
    pub(crate) fn lead(&mut self, now: Duration, period: Duration) -> Duration {
        self.forget_old(now);
        let longest = self
            .recent
            .iter()
            .map(|&(_, took)| took)
            .max()
            .unwrap_or(Duration::ZERO)
            .max(DRAWING_FLOOR);
        longest.saturating_add(LEAD_MARGIN).min(period)
    }

    fn forget_old(&mut self, now: Duration) {
        while self
            .recent
            .front()
            .is_some_and(|&(ended, _)| ended.saturating_add(DRAWING_WINDOW) <= now)
        {
            self.recent.pop_front();
        }
    }
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

/// Whether a surface waits for presentation feedback.
fn has_presentation_feedback(states: &SurfaceData) -> bool {
    !states
        .cached_state
        .get::<PresentationFeedbackCachedState>()
        .current()
        .callbacks
        .is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);
    const PERIOD_240_HZ: Duration = Duration::from_nanos(4_166_666);

    #[test]
    fn with_no_drawing_the_lead_is_the_floor_and_the_margin() {
        let mut drawings = Drawings::default();
        assert_eq!(drawings.lead(Duration::ZERO, PERIOD_240_HZ), 3 * MS);
    }

    #[test]
    fn the_lead_is_the_longest_recent_drawing_and_the_margin() {
        let mut drawings = Drawings::default();
        let t = Duration::from_secs(100);
        drawings.record(t, 2_500 * Duration::from_micros(1));
        drawings.record(t + 4 * MS, MS);
        assert_eq!(
            drawings.lead(t + 5 * MS, PERIOD_240_HZ),
            3_500 * Duration::from_micros(1)
        );
    }

    #[test]
    fn the_lead_is_at_most_one_period() {
        let mut drawings = Drawings::default();
        let t = Duration::from_secs(100);
        drawings.record(t, 50 * MS);
        assert_eq!(drawings.lead(t, PERIOD_240_HZ), PERIOD_240_HZ);
    }

    #[test]
    fn a_drawing_stops_counting_half_a_second_after_it_ended() {
        let mut drawings = Drawings::default();
        let t = Duration::from_secs(100);
        drawings.record(t, 3 * MS);
        let just_before = t + DRAWING_WINDOW - Duration::from_nanos(1);
        assert_eq!(drawings.lead(just_before, PERIOD_240_HZ), 4 * MS);
        assert_eq!(drawings.lead(t + DRAWING_WINDOW, PERIOD_240_HZ), 3 * MS);
        assert!(drawings.recent.is_empty());
    }
}
