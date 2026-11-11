//! Property tests: random sequences of operations, over two seats and up to three outputs of
//! different sizes and scales, with ids drawn from small ranges so that stale and duplicate ids
//! occur. After every operation:
//!
//! - every invariant of [`Layout::check_invariants`] holds;
//! - what every other seat's outputs show, and what it focuses, is unchanged: one seat never
//!   reaches another (ADR-0015);
//! - an operation that returns an error leaves the model exactly as it was.
//!
//! A failure shrinks to the shortest sequence that still fails; proptest keeps its seed in
//! `properties.proptest-regressions`, which is committed so that the case runs again every time.

use proptest::prelude::*;
use yuna_layout::geometry::{Point, Rect};
use yuna_layout::{
    ColumnWidth, Direction, Layout, OpenOptions, Options, OutputConfig, OutputId, SeatId, Tiling,
    WindowId,
};

/// The seats that exist from the start; seat 3 never does, to exercise unknown ids.
const SEATS: [u64; 3] = [1, 2, 3];
const SCALES: [f64; 4] = [1.0, 1.25, 1.5, 2.0];
const SIZES: [(u32, u32); 4] = [(3840, 2160), (1920, 1080), (1280, 800), (800, 1280)];
const NAMES: [&str; 3] = ["1", "2", "web"];

#[derive(Debug, Clone)]
enum Op {
    AddOutput {
        seat: u64,
        output: u64,
        scale: usize,
        size: usize,
        x: i16,
    },
    RemoveOutput(u64),
    FocusOutput {
        seat: u64,
        output: u64,
    },
    AddWorkspace {
        seat: u64,
        name: usize,
        output: Option<u64>,
    },
    RemoveWorkspace {
        seat: u64,
        name: usize,
    },
    FocusWorkspace {
        seat: u64,
        name: usize,
    },
    FocusWorkspaceRelative {
        seat: u64,
        direction: Direction,
    },
    ToggleSpecial {
        seat: u64,
        name: usize,
    },
    SetTiling {
        seat: u64,
        dwindle: bool,
    },
    Focus {
        seat: u64,
        direction: Direction,
    },
    MoveFocused {
        seat: u64,
        direction: Direction,
    },
    ConsumeOrExpel {
        seat: u64,
        direction: Direction,
    },
    SwitchFocusFloating {
        seat: u64,
    },
    MoveToWorkspace {
        seat: u64,
        name: usize,
    },
    MoveToWorkspaceRelative {
        seat: u64,
        direction: Direction,
    },
    MoveToSpecial {
        seat: u64,
        name: usize,
    },
    Open {
        seat: u64,
        window: u64,
        floating: bool,
        fullscreen: bool,
        width: Option<ColumnWidth>,
        workspace: Option<usize>,
    },
    Close(u64),
    FocusWindow(u64),
    SetFloating(u64, bool),
    SetFloatingRect {
        window: u64,
        x: i16,
        y: i16,
        w: u16,
        h: u16,
    },
    SetFullscreen(u64, bool),
    SetFakeFullscreen(u64, bool),
    SetFullWidth(u64, bool),
    CyclePresetWidth(u64, bool),
    ChangeWidth(u64, i8),
    SetPinned(u64, bool),
}

fn seat() -> impl Strategy<Value = u64> {
    prop_oneof![8 => 1..=2u64, 1 => Just(3u64)]
}

fn window() -> impl Strategy<Value = u64> {
    1..=8u64
}

fn output() -> impl Strategy<Value = u64> {
    1..=3u64
}

fn direction() -> impl Strategy<Value = Direction> {
    prop_oneof![
        Just(Direction::Left),
        Just(Direction::Right),
        Just(Direction::Up),
        Just(Direction::Down),
    ]
}

fn width() -> impl Strategy<Value = ColumnWidth> {
    prop_oneof![
        (0.0..1.2f64).prop_map(ColumnWidth::Proportion),
        (-10.0..4000.0f64).prop_map(ColumnWidth::Fixed),
        Just(ColumnWidth::Proportion(f64::NAN)),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let name = 0..NAMES.len();
    prop_oneof![
        2 => (seat(), output(), 0..SCALES.len(), 0..SIZES.len(), -4000..4000i16)
            .prop_map(|(seat, output, scale, size, x)| Op::AddOutput { seat, output, scale, size, x }),
        1 => output().prop_map(Op::RemoveOutput),
        1 => (seat(), output()).prop_map(|(seat, output)| Op::FocusOutput { seat, output }),
        1 => (seat(), name.clone(), proptest::option::of(output()))
            .prop_map(|(seat, name, output)| Op::AddWorkspace { seat, name, output }),
        1 => (seat(), name.clone()).prop_map(|(seat, name)| Op::RemoveWorkspace { seat, name }),
        2 => (seat(), name.clone()).prop_map(|(seat, name)| Op::FocusWorkspace { seat, name }),
        2 => (seat(), direction())
            .prop_map(|(seat, direction)| Op::FocusWorkspaceRelative { seat, direction }),
        2 => (seat(), name.clone()).prop_map(|(seat, name)| Op::ToggleSpecial { seat, name }),
        1 => (seat(), any::<bool>()).prop_map(|(seat, dwindle)| Op::SetTiling { seat, dwindle }),
        4 => (seat(), direction()).prop_map(|(seat, direction)| Op::Focus { seat, direction }),
        3 => (seat(), direction()).prop_map(|(seat, direction)| Op::MoveFocused { seat, direction }),
        2 => (seat(), direction())
            .prop_map(|(seat, direction)| Op::ConsumeOrExpel { seat, direction }),
        1 => seat().prop_map(|seat| Op::SwitchFocusFloating { seat }),
        2 => (seat(), name.clone()).prop_map(|(seat, name)| Op::MoveToWorkspace { seat, name }),
        1 => (seat(), direction())
            .prop_map(|(seat, direction)| Op::MoveToWorkspaceRelative { seat, direction }),
        1 => (seat(), name.clone()).prop_map(|(seat, name)| Op::MoveToSpecial { seat, name }),
        8 => (
            seat(),
            window(),
            prop::bool::weighted(0.25),
            prop::bool::weighted(0.1),
            proptest::option::weighted(0.2, width()),
            proptest::option::weighted(0.2, name.clone()),
        )
            .prop_map(|(seat, window, floating, fullscreen, width, workspace)| Op::Open {
                seat,
                window,
                floating,
                fullscreen,
                width,
                workspace,
            }),
        4 => window().prop_map(Op::Close),
        2 => window().prop_map(Op::FocusWindow),
        2 => (window(), any::<bool>()).prop_map(|(w, on)| Op::SetFloating(w, on)),
        1 => (window(), any::<i16>(), any::<i16>(), any::<u16>(), any::<u16>())
            .prop_map(|(window, x, y, w, h)| Op::SetFloatingRect { window, x, y, w, h }),
        3 => (window(), any::<bool>()).prop_map(|(w, on)| Op::SetFullscreen(w, on)),
        1 => (window(), any::<bool>()).prop_map(|(w, on)| Op::SetFakeFullscreen(w, on)),
        2 => (window(), any::<bool>()).prop_map(|(w, on)| Op::SetFullWidth(w, on)),
        2 => (window(), any::<bool>()).prop_map(|(w, on)| Op::CyclePresetWidth(w, on)),
        1 => (window(), any::<i8>()).prop_map(|(w, d)| Op::ChangeWidth(w, d)),
        2 => (window(), any::<bool>()).prop_map(|(w, on)| Op::SetPinned(w, on)),
    ]
}

fn options() -> impl Strategy<Value = Options> {
    (
        any::<bool>(),
        prop_oneof![Just(0.0), Just(16.0), Just(7.3)],
        any::<bool>(),
    )
        .prop_map(|(dynamic, gap, dwindle)| Options {
            gap,
            dynamic_workspaces: dynamic,
            default_tiling: if dwindle {
                Tiling::Dwindle
            } else {
                Tiling::Scrolling
            },
            ..Options::default()
        })
}

/// The seat an operation acts for, if it names one that exists: the only seat whose state it may
/// change.
fn acting_seat(layout: &Layout, op: &Op) -> Option<SeatId> {
    let window_seat = |w: u64| layout.window(WindowId(w)).map(|info| info.seat);
    let output_seat = |o: u64| {
        SEATS.iter().map(|&s| SeatId(s)).find(|&s| {
            layout
                .outputs(s)
                .is_ok_and(|outputs| outputs.contains(&OutputId(o)))
        })
    };
    match *op {
        Op::AddOutput { seat, .. }
        | Op::FocusOutput { seat, .. }
        | Op::AddWorkspace { seat, .. }
        | Op::RemoveWorkspace { seat, .. }
        | Op::FocusWorkspace { seat, .. }
        | Op::FocusWorkspaceRelative { seat, .. }
        | Op::ToggleSpecial { seat, .. }
        | Op::SetTiling { seat, .. }
        | Op::Focus { seat, .. }
        | Op::MoveFocused { seat, .. }
        | Op::ConsumeOrExpel { seat, .. }
        | Op::SwitchFocusFloating { seat }
        | Op::MoveToWorkspace { seat, .. }
        | Op::MoveToWorkspaceRelative { seat, .. }
        | Op::MoveToSpecial { seat, .. }
        | Op::Open { seat, .. } => Some(SeatId(seat)),
        Op::RemoveOutput(o) => output_seat(o),
        Op::Close(w)
        | Op::FocusWindow(w)
        | Op::SetFloating(w, _)
        | Op::SetFloatingRect { window: w, .. }
        | Op::SetFullscreen(w, _)
        | Op::SetFakeFullscreen(w, _)
        | Op::SetFullWidth(w, _)
        | Op::CyclePresetWidth(w, _)
        | Op::ChangeWidth(w, _)
        | Op::SetPinned(w, _) => window_seat(w),
    }
}

/// Everything a seat holds that can be read: what its outputs show, their workspaces and special
/// workspaces, its focus, and the place and state of each of its windows, hidden ones included.
/// Compared before and after another seat's operation.
fn seat_state(layout: &Layout, seat: SeatId) -> String {
    let outputs = layout.outputs(seat).unwrap_or_default();
    let shown: Vec<_> = outputs
        .iter()
        .map(|&o| {
            (
                o,
                layout.arrange(o).ok(),
                layout.workspaces(o).ok(),
                layout.shown_special(o).ok(),
                layout.output_rect(o).ok(),
            )
        })
        .collect();
    let windows: Vec<_> = (1..=8u64)
        .filter_map(|w| layout.window(WindowId(w)))
        .filter(|info| info.seat == seat)
        .collect();
    format!(
        "{shown:?} {:?} {:?} {windows:?}",
        layout.focused(seat),
        layout.active_output(seat)
    )
}

fn apply(layout: &mut Layout, op: &Op) -> Result<(), yuna_layout::LayoutError> {
    let s = SeatId;
    match *op {
        Op::AddOutput {
            seat,
            output,
            scale,
            size,
            x,
        } => layout.add_output(
            s(seat),
            OutputId(output),
            OutputConfig {
                position: Point {
                    x: f64::from(x),
                    y: 0.0,
                },
                physical_width: SIZES[size].0,
                physical_height: SIZES[size].1,
                scale: SCALES[scale],
            },
        ),
        Op::RemoveOutput(o) => layout.remove_output(OutputId(o)),
        Op::FocusOutput { seat, output } => layout.focus_output(s(seat), OutputId(output)),
        Op::AddWorkspace { seat, name, output } => {
            layout.add_workspace(s(seat), NAMES[name], output.map(OutputId))
        }
        Op::RemoveWorkspace { seat, name } => layout.remove_workspace(s(seat), NAMES[name]),
        Op::FocusWorkspace { seat, name } => layout.focus_workspace(s(seat), NAMES[name]),
        Op::FocusWorkspaceRelative { seat, direction } => {
            layout.focus_workspace_relative(s(seat), direction)
        }
        Op::ToggleSpecial { seat, name } => layout.toggle_special(s(seat), NAMES[name]),
        Op::SetTiling { seat, dwindle } => layout.set_tiling(
            s(seat),
            if dwindle {
                Tiling::Dwindle
            } else {
                Tiling::Scrolling
            },
        ),
        Op::Focus { seat, direction } => layout.focus(s(seat), direction),
        Op::MoveFocused { seat, direction } => layout.move_focused(s(seat), direction),
        Op::ConsumeOrExpel { seat, direction } => layout.consume_or_expel(s(seat), direction),
        Op::SwitchFocusFloating { seat } => layout.switch_focus_floating(s(seat)),
        Op::MoveToWorkspace { seat, name } => layout.move_to_workspace(s(seat), NAMES[name]),
        Op::MoveToWorkspaceRelative { seat, direction } => {
            layout.move_to_workspace_relative(s(seat), direction)
        }
        Op::MoveToSpecial { seat, name } => layout.move_to_special(s(seat), NAMES[name]),
        Op::Open {
            seat,
            window,
            floating,
            fullscreen,
            width,
            workspace,
        } => layout.open_window(
            s(seat),
            WindowId(window),
            OpenOptions {
                floating,
                fullscreen,
                size: None,
                width,
                workspace: workspace.map(|i| NAMES[i].to_owned()),
            },
        ),
        Op::Close(w) => layout.close_window(WindowId(w)),
        Op::FocusWindow(w) => layout.focus_window(WindowId(w)),
        Op::SetFloating(w, on) => layout.set_floating(WindowId(w), on),
        Op::SetFloatingRect { window, x, y, w, h } => layout.set_floating_rect(
            WindowId(window),
            Rect::new(f64::from(x), f64::from(y), f64::from(w), f64::from(h)),
        ),
        Op::SetFullscreen(w, on) => layout.set_fullscreen(WindowId(w), on),
        Op::SetFakeFullscreen(w, on) => layout.set_fake_fullscreen(WindowId(w), on),
        Op::SetFullWidth(w, on) => layout.set_full_width(WindowId(w), on),
        Op::CyclePresetWidth(w, forward) => layout.cycle_preset_width(WindowId(w), forward),
        Op::ChangeWidth(w, delta) => layout.change_width(WindowId(w), f64::from(delta) / 50.0),
        Op::SetPinned(w, on) => layout.set_pinned(WindowId(w), on),
    }
}

/// Runs `ops` from two seats without outputs, checking every property after each step.
fn run(options: Options, ops: &[Op]) -> Result<(), TestCaseError> {
    let mut layout = Layout::new(options).expect("the strategy gives valid options");
    layout.add_seat(SeatId(1)).unwrap();
    layout.add_seat(SeatId(2)).unwrap();
    for (step, op) in ops.iter().enumerate() {
        let actor = acting_seat(&layout, op);
        let others: Vec<_> = SEATS
            .iter()
            .map(|&s| SeatId(s))
            .filter(|&s| Some(s) != actor)
            .map(|s| (s, seat_state(&layout, s)))
            .collect();
        let before = format!("{layout:?}");
        let result = apply(&mut layout, op);
        if let Err(error) = layout.check_invariants() {
            return Err(TestCaseError::fail(format!(
                "step {step}, {op:?}: invariant broken: {error}"
            )));
        }
        if result.is_err() {
            prop_assert_eq!(
                &before,
                &format!("{layout:?}"),
                "step {}, {:?} failed but changed the model",
                step,
                op
            );
        }
        for (seat, view) in others {
            prop_assert_eq!(
                &view,
                &seat_state(&layout, seat),
                "step {}, {:?} changed seat {:?}",
                step,
                op,
                seat
            );
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_operations_keep_every_property(
        options in options(),
        ops in prop::collection::vec(op(), 1..80),
    ) {
        run(options, &ops)?;
    }
}

/// A sequence that starts from a busy state, so that short random tails reach deep states.
#[test]
fn operations_from_a_busy_state_keep_every_property() {
    let mut start = vec![
        Op::AddOutput {
            seat: 1,
            output: 1,
            scale: 2,
            size: 0,
            x: 0,
        },
        Op::AddOutput {
            seat: 1,
            output: 2,
            scale: 0,
            size: 1,
            x: 2560,
        },
        Op::AddOutput {
            seat: 2,
            output: 3,
            scale: 0,
            size: 1,
            x: -1920,
        },
    ];
    for window in 1..=8 {
        start.push(Op::Open {
            seat: 1 + window % 2,
            window,
            floating: window % 4 == 0,
            fullscreen: false,
            width: None,
            workspace: None,
        });
    }
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 512,
        ..ProptestConfig::default()
    });
    runner
        .run(
            &(options(), prop::collection::vec(op(), 1..40)),
            |(options, tail)| {
                let ops: Vec<Op> = start.iter().cloned().chain(tail).collect();
                run(options, &ops)
            },
        )
        .unwrap();
}
