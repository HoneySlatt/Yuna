//! Property tests of the motion of an arrangement: random sequences of layout operations over two
//! seats and up to three outputs of different sizes and scales, each followed by an update,
//! mostly animated, else instant on every output or on one seat's; random clock steps of 0 to 50 ms between them; frames sampled at random
//! times, some before the last update; random settings. After every step:
//!
//! - every drawn value is finite, every alpha within 0 and 1, and a frame draws a window once;
//! - nothing jumps on screen at an operation: a window drawn just before an update that animates
//!   its output's seat and just after it, on the same output, in the same container (its workspace and layer, or
//!   floating and tiled within one workspace, or pinned), its fullscreen unchanged and its
//!   workspace's too, is drawn with its corner at the same place, to 1e-6; its size may jump
//!   ([ADR-0052](../../../docs/adr/0052-motion-in-a-pure-crate.md)). One exception: when a
//!   workspace leaves the output during a switch (one starting or one drawn), the switch's stack
//!   closes up, and if it filled the view, the workspaces beside it cannot all keep their places;
//!   at such an update, the main workspaces' windows are not checked, the special and pinned ones
//!   are. A window that vanishes at an update is not checked: closing, or moving to a workspace
//!   not shown, takes it away at once;
//! - an update instant for an output's seat ends that output's motions;
//! - at rest, from the end the motion gives on, the frame is exactly the layout's arrangement,
//!   and the end is then `None`;
//! - every window whose place overlaps its output is told it is visible.
//!
//! A failure shrinks to the shortest sequence that still fails; proptest keeps its seed in
//! `arrangement_properties.proptest-regressions`, to be committed so that the case runs again.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use proptest::prelude::*;
use yuna_layout::geometry::{Point, Rect, Size};
use yuna_layout::{
    Direction, Layer, Layout, OpenOptions, Options, OutputConfig, OutputId, SeatId, Tiling,
    WindowId, WorkspaceId,
};
use yuna_motion::{Change, CurveParams, Drawn, Easing, Frame, Motion, Settings, SpringParams};

/// The seats that may exist: seat 1 from the start, seat 2 added and removed by the steps.
const SEATS: [u64; 2] = [1, 2];
const SCALES: [f64; 4] = [1.0, 1.25, 1.5, 2.0];
const SIZES: [(u32, u32); 3] = [(1920, 1080), (1280, 800), (3840, 2160)];
const NAMES: [&str; 3] = ["1", "2", "3"];

#[derive(Debug, Clone)]
enum Op {
    AddSeat,
    RemoveSeat,
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
    Open {
        seat: u64,
        window: u64,
        floating: bool,
        fullscreen: bool,
    },
    Close(u64),
    Focus {
        seat: u64,
        direction: Direction,
    },
    FocusWindow(u64),
    MoveFocused {
        seat: u64,
        direction: Direction,
    },
    ConsumeOrExpel {
        seat: u64,
        direction: Direction,
    },
    ChangeWidth(u64, i8),
    CyclePresetWidth(u64, bool),
    SetFullWidth(u64, bool),
    SetFullscreen(u64, bool),
    SetFloating(u64, bool),
    SetFloatingRect {
        window: u64,
        x: i16,
        y: i16,
    },
    SetPinned(u64, bool),
    CommitSize {
        window: u64,
        w: u16,
        h: u16,
    },
    AddWorkspace {
        seat: u64,
        name: usize,
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
    MoveToWorkspace {
        seat: u64,
        name: usize,
    },
    ToggleSpecial {
        seat: u64,
        name: usize,
    },
    MoveToSpecial {
        seat: u64,
        name: usize,
    },
    SetTiling {
        seat: u64,
        dwindle: bool,
    },
}

#[derive(Debug, Clone)]
enum Step {
    /// An operation on the layout, then an update with this change.
    Operate(Op, Change),
    /// The clock moves on by this many milliseconds.
    Clock(u64),
    /// Every output's frame is sampled this many milliseconds from now, before it or after.
    Sample(i64),
    /// The settings change, which ends every motion.
    Settings(Settings),
}

fn seat() -> impl Strategy<Value = u64> {
    prop_oneof![7 => Just(1u64), 1 => Just(2u64)]
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

fn op() -> impl Strategy<Value = Op> {
    let name = 0..NAMES.len();
    prop_oneof![
        1 => Just(Op::AddSeat),
        1 => Just(Op::RemoveSeat),
        2 => (seat(), output(), 0..SCALES.len(), 0..SIZES.len(), -3000..3000i16)
            .prop_map(|(seat, output, scale, size, x)| Op::AddOutput { seat, output, scale, size, x }),
        1 => output().prop_map(Op::RemoveOutput),
        1 => (seat(), output()).prop_map(|(seat, output)| Op::FocusOutput { seat, output }),
        8 => (seat(), window(), prop::bool::weighted(0.25), prop::bool::weighted(0.1))
            .prop_map(|(seat, window, floating, fullscreen)| Op::Open { seat, window, floating, fullscreen }),
        3 => window().prop_map(Op::Close),
        6 => (seat(), direction()).prop_map(|(seat, direction)| Op::Focus { seat, direction }),
        3 => window().prop_map(Op::FocusWindow),
        3 => (seat(), direction()).prop_map(|(seat, direction)| Op::MoveFocused { seat, direction }),
        3 => (seat(), direction()).prop_map(|(seat, direction)| Op::ConsumeOrExpel { seat, direction }),
        2 => (window(), any::<i8>()).prop_map(|(w, d)| Op::ChangeWidth(w, d)),
        2 => (window(), any::<bool>()).prop_map(|(w, on)| Op::CyclePresetWidth(w, on)),
        1 => (window(), any::<bool>()).prop_map(|(w, on)| Op::SetFullWidth(w, on)),
        2 => (window(), any::<bool>()).prop_map(|(w, on)| Op::SetFullscreen(w, on)),
        2 => (window(), any::<bool>()).prop_map(|(w, on)| Op::SetFloating(w, on)),
        2 => (window(), -500..2500i16, -300..1500i16)
            .prop_map(|(window, x, y)| Op::SetFloatingRect { window, x, y }),
        1 => (window(), any::<bool>()).prop_map(|(w, on)| Op::SetPinned(w, on)),
        2 => (window(), 1..3000u16, 1..2000u16)
            .prop_map(|(window, w, h)| Op::CommitSize { window, w, h }),
        1 => (seat(), name.clone()).prop_map(|(seat, name)| Op::AddWorkspace { seat, name }),
        1 => (seat(), name.clone()).prop_map(|(seat, name)| Op::RemoveWorkspace { seat, name }),
        4 => (seat(), name.clone()).prop_map(|(seat, name)| Op::FocusWorkspace { seat, name }),
        2 => (seat(), direction())
            .prop_map(|(seat, direction)| Op::FocusWorkspaceRelative { seat, direction }),
        2 => (seat(), name.clone()).prop_map(|(seat, name)| Op::MoveToWorkspace { seat, name }),
        3 => (seat(), name.clone()).prop_map(|(seat, name)| Op::ToggleSpecial { seat, name }),
        1 => (seat(), name.clone()).prop_map(|(seat, name)| Op::MoveToSpecial { seat, name }),
        1 => (seat(), any::<bool>()).prop_map(|(seat, dwindle)| Op::SetTiling { seat, dwindle }),
    ]
}

fn spring_params() -> impl Strategy<Value = SpringParams> {
    (
        prop_oneof![Just(0.6), Just(1.0), Just(1.4)],
        prop_oneof![Just(300.0), Just(800.0), Just(1500.0)],
    )
        .prop_map(|(ratio, stiffness)| SpringParams::new(ratio, stiffness).unwrap())
}

fn settings() -> impl Strategy<Value = Settings> {
    let easing = prop_oneof![
        Just(Easing::EaseOutCubic),
        Just(Easing::Linear),
        Just(Easing::CubicBezier {
            x1: 0.3,
            y1: -0.6,
            x2: 0.4,
            y2: 1.8
        }),
    ];
    (
        prop::bool::weighted(0.05),
        prop::bool::weighted(0.1),
        spring_params(),
        spring_params(),
        spring_params(),
        easing,
        50..400u64,
    )
        .prop_map(
            |(off, reduced_motion, camera, windows, workspace_switch, easing, fade)| Settings {
                off,
                reduced_motion,
                camera,
                windows,
                workspace_switch,
                special_fade: CurveParams::new(easing, Duration::from_millis(fade)).unwrap(),
            },
        )
}

fn step() -> impl Strategy<Value = Step> {
    let change = prop_oneof![
        9 => Just(Change::Animated),
        1 => Just(Change::Instant),
        1 => seat().prop_map(|seat| Change::InstantFor(BTreeSet::from([SeatId(seat)]))),
    ];
    prop_oneof![
        12 => (op(), change).prop_map(|(op, change)| Step::Operate(op, change)),
        6 => (0..=50u64).prop_map(Step::Clock),
        3 => (-20..600i64).prop_map(Step::Sample),
        1 => settings().prop_map(Step::Settings),
    ]
}

fn options() -> impl Strategy<Value = Options> {
    (any::<bool>(), any::<bool>()).prop_map(|(dynamic, dwindle)| Options {
        dynamic_workspaces: dynamic,
        default_tiling: if dwindle {
            Tiling::Dwindle
        } else {
            Tiling::Scrolling
        },
        ..Options::default()
    })
}

/// The layout, its motion, the scale of each output added, and the clock.
struct World {
    layout: Layout,
    motion: Motion,
    /// Whether nothing moves across the screen: animations off, or reduced motion; continuity is
    /// then not checked, since every move shows at once.
    off: bool,
    scales: HashMap<OutputId, f64>,
    now: Duration,
}

impl World {
    /// Every output of every seat, with its scale.
    fn outputs(&self) -> Vec<(OutputId, f64)> {
        SEATS
            .iter()
            .flat_map(|&seat| self.layout.outputs(SeatId(seat)).unwrap_or_default())
            .map(|output| (output, self.scales[&output]))
            .collect()
    }

    /// Each output's frame at `at`, with the output's rectangle and scale.
    fn frames(&self, at: Duration) -> HashMap<OutputId, (Frame, Rect, f64)> {
        self.outputs()
            .into_iter()
            .map(|(output, scale)| {
                let rect = self.layout.output_rect(output).unwrap();
                (output, (self.motion.frame(output, at), rect, scale))
            })
            .collect()
    }

    fn apply(&mut self, op: &Op) {
        let s = SeatId;
        let w = WindowId;
        let l = &mut self.layout;
        // A refused operation changes nothing: what follows is checked all the same.
        let _ = match *op {
            Op::AddSeat => l.add_seat(s(2)),
            Op::RemoveSeat => l.remove_seat(s(2)),
            Op::AddOutput {
                seat,
                output,
                scale,
                size,
                x,
            } => {
                let config = OutputConfig {
                    position: Point {
                        x: f64::from(x),
                        y: 0.0,
                    },
                    physical_width: SIZES[size].0,
                    physical_height: SIZES[size].1,
                    scale: SCALES[scale],
                };
                let added = l.add_output(s(seat), OutputId(output), config);
                if added.is_ok() {
                    self.scales.insert(OutputId(output), SCALES[scale]);
                }
                added
            }
            Op::RemoveOutput(o) => l.remove_output(OutputId(o)),
            Op::FocusOutput { seat, output } => l.focus_output(s(seat), OutputId(output)),
            Op::Open {
                seat,
                window,
                floating,
                fullscreen,
            } => {
                let options = OpenOptions {
                    floating,
                    fullscreen,
                    committed: floating.then_some(Size { w: 500.0, h: 400.0 }),
                    ..OpenOptions::default()
                };
                l.open_window(s(seat), w(window), options)
            }
            Op::Close(window) => l.close_window(w(window)),
            Op::Focus { seat, direction } => l.focus(s(seat), direction),
            Op::FocusWindow(window) => l.focus_window(w(window)),
            Op::MoveFocused { seat, direction } => l.move_focused(s(seat), direction),
            Op::ConsumeOrExpel { seat, direction } => l.consume_or_expel(s(seat), direction),
            Op::ChangeWidth(window, delta) => l.change_width(w(window), f64::from(delta) / 200.0),
            Op::CyclePresetWidth(window, forward) => l.cycle_preset_width(w(window), forward),
            Op::SetFullWidth(window, on) => l.set_full_width(w(window), on),
            Op::SetFullscreen(window, on) => l.set_fullscreen(w(window), on),
            Op::SetFloating(window, on) => l.set_floating(w(window), on),
            Op::SetFloatingRect { window, x, y } => l.set_floating_rect(
                w(window),
                Rect::new(f64::from(x), f64::from(y), 500.0, 400.0),
            ),
            Op::SetPinned(window, on) => l.set_pinned(w(window), on),
            Op::CommitSize {
                window,
                w: width,
                h,
            } => l.commit_size(
                w(window),
                Size {
                    w: f64::from(width),
                    h: f64::from(h),
                },
            ),
            Op::AddWorkspace { seat, name } => l.add_workspace(s(seat), NAMES[name], None),
            Op::RemoveWorkspace { seat, name } => l.remove_workspace(s(seat), NAMES[name]),
            Op::FocusWorkspace { seat, name } => l.focus_workspace(s(seat), NAMES[name]),
            Op::FocusWorkspaceRelative { seat, direction } => {
                l.focus_workspace_relative(s(seat), direction)
            }
            Op::MoveToWorkspace { seat, name } => l.move_to_workspace(s(seat), NAMES[name]),
            Op::ToggleSpecial { seat, name } => l.toggle_special(s(seat), NAMES[name]),
            Op::MoveToSpecial { seat, name } => l.move_to_special(s(seat), NAMES[name]),
            Op::SetTiling { seat, dwindle } => l.set_tiling(
                s(seat),
                if dwindle {
                    Tiling::Dwindle
                } else {
                    Tiling::Scrolling
                },
            ),
        };
    }

    fn update(&mut self, change: Change) {
        self.motion.update(&self.layout, self.now, change);
    }
}

/// Whether `change` turns what moved on `output` into motion: whether it is animated for the seat
/// that holds `output`.
fn animates(change: &Change, layout: &Layout, output: OutputId) -> bool {
    match change {
        Change::Animated => true,
        Change::Instant => false,
        Change::InstantFor(seats) => !seats.iter().any(|&seat| {
            layout
                .outputs(seat)
                .is_ok_and(|outputs| outputs.contains(&output))
        }),
    }
}

/// The layout's arrangement as a frame at rest draws it.
fn arrangement(layout: &Layout, output: OutputId) -> Frame {
    let windows = layout
        .arrange(output)
        .unwrap()
        .into_iter()
        .map(|placed| Drawn {
            window: placed.window,
            rect: placed.rect,
            alpha: 1.0,
            workspace: placed.workspace,
            placed,
        })
        .collect();
    Frame { windows }
}

/// Every value finite, every alpha within 0 and 1, each window once.
fn check_frame(frame: &Frame) -> Result<(), TestCaseError> {
    let mut seen = HashSet::new();
    for drawn in &frame.windows {
        let r = drawn.rect;
        prop_assert!(
            [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()),
            "{drawn:?} is not finite"
        );
        prop_assert!((0.0..=1.0).contains(&drawn.alpha), "{drawn:?}'s alpha");
        prop_assert!(
            seen.insert(drawn.window),
            "{:?} is drawn twice",
            drawn.window
        );
    }
    Ok(())
}

/// What the brief calls staying in a container: the same workspace and layer, floating and tiled
/// within one workspace, or pinned; the fullscreen unchanged.
fn keeps_its_motion(before: &Drawn, after: &Drawn) -> bool {
    let (b, a) = (&before.placed, &after.placed);
    b.fullscreen == a.fullscreen
        && match (b.layer, a.layer) {
            (Layer::Pinned, Layer::Pinned) => true,
            (Layer::Pinned, _) | (_, Layer::Pinned) => false,
            (x, y) => {
                b.workspace == a.workspace
                    && (x == y || x == Layer::Floating || y == Layer::Floating)
            }
        }
}

/// Each window the layout makes fullscreen on `output`, with its workspace: on every workspace
/// of the output, shown or not, and the special one shown over it; pinned windows aside, since a
/// switch moves them between workspaces.
fn fullscreen_windows(layout: &Layout, output: OutputId) -> HashSet<(WorkspaceId, WindowId)> {
    let workspaces = layout.workspaces(output).unwrap_or_default();
    let peeked = workspaces
        .iter()
        .flat_map(|ws| layout.arrange_workspace(output, ws.id).unwrap_or_default());
    let shown = layout.arrange(output).unwrap_or_default();
    peeked
        .chain(shown)
        .filter(|p| p.fullscreen && p.layer != Layer::Pinned)
        .map(|p| (p.workspace, p.window))
        .collect()
}

/// The workspaces the layout lists on `output`.
fn workspaces_of(layout: &Layout, output: OutputId) -> HashSet<WorkspaceId> {
    let workspaces = layout.workspaces(output).unwrap_or_default();
    workspaces
        .into_iter()
        .map(|workspace| workspace.id)
        .collect()
}

/// No window that keeps its motion jumps on screen at the update. The windows of `exempt`, but
/// pinned ones, are not checked: workspaces where a window went fullscreen or left it, which are
/// instant, and the main workspaces when one left the output.
fn check_continuity(
    before: &Frame,
    after: &Frame,
    exempt: &HashSet<WorkspaceId>,
) -> Result<(), TestCaseError> {
    for now in &after.windows {
        let Some(was) = before.windows.iter().find(|d| d.window == now.window) else {
            continue;
        };
        let exempted = now.placed.layer != Layer::Pinned && exempt.contains(&now.workspace);
        if !keeps_its_motion(was, now) || exempted {
            continue;
        }
        prop_assert!(
            (now.rect.x - was.rect.x).abs() <= 1e-6 && (now.rect.y - was.rect.y).abs() <= 1e-6,
            "{:?} jumps from {:?} to {:?}",
            now.window,
            was,
            now
        );
    }
    Ok(())
}

/// At `at`, for each output: frames well formed, the windows whose place overlaps told, and at
/// rest, from the end on, the arrangement exactly.
fn check_at(world: &World, at: Duration) -> Result<(), TestCaseError> {
    for (output, _) in world.outputs() {
        let frame = world.motion.frame(output, at);
        check_frame(&frame)?;
        let rect = world.layout.output_rect(output).unwrap();
        let told = world.motion.told_visible(output, at);
        for placed in world.layout.arrange(output).unwrap() {
            if placed.rect.overlaps(&rect) {
                prop_assert!(
                    told.contains(&placed.window),
                    "{:?} overlaps {output:?} but is not told",
                    placed.window
                );
            }
        }
        let arranged = arrangement(&world.layout, output);
        match world.motion.end(output, at) {
            None => prop_assert_eq!(&frame, &arranged),
            Some(end) => {
                prop_assert!(end > at);
                prop_assert_eq!(&world.motion.frame(output, end), &arranged);
                prop_assert_eq!(world.motion.end(output, end), None);
                prop_assert_eq!(world.motion.end(output, end + Duration::from_secs(1)), None);
                let last = world.motion.frame(output, end - Duration::from_nanos(1));
                check_soft_end(&last, &arranged, rect, world.scales[&output])?;
            }
        }
    }
    Ok(())
}

/// Nothing jumps when the motions end: from the frame a nanosecond before the end, `last`, to
/// the arrangement, `rest`, a window drawn in both moves its corner by a physical pixel at most
/// and its alpha by a hundredth. One drawn in only one of them, sliding out or fading out
/// before, or out of view during a switch and listed at rest, is transparent there or shows at
/// most a physical pixel of itself: the last step of a slide out of view.
fn check_soft_end(
    last: &Frame,
    rest: &Frame,
    output: Rect,
    scale: f64,
) -> Result<(), TestCaseError> {
    let pixel = 1.0 / scale + 1e-9;
    let in_one =
        |drawn: &Drawn, other: &Frame| other.windows.iter().all(|d| d.window != drawn.window);
    for at_rest in &rest.windows {
        if let Some(drawn) = last.windows.iter().find(|d| d.window == at_rest.window) {
            prop_assert!(
                (drawn.rect.x - at_rest.rect.x).abs() <= pixel
                    && (drawn.rect.y - at_rest.rect.y).abs() <= pixel
                    && (drawn.alpha - at_rest.alpha).abs() <= 0.01,
                "{:?} jumps from {:?} to {:?} at the end",
                drawn.window,
                drawn,
                at_rest
            );
        }
    }
    for drawn in &rest.windows {
        if in_one(drawn, last) {
            prop_assert!(
                shows_at_most(drawn, output, pixel),
                "{drawn:?} appears at the end"
            );
        }
    }
    for drawn in &last.windows {
        if in_one(drawn, rest) {
            prop_assert!(
                drawn.alpha <= 0.01 || shows_at_most(drawn, output, pixel),
                "{drawn:?} vanishes at the end"
            );
        }
    }
    Ok(())
}

/// Whether `drawn` shows at most `pixel` of itself on `output`, across or along, beyond what its
/// place hangs over the output's edges. A switch slides by the output's height, so a window
/// taller than its place, whose client committed more than it was asked, shows that much past
/// its workspace until the switch ends.
fn shows_at_most(drawn: &Drawn, output: Rect, pixel: f64) -> bool {
    let (r, place) = (drawn.rect, drawn.placed.rect);
    let shown_w = r.right().min(output.right()) - r.x.max(output.x);
    let shown_h = r.bottom().min(output.bottom()) - r.y.max(output.y);
    let overhang_w = (output.x - place.x)
        .max(place.right() - output.right())
        .max(0.0);
    let overhang_h = (output.y - place.y)
        .max(place.bottom() - output.bottom())
        .max(0.0);
    shown_w <= pixel + overhang_w || shown_h <= pixel + overhang_h
}

fn run(options: Options, settings: Settings, steps: &[Step]) -> Result<(), TestCaseError> {
    let mut layout = Layout::new(options).unwrap();
    layout.add_seat(SeatId(1)).unwrap();
    let config = OutputConfig {
        position: Point { x: 0.0, y: 0.0 },
        physical_width: 1920,
        physical_height: 1080,
        scale: 1.0,
    };
    layout.add_output(SeatId(1), OutputId(1), config).unwrap();
    for window in 1..=3 {
        let options = OpenOptions::default();
        layout
            .open_window(SeatId(1), WindowId(window), options)
            .unwrap();
    }
    let mut world = World {
        layout,
        motion: Motion::new(settings),
        off: settings.off || settings.reduced_motion,
        scales: HashMap::from([(OutputId(1), 1.0)]),
        now: Duration::from_secs(10),
    };
    world.update(Change::Instant);
    for step in steps {
        match step {
            Step::Operate(op, change) => {
                let before = world.frames(world.now);
                let fullscreen: HashMap<OutputId, _> = before
                    .keys()
                    .map(|&output| (output, fullscreen_windows(&world.layout, output)))
                    .collect();
                let workspaces: HashMap<OutputId, _> = before
                    .keys()
                    .map(|&output| (output, workspaces_of(&world.layout, output)))
                    .collect();
                let shown_before: HashMap<OutputId, _> = before
                    .keys()
                    .filter_map(|&output| {
                        Some((output, world.layout.shown_workspaces(output).ok()?))
                    })
                    .collect();
                world.apply(op);
                world.update(change.clone());
                let after = world.frames(world.now);
                for output in after.keys() {
                    if !animates(change, &world.layout, *output) {
                        prop_assert_eq!(world.motion.end(*output, world.now), None);
                    }
                }
                if !world.off {
                    for (output, (frame, rect, scale)) in &after {
                        if !animates(change, &world.layout, *output) {
                            continue;
                        }
                        if let Some((was, was_rect, was_scale)) = before.get(output)
                            && (was_rect, was_scale) == (rect, scale)
                        {
                            let mut exempt: HashSet<WorkspaceId> = fullscreen[output]
                                .symmetric_difference(&fullscreen_windows(&world.layout, *output))
                                .map(|&(workspace, _)| workspace)
                                .collect();
                            let now_there = workspaces_of(&world.layout, *output);
                            let switching = shown_before.get(output).is_some_and(|shown| {
                                let main_changed = world
                                    .layout
                                    .shown_workspaces(*output)
                                    .is_ok_and(|now| now.main.id != shown.main.id);
                                let mains_drawn: HashSet<WorkspaceId> = was
                                    .windows
                                    .iter()
                                    .filter(|d| d.placed.layer != Layer::Pinned)
                                    .filter(|d| Some(d.workspace) != shown.special.map(|v| v.id))
                                    .map(|d| d.workspace)
                                    .collect();
                                main_changed || mains_drawn.len() > 1
                            });
                            if switching && !workspaces[output].is_subset(&now_there) {
                                exempt.extend(&workspaces[output]);
                                exempt.extend(now_there);
                            }
                            check_continuity(was, frame, &exempt)?;
                        }
                    }
                }
                check_at(&world, world.now)?;
            }
            Step::Clock(ms) => {
                world.now += Duration::from_millis(*ms);
                check_at(&world, world.now)?;
            }
            Step::Sample(ms) => {
                let at = if *ms < 0 {
                    world
                        .now
                        .saturating_sub(Duration::from_millis(ms.unsigned_abs()))
                } else {
                    world.now + Duration::from_millis(ms.unsigned_abs())
                };
                check_at(&world, at)?;
            }
            Step::Settings(settings) => {
                world.motion.set_settings(*settings);
                world.off = settings.off || settings.reduced_motion;
                check_at(&world, world.now)?;
                for (output, _) in world.outputs() {
                    prop_assert_eq!(world.motion.end(output, world.now), None);
                }
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn the_motion_never_jumps_rests_on_the_arrangement_and_tells_what_overlaps(
        options in options(),
        settings in settings(),
        steps in prop::collection::vec(step(), 30..80),
    ) {
        run(options, settings, &steps)?;
    }
}
