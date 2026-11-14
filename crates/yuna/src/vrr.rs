//! A VRR panel, emulated as amdgpu drives a DisplayPort Adaptive-Sync monitor
//! ([ADR-0045](../../../docs/adr/0045-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)).
//!
//! A refresh lasts at least the range's shortest period, 1/max, and at most its longest, 1/min.
//! A frame ready sooner than the shortest period after the refresh in progress began waits for
//! it; one ready later starts a refresh at once. With no new frame, the panel refreshes by itself
//! at the longest period.
//!
//! Below the range, amdgpu inserts refreshes (*BTR*, below-the-range): from the time between the
//! last two frames it picks a number `n` of refreshes of equal, fixed length near the range's
//! middle, so that a frame is shown `n` times. A frame ready during one of those fixed refreshes
//! waits for its end. [`Btr`] is `apply_below_the_range` of Linux's
//! `drivers/gpu/drm/amd/display/modules/freesync/freesync.c` (master `97d73c82`, the same as
//! 6.17), with its integer arithmetic in microseconds; the panel's lengths are kept in
//! nanoseconds.
//!
//! Nothing here is a timer: the refreshes the panel makes by itself are computed when a frame
//! asks when it would show, so that an output at rest wakes nothing.

use std::time::Duration;

/// freesync.c's `BTR_MAX_MARGIN`, in microseconds.
const BTR_MAX_MARGIN: u64 = 2_500;
/// freesync.c's `BTR_DRIFT_MARGIN`, in microseconds.
const BTR_DRIFT_MARGIN: u64 = 2_000;

/// amdgpu's below-the-range state: its parameters, from the range, and what the last frames
/// decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Btr {
    /// The range's longest and shortest periods, in microseconds, floored as freesync.c's.
    max_duration: u64,
    min_duration: u64,
    /// freesync.c's margin and the range's middle period.
    margin: u64,
    mid_point: u64,
    /// Whether it inserts refreshes now.
    active: bool,
    /// How many refreshes a frame is shown for, 0 when none are inserted, and each one's
    /// length, in microseconds.
    frames_to_insert: u64,
    inserted_duration: u64,
}

impl Btr {
    /// The state of a range of `min_millihertz` to `max_millihertz`, at least twice as wide:
    /// narrower ranges take another path of the driver, which Yuna does not emulate.
    pub(crate) fn new(min_millihertz: u32, max_millihertz: u32) -> Btr {
        // freesync.c, `mod_freesync_build_vrr_params`: durations from µHz, floored.
        let duration = |millihertz: u32| 1_000_000_000_000 / (u64::from(millihertz.max(1)) * 1_000);
        let max_duration = duration(min_millihertz);
        let min_duration = duration(max_millihertz);
        let margin = max_duration
            .saturating_sub(2 * min_duration)
            .min(BTR_MAX_MARGIN);
        Btr {
            max_duration,
            min_duration,
            margin,
            mid_point: (min_duration + max_duration) / 2,
            active: false,
            frames_to_insert: 0,
            inserted_duration: 0,
        }
    }

    /// `apply_below_the_range`, for a frame `last` microseconds after the one before.
    pub(crate) fn update(&mut self, last: u64) {
        let max_render = self.max_duration.saturating_sub(self.margin);
        let half_margin = self.margin / 2;
        if last.saturating_add(half_margin) < max_render {
            self.active = false;
        } else if last > max_render + half_margin {
            self.active = true;
        }
        // Between the two thresholds, the state stays as it was: freesync.c's hysteresis.
        if !self.active {
            self.frames_to_insert = 0;
            self.inserted_duration = 0;
            return;
        }
        let mid = self.mid_point.max(1);
        let ceil = last.div_ceil(mid);
        let floor = last / mid;
        // freesync.c starts each delta at 0xFFFFFFFF, kept when its count is 0.
        let delta = |count: u64| {
            last.checked_div(count)
                .map_or(u64::from(u32::MAX), |length| mid.abs_diff(length))
        };
        let (delta_ceil, delta_floor) = (delta(ceil), delta(floor));
        let mut count = if ceil > 0 && last / ceil < self.min_duration {
            floor
        } else if floor < 2 || delta_ceil < delta_floor {
            ceil
        } else {
            floor
        };
        // Keep the count of the frames before while the two choices stay close: no flip-flop.
        let previous = self.frames_to_insert;
        if previous != 0
            && delta_ceil.abs_diff(delta_floor) < BTR_DRIFT_MARGIN
            && last / previous < max_render
            && last / previous > self.min_duration
        {
            count = previous;
        }
        if count > 1 && last / count < self.min_duration {
            count -= 1;
        }
        let inserted = last.checked_div(count).unwrap_or(0);
        self.frames_to_insert = count;
        self.inserted_duration = inserted.max(self.min_duration);
    }

    /// How many refreshes the last frame is shown for, and each one's length; none when no
    /// refresh is inserted.
    pub(crate) fn inserted(&self) -> Option<(u64, Duration)> {
        (self.active && self.frames_to_insert > 0).then(|| {
            (
                self.frames_to_insert,
                Duration::from_micros(self.inserted_duration),
            )
        })
    }
}

/// A VRR panel: its range, and what it last showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Panel {
    /// The range's shortest and longest refresh.
    shortest: Duration,
    longest: Duration,
    btr: Btr,
    /// When the refresh that began showing the last frame started, and its number.
    last_start: Duration,
    last_seq: u64,
    /// When the last frame was ready: the time between two frames feeds BTR.
    last_ready: Option<Duration>,
}

impl Panel {
    /// A panel of `min_millihertz` to `max_millihertz`, whose refresh 0 starts at `start`.
    pub(crate) fn new(min_millihertz: u32, max_millihertz: u32, start: Duration) -> Panel {
        let period = |millihertz: u32| {
            Duration::from_nanos(1_000_000_000_000 / u64::from(millihertz.max(1)))
        };
        Panel {
            shortest: period(max_millihertz),
            longest: period(min_millihertz),
            btr: Btr::new(min_millihertz, max_millihertz),
            last_start: start,
            last_seq: 0,
            last_ready: None,
        }
    }

    /// The shortest refresh: what `wp_presentation` gives as a VRR output's refresh.
    pub(crate) fn shortest(&self) -> Duration {
        self.shortest
    }

    /// The refreshes after the last frame's, before the panel refreshes by itself at its
    /// longest period: `n − 1` inserted ones of a fixed length when BTR inserts `n`, none
    /// otherwise. Returns where those end and the least the last of them lasts.
    fn fixed(&self) -> (u64, Duration, Duration) {
        match self.btr.inserted() {
            Some((count, length)) => (count, length, length),
            None => (1, Duration::ZERO, self.shortest),
        }
    }

    /// When a frame ready at `ready` starts showing, and that refresh's number.
    pub(crate) fn first_reachable(&self, ready: Duration) -> (u64, Duration) {
        let ready = ready.max(self.last_start);
        let (count, length, least) = self.fixed();
        // The inserted refreshes: a frame ready during one waits for its end.
        let fixed_end = self
            .last_start
            .saturating_add(length.saturating_mul(count as u32));
        if count > 1 && ready < fixed_end {
            let since = (ready - self.last_start).as_nanos();
            let step = length.as_nanos().max(1);
            // At least the next refresh: the last frame's own began at `last_start`.
            let k = (since.div_ceil(step) as u64).max(1);
            let start = self.last_start.saturating_add(nanos(u128::from(k) * step));
            return (self.last_seq + k, start);
        }
        // From the last refresh that showed the last frame, its least length, then the panel's
        // own refreshes at its longest period.
        let base = self
            .last_start
            .saturating_add(length.saturating_mul(count.saturating_sub(1) as u32));
        let base_seq = self.last_seq + count - 1;
        if ready < base.saturating_add(least) {
            return (base_seq + 1, base.saturating_add(least));
        }
        let since = (ready - base).as_nanos();
        let longest = self.longest.as_nanos().max(1);
        let own = (since / longest) as u64;
        if own == 0 {
            return (base_seq + 1, ready);
        }
        let own_start = base.saturating_add(nanos(u128::from(own) * longest));
        let start = ready.max(own_start.saturating_add(self.shortest));
        (base_seq + own + 1, start)
    }

    /// Records that a frame ready at `ready` started showing at refresh `seq`, at `start`: BTR
    /// learns the time since the frame before.
    pub(crate) fn flip(&mut self, ready: Duration, seq: u64, start: Duration) {
        if let Some(before) = self.last_ready {
            // freesync.c times frames from their commits, in microseconds.
            let last = (ready.as_micros() as u64).saturating_sub(before.as_micros() as u64);
            self.btr.update(last);
        }
        self.last_ready = Some(ready);
        self.last_start = start;
        self.last_seq = seq;
    }
}

/// `nanos` nanoseconds, at most `u64::MAX` of them.
fn nanos(nanos: u128) -> Duration {
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 48 to 240 Hz, the owner's monitor.
    fn monitor_btr() -> Btr {
        Btr::new(48_000, 240_000)
    }

    /// BTR's choice for a steady rate, in microseconds between frames as the kernel floors
    /// them, after enough frames to settle. The floored intervals alternate by 1 µs (41,666 and
    /// 41,667 at 24 fps), and the inserted length with them: the last interval's is given.
    fn settled(fps: u64) -> Option<(u64, Duration)> {
        let mut btr = monitor_btr();
        let times: Vec<u64> = (0..200).map(|k| k * 1_000_000_000 / fps / 1_000).collect();
        for pair in times.windows(2) {
            btr.update(pair[1] - pair[0]);
        }
        btr.inserted()
    }

    #[test]
    fn the_ranges_parameters_are_freesyncs() {
        let btr = monitor_btr();
        assert_eq!(
            (
                btr.max_duration,
                btr.min_duration,
                btr.margin,
                btr.mid_point
            ),
            (20_833, 4_166, 2_500, 12_499)
        );
    }

    /// The figures freesync.c's own arithmetic gives, run apart in the M1.6 design's challenge
    /// (24 fps: 3 refreshes of 13,889 µs, not 2).
    #[test]
    fn films_and_slow_games_get_freesyncs_inserted_refreshes() {
        assert_eq!(settled(24), Some((3, Duration::from_micros(13_888))));
        assert_eq!(settled(30), Some((3, Duration::from_micros(11_111))));
        assert_eq!(settled(48).map(|(n, _)| n), Some(2));
        for fps in [60, 100, 144, 240, 300] {
            assert_eq!(settled(fps), None, "{fps} fps");
        }
    }

    #[test]
    fn btr_holds_its_state_between_its_thresholds() {
        // max_render 18,333 µs: off under 17,083, on above 19,583.
        let mut btr = monitor_btr();
        btr.update(18_000);
        assert_eq!(btr.inserted(), None, "off, and stays off");
        btr.update(20_000);
        assert!(btr.inserted().is_some(), "on");
        btr.update(18_000);
        assert!(btr.inserted().is_some(), "stays on");
        btr.update(17_000);
        assert_eq!(btr.inserted(), None, "off again");
    }

    #[test]
    fn extreme_intervals_neither_overflow_nor_divide_by_zero() {
        let mut btr = monitor_btr();
        for last in [0, 1, u64::from(u32::MAX), u64::MAX / 2, u64::MAX] {
            btr.update(last);
        }
        let mut narrow = Btr::new(1, 1);
        narrow.update(u64::MAX);
        let mut panel = Panel::new(48_000, 240_000, Duration::ZERO);
        panel.flip(Duration::ZERO, 0, Duration::ZERO);
        panel.flip(Duration::MAX, 1, Duration::MAX);
        panel.first_reachable(Duration::MAX);
    }

    /// A panel whose last frame started showing at 1 s, refresh 10, with no refresh inserted.
    fn panel_at_one_second() -> Panel {
        let mut panel = Panel::new(48_000, 240_000, Duration::ZERO);
        panel.flip(Duration::from_millis(990), 10, Duration::from_secs(1));
        panel
    }

    #[test]
    fn a_frame_sooner_than_the_shortest_refresh_waits_and_a_later_one_shows_at_once() {
        let panel = panel_at_one_second();
        let at = |ms: f64| Duration::from_secs(1) + Duration::from_secs_f64(ms / 1_000.0);
        // 1/240 s = 4,166,666 ns.
        let shortest = Duration::from_nanos(4_166_666);
        assert_eq!(
            panel.first_reachable(at(1.0)),
            (11, Duration::from_secs(1) + shortest)
        );
        assert_eq!(panel.first_reachable(at(10.0)), (11, at(10.0)));
        assert_eq!(panel.first_reachable(at(20.0)), (11, at(20.0)));
    }

    #[test]
    fn an_idle_panel_refreshes_by_itself_at_its_longest_period() {
        let panel = panel_at_one_second();
        // 1/48 s = 20,833,333 ns: the panel showed the frame again at 1.0208 s.
        let longest = Duration::from_nanos(20_833_333);
        let own = Duration::from_secs(1) + longest;
        // Ready just after that refresh began: it waits for the shortest refresh.
        let (seq, start) = panel.first_reachable(own + Duration::from_millis(1));
        assert_eq!((seq, start), (12, own + Duration::from_nanos(4_166_666)));
        // Ready later in it: at once; two of the panel's own refreshes passed after 50 ms.
        assert_eq!(
            panel.first_reachable(Duration::from_millis(1_050)),
            (13, Duration::from_millis(1_050))
        );
    }

    #[test]
    fn a_frame_during_an_inserted_refresh_waits_for_its_end() {
        let mut panel = Panel::new(48_000, 240_000, Duration::ZERO);
        // Frames at 24 fps until BTR inserts 3 refreshes.
        let frame = Duration::from_nanos(41_666_667);
        let mut seq = 0;
        let mut last = Duration::ZERO;
        for k in 1..10u32 {
            let at = frame * k;
            let (next, start) = panel.first_reachable(at);
            // From its third frame, once BTR has the time between two: before, a frame lands
            // where the panel's own refresh begins, and waits.
            // The inserted lengths are whole microseconds: a frame may wait for a fraction of
            // one, far less than a line of the monitor's scan.
            if k > 2 {
                assert!(
                    start - at < Duration::from_micros(1),
                    "{:?} late",
                    start - at
                );
                assert_eq!(next - seq, 3, "each shown three times");
            }
            seq = next;
            last = start;
            panel.flip(at, next, start);
        }
        let inserted = panel.btr.inserted().expect("inserted refreshes").1;
        // Ready 5 ms after the last frame: during its first refresh, which ends after 13.89 ms.
        assert_eq!(
            panel.first_reachable(last + Duration::from_millis(5)),
            (seq + 1, last + inserted)
        );
        // Ready 20 ms after: during the second, which ends after 27.78 ms.
        assert_eq!(
            panel.first_reachable(last + Duration::from_millis(20)),
            (seq + 2, last + inserted * 2)
        );
    }

    /// The owner's monitor's trace (ADR-0045): each rate's flips, as `vrr-trace` recorded them.
    const TRACE: &str = include_str!("../tests/fixtures/vrr-trace-pg32ucdm.txt");

    /// A flip of the trace: when it was submitted, when its event came, and its refresh.
    struct TracedFlip {
        submit: Duration,
        event: Duration,
        seq: u64,
    }

    /// The trace's rates, each with its flips, in order.
    fn traced_rates() -> Vec<(u32, Vec<TracedFlip>)> {
        let mut rates: Vec<(u32, Vec<TracedFlip>)> = Vec::new();
        for line in TRACE.lines().filter(|line| !line.starts_with('#')) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            match fields[..] {
                ["rate", millihertz] => {
                    rates.push((millihertz.parse().expect("a rate"), Vec::new()));
                }
                [submit, event, seq] => {
                    let number = |text: &str| text.parse::<u64>().expect("a number");
                    rates.last_mut().expect("a rate first").1.push(TracedFlip {
                        submit: Duration::from_nanos(number(submit)),
                        event: Duration::from_nanos(number(event)),
                        seq: number(seq),
                    });
                }
                _ => panic!("a malformed line: {line}"),
            }
        }
        rates
    }

    /// Replays a rate's submissions through the panel, its first flip as the panel's start, and
    /// gives, for each flip after `warm_up` whose submission and the one before came within
    /// 200 µs of the rate's period, the error of the interval since the flip before and whether
    /// the count of refreshes between them matched. Frames submitted late are left out: the
    /// monitor holds them longer than the emulation does ([debt](../../../docs/debt.md#vrr)).
    fn replay_steady(
        millihertz: u32,
        flips: &[TracedFlip],
        warm_up: usize,
    ) -> Vec<(Duration, bool)> {
        let period = Duration::from_nanos(1_000_000_000_000 / u64::from(millihertz));
        let on_time = |k: usize| {
            (flips[k].submit - flips[k - 1].submit).abs_diff(period) < Duration::from_micros(200)
        };
        let first = &flips[0];
        let mut panel = Panel::new(48_000, 240_016, first.event);
        panel.flip(first.submit, 0, first.event);
        let mut before = (0, first.event);
        let mut found = Vec::new();
        for (k, flip) in flips.iter().enumerate().skip(1) {
            let (seq, start) = panel.first_reachable(flip.submit);
            panel.flip(flip.submit, seq, start);
            if k > warm_up && on_time(k) && on_time(k - 1) {
                let traced = flip.event - flips[k - 1].event;
                found.push((
                    (start - before.1).abs_diff(traced),
                    seq - before.0 == flip.seq - flips[k - 1].seq,
                ));
            }
            before = (seq, start);
        }
        found
    }

    /// ADR-0045's check: the owner's monitor's trace, replayed. For frames on time, at every rate
    /// from 23.976 to 300 fps, the time between two presentations is the monitor's within
    /// 100 µs for at least 85 % of them (the trace's event times are whole microseconds, its
    /// submissions jitter), and the count of refreshes between them is the monitor's for at
    /// least 95 %. At 25 fps, the monitor keeps 4 refreshes a frame after a perturbation where
    /// the emulation goes back to 3, about half the time: at least 50 % there.
    #[test]
    fn the_emulation_replays_the_owners_monitor() {
        let rates = traced_rates();
        assert_eq!(rates.len(), 10);
        for (millihertz, flips) in rates {
            let found = replay_steady(millihertz, &flips, 10);
            assert!(
                found.len() > 150,
                "{millihertz}: {} frames on time",
                found.len()
            );
            let share = |count: usize| count as f64 / found.len() as f64;
            let timed = share(
                found
                    .iter()
                    .filter(|(error, _)| *error <= Duration::from_micros(100))
                    .count(),
            );
            let counted = share(found.iter().filter(|(_, same)| *same).count());
            let least_counted = if millihertz == 25_000 { 0.5 } else { 0.95 };
            assert!(
                timed >= 0.85,
                "{millihertz}: {timed:.3} of intervals within 100 µs"
            );
            assert!(
                counted >= least_counted,
                "{millihertz}: {counted:.3} of refresh counts"
            );
        }
    }
}
