//! Output scales, the physical pixel grid and the choice of the nearest rectangle in a direction.
//!
//! The logical geometry itself, `Point`, `Size`, `Rect` and `EPSILON`, lives in `yuna-types`
//! ([ADR-0157](../../../docs/adr/0157-yuna-types-takes-the-logical-geometry-and-a-windows-shell.md)).
//!
//! The model works in logical coordinates, shared by every output, as `f64`: at a fractional scale,
//! most physical pixel edges fall between integer logical values (at 1.5, physical pixel 1 is at
//! logical 0.667). Every rectangle the model gives out is snapped to the physical grid of its
//! output, so that windows and borders land on whole pixels and stay crisp
//! ([ADR-0020](../../../docs/adr/0020-the-layout-models-shape.md)).

use crate::Direction;
use yuna_types::{EPSILON, Rect};

/// An output's scale: how many physical pixels one logical unit spans.
///
/// Always finite and positive; [`Scale::new`] refuses anything else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale(f64);

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

    /// `value` moved to the nearest edge of a physical pixel; a value half way between two goes
    /// to the higher one, so that a value moved by whole pixels snaps by as many
    /// ([ADR-0052](../../../docs/adr/0052-motion-in-a-pure-crate.md)).
    pub fn snap(self, value: f64) -> f64 {
        (value * self.0 + 0.5).floor() / self.0
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

    /// Four candidates around `from`, each at a distance of 10, 20, 30 or 40 on its side.
    fn around(from: &Rect) -> [(&'static str, Rect); 4] {
        [
            ("left", Rect::new(from.x - 20.0, from.y, 10.0, 10.0)),
            ("right", Rect::new(from.right() + 30.0, from.y, 10.0, 10.0)),
            ("up", Rect::new(from.x, from.y - 40.0, 10.0, 10.0)),
            ("down", Rect::new(from.x, from.bottom() + 10.0, 10.0, 10.0)),
        ]
    }

    #[test]
    fn each_direction_picks_the_candidate_on_its_own_side() {
        let from = Rect::new(100.0, 100.0, 10.0, 10.0);
        let candidates = around(&from);
        for (direction, expected) in [
            (Direction::Left, "left"),
            (Direction::Right, "right"),
            (Direction::Up, "up"),
            (Direction::Down, "down"),
        ] {
            assert_eq!(
                nearest_in_direction(from, direction, &candidates),
                Some(expected)
            );
        }
    }

    #[test]
    fn the_closer_candidate_wins_whatever_the_order_and_the_direction() {
        let from = Rect::new(100.0, 100.0, 10.0, 10.0);
        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            // Distances 10 and 50 along the direction, both beside `from` on the other axis.
            let place = |distance: f64| match direction {
                Direction::Left => Rect::new(from.x - distance - 10.0, from.y, 10.0, 10.0),
                Direction::Right => Rect::new(from.right() + distance, from.y, 10.0, 10.0),
                Direction::Up => Rect::new(from.x, from.y - distance - 10.0, 10.0, 10.0),
                Direction::Down => Rect::new(from.x, from.bottom() + distance, 10.0, 10.0),
            };
            let (near, far) = ((1, place(10.0)), (2, place(50.0)));
            assert_eq!(nearest_in_direction(from, direction, &[near, far]), Some(1));
            assert_eq!(nearest_in_direction(from, direction, &[far, near]), Some(1));
        }
    }

    #[test]
    fn a_candidate_touching_the_focused_rectangle_is_the_nearest_on_its_side() {
        let from = Rect::new(100.0, 100.0, 10.0, 10.0);
        let touching = [
            (Direction::Left, Rect::new(90.0, 100.0, 10.0, 10.0)),
            (Direction::Right, Rect::new(110.0, 100.0, 10.0, 10.0)),
            (Direction::Up, Rect::new(100.0, 90.0, 10.0, 10.0)),
            (Direction::Down, Rect::new(100.0, 110.0, 10.0, 10.0)),
        ];
        for (direction, rect) in touching {
            let far = Rect::new(rect.x * 2.0 - 100.0, rect.y * 2.0 - 100.0, 10.0, 10.0);
            assert_eq!(
                nearest_in_direction(from, direction, &[(2, far), (1, rect)]),
                Some(1),
                "{direction:?}"
            );
        }
    }

    #[test]
    fn a_candidate_overlapping_or_sharing_no_extent_across_is_never_chosen() {
        let from = Rect::new(100.0, 100.0, 10.0, 10.0);
        let candidates = [
            // Overlaps `from` itself: it lies on no side of it.
            (1, Rect::new(105.0, 100.0, 10.0, 10.0)),
            // Beside it but only touching across: no shared extent on the other axis.
            (2, Rect::new(120.0, 110.0, 10.0, 10.0)),
            (3, Rect::new(120.0, 90.0, 10.0, 10.0)),
            (4, Rect::new(110.0, 120.0, 10.0, 10.0)),
        ];
        for direction in [Direction::Right, Direction::Down] {
            assert_eq!(nearest_in_direction(from, direction, &candidates), None);
        }
        let candidates = [
            (1, Rect::new(95.0, 100.0, 10.0, 10.0)),
            (2, Rect::new(80.0, 110.0, 10.0, 10.0)),
            (3, Rect::new(80.0, 90.0, 10.0, 10.0)),
            (4, Rect::new(90.0, 80.0, 10.0, 10.0)),
        ];
        for direction in [Direction::Left, Direction::Up] {
            assert_eq!(nearest_in_direction(from, direction, &candidates), None);
        }
    }

    #[test]
    fn at_the_same_distance_the_candidate_sharing_more_of_the_other_axis_wins() {
        let from = Rect::new(100.0, 100.0, 10.0, 10.0);
        // Both 20 away on their side, one sharing 3 units across, the other 8.
        let cases = [
            (
                Direction::Right,
                Rect::new(130.0, 107.0, 10.0, 10.0),
                Rect::new(130.0, 102.0, 10.0, 10.0),
            ),
            (
                Direction::Left,
                Rect::new(70.0, 107.0, 10.0, 10.0),
                Rect::new(70.0, 102.0, 10.0, 10.0),
            ),
            (
                Direction::Down,
                Rect::new(107.0, 130.0, 10.0, 10.0),
                Rect::new(102.0, 130.0, 10.0, 10.0),
            ),
            (
                Direction::Up,
                Rect::new(107.0, 70.0, 10.0, 10.0),
                Rect::new(102.0, 70.0, 10.0, 10.0),
            ),
        ];
        for (direction, little, more) in cases {
            let (little, more) = ((1, little), (2, more));
            assert_eq!(
                nearest_in_direction(from, direction, &[little, more]),
                Some(2),
                "{direction:?}"
            );
            assert_eq!(
                nearest_in_direction(from, direction, &[more, little]),
                Some(2),
                "{direction:?}"
            );
        }
    }

    #[test]
    fn at_the_same_distance_and_overlap_the_topmost_or_leftmost_wins_and_a_full_tie_keeps_the_first()
     {
        let from = Rect::new(100.0, 100.0, 10.0, 30.0);
        // Both on the right at 20, sharing 10 units of height: the upper one starts higher.
        let upper = (1, Rect::new(130.0, 90.0, 10.0, 20.0));
        let lower = (2, Rect::new(130.0, 120.0, 10.0, 20.0));
        assert_eq!(
            nearest_in_direction(from, Direction::Right, &[upper, lower]),
            Some(1)
        );
        assert_eq!(
            nearest_in_direction(from, Direction::Right, &[lower, upper]),
            Some(1)
        );
        // Below `from`, the leftmost wins.
        let from = Rect::new(100.0, 100.0, 30.0, 10.0);
        let left = (1, Rect::new(90.0, 130.0, 20.0, 10.0));
        let right = (2, Rect::new(120.0, 130.0, 20.0, 10.0));
        assert_eq!(
            nearest_in_direction(from, Direction::Down, &[left, right]),
            Some(1)
        );
        assert_eq!(
            nearest_in_direction(from, Direction::Down, &[right, left]),
            Some(1)
        );
        // The same rectangle twice: the first stays.
        let same = Rect::new(130.0, 100.0, 10.0, 30.0);
        assert_eq!(
            nearest_in_direction(from, Direction::Right, &[(1, same), (2, same)]),
            Some(1)
        );
    }

    #[test]
    fn at_the_same_distance_more_shared_extent_wins_even_from_a_larger_across() {
        let from = Rect::new(100.0, 100.0, 10.0, 10.0);
        // Both 20 to the right: one starting above `from` and sharing 5, one sharing all 10.
        let less = (1, Rect::new(130.0, 90.0, 10.0, 15.0));
        let more = (2, Rect::new(130.0, 100.0, 10.0, 10.0));
        assert_eq!(
            nearest_in_direction(from, Direction::Right, &[less, more]),
            Some(2)
        );
        assert_eq!(
            nearest_in_direction(from, Direction::Right, &[more, less]),
            Some(2)
        );
    }

    #[test]
    fn a_candidate_closer_but_sharing_less_wins_over_one_farther_sharing_more() {
        let from = Rect::new(100.0, 100.0, 10.0, 10.0);
        let near_little = (1, Rect::new(120.0, 108.0, 10.0, 10.0));
        let far_more = (2, Rect::new(160.0, 100.0, 10.0, 10.0));
        assert_eq!(
            nearest_in_direction(from, Direction::Right, &[far_more, near_little]),
            Some(1)
        );
    }
}
