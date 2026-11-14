//! A VRR panel, emulated as amdgpu drives a DisplayPort Adaptive-Sync monitor
//! ([ADR-0045](../../../docs/adr/0045-emulated-vrr-amdgpus-rule-checked-on-the-monitor.md)).
//!
//! A refresh lasts at least the range's shortest period, 1/max, and at most its longest, 1/min.
//! A frame ready sooner than the shortest period after the refresh in progress began waits for
//! it; one ready later starts a refresh a vertical blank later. With no new frame, the panel
//! refreshes by itself at the longest period.
//!
//! Below the range, amdgpu inserts refreshes (*BTR*, below-the-range): from the time between the
//! last two frames it picks a number `n` of refreshes near the range's middle, so that a frame is
//! shown `n` times; the owner's monitor showed the first `n − 1` with a fixed length, and the last
//! variable again. A frame ready during a fixed one waits for its end. A frame shows a vertical
//! blank after it is ready at the soonest
//! ([ADR-0050](../../../docs/adr/0050-emulated-vrr-as-the-monitors-traces-showed.md)). [`Btr`] is `apply_below_the_range` of Linux's
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

/// The share of a refresh the owner's monitor spends in its vertical blank: 269 of its mode's
/// 2,429 lines at 240 Hz, 461 µs (its EDID's 3840 × 2160 mode, read in its trace). A flip must
/// come that long before a refresh begins showing to make it.
const BLANK_LINES: u32 = 269;
const TOTAL_LINES: u32 = 2_429;

/// A VRR panel: its range, and what it last showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Panel {
    /// The range's shortest and longest refresh.
    shortest: Duration,
    longest: Duration,
    /// Its vertical blank, at its shortest refresh.
    blank: Duration,
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
        let shortest = period(max_millihertz);
        Panel {
            shortest,
            longest: period(min_millihertz),
            blank: shortest * BLANK_LINES / TOTAL_LINES,
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

    /// Its vertical blank: the least time between a frame ready and its refresh beginning.
    pub(crate) fn blank(&self) -> Duration {
        self.blank
    }

    /// The refreshes of a fixed length that show the last frame, and that length: when BTR
    /// inserts `n` refreshes, the first `n − 1` of them, as the owner's monitor showed; the last
    /// one is variable again. None otherwise.
    fn fixed(&self) -> (u64, Duration) {
        match self.btr.inserted() {
            Some((count, length)) => (count - 1, length),
            None => (0, Duration::ZERO),
        }
    }

    /// When a frame ready at `ready` starts showing, and that refresh's number. A refresh
    /// begins showing a blank after its frame is ready at the soonest. During the fixed
    /// refreshes, a frame shows at the first one that begins after that. Then the panel's
    /// refresh is variable: it lasts at least the shortest refresh, ends when a frame comes, and
    /// at the longest refresh begins another, the panel's own.
    pub(crate) fn first_reachable(&self, ready: Duration) -> (u64, Duration) {
        let ready = ready.saturating_add(self.blank).max(self.last_start);
        let (fixed, length) = self.fixed();
        let base = self
            .last_start
            .saturating_add(length.saturating_mul(fixed as u32));
        let base_seq = self.last_seq + fixed;
        if fixed > 0 && ready <= base {
            let since = (ready - self.last_start).as_nanos();
            let step = length.as_nanos().max(1);
            // At least the next refresh: the last frame's own began at `last_start`.
            let k = (since.div_ceil(step) as u64).clamp(1, fixed);
            let start = self.last_start.saturating_add(nanos(u128::from(k) * step));
            return (self.last_seq + k, start);
        }
        // The variable refresh began at `base`, showing the last frame.
        if ready < base.saturating_add(self.shortest) {
            return (base_seq + 1, base.saturating_add(self.shortest));
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
    fn the_blank_is_the_owners_modes_share_of_a_refresh() {
        // 269 of 2,429 lines of 1/240 s: 461 µs.
        let panel = panel_at_one_second();
        assert_eq!(panel.blank().as_micros(), 461);
    }

    #[test]
    fn a_frame_sooner_than_the_shortest_refresh_waits_and_a_later_one_shows_a_blank_later() {
        let panel = panel_at_one_second();
        let blank = panel.blank();
        let at = |ms: f64| Duration::from_secs(1) + Duration::from_secs_f64(ms / 1_000.0);
        // 1/240 s = 4,166,666 ns.
        let shortest = Duration::from_nanos(4_166_666);
        assert_eq!(
            panel.first_reachable(at(1.0)),
            (11, Duration::from_secs(1) + shortest)
        );
        assert_eq!(panel.first_reachable(at(10.0)), (11, at(10.0) + blank));
        assert_eq!(panel.first_reachable(at(20.0)), (11, at(20.0) + blank));
    }

    #[test]
    fn an_idle_panel_refreshes_by_itself_at_its_longest_period() {
        let panel = panel_at_one_second();
        let blank = panel.blank();
        // 1/48 s = 20,833,333 ns: the panel showed the frame again at 1.0208 s.
        let longest = Duration::from_nanos(20_833_333);
        let own = Duration::from_secs(1) + longest;
        // Ready just after that refresh began: it waits for the shortest refresh.
        let (seq, start) = panel.first_reachable(own + Duration::from_millis(1));
        assert_eq!((seq, start), (12, own + Duration::from_nanos(4_166_666)));
        // Ready later in it: a blank later; two of the panel's own refreshes passed by 50 ms.
        assert_eq!(
            panel.first_reachable(Duration::from_millis(1_050)),
            (13, Duration::from_millis(1_050) + blank)
        );
    }

    /// As the owner's monitor showed: when BTR inserts `n` refreshes, the first `n − 1` have
    /// its fixed length, and a frame ready during one shows at its end; the last is variable.
    #[test]
    fn under_btr_all_but_the_last_inserted_refresh_are_fixed() {
        let mut panel = Panel::new(48_000, 240_000, Duration::ZERO);
        let blank = panel.blank();
        // Frames at 24 fps until BTR inserts 3 refreshes.
        let frame = Duration::from_nanos(41_666_667);
        let mut seq = 0;
        let mut last = Duration::ZERO;
        for k in 1..10u32 {
            let at = frame * k;
            let (next, start) = panel.first_reachable(at);
            // From its third frame, once BTR has the time between two: each shows a blank
            // after it is ready, three refreshes after the one before.
            if k > 2 {
                assert_eq!(start, at + blank);
                assert_eq!(next - seq, 3, "each shown three times");
            }
            seq = next;
            last = start;
            panel.flip(at, next, start);
        }
        let inserted = panel.btr.inserted().expect("inserted refreshes").1;
        let shortest = Duration::from_nanos(4_166_666);
        // Ready during the first fixed refresh, then the second: shown at their ends.
        assert_eq!(
            panel.first_reachable(last + Duration::from_millis(5)),
            (seq + 1, last + inserted)
        );
        assert_eq!(
            panel.first_reachable(last + Duration::from_millis(20)),
            (seq + 2, last + inserted * 2)
        );
        // The third is variable: from 27.78 ms, at least the shortest refresh, then at once.
        assert_eq!(
            panel.first_reachable(last + Duration::from_millis(30)),
            (seq + 3, last + inserted * 2 + shortest)
        );
        assert_eq!(
            panel.first_reachable(last + Duration::from_millis(35)),
            (seq + 3, last + Duration::from_millis(35) + blank)
        );
    }

    /// The owner's monitor's traces (ADR-0045): the first, of steady rates, recorded while
    /// other builds loaded the machine; the second, of probes, on a quiet one.
    const TRACES: [&str; 2] = [
        include_str!("../tests/fixtures/vrr-trace-pg32ucdm.txt"),
        include_str!("../tests/fixtures/vrr-trace-pg32ucdm-probes.txt"),
    ];

    /// A flip of a trace: when it was submitted, when its event came, and its refresh.
    struct TracedFlip {
        submit: Duration,
        event: Duration,
        seq: u64,
    }

    /// A trace's phases, each with its label and its flips, in order.
    fn phases(trace: &str) -> Vec<(String, Vec<TracedFlip>)> {
        let mut phases: Vec<(String, Vec<TracedFlip>)> = Vec::new();
        for line in trace.lines().filter(|line| !line.starts_with('#')) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            match fields[..] {
                ["phase", label] => phases.push((label.to_owned(), Vec::new())),
                [submit, event, seq] => {
                    let number = |text: &str| text.parse::<u64>().expect("a number");
                    phases
                        .last_mut()
                        .expect("a phase first")
                        .1
                        .push(TracedFlip {
                            submit: Duration::from_nanos(number(submit)),
                            event: Duration::from_nanos(number(event)),
                            seq: number(seq),
                        });
                }
                _ => panic!("a malformed line: {line}"),
            }
        }
        phases
    }

    /// Replays a phase's submissions through the panel, its first flip as the panel's start,
    /// and gives, for each flip after `warm_up`, the error of its predicted start against the
    /// monitor's flip event, and whether its refresh's number matched. After each flip the panel
    /// is set back on the monitor's own history, so that each frame is judged alone and one
    /// miss does not spread to the frames after it.
    fn replay(flips: &[TracedFlip], warm_up: usize) -> Vec<(Duration, bool)> {
        let first = &flips[0];
        let mut panel = Panel::new(48_000, 240_016, first.event);
        panel.flip(first.submit, 0, first.event);
        let mut found = Vec::new();
        for (k, flip) in flips.iter().enumerate().skip(1) {
            let (seq, start) = panel.first_reachable(flip.submit);
            panel.flip(flip.submit, flip.seq, flip.event);
            if k > warm_up {
                found.push((start.abs_diff(flip.event), seq == flip.seq));
            }
        }
        found
    }

    /// The share of a trace's frames whose start the emulation predicts within `tolerance`,
    /// and whose refresh's number it predicts, every phase together.
    fn agreement(trace: &str, tolerance: Duration) -> (f64, f64) {
        let found: Vec<(Duration, bool)> = phases(trace)
            .iter()
            .flat_map(|(_, flips)| replay(flips, 5))
            .collect();
        let share = |count: usize| count as f64 / found.len() as f64;
        (
            share(
                found
                    .iter()
                    .filter(|(error, _)| *error <= tolerance)
                    .count(),
            ),
            share(found.iter().filter(|(_, same)| *same).count()),
        )
    }

    /// ADR-0045's check: the owner's monitor's two traces, replayed, every frame. A frame's
    /// start is the monitor's within 500 µs, the spread of the kernel's commit path the traces
    /// show (0.5 to 0.95 ms from a flip's submission to its showing when nothing holds it):
    /// for 90 % of the steady rates' frames, 80 % of the probes' (late and early frames, gaps,
    /// changes of rate); its refresh's number, for 95 % and 93 %. What is left is known: after
    /// the panel's own refresh, the monitor holds a frame 7 to 12 ms more, and BTR's count may
    /// differ after a perturbation ([debt](../../../docs/debt.md#vrr)). The thresholds are a
    /// little under this replay's figures (91 and 82 %, 96 and 94 %): they guard the emulation
    /// against regressions. The model before the traces fails the times (63 % for the steady
    /// rates), not the refresh numbers, which BTR's count alone decides.
    #[test]
    fn the_emulation_replays_the_owners_monitor() {
        let tolerance = Duration::from_micros(500);
        let (steady_timed, steady_counted) = agreement(TRACES[0], tolerance);
        let (probes_timed, probes_counted) = agreement(TRACES[1], tolerance);
        assert!(
            steady_timed >= 0.90,
            "steady rates: {steady_timed:.3} timed"
        );
        assert!(
            steady_counted >= 0.95,
            "steady rates: {steady_counted:.3} counted"
        );
        assert!(probes_timed >= 0.80, "probes: {probes_timed:.3} timed");
        assert!(
            probes_counted >= 0.93,
            "probes: {probes_counted:.3} counted"
        );
    }
}
