//! The manager's time: what its transfers and drags stamp when they move, and what their time
//! limits are checked against (`bridge.rs`, `xdnd.rs`), on `CLOCK_MONOTONIC`'s scale, as Yuna's
//! clock gives it (`clock.rs`), so that the limit the manager tells Yuna is a time of Yuna's
//! ([ADR-0152](../../../../../docs/adr/0152-the-table-of-tasks-one-spawner-the-x11-manager-inline-at-m2-17.md)).
//!
//! It is chosen at the manager's start from Yuna's clock. On the real clock it is the monotonic
//! clock, as Yuna's own is. On the manual clock it is the last of Yuna's times the manager took,
//! each in its place among Yuna's commands (`owed.rs`), from Yuna's time at its start: it moves
//! only when Yuna's clock does, never on its own, and never back. The watchdog of the commands
//! the manager has not taken stays on real time (`owed.rs`): it judges a stalled thread, not the
//! story a test tells.

use std::cell::Cell;
use std::time::Duration;

use crate::ClockMode;
use crate::clock::monotonic_now;

/// Where the manager's "now" comes from. See the module's documentation.
#[derive(Debug)]
pub(crate) enum ManagerTime {
    /// `CLOCK_MONOTONIC`, read at each look.
    Real,
    /// The latest of Yuna's times the manager took.
    Manual(Cell<Duration>),
}

impl ManagerTime {
    /// The manager's time for Yuna's clock in `mode`, whose time is `yunas_now`.
    pub(crate) fn new(mode: ClockMode, yunas_now: Duration) -> ManagerTime {
        match mode {
            ClockMode::Real => ManagerTime::Real,
            ClockMode::Manual => ManagerTime::Manual(Cell::new(yunas_now)),
        }
    }

    /// Whether it is Yuna's manual time.
    pub(crate) fn is_manual(&self) -> bool {
        matches!(self, ManagerTime::Manual(_))
    }

    /// Now, on `CLOCK_MONOTONIC`'s scale.
    pub(crate) fn now(&self) -> Duration {
        match self {
            ManagerTime::Real => monotonic_now(),
            ManagerTime::Manual(now) => now.get(),
        }
    }

    /// Moves the manual time to Yuna's time `yunas_now`, sent by Yuna, unless that would move it
    /// back, as [`Clock::move_to`](crate::clock::Clock::move_to) does. Real time moves by
    /// itself: Yuna sends none, and it takes none.
    pub(crate) fn move_to(&self, yunas_now: Duration) {
        if let ManagerTime::Manual(now) = self {
            now.set(now.get().max(yunas_now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Clock;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn the_real_time_is_the_monotonic_clock_yunas_reads() {
        let yunas = Clock::new(ClockMode::Real);
        let time = ManagerTime::new(ClockMode::Real, Duration::ZERO);
        assert!(!time.is_manual());
        let before = yunas.now();
        let now = time.now();
        assert!(before <= now && now <= yunas.now(), "Yuna's scale");
        std::thread::sleep(Duration::from_millis(5));
        assert!(
            time.now() >= now + Duration::from_millis(5),
            "it moves by itself"
        );
        // Yuna sends no time on the real clock; one sent would change nothing.
        time.move_to(now + 3600 * SECOND);
        assert!(time.now() < now + 3600 * SECOND);
    }

    #[test]
    fn the_manual_time_is_the_latest_yuna_sent_and_never_moves_alone_nor_back() {
        let start = monotonic_now();
        let time = ManagerTime::new(ClockMode::Manual, start);
        assert!(time.is_manual());
        assert_eq!(time.now(), start, "Yuna's time at the start");
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(time.now(), start, "it never moves on its own");
        time.move_to(start + 7 * SECOND);
        assert_eq!(time.now(), start + 7 * SECOND);
        time.move_to(start + 2 * SECOND);
        assert_eq!(time.now(), start + 7 * SECOND, "an older time is ignored");
        time.move_to(start + 7 * SECOND);
        assert_eq!(time.now(), start + 7 * SECOND);
    }
}
