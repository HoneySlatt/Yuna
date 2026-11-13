//! Yuna's clock: the time the frame loop reads, and the deadlines it waits for
//! ([ADR-0023](../../../docs/adr/0023-frames-presentation-and-the-clock.md)).
//!
//! Every timer of the frame loop is a deadline the clock holds. In real mode, time is
//! `CLOCK_MONOTONIC` and the compositor arms one calloop timer at the earliest deadline. In manual
//! mode, for tests, time moves only when [`Compositor::advance`](crate::Compositor::advance) moves
//! it, and every deadline it passes runs, in order. Either way, nothing is armed at rest.

use std::time::Duration;

use smithay::utils::{Clock as SystemClock, Monotonic};
use yuna_layout::OutputId;

/// How the clock's time moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockMode {
    /// `CLOCK_MONOTONIC`: deadlines fire on their own.
    Real,
    /// Time starts at the real monotonic time and moves only when a test advances it.
    Manual,
}

/// What a deadline is for. Each is armed at most once at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Deadline {
    /// An output's next refresh: at its boundary minus its lead, or at once when it is idle.
    Refresh(OutputId),
    /// The boundary an output's held presentation feedback waits for.
    Presentation(OutputId),
    /// The next frame callbacks of windows that are not shown.
    HiddenFrames,
}

/// The time and the armed deadlines. See the module's documentation.
#[derive(Debug)]
pub struct Clock {
    mode: ClockMode,
    system: SystemClock<Monotonic>,
    /// Manual mode's time; unused in real mode.
    manual_now: Duration,
    /// Armed deadlines, in the order they were first armed: two per output and one more at
    /// most, so a scan costs less than any ordered structure.
    deadlines: Vec<(Duration, Deadline)>,
}

impl Clock {
    pub(crate) fn new(mode: ClockMode) -> Clock {
        let system = SystemClock::new();
        let manual_now = system.now().into();
        Clock {
            mode,
            system,
            manual_now,
            deadlines: Vec::new(),
        }
    }

    /// How time moves.
    pub fn mode(&self) -> ClockMode {
        self.mode
    }

    /// The current time, on `CLOCK_MONOTONIC`'s scale.
    pub fn now(&self) -> Duration {
        match self.mode {
            ClockMode::Real => self.system.now().into(),
            ClockMode::Manual => self.manual_now,
        }
    }

    /// In manual mode, moves time forward by `by` without running any deadline, as work that
    /// takes time would: the deadlines it passes run when the advance in progress goes on. In real
    /// mode, does nothing: real time moves by itself.
    pub fn pass(&mut self, by: Duration) {
        if self.mode == ClockMode::Manual {
            self.manual_now = self.manual_now.saturating_add(by);
        }
    }

    /// Sets manual time to `to`, unless that would move it back.
    pub(crate) fn move_to(&mut self, to: Duration) {
        if self.mode == ClockMode::Manual {
            self.manual_now = self.manual_now.max(to);
        }
    }

    /// Arms `deadline` at `at`; if it is already armed, it keeps the earlier of the two times.
    pub(crate) fn arm(&mut self, deadline: Deadline, at: Duration) {
        match self.deadlines.iter_mut().find(|(_, d)| *d == deadline) {
            Some((armed_at, _)) => *armed_at = (*armed_at).min(at),
            None => self.deadlines.push((at, deadline)),
        }
    }

    /// Disarms `deadline`, if it is armed.
    pub(crate) fn disarm(&mut self, deadline: Deadline) {
        self.deadlines.retain(|(_, d)| *d != deadline);
    }

    /// Whether `deadline` is armed.
    pub(crate) fn is_armed(&self, deadline: Deadline) -> bool {
        self.deadlines.iter().any(|(_, d)| *d == deadline)
    }

    /// The earliest armed deadline's time; none at rest.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.deadlines.iter().map(|(at, _)| *at).min()
    }

    /// Disarms and returns the earliest deadline due at `until` or before; of two due at the same
    /// time, the one armed first.
    pub(crate) fn take_due(&mut self, until: Duration) -> Option<(Duration, Deadline)> {
        let index = self
            .deadlines
            .iter()
            .enumerate()
            .filter(|(_, (at, _))| *at <= until)
            .min_by_key(|(_, (at, _))| *at)
            .map(|(index, _)| index)?;
        Some(self.deadlines.remove(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn manual_time_moves_only_when_told_and_never_back() {
        let mut clock = Clock::new(ClockMode::Manual);
        let start = clock.now();
        assert_eq!(clock.now(), start);
        clock.pass(3 * MS);
        assert_eq!(clock.now(), start + 3 * MS);
        clock.move_to(start);
        assert_eq!(clock.now(), start + 3 * MS);
    }

    #[test]
    fn manual_time_starts_at_the_real_monotonic_time() {
        let real = Clock::new(ClockMode::Real);
        let before = real.now();
        let manual = Clock::new(ClockMode::Manual);
        assert!(manual.now() >= before && manual.now() <= real.now());
    }

    #[test]
    fn a_deadline_armed_twice_keeps_its_earlier_time() {
        let mut clock = Clock::new(ClockMode::Manual);
        let t = clock.now();
        clock.arm(Deadline::HiddenFrames, t + 5 * MS);
        clock.arm(Deadline::HiddenFrames, t + 9 * MS);
        assert_eq!(clock.next_deadline(), Some(t + 5 * MS));
        clock.arm(Deadline::HiddenFrames, t + 2 * MS);
        assert_eq!(clock.next_deadline(), Some(t + 2 * MS));
    }

    #[test]
    fn due_deadlines_come_out_in_order_of_time_then_of_arming() {
        let mut clock = Clock::new(ClockMode::Manual);
        let t = clock.now();
        let (a, b) = (OutputId(1), OutputId(2));
        clock.arm(Deadline::Refresh(a), t + 4 * MS);
        clock.arm(Deadline::HiddenFrames, t + 2 * MS);
        clock.arm(Deadline::Refresh(b), t + 4 * MS);
        assert_eq!(clock.take_due(t + MS), None);
        let due: Vec<_> = std::iter::from_fn(|| clock.take_due(t + 4 * MS)).collect();
        assert_eq!(
            due,
            vec![
                (t + 2 * MS, Deadline::HiddenFrames),
                (t + 4 * MS, Deadline::Refresh(a)),
                (t + 4 * MS, Deadline::Refresh(b)),
            ]
        );
        assert_eq!(clock.next_deadline(), None);
    }

    #[test]
    fn disarming_leaves_nothing_armed() {
        let mut clock = Clock::new(ClockMode::Manual);
        let t = clock.now();
        clock.arm(Deadline::HiddenFrames, t);
        assert!(clock.is_armed(Deadline::HiddenFrames));
        clock.disarm(Deadline::HiddenFrames);
        assert!(!clock.is_armed(Deadline::HiddenFrames));
        assert_eq!(clock.next_deadline(), None);
    }

    #[test]
    fn real_mode_ignores_manual_moves() {
        let mut clock = Clock::new(ClockMode::Real);
        let before = clock.now();
        clock.pass(Duration::from_secs(3600));
        assert!(clock.now() < before + Duration::from_secs(3600));
    }
}
