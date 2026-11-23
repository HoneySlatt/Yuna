//! Motion's animated update, for the parity script (`yuna-bench parity`,
//! [ADR-0142](../../../docs/adr/0142-parity-a-pinned-baseline-figures-in-classes-three-verdicts.md)):
//! the updates `motion_timing.rs` times, on one strip of 1,000 windows and of 10,000, the
//! compositor's cap, repeated a fixed count and timed by nothing, so that callgrind counts their
//! instructions. A search that turned quadratic in the windows shows at 10,000.
//!
//! ```sh
//! motion_operations --list               # the cases, one a line: <update>@<windows>
//! motion_operations <case> <repeats>     # sets the case up, runs its update <repeats> times
//! ```
//!
//! Each repeat runs the layout's operation, one way then the other, and [`Motion::update`]
//! animated, one refresh at 240 Hz after the last, so that it interrupts the motion the last one
//! started, as `motion_timing.rs` does. A case's setup is the same whatever the repeats; each run
//! prints `digest <16 hexadecimal digits>`, an FNV-1a hash of when each repeat's motion ends and
//! which window is focused: two builds whose digests differ did not do the same work. Its output and its arguments are the parity script's contract with this program.

use std::process::ExitCode;
use std::time::Duration;

use yuna_layout::{Layout, OpenOptions, Options, OutputConfig};
use yuna_motion::{Change, Motion, Settings};
use yuna_types::{Direction, OutputId, Point, SeatId, WindowId};

const SEAT: SeatId = SeatId(1);
const OUTPUT: OutputId = OutputId(1);
/// One refresh at 240 Hz: the time from one update to the next.
const REFRESH: Duration = Duration::from_nanos(4_166_667);
const WINDOWS: [u64; 2] = [1000, 10_000];
/// The updates, as `motion_timing.rs` names them, in short.
const UPDATES: [&str; 2] = ["focus-two-left", "open-left"];

/// Every case's name: both updates at 1,000 windows, the focus's alone at 10,000, whose strip
/// takes seconds to set up natively and minutes under callgrind, each window opened costing
/// more the more there are.
fn cases() -> Vec<String> {
    let mut cases: Vec<String> = UPDATES
        .iter()
        .map(|update| format!("{update}@{}", WINDOWS[0]))
        .collect();
    cases.push(format!("{}@{}", UPDATES[0], WINDOWS[1]));
    cases
}

/// The owner's 4K monitor at scale 1.5, with `windows` windows on one strip, the middle one
/// focused, and its motion at rest at `now`, as in `motion_timing.rs`.
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

/// FNV-1a, 64 bits, fed with what the updates started.
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
    let (update, windows) = case.split_once('@')?;
    let windows: u64 = windows.parse().ok()?;
    if !UPDATES.contains(&update) || !WINDOWS.contains(&windows) {
        return None;
    }
    let mut now = Duration::ZERO;
    let (mut layout, mut motion) = strip_of(windows, now);
    let (middle, two_left) = (WindowId(windows / 2), WindowId(windows / 2 - 2));
    let mut next_window = windows;
    let mut added = WindowId(next_window);
    let mut digest = Digest(0xcbf2_9ce4_8422_2325);
    for repeat in 0..repeats {
        let forth = repeat % 2 == 0;
        match update {
            "focus-two-left" => {
                let window = if forth { two_left } else { middle };
                layout.focus_window(window).expect("a known window");
            }
            _ => {
                if forth {
                    // A new id each time, as the layout's caller gives them; it opens right of
                    // the focused column, moves left of it, and the focus goes back.
                    added = WindowId(next_window);
                    next_window += 1;
                    layout
                        .open_window(SEAT, added, OpenOptions::default())
                        .expect("a new window");
                    layout
                        .move_focused(SEAT, Direction::Left)
                        .expect("a known seat");
                    layout.focus(SEAT, Direction::Right).expect("a known seat");
                } else {
                    layout.close_window(added).expect("an open window");
                }
            }
        }
        now += REFRESH;
        motion.update(&layout, now, Change::Animated);
        // When the motion ends says what the update started, at no cost that grows with the
        // windows: a frame would draw every one of them.
        let end = motion
            .end(OUTPUT, now)
            .map_or(u64::MAX, |end| end.as_nanos() as u64);
        digest.feed(end);
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
        [case, repeats] => match repeats.parse::<u64>() {
            Ok(repeats) => match run(case, repeats) {
                Some(digest) => {
                    println!("digest {digest:016x}");
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!("motion_operations: no case {case:?}; see --list");
                    ExitCode::from(2)
                }
            },
            Err(_) => {
                eprintln!("motion_operations: <repeats> is a whole number");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("usage: motion_operations --list | motion_operations <case> <repeats>");
            ExitCode::from(2)
        }
    }
}
