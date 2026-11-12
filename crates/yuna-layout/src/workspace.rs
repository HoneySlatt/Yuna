//! One workspace: its tiled windows, in scrolling columns or dwindle, and its floating windows.

use crate::area::Area;
use crate::dwindle::Dwindle;
use crate::floating::Floating;
use crate::geometry::{Rect, Size};
use crate::scrolling::{ColumnWidth, Scrolling};
use crate::{Direction, OutputId, Tiling, WindowId};

/// A window as its workspace places it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Arranged {
    pub window: WindowId,
    /// Where it is drawn, on the physical grid.
    pub rect: Rect,
    /// The size its client is asked, in whole logical units; 0 on an axis to let it choose.
    pub configure: Size,
    pub tiled: bool,
    pub fullscreen: bool,
    pub pinned: bool,
}

/// How a workspace tiles its windows.
#[derive(Debug, Clone)]
pub(crate) enum TilingLayout {
    Scrolling(Scrolling),
    Dwindle(Dwindle),
}

/// A workspace.
#[derive(Debug, Clone)]
pub(crate) struct Workspace {
    /// Named workspaces persist until removed; unnamed ones go when empty and not shown.
    pub name: Option<String>,
    /// The output this workspace goes back to when that output returns after being removed
    /// (niri's "original monitor"). See [`Workspace::settle_on`].
    pub home: Option<OutputId>,
    /// It was the workspace its home output showed when that output was removed, so that the
    /// output shows it again when it returns.
    pub was_shown_at_home: bool,
    pub tiling: TilingLayout,
    pub floating: Floating,
    /// Whether focus is among the floating windows rather than the tiled ones. Only true while a
    /// window floats.
    pub focus_floating: bool,
}

impl Workspace {
    pub fn new(name: Option<String>, home: Option<OutputId>, tiling: Tiling) -> Workspace {
        Workspace {
            name,
            home,
            was_shown_at_home: false,
            tiling: match tiling {
                Tiling::Scrolling => TilingLayout::Scrolling(Scrolling::default()),
                Tiling::Dwindle => TilingLayout::Dwindle(Dwindle::default()),
            },
            floating: Floating::default(),
            focus_floating: false,
        }
    }

    /// A window opened on this workspace while it is on `output`: an unnamed workspace, or one
    /// without a home, makes `output` its home, so that the workspaces in use stay where they are.
    /// A named workspace keeps the home it was given, as in niri since 25.02.
    pub fn settle_on(&mut self, output: OutputId) {
        if (self.name.is_none() || self.home.is_none()) && self.home != Some(output) {
            self.home = Some(output);
            self.was_shown_at_home = false;
        }
    }

    /// Which tiling layout this workspace uses.
    pub fn tiling_kind(&self) -> Tiling {
        match self.tiling {
            TilingLayout::Scrolling(_) => Tiling::Scrolling,
            TilingLayout::Dwindle(_) => Tiling::Dwindle,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tiled_is_empty() && self.floating.is_empty()
    }

    fn tiled_is_empty(&self) -> bool {
        match &self.tiling {
            TilingLayout::Scrolling(s) => s.is_empty(),
            TilingLayout::Dwindle(d) => d.is_empty(),
        }
    }

    /// The tiled windows, in the layout's reading order.
    pub fn tiled_windows(&self) -> Vec<WindowId> {
        match &self.tiling {
            TilingLayout::Scrolling(s) => s.windows().collect(),
            TilingLayout::Dwindle(d) => d.windows(),
        }
    }

    /// Every window: the tiled ones, then the floating ones bottom to top.
    pub fn windows(&self) -> Vec<WindowId> {
        let mut windows = self.tiled_windows();
        windows.extend(self.floating.windows());
        windows
    }

    pub fn contains(&self, window: WindowId) -> bool {
        self.is_tiled(window) || self.floating.contains(window)
    }

    pub fn is_tiled(&self, window: WindowId) -> bool {
        match &self.tiling {
            TilingLayout::Scrolling(s) => s.contains(window),
            TilingLayout::Dwindle(d) => d.contains(window),
        }
    }

    /// The focused tiled window, whichever side has the focus.
    pub fn focused_tiled(&self) -> Option<WindowId> {
        match &self.tiling {
            TilingLayout::Scrolling(s) => s.focused_window(),
            TilingLayout::Dwindle(d) => d.focused_window(),
        }
    }

    /// The focused window: the focused floating one when focus is among floating windows, else
    /// the focused tiled one.
    pub fn focused(&self) -> Option<WindowId> {
        if self.focus_floating {
            self.floating.focused()
        } else {
            self.focused_tiled()
        }
    }

    /// Whether `window` is fullscreen here.
    pub fn is_fullscreen(&self, window: WindowId) -> bool {
        match &self.tiling {
            TilingLayout::Scrolling(s) => s.is_fullscreen(window),
            TilingLayout::Dwindle(d) => d.fullscreen_window() == Some(window),
        }
    }

    /// Whether `window` is full width (scrolling) or maximised (dwindle).
    pub fn is_full_width(&self, window: WindowId) -> bool {
        match &self.tiling {
            TilingLayout::Scrolling(s) => s.is_full_width(window),
            TilingLayout::Dwindle(d) => d.maximized_window() == Some(window),
        }
    }

    /// Whether `window` is dwindle's maximised window, which lies over the other tiled ones.
    pub fn is_maximized(&self, window: WindowId) -> bool {
        matches!(&self.tiling, TilingLayout::Dwindle(d) if d.maximized_window() == Some(window))
    }

    /// The fullscreen window drawn over this workspace's tiles, whatever has the focus: a game
    /// stays shown while its dialog or another floating window over it is focused.
    pub fn shown_fullscreen(&self) -> Option<WindowId> {
        match &self.tiling {
            TilingLayout::Scrolling(s) if s.focused_is_fullscreen() => s.focused_window(),
            TilingLayout::Scrolling(_) => None,
            TilingLayout::Dwindle(d) => d.fullscreen_window(),
        }
    }

    /// Whether a fullscreen window holds the focus, so that it covers the output and the floating
    /// windows hide (niri's rule, kept for both layouts: nothing over a game).
    pub fn fullscreen_covers(&self) -> bool {
        if self.focus_floating {
            return false;
        }
        match &self.tiling {
            TilingLayout::Scrolling(s) => s.focused_is_fullscreen(),
            TilingLayout::Dwindle(d) => d.fullscreen_window().is_some(),
        }
    }

    /// Adds `window` to the tiled layout, where a new window goes, and focuses it.
    pub fn add_tiled(&mut self, window: WindowId, width: ColumnWidth, area: Option<&Area>) {
        match &mut self.tiling {
            TilingLayout::Scrolling(s) => s.add_window(window, width, area),
            TilingLayout::Dwindle(d) => d.add_window(window),
        }
        if self.focused_tiled() == Some(window) {
            self.focus_floating = false;
        }
    }

    /// The width of `window`'s column and whether it is full width, in scrolling columns.
    pub fn column_width(&self, window: WindowId) -> Option<(ColumnWidth, bool)> {
        match &self.tiling {
            TilingLayout::Scrolling(s) => s.column_width(window),
            TilingLayout::Dwindle(_) => None,
        }
    }

    /// Adds `window` on top of the floating windows; with `focus`, focuses it, unless a fullscreen
    /// window holds the focus.
    pub fn add_floating(
        &mut self,
        window: WindowId,
        rect: Option<Rect>,
        size: Size,
        area: Option<&Area>,
        focus: bool,
    ) {
        self.floating.add(window, rect, size, area);
        if focus && !self.fullscreen_covers() {
            self.floating.focus(window);
            self.focus_floating = true;
        }
        self.settle_focus_side();
    }

    /// Removes `window`. Returns its floating rectangle and whether it was pinned when it floated,
    /// `Some(None)` when it was tiled, `None` when it is not here.
    pub fn remove(
        &mut self,
        window: WindowId,
        area: Option<&Area>,
    ) -> Option<Option<(Rect, bool)>> {
        let removed = if let Some(floating) = self.floating.remove(window) {
            Some(Some(floating))
        } else {
            let tiled = match &mut self.tiling {
                TilingLayout::Scrolling(s) => s.remove_window(window, area),
                TilingLayout::Dwindle(d) => d.remove_window(window, area),
            };
            tiled.then_some(None)
        };
        self.settle_focus_side();
        removed
    }

    /// Keeps `focus_floating` pointing at a side that has windows.
    fn settle_focus_side(&mut self) {
        if self.focus_floating && self.floating.is_empty() {
            self.focus_floating = false;
        } else if !self.focus_floating && self.tiled_is_empty() && !self.floating.is_empty() {
            self.focus_floating = true;
        }
    }

    /// Focuses `window`, raising it if it floats; returns whether it is here.
    pub fn focus_window(&mut self, window: WindowId, area: Option<&Area>) -> bool {
        if self.floating.focus(window) {
            self.focus_floating = true;
            return true;
        }
        let found = match &mut self.tiling {
            TilingLayout::Scrolling(s) => s.focus_window(window, area),
            TilingLayout::Dwindle(d) => d.focus_window(window),
        };
        if found {
            self.focus_floating = false;
        }
        found
    }

    /// Moves focus in `direction` among the windows of the side that has it.
    pub fn focus(&mut self, direction: Direction, area: Option<&Area>) {
        if self.focus_floating {
            if let Some(next) = self.floating.neighbour(direction) {
                self.floating.focus(next);
            }
            return;
        }
        match (&mut self.tiling, area) {
            (TilingLayout::Scrolling(s), _) => s.focus(direction, area),
            (TilingLayout::Dwindle(d), Some(area)) => d.focus(direction, area),
            (TilingLayout::Dwindle(_), None) => {}
        }
    }

    /// Moves the focused tiled window or column in `direction`. Floating windows do not move by
    /// direction.
    pub fn move_focused(&mut self, direction: Direction, area: Option<&Area>) {
        if self.focus_floating {
            return;
        }
        match (&mut self.tiling, area) {
            (TilingLayout::Scrolling(s), _) => s.move_focused(direction, area),
            (TilingLayout::Dwindle(d), Some(area)) => d.move_focused(direction, area),
            (TilingLayout::Dwindle(_), None) => {}
        }
    }

    /// Switches focus between the floating and the tiled windows, if the other side has any.
    pub fn switch_focus_side(&mut self) {
        let other_has_windows = if self.focus_floating {
            !self.tiled_is_empty()
        } else {
            !self.floating.is_empty()
        };
        if other_has_windows {
            self.focus_floating = !self.focus_floating;
        }
    }

    /// Changes the tiling layout, keeping the windows' order, the focus, fullscreen and full
    /// width: scrolling to dwindle inserts the windows in reading order, each splitting the one
    /// before; dwindle to scrolling gives each window, in tree order, a column of `width`. Dwindle
    /// holds one fullscreen and one maximised window, and never hides the focused one: there,
    /// only the focused window keeps either state. Column widths and groupings are not kept.
    pub fn set_tiling(&mut self, kind: Tiling, width: ColumnWidth, area: Option<&Area>) {
        if self.tiling_kind() == kind {
            return;
        }
        let windows = self.tiled_windows();
        let focus = self.focused_tiled();
        // Read in one pass each: asking every window would be quadratic.
        let (fullscreen, full_width): (Vec<WindowId>, Vec<WindowId>) = match &self.tiling {
            TilingLayout::Scrolling(s) => (
                s.fullscreen_windows().collect(),
                s.full_width_windows().collect(),
            ),
            TilingLayout::Dwindle(d) => (
                d.fullscreen_window().into_iter().collect(),
                d.maximized_window().into_iter().collect(),
            ),
        };
        match kind {
            Tiling::Dwindle => {
                let mut dwindle = Dwindle::from_windows(&windows, focus);
                if let Some(focus) = focus {
                    if full_width.contains(&focus) {
                        dwindle.set_maximized(focus, true);
                    }
                    if fullscreen.contains(&focus) {
                        dwindle.set_fullscreen(focus, true);
                    }
                }
                self.tiling = TilingLayout::Dwindle(dwindle);
            }
            Tiling::Scrolling => {
                let mut scrolling = Scrolling::from_windows(&windows, width, focus);
                for &w in &full_width {
                    scrolling.set_full_width(w, true, area);
                }
                for &w in &fullscreen {
                    scrolling.set_fullscreen(w, true, area);
                }
                scrolling.fit_view(area);
                self.tiling = TilingLayout::Scrolling(scrolling);
            }
        }
    }

    /// Where the view is from the focused column, in scrolling columns: see
    /// [`Scrolling::view_from_focused`]. None in dwindle, which has no view to keep.
    pub fn view_from_focused(&self, area: &Area) -> Option<f64> {
        match &self.tiling {
            TilingLayout::Scrolling(s) => Some(s.view_from_focused(area)),
            TilingLayout::Dwindle(_) => None,
        }
    }

    /// Puts the view at `offset` from the focused column, in scrolling columns.
    pub fn set_view_from_focused(&mut self, offset: f64, area: &Area) {
        if let TilingLayout::Scrolling(s) = &mut self.tiling {
            s.set_view_from_focused(offset, area);
        }
    }

    /// Puts the view back on the focused window after the workspace's area changed.
    pub fn fit_view(&mut self, area: &Area) {
        if let TilingLayout::Scrolling(s) = &mut self.tiling {
            s.fit_view(Some(area));
        }
        self.floating.refit(area);
    }

    /// Every window drawn, with its rectangle and the size it is asked, in drawing order: the
    /// tiled ones, then the floating ones, unless a fullscreen window holds the focus. Linear in
    /// the number of windows, save a scrolling column whose minimum heights do not fit
    /// ([`Scrolling::arrange`]): `arrange` runs after every change.
    pub fn arrange(&self, area: &Area, out: &mut Vec<Arranged>) {
        self.arrange_windows(area, out, false);
    }

    /// Every window, those hidden behind a fullscreen one included, with its rectangle and the
    /// size it is asked: what a window's configure is read from, drawn or not.
    pub fn arrange_with_hidden(&self, area: &Area, out: &mut Vec<Arranged>) {
        self.arrange_windows(area, out, true);
    }

    fn arrange_windows(&self, area: &Area, out: &mut Vec<Arranged>, with_hidden: bool) {
        match &self.tiling {
            TilingLayout::Scrolling(s) => s.arrange(area, out),
            TilingLayout::Dwindle(d) => d.arrange(area, out, with_hidden),
        }
        if with_hidden || !self.fullscreen_covers() {
            self.floating.arrange(area, out);
        }
    }

    /// Checks this workspace's own rules.
    pub fn check(&self, area: Option<&Area>) -> Result<(), String> {
        match &self.tiling {
            TilingLayout::Scrolling(s) => s.check(area)?,
            TilingLayout::Dwindle(d) => d.check(area)?,
        }
        if self.focus_floating && self.floating.is_empty() {
            return Err("focus is among floating windows, but none floats".to_owned());
        }
        if !self.is_empty() && self.focused().is_none() {
            return Err("a workspace with windows focuses none".to_owned());
        }
        let windows = self.windows();
        let mut sorted: Vec<u64> = windows.iter().map(|w| w.0).collect();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.len() != windows.len() {
            return Err("a window is both tiled and floating".to_owned());
        }
        Ok(())
    }
}
