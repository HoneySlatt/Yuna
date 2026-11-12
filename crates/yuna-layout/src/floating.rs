//! Floating windows: free rectangles over the tiled ones, in a stacking order.
//!
//! Rectangles are kept relative to the working area's top-left corner, so that a workspace moved
//! to another output keeps its windows where they were on the screen.
//!
//! A floating window chooses its own size, as in niri: it is asked 0 on each axis not fixed, and
//! its rectangle follows what its client commits, keeping its top-left corner. A rule's size, or
//! a resize by the compositor, is asked instead and kept
//! ([ADR-0026](../../../docs/adr/0026-real-clients-in-the-layout.md)).

use crate::area::Area;
use crate::geometry::{Rect, Size, nearest_in_direction};
use crate::sizing::whole_size;
use crate::workspace::Arranged;
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

    /// Adds `window` on top, focused only if no other window floats. `rect` is where it was when
    /// it last floated, relative to the working area; without it, the window is centred at
    /// `size`. Either way enough of it stays on the working area to grab it (see [`fit`]).
    pub fn add(&mut self, window: WindowId, rect: Option<Rect>, size: Size, area: Option<&Area>) {
        let rect = match (rect, area) {
            (Some(rect), Some(area)) => fit(rect, area),
            (Some(rect), None) => rect,
            (None, Some(area)) => {
                // Centred if it fits; else from the top-left corner, as niri, where a window's
                // important part usually is.
                let (w, h) = (size.w.max(1.0), size.h.max(1.0));
                let x = ((area.working.w - w) / 2.0).max(0.0);
                let y = ((area.working.h - h) / 2.0).max(0.0);
                let rect = Rect::new(x, y, w, h);
                fit(rect, area)
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

    /// `window`'s rectangle, relative to the working area.
    pub fn rect(&self, window: WindowId) -> Option<Rect> {
        self.index(window).map(|i| self.windows[i].rect)
    }

    /// Gives `window` the size its client committed, keeping its top-left corner, made to fit the
    /// working area; nothing happens to a window that does not float here.
    pub fn follow_commit(&mut self, window: WindowId, size: Size, area: Option<&Area>) {
        if let Some(rect) = self.rect(window) {
            self.set_rect(window, Rect::new(rect.x, rect.y, size.w, size.h), area);
        }
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

    /// Every window with its rectangle on the output and the size it is asked, bottom to top,
    /// snapped to the grid. A window is asked 0 on each axis, to choose its own size, but on an
    /// axis whose size is fixed; or the size a rule or the compositor set. It is drawn at what its
    /// client committed, from its corner, as niri draws a tile at its window's size.
    pub fn arrange(&self, area: &Area, out: &mut Vec<Arranged>) {
        for f in &self.windows {
            let sizing = area.sizing(f.window);
            let drawn = area.drawn_size(&sizing, f.rect.size());
            // The corner is snapped alone and the size, already on the grid, kept, so that a
            // window of one pixel never snaps to nothing.
            let rect = Rect::new(
                area.scale.snap(area.working.x + f.rect.x),
                area.scale.snap(area.working.y + f.rect.y),
                drawn.w,
                drawn.h,
            );
            let configure = if sizing.floating_size_set {
                whole_size(f.rect.size())
            } else {
                sizing.fixed_axes()
            };
            out.push(Arranged {
                window: f.window,
                rect,
                configure,
                tiled: false,
                fullscreen: false,
                pinned: f.pinned,
            });
        }
    }

    /// Makes every window fit `area`, after the workspace moved to another output.
    pub fn refit(&mut self, area: &Area) {
        for f in &mut self.windows {
            f.rect = fit(f.rect, area);
        }
    }
}

/// `rect`, relative to the working area, moved so that enough of it stays on the working area to
/// grab it. Its size is kept, at least a unit, on the physical grid and at least a pixel; its
/// corner is snapped alone, so that its size does not change with its place. As niri, after
/// Mutter: on each axis, a quarter of the window stays on, between 10 and 75 units.
fn fit(rect: Rect, area: &Area) -> Rect {
    let pixel = 1.0 / area.scale.get();
    let w = area.scale.snap(rect.w.max(1.0)).max(pixel);
    let h = area.scale.snap(rect.h.max(1.0)).max(pixel);
    let off_x = (w - (w / 4.0).clamp(10.0, 75.0)).max(0.0);
    let off_y = (h - (h / 4.0).clamp(10.0, 75.0)).max(0.0);
    let clamp = |v: f64, low: f64, high: f64| v.max(low).min(high.max(low));
    let x = clamp(rect.x, -off_x, area.working.w - w + off_x);
    let y = clamp(rect.y, -off_y, area.working.h - h + off_y);
    Rect::new(area.scale.snap(x), area.scale.snap(y), w, h)
}
