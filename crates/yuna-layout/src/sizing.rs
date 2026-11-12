//! What decides a window's size beyond its place: the limits its client asks, the size it
//! committed, and whether a rule set its floating size
//! ([ADR-0026](../../../docs/adr/0026-real-clients-in-the-layout.md)).
//!
//! Each window has two sizes. The size it is **asked** is what the compositor sends in a
//! configure: whole logical units, as Wayland carries them; widths of scrolling columns are
//! floored, every other length rounded. The size it is **drawn** at is what
//! its client **committed**, its xdg window geometry; before its first commit, the size it is
//! asked. Scrolling columns and floating windows are placed from drawn sizes, as in niri;
//! dwindle draws each window inside its slot, clipped to it.

use crate::geometry::Size;

/// A window's size limits, its committed size, and whether a rule set its floating size.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Sizing {
    /// The smallest size its client asks; 0 on an axis for none.
    pub min: Size,
    /// The largest size its client asks; 0 on an axis for none.
    pub max: Size,
    /// Its window geometry as its client last committed it; none before its first commit.
    pub committed: Option<Size>,
    /// A rule or the compositor set its floating size: floating, it is asked that size and keeps
    /// it, rather than following what its client commits.
    pub floating_size_set: bool,
}

impl Sizing {
    /// Whether its height is fixed: its minimum and maximum heights are set and equal. Such a
    /// window opens floating, as in niri (`src/window/mod.rs:395`), and keeps its height in a
    /// column (`src/layout/scrolling.rs:4763-4765`).
    pub fn fixed_height(&self) -> bool {
        self.min.h > 0.0 && self.min.h == self.max.h
    }

    /// Its size, when both its axes are fixed: a floating window opens at it.
    pub fn fixed_size(&self) -> Option<Size> {
        let fixed = |min: f64, max: f64| min > 0.0 && min == max;
        (fixed(self.min.w, self.max.w) && fixed(self.min.h, self.max.h)).then_some(self.min)
    }

    /// What a floating window that chooses its own size is asked, axis by axis: an axis whose
    /// minimum and maximum are set and equal, that size in whole units; any other, 0, to let the
    /// client choose it (niri: `src/utils/mod.rs:511-519`).
    pub fn fixed_axes(&self) -> Size {
        let axis = |min: f64, max: f64| {
            if min > 0.0 && min == max {
                whole(min)
            } else {
                0.0
            }
        };
        Size {
            w: axis(self.min.w, self.max.w),
            h: axis(self.min.h, self.max.h),
        }
    }
}

/// `value` rounded to a whole logical unit, at least 1: a size asked of a client. Wayland carries
/// sizes as integers, and 0 would tell the client to choose its own (niri rounds the same way,
/// `src/layout/scrolling.rs:4838`).
pub(crate) fn whole(value: f64) -> f64 {
    value.round().max(1.0)
}

/// `value` floored to a whole logical unit, at least 1: the width asked of a scrolling column's
/// windows. niri floors it so that proportional columns fit the screen exactly
/// (`src/layout/tile.rs:955-959`).
pub(crate) fn whole_floor(value: f64) -> f64 {
    value.floor().max(1.0)
}

/// Both axes of `size` rounded by [`whole`].
pub(crate) fn whole_size(size: Size) -> Size {
    Size {
        w: whole(size.w),
        h: whole(size.h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_size_needs_both_axes_fixed() {
        let sizing = |min: Size, max: Size| Sizing {
            min,
            max,
            ..Sizing::default()
        };
        let size = |w, h| Size { w, h };
        let fixed = sizing(size(400.0, 300.0), size(400.0, 300.0));
        assert_eq!(fixed.fixed_size(), Some(size(400.0, 300.0)));
        assert!(fixed.fixed_height());
        let height_only = sizing(size(0.0, 300.0), size(0.0, 300.0));
        assert_eq!(height_only.fixed_size(), None);
        assert_eq!(height_only.fixed_axes(), size(0.0, 300.0));
        assert!(height_only.fixed_height());
        assert!(!Sizing::default().fixed_height());
        assert_eq!(Sizing::default().fixed_axes(), size(0.0, 0.0));
    }

    #[test]
    fn asked_sizes_are_whole_and_never_zero() {
        assert_eq!(whole(1128.67), 1129.0);
        assert_eq!(whole(338.5), 339.0);
        assert_eq!(whole(0.0), 1.0);
        assert_eq!(whole(-24.0), 1.0);
        assert_eq!(whole_floor(1128.67), 1128.0);
        assert_eq!(whole_floor(0.4), 1.0);
    }
}
