//! Logical coordinates, shared by every output: a point, a size and a rectangle, as `f64`.
//!
//! The geometry is `Copy` and compares as `f64` does: by `PartialEq` alone, with no `Eq` or `Hash`,
//! since a coordinate may be NaN. Its methods are pure functions of their values. Comparisons that
//! must tolerate rounding go through [`EPSILON`].

/// A point in logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    /// Horizontal, growing to the right.
    pub x: f64,
    /// Vertical, growing downwards.
    pub y: f64,
}

/// A size in logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// A rectangle in logical coordinates: its top-left corner and its size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width, never negative.
    pub w: f64,
    /// Height, never negative.
    pub h: f64,
}

impl Rect {
    /// A rectangle from its corner and size.
    #[inline]
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// The right edge.
    #[inline]
    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    /// The bottom edge.
    #[inline]
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// The centre.
    #[inline]
    pub fn center(&self) -> Point {
        Point {
            x: self.x + self.w / 2.0,
            y: self.y + self.h / 2.0,
        }
    }

    /// The size alone.
    #[inline]
    pub fn size(&self) -> Size {
        Size {
            w: self.w,
            h: self.h,
        }
    }

    /// This rectangle with `inset` taken from each of its four sides; never smaller than empty.
    #[inline]
    pub fn shrunk(&self, inset: f64) -> Rect {
        Rect {
            x: self.x + inset,
            y: self.y + inset,
            w: (self.w - 2.0 * inset).max(0.0),
            h: (self.h - 2.0 * inset).max(0.0),
        }
    }

    /// The part of this rectangle inside `other`; an empty rectangle on `other`'s nearest edge
    /// when they do not meet.
    #[inline]
    pub fn clipped_to(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x).min(other.right());
        let y = self.y.max(other.y).min(other.bottom());
        Rect {
            x,
            y,
            w: (self.right().min(other.right()) - x).max(0.0),
            h: (self.bottom().min(other.bottom()) - y).max(0.0),
        }
    }

    /// Whether this rectangle lies inside `other`, to `EPSILON`.
    #[inline]
    pub fn is_inside(&self, other: &Rect) -> bool {
        self.x >= other.x - EPSILON
            && self.y >= other.y - EPSILON
            && self.right() <= other.right() + EPSILON
            && self.bottom() <= other.bottom() + EPSILON
    }

    /// Whether this rectangle has an area: both its sides longer than `EPSILON`.
    #[inline]
    pub fn has_area(&self) -> bool {
        self.w > EPSILON && self.h > EPSILON
    }

    /// Whether the two rectangles share an area larger than `EPSILON` on both axes. Rectangles that
    /// only touch along an edge do not overlap.
    #[inline]
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.right() - EPSILON
            && other.x < self.right() - EPSILON
            && self.y < other.bottom() - EPSILON
            && other.y < self.bottom() - EPSILON
    }
}

/// The tolerance of comparisons between logical coordinates: far below a physical pixel at any
/// scale an output can have, far above the rounding error of a few `f64` operations.
pub const EPSILON: f64 = 1e-6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_keeps_the_part_inside_and_empties_what_misses() {
        let slot = Rect::new(10.0, 10.0, 100.0, 50.0);
        let inside = Rect::new(10.0, 10.0, 40.0, 30.0);
        assert_eq!(inside.clipped_to(&slot), inside);
        let larger = Rect::new(10.0, 10.0, 500.0, 500.0);
        assert_eq!(larger.clipped_to(&slot), slot);
        let away = Rect::new(500.0, 0.0, 20.0, 20.0).clipped_to(&slot);
        assert!(!away.has_area() && away.is_inside(&slot));
    }

    #[test]
    fn the_centre_of_a_rectangle_is_halfway_along_each_side() {
        let centre = Rect::new(10.0, 20.0, 30.0, 50.0).center();
        assert_eq!((centre.x, centre.y), (25.0, 45.0));
    }

    #[test]
    fn a_side_must_be_longer_than_epsilon_for_a_rectangle_to_have_area() {
        assert!(Rect::new(0.0, 0.0, 1.0, 1.0).has_area());
        assert!(!Rect::new(0.0, 0.0, EPSILON, 1.0).has_area());
        assert!(!Rect::new(0.0, 0.0, 1.0, EPSILON).has_area());
        assert!(!Rect::new(0.0, 0.0, 0.0, 0.0).has_area());
    }

    #[test]
    fn rectangles_apart_or_touching_on_any_side_do_not_overlap() {
        let middle = Rect::new(100.0, 100.0, 10.0, 10.0);
        // The same rectangle moved one gap, then exactly touching, on each side.
        for (dx, dy) in [(-30.0, 0.0), (30.0, 0.0), (0.0, -30.0), (0.0, 30.0)] {
            let apart = Rect::new(100.0 + dx, 100.0 + dy, 10.0, 10.0);
            assert!(!middle.overlaps(&apart), "{apart:?} lies apart");
            assert!(!apart.overlaps(&middle), "{apart:?} lies apart");
        }
        for (dx, dy) in [(-10.0, 0.0), (10.0, 0.0), (0.0, -10.0), (0.0, 10.0)] {
            let touching = Rect::new(100.0 + dx, 100.0 + dy, 10.0, 10.0);
            assert!(!middle.overlaps(&touching), "{touching:?} only touches");
            assert!(!touching.overlaps(&middle), "{touching:?} only touches");
        }
    }

    #[test]
    fn rectangles_overlap_only_by_more_than_epsilon() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let by_epsilon = |dx: f64, dy: f64| Rect::new(dx, dy, 10.0, 10.0);
        assert!(!a.overlaps(&by_epsilon(10.0 - EPSILON, 0.0)));
        assert!(!a.overlaps(&by_epsilon(0.0, 10.0 - EPSILON)));
        assert!(!by_epsilon(10.0 - EPSILON, 0.0).overlaps(&a));
        assert!(!by_epsilon(0.0, 10.0 - EPSILON).overlaps(&a));
        assert!(a.overlaps(&by_epsilon(10.0 - 2.0 * EPSILON, 0.0)));
        assert!(a.overlaps(&by_epsilon(0.0, 10.0 - 2.0 * EPSILON)));
    }

    #[test]
    fn touching_rectangles_do_not_overlap() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(!a.overlaps(&Rect::new(10.0, 0.0, 10.0, 10.0)));
        assert!(a.overlaps(&Rect::new(9.0, 9.0, 10.0, 10.0)));
    }
}
