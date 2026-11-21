//! The parity script's canaries
//! ([ADR-0142](../../../docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)'s
//! controls): costs injected on purpose, each about twice the threshold of the figure that must
//! flag it, so that the script proves it sees what it watches (`yuna-bench parity run
//! --canary <kind>`).
//!
//! They exist only with the `parity-canary` feature, which no shipped build turns on: without it,
//! [`ParityCanary::from_environment`] gives none, every site is a branch on a constant the
//! compiler removes, and Yuna reads no variable for them. With it, `YUNA_PARITY_CANARY`, read once
//! at start, names the one cost turned on:
//!
//! - `spin`: instructions in Yuna's own code each frame drawn, sized for Yuna's own figure;
//! - `spin-total`: the same, sized for the total's figure, llvmpipe's and Mesa's instructions
//!   included;
//! - `allocate`: one allocation each frame drawn, freed at once;
//! - `leak`: 64 bytes kept every 16 frames drawn, 4 bytes a frame;
//! - `memory`: 1 MiB kept and written at start;
//! - `wake`: a timer each second, at rest too;
//! - `frame-wake`: one more wake of the event loop each frame drawn, after a sleep: 20 times the
//!   threshold of a frame's wakes, 5 times that of its context switches, the least a frame's own
//!   wake can be;
//! - `round`: instructions in each round of the event loop;
//! - `spin-draw`: the timed tier's spin, a wait of [`DRAW_SPIN`] on the processor inside each
//!   frame's drawing, which its time (`took_us`) counts;
//! - `start`: a sleep of [`START_SLEEP`] as Yuna starts, before it reads any call.

use std::hint::black_box;
use std::time::{Duration, Instant};

/// The turns of [`spin`] a frame for `spin`: about 2,600 of Yuna's instructions, 1.9 times the
/// threshold of a frame's own instructions in `motion` (1,400), the widest of the scenarios that
/// draw a frame each period; 3.6 to 3.8 times `partial`'s
/// (`crates/yuna-bench/parity-thresholds.toml`). `input`'s is wider, 0.6 % of 542,000, but
/// `input` draws no frame, so no spin.
const SPIN_TURNS: u64 = 600;
/// The turns of [`spin`] a frame for `spin-total`: about 160,000 instructions, twice the
/// threshold of a frame's total in `full`, the widest of the scenarios that draw (0.1 % of 80.6
/// million at M2.1); 2.2 times `motion`'s, 4.8 times `partial`'s, 8.4 times `many`'s. `idle` and
/// `input` draw no frame.
const TOTAL_SPIN_TURNS: u64 = 37_000;
/// The turns of [`spin`] a round for `round`: about 1,300 instructions, twice the threshold of
/// `idle`'s, whose rounds draw nothing.
const ROUND_TURNS: u64 = 300;
/// How long `frame-wake` sleeps each frame. The wake it asks of the loop comes at once, its timer
/// due already, so the loop never sleeps before it; the sleep is the context switch a wake from
/// sleep costs. In the parity script's periods, an advance draws the frame: the sleep comes
/// before the advance's reply, the wake before Yuna next sleeps, which the script waits for
/// before it sends anything more. So every frame counts one of each, whatever the machine's speed.
const FRAME_WAKE_SLEEP: Duration = Duration::from_micros(50);
/// What `leak` keeps, every [`LEAK_EVERY`] frames: 4 bytes a frame, twice the threshold of the
/// heap's growth.
const LEAKED: usize = 64;
/// How many frames `leak` keeps [`LEAKED`] bytes in.
const LEAK_EVERY: u64 = 16;
/// What `memory` keeps.
const BALLAST: usize = 1 << 20;
/// How long `spin-draw` keeps the processor busy in each frame's drawing: 28 µs, on llvmpipe 2.6 times
/// the threshold of `partial`'s drawing time (6.5 % of 167 µs) and 2.4 times `scaled`'s (5 % of
/// 236 µs), the timed tier's (`crates/yuna-bench/parity-thresholds.toml`).
const DRAW_SPIN: Duration = Duration::from_micros(28);
/// How long `start` sleeps as Yuna starts: 24 ms, 2.1 to 2.9 times the thresholds of the starts'
/// figures on llvmpipe (9 to 11.5 % of 70 to 105 ms).
const START_SLEEP: Duration = Duration::from_millis(24);

/// Which cost is injected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Spin,
    SpinTotal,
    Allocate,
    Leak,
    Memory,
    Wake,
    FrameWake,
    Round,
    SpinDraw,
    Start,
}

impl Kind {
    fn named(name: &str) -> Option<Kind> {
        Some(match name {
            "spin" => Kind::Spin,
            "spin-total" => Kind::SpinTotal,
            "allocate" => Kind::Allocate,
            "leak" => Kind::Leak,
            "memory" => Kind::Memory,
            "wake" => Kind::Wake,
            "frame-wake" => Kind::FrameWake,
            "round" => Kind::Round,
            "spin-draw" => Kind::SpinDraw,
            "start" => Kind::Start,
            _ => return None,
        })
    }
}

/// The canary of this Yuna, and what it keeps: none in a shipped build.
#[derive(Debug, Default)]
pub(crate) struct ParityCanary {
    kind: Option<Kind>,
    /// What `leak` kept: one block of its own every [`LEAK_EVERY`] frames, as a leak keeps them.
    leaked: Vec<Vec<u8>>,
    /// The frames drawn since start, for `leak`.
    frames: u64,
    /// What `memory` keeps.
    ballast: Vec<u8>,
}

impl ParityCanary {
    /// The canary `YUNA_PARITY_CANARY` names, with the `parity-canary` feature; none otherwise.
    pub(crate) fn from_environment() -> ParityCanary {
        let kind = if cfg!(feature = "parity-canary") {
            std::env::var("YUNA_PARITY_CANARY")
                .ok()
                .and_then(|name| Kind::named(&name))
        } else {
            None
        };
        let mut canary = ParityCanary {
            kind,
            ..ParityCanary::default()
        };
        if canary.kind == Some(Kind::Memory) {
            // Written whole, so that every page is the process's own.
            canary.ballast = black_box(vec![1; BALLAST]);
        }
        if let Some(kind) = canary.kind {
            tracing::warn!(
                ?kind,
                "a parity canary is on: this build measures nothing but it"
            );
        }
        if canary.kind == Some(Kind::Start) {
            std::thread::sleep(START_SLEEP);
        }
        canary
    }

    /// Whether it wants a timer each second.
    pub(crate) fn wakes(&self) -> bool {
        self.kind == Some(Kind::Wake)
    }

    /// How long it keeps the processor busy at the end of each frame's drawing, if it does: run
    /// by the drawing's hook, which the frame's time counts.
    pub(crate) fn drawing_spin(&self) -> Option<Duration> {
        (self.kind == Some(Kind::SpinDraw)).then_some(DRAW_SPIN)
    }

    /// After a round of the event loop, or an advance of the manual clock, that drew `frames`
    /// frames; `round` when it was a round. Gives the wakes it asks of the loop: for
    /// `frame-wake`, one a frame, each a timer already due, which the caller inserts.
    #[must_use]
    pub(crate) fn after(&mut self, frames: u64, round: bool) -> u64 {
        let mut wakes = 0;
        match self.kind {
            Some(Kind::Round) if round => spin(ROUND_TURNS),
            Some(Kind::Spin) => (0..frames).for_each(|_| spin(SPIN_TURNS)),
            Some(Kind::SpinTotal) => (0..frames).for_each(|_| spin(TOTAL_SPIN_TURNS)),
            Some(Kind::FrameWake) => {
                for _ in 0..frames {
                    std::thread::sleep(FRAME_WAKE_SLEEP);
                }
                wakes = frames;
            }
            Some(Kind::Allocate) => {
                for _ in 0..frames {
                    drop(black_box(Box::new([0_u8; LEAKED])));
                }
            }
            Some(Kind::Leak) => {
                for _ in 0..frames {
                    self.frames += 1;
                    if self.frames.is_multiple_of(LEAK_EVERY) {
                        self.leaked.push(black_box(vec![1; LEAKED]));
                    }
                }
            }
            _ => {}
        }
        black_box(&self.ballast);
        wakes
    }
}

/// Keeps the processor busy for `time`, as a drawing that takes that much longer does, never
/// sleeping: a sleep would let the processor rest, and the frame's CPU time would not see it.
pub(crate) fn spin_for(time: Duration) {
    let started = Instant::now();
    while started.elapsed() < time {
        std::hint::spin_loop();
    }
}

/// Work the compiler cannot remove: about four instructions a turn.
fn spin(turns: u64) {
    let mut sum = 0_u64;
    for turn in 0..turns {
        sum = black_box(sum.wrapping_add(turn));
    }
    black_box(sum);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_without_the_feature_has_no_canary() {
        let canary = ParityCanary::from_environment();
        if !cfg!(feature = "parity-canary") {
            assert_eq!(canary.kind, None);
            assert!(!canary.wakes());
        }
        assert_eq!(Kind::named("leak"), Some(Kind::Leak));
        assert_eq!(Kind::named("spin-draw"), Some(Kind::SpinDraw));
        assert_eq!(Kind::named("start"), Some(Kind::Start));
        assert_eq!(Kind::named("slow"), None);
        assert_eq!(
            canary.drawing_spin().is_some(),
            canary.kind == Some(Kind::SpinDraw)
        );
    }

    #[test]
    fn draw_spins_for_its_time_in_each_drawing_and_the_others_do_not() {
        let draw = ParityCanary {
            kind: Some(Kind::SpinDraw),
            ..ParityCanary::default()
        };
        assert_eq!(draw.drawing_spin(), Some(DRAW_SPIN));
        let started = Instant::now();
        spin_for(DRAW_SPIN);
        assert!(started.elapsed() >= DRAW_SPIN);
        let spin = ParityCanary {
            kind: Some(Kind::Spin),
            ..ParityCanary::default()
        };
        assert_eq!(spin.drawing_spin(), None);
    }

    #[test]
    fn leak_keeps_what_each_frame_gave_and_allocate_keeps_nothing() {
        let mut leak = ParityCanary {
            kind: Some(Kind::Leak),
            ..ParityCanary::default()
        };
        assert_eq!(leak.after(3, false), 0);
        assert_eq!(leak.after(0, true), 0);
        assert_eq!(leak.leaked.len(), 0);
        assert_eq!(leak.after(2 * LEAK_EVERY - 3, false), 0);
        assert_eq!(leak.leaked.len(), 2, "a block every {LEAK_EVERY} frames");
        let mut allocate = ParityCanary {
            kind: Some(Kind::Allocate),
            ..ParityCanary::default()
        };
        assert_eq!(allocate.after(3, false), 0);
        assert!(allocate.leaked.is_empty());
    }

    #[test]
    fn frame_wake_asks_one_wake_a_frame_and_none_at_rest() {
        let mut canary = ParityCanary {
            kind: Some(Kind::FrameWake),
            ..ParityCanary::default()
        };
        assert_eq!(canary.after(1, false), 1);
        assert_eq!(canary.after(2, true), 2);
        // A round or an advance that drew nothing asks for nothing: at rest, it never wakes.
        assert_eq!(canary.after(0, true), 0);
        assert_eq!(canary.after(0, false), 0);
        assert!(!canary.wakes(), "no timer of its own each second");
        for other in ["spin", "spin-total", "wake", "round"] {
            let mut canary = ParityCanary {
                kind: Kind::named(other),
                ..ParityCanary::default()
            };
            assert_eq!(canary.after(3, true), 0, "{other}");
        }
        assert_eq!(Kind::named("frame-wake"), Some(Kind::FrameWake));
        assert_eq!(Kind::named("spin-total"), Some(Kind::SpinTotal));
    }
}
