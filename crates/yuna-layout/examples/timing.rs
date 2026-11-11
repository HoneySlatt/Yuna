//! Measures the time of one operation of the layout model on a workspace of 100 windows, the
//! figure M1.2 asks for (`docs/PLAN.md`), and of 1,000, to show how it grows. Each operation is followed by `arrange`, which a
//! compositor calls to draw the result, so that the figure is what a key press costs the model.
//!
//! Run it built for release, alone on an idle machine:
//!
//! ```sh
//! nix develop -c cargo run --release -p yuna-layout --example timing
//! ```
//!
//! It prints, per operation and layout, the median and the 99th percentile over many runs, in
//! microseconds. The figures go to `docs/measurements.md` with the machine and the commit.

use std::hint::black_box;
use std::time::{Duration, Instant};

use yuna_layout::geometry::Point;
use yuna_layout::{
    Direction, Layout, OpenOptions, Options, OutputConfig, OutputId, SeatId, Tiling, WindowId,
};

const SEAT: SeatId = SeatId(1);
const OUTPUT: OutputId = OutputId(1);
const RUNS: usize = 5_000;

/// An operation to time, and its name.
type Operation<'a> = (&'static str, Box<dyn FnMut(&mut Layout) + 'a>);

/// The owner's 4K monitor at scale 1.5 with `windows` windows, the middle one focused.
fn workspace_of(windows: u64, tiling: Tiling) -> Layout {
    let mut layout = Layout::new(Options {
        default_tiling: tiling,
        ..Options::default()
    });
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
    layout
}

/// Times `operation` then `arrange`, `RUNS` times, and returns the median and the 99th
/// percentile. `operation` leaves the layout as it found it, or close, so that every run measures
/// the same state.
fn time(layout: &mut Layout, mut operation: impl FnMut(&mut Layout)) -> (Duration, Duration) {
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let start = Instant::now();
        operation(layout);
        black_box(layout.arrange(OUTPUT).expect("a known output"));
        samples.push(start.elapsed());
    }
    samples.sort_unstable();
    (samples[RUNS / 2], samples[RUNS * 99 / 100])
}

fn main() {
    println!("operation + arrange                 | windows | layout    | median µs | p99 µs");
    for (windows, tiling) in [
        (100, Tiling::Scrolling),
        (100, Tiling::Dwindle),
        (1000, Tiling::Scrolling),
        (1000, Tiling::Dwindle),
    ] {
        let mut layout = workspace_of(windows, tiling);
        let mut toggle = false;
        let operations: [Operation; 7] = [
            ("arrange alone", Box::new(|_| {})),
            (
                "focus left, then right",
                Box::new(|l| {
                    toggle = !toggle;
                    let direction = if toggle {
                        Direction::Left
                    } else {
                        Direction::Right
                    };
                    l.focus(SEAT, direction).expect("a known seat");
                }),
            ),
            (
                "open a window, then close it",
                Box::new(|l| {
                    let window = WindowId(windows);
                    if l.window(window).is_some() {
                        l.close_window(window).expect("an open window");
                    } else {
                        l.open_window(SEAT, window, OpenOptions::default())
                            .expect("a new window");
                    }
                }),
            ),
            (
                "move the focused window",
                Box::new(|l| {
                    l.move_focused(SEAT, Direction::Right)
                        .expect("a known seat")
                }),
            ),
            (
                "fullscreen on, then off",
                Box::new(|l| {
                    let window = l.focused(SEAT).expect("a focused window");
                    let on = !l.window(window).expect("a known window").fullscreen;
                    l.set_fullscreen(window, on).expect("a known window");
                }),
            ),
            (
                "float, then tile",
                Box::new(|l| {
                    let window = l.focused(SEAT).expect("a focused window");
                    let on = !l.window(window).expect("a known window").floating;
                    l.set_floating(window, on).expect("a known window");
                }),
            ),
            (
                "switch the tiling and back",
                Box::new(move |l| {
                    let other = match tiling {
                        Tiling::Scrolling => Tiling::Dwindle,
                        Tiling::Dwindle => Tiling::Scrolling,
                    };
                    l.set_tiling(SEAT, other).expect("a known seat");
                    l.set_tiling(SEAT, tiling).expect("a known seat");
                }),
            ),
        ];
        for (name, operation) in operations {
            let (median, p99) = time(&mut layout, operation);
            println!(
                "{name:<36} | {windows:>7} | {:<9} | {:>9.2} | {:>6.2}",
                format!("{tiling:?}"),
                median.as_secs_f64() * 1e6,
                p99.as_secs_f64() * 1e6
            );
        }
    }
}
