//! Measures the time of one operation of the layout model on a workspace of 100 windows, the
//! figure M1.2 asks for (`docs/PLAN.md`), and of 1,000, to show how it grows. Each operation is
//! followed by `arrange`, which a compositor calls to draw the result, so that the figure is what
//! a key press costs the model.
//!
//! Run it built for release, alone on an idle machine, on one core:
//!
//! ```sh
//! nix develop -c cargo build --release -p yuna-layout --example timing
//! taskset -c 6 target/release/examples/timing
//! ```
//!
//! It prints, per operation and layout, the median and the 99th percentile over many runs, in
//! microseconds. Before timing an operation, it checks that the operation changes what the output
//! shows or what the seat focuses, so that no row times nothing. The figures go to
//! `docs/measurements.md` with the machine and the commit.

use std::hint::black_box;
use std::time::{Duration, Instant};

use yuna_layout::geometry::Point;
use yuna_layout::{
    Direction, Layout, OpenOptions, Options, OutputConfig, OutputId, Placed, SeatId, Tiling,
    WindowId,
};

const SEAT: SeatId = SeatId(1);
const OUTPUT: OutputId = OutputId(1);
const RUNS: usize = 5_000;

/// An operation to time, and its name.
type Operation<'a> = (&'static str, Box<dyn FnMut(&mut Layout) + 'a>);

/// The owner's 4K monitor at scale 1.5 with `windows` windows. In scrolling columns the middle
/// window is focused. In dwindle, where each window halves the one before, only the first dozen
/// have a size; the third is focused, so that it has neighbours to focus and swap with.
fn workspace_of(windows: u64, tiling: Tiling) -> Layout {
    let options = Options {
        default_tiling: tiling,
        ..Options::default()
    };
    let mut layout = Layout::new(options).expect("valid options");
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
    let focused = match tiling {
        Tiling::Scrolling => windows / 2,
        Tiling::Dwindle => 2,
    };
    layout
        .focus_window(WindowId(focused))
        .expect("a known window");
    layout
}

/// What the output shows and what the seat focuses.
fn state(layout: &Layout) -> (Vec<Placed>, Option<WindowId>) {
    (
        layout.arrange(OUTPUT).expect("a known output"),
        layout.focused(SEAT),
    )
}

/// Times `operation` then `arrange`, `RUNS` times, and returns the median and the 99th
/// percentile.
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

/// An operation that goes one way at a call and back at the next, so that every run changes
/// something and the state stays the same over many runs.
fn back_and_forth(mut step: impl FnMut(&mut Layout, bool)) -> impl FnMut(&mut Layout) {
    let mut forth = false;
    move |layout| {
        forth = !forth;
        step(layout, forth);
    }
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
        let other_tiling = match tiling {
            Tiling::Scrolling => Tiling::Dwindle,
            Tiling::Dwindle => Tiling::Scrolling,
        };
        let operations: [Operation; 7] = [
            ("arrange alone", Box::new(|_| {})),
            (
                "focus left, then right",
                Box::new(back_and_forth(|l, forth| {
                    let direction = if forth {
                        Direction::Left
                    } else {
                        Direction::Right
                    };
                    l.focus(SEAT, direction).expect("a known seat");
                })),
            ),
            (
                "open a window, then close it",
                Box::new(back_and_forth(move |l, forth| {
                    let window = WindowId(windows);
                    if forth {
                        l.open_window(SEAT, window, OpenOptions::default())
                            .expect("a new window");
                    } else {
                        l.close_window(window).expect("an open window");
                    }
                })),
            ),
            (
                "move the focused window, then back",
                Box::new(back_and_forth(|l, forth| {
                    let direction = if forth {
                        Direction::Left
                    } else {
                        Direction::Right
                    };
                    l.move_focused(SEAT, direction).expect("a known seat");
                })),
            ),
            (
                "fullscreen on, then off",
                Box::new(back_and_forth(|l, forth| {
                    let window = l.focused(SEAT).expect("a focused window");
                    l.set_fullscreen(window, forth).expect("a known window");
                })),
            ),
            (
                "float, then tile",
                Box::new(back_and_forth(|l, forth| {
                    let window = l.focused(SEAT).expect("a focused window");
                    l.set_floating(window, forth).expect("a known window");
                })),
            ),
            (
                "switch the tiling, then back",
                Box::new(back_and_forth(move |l, forth| {
                    let to = if forth { other_tiling } else { tiling };
                    l.set_tiling(SEAT, to).expect("a known seat");
                })),
            ),
        ];
        for (name, mut operation) in operations {
            if name != "arrange alone" {
                let before = state(&layout);
                operation(&mut layout);
                assert_ne!(
                    before,
                    state(&layout),
                    "{name} changes nothing: it would time nothing"
                );
                operation(&mut layout);
            }
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
