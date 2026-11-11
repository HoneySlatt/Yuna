//! Floating windows: free rectangles over the tiled ones, in a stacking order.
//!
//! Rectangles are kept relative to the working area's top-left corner, so that a workspace moved
//! to another output keeps its windows where they were on the screen.

use crate::area::Area;
use crate::geometry::{Rect, Size, nearest_in_direction};
use crate::{Direction, WindowId};

/// One floating window.
#[derive(Debug, Clone, Copy)]
struct FloatingWindow {
    window: WindowId,
    /// Relative to the working area's top-left corner.
    rect: Rect,
    /// Shown over every workspace of its output (Hyprland's pin).
    pinned: bool,
}

/// The floating windows of one workspace.
#[derive(Debug, Clone, Default)]
pub(crate) struct Floating {
    /// Bottom to top.
    windows: Vec<FloatingWindow>,
    /// The one focused when focus is among floating windows; set whenever a window floats.
    focused: Option<WindowId>,
}

impl Floating {
    /// Whether no window floats.
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Every floating window, bottom to top.
    pub fn windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.windows.iter().map(|f| f.window)
    }

    /// Whether `window` floats here.
    pub fn contains(&self, window: WindowId) -> bool {
        self.index(window).is_some()
    }

    fn index(&self, window: WindowId) -> Option<usize> {
        self.windows.iter().position(|f| f.window == window)
    }

    /// The window focus goes to among floating windows.
    pub fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    /// Adds `window` on top, focused only if no other window floats. `rect` is where it was when it last floated, relative to the working
    /// area; without it, the window is centred at `size`. Either way it is made to fit the working
    /// area, and to show whole when it can.
    pub fn add(&mut self, window: WindowId, rect: Option<Rect>, size: Size, area: Option<&Area>) {
        let rect = match (rect, area) {
            (Some(rect), Some(area)) => fit(rect, area),
            (Some(rect), None) => rect,
            (None, Some(area)) => {
                let w = size.w.min(area.working.w).max(1.0);
                let h = size.h.min(area.working.h).max(1.0);
                let rect = Rect::new((area.working.w - w) / 2.0, (area.working.h - h) / 2.0, w, h);
                area.scale.snap_rect(rect)
            }
            (None, None) => Rect::new(0.0, 0.0, size.w.max(1.0), size.h.max(1.0)),
        };
        self.windows.push(FloatingWindow {
            window,
            rect,
            pinned: false,
        });
        if self.focused.is_none() {
            self.focused = Some(window);
        }
    }

    /// Removes `window`; returns its rectangle, relative to the working area, and whether it was
    /// pinned.
    pub fn remove(&mut self, window: WindowId) -> Option<(Rect, bool)> {
        let index = self.index(window)?;
        let removed = self.windows.remove(index);
        if self.focused == Some(window) {
            self.focused = self.windows.last().map(|f| f.window);
        }
        Some((removed.rect, removed.pinned))
    }

    /// Focuses `window` and puts it on top; returns whether it floats here.
    pub fn focus(&mut self, window: WindowId) -> bool {
        let Some(index) = self.index(window) else {
            return false;
        };
        let raised = self.windows.remove(index);
        self.windows.push(raised);
        self.focused = Some(window);
        true
    }

    /// Moves and resizes `window` to `rect`, relative to the working area, made to fit it.
    pub fn set_rect(&mut self, window: WindowId, rect: Rect, area: Option<&Area>) -> bool {
        let Some(index) = self.index(window) else {
            return false;
        };
        self.windows[index].rect = match area {
            Some(area) => fit(rect, area),
            None => rect,
        };
        true
    }

    /// Whether `window` is pinned.
    pub fn is_pinned(&self, window: WindowId) -> bool {
        self.index(window).is_some_and(|i| self.windows[i].pinned)
    }

    /// Pins or unpins `window`; returns whether it floats here.
    pub fn set_pinned(&mut self, window: WindowId, pinned: bool) -> bool {
        let Some(index) = self.index(window) else {
            return false;
        };
        self.windows[index].pinned = pinned;
        true
    }

    /// The pinned windows, bottom to top.
    pub fn pinned(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.windows.iter().filter(|f| f.pinned).map(|f| f.window)
    }

    /// The window next to the focused one in `direction`, by their rectangles.
    pub fn neighbour(&self, direction: Direction) -> Option<WindowId> {
        let focused = self.focused?;
        let from = self.windows[self.index(focused)?].rect;
        let others: Vec<(WindowId, Rect)> = self
            .windows
            .iter()
            .filter(|f| f.window != focused)
            .map(|f| (f.window, f.rect))
            .collect();
        nearest_in_direction(from, direction, &others)
    }

    /// Every window with its rectangle on the output, bottom to top, snapped to the grid.
    pub fn arrange(&self, area: &Area, out: &mut Vec<(WindowId, Rect)>) {
        for f in &self.windows {
            let rect = Rect::new(
                area.working.x + f.rect.x,
                area.working.y + f.rect.y,
                f.rect.w,
                f.rect.h,
            );
            out.push((f.window, area.scale.snap_rect(rect)));
        }
    }

    /// Makes every window fit `area`, after the workspace moved to another output.
    pub fn refit(&mut self, area: &Area) {
        for f in &mut self.windows {
            f.rect = fit(f.rect, area);
        }
    }
}

/// `rect`, relative to the working area, made no larger than it and moved to show whole, on the
/// physical grid.
fn fit(rect: Rect, area: &Area) -> Rect {
    let w = rect.w.clamp(1.0, area.working.w.max(1.0));
    let h = rect.h.clamp(1.0, area.working.h.max(1.0));
    let x = rect.x.clamp(0.0, (area.working.w - w).max(0.0));
    let y = rect.y.clamp(0.0, (area.working.h - h).max(0.0));
    area.scale.snap_rect(Rect::new(x, y, w, h))
}
