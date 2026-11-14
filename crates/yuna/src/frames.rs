//! The frame loop ([ADR-0023](../../../docs/adr/0023-frames-presentation-and-the-clock.md),
//! [ADR-0031](../../../docs/adr/0031-frames-drawn-ahead-of-their-boundary.md)), and motion in it
//! ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
//!
//! A commit of a surface shown on an output, or a change of the layout, arms that output's next
//! refresh; so does a refresh while the output's motion runs; nothing is armed otherwise. The
//! refresh is due at once when the output is idle: no frame waits for its refresh, and one could
//! begin now: at a fixed rate, the boundary after its last refresh has passed; under VRR, the
//! panel can start a refresh a vertical blank from now, and, with nothing new to draw, a shortest
//! refresh after the last one
//! ([ADR-0051](../../../docs/adr/0051-emulated-vrr-as-the-monitors-traces-showed.md)).
//! Otherwise it is due at the refresh it can reach minus the output's *lead*, so that its
//! drawing ends before that refresh: at a fixed rate, the first boundary after its last refresh
//! whose time minus the lead is not yet past, and, while its motion runs, after the boundary its
//! last frame sampled (step 2 below). The lead is the longest drawing of the last
//! [`DRAWING_WINDOW`], each at least [`DRAWING_FLOOR`], plus [`LEAD_MARGIN`], at most one period;
//! a drawing is timed from its deadline to its end.
//!
//! At the refresh, in order:
//!
//! 1. If the layout changed, it is arranged once, for every output, and handed to the motion
//!    (`yuna_motion::Motion::update`), as an animated change or, after an output or a seat was
//!    removed, one instant for that seat's outputs alone, so that one seat's outputs coming and
//!    going never cut another seat's motion short
//!    ([ADR-0015](../../../docs/adr/0015-agents-workspaces.md)); an output added starts at rest.
//!    The windows are told their sizes and states.
//! 2. The motion gives the output's frame at the time it is presented when on time: the boundary
//!    the refresh aims at; for a refresh due at once, the boundary its drawing reaches if it takes
//!    as long as the output's longest drawing of the last [`DRAWING_WINDOW`], or after rest its
//!    last one, at most one period, without the lead's floor and margin: on the manual clock with
//!    no draw hook, the first boundary at or after now. While the output's motion runs, each
//!    refresh samples a later time than the output's last frame did, at a fixed rate the
//!    boundary after at least, so that the motion never steps back: a frame drawn at once whose
//!    drawing ended sooner than predicted shows before the boundary it sampled, and at a fixed
//!    rate that boundary shows it again. Each window is drawn at its rectangle and alpha then;
//!    with animations off, or at rest, that is the arrangement. The output is drawn if anything on
//!    it is damaged, and keeps what it drew, which captures cut.
//! 3. The windows it shows, and those it tells they are visible, get their frame callbacks. The
//!    presentation feedback of those it shows is taken and held until the boundary the frame is
//!    presented at: the one it aimed at if its drawing ended by then, else the first after. It is
//!    sent at that boundary, through a deadline of its own. The captures that waited for the
//!    refresh are copied after its drawing (`capture.rs`).
//! 4. While the motion is not over at the boundary sampled, the next refresh is armed as for
//!    something new, whether this one drew, found nothing changed or failed: the last frame is
//!    the arrangement, and then nothing is armed. Under VRR, a refresh of the motion that found
//!    nothing changed shows its frame again, so that the panel refreshes where the loop counts
//!    a refresh and the motion keeps the shortest refresh.
//!
//! What a client is told follows its place, not its drawn rectangle: a window is told it is
//! visible, `enter` and full-rate callbacks, when its place comes onto the output, at an
//! arrangement; it stays told while the motion draws it, and is told it is hidden only at the
//! refresh where its output's motion ends, if its place is then off the output. A window that
//! only passes through the view is drawn and called back, never told. So each window gets at most
//! one such change per operation, never one per frame.
//!
//! What follows what is drawn, hit tests, the pointer's focus and MCP's `windows`, reads an
//! output's motion now, or at the time its last frame sampled if that is later
//! (`VirtualOutput::motion_read_at`): never a motion that frame drew past. Once the refresh that
//! sampled a motion's end has run, they read the output at rest, as that frame shows it, though
//! the clock has not reached that end.
//!
//! A window shown on no output is hidden: it is told `suspended`, its content updates are
//! discarded, and its frame callbacks go out at most once a second, through one deadline armed
//! only while such a window waits for one. A window the layout refused to open is treated the
//! same way, so that its client never waits for ever.
//!
//! A window's popups go with it: drawn just above it, at its rectangle and alpha, called back,
//! reported presented or discarded, and entering or leaving outputs with its own surfaces.

use std::collections::{HashMap, HashSet, VecDeque};
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
use yuna_motion::{Change, Drawn};

use crate::capture::Failure;
use crate::clock::Deadline;
use crate::output::{RefreshAt, Timing, VirtualOutput};
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
    /// Marks the layout changed: every output arranges it again at its next refresh. The changes
    /// made until then combine ([`Change::and`]): one that must show at once on some outputs ends
    /// their motions at that arrangement, however many animated changes come with it.
    pub(crate) fn mark_layout_changed(&mut self, change: Change) {
        self.layout_dirty = true;
        self.layout_change = std::mem::take(&mut self.layout_change).and(change);
        let ids: Vec<OutputId> = self.outputs.iter().map(|o| o.id).collect();
        for id in ids {
            self.schedule_refresh_for_content(id);
        }
    }

    /// Arms `output`'s next refresh for something new to draw: under VRR, unpaced, and sooner
    /// than a refresh armed already for nothing new.
    pub(crate) fn schedule_refresh_for_content(&mut self, output: OutputId) {
        if let Some(index) = self.output_index(output) {
            self.outputs[index].content_pending = true;
        }
        self.schedule_refresh(output);
    }

    /// Arms `output`'s next refresh, unless it is armed already: at once when the output is
    /// idle, else at the boundary it aims at minus its lead. See the module's documentation.
    /// Under VRR, a refresh armed for nothing new is paced; something new that comes before it
    /// arms it again, sooner.
    pub(crate) fn schedule_refresh(&mut self, output: OutputId) {
        let deadline = Deadline::Refresh(output);
        let now = self.clock.now();
        let Some(index) = self.output_index(output) else {
            return;
        };
        let output = &mut self.outputs[index];
        let paced = !output.content_pending || output.content_misses >= 2;
        // At a fixed rate nothing is paced: a refresh armed stands, as before VRR.
        let variable = matches!(output.timing, Timing::Variable(_));
        if self.clock.is_armed(deadline) && (!variable || paced || output.armed_for_content) {
            return;
        }
        // While its motion runs, a frame is ready no sooner than just after the time its last
        // frame sampled, so that it samples a later refresh and the motion never steps back:
        // that frame, if drawn at once and faster than predicted, showed before the refresh it
        // sampled (ADR-0054). At rest, the frame is the arrangement whatever the time it
        // samples: a client's frames keep the soonest refresh they can reach.
        let ready_after = if output.in_motion {
            output.sampled_at.saturating_add(Duration::from_nanos(1))
        } else {
            Duration::ZERO
        };
        let period = output.mode.period();
        // At a fixed rate, the boundary after its last refresh has passed; under VRR, the panel
        // may start a refresh now: no frame waits for its own.
        let idle = output.timing.is_idle(now, output.last_refresh, paced);
        let (at, aimed, sampled) = if idle {
            // Drawn at once, a frame is shown at the first refresh after its drawing ends: its
            // motion is sampled there, its drawing predicted from the output's recent ones. Not
            // the lead, whose floor and margin keep a deadline safe rather than predict: a frame
            // shown before the refresh it sampled leaves that refresh nothing new to show.
            let drawing = output.drawings.expected(now, period);
            let ready = now.saturating_add(drawing).max(ready_after);
            (
                now,
                output.timing.reachable(now, output.last_refresh, paced),
                output.timing.reachable(ready, output.last_refresh, paced),
            )
        } else {
            let lead = output.drawings.lead(now, period);
            let ready = now.saturating_add(lead).max(ready_after);
            let ready = output.timing.reachable(ready, output.last_refresh, paced);
            (ready.at.saturating_sub(lead), ready, ready)
        };
        if self.clock.is_armed(deadline) && output.armed_at <= at {
            // The refresh armed for nothing new comes first already: it draws the new content.
            output.armed_for_content = true;
            return;
        }
        output.armed_for_content = !paced;
        output.armed_refresh = aimed;
        output.armed_sample_at = sampled.at;
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
            if std::mem::take(&mut self.committed_content) {
                self.schedule_refresh_for_content(output);
            } else {
                self.schedule_refresh(output);
            }
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
        if self.layout_dirty {
            self.arrange();
        }
        let Some(index) = self.output_index(output) else {
            return;
        };
        let sample_at = self.outputs[index].armed_sample_at;
        let frame = self.motion.frame(output, sample_at);
        self.draw_and_present(index, frame.windows, sample_at);
        let Some(index) = self.output_index(output) else {
            return;
        };
        if self.motion.end(output, sample_at).is_some() {
            // The next boundary has a frame of its own, whether this one drew, found nothing
            // changed or failed: a motion never stops short of its end.
            self.outputs[index].in_motion = true;
            self.schedule_refresh_for_content(output);
        } else if std::mem::take(&mut self.outputs[index].in_motion) {
            self.tell_motion_ended(output, sample_at);
        }
    }

    /// Draws output `index`'s frame, `drawn` as the motion gives it, if anything on it is
    /// damaged; calls back the surfaces it shows or tells they are visible; holds their
    /// presentation feedback until the frame's boundary; and copies the captures that waited.
    fn draw_and_present(&mut self, index: usize, drawn: Vec<Drawn>, sample_at: Duration) {
        let output_id = self.outputs[index].id;
        // What waited is drawn now, or found unchanged.
        let armed_for_content = std::mem::take(&mut self.outputs[index].armed_for_content);
        self.outputs[index].content_pending = false;
        let output_rect = self.outputs[index].rect;
        let shown: Vec<Drawn> = drawn
            .iter()
            .filter(|window| window.rect.overlaps(&output_rect))
            .copied()
            .collect();
        let drawing = self.draw(index, &shown);
        // Readers follow what the buffer holds: a failed drawing leaves the frame before.
        if !matches!(drawing, Drawing::Failed) {
            self.outputs[index].drawn = drawn;
            self.outputs[index].sampled_at = sample_at;
        }
        let now = self.clock.now();
        for window in self.called_back(index, &shown) {
            if let Some(root) = self.root_of(window) {
                self.send_frames(&root, now);
            }
        }
        let drew = matches!(drawing, Drawing::Drawn { .. });
        let misses = &mut self.outputs[index].content_misses;
        *misses = match (drew, armed_for_content) {
            (true, _) => 0,
            (false, true) => misses.saturating_add(1),
            (false, false) => *misses,
        };
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
                    presented_at_us = presented.at.as_micros() as u64,
                    "frame drawn"
                );
                presented
            }
            // While a motion runs, a VRR panel is shown the same frame again, so that it refreshes
            // where this refresh is counted and keeps the motion's pace: shown nothing, it would
            // show the next frame sooner than a shortest refresh after this one, and after a few
            // such refreshes insert its own below its range.
            Drawing::Unchanged
                if output.in_motion && matches!(output.timing, Timing::Variable(_)) =>
            {
                output.timing.present(now, aimed, output.last_refresh)
            }
            // Nothing new to show: content updates show with the frame this refresh aimed at.
            Drawing::Unchanged => aimed,
            Drawing::Failed => {
                output.last_refresh = later(output.last_refresh, aimed);
                for drawn in &shown {
                    if let Some(root) = self.root_of(drawn.window) {
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
        for drawn in &shown {
            if let Some(root) = self.root_of(drawn.window) {
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
    fn draw(&mut self, index: usize, shown: &[Drawn]) -> Drawing {
        // From here, so that a frame's time includes the import of its clients' buffers, which
        // Smithay does while it builds their elements: a `wl_shm` buffer is copied then.
        let started = Instant::now();
        let output = &self.outputs[index];
        let mut elements = Vec::new();
        // The motion gives windows bottom to top; the renderer takes them front to back.
        for drawn in shown.iter().rev() {
            let Some(toplevel) = self
                .windows
                .get(&drawn.window)
                .and_then(|id| self.toplevels.get(id))
            else {
                continue;
            };
            // Within 0 and 1 already; a renderer's alpha is an `f32`.
            let alpha = drawn.alpha as f32;
            // A window's popups are in front of it, behind the windows over it, and go with it.
            elements.extend(render::popup_elements(
                &mut self.renderer,
                toplevel.wl_surface(),
                drawn.rect,
                alpha,
                output,
            ));
            elements.extend(render::window_elements(
                &mut self.renderer,
                toplevel.wl_surface(),
                toplevel.geometry,
                drawn.rect,
                alpha,
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

    /// The windows output `index` calls back at its refresh, at full rate: those its frame
    /// shows, bottom to top, then those it tells they are visible that the frame does not show
    /// yet ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
    fn called_back(&self, index: usize, shown: &[Drawn]) -> Vec<WindowId> {
        let on_frame: HashSet<WindowId> = shown.iter().map(|drawn| drawn.window).collect();
        let not_on_frame = self.outputs[index]
            .told
            .iter()
            .filter(|window| !on_frame.contains(window));
        shown
            .iter()
            .map(|drawn| drawn.window)
            .chain(not_on_frame.copied())
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

    /// Arranges every output, turns what moved into motion, tells each window its size and
    /// states, and moves each between shown and hidden.
    pub(crate) fn arrange(&mut self) {
        self.layout_dirty = false;
        let change = std::mem::take(&mut self.layout_change);
        let now = self.clock.now();
        self.motion.update(&self.layout, now, change);
        for output in &mut self.outputs {
            output.placed = self.layout.arrange(output.id).unwrap_or_else(|error| {
                tracing::warn!(%error, output = output.id.0, "the layout has no such output");
                Vec::new()
            });
            output.told = self.motion.told_visible(output.id, now);
            output.in_motion = self.motion.end(output.id, now).is_some();
        }
        self.tell_windows();
        self.focus_keyboard();
        self.clear_urgency_of_focused();
        self.arm_hidden_frames();
        // Windows may have moved to another output, with other colours.
        self.update_preferred_descriptions();
    }

    /// The refresh where `output`'s motion ended, its last frame sampled at `at`: the windows it
    /// told while it moved are told what their places say now, once, and its seat's pointer, if
    /// on it, is put on what is under it now that windows stopped, as hit tests read the output
    /// from then on, at rest
    /// ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
    fn tell_motion_ended(&mut self, output: OutputId, at: Duration) {
        let Some(index) = self.output_index(output) else {
            return;
        };
        self.outputs[index].told = self.motion.told_visible(output, at);
        self.tell_windows();
        self.arm_hidden_frames();
        self.update_preferred_descriptions();
        let seat = self.outputs[index].seat;
        if self.pointer_output(seat) == Some(output) {
            self.refresh_pointer_focus(seat);
        }
    }

    /// Tells each window its size and states, and moves each between shown and hidden, as each
    /// output's `told` says: a window told it is visible there enters it and is told its place's
    /// size and states; any other is hidden: told `suspended`, it leaves its output and its
    /// content updates are discarded.
    fn tell_windows(&mut self) {
        let mut told: HashMap<WindowId, (OutputId, Option<Placed>)> = HashMap::new();
        let mut placed_anywhere: HashMap<WindowId, Placed> = HashMap::new();
        for output in &self.outputs {
            let places: HashMap<WindowId, Placed> = output
                .placed
                .iter()
                .map(|placed| (placed.window, *placed))
                .collect();
            for &window in &output.told {
                told.insert(window, (output.id, places.get(&window).copied()));
            }
            placed_anywhere.extend(places);
        }
        let mut windows: Vec<(WindowId, WlSurface)> = self
            .windows
            .keys()
            .filter_map(|&window| self.root_of(window).map(|root| (window, root)))
            .collect();
        // In the order they opened, so that a test reads configures in a stable order.
        windows.sort_by_key(|(window, _)| *window);
        for (window, root) in windows {
            match told.get(&window) {
                Some(&(output, placed)) => {
                    // The output, and with it its scale, before the configure: the client then
                    // draws the buffer that acknowledges it at the right scale.
                    self.move_to_output(&root, Some(output));
                    match placed {
                        Some(placed) => self.configure(
                            &root,
                            Some(placed.configure),
                            placed.fullscreen,
                            placed.activated,
                            false,
                        ),
                        // Still drawn while it slides out of view, no longer arranged: visible
                        // until the motion ends, at the size it has.
                        None => {
                            let fullscreen = self.is_told_fullscreen(window);
                            self.configure(&root, None, fullscreen, false, false);
                        }
                    }
                }
                None => {
                    let size = placed_anywhere.get(&window).map(|placed| placed.configure);
                    let fullscreen = self.is_told_fullscreen(window);
                    self.configure(&root, size, fullscreen, false, true);
                    self.move_to_output(&root, None);
                    discard_presentation_feedback(&root);
                }
            }
        }
    }

    /// Whether `window`'s client is told it is fullscreen, as the layout has it: it is, or it is
    /// fake fullscreen.
    fn is_told_fullscreen(&self, window: WindowId) -> bool {
        self.layout
            .window(window)
            .is_some_and(|info| info.fullscreen || info.fake_fullscreen)
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
    /// that windows moved, unless they move on its output: then its focus waits for the next
    /// pointer event or the motion's end (ADR-0054). Only here does a keyboard change window:
    /// between boundaries, `popups.rs` moves it within the window it is on, and a click moves the
    /// seat's focus.
    fn focus_keyboard(&mut self) {
        for seat in self.seat_ids() {
            let window = self
                .layout
                .focused(seat)
                .and_then(|window| self.root_of(window));
            self.give_keyboard_to(seat, window);
            if !self.pointer_in_motion(seat) {
                self.refresh_pointer_focus(seat);
            }
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
    /// How long the last drawing took, however long ago it ended.
    last: Option<Duration>,
}

impl Drawings {
    /// Records a drawing that ended at `ended` and took `took` from its deadline.
    pub(crate) fn record(&mut self, ended: Duration, took: Duration) {
        self.forget_old(ended);
        self.recent.push_back((ended, took));
        self.last = Some(took);
    }

    /// The lead at `now` of an output of refresh period `period`: the longest drawing of the
    /// last [`DRAWING_WINDOW`], at least [`DRAWING_FLOOR`], plus [`LEAD_MARGIN`], at most
    /// `period`.
    pub(crate) fn lead(&mut self, now: Duration, period: Duration) -> Duration {
        self.longest(now)
            .max(DRAWING_FLOOR)
            .saturating_add(LEAD_MARGIN)
            .min(period)
    }

    /// The longest drawing of the last [`DRAWING_WINDOW`] at `now`, as it took; zero when there
    /// was none.
    pub(crate) fn longest(&mut self, now: Duration) -> Duration {
        self.forget_old(now);
        self.recent
            .iter()
            .map(|&(_, took)| took)
            .max()
            .unwrap_or(Duration::ZERO)
    }

    /// How long a drawing starting at `now` is expected to take, on an output of refresh period
    /// `period`: the longest of the last [`DRAWING_WINDOW`], or after rest the last one, however
    /// old; zero before any drawing; at most `period`, as the lead is, so that one slow drawing
    /// never has a frame drawn at once sample several refreshes ahead.
    pub(crate) fn expected(&mut self, now: Duration, period: Duration) -> Duration {
        let longest = self.longest(now);
        let expected = if self.recent.is_empty() {
            self.last.unwrap_or(Duration::ZERO)
        } else {
            longest
        };
        expected.min(period)
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
        assert_eq!(drawings.longest(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn the_longest_recent_drawing_has_neither_floor_nor_margin() {
        let mut drawings = Drawings::default();
        let t = Duration::from_secs(100);
        drawings.record(t, MS / 2);
        drawings.record(t + 4 * MS, MS / 4);
        assert_eq!(drawings.longest(t + 5 * MS), MS / 2);
        assert_eq!(drawings.longest(t + DRAWING_WINDOW), MS / 4);
        assert_eq!(
            drawings.longest(t + 4 * MS + DRAWING_WINDOW),
            Duration::ZERO
        );
    }

    #[test]
    fn after_rest_a_drawing_is_expected_to_take_as_long_as_the_last() {
        let mut drawings = Drawings::default();
        assert_eq!(
            drawings.expected(Duration::ZERO, PERIOD_240_HZ),
            Duration::ZERO
        );
        let t = Duration::from_secs(100);
        drawings.record(t, 3 * MS);
        drawings.record(t + 4 * MS, MS);
        assert_eq!(drawings.expected(t + 5 * MS, PERIOD_240_HZ), 3 * MS);
        assert_eq!(
            drawings.expected(t + 60 * DRAWING_WINDOW, PERIOD_240_HZ),
            MS
        );
    }

    #[test]
    fn a_drawing_is_expected_to_take_at_most_one_period() {
        let mut drawings = Drawings::default();
        let t = Duration::from_secs(100);
        drawings.record(t, 20 * MS);
        assert_eq!(drawings.expected(t + MS, PERIOD_240_HZ), PERIOD_240_HZ);
        assert_eq!(
            drawings.expected(t + 60 * DRAWING_WINDOW, PERIOD_240_HZ),
            PERIOD_240_HZ
        );
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
