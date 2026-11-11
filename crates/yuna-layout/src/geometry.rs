//! Logical coordinates, output scales and the physical pixel grid.
//!
//! The model works in logical coordinates, shared by every output, as `f64`: at a fractional scale,
//! most physical pixel edges fall between integer logical values (at 1.5, physical pixel 1 is at
//! logical 0.667). Every rectangle the model gives out is snapped to the physical grid of its
//! output, so that windows and borders land on whole pixels and stay crisp
//! ([ADR-0020](../../../docs/adr/0020-the-layout-models-shape.md)).

use crate::Direction;

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
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// The right edge.
    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    /// The bottom edge.
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// The centre.
    pub fn center(&self) -> Point {
        Point {
            x: self.x + self.w / 2.0,
            y: self.y + self.h / 2.0,
        }
    }

    /// The size alone.
    pub fn size(&self) -> Size {
        Size {
            w: self.w,
            h: self.h,
        }
    }

    /// This rectangle with `inset` taken from each of its four sides; never smaller than empty.
    pub fn shrunk(&self, inset: f64) -> Rect {
        Rect {
            x: self.x + inset,
            y: self.y + inset,
            w: (self.w - 2.0 * inset).max(0.0),
            h: (self.h - 2.0 * inset).max(0.0),
        }
    }

    /// Whether the two rectangles share an area larger than `EPSILON` on both axes. Rectangles that
    /// only touch along an edge do not overlap.
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

/// An output's scale: how many physical pixels one logical unit spans.
///
/// Always finite and positive; [`Scale::new`] refuses anything else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Scale(f64);

impl Scale {
    /// The smallest scale accepted. Below it, a 4K output would be more than 16,000 logical units
    /// wide, which no configuration means.
    pub const MIN: f64 = 0.25;
    /// The largest scale accepted.
    pub const MAX: f64 = 10.0;

    /// A scale, if `value` is finite and between [`Scale::MIN`] and [`Scale::MAX`].
    pub fn new(value: f64) -> Option<Scale> {
        (value.is_finite() && (Self::MIN..=Self::MAX).contains(&value)).then_some(Scale(value))
    }

    /// The scale as a number.
    pub fn get(self) -> f64 {
        self.0
    }

    /// `value` moved to the nearest edge of a physical pixel.
    pub fn snap(self, value: f64) -> f64 {
        (value * self.0).round() / self.0
    }

    /// Whether `value` lies on an edge of a physical pixel, to a ten-thousandth of a pixel: far
    /// above the rounding error of the few operations a snapped value goes through.
    pub fn is_on_grid(self, value: f64) -> bool {
        let physical = value * self.0;
        (physical - physical.round()).abs() < 1e-4
    }

    /// The rectangle with each of its four edges snapped to the physical grid. Two rectangles that
    /// share an edge before snapping still share it after, since the edge snaps to one value.
    pub fn snap_rect(self, rect: Rect) -> Rect {
        let x = self.snap(rect.x);
        let y = self.snap(rect.y);
        Rect {
            x,
            y,
            w: (self.snap(rect.right()) - x).max(0.0),
            h: (self.snap(rect.bottom()) - y).max(0.0),
        }
    }

    /// Whether the four edges of `rect` lie on the physical grid.
    pub fn is_rect_on_grid(self, rect: &Rect) -> bool {
        self.is_on_grid(rect.x)
            && self.is_on_grid(rect.y)
            && self.is_on_grid(rect.right())
            && self.is_on_grid(rect.bottom())
    }
}

/// The candidate nearest to `from` in `direction`: among those lying wholly on that side of it and
/// overlapping it on the other axis, the closest; then the one sharing the most of that axis; then
/// the topmost or leftmost. Used where windows have no order of their own (dwindle, floating).
pub(crate) fn nearest_in_direction<'a, T: Copy + 'a>(
    from: Rect,
    direction: Direction,
    candidates: impl IntoIterator<Item = &'a (T, Rect)>,
) -> Option<T> {
    let mut best: Option<(f64, f64, f64, T)> = None;
    for &(item, rect) in candidates {
        let (distance, overlap, across) = match direction {
            Direction::Left => (from.x - rect.right(), overlap_y(&from, &rect), rect.y),
            Direction::Right => (rect.x - from.right(), overlap_y(&from, &rect), rect.y),
            Direction::Up => (from.y - rect.bottom(), overlap_x(&from, &rect), rect.x),
            Direction::Down => (rect.y - from.bottom(), overlap_x(&from, &rect), rect.x),
        };
        if distance < -EPSILON || overlap <= EPSILON {
            continue;
        }
        let better = match best {
            None => true,
            Some((d, o, a, _)) => {
                distance < d - EPSILON
                    || (distance < d + EPSILON
                        && (overlap > o + EPSILON || (overlap > o - EPSILON && across < a)))
            }
        };
        if better {
            best = Some((distance, overlap, across, item));
        }
    }
    best.map(|(_, _, _, item)| item)
}

fn overlap_x(a: &Rect, b: &Rect) -> f64 {
    a.right().min(b.right()) - a.x.max(b.x)
}

fn overlap_y(a: &Rect, b: &Rect) -> f64 {
    a.bottom().min(b.bottom()) - a.y.max(b.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_in_direction_takes_the_closest_overlapping_one() {
        let from = Rect::new(0.0, 0.0, 10.0, 10.0);
        let candidates = [
            (1, Rect::new(30.0, 0.0, 10.0, 10.0)),
            (2, Rect::new(15.0, 5.0, 10.0, 10.0)),
            (3, Rect::new(12.0, 20.0, 10.0, 10.0)), // closer, but not beside `from`
        ];
        assert_eq!(
            nearest_in_direction(from, Direction::Right, &candidates),
            Some(2)
        );
        assert_eq!(
            nearest_in_direction(from, Direction::Left, &candidates),
            None
        );
    }

    #[test]
    fn scale_refuses_nonsense() {
        assert!(Scale::new(0.0).is_none());
        assert!(Scale::new(-1.0).is_none());
        assert!(Scale::new(f64::NAN).is_none());
        assert!(Scale::new(f64::INFINITY).is_none());
        assert!(Scale::new(0.1).is_none());
        assert!(Scale::new(11.0).is_none());
        assert_eq!(Scale::new(1.5).map(Scale::get), Some(1.5));
    }

    #[test]
    fn snapping_at_one_and_a_half_lands_on_whole_physical_pixels() {
        let scale = Scale::new(1.5).unwrap();
        // 1 logical unit is 1.5 physical pixels: snapped to 1 physical pixel, 0.667 logical.
        assert!((scale.snap(0.9) - 2.0 / 3.0).abs() < EPSILON);
        assert!(scale.is_on_grid(scale.snap(123.456)));
        assert!(!scale.is_on_grid(1.0));
    }

    #[test]
    fn neighbours_stay_neighbours_after_snapping() {
        let scale = Scale::new(1.25).unwrap();
        let left = Rect::new(0.3, 0.0, 100.37, 50.0);
        let right = Rect::new(left.right(), 0.0, 77.1, 50.0);
        let (left, right) = (scale.snap_rect(left), scale.snap_rect(right));
        assert!((left.right() - right.x).abs() < EPSILON);
        assert!(scale.is_rect_on_grid(&left) && scale.is_rect_on_grid(&right));
    }

    #[test]
    fn touching_rectangles_do_not_overlap() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(!a.overlaps(&Rect::new(10.0, 0.0, 10.0, 10.0)));
        assert!(a.overlaps(&Rect::new(9.0, 9.0, 10.0, 10.0)));
    }
}
