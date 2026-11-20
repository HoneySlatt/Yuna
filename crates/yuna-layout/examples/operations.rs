//! The layout model's operations, for the parity script (`yuna-bench parity`,
//! [ADR-0142](../../../docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)):
//! the operations `timing.rs` times, each followed by `arrange`, repeated a fixed count and timed
//! by nothing, so that callgrind counts their instructions, which the machine's load does not
//! move.
//!
//! ```sh
//! operations --list               # the cases, one a line: <operation>@<windows>-<layout>
//! operations <case> <repeats>     # sets the case up, runs its operation <repeats> times
//! ```
//!
//! A case's setup is the same whatever the repeats, so that the difference between two runs of
//! `n` and `2n` repeats is `n` operations. Each run prints `digest <16 hexadecimal digits>`, an
//! FNV-1a hash of what each repeat arranged and focused: two builds whose digests differ did not
//! do the same work, and their counts are not compared. Its output and its arguments are the
//! parity script's contract with this program.

use std::process::ExitCode;

use yuna_layout::geometry::Point;
use yuna_layout::{Layout, OpenOptions, Options, OutputConfig};
use yuna_types::{Direction, OutputId, SeatId, Tiling, WindowId};

const SEAT: SeatId = SeatId(1);
const OUTPUT: OutputId = OutputId(1);
/// The workspaces' sizes, as `timing.rs`'s.
const WINDOWS: [u64; 2] = [100, 1000];
/// The operations, as `timing.rs` names them, in short.
const OPERATIONS: [&str; 7] = [
    "arrange",
    "focus-left-right",
    "open-close",
    "move-left-right",
    "fullscreen-on-off",
    "float-tile",
    "switch-tiling",
];

fn tiling_name(tiling: Tiling) -> &'static str {
    match tiling {
        Tiling::Scrolling => "scrolling",
        Tiling::Dwindle => "dwindle",
    }
}

/// Every case's name.
fn cases() -> Vec<String> {
    let mut cases = Vec::new();
    for windows in WINDOWS {
        for tiling in [Tiling::Scrolling, Tiling::Dwindle] {
            for operation in OPERATIONS {
                cases.push(format!("{operation}@{windows}-{}", tiling_name(tiling)));
            }
        }
    }
    cases
}

/// The owner's 4K monitor at scale 1.5 with `windows` windows, as in `timing.rs`: in scrolling
/// columns the middle window focused, in dwindle the third.
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

/// One way of `operation`, forth or back, so that two repeats in a row leave the layout as it
/// was.
fn operate(layout: &mut Layout, operation: &str, windows: u64, tiling: Tiling, forth: bool) {
    let sideways = if forth {
        Direction::Left
    } else {
        Direction::Right
    };
    match operation {
        "arrange" => {}
        "focus-left-right" => layout.focus(SEAT, sideways).expect("a known seat"),
        "open-close" => {
            let window = WindowId(windows);
            if forth {
                layout
                    .open_window(SEAT, window, OpenOptions::default())
                    .expect("a new window");
            } else {
                layout.close_window(window).expect("an open window");
            }
        }
        "move-left-right" => layout.move_focused(SEAT, sideways).expect("a known seat"),
        "fullscreen-on-off" => {
            let window = layout.focused(SEAT).expect("a focused window");
            layout
                .set_fullscreen(window, forth)
                .expect("a known window");
        }
        "float-tile" => {
            let window = layout.focused(SEAT).expect("a focused window");
            layout.set_floating(window, forth).expect("a known window");
        }
        "switch-tiling" => {
            let other = match tiling {
                Tiling::Scrolling => Tiling::Dwindle,
                Tiling::Dwindle => Tiling::Scrolling,
            };
            let to = if forth { other } else { tiling };
            layout.set_tiling(SEAT, to).expect("a known seat");
        }
        other => unreachable!("no operation {other}"),
    }
}

/// FNV-1a, 64 bits, fed with what the repeats arranged.
struct Digest(u64);

impl Digest {
    fn feed(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// Runs `case` `repeats` times; its digest, or none for no such case.
fn run(case: &str, repeats: u64) -> Option<u64> {
    let (operation, rest) = case.split_once('@')?;
    let (windows, tiling) = rest.split_once('-')?;
    let windows: u64 = windows.parse().ok()?;
    let tiling = match tiling {
        "scrolling" => Tiling::Scrolling,
        "dwindle" => Tiling::Dwindle,
        _ => return None,
    };
    if !OPERATIONS.contains(&operation) || !WINDOWS.contains(&windows) {
        return None;
    }
    let mut layout = workspace_of(windows, tiling);
    let mut digest = Digest(0xcbf2_9ce4_8422_2325);
    for repeat in 0..repeats {
        operate(&mut layout, operation, windows, tiling, repeat % 2 == 0);
        let placed = layout.arrange(OUTPUT).expect("a known output");
        for window in &placed {
            digest.feed(window.window.0);
            for side in [window.rect.x, window.rect.y, window.rect.w, window.rect.h] {
                digest.feed(side.to_bits());
            }
        }
        digest.feed(layout.focused(SEAT).map_or(u64::MAX, |window| window.0));
    }
    Some(digest.0)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match &arguments[..] {
        [list] if list == "--list" => {
            for case in cases() {
                println!("{case}");
            }
            ExitCode::SUCCESS
        }
        [case, repeats] => match (repeats.parse::<u64>().ok(), case) {
            (Some(repeats), case) => match run(case, repeats) {
                Some(digest) => {
                    println!("digest {digest:016x}");
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!("operations: no case {case:?}; see --list");
                    ExitCode::from(2)
                }
            },
            (None, _) => {
                eprintln!("operations: <repeats> is a whole number");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("usage: operations --list | operations <case> <repeats>");
            ExitCode::from(2)
        }
    }
}
