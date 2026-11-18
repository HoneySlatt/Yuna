//! Virtual outputs: outputs of any size, rate and scale that no monitor shows, drawn offscreen
//! ([ADR-0022](../../../docs/adr/0022-the-headless-backend-in-yuna.md)), and when they refresh:
//! at fixed boundaries ([ADR-0023](../../../docs/adr/0023-frames-presentation-and-the-clock.md)),
//! or as a VRR panel (`vrr.rs`).

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesRenderbuffer;
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Transform;
use smithay::wayland::presentation::Refresh;
use yuna_layout::geometry::{Point, Rect};
use yuna_layout::{OutputConfig, OutputId, Placed, SeatId, WindowId};
use yuna_motion::{Change, Drawn};

use crate::Error;
use crate::capture::Failure;
use crate::clock::Deadline;
use crate::colour::Record;
use crate::colour::description::{Monitor, sdr_description};
use crate::compositor::OutputDescription;
use crate::frames::{Drawings, FailedDrawings};
use crate::state::State;
use crate::vrr::Panel;

/// The slowest refresh rate accepted, in Hz.
const MIN_RATE: f64 = 1.0;
/// The fastest refresh rate accepted, in Hz.
const MAX_RATE: f64 = 1_000.0;
/// The smallest scale accepted.
const MIN_SCALE: f64 = 0.25;
/// The largest scale accepted.
const MAX_SCALE: f64 = 8.0;
/// Picoseconds in a second over millihertz: a refresh period in nanoseconds is this over the
/// rate in millihertz.
const NANOS_PER_SECOND_IN_MILLIHERTZ: u128 = 1_000_000_000_000;

/// A virtual output's size, refresh rate and scale, checked by [`OutputMode::new`]; the least
/// rate of its VRR range, if it runs under VRR ([`OutputMode::with_vrr`]); and the monitor
/// whose HDR colours it announces, if any ([`OutputMode::with_hdr`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputMode {
    width: u32,
    height: u32,
    refresh_millihertz: u32,
    scale: f64,
    vrr_min_millihertz: Option<u32>,
    hdr: Option<Monitor>,
}

/// Why an output's mode was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidOutput(pub String);

impl fmt::Display for InvalidOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidOutput {}

impl OutputMode {
    /// A mode of `width` × `height` physical pixels, refreshed `rate` times a second, at
    /// `scale` physical pixels per logical unit. Refused unless both sides are positive, the
    /// rate between 1 and 1,000 Hz and the scale between 0.25 and 8. The renderer's own limit
    /// on sizes is checked when the output is created.
    pub fn new(
        width: u32,
        height: u32,
        rate: f64,
        scale: f64,
    ) -> Result<OutputMode, InvalidOutput> {
        if width == 0 || height == 0 {
            return Err(InvalidOutput(format!(
                "an output of {width} × {height} pixels has no area"
            )));
        }
        if !(MIN_RATE..=MAX_RATE).contains(&rate) {
            return Err(InvalidOutput(format!(
                "a refresh rate of {rate} Hz is outside {MIN_RATE}–{MAX_RATE} Hz"
            )));
        }
        if !(MIN_SCALE..=MAX_SCALE).contains(&scale) {
            return Err(InvalidOutput(format!(
                "a scale of {scale} is outside {MIN_SCALE}–{MAX_SCALE}"
            )));
        }
        // Within 1–1,000 Hz, the rate in millihertz fits a u32 and is at least 1,000.
        let refresh_millihertz = (rate * 1_000.0).round() as u32;
        Ok(OutputMode {
            width,
            height,
            refresh_millihertz,
            scale,
            vrr_min_millihertz: None,
            hdr: None,
        })
    }

    /// The same mode under VRR, over `min_rate` to its rate, in Hz
    /// ([ADR-0046](../../../docs/adr/0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)).
    /// Refused unless its rate is at least twice `min_rate`: a narrower range takes a path of
    /// amdgpu's that Yuna does not emulate.
    pub fn with_vrr(self, min_rate: f64) -> Result<OutputMode, InvalidOutput> {
        let rate = self.rate();
        if !(MIN_RATE..=rate / 2.0).contains(&min_rate) {
            return Err(InvalidOutput(format!(
                "a VRR range of {min_rate} to {rate} Hz is refused: its least rate must be \
                 at least {MIN_RATE} Hz and at most half its rate"
            )));
        }
        Ok(OutputMode {
            vrr_min_millihertz: Some((min_rate * 1_000.0).round() as u32),
            ..self
        })
    }

    /// The least rate of its VRR range, in millihertz; none at a fixed rate.
    pub fn vrr_min_millihertz(&self) -> Option<u32> {
        self.vrr_min_millihertz
    }

    /// The same mode, announcing `monitor`'s HDR colours
    /// ([ADR-0048](../../../docs/adr/0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md)).
    pub fn with_hdr(self, monitor: Monitor) -> OutputMode {
        OutputMode {
            hdr: Some(monitor),
            ..self
        }
    }

    /// The monitor whose HDR colours it announces; none for an SDR output.
    pub fn hdr(&self) -> Option<Monitor> {
        self.hdr
    }

    /// Its width in physical pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Its height in physical pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Its refresh rate in millihertz, as Wayland gives it.
    pub fn refresh_millihertz(&self) -> u32 {
        self.refresh_millihertz
    }

    /// Its refresh rate in Hz.
    pub fn rate(&self) -> f64 {
        hertz(self.refresh_millihertz)
    }

    /// The least rate of its VRR range, in Hz; none at a fixed rate.
    pub fn vrr_min(&self) -> Option<f64> {
        self.vrr_min_millihertz.map(hertz)
    }

    /// Its scale: physical pixels per logical unit.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// The time between two refresh boundaries, to the nanosecond below.
    pub fn period(&self) -> Duration {
        let nanos = NANOS_PER_SECOND_IN_MILLIHERTZ / u128::from(self.refresh_millihertz);
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }
}

impl Default for OutputMode {
    /// 3840 × 2160 at 240 Hz and scale 1.5: an agent's output (ADR-0015, ADR-0022).
    fn default() -> OutputMode {
        OutputMode {
            width: 3840,
            height: 2160,
            refresh_millihertz: 240_000,
            scale: 1.5,
            vrr_min_millihertz: None,
            hdr: None,
        }
    }
}

/// A virtual output as the compositor holds it.
#[derive(Debug)]
pub(crate) struct VirtualOutput {
    pub id: OutputId,
    /// The seat it belongs to, whose workspaces it shows.
    pub seat: SeatId,
    /// Smithay's output: what `wl_output` and `xdg-output` tell clients.
    pub output: Output,
    /// Its `wl_output` global.
    pub global: GlobalId,
    pub mode: OutputMode,
    /// Its place in the layout's logical coordinates.
    pub rect: Rect,
    /// What it is drawn into: `Abgr8888` at its physical size, its format its own, so that an
    /// output can later be HDR (ADR-0022).
    pub buffer: GlesRenderbuffer,
    pub damage: OutputDamageTracker,
    /// Whether `buffer` holds the last frame drawn, so that only damage needs drawing.
    pub drawn_once: bool,
    /// How many frames were drawn on it: a capture session's later frame waits for new content
    /// only while this is what it was at the session's last frame.
    pub drawn_frames: u64,
    /// The colours it announces (`colour/`), new when its HDR is turned on or off.
    pub colour: Arc<Record>,
    /// When it refreshes: at fixed boundaries from its creation or the last change of its rate,
    /// or as a VRR panel.
    pub timing: Timing,
    /// The refresh its armed refresh aims at, and when that refresh is due.
    pub armed_refresh: RefreshAt,
    pub armed_at: Duration,
    /// When the frame of its armed refresh is presented if on time, the time its motion is
    /// sampled at: the boundary it aims at, or for a refresh armed at once, the one its drawing
    /// reaches if it takes as long as the longest of the last
    /// [`DRAWING_WINDOW`](crate::frames::DRAWING_WINDOW), or after rest the last one, at most one
    /// period. While its motion runs, it is later than `sampled_at`
    /// ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
    pub armed_sample_at: Duration,
    /// The time its last frame sampled its motion at, the frame its buffer holds: that of its
    /// last refresh whose drawing did not fail; zero before its first.
    pub sampled_at: Duration,
    /// Whether that refresh was armed at once, the output being idle (ADR-0031).
    pub armed_at_once: bool,
    /// Whether something new waits to be drawn: a commit with a buffer or damage, or a change of
    /// the layout. Under VRR, a refresh with nothing new is paced; one with something is not.
    pub content_pending: bool,
    /// Whether the armed refresh was armed for something new, unpaced.
    pub armed_for_content: bool,
    /// How many refreshes in a row were armed for something new and drew nothing, its buffers
    /// all outside the output: from two, refreshes are paced again, so that a client that
    /// keeps doing so never loops.
    pub content_misses: u32,
    /// Its frames that failed to draw, so that a renderer that keeps failing logs a few lines.
    pub failed_drawings: FailedDrawings,
    /// The latest refresh a refresh of it aimed at or was presented at; refresh 0, its
    /// creation, at first.
    pub last_refresh: RefreshAt,
    /// The presentation feedback of frames drawn, held until their refresh.
    pub held_feedback: Vec<(RefreshAt, OutputPresentationFeedback)>,
    /// Its recent drawings, from which its lead is predicted.
    pub drawings: Drawings,
    /// The windows it shows, bottom to top, as the layout last arranged them: their places.
    pub placed: Vec<Placed>,
    /// What its last frame drew, bottom to top, as the motion gave it: what a capture of that
    /// frame cuts.
    pub drawn: Vec<Drawn>,
    /// The windows it tells they are visible, in increasing order: as the motion said at the
    /// last arrangement, or at the refresh where its motion ended.
    pub told: Vec<WindowId>,
    /// Whether its motion ran at its last arrangement or refresh: the refresh that finds it
    /// ended tells its windows once.
    pub in_motion: bool,
    /// Why its next refresh draws, noted since its last: what the rest auditor counts a frame
    /// drawn by (`rest.rs`).
    pub draw_causes: crate::rest::PendingCauses,
}

impl VirtualOutput {
    /// The time hit tests, the pointer's focus and MCP's `windows` read its motion at, at `now`:
    /// `now`, or the time its last frame sampled if that is later, so that none reads a motion
    /// that frame drew past. Once it drew a motion's end, they read the output at rest, as it
    /// shows it, though the clock has not reached that end
    /// ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
    pub fn motion_read_at(&self, now: Duration) -> Duration {
        now.max(self.sampled_at)
    }
}

impl State {
    /// Creates a virtual output of `seat` to the right of every output, and arms its first
    /// frame. Refused, with nothing created, for a seat that does not exist, a side beyond the
    /// renderer's `GL_MAX_RENDERBUFFER_SIZE`, and past [`MAX_OUTPUTS`].
    pub(crate) fn add_output(&mut self, seat: SeatId, mode: OutputMode) -> Result<OutputId, Error> {
        if self.seat(seat).is_none() {
            return Err(Error::Refused(format!("no seat {}", seat.0)));
        }
        if self.outputs.len() >= MAX_OUTPUTS {
            return Err(Error::Refused(format!(
                "Yuna has {MAX_OUTPUTS} outputs already, the most it keeps"
            )));
        }
        self.check_renderer_limit(mode)?;
        let buffer = crate::render::create_buffer(&mut self.renderer, mode.width(), mode.height())?;
        let id = self.take_output_id();
        let right_edge = self
            .outputs
            .iter()
            .map(|o| o.rect.right())
            .fold(0.0, f64::max);
        let config = OutputConfig {
            position: Point {
                x: right_edge,
                y: 0.0,
            },
            physical_width: mode.width(),
            physical_height: mode.height(),
            scale: mode.scale(),
        };
        self.layout.add_output(seat, id, config).map_err(|e| {
            Error::InvalidOutput(InvalidOutput(format!("the layout refuses it: {e}")))
        })?;
        self.note_description_changed();
        let rect = self
            .layout
            .output_rect(id)
            .map_err(|e| Error::Layout(e.to_string()))?;
        let output = Output::new(
            format!("HEADLESS-{}", id.0),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Yuna".to_owned(),
                model: "Virtual output".to_owned(),
                serial_number: id.0.to_string(),
            },
        );
        let smithay_mode = smithay_mode(&mode);
        output.change_current_state(
            Some(smithay_mode),
            Some(Transform::Normal),
            Some(Scale::Fractional(mode.scale())),
            Some((rect.x.round() as i32, rect.y.round() as i32).into()),
        );
        output.set_preferred(smithay_mode);
        // Shown to its seat's clients alone (ADR-0042).
        let global = output
            .create_global_with_filter::<State, _>(&self.display_handle, move |client| {
                crate::seats::belongs_to(client, seat)
            });
        let damage = OutputDamageTracker::from_output(&output);
        let colour = self.colour.record(
            mode.hdr()
                .map_or_else(sdr_description, |monitor| monitor.hdr_description()),
        );
        let created_at = self.clock.now();
        let timing = Timing::new(&mode, created_at, 0);
        let creation = RefreshAt {
            seq: 0,
            at: created_at,
        };
        self.outputs.push(VirtualOutput {
            id,
            seat,
            output,
            global,
            mode,
            rect,
            buffer,
            damage,
            drawn_once: false,
            drawn_frames: 0,
            colour,
            timing,
            armed_refresh: creation,
            armed_at: Duration::ZERO,
            armed_sample_at: Duration::ZERO,
            sampled_at: Duration::ZERO,
            armed_at_once: false,
            content_pending: false,
            armed_for_content: false,
            content_misses: 0,
            failed_drawings: FailedDrawings::default(),
            last_refresh: creation,
            held_feedback: Vec::new(),
            drawings: Drawings::default(),
            placed: Vec::new(),
            drawn: Vec::new(),
            told: Vec::new(),
            in_motion: false,
            draw_causes: crate::rest::PendingCauses::default(),
        });
        tracing::info!(
            output = id.0,
            width = mode.width(),
            height = mode.height(),
            millihertz = mode.refresh_millihertz(),
            scale = mode.scale(),
            "virtual output created"
        );
        // A new output starts at rest (ADR-0052): nothing else need show at once, and every
        // other output's motion goes on (ADR-0015).
        self.mark_layout_changed(Change::Animated);
        self.note_draw_cause(id, crate::rest::DrawCause::FirstFrame);
        Ok(id)
    }
}

impl State {
    /// Refuses a mode wider or taller than the renderer's `GL_MAX_RENDERBUFFER_SIZE`.
    fn check_renderer_limit(&self, mode: OutputMode) -> Result<(), Error> {
        let limit = self.max_output_side;
        if mode.width() > limit || mode.height() > limit {
            return Err(Error::InvalidOutput(InvalidOutput(format!(
                "an output of {} × {} pixels is larger than the renderer's limit of {limit}",
                mode.width(),
                mode.height()
            ))));
        }
        Ok(())
    }

    /// Gives output `id` another mode, as a monitor set to one, applied whole or not at all
    /// ([ADR-0082](../../../docs/adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md),
    /// item 7). Refused, with nothing changed, for an unknown output, a side beyond the
    /// renderer's limit, a buffer the renderer cannot make, and a mode the layout refuses
    /// ([`Layout::configure_output`](yuna_layout::Layout::configure_output)); a mode equal to
    /// its own changes nothing. Then, at once:
    ///
    /// - the layout places its windows again, and moves its seat's outputs on its right with
    ///   its right edge;
    /// - a new size gets a new buffer; a new size or scale a new damage tracker, so that its
    ///   next frame is drawn whole, and captures wait for it;
    /// - a new rate or VRR range starts its refresh boundaries, or its VRR panel, again, at the
    ///   change or at the refresh already promised a frame if that is later; the first refresh
    ///   after takes the number the old timing's next would have had;
    /// - HDR turned on or off gives it its new colours, and `color-management-v1` tells them;
    /// - `wl_output` and `xdg_output` tell its clients its mode, scale and place, and those of
    ///   the outputs that moved;
    /// - each surface shown on it is told its new scale, before the configure that gives its
    ///   window its size there;
    /// - a new size or scale fails the captures waiting on it, asked of the old one, and tells
    ///   capture sessions its new size.
    ///
    /// Its windows show their new places from its next refresh, which the change arms. An X11
    /// window keeps XWayland's scale, fixed for its life
    /// ([ADR-0068](../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)):
    /// it is placed again in X11 pixels and drawn at the output's new scale over that one.
    pub(crate) fn configure_output(&mut self, id: OutputId, mode: OutputMode) -> Result<(), Error> {
        let index = self
            .output_index(id)
            .ok_or_else(|| Error::Refused(format!("no output {}", id.0)))?;
        let old = self.outputs[index].mode;
        if mode == old {
            return Ok(());
        }
        self.check_renderer_limit(mode)?;
        let resized = (mode.width(), mode.height()) != (old.width(), old.height());
        let rescaled = mode.scale() != old.scale();
        let retimed = (mode.refresh_millihertz(), mode.vrr_min_millihertz())
            != (old.refresh_millihertz(), old.vrr_min_millihertz());
        // Made before anything changes: a renderer that cannot make it refuses the change.
        let buffer = if resized {
            Some(crate::render::create_buffer(
                &mut self.renderer,
                mode.width(),
                mode.height(),
            )?)
        } else {
            None
        };
        self.layout
            .configure_output(id, mode.width(), mode.height(), mode.scale())
            .map_err(|e| {
                Error::InvalidOutput(InvalidOutput(format!("the layout refuses it: {e}")))
            })?;
        // Nothing is refused from here.
        let now = self.clock.now();
        let colour = (mode.hdr() != old.hdr()).then(|| {
            self.colour.record(
                mode.hdr()
                    .map_or_else(sdr_description, |monitor| monitor.hdr_description()),
            )
        });
        let output = &mut self.outputs[index];
        output.mode = mode;
        if let Some(buffer) = buffer {
            output.buffer = buffer;
        }
        if retimed {
            // The new timing begins at the change, or at the refresh already promised a frame
            // if that is later, which is still presented as promised. Its first refresh after
            // that takes the number the old timing's next would have had, so that refreshes
            // keep counting through the change.
            let last = output.last_refresh;
            let next = output.timing.reachable(now, last, false);
            output.timing = Timing::new(&mode, now.max(last.at), next.seq.saturating_sub(1));
            // Its refresh armed aimed at a refresh of the old timing; the causes noted for it go
            // with it, the change arming the next.
            self.clock.disarm(Deadline::Refresh(id));
            output.draw_causes.clear();
        }
        let output = &mut self.outputs[index];
        if let Some(colour) = colour {
            output.colour = colour;
            crate::colour::protocol::tell_output_colours_changed(&self.colour, &output.output);
        }
        let (new_mode, old_mode) = (smithay_mode(&mode), smithay_mode(&old));
        if new_mode != old_mode {
            // A virtual output announces one mode, the one it has, so that changes in a loop
            // never grow the list every new client is sent.
            output.output.delete_mode(old_mode);
            output.output.set_preferred(new_mode);
        }
        // `done` ends what changed, colours included, and is sent even when only they did.
        output.output.change_current_state(
            (new_mode != old_mode).then_some(new_mode),
            None,
            rescaled.then_some(Scale::Fractional(mode.scale())),
            None,
        );
        if resized || rescaled {
            output.damage = OutputDamageTracker::from_output(&output.output);
            output.drawn_once = false;
        }
        self.move_outputs_to_their_places();
        if rescaled {
            self.tell_scale_of(id);
        }
        if resized || rescaled {
            self.fail_captures(
                id,
                "its output's mode changed: capture it again",
                Failure::ForNow,
            );
        }
        if resized {
            self.update_capture_constraints(id);
        }
        tracing::info!(
            output = id.0,
            width = mode.width(),
            height = mode.height(),
            millihertz = mode.refresh_millihertz(),
            scale = mode.scale(),
            "virtual output configured"
        );
        // Its windows, and those of the outputs that moved, start at rest in their new places
        // (ADR-0052); every other output's motion goes on.
        self.mark_layout_changed(Change::Animated);
        self.note_draw_cause(id, crate::rest::DrawCause::OutputChange);
        Ok(())
    }

    /// Gives each output the rectangle the layout has for it, and tells the clients of those
    /// that moved their new place.
    fn move_outputs_to_their_places(&mut self) {
        for output in &mut self.outputs {
            let Ok(rect) = self.layout.output_rect(output.id) else {
                continue;
            };
            let corner = |rect: Rect| (rect.x.round() as i32, rect.y.round() as i32);
            let moved = corner(rect) != corner(output.rect);
            output.rect = rect;
            if moved {
                output
                    .output
                    .change_current_state(None, None, None, Some(corner(rect).into()));
            }
        }
    }

    /// Tells every surface of the windows shown on output `id`, their popups' included, the
    /// output's scale.
    fn tell_scale_of(&mut self, id: OutputId) {
        let shown: Vec<WlSurface> = self
            .toplevels
            .values()
            .filter(|toplevel| toplevel.shown_on == Some(id))
            .map(|toplevel| toplevel.wl_surface().clone())
            .collect();
        for root in shown {
            for tree in self.window_trees(&root) {
                self.tell_preferred_scale(&tree, id);
            }
        }
    }

    /// Removes output `id`. Its windows go to another output of its seat, as the layout moves its
    /// workspaces ([`Layout::remove_output`](yuna_layout::Layout::remove_output)). Refused for an
    /// unknown output, and for its seat's last one: a seat keeps one while it lives, so that its
    /// windows have somewhere to show. See [`State::drop_output`].
    pub(crate) fn remove_output(&mut self, id: OutputId) -> Result<(), Error> {
        let index = self
            .output_index(id)
            .ok_or_else(|| Error::Refused(format!("no output {}", id.0)))?;
        let seat = self.outputs[index].seat;
        if self.layout.outputs(seat).map_or(0, |outputs| outputs.len()) <= 1 {
            return Err(Error::Refused(format!(
                "output {} is its seat's last one, and a seat keeps one while it lives",
                id.0
            )));
        }
        self.drop_output(index);
        Ok(())
    }

    /// Removes the output at `index`, its seat's last one included. Its windows' surfaces leave
    /// it now, and enter their new output at the arrangement. The feedback it holds is
    /// discarded, its deadlines disarmed, the captures waiting on it failed and its capture
    /// sessions stopped. Its global is disabled at once and destroyed
    /// [`RETIRED_GLOBAL_LIFETIME`] later.
    pub(crate) fn drop_output(&mut self, index: usize) {
        let id = self.outputs[index].id;
        if let Err(error) = self.layout.remove_output(id) {
            tracing::error!(%error, output = id.0, "the layout has no such output");
        }
        self.note_description_changed();
        let leaving: Vec<WlSurface> = self
            .toplevels
            .values()
            .filter(|toplevel| toplevel.shown_on == Some(id))
            .map(|toplevel| toplevel.wl_surface().clone())
            .collect();
        for root in leaving {
            self.leave_output(&root, id);
            if let Some(toplevel) = self.toplevels.get_mut(&root.id()) {
                toplevel.shown_on = None;
            }
        }
        self.clock.disarm(Deadline::Refresh(id));
        self.clock.disarm(Deadline::Presentation(id));
        self.fail_captures(id, "its output was removed", Failure::OutputGone);
        self.stop_capture_sessions(id);
        let removed = self.outputs.remove(index);
        let seat = removed.seat;
        for (_, mut feedback) in removed.held_feedback {
            feedback.discarded();
        }
        self.display_handle
            .disable_global::<State>(removed.global.clone());
        self.retired_globals
            .push((self.clock.now(), removed.global));
        tracing::info!(output = id.0, "virtual output removed");
        // Its seat's outputs, which take its workspaces, show the change at once (ADR-0052); the
        // other seats' motions go on (ADR-0015).
        self.mark_layout_changed(Change::InstantFor(BTreeSet::from([seat])));
    }

    /// Every output, in the order they were created.
    pub(crate) fn describe_outputs(&self) -> Vec<OutputDescription> {
        self.outputs
            .iter()
            .map(|output| OutputDescription {
                id: output.id,
                seat: output.seat,
                name: output.output.name(),
                mode: output.mode,
                rect: output.rect,
            })
            .collect()
    }

    /// Destroys the globals of outputs and seats removed at least [`RETIRED_GLOBAL_LIFETIME`]
    /// ago on Yuna's clock, so that on the manual clock they follow the time a test steps. Run
    /// at the end of each round: at rest, a disabled global waits for the next round, which
    /// costs a little memory and no wake-up.
    pub(crate) fn destroy_retired_globals(&mut self) {
        let now = self.clock.now();
        let display_handle = &self.display_handle;
        self.retired_globals.retain(|(retired_at, global)| {
            let keep = now.saturating_sub(*retired_at) < RETIRED_GLOBAL_LIFETIME;
            if !keep {
                display_handle.remove_global::<State>(global.clone());
            }
            keep
        });
    }
}

/// The most outputs a compositor keeps: an agent creates them through MCP, and each costs its
/// buffer, about 33 MB at 3840 × 2160, so 16 of them about 530 MB.
pub const MAX_OUTPUTS: usize = 16;

/// How long the global of a removed output stays, disabled, before it is destroyed: a client
/// that asked to bind it before it was told of its removal must not be ended for it, as it
/// would be for binding a global that no longer exists.
pub const RETIRED_GLOBAL_LIFETIME: Duration = Duration::from_secs(10);

/// `mode`'s size and rate as `wl_output` tells them.
fn smithay_mode(mode: &OutputMode) -> Mode {
    Mode {
        size: (to_i32(mode.width()), to_i32(mode.height())).into(),
        refresh: to_i32(mode.refresh_millihertz()),
    }
}

/// A rate in Hz, from millihertz.
fn hertz(millihertz: u32) -> f64 {
    f64::from(millihertz) / 1_000.0
}

/// A side or a rate as Smithay takes it, as an `i32`: sides are checked against the renderer's
/// limit, and rates against 1,000 Hz, far below `i32::MAX`, before they get here.
pub(crate) fn to_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// A refresh of an output: its number, counting from 0 at the output's creation, and when it
/// starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RefreshAt {
    pub seq: u64,
    pub at: Duration,
}

/// When an output refreshes: at boundaries a fixed period apart, or as a VRR panel does
/// ([ADR-0023](../../../docs/adr/0023-frames-presentation-and-the-clock.md),
/// [ADR-0046](../../../docs/adr/0046-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)).
///
/// Its boundaries, or its panel, count refreshes from 0 at their start: the output's creation,
/// or the last change of its rate or VRR range. The output's own numbers go on across a change:
/// their refresh `k` is the output's refresh `first_seq + k`, so that presentation feedback
/// never counts back.
#[derive(Debug)]
pub(crate) struct Timing {
    /// The output's refresh that is its boundaries' or its panel's refresh 0.
    first_seq: u64,
    cadence: Cadence,
}

#[derive(Debug)]
enum Cadence {
    Fixed(Boundaries),
    Variable(Panel),
}

impl Timing {
    /// The timing of `mode` whose refresh `first_seq` begins at `start`: at a fixed rate,
    /// boundaries a period apart from then; under VRR, a panel whose refresh in progress began
    /// then.
    pub fn new(mode: &OutputMode, start: Duration, first_seq: u64) -> Timing {
        let rate = mode.refresh_millihertz();
        let cadence = match mode.vrr_min_millihertz() {
            Some(min) => Cadence::Variable(Panel::new(min, rate, start)),
            None => Cadence::Fixed(Boundaries {
                created_at: start,
                refresh_millihertz: rate,
            }),
        };
        Timing { first_seq, cadence }
    }

    /// Whether it runs under VRR.
    pub fn is_variable(&self) -> bool {
        matches!(self.cadence, Cadence::Variable(_))
    }

    /// The output's refresh `seq` as its boundaries or its panel number it. A refresh is never
    /// asked of before `first_seq`: the output's last refresh is that one at least.
    fn own(&self, seq: u64) -> u64 {
        seq.saturating_sub(self.first_seq)
    }

    /// The first refresh a frame ready at `ready` can show at, after the output's `last`: at a
    /// fixed rate, the first boundary at or after it; under VRR, the panel's answer. With
    /// `paced`, for a refresh with nothing new to draw, not before a shortest refresh after
    /// `last` either: at rest the panel never learns of a refresh that drew nothing, and callbacks
    /// asked without damage must come once a shortest refresh, never in a loop.
    pub fn reachable(&self, ready: Duration, last: RefreshAt, paced: bool) -> RefreshAt {
        match &self.cadence {
            Cadence::Fixed(boundaries) => {
                let k = boundaries
                    .first_at_or_after(ready)
                    .max(self.own(last.seq) + 1);
                RefreshAt {
                    seq: self.first_seq.saturating_add(k),
                    at: boundaries.at(k),
                }
            }
            Cadence::Variable(panel) => {
                let ready = if paced {
                    let after_last = last.at.saturating_add(panel.shortest());
                    ready.max(after_last.saturating_sub(panel.blank()))
                } else {
                    ready
                };
                let (seq, at) = panel.first_reachable(ready);
                RefreshAt {
                    seq: self.first_seq.saturating_add(seq).max(last.seq + 1),
                    at,
                }
            }
        }
    }

    /// Whether a frame drawn at `now` shows at once: the output is idle. Under VRR, a frame
    /// shows a vertical blank after it is ready at the soonest.
    pub fn is_idle(&self, now: Duration, last: RefreshAt, paced: bool) -> bool {
        match &self.cadence {
            Cadence::Fixed(boundaries) => now >= boundaries.at(self.own(last.seq) + 1),
            Cadence::Variable(panel) => {
                self.reachable(now, last, paced).at == now.saturating_add(panel.blank())
            }
        }
    }

    /// Where a frame whose drawing ended at `ready` is presented, never before the refresh it
    /// aimed at: an earlier one is the frame before's. A VRR panel records it. During a motion
    /// on a VRR output, a refresh that drew nothing shows its frame again through it too
    /// ([ADR-0054](../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
    pub fn present(&mut self, ready: Duration, aimed: RefreshAt, last: RefreshAt) -> RefreshAt {
        let reached = self.reachable(ready, last, false);
        let presented = if reached.seq < aimed.seq {
            aimed
        } else {
            reached
        };
        let own = self.own(presented.seq);
        if let Cadence::Variable(panel) = &mut self.cadence {
            panel.flip(ready, own, presented.at);
        }
        presented
    }

    /// The refresh `wp_presentation` gives: the period, or a VRR panel's shortest refresh.
    pub fn refresh(&self, period: Duration) -> Refresh {
        match &self.cadence {
            Cadence::Fixed(_) => Refresh::Fixed(period),
            Cadence::Variable(panel) => Refresh::Variable(panel.shortest()),
        }
    }
}

/// An output's refresh boundaries at one rate: boundary 0 is when they were made, and boundary
/// `k` comes `k` periods later, to the nanosecond below, computed from the rate so that errors
/// never add up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Boundaries {
    /// When they were made: the output's creation, or the last change of its rate.
    pub created_at: Duration,
    pub refresh_millihertz: u32,
}

impl Boundaries {
    /// The time of boundary `k`.
    pub fn at(&self, k: u64) -> Duration {
        let nanos =
            u128::from(k) * NANOS_PER_SECOND_IN_MILLIHERTZ / u128::from(self.refresh_millihertz);
        self.created_at.saturating_add(Duration::from_nanos(
            u64::try_from(nanos).unwrap_or(u64::MAX),
        ))
    }

    /// The first boundary at `time` or after it.
    pub fn first_at_or_after(&self, time: Duration) -> u64 {
        let elapsed = time.saturating_sub(self.created_at).as_nanos();
        let estimate = (elapsed * u128::from(self.refresh_millihertz))
            .div_ceil(NANOS_PER_SECOND_IN_MILLIHERTZ);
        let mut k = u64::try_from(estimate).unwrap_or(u64::MAX);
        // `at` rounds down: the estimate may be one off either way.
        while k > 0 && self.at(k - 1) >= time {
            k -= 1;
        }
        while self.at(k) < time {
            k += 1;
        }
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_out_of_range_are_refused() {
        assert!(OutputMode::new(0, 1080, 60.0, 1.0).is_err());
        assert!(OutputMode::new(1920, 0, 60.0, 1.0).is_err());
        assert!(OutputMode::new(1920, 1080, 0.5, 1.0).is_err());
        assert!(OutputMode::new(1920, 1080, 1_000.5, 1.0).is_err());
        assert!(OutputMode::new(1920, 1080, f64::NAN, 1.0).is_err());
        assert!(OutputMode::new(1920, 1080, 60.0, 0.2).is_err());
        assert!(OutputMode::new(1920, 1080, 60.0, 8.5).is_err());
        assert!(OutputMode::new(1920, 1080, 60.0, f64::INFINITY).is_err());
        assert!(OutputMode::new(1, 1, 1.0, 0.25).is_ok());
        assert!(OutputMode::new(1920, 1080, 1_000.0, 8.0).is_ok());
    }

    #[test]
    fn a_rate_is_kept_in_millihertz() {
        let mode = OutputMode::new(1920, 1080, 59.94, 1.0).expect("a valid mode");
        assert_eq!(mode.refresh_millihertz(), 59_940);
        assert_eq!(OutputMode::default().refresh_millihertz(), 240_000);
        assert_eq!(
            OutputMode::default().period(),
            Duration::from_nanos(4_166_666)
        );
    }

    fn boundaries_at_240_hz() -> Boundaries {
        Boundaries {
            created_at: Duration::from_secs(1_000),
            refresh_millihertz: 240_000,
        }
    }

    #[test]
    fn boundaries_are_the_creation_plus_whole_periods() {
        let b = boundaries_at_240_hz();
        assert_eq!(b.at(0), Duration::from_secs(1_000));
        assert_eq!(
            b.at(1),
            Duration::from_secs(1_000) + Duration::from_nanos(4_166_666)
        );
        assert_eq!(
            b.at(3),
            Duration::from_secs(1_000) + Duration::from_nanos(12_500_000)
        );
        // A second of boundaries ends exactly on the second: errors never add up.
        assert_eq!(b.at(240), Duration::from_secs(1_001));
        assert_eq!(b.at(240 * 3_600), Duration::from_secs(4_600));
    }

    #[test]
    fn the_boundary_at_or_after_a_time() {
        let b = boundaries_at_240_hz();
        let start = b.at(0);
        assert_eq!(b.first_at_or_after(start), 0);
        assert_eq!(b.first_at_or_after(b.at(1)), 1);
        assert_eq!(b.first_at_or_after(b.at(1) + Duration::from_nanos(1)), 2);
        assert_eq!(b.first_at_or_after(b.at(1) - Duration::from_nanos(1)), 1);
        // Before the output existed, its first boundary is its creation.
        assert_eq!(b.first_at_or_after(start - Duration::from_secs(1)), 0);
        for k in [7, 239, 240, 241, 100_000] {
            assert_eq!(b.first_at_or_after(b.at(k)), k);
            assert_eq!(b.first_at_or_after(b.at(k) - Duration::from_nanos(1)), k);
            assert_eq!(
                b.first_at_or_after(b.at(k) + Duration::from_nanos(1)),
                k + 1
            );
        }
    }

    #[test]
    fn boundaries_at_a_rate_that_does_not_divide_a_second() {
        let b = Boundaries {
            created_at: Duration::ZERO,
            refresh_millihertz: 59_940,
        };
        for k in [1, 2, 59, 60, 1_000, 1_000_000] {
            assert_eq!(b.first_at_or_after(b.at(k)), k);
            assert!(b.at(k) > b.at(k - 1));
        }
    }
}
