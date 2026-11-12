//! Scrolling columns: windows in columns on a horizontal strip that scrolls, niri's layout
//! ([ADR-0019](../../../docs/adr/0019-the-layouts-behaviours.md)).
//!
//! Positions on the strip start at 0 at the left edge of the first column; the view is a window of
//! the working area's width onto the strip, at `view_x`. A column's screen position is the working
//! area's left edge plus its strip position minus `view_x`.
//!
//! Each window is asked a size by niri's rules: its column's width, within its windows' minimum
//! and maximum widths, floored, and a share of the column's height, rounded
//! ([ADR-0026](../../../docs/adr/0026-real-clients-in-the-layout.md)). Columns and windows are
//! placed from the sizes their clients committed: a column is as wide as its widest window, and
//! windows stack by their committed heights. Before its first commit, a window counts at the
//! size it is asked. When a column changes width, the focused column stays where it is on
//! screen and the columns on its left move, as in niri.

use crate::area::Area;
use crate::geometry::{Rect, Size};
use crate::sizing::{Sizing, whole, whole_floor};
use crate::workspace::Arranged;
use crate::{Direction, WindowId};

/// How wide a column is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnWidth {
    /// A share of the working area's width, gaps included: two columns of 0.5 fill it exactly,
    /// with a gap at each edge and one between them. Kept between [`ColumnWidth::MIN_PROPORTION`]
    /// and 1.
    Proportion(f64),
    /// A width in logical units.
    Fixed(f64),
}

impl ColumnWidth {
    /// The narrowest proportion a width change goes down to.
    pub const MIN_PROPORTION: f64 = 0.05;

    /// The width in logical units, on the physical grid, at least one physical pixel.
    fn resolve(self, area: &Area) -> f64 {
        let width = match self {
            ColumnWidth::Proportion(share) => (area.working.w - area.gap) * share - area.gap,
            ColumnWidth::Fixed(width) => width,
        };
        area.scale.snap(width).max(1.0 / area.scale.get())
    }

    /// The width as a proportion of the working area, the inverse of [`ColumnWidth::resolve`].
    fn as_proportion(self, area: &Area) -> f64 {
        match self {
            ColumnWidth::Proportion(share) => share,
            ColumnWidth::Fixed(width) => {
                let usable = area.working.w - area.gap;
                if usable <= 0.0 {
                    1.0
                } else {
                    (width + area.gap) / usable
                }
            }
        }
    }
}

/// One column: windows stacked from top to bottom.
#[derive(Debug, Clone)]
struct Column {
    /// Never empty.
    windows: Vec<WindowId>,
    /// The index in `windows` of the window focused in this column.
    focused: usize,
    width: ColumnWidth,
    /// The preset `width` was last set from, so that cycling continues from it.
    preset: Option<usize>,
    /// Whether the column spans the working area, gaps aside; `width` is kept for when it stops.
    full_width: bool,
    /// Whether the column's one window is fullscreen: the column takes the whole output.
    /// A fullscreen column always holds exactly one window.
    fullscreen: bool,
}

impl Column {
    fn new(window: WindowId, width: ColumnWidth) -> Column {
        Column {
            windows: vec![window],
            focused: 0,
            width,
            preset: None,
            full_width: false,
            fullscreen: false,
        }
    }

    /// A column for a window leaving this one, with its width and full width, as niri does.
    fn split_off(&self, window: WindowId) -> Column {
        Column {
            full_width: self.full_width,
            ..Column::new(window, self.width)
        }
    }

    /// The width the column is given, in logical units, before its windows' limits: the
    /// output's when fullscreen, the working area's when full width, else its own width.
    fn given_width(&self, area: &Area) -> f64 {
        if self.fullscreen {
            area.output.w
        } else if self.full_width {
            (area.working.w - 2.0 * area.gap).max(1.0 / area.scale.get())
        } else {
            self.width.resolve(area)
        }
    }

    /// Its windows' sizings, top to bottom, into `out`: each window's record is read once, and
    /// the column's sizes are computed from these.
    fn read_sizings(&self, area: &Area, out: &mut Vec<Sizing>) {
        out.clear();
        out.extend(self.windows.iter().map(|&window| area.sizing(window)));
    }

    /// The width its windows are asked: its set width within the largest of its windows'
    /// minimum widths and the smallest of their maximum ones, a minimum winning over a smaller
    /// maximum (niri: `src/layout/scrolling.rs:4634-4665`); floored to whole logical units, so
    /// that proportional columns fit the screen exactly (`src/layout/tile.rs:955-959`). Limits
    /// count in whole units, as Wayland carries them. `sizings` are its windows'. Not for a
    /// fullscreen column, whose window is asked the output's size.
    fn asked_width(&self, area: &Area, sizings: &[Sizing]) -> f64 {
        let mut min = 1.0_f64;
        let mut max = f64::INFINITY;
        for sizing in sizings {
            min = min.max(whole(sizing.min.w));
            if sizing.max.w > 0.0 {
                max = max.min(whole(sizing.max.w));
            }
        }
        whole_floor(self.given_width(area).min(max.max(min)).max(min))
    }

    /// The height each of its windows is asked, top to bottom, into `out`, in whole logical units
    /// (niri: `src/layout/scrolling.rs:4757-4865`). `sizings` are its windows'. A window whose
    /// minimum and maximum heights are equal keeps that height. The others share the rest in
    /// order, each share rounded in turn; a window whose share is below its minimum gets its
    /// minimum, and the rest is shared again. Other maximum heights are ignored, as in niri.
    /// Minimums that do not fit overflow the working area. Not for a fullscreen column.
    ///
    /// Linear in the column's windows, times one more pass for each window whose minimum exceeds
    /// its share: quadratic at worst, in a column whose minimums overflow it.
    fn asked_heights(&self, area: &Area, sizings: &[Sizing], out: &mut Vec<f64>) {
        // 0 marks a window that still shares the rest: every height given is at least 1.
        out.clear();
        out.extend(sizings.iter().map(|sizing| {
            if sizing.fixed_height() {
                whole(sizing.min.h)
            } else {
                0.0
            }
        }));
        let gaps = (sizings.len() + 1) as f64 * area.gap;
        let mut left = area.working.h - gaps - out.iter().sum::<f64>();
        let mut sharing = out.iter().filter(|&&height| height == 0.0).count();
        'share: while sharing > 0 {
            // A trial: the shares as they will be rounded, each checked against its minimum.
            let (mut trial_left, mut trial_sharing) = (left, sharing);
            for (height, sizing) in out.iter_mut().zip(sizings) {
                if *height != 0.0 {
                    continue;
                }
                let min = whole(sizing.min.h);
                let share = trial_left / trial_sharing as f64;
                if min > share {
                    *height = min;
                    left -= min;
                    sharing -= 1;
                    continue 'share;
                }
                trial_left -= whole(share);
                trial_sharing -= 1;
            }
            // Every share holds its window's minimum: give them out.
            for height in out.iter_mut().filter(|height| **height == 0.0) {
                let share = whole(left / sharing as f64);
                *height = share;
                left -= share;
                sharing -= 1;
            }
        }
    }

    /// The column's width on the strip: the output's when fullscreen, else that of its widest
    /// window as drawn (niri: `src/layout/scrolling.rs:4888-4895`). `sizings` are its windows'.
    /// Columns are placed by it, and the view keeps the focused column whole by it.
    fn width_on_strip(&self, area: &Area, sizings: &[Sizing]) -> f64 {
        if self.fullscreen {
            return area.output.w;
        }
        let asked = self.asked_width(area, sizings);
        sizings
            .iter()
            .map(|sizing| area.drawn_width(sizing, asked))
            .fold(0.0, f64::max)
    }

    /// Checks the sizes its windows are asked: never below their minimums, and, when their
    /// minimums fit, within the working area's height to the half unit a rounding may add.
    fn check_sizes(&self, area: &Area) -> Result<(), String> {
        let mut sizings = Vec::new();
        self.read_sizings(area, &mut sizings);
        let largest_min_width = sizings
            .iter()
            .map(|sizing| whole(sizing.min.w))
            .fold(1.0, f64::max);
        if self.asked_width(area, &sizings) < largest_min_width {
            return Err("a window is asked less than its minimum width".to_owned());
        }
        let mins: Vec<f64> = sizings.iter().map(|sizing| whole(sizing.min.h)).collect();
        let mut heights = Vec::new();
        self.asked_heights(area, &sizings, &mut heights);
        if heights.iter().zip(&mins).any(|(height, min)| height < min) {
            return Err("a window is asked less than its minimum height".to_owned());
        }
        let gaps = (self.windows.len() + 1) as f64 * area.gap;
        let fits = mins.iter().sum::<f64>() + gaps <= area.working.h;
        let asked = heights.iter().sum::<f64>() + gaps;
        if fits && asked > area.working.h + 0.5 + crate::geometry::EPSILON {
            return Err(format!(
                "its windows are asked {asked} high, though their minimums fit in {}",
                area.working.h
            ));
        }
        Ok(())
    }
}

/// The columns of one workspace and the view onto them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Scrolling {
    columns: Vec<Column>,
    /// The index of the focused column; 0 when there is none.
    focused: usize,
    /// The strip position shown at the working area's left edge.
    view_x: f64,
    /// Set when a window opens, cleared when focus moves on: if the new window closes before,
    /// focus and view go back to where they were (niri's principle that dealing with a short-lived
    /// window does not move the view).
    before_open: Option<BeforeOpen>,
    /// The view before the focused column went fullscreen, restored when it leaves fullscreen;
    /// kept relative to that column, as niri, so that columns closing on its left do not spoil it.
    view_before_fullscreen: Option<f64>,
}

/// Where focus and view were before a window opened.
#[derive(Debug, Clone, Copy)]
struct BeforeOpen {
    window: WindowId,
    focused_column: usize,
    /// The view, relative to the left edge of `focused_column`.
    view_offset: f64,
}

impl Scrolling {
    /// Builds a strip from windows in order, one column each, the window at `focus` focused.
    pub fn from_windows(windows: &[WindowId], width: ColumnWidth, focus: Option<WindowId>) -> Self {
        let columns: Vec<Column> = windows.iter().map(|&w| Column::new(w, width)).collect();
        let focused = focus
            .and_then(|f| windows.iter().position(|&w| w == f))
            .unwrap_or(0);
        Scrolling {
            columns,
            focused,
            ..Scrolling::default()
        }
    }

    /// Whether the strip holds no window.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// Every window, column by column, from left to right and top to bottom.
    pub fn windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.columns.iter().flat_map(|c| c.windows.iter().copied())
    }

    /// Whether `window` is in the strip.
    pub fn contains(&self, window: WindowId) -> bool {
        self.locate(window).is_some()
    }

    /// The focused window.
    pub fn focused_window(&self) -> Option<WindowId> {
        let column = self.columns.get(self.focused)?;
        column.windows.get(column.focused).copied()
    }

    /// Whether the focused column is fullscreen, so that it covers the output and floating
    /// windows hide.
    pub fn focused_is_fullscreen(&self) -> bool {
        self.columns.get(self.focused).is_some_and(|c| c.fullscreen)
    }

    /// The fullscreen windows, left to right.
    pub fn fullscreen_windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.columns
            .iter()
            .filter(|c| c.fullscreen)
            .map(|c| c.windows[0])
    }

    /// The windows of full-width columns, left to right and top to bottom.
    pub fn full_width_windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.columns
            .iter()
            .filter(|c| c.full_width)
            .flat_map(|c| c.windows.iter().copied())
    }

    /// The column index and the index in the column of `window`.
    fn locate(&self, window: WindowId) -> Option<(usize, usize)> {
        self.columns.iter().enumerate().find_map(|(ci, c)| {
            c.windows
                .iter()
                .position(|&w| w == window)
                .map(|wi| (ci, wi))
        })
    }

    /// The strip position of the left edge of column `index`.
    fn column_x(&self, index: usize, area: &Area) -> f64 {
        let mut sizings = Vec::new();
        self.columns[..index]
            .iter()
            .map(|column| {
                column.read_sizings(area, &mut sizings);
                column.width_on_strip(area, &sizings) + area.gap
            })
            .sum()
    }

    /// Where the view is from the focused column's left edge on the strip. Kept across a change
    /// of width on the focused column's left, by [`Scrolling::set_view_from_focused`], it keeps
    /// the focused column where it is on screen while the columns on its left move, as niri does
    /// (`src/layout/scrolling.rs:1308-1343`).
    pub fn view_from_focused(&self, area: &Area) -> f64 {
        self.view_x - self.column_x(self.focused, area)
    }

    /// Puts the view at `offset` from the focused column's left edge on the strip.
    pub fn set_view_from_focused(&mut self, offset: f64, area: &Area) {
        self.view_x = self.column_x(self.focused, area) + offset;
    }

    /// The width on the strip of column `index`.
    fn strip_width(&self, index: usize, area: &Area) -> f64 {
        let column = &self.columns[index];
        let mut sizings = Vec::new();
        column.read_sizings(area, &mut sizings);
        column.width_on_strip(area, &sizings)
    }

    /// Focus moved on by the user: a window that opens and closes no longer brings focus back.
    fn forget_before_open(&mut self) {
        self.before_open = None;
    }

    /// Adds `window` in a new column right of the focused one, or first if the strip is empty,
    /// and focuses it, unless the focused column is fullscreen: then focus and view stay, so that
    /// nothing takes a game's screen (niri's rule). Existing columns keep their sizes and the
    /// focused one does not move.
    pub fn add_window(&mut self, window: WindowId, width: ColumnWidth, area: Option<&Area>) {
        if self.columns.is_empty() {
            self.columns.push(Column::new(window, width));
            self.focused = 0;
            self.before_open = None;
            self.fit_view(area);
            return;
        }
        let index = self.focused + 1;
        self.columns.insert(index, Column::new(window, width));
        if self.columns[self.focused].fullscreen {
            return;
        }
        self.before_open = area.map(|area| BeforeOpen {
            window,
            focused_column: self.focused,
            view_offset: self.view_x - self.column_x(self.focused, area),
        });
        self.focused = index;
        self.fit_view(area);
    }

    /// Removes `window`; returns whether it was here. When its column empties, the column goes.
    /// Columns left of the focused one going away do not move the focused one on screen.
    pub fn remove_window(&mut self, window: WindowId, area: Option<&Area>) -> bool {
        let Some((ci, wi)) = self.locate(window) else {
            return false;
        };
        let reopen = self.before_open.filter(|b| b.window == window);
        if self.columns[ci].windows.len() > 1 {
            // Its column may narrow: the focused column stays where it is on screen.
            let held = area.map(|a| self.view_from_focused(a));
            let column = &mut self.columns[ci];
            column.windows.remove(wi);
            if column.focused > wi || column.focused == column.windows.len() {
                column.focused -= 1;
            }
            if let (Some(held), Some(area)) = (held, area) {
                self.set_view_from_focused(held, area);
            }
            self.fit_view(area);
            return true;
        }
        let removed_width = area.map(|a| self.strip_width(ci, a) + a.gap);
        let was_fullscreen_focus = ci == self.focused && self.columns[ci].fullscreen;
        self.columns.remove(ci);
        if let Some(before) = reopen {
            // The window that just opened closes again: focus and view go back as they were.
            self.before_open = None;
            self.focused = before
                .focused_column
                .min(self.columns.len().saturating_sub(1));
            if let Some(area) = area {
                self.view_x = self.column_x(self.focused, area) + before.view_offset;
            }
        } else if ci < self.focused {
            // Indices shift: the point to go back to is lost, as in niri.
            self.before_open = None;
            self.focused -= 1;
            if let Some(width) = removed_width {
                self.view_x -= width;
            }
        } else if ci == self.focused && self.focused == self.columns.len() {
            // The last column closed while focused: focus goes to the new last one.
            self.focused = self.focused.saturating_sub(1);
        }
        if was_fullscreen_focus {
            // The view was kept for the column that just went: the new focus decides the view.
            self.view_before_fullscreen = None;
        }
        self.fit_view(area);
        true
    }

    /// Focuses `window`; returns whether it is here.
    pub fn focus_window(&mut self, window: WindowId, area: Option<&Area>) -> bool {
        let Some((ci, wi)) = self.locate(window) else {
            return false;
        };
        if ci != self.focused || self.columns[ci].focused != wi {
            self.forget_before_open();
        }
        self.focused = ci;
        self.columns[ci].focused = wi;
        self.fit_view(area);
        true
    }

    /// Moves focus one column or one window in `direction`; at an edge, nothing moves.
    pub fn focus(&mut self, direction: Direction, area: Option<&Area>) {
        if self.columns.is_empty() {
            return;
        }
        match direction {
            Direction::Left if self.focused > 0 => self.focused -= 1,
            Direction::Right if self.focused + 1 < self.columns.len() => self.focused += 1,
            Direction::Up => {
                let column = &mut self.columns[self.focused];
                if column.focused == 0 {
                    return;
                }
                column.focused -= 1;
            }
            Direction::Down => {
                let column = &mut self.columns[self.focused];
                if column.focused + 1 == column.windows.len() {
                    return;
                }
                column.focused += 1;
            }
            _ => return,
        }
        self.forget_before_open();
        self.fit_view(area);
    }

    /// Moves the focused column one place left or right, or the focused window one place up or
    /// down in its column. Focus follows.
    pub fn move_focused(&mut self, direction: Direction, area: Option<&Area>) {
        if self.columns.is_empty() {
            return;
        }
        let ci = self.focused;
        match direction {
            Direction::Left if ci > 0 => {
                self.columns.swap(ci, ci - 1);
                self.focused -= 1;
            }
            Direction::Right if ci + 1 < self.columns.len() => {
                self.columns.swap(ci, ci + 1);
                self.focused += 1;
            }
            Direction::Up => {
                let column = &mut self.columns[ci];
                if column.focused == 0 {
                    return;
                }
                column.windows.swap(column.focused, column.focused - 1);
                column.focused -= 1;
            }
            Direction::Down => {
                let column = &mut self.columns[ci];
                if column.focused + 1 == column.windows.len() {
                    return;
                }
                column.windows.swap(column.focused, column.focused + 1);
                column.focused += 1;
            }
            _ => return,
        }
        self.forget_before_open();
        self.fit_view(area);
    }

    /// niri's consume-or-expel: a focused window sharing its column leaves it for a new column on
    /// that side; a window alone in its column joins the bottom of the neighbour column on that
    /// side, if there is one. Only left and right mean something. A fullscreen column leaves
    /// fullscreen first, so that a fullscreen column keeps holding one window.
    pub fn consume_or_expel(&mut self, direction: Direction, area: Option<&Area>) {
        if self.columns.is_empty() || !matches!(direction, Direction::Left | Direction::Right) {
            return;
        }
        let ci = self.focused;
        let left = direction == Direction::Left;
        if self.columns[ci].windows.len() > 1 {
            let column = &mut self.columns[ci];
            let window = column.windows.remove(column.focused);
            if column.focused == column.windows.len() {
                column.focused -= 1;
            }
            let new_column = column.split_off(window);
            let index = if left { ci } else { ci + 1 };
            self.columns.insert(index, new_column);
            self.focused = index;
        } else {
            let target = if left {
                match ci.checked_sub(1) {
                    Some(t) => t,
                    None => return,
                }
            } else if ci + 1 < self.columns.len() {
                ci + 1
            } else {
                return;
            };
            self.leave_fullscreen_at(target, area);
            self.leave_fullscreen_at(ci, area);
            let column = self.columns.remove(ci);
            let target = if left { target } else { target - 1 };
            let into = &mut self.columns[target];
            into.windows.extend(column.windows);
            into.focused = into.windows.len() - 1;
            self.focused = target;
        }
        self.forget_before_open();
        self.fit_view(area);
    }

    /// Makes `window`'s column full width or not; returns whether the window is here.
    pub fn set_full_width(&mut self, window: WindowId, on: bool, area: Option<&Area>) -> bool {
        let Some((ci, _)) = self.locate(window) else {
            return false;
        };
        self.change_width_keeping_focus(ci, area, |column| column.full_width = on);
        true
    }

    /// The width of `window`'s column and whether it is full width.
    pub fn column_width(&self, window: WindowId) -> Option<(ColumnWidth, bool)> {
        let (ci, _) = self.locate(window)?;
        Some((self.columns[ci].width, self.columns[ci].full_width))
    }

    /// Whether `window`'s column is full width.
    pub fn is_full_width(&self, window: WindowId) -> bool {
        self.locate(window)
            .is_some_and(|(ci, _)| self.columns[ci].full_width)
    }

    /// Whether `window` is fullscreen.
    pub fn is_fullscreen(&self, window: WindowId) -> bool {
        self.locate(window)
            .is_some_and(|(ci, _)| self.columns[ci].fullscreen)
    }

    /// Makes `window` fullscreen or not; returns whether it is here. A window sharing its column
    /// first leaves it for a column of its own, right of it, since a fullscreen column holds one
    /// window. Leaving fullscreen gives the column back its width, and the view where it was.
    pub fn set_fullscreen(&mut self, window: WindowId, on: bool, area: Option<&Area>) -> bool {
        let Some((mut ci, wi)) = self.locate(window) else {
            return false;
        };
        if self.columns[ci].fullscreen == on {
            return true;
        }
        // Extracting the window moves column indices that a just-opened window would go back to.
        self.forget_before_open();
        // Another column changes width: the focused column stays where it is on screen.
        let held = match area {
            Some(area) if ci != self.focused => Some(self.view_from_focused(area)),
            _ => None,
        };
        if on {
            if self.columns[ci].windows.len() > 1 {
                let column = &mut self.columns[ci];
                let had_focus = self.focused == ci && column.focused == wi;
                column.windows.remove(wi);
                if column.focused > wi || column.focused == column.windows.len() {
                    column.focused -= 1;
                }
                let new_column = column.split_off(window);
                if self.focused > ci || had_focus {
                    self.focused += 1;
                }
                ci += 1;
                self.columns.insert(ci, new_column);
            }
            if ci == self.focused
                && let Some(area) = area
            {
                self.view_before_fullscreen = Some(self.view_x - self.column_x(ci, area));
            }
            self.columns[ci].fullscreen = true;
        } else {
            self.leave_fullscreen_at(ci, area);
        }
        if let (Some(held), Some(area)) = (held, area) {
            self.set_view_from_focused(held, area);
        }
        self.fit_view(area);
        true
    }

    /// Takes column `index` out of fullscreen, putting the view back if it is the focused one.
    fn leave_fullscreen_at(&mut self, index: usize, area: Option<&Area>) {
        if !self.columns[index].fullscreen {
            return;
        }
        self.columns[index].fullscreen = false;
        if index == self.focused
            && let Some(offset) = self.view_before_fullscreen.take()
            && let Some(area) = area
        {
            self.view_x = self.column_x(index, area) + offset;
        }
    }

    /// Sets `window`'s column to the next preset width (`forward`) or the previous one; returns
    /// whether the window is here. From a width that is not a preset, forward goes to the first
    /// preset wider than it, backward to the last narrower one. Leaves full width.
    pub fn cycle_preset_width(
        &mut self,
        window: WindowId,
        presets: &[ColumnWidth],
        forward: bool,
        area: &Area,
    ) -> bool {
        let Some((ci, _)) = self.locate(window) else {
            return false;
        };
        if presets.is_empty() {
            return true;
        }
        let column = &self.columns[ci];
        let current = column.given_width(area);
        let next = match (column.preset, forward) {
            (Some(i), true) if !column.full_width => (i + 1) % presets.len(),
            (Some(i), false) if !column.full_width => (i + presets.len() - 1) % presets.len(),
            (_, true) => presets
                .iter()
                .position(|p| p.resolve(area) > current + 0.5)
                .unwrap_or(0),
            (_, false) => presets
                .iter()
                .rposition(|p| p.resolve(area) < current - 0.5)
                .unwrap_or(presets.len() - 1),
        };
        self.change_width_keeping_focus(ci, Some(area), |column| {
            column.width = presets[next];
            column.preset = Some(next);
            column.full_width = false;
        });
        true
    }

    /// Widens `window`'s column by `delta`, a share of the working area (negative narrows), within
    /// [`ColumnWidth::MIN_PROPORTION`] and 1; returns whether the window is here. Leaves full
    /// width.
    pub fn change_width(&mut self, window: WindowId, delta: f64, area: &Area) -> bool {
        let Some((ci, _)) = self.locate(window) else {
            return false;
        };
        let share = self.columns[ci].width.as_proportion(area) + delta;
        self.change_width_keeping_focus(ci, Some(area), |column| {
            column.width = ColumnWidth::Proportion(share.clamp(ColumnWidth::MIN_PROPORTION, 1.0));
            column.preset = None;
            column.full_width = false;
        });
        true
    }

    /// Applies `change` to column `index`, keeping the focused column where it is on screen. A
    /// column whose windows committed their sizes changes width only when they commit again;
    /// `Layout::commit_size` keeps the focused column in place then.
    fn change_width_keeping_focus(
        &mut self,
        index: usize,
        area: Option<&Area>,
        change: impl FnOnce(&mut Column),
    ) {
        let held = area.map(|a| self.view_from_focused(a));
        change(&mut self.columns[index]);
        if let (Some(held), Some(area)) = (held, area) {
            self.set_view_from_focused(held, area);
        }
        self.fit_view(area);
    }

    /// Moves the view the least needed for the focused column to show whole, with a gap of
    /// padding on the side it comes in from, less when the column is nearly as wide as the view; a
    /// column wider than the view shows from its left edge, flush; a fullscreen column takes
    /// exactly the output (niri's rules, with centring "never"). Without an area, the workspace is
    /// not shown and the view waits until it is. The view kept for leaving fullscreen is dropped
    /// once the focused column is not fullscreen.
    pub fn fit_view(&mut self, area: Option<&Area>) {
        if !self.focused_is_fullscreen() {
            self.view_before_fullscreen = None;
        }
        let Some(area) = area else { return };
        if self.columns.is_empty() {
            self.view_x = -area.gap;
            return;
        }
        let x = self.column_x(self.focused, area);
        let width = self.strip_width(self.focused, area);
        let view_width = area.working.w;
        if self.columns[self.focused].fullscreen {
            self.view_x = x - (area.output.x - area.working.x);
        } else if width >= view_width {
            self.view_x = x;
        } else {
            let padding = ((view_width - width) / 2.0).clamp(0.0, area.gap);
            let left = x - padding;
            let right = x + width + padding;
            if left < self.view_x {
                self.view_x = left;
            } else if right > self.view_x + view_width {
                self.view_x = right - view_width;
            }
        }
        self.view_x = area.scale.snap(self.view_x);
    }

    /// Whether the focused column is fully shown, the invariant [`Scrolling::fit_view`] keeps.
    pub fn focused_is_in_view(&self, area: &Area) -> bool {
        let Some(column) = self.columns.get(self.focused) else {
            return true;
        };
        let x = self.column_x(self.focused, area) - self.view_x;
        let width = self.strip_width(self.focused, area);
        let view_width = area.working.w;
        let tolerance = 1.0 / area.scale.get() + crate::geometry::EPSILON;
        if column.fullscreen {
            return (area.working.x + x - area.output.x).abs() < tolerance;
        }
        if width >= view_width {
            return x.abs() < tolerance;
        }
        let padding = ((view_width - width) / 2.0).clamp(0.0, area.gap);
        x - padding > -tolerance && x + width + padding < view_width + tolerance
    }

    /// Every window with its rectangle and the size it is asked, left to right and top to bottom.
    /// Columns sit side by side by their widths on the strip, and windows one below the other by
    /// their drawn heights, gaps between; each window is drawn at its committed size from its
    /// tile's top-left corner (niri: `src/layout/scrolling.rs:5380-5410`), a fullscreen one
    /// centred on its output-sized tile. Columns scrolled out of view are included, outside the
    /// output. A column whose minimum heights do not fit overflows the working area downwards.
    pub fn arrange(&self, area: &Area, out: &mut Vec<Arranged>) {
        let mut x = area.working.x - self.view_x;
        // Reused from column to column, so that arranging allocates nothing per column.
        let (mut sizings, mut heights) = (Vec::new(), Vec::new());
        for column in &self.columns {
            column.read_sizings(area, &mut sizings);
            let width_on_strip = if column.fullscreen {
                let window = column.windows[0];
                let tile = Rect::new(x, area.output.y, area.output.w, area.output.h);
                let (rect, configure) = area.place_fullscreen(window, tile);
                out.push(Arranged {
                    window,
                    rect,
                    configure,
                    tiled: true,
                    fullscreen: true,
                    pinned: false,
                });
                area.output.w
            } else {
                let width = column.asked_width(area, &sizings);
                column.asked_heights(area, &sizings, &mut heights);
                let mut y = area.working.y + area.gap;
                // The widest window as drawn: `Column::width_on_strip`, from the sizes at hand.
                let mut widest = 0.0_f64;
                for ((&window, sizing), &height) in
                    column.windows.iter().zip(&sizings).zip(&heights)
                {
                    let configure = Size {
                        w: width,
                        h: height,
                    };
                    let drawn = area.drawn_size(sizing, configure);
                    // The size is on the grid: the corner alone is snapped.
                    let corner = (area.scale.snap(x), area.scale.snap(y));
                    out.push(Arranged {
                        window,
                        rect: Rect::new(corner.0, corner.1, drawn.w, drawn.h),
                        configure,
                        tiled: true,
                        fullscreen: false,
                        pinned: false,
                    });
                    y += drawn.h + area.gap;
                    widest = widest.max(drawn.w);
                }
                widest
            };
            x += width_on_strip + area.gap;
        }
    }

    /// Checks that `arrange` draws each column where the view logic places it, by
    /// [`Column::width_on_strip`]: the two compute a column's width apart, `arrange` from the
    /// sizes it draws.
    fn check_placed_where_the_view_has_them(&self, area: &Area) -> Result<(), String> {
        let mut placed = Vec::new();
        self.arrange(area, &mut placed);
        // `arrange` gives each column's windows in a row, top to bottom.
        let mut first = 0;
        let mut x = area.working.x - self.view_x;
        for (i, column) in self.columns.iter().enumerate() {
            let drawn_x = placed.get(first).map(|p| p.rect.x);
            let expected = area.scale.snap(x);
            if !column.fullscreen && drawn_x.is_none_or(|drawn| (drawn - expected).abs() > 1e-6) {
                return Err(format!(
                    "column {i} is drawn at {drawn_x:?}, not {expected}"
                ));
            }
            first += column.windows.len();
            x += self.strip_width(i, area) + area.gap;
        }
        Ok(())
    }

    /// Checks this strip's own rules; `area` is given when the workspace is shown.
    pub fn check(&self, area: Option<&Area>) -> Result<(), String> {
        if !self.columns.is_empty() && self.focused >= self.columns.len() {
            return Err(format!("focused column {} out of range", self.focused));
        }
        for (i, column) in self.columns.iter().enumerate() {
            if column.windows.is_empty() {
                return Err(format!("column {i} is empty"));
            }
            if column.focused >= column.windows.len() {
                return Err(format!("column {i} focuses a window out of range"));
            }
            if column.fullscreen && column.windows.len() != 1 {
                return Err(format!("fullscreen column {i} holds several windows"));
            }
            if let ColumnWidth::Proportion(share) = column.width
                && !(ColumnWidth::MIN_PROPORTION - 1e-9..=1.0 + 1e-9).contains(&share)
            {
                return Err(format!("column {i} has proportion {share}"));
            }
        }
        if self.view_before_fullscreen.is_some() && !self.focused_is_fullscreen() {
            return Err(
                "a view is kept for leaving fullscreen, but the focused column is not".into(),
            );
        }
        if let Some(area) = area {
            if !self.focused_is_in_view(area) {
                return Err("the focused column is not fully in view".to_owned());
            }
            self.check_placed_where_the_view_has_them(area)?;
            for (i, column) in self.columns.iter().enumerate() {
                if !column.fullscreen {
                    column
                        .check_sizes(area)
                        .map_err(|e| format!("column {i}: {e}"))?;
                }
            }
        }
        Ok(())
    }
}
