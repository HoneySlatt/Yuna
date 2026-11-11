//! Scrolling columns: windows in columns on a horizontal strip that scrolls, niri's layout
//! ([ADR-0019](../../../docs/adr/0019-the-layouts-behaviours.md)).
//!
//! Positions on the strip start at 0 at the left edge of the first column; the view is a window of
//! the working area's width onto the strip, at `view_x`. A column's screen position is the working
//! area's left edge plus its strip position minus `view_x`.

use crate::area::Area;
use crate::geometry::Rect;
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

/// One column: windows stacked from top to bottom, at equal heights.
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

    /// The column's width on screen, in logical units.
    fn tile_width(&self, area: &Area) -> f64 {
        if self.fullscreen {
            area.output.w
        } else if self.full_width {
            (area.working.w - 2.0 * area.gap).max(1.0 / area.scale.get())
        } else {
            self.width.resolve(area)
        }
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
    /// The view before the focused column went fullscreen, restored when it leaves fullscreen.
    view_before_fullscreen: Option<f64>,
}

/// Where focus and view were before a window opened.
#[derive(Debug, Clone, Copy)]
struct BeforeOpen {
    window: WindowId,
    focused_column: usize,
    view_x: f64,
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
        self.columns[..index]
            .iter()
            .map(|c| c.tile_width(area) + area.gap)
            .sum()
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
        self.before_open = Some(BeforeOpen {
            window,
            focused_column: self.focused,
            view_x: self.view_x,
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
        let column = &mut self.columns[ci];
        if column.windows.len() > 1 {
            column.windows.remove(wi);
            if column.focused > wi || column.focused == column.windows.len() {
                column.focused -= 1;
            }
            self.fit_view(area);
            return true;
        }
        let removed_width = area.map(|a| column.tile_width(a) + a.gap);
        let was_fullscreen_focus = ci == self.focused && column.fullscreen;
        self.columns.remove(ci);
        if let Some(before) = reopen {
            // The window that just opened closes again: focus and view go back as they were.
            self.before_open = None;
            self.focused = before
                .focused_column
                .min(self.columns.len().saturating_sub(1));
            self.view_x = before.view_x;
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
        if was_fullscreen_focus && let Some(view) = self.view_before_fullscreen.take() {
            self.view_x = view;
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
            self.leave_fullscreen_at(target);
            self.leave_fullscreen_at(ci);
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
            if ci == self.focused {
                self.view_before_fullscreen = Some(self.view_x);
            }
            self.columns[ci].fullscreen = true;
        } else {
            self.leave_fullscreen_at(ci);
        }
        self.fit_view(area);
        true
    }

    /// Takes column `index` out of fullscreen, putting the view back if it is the focused one.
    fn leave_fullscreen_at(&mut self, index: usize) {
        if !self.columns[index].fullscreen {
            return;
        }
        self.columns[index].fullscreen = false;
        if index == self.focused
            && let Some(view) = self.view_before_fullscreen.take()
        {
            self.view_x = view;
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
        let current = column.tile_width(area);
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

    /// Applies `change` to column `index`; if the column is left of the focused one, shifts the
    /// view by the change in width so that the focused column stays still on screen.
    fn change_width_keeping_focus(
        &mut self,
        index: usize,
        area: Option<&Area>,
        change: impl FnOnce(&mut Column),
    ) {
        let before = area.map(|a| self.columns[index].tile_width(a));
        change(&mut self.columns[index]);
        if let (Some(before), Some(area)) = (before, area)
            && index < self.focused
        {
            self.view_x += self.columns[index].tile_width(area) - before;
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
        let column = &self.columns[self.focused];
        let width = column.tile_width(area);
        let view_width = area.working.w;
        if column.fullscreen {
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
        let width = column.tile_width(area);
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

    /// Every window with its rectangle and whether it is fullscreen, left to right and top to
    /// bottom, snapped to the grid. Columns scrolled out of view are included, outside the output.
    /// A column holding more windows than its height fits, gaps included, gives the last ones no
    /// height rather than letting them leave the working area.
    pub fn arrange(&self, area: &Area, out: &mut Vec<(WindowId, Rect, bool)>) {
        let mut x = area.working.x - self.view_x;
        let bottom = area.working.bottom();
        for column in &self.columns {
            let width = column.tile_width(area);
            if column.fullscreen {
                let rect = Rect::new(x, area.output.y, width, area.output.h);
                out.push((column.windows[0], area.scale.snap_rect(rect), true));
            } else {
                let count = column.windows.len() as f64;
                let usable = area.working.h - (count + 1.0) * area.gap;
                let height = (usable / count).max(0.0);
                for (i, &window) in column.windows.iter().enumerate() {
                    let y =
                        (area.working.y + area.gap + i as f64 * (height + area.gap)).min(bottom);
                    let rect = Rect::new(x, y, width, height.min(bottom - y));
                    out.push((window, area.scale.snap_rect(rect), false));
                }
            }
            x += width + area.gap;
        }
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
        if let Some(area) = area
            && !self.focused_is_in_view(area)
        {
            return Err("the focused column is not fully in view".to_owned());
        }
        Ok(())
    }
}
