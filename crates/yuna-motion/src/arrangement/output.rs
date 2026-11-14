//! One output's motion: the arrangement the layout gave at the last update, the motions running
//! over it, and what they draw at any time.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use yuna_layout::geometry::{EPSILON, Rect, Scale};
use yuna_layout::{
    ColumnId, Layer, Layout, OutputId, Placed, ShownWorkspaces, WindowId, WorkspaceId,
};

use super::special::SpecialFade;
use super::switch::{Slide, Switch};
use super::{Drawn, Frame, Settings};
use crate::{Curve, CurveParams, Spring, SpringParams};

/// An output's arrangement and the motions running over it.
#[derive(Debug, Clone)]
pub(super) struct OutputMotion {
    /// The output's rectangle, in logical coordinates, at the last update.
    rect: Rect,
    /// Its scale: physical pixels per logical unit.
    scale: Scale,
    /// When the last update was: an earlier time counts as this one.
    updated: Duration,
    /// What the layout arranged at the last update, in its order: the main workspace's windows
    /// but the pinned ones, the special workspace's, the pinned ones.
    arranged: Vec<Placed>,
    /// The main workspace shown at the last update.
    main: WorkspaceId,
    /// The special workspace shown over it.
    special: Option<WorkspaceId>,
    /// The workspace switch, while it is drawn.
    switch: Option<Switch>,
    /// Until when the switch is drawn: until its slide ends, or later while a window of a
    /// workspace it slides still moves, so that none is cut off on its way.
    switch_end: Duration,
    /// While a switch is drawn, the windows of its workspaces but the main one, as the output
    /// would show each alone, without pinned windows: each workspace's in a row, in the stack's
    /// order.
    sliding: Vec<Placed>,
    /// The special workspace's fade, while it runs.
    fade: Option<SpecialFade>,
    /// The offset of each window that has one running.
    offsets: HashMap<WindowId, Offset>,
    /// The camera of each scrolling workspace drawn.
    cameras: HashMap<WorkspaceId, Camera>,
    /// The windows told they are visible while the motions run, in increasing order.
    told: Vec<WindowId>,
    /// The windows whose place overlaps the output, in increasing order: those told once the
    /// motions end.
    overlapping: Vec<WindowId>,
    /// When the last motion ends; the last update's time when none runs.
    end: Duration,
}

/// A window's offset from its place, decaying to 0 on the windows' spring, on each axis.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Offset {
    x: Spring,
    y: Spring,
}

impl Offset {
    fn is_done(&self, at: Duration) -> bool {
        self.x.is_done(at) && self.y.is_done(at)
    }

    fn end(&self) -> Duration {
        self.x.end().max(self.y.end())
    }
}

/// A scrolling workspace's camera: the strip position drawn at its working area's left edge.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Camera {
    /// Where the layout puts the view: `view_x` of [`yuna_layout::WorkspaceView`]. Every strip
    /// window's place is arranged through it.
    view_x: f64,
    /// The camera moving toward `view_x`, while it moves.
    spring: Option<Spring>,
    /// The strip's focused column, as last seen: the column of its window that has the seat's
    /// focus, kept while the focus is elsewhere.
    focused_column: Option<ColumnId>,
}

impl Camera {
    fn velocity(&self, at: Duration) -> f64 {
        self.spring.map_or(0.0, |spring| spring.velocity(at))
    }

    /// How far right of their places the strip's windows are drawn at `at`: the view's place
    /// minus the camera. Exactly 0 at rest.
    fn lag(&self, at: Duration) -> f64 {
        self.spring
            .map_or(0.0, |spring| spring.target() - spring.value(at))
    }

    fn end(&self) -> Duration {
        self.spring.map_or(Duration::ZERO, |spring| spring.end())
    }

    /// The camera after an update at `now` that put the view at `view_x`, the strip having moved
    /// by `shift` under it ([`anchor_shift`]): its value and target move by `shift`, keeping its
    /// velocity, so that the strip does not move on screen; then it moves toward `view_x`.
    fn follow(
        &self,
        shift: f64,
        view_x: f64,
        focused_column: Option<ColumnId>,
        now: Duration,
        params: SpringParams,
        tolerance: f64,
    ) -> Camera {
        if shift == 0.0 && view_x == self.view_x {
            return Camera {
                focused_column,
                ..*self
            };
        }
        let spring = match self.spring {
            Some(spring) => spring.shifted(shift).retarget(now, view_x),
            None => Spring::new(params, self.view_x + shift, view_x, 0.0, now, tolerance),
        };
        Camera {
            view_x,
            spring: (!spring.is_done(now)).then_some(spring),
            focused_column,
        }
    }
}

/// How far from its place a window is drawn, unsnapped, without the switch's slide, and how fast
/// that changes: in logical units, and units per second.
#[derive(Debug, Clone, Copy, Default)]
struct Displacement {
    x: f64,
    y: f64,
    velocity_x: f64,
    velocity_y: f64,
}

impl OutputMotion {
    /// The output's motion after an update at `now`, from `previous`, its motion until then;
    /// `None` when the layout does not know `output`.
    pub fn update(
        previous: Option<OutputMotion>,
        layout: &Layout,
        output: OutputId,
        now: Duration,
        animate: bool,
        settings: &Settings,
    ) -> Option<OutputMotion> {
        let rect = layout.output_rect(output).ok()?;
        let scale = layout.output_scale(output).ok()?;
        let arranged = layout.arrange(output).ok()?;
        let shown = layout.shown_workspaces(output).ok()?;
        let order: Vec<WorkspaceId> = layout
            .workspaces(output)
            .ok()?
            .into_iter()
            .map(|workspace| workspace.id)
            .collect();
        let now = previous.as_ref().map_or(now, |p| now.max(p.updated));
        let mut next = OutputMotion::at_rest(rect, scale, now, arranged, &shown);
        if let Some(previous) = previous {
            next.remember_focus(&previous);
            if animate && previous.rect == rect && previous.scale == scale {
                next.follow(&previous, layout, output, &order, &shown, settings);
            }
        }
        Some(next)
    }

    /// The output at rest on `arranged`: every window at its place, every camera at its view.
    fn at_rest(
        rect: Rect,
        scale: Scale,
        now: Duration,
        arranged: Vec<Placed>,
        shown: &ShownWorkspaces,
    ) -> OutputMotion {
        let mut overlapping: Vec<WindowId> = arranged
            .iter()
            .filter(|placed| placed.rect.overlaps(&rect))
            .map(|placed| placed.window)
            .collect();
        overlapping.sort_unstable();
        overlapping.dedup();
        let cameras = [Some(shown.main), shown.special]
            .into_iter()
            .flatten()
            .filter_map(|view| {
                let camera = Camera {
                    view_x: view.view_x?,
                    spring: None,
                    focused_column: focused_column(&arranged, view.id),
                };
                Some((view.id, camera))
            })
            .collect();
        OutputMotion {
            rect,
            scale,
            updated: now,
            arranged,
            main: shown.main.id,
            special: shown.special.map(|view| view.id),
            switch: None,
            switch_end: now,
            sliding: Vec::new(),
            fade: None,
            offsets: HashMap::new(),
            cameras,
            told: overlapping.clone(),
            overlapping,
            end: now,
        }
    }

    /// Keeps the focused column of each strip whose focused window the seat's focus left.
    fn remember_focus(&mut self, previous: &OutputMotion) {
        for (workspace, camera) in &mut self.cameras {
            if camera.focused_column.is_none() {
                camera.focused_column = previous
                    .cameras
                    .get(workspace)
                    .and_then(|before| before.focused_column);
            }
        }
    }

    /// Turns what moved since `previous` into motion: the switch, the special workspace's fade,
    /// the cameras, the windows' offsets; then what is told and when it all ends. Under reduced
    /// motion, the fade alone.
    fn follow(
        &mut self,
        previous: &OutputMotion,
        layout: &Layout,
        output: OutputId,
        order: &[WorkspaceId],
        shown: &ShownWorkspaces,
        settings: &Settings,
    ) {
        let now = self.updated;
        // A window of a special workspace fading out is drawn while it stays hidden: once
        // closed or shown elsewhere, it is not.
        let still_hidden = |window| {
            layout
                .window(window)
                .is_some_and(|info| info.special && info.output.is_none())
        };
        if settings.reduced_motion {
            self.follow_fade(previous, settings.special_fade.halved(), still_hidden);
            self.follow_told(previous);
            self.end = self.latest_end();
            return;
        }
        let slide = Slide {
            height: self.rect.h,
            tolerance: self.tolerance(),
            params: settings.workspace_switch,
        };
        let drawn = previous.switch_at(now);
        self.switch = Switch::follow(drawn, previous.main, self.main, order, now, slide);
        if let Some(switch) = &self.switch {
            for &workspace in switch.stack() {
                if workspace != self.main {
                    let placed = layout
                        .arrange_workspace(output, workspace)
                        .unwrap_or_default();
                    self.sliding
                        .extend(placed.into_iter().filter(|p| p.layer != Layer::Pinned));
                }
            }
        }
        self.follow_fade(previous, settings.special_fade, still_hidden);
        let before: Vec<&Placed> = previous.drawn_at(now).collect();
        let instant = fullscreen_changed(&before, self.drawn_places());
        self.follow_cameras(previous, &before, shown, &instant, settings.camera);
        self.follow_offsets(previous, &before, &instant, settings.windows);
        self.end_switch();
        self.follow_told(previous);
        self.end = self.latest_end();
    }

    /// Sets until when the switch is drawn: until its slide ends and every window and camera
    /// of the workspaces it slides out are at rest. A switch already over is dropped.
    fn end_switch(&mut self) {
        let Some(switch) = &self.switch else {
            return;
        };
        let windows = self
            .sliding
            .iter()
            .filter_map(|placed| self.offsets.get(&placed.window))
            .map(Offset::end);
        let cameras = switch
            .stack()
            .iter()
            .filter(|&&workspace| workspace != self.main)
            .filter_map(|workspace| self.cameras.get(workspace))
            .map(Camera::end);
        self.switch_end = windows.chain(cameras).fold(switch.end(), Duration::max);
        if self.switch_end <= self.updated {
            self.switch = None;
            self.sliding.clear();
        }
    }

    /// The special workspace's fade after the update: in when it is shown, out when it is
    /// hidden, from wherever the fade before had reached.
    fn follow_fade(
        &mut self,
        previous: &OutputMotion,
        params: CurveParams,
        still_hidden: impl Fn(WindowId) -> bool,
    ) {
        let now = self.updated;
        let before = previous.special_at(now, params);
        let fade = match (self.special, before) {
            (Some(shown), Some((was, alpha, _))) if shown == was => {
                Some(SpecialFade::showing(shown, Some(alpha), now, params))
            }
            (Some(shown), _) => Some(SpecialFade::showing(shown, None, now, params)),
            (None, Some((was, alpha, was_shown))) => {
                let places: &[Placed] = if was_shown {
                    &previous.arranged
                } else {
                    previous.fade.as_ref().map_or(&[], |fade| &fade.hidden)
                };
                let hidden: Vec<Placed> = places
                    .iter()
                    .filter(|placed| placed.workspace == was && still_hidden(placed.window))
                    .copied()
                    .collect();
                // A workspace with nothing left to draw needs no fade.
                (!hidden.is_empty()).then(|| SpecialFade::hiding(was, alpha, hidden, now))
            }
            (None, None) => None,
        };
        self.fade = fade.filter(|fade| !fade.is_done(now));
    }

    /// The cameras after the update. Each scrolling workspace shown before and after, and not
    /// made instant, keeps its camera, moved by its strip's shift under the view and toward its
    /// new view. The workspaces drawn beside the shown ones, sliding out of a switch or fading
    /// out, keep theirs as they were: the layout gives no view for a workspace it does not show.
    /// If it moves one meanwhile, the windows whose places change reach them on their offsets,
    /// which always take up what the cameras leave, so that nothing jumps.
    fn follow_cameras(
        &mut self,
        previous: &OutputMotion,
        before: &[&Placed],
        shown: &ShownWorkspaces,
        instant: &HashSet<WorkspaceId>,
        params: SpringParams,
    ) {
        let now = self.updated;
        let tolerance = self.tolerance();
        for view in [Some(shown.main), shown.special].into_iter().flatten() {
            let (Some(view_x), Some(at_rest)) = (view.view_x, self.cameras.get(&view.id)) else {
                continue;
            };
            let focus = at_rest.focused_column;
            let Some(old) = previous
                .cameras
                .get(&view.id)
                .filter(|_| previous.draws(view.id, now) && !instant.contains(&view.id))
            else {
                continue;
            };
            let shift = anchor_shift(before, &self.arranged, view.id, old, view_x, focus);
            let camera = old.follow(shift, view_x, focus, now, params, tolerance);
            self.cameras.insert(view.id, camera);
        }
        let beside = self
            .switch
            .iter()
            .flat_map(|switch| switch.stack().iter().copied())
            .chain(self.fade.as_ref().map(|fade| fade.workspace))
            .filter(|&workspace| workspace != self.main && Some(workspace) != self.special);
        let mut kept = Vec::new();
        for workspace in beside {
            if let Some(camera) = previous.cameras.get(&workspace)
                && previous.draws(workspace, now)
            {
                kept.push((workspace, *camera));
            }
        }
        self.cameras.extend(kept);
    }

    /// The windows' offsets after the update. A window drawn before and after that keeps its
    /// motion ([`keeps_its_motion`]), on a workspace not made instant or pinned, gets the offset
    /// that draws
    /// it where it was drawn at the update, with the velocity it had: its drawn position before
    /// minus where its new place and its camera draw it.
    fn follow_offsets(
        &mut self,
        previous: &OutputMotion,
        before: &[&Placed],
        instant: &HashSet<WorkspaceId>,
        params: SpringParams,
    ) {
        let now = self.updated;
        let tolerance = self.tolerance();
        let before: HashMap<WindowId, &Placed> = before
            .iter()
            .map(|&placed| (placed.window, placed))
            .collect();
        let mut offsets = HashMap::new();
        for placed in self.drawn_places() {
            let Some(&old) = before.get(&placed.window) else {
                continue;
            };
            // A pinned window floats over every workspace of the output: none makes it instant.
            let made_instant = placed.layer != Layer::Pinned && instant.contains(&placed.workspace);
            if !keeps_its_motion(old, placed) || made_instant {
                continue;
            }
            if self.unmoved(previous, old, placed) {
                // Nothing moved under it: its offset runs on as it was.
                let kept = previous.offsets.get(&placed.window);
                let running = kept.filter(|offset| !offset.is_done(now));
                offsets.extend(running.map(|offset| (placed.window, *offset)));
                continue;
            }
            let from = previous.displacement(old, now);
            let camera = self.camera_of(placed);
            let lag = camera.map_or(0.0, |camera| camera.lag(now));
            let lag_velocity = camera.map_or(0.0, |camera| -camera.velocity(now));
            let x = (old.rect.x + from.x) - (placed.rect.x + lag);
            let y = (old.rect.y + from.y) - placed.rect.y;
            let offset = Offset {
                x: Spring::new(
                    params,
                    x,
                    0.0,
                    from.velocity_x - lag_velocity,
                    now,
                    tolerance,
                ),
                y: Spring::new(params, y, 0.0, from.velocity_y, now, tolerance),
            };
            if !offset.is_done(now) {
                offsets.insert(placed.window, offset);
            }
        }
        self.offsets = offsets;
    }

    /// Whether `placed`, at `old` before the update, is drawn by the same motion after it: the
    /// same place in the same layer, and on a strip, the same camera.
    fn unmoved(&self, previous: &OutputMotion, old: &Placed, placed: &Placed) -> bool {
        if old.rect != placed.rect || old.layer != placed.layer {
            return false;
        }
        // A switch moves pinned windows to the workspace shown; no camera draws them.
        if placed.layer == Layer::Pinned {
            return true;
        }
        let same_camera = match (self.camera_of(placed), previous.camera_of(old)) {
            (Some(now), Some(before)) => now.view_x == before.view_x && now.spring == before.spring,
            (None, None) => true,
            _ => false,
        };
        old.workspace == placed.workspace && same_camera
    }

    /// The windows told they are visible after the update: those told at its time that are
    /// still drawn, and those whose place overlaps the output.
    fn follow_told(&mut self, previous: &OutputMotion) {
        let drawn: HashSet<WindowId> = self.drawn_places().map(|placed| placed.window).collect();
        let mut told: Vec<WindowId> = previous
            .told_visible(self.updated)
            .iter()
            .copied()
            .filter(|window| drawn.contains(window))
            .chain(self.overlapping.iter().copied())
            .collect();
        told.sort_unstable();
        told.dedup();
        self.told = told;
    }

    /// When the last motion that changes a frame ends. The windows and cameras of the special
    /// workspace fading out are drawn only until the fade ends: that end bounds theirs.
    fn latest_end(&self) -> Duration {
        let mut end = self.updated;
        for placed in &self.arranged {
            if let Some(offset) = self.offsets.get(&placed.window) {
                end = end.max(offset.end());
            }
        }
        for workspace in std::iter::once(self.main).chain(self.special) {
            if let Some(camera) = self.cameras.get(&workspace) {
                end = end.max(camera.end());
            }
        }
        if self.switch.is_some() {
            end = end.max(self.switch_end);
        }
        if let Some(fade) = &self.fade {
            end = end.max(fade.end());
        }
        end
    }

    /// Ends every motion at once: the next frame is the arrangement.
    pub fn settle(&mut self) {
        let (main, special) = (self.main, self.special);
        self.switch = None;
        self.switch_end = self.updated;
        self.sliding.clear();
        self.fade = None;
        self.offsets.clear();
        self.cameras
            .retain(|&workspace, _| workspace == main || Some(workspace) == special);
        for camera in self.cameras.values_mut() {
            camera.spring = None;
        }
        self.told = self.overlapping.clone();
        self.end = self.updated;
    }

    /// What the output draws at `at`, bottom to top.
    pub fn frame(&self, at: Duration) -> Frame {
        let at = at.max(self.updated);
        if at >= self.end {
            let windows = self.arranged.iter().map(at_place).collect();
            return Frame { windows };
        }
        let hidden = self.fading_out(at).map_or(&[][..], |fade| &fade.hidden);
        let mut windows =
            Vec::with_capacity(self.arranged.len() + self.sliding.len() + hidden.len());
        match self.switch_at(at) {
            Some(switch) => {
                // `sliding` holds each workspace's windows in a row, in the stack's order.
                let mut rest = self.sliding.as_slice();
                for (index, &workspace) in switch.stack().iter().enumerate() {
                    let slide = switch.slide(index, at);
                    if workspace == self.main {
                        self.push_in_view(&mut windows, self.main_windows(), slide, at);
                    } else {
                        let count = rest.iter().take_while(|p| p.workspace == workspace).count();
                        let (own, after) = rest.split_at(count);
                        rest = after;
                        self.push_in_view(&mut windows, own.iter(), slide, at);
                    }
                }
            }
            None => {
                let main = self.main_windows().map(|p| self.drawn(p, 0.0, 1.0, at));
                windows.extend(main);
            }
        }
        if let Some(special) = self.special {
            let alpha = self
                .fade
                .as_ref()
                .filter(|fade| fade.workspace == special)
                .map_or(1.0, |fade| fade.alpha(at));
            let own = self.arranged.iter().filter(|p| p.workspace == special);
            windows.extend(own.map(|p| self.drawn(p, 0.0, alpha, at)));
        } else if let Some(fade) = self.fading_out(at) {
            let alpha = fade.alpha(at);
            windows.extend(fade.hidden.iter().map(|p| self.drawn(p, 0.0, alpha, at)));
        }
        let pinned = self.arranged.iter().filter(|p| self.is_pinned_section(p));
        windows.extend(pinned.map(|p| self.drawn(p, 0.0, 1.0, at)));
        Frame { windows }
    }

    /// The main workspace's windows arranged, but the pinned ones.
    fn main_windows(&self) -> impl Iterator<Item = &Placed> {
        self.arranged
            .iter()
            .filter(|p| p.workspace == self.main && p.layer != Layer::Pinned)
    }

    /// Draws those of `places` that show on the output at `at`, `slide` down: during a switch,
    /// a frame lists what it shows of the workspaces sliding, not whole workspaces, so that a
    /// window that an offset or its size carries past its workspace's edge is drawn until it
    /// rests.
    fn push_in_view<'a>(
        &self,
        windows: &mut Vec<Drawn>,
        places: impl Iterator<Item = &'a Placed>,
        slide: f64,
        at: Duration,
    ) {
        let drawn = places.map(|p| self.drawn(p, slide, 1.0, at));
        windows.extend(drawn.filter(|drawn| drawn.rect.overlaps(&self.rect)));
    }

    /// Whether `placed`, arranged, is a pinned window of the main workspace: drawn over the
    /// special workspace, out of the switch.
    fn is_pinned_section(&self, placed: &Placed) -> bool {
        placed.workspace == self.main && placed.layer == Layer::Pinned
    }

    /// `placed` as drawn at `at`, `slide` down, with `alpha`: its place moved by its offset and
    /// its camera, its corner rounded once to the physical grid. An axis that does not move is
    /// exactly the place's.
    fn drawn(&self, placed: &Placed, slide: f64, alpha: f64, at: Duration) -> Drawn {
        let moved = self.displacement(placed, at);
        let rect = Rect {
            x: self.moved(placed.rect.x, moved.x),
            y: self.moved(placed.rect.y, moved.y + slide),
            ..placed.rect
        };
        Drawn {
            window: placed.window,
            rect,
            alpha,
            workspace: placed.workspace,
            placed: *placed,
        }
    }

    /// `value` moved by `by` and snapped to the physical grid; `value` itself when `by` is 0.
    fn moved(&self, value: f64, by: f64) -> f64 {
        if by == 0.0 {
            value
        } else {
            self.scale.snap(value + by)
        }
    }

    /// How far from its place `placed` is drawn at `at`, and how fast that changes: its offset,
    /// and on a strip, its camera's lag.
    fn displacement(&self, placed: &Placed, at: Duration) -> Displacement {
        let mut moved = Displacement::default();
        if let Some(offset) = self.offsets.get(&placed.window) {
            moved.x += offset.x.value(at);
            moved.y += offset.y.value(at);
            moved.velocity_x += offset.x.velocity(at);
            moved.velocity_y += offset.y.velocity(at);
        }
        if let Some(camera) = self.camera_of(placed) {
            moved.x += camera.lag(at);
            moved.velocity_x -= camera.velocity(at);
        }
        moved
    }

    /// The camera that draws `placed`: its workspace's, for a window of a strip.
    fn camera_of(&self, placed: &Placed) -> Option<&Camera> {
        if placed.layer == Layer::Strip {
            self.cameras.get(&placed.workspace)
        } else {
            None
        }
    }

    /// What the output draws of a special workspace at `at`: the workspace, its opacity's curve,
    /// and whether it is shown (rather than fading out).
    fn special_at(&self, at: Duration, params: CurveParams) -> Option<(WorkspaceId, Curve, bool)> {
        match self.special {
            Some(special) => {
                let alpha = self
                    .fade
                    .as_ref()
                    .filter(|fade| fade.workspace == special)
                    .map_or_else(|| Curve::new(params, 1.0, 1.0, at), SpecialFade::curve);
                Some((special, alpha, true))
            }
            None => self
                .fading_out(at)
                .map(|fade| (fade.workspace, fade.curve(), false)),
        }
    }

    /// The special workspace fading out at `at`, if one is.
    fn fading_out(&self, at: Duration) -> Option<&SpecialFade> {
        self.fade
            .as_ref()
            .filter(|fade| self.special != Some(fade.workspace) && !fade.is_done(at))
    }

    /// The switch, while it is drawn at `at`.
    fn switch_at(&self, at: Duration) -> Option<&Switch> {
        self.switch.as_ref().filter(|_| at < self.switch_end)
    }

    /// The windows the output may draw at `at`, wherever they are: its arrangement, the
    /// workspaces sliding while the switch is drawn, the special one while it fades out.
    fn drawn_at(&self, at: Duration) -> impl Iterator<Item = &Placed> {
        let sliding = self.switch_at(at).map_or(&[][..], |_| &self.sliding);
        let hidden = self.fading_out(at).map_or(&[][..], |fade| &fade.hidden);
        self.arranged.iter().chain(sliding).chain(hidden)
    }

    /// The windows the output may draw right after the update, while its motions are being
    /// set: the switch and the fade it has then are drawn.
    fn drawn_places(&self) -> impl Iterator<Item = &Placed> {
        let hidden = match &self.fade {
            Some(fade) if self.special != Some(fade.workspace) => &fade.hidden[..],
            _ => &[],
        };
        self.arranged.iter().chain(&self.sliding).chain(hidden)
    }

    /// Whether the output draws `workspace` at `at`.
    fn draws(&self, workspace: WorkspaceId, at: Duration) -> bool {
        workspace == self.main
            || Some(workspace) == self.special
            || self
                .switch_at(at)
                .is_some_and(|switch| switch.stack().contains(&workspace))
            || self
                .fading_out(at)
                .is_some_and(|fade| fade.workspace == workspace)
    }

    /// When the motions end; `None` at rest at `at`.
    pub fn end(&self, at: Duration) -> Option<Duration> {
        (self.end > at.max(self.updated)).then_some(self.end)
    }

    /// The windows told they are visible at `at`, in increasing order.
    pub fn told_visible(&self, at: Duration) -> &[WindowId] {
        if self.end > at.max(self.updated) {
            &self.told
        } else {
            &self.overlapping
        }
    }

    /// Half a physical pixel, in logical units: under it, motion cannot be seen.
    fn tolerance(&self) -> f64 {
        0.5 / self.scale.get()
    }
}

/// `placed` drawn at its place, opaque.
fn at_place(placed: &Placed) -> Drawn {
    Drawn {
        window: placed.window,
        rect: placed.rect,
        alpha: 1.0,
        workspace: placed.workspace,
        placed: *placed,
    }
}

/// Whether a window placed at `old` before an update and at `new` after keeps its motion: it
/// stays in its container, its workspace and its layer, or goes from floating to tiled or back
/// within its workspace; a pinned window stays pinned, whatever workspace the switch gives it;
/// and it does not go fullscreen or leave it. Otherwise it is drawn at its new place at once
/// ([ADR-0052](../../../../docs/adr/0052-motion-in-a-pure-crate.md)).
fn keeps_its_motion(old: &Placed, new: &Placed) -> bool {
    if old.fullscreen != new.fullscreen {
        return false;
    }
    match (old.layer, new.layer) {
        (Layer::Pinned, Layer::Pinned) => true,
        (Layer::Pinned, _) | (_, Layer::Pinned) => false,
        (before, after) => {
            old.workspace == new.workspace
                && (before == after || before == Layer::Floating || after == Layer::Floating)
        }
    }
}

/// The workspaces where a window went fullscreen or left it, by the windows drawn before and
/// after an update; pinned windows aside, since the switch moves them between workspaces. Such
/// a workspace's camera and offsets are instant: a game wants its output at once
/// ([ADR-0055](../../../../docs/adr/0055-what-moves-first-and-animations-off-headless.md)).
fn fullscreen_changed<'a>(
    before: &[&Placed],
    after: impl Iterator<Item = &'a Placed>,
) -> HashSet<WorkspaceId> {
    let key = |placed: &Placed| {
        (placed.fullscreen && placed.layer != Layer::Pinned)
            .then_some((placed.workspace, placed.window))
    };
    let before: HashSet<(WorkspaceId, WindowId)> =
        before.iter().filter_map(|placed| key(placed)).collect();
    let after: HashSet<(WorkspaceId, WindowId)> = after.filter_map(key).collect();
    before
        .symmetric_difference(&after)
        .map(|&(workspace, _)| workspace)
        .collect()
}

/// The column of `workspace` that holds the seat's focus, if a window of its strip does.
fn focused_column(arranged: &[Placed], workspace: WorkspaceId) -> Option<ColumnId> {
    arranged
        .iter()
        .find(|placed| placed.workspace == workspace && placed.activated)
        .and_then(|placed| placed.column)
}

/// How far `workspace`'s strip moved under the view in an update: the move along the strip of
/// its anchor, a column there before and after
/// ([ADR-0052](../../../../docs/adr/0052-motion-in-a-pure-crate.md)).
///
/// The anchor is a column that the update left in place on screen: the focused one after the
/// update, `focus`, or before it, or else the one nearest the focused one. Shifting by its move
/// keeps the camera still, so that what the operation left in place does not move, whatever the
/// springs of the camera and of the windows. When no column stayed in place (the view moved to
/// another column), the anchor is the focused column after the update if it was there before,
/// else the focused column before if it is there after; with neither, the shift is 0. A column's
/// position on the strip is its left edge plus the view: `rect.x + view_x`.
fn anchor_shift(
    before: &[&Placed],
    after: &[Placed],
    workspace: WorkspaceId,
    old: &Camera,
    view_x: f64,
    focus: Option<ColumnId>,
) -> f64 {
    let before_x = |column| column_left(before.iter().copied(), workspace, column);
    let after_x = |column| column_left(after.iter(), workspace, column);
    let shift = |column| Some((after_x(column)? + view_x) - (before_x(column)? + old.view_x));
    let in_place = |column: &ColumnId| match (before_x(*column), after_x(*column)) {
        (Some(before), Some(after)) => (after - before).abs() < EPSILON,
        _ => false,
    };
    let preferred = [focus, old.focused_column].into_iter().flatten();
    let focus_x = focus.and_then(after_x);
    let mut columns: Vec<ColumnId> = after
        .iter()
        .filter(|placed| placed.workspace == workspace)
        .filter_map(|placed| placed.column)
        .collect();
    columns.sort_unstable();
    columns.dedup();
    let nearest = focus_x.and_then(|focus_x| {
        columns.iter().copied().filter(in_place).min_by(|a, b| {
            let distance = |column| after_x(column).map_or(f64::INFINITY, |x| (x - focus_x).abs());
            distance(*a).total_cmp(&distance(*b))
        })
    });
    preferred
        .clone()
        .filter(in_place)
        .chain(nearest)
        .chain(columns.iter().copied().filter(in_place))
        .find_map(shift)
        .or_else(|| preferred.into_iter().find_map(shift))
        .unwrap_or(0.0)
}

/// The left edge of `column` of `workspace` among `places`, if it is there.
fn column_left<'a>(
    places: impl Iterator<Item = &'a Placed>,
    workspace: WorkspaceId,
    column: ColumnId,
) -> Option<f64> {
    places
        .filter(|placed| placed.workspace == workspace && placed.column == Some(column))
        .map(|placed| placed.rect.x)
        .reduce(f64::min)
}
