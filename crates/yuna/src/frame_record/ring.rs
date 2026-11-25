//! A ring of numbered entries, of a fixed size made whole at once: what the frame record keeps of
//! each seat's inputs, commits and frames.

use std::collections::VecDeque;

/// The newest entries pushed, at most its size, numbered from 1 in the order they came: a push
/// past its size drops the oldest and counts it. Its room is made when it is, so a push never
/// allocates.
#[derive(Debug)]
pub(crate) struct Ring<T> {
    entries: VecDeque<T>,
    size: usize,
    /// The number of the newest entry; 0 before the first.
    last: u64,
}

impl<T> Ring<T> {
    /// An empty ring that keeps `size` entries, at least one.
    pub(crate) fn new(size: usize) -> Ring<T> {
        let size = size.max(1);
        Ring {
            entries: VecDeque::with_capacity(size),
            size,
            last: 0,
        }
    }

    /// Pushes `entry`, dropping the oldest when the ring is full, and gives its number.
    pub(crate) fn push(&mut self, entry: T) -> u64 {
        if self.entries.len() == self.size {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
        self.last += 1;
        self.last
    }

    /// The newest entry's number; 0 before the first.
    pub(crate) fn last(&self) -> u64 {
        self.last
    }

    /// How many entries were dropped: those numbered 1 to this. Exact, since entries are dropped
    /// oldest first and numbered without gaps.
    pub(crate) fn dropped(&self) -> u64 {
        self.last - self.entries.len() as u64
    }

    /// How many entries numbered above `after` were dropped.
    pub(crate) fn missed(&self, after: u64) -> u64 {
        self.dropped().saturating_sub(after)
    }

    /// The entries kept numbered from `first` to `last`, both included, oldest first, each with
    /// its number: those dropped are left out.
    pub(crate) fn range(&self, first: u64, last: u64) -> impl Iterator<Item = (u64, &T)> {
        let dropped = self.dropped();
        let first = first.max(dropped + 1);
        let last = last.min(self.last);
        let (start, end) = if first > last {
            (0, 0)
        } else {
            // Within the entries kept: `first - dropped - 1` to `last - dropped`, at most their
            // count, which fits a `usize` as it is one.
            ((first - dropped - 1) as usize, (last - dropped) as usize)
        };
        self.entries
            .range(start..end)
            .zip(first..)
            .map(|(entry, number)| (number, entry))
    }

    /// The entries kept numbered above `after`, oldest first, each with its number.
    pub(crate) fn after(&self, after: u64) -> impl Iterator<Item = (u64, &T)> {
        self.range(after.saturating_add(1), self.last)
    }

    /// The newest entry, for it to be completed in place.
    pub(crate) fn newest_mut(&mut self) -> Option<&mut T> {
        self.entries.back_mut()
    }

    /// The entries kept, newest first, to be changed in place.
    pub(crate) fn newest_first_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.entries.iter_mut().rev()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// What a ring of `size` should hold after `pushes` entries: the model, every entry kept.
    fn kept(size: usize, pushes: u64) -> Vec<u64> {
        let first = pushes.saturating_sub(size as u64) + 1;
        (first..=pushes).collect()
    }

    #[test]
    fn a_full_ring_drops_its_oldest_and_counts_it_without_growing() {
        let mut ring = Ring::new(3);
        let capacity = ring.entries.capacity();
        assert_eq!((ring.last(), ring.dropped()), (0, 0));
        assert_eq!(ring.after(0).count(), 0);
        for value in 1..=10_u64 {
            assert_eq!(ring.push(value * 10), value);
        }
        assert_eq!(ring.entries.capacity(), capacity, "a push allocated");
        assert_eq!((ring.last(), ring.dropped()), (10, 7));
        let all: Vec<(u64, u64)> = ring.after(0).map(|(n, v)| (n, *v)).collect();
        assert_eq!(all, vec![(8, 80), (9, 90), (10, 100)]);
        assert_eq!(ring.missed(0), 7);
        assert_eq!(ring.missed(5), 2);
        assert_eq!(ring.missed(7), 0);
        assert_eq!(ring.missed(10), 0);
        assert_eq!(
            ring.range(9, 9).map(|(n, _)| n).collect::<Vec<_>>(),
            vec![9]
        );
        assert_eq!(
            ring.range(1, 8).map(|(n, _)| n).collect::<Vec<_>>(),
            vec![8]
        );
        assert_eq!(ring.range(9, 8).count(), 0);
        assert_eq!(ring.range(11, 20).count(), 0);
        assert_eq!(ring.after(10).count(), 0);
        assert_eq!(ring.after(u64::MAX).count(), 0);
        *ring.newest_mut().expect("an entry") += 1;
        assert_eq!(ring.range(10, 10).next(), Some((10, &101)));
    }

    #[test]
    fn a_ring_of_no_size_keeps_one() {
        let mut ring = Ring::new(0);
        ring.push('a');
        ring.push('b');
        assert_eq!(ring.after(0).collect::<Vec<_>>(), vec![(2, &'b')]);
        assert_eq!(ring.missed(0), 1);
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 2_000, ..ProptestConfig::default() })]

        /// Against a model that keeps every entry: each range read gives exactly the numbers kept
        /// in it, each with the entry pushed under that number, and `missed` counts exactly the
        /// numbers above the cursor that were dropped.
        #[test]
        fn a_ring_reads_as_its_model(
            size in 1_usize..40,
            pushes in 0_u64..200,
            first in 0_u64..220,
            length in 0_u64..60,
        ) {
            let mut ring = Ring::new(size);
            for value in 1..=pushes {
                prop_assert_eq!(ring.push(value * 3), value);
            }
            let kept = kept(size, pushes);
            let last = first.saturating_add(length);
            let read: Vec<(u64, u64)> = ring.range(first, last).map(|(n, v)| (n, *v)).collect();
            let expected: Vec<(u64, u64)> = kept
                .iter()
                .filter(|n| **n >= first && **n <= last)
                .map(|n| (*n, n * 3))
                .collect();
            prop_assert_eq!(read, expected);
            let dropped = pushes.saturating_sub(size as u64);
            prop_assert_eq!(ring.missed(first), dropped.saturating_sub(first));
            prop_assert_eq!(ring.after(first).count() as u64, pushes.saturating_sub(first.max(dropped)));
        }
    }
}
