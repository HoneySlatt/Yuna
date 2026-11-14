//! Measures what motion costs at an operation and at a frame, on one strip of 100 windows and of
//! 1,000: [`Motion::update`] after an operation, animated, which arranges the output itself
//! ([`Layout::arrange`]); and [`Motion::frame`] while the motion runs. The layout's operation is
//! not timed: `yuna-layout`'s `timing` example measures it. Each update comes one refresh at
//! 240 Hz after the last, so that it interrupts the motion that one started: the heaviest case,
//! where every window that moved has a spring to read.
//!
//! Run it built for release, alone on an idle machine, on one core:
//!
//! ```sh
//! nix develop -c cargo build --release -p yuna-motion --example motion_timing
//! taskset -c 6 target/release/examples/motion_timing
//! ```
//!
//! It prints, per row, the median and the 99th percentile over many runs, in microseconds. Before
//! timing an update, it checks that the operation starts a motion from rest, both ways, so that
//! no row times an output at rest. The figures go to `docs/measurements.md` with the machine and
//! the commit.
//!
//! [`Layout::arrange`]: yuna_layout::Layout::arrange

use std::hint::black_box;
use std::time::{Duration, Instant};

use yuna_layout::geometry::Point;
use yuna_layout::{
    Direction, Layout, OpenOptions, Options, OutputConfig, OutputId, SeatId, WindowId,
};
use yuna_motion::{Change, Motion, Settings};

const SEAT: SeatId = SeatId(1);
const OUTPUT: OutputId = OutputId(1);
const RUNS: usize = 5_000;
/// One refresh at 240 Hz: the time from one update to the next.
const REFRESH: Duration = Duration::from_nanos(4_166_667);

/// An operation on the layout that goes forth when given `true` and back when given `false`, so
/// that every run changes something and the layout stays the same over many runs.
type Operation<'a> = &'a mut dyn FnMut(&mut Layout, bool);

/// The owner's 4K monitor at scale 1.5, with `windows` windows on one strip, the middle one
/// focused; and its motion at rest at `now`, animations on at their defaults.
fn strip_of(windows: u64, now: Duration) -> (Layout, Motion) {
    let mut layout = Layout::new(Options::default()).expect("valid options");
    layout.add_seat(SEAT).expect("a new seat");
    let config = OutputConfig {
        position: Point { x: 0.0, y: 0.0 },
        physical_width: 3840,
        physical_height: 2160,
        scale: 1.5,
    };
    layout
        .add_output(SEAT, OUTPUT, config)
        .expect("a new output");
    for n in 0..windows {
        layout
            .open_window(SEAT, WindowId(n), OpenOptions::default())
            .expect("a new window");
    }
    layout
        .focus_window(WindowId(windows / 2))
        .expect("a known window");
    let mut motion = Motion::new(Settings::default());
    motion.update(&layout, now, Change::Instant);
    (layout, motion)
}

/// The median and the 99th percentile of `samples`.
fn percentiles(mut samples: Vec<Duration>) -> (Duration, Duration) {
    samples.sort_unstable();
    (
        samples[samples.len() / 2],
        samples[samples.len() * 99 / 100],
    )
}

/// Checks that `operation` starts a motion from rest, forth and back: a row whose operation moves
/// nothing would time an output at rest.
fn assert_starts_a_motion(
    name: &str,
    layout: &mut Layout,
    motion: &mut Motion,
    now: &mut Duration,
    operation: Operation,
) {
    for forth in [true, false] {
        // Every motion has ended 10 s later.
        *now += Duration::from_secs(10);
        motion.update(layout, *now, Change::Instant);
        operation(layout, forth);
        motion.update(layout, *now, Change::Animated);
        assert!(
            motion.end(OUTPUT, *now).is_some(),
            "{name} starts no motion: it would time an output at rest"
        );
    }
}

/// Times [`Motion::update`] after `operation`, `RUNS` times, forth then back, each a refresh after
/// the last; returns the median and the 99th percentile.
fn time_update(
    layout: &mut Layout,
    motion: &mut Motion,
    now: &mut Duration,
    operation: Operation,
) -> (Duration, Duration) {
    let mut samples = Vec::with_capacity(RUNS);
    for run in 0..RUNS {
        operation(layout, run % 2 == 0);
        *now += REFRESH;
        let start = Instant::now();
        motion.update(layout, *now, Change::Animated);
        samples.push(start.elapsed());
    }
    percentiles(samples)
}

/// Times [`Motion::frame`] at `RUNS` times spread over the motion `operation` starts from rest,
/// going forth; returns the median and the 99th percentile.
fn time_frame(
    layout: &mut Layout,
    motion: &mut Motion,
    now: &mut Duration,
    operation: Operation,
) -> (Duration, Duration) {
    *now += Duration::from_secs(10);
    motion.update(layout, *now, Change::Instant);
    operation(layout, true);
    motion.update(layout, *now, Change::Animated);
    let end = motion
        .end(OUTPUT, *now)
        .expect("the operation starts a motion");
    let step = (end - *now) / u32::try_from(RUNS).expect("RUNS fits in 32 bits");
    let mut samples = Vec::with_capacity(RUNS);
    let mut at = *now;
    for _ in 0..RUNS {
        let start = Instant::now();
        black_box(motion.frame(OUTPUT, at));
        samples.push(start.elapsed());
        at += step;
    }
    percentiles(samples)
}

/// Prints one row of figures.
fn print_row(name: &str, windows: u64, (median, p99): (Duration, Duration)) {
    println!(
        "{name:<44} | {windows:>7} | {:>9.2} | {:>6.2}",
        median.as_secs_f64() * 1e6,
        p99.as_secs_f64() * 1e6
    );
}

fn main() {
    println!("{:<44} | windows | median µs | p99 µs", "what is timed");
    for windows in [100, 1000] {
        let mut now = Duration::ZERO;
        let (mut layout, mut motion) = strip_of(windows, now);
        let (middle, two_left) = (WindowId(windows / 2), WindowId(windows / 2 - 2));
        // Each way, the column focused is out of view: the camera moves.
        let mut focus_two_left = |l: &mut Layout, forth: bool| {
            let window = if forth { two_left } else { middle };
            l.focus_window(window).expect("a known window");
        };
        // Each way, the strip moves under the view and the columns on the left slide. Each new
        // window has an id never used before, as the layout's caller gives them.
        let mut next_window = windows;
        let mut added = WindowId(next_window);
        let mut open_left = |l: &mut Layout, forth: bool| {
            if forth {
                added = WindowId(next_window);
                next_window += 1;
                l.open_window(SEAT, added, OpenOptions::default())
                    .expect("a new window");
                // It opens right of the focused column, focused: it moves left of that column,
                // and the focus goes back to it.
                l.move_focused(SEAT, Direction::Left).expect("a known seat");
                l.focus(SEAT, Direction::Right).expect("a known seat");
            } else {
                l.close_window(added).expect("an open window");
            }
        };
        let updates: [(&str, Operation); 2] = [
            (
                "update: focus 2 columns left, then back",
                &mut focus_two_left,
            ),
            ("update: open a column left, then close it", &mut open_left),
        ];
        for (name, operation) in updates {
            assert_starts_a_motion(name, &mut layout, &mut motion, &mut now, operation);
            let figures = time_update(&mut layout, &mut motion, &mut now, operation);
            print_row(name, windows, figures);
        }
        let figures = time_frame(&mut layout, &mut motion, &mut now, &mut open_left);
        print_row("frame: while a column opens left", windows, figures);
    }
}
