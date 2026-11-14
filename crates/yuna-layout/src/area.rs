//! The space a workspace is laid out in, and how a window's sizes fit it.

use crate::WindowId;
use crate::geometry::{Rect, Scale, Size};
use crate::layout::Windows;
use crate::sizing::{Sizing, whole_size};

/// Where a workspace is laid out: the output it is shown on, the part of it windows may tile in,
/// and the gap between windows, all on the output's physical grid; and what the model knows of
/// each window's size ([`Sizing`]), which its place depends on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Area<'a> {
    /// The whole output, in logical coordinates. Fullscreen windows take it.
    pub output: Rect,
    /// The part of the output tiled windows go in. The whole output today; panels' reserved
    /// zones will narrow it when layer shell arrives.
    pub working: Rect,
    /// The output's scale.
    pub scale: Scale,
    /// The gap between tiled windows and around them, snapped to whole physical pixels.
    pub gap: f64,
    /// Every window's record, read for its sizes.
    windows: &'a Windows,
}

impl<'a> Area<'a> {
    /// The area of an output covering `output`, in logical coordinates, at `scale`, with `gap`
    /// between windows, reading sizes from `windows`. Negative gaps count as none.
    pub fn new(output: Rect, scale: Scale, gap: f64, windows: &'a Windows) -> Area<'a> {
        let output = scale.snap_rect(output);
        Area {
            output,
            working: output,
            scale,
            gap: scale.snap(gap.max(0.0)),
            windows,
        }
    }

    /// What decides `window`'s size; nothing, for a window the model does not know.
    pub fn sizing(&self, window: WindowId) -> Sizing {
        self.windows
            .get(&window)
            .map_or_else(Sizing::default, |record| record.sizing)
    }

    /// The size a window of `sizing` is drawn at when it is asked `asked`: what its client
    /// committed, else `asked`; on the physical grid, as niri rounds a window's size
    /// (`src/layout/tile.rs:845-850`), and at least one physical pixel on each axis, so that it
    /// never vanishes.
    pub fn drawn_size(&self, sizing: &Sizing, asked: Size) -> Size {
        Size {
            w: self.drawn_width(sizing, asked.w),
            h: self.on_grid(sizing.committed.map_or(asked.h, |size| size.h)),
        }
    }

    /// The width alone of [`Area::drawn_size`].
    pub fn drawn_width(&self, sizing: &Sizing, asked: f64) -> f64 {
        self.on_grid(sizing.committed.map_or(asked, |size| size.w))
    }

    /// `length` on the physical grid, at least one pixel.
    fn on_grid(&self, length: f64) -> f64 {
        self.scale.snap(length).max(1.0 / self.scale.get())
    }

    /// A tiled window given `slot` (dwindle's way): asked the slot's size, drawn at its committed
    /// size from the slot's top-left corner, clipped to the slot so that it never covers its
    /// neighbours (ADR-0026). Returns where it is drawn and the size it is asked. An empty slot,
    /// from a tree deeper than its space, gives an empty rectangle: the window is not drawn.
    pub fn place_in_slot(&self, window: WindowId, slot: Rect) -> (Rect, Size) {
        let asked = whole_size(slot.size());
        let drawn = self.drawn_size(&self.sizing(window), asked);
        // The slot and the size are on the grid, and so is their meeting.
        let rect = Rect::new(slot.x, slot.y, drawn.w, drawn.h).clipped_to(&slot);
        (rect, asked)
    }

    /// A fullscreen window whose tile is `tile`, the output's size: asked the output's size,
    /// drawn centred on the tile at its committed size, clipped to it (niri:
    /// `src/layout/tile.rs:782-805`). Returns where it is drawn and the size it is asked.
    pub fn place_fullscreen(&self, window: WindowId, tile: Rect) -> (Rect, Size) {
        let asked = whole_size(self.output.size());
        let drawn = self.drawn_size(&self.sizing(window), asked);
        // The corner is snapped alone and the size kept: snapping both edges of a rectangle that
        // starts on a half pixel could round its width away. The margin is snapped apart from
        // the tile, so that the window moves with its tile by whole pixels wherever the view
        // puts it, a margin of a half pixel included.
        let x = self.scale.snap(tile.x) + self.scale.snap((tile.w - drawn.w) / 2.0);
        let y = self.scale.snap(tile.y) + self.scale.snap((tile.h - drawn.h) / 2.0);
        (Rect::new(x, y, drawn.w, drawn.h).clipped_to(&tile), asked)
    }
}
