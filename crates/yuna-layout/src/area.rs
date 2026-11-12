//! The space a workspace is laid out in.

use crate::geometry::{Rect, Scale};

/// Where a workspace is laid out: the output it is shown on, the part of it windows may tile in,
/// and the gap between windows, all on the output's physical grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Area {
    /// The whole output, in logical coordinates. Fullscreen windows take it.
    pub output: Rect,
    /// The part of the output tiled windows go in. The whole output today; panels' reserved
    /// zones will narrow it when layer shell arrives.
    pub working: Rect,
    /// The output's scale.
    pub scale: Scale,
    /// The gap between tiled windows and around them, snapped to whole physical pixels.
    pub gap: f64,
}

impl Area {
    /// The area of an output covering `output`, in logical coordinates, at `scale`, with `gap`
    /// between windows. Negative gaps count as none.
    pub fn new(output: Rect, scale: Scale, gap: f64) -> Area {
        let output = scale.snap_rect(output);
        Area {
            output,
            working: output,
            scale,
            gap: scale.snap(gap.max(0.0)),
        }
    }
}
