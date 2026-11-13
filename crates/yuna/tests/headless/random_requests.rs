//! Random sequences of requests (ADR-0007, ADR-0025), with proptest: up to three clients send any
//! request in any order, with any values, extremes and nonsense included, while the test moves
//! the clock, changes the layout as key bindings would, and focuses windows. A request that breaks
//! the protocol ends its client, which is allowed; its slot connects a new client at its next
//! request.
//!
//! The requests that need the keyboard's focus and a real serial, popup grabs, activation tokens,
//! activations and selections, are often sent by the client that has the focus, naming the surface
//! its keyboard is on and the serial of its last `enter`, so that they take effect as well as
//! being refused. The test counts how often they take effect ([`Reach`]), and prints it.
//!
//! After every step: Yuna dispatches; the layout keeps its invariants; the layout and the shell
//! agree on windows ([`Compositor::check_windows`]); and the popups and the keyboard are sound
//! ([`Compositor::check_popups_and_keyboard`]). After the sequence, a sentinel client that joined
//! first and stayed well-behaved, focused by a key binding, gets its frame callback at the next
//! refresh boundary; once every client has gone, Yuna rests.
//!
//! Two tests share the cases, so that a runner with several cores runs them at once: one starts
//! from a busy state, three clients with a window, a dialog and a menu each; the other from the
//! sentinel alone. A failure shrinks to the shortest sequence that still fails; proptest keeps its
//! seed in `random_requests.proptest-regressions`, beside this file, committed so that the case
//! runs at every check. A seed replays its case only while the generator stays the same: when the
//! generator changes, the sequences found move to [`sequences_found_before`]. The case counts fit
//! about 10 s of the check in a debug build (`docs/ARCHITECTURE.md`); `PROPTEST_CASES=<n>` runs
//! `n` in each test.

use std::cell::RefCell;
use std::fmt;
use std::time::Duration;

use proptest::prelude::*;
use proptest::test_runner::{FileFailurePersistence, TestRunner, contextualize_config};
use wayland_client::protocol::wl_surface::WlSurface;
use yuna::yuna_layout::{Direction, OutputId, Tiling, WindowId};
use yuna::{ClockMode, Compositor, Config, OWNER_SEAT, OutputMode};

use crate::client::{Anchor, Client, ConstraintAdjustment, Gravity, Parent, Placement};
use crate::selection_client::{SelectionClient, SelectionEvent};
use crate::{LEAD, assert_rests, run_armed_deadlines};

/// The clients that send random requests, beside the sentinel.
const CLIENTS: usize = 3;
/// The cases of each of the two tests.
const CASES_PER_TEST: u32 = 80;
/// The output: 1280 × 720 pixels at 60 Hz and scale 1.5, small so that frames cost little.
const OUTPUT_SIZE: (u32, u32) = (1280, 720);
const OUTPUT: OutputId = OutputId(1);
const RATE: u32 = 60;
/// 1 s / 60, to the nanosecond below.
const PERIOD: Duration = Duration::from_nanos(16_666_666);
const SCALE: f64 = 1.5;
const TEXT: &str = "text/plain;charset=utf-8";

/// Who sends a request.
#[derive(Debug, Clone, Copy)]
enum Who {
    /// The client of this slot, connected at its first request.
    Slot(usize),
    /// The client that has the keyboard focus; the first slot's when none has it.
    Focused,
}

/// A serial a request names.
#[derive(Debug, Clone, Copy)]
enum Serial {
    /// None, where the request allows it; else as `Real`.
    Absent,
    /// The one a well-behaved client would name: its window's last configure, or the keyboard's
    /// last `enter`.
    Real,
    Any(u32),
}

/// A surface of the client: a window's or a popup's, by an index taken modulo their count, or
/// the one its keyboard is on.
#[derive(Debug, Clone, Copy)]
enum Target {
    Window(usize),
    Popup(usize),
    /// None when the client does not have the keyboard.
    Focused,
    /// A window but the one its keyboard is on, by an index taken modulo their count.
    OtherWindow(usize),
}

/// Which of its activation tokens a client uses.
#[derive(Debug, Clone, Copy)]
enum Token {
    Newest,
    /// By an index taken modulo the tokens it asked.
    Any(usize),
}

/// What a commit does to the surface's buffer.
#[derive(Debug, Clone, Copy)]
enum Buffer {
    /// A new one, of this size in pixels.
    New(i32, i32),
    /// None: the surface unmaps.
    Remove,
    /// The same.
    Keep,
}

/// A positioner's values, any of them nonsense.
#[derive(Debug, Clone, Copy)]
struct RandomPlacement {
    size: (i32, i32),
    anchor_rect: (i32, i32, i32, i32),
    anchor: u32,
    gravity: u32,
    adjustment: u32,
    offset: (i32, i32),
}

/// An object the client may destroy, in any order.
#[derive(Debug, Clone, Copy)]
enum Object {
    /// The window's objects, in the order a client destroys them.
    Window(usize),
    Decoration(usize),
    Toplevel(usize),
    XdgSurface(usize),
    Surface(usize),
    /// The popup's objects, in order.
    Popup(usize),
    PopupRole(usize),
    PopupXdgSurface(usize),
    PopupSurface(usize),
    Viewport(usize),
    FractionalScale(usize),
    Source(usize),
}

/// One request of a client. Indices are taken modulo the count of what they name; a request
/// that names nothing, the client having none, does nothing.
#[derive(Debug, Clone)]
enum Request {
    /// A new toplevel, with its initial commit.
    OpenToplevel,
    /// A frame as a well-behaved client draws it: the window's last configure acknowledged, a
    /// buffer of that size, or of its own when given or when the configure lets it choose,
    /// committed with a frame callback or not.
    Draw {
        target: Target,
        size: Option<(i32, i32)>,
        frame: bool,
    },
    Ack {
        window: usize,
        serial: Serial,
    },
    Commit {
        target: Target,
        buffer: Buffer,
        frame: bool,
    },
    SetMinSize {
        window: usize,
        size: (i32, i32),
    },
    SetMaxSize {
        window: usize,
        size: (i32, i32),
    },
    SetGeometry {
        window: usize,
        at: (i32, i32),
        size: (i32, i32),
    },
    Fullscreen {
        window: usize,
        on: bool,
    },
    Maximize {
        window: usize,
        on: bool,
    },
    /// Another of its toplevels, itself included, or none.
    SetParent {
        window: usize,
        parent: Option<usize>,
    },
    /// A popup on a window, on a popup, or on nothing; when `grab`, it asks a grab with the
    /// serial of the client's last `enter`, if it had one; then its initial commit.
    OpenPopup {
        parent: Option<Target>,
        placement: RandomPlacement,
        grab: bool,
    },
    Grab {
        popup: usize,
        serial: Serial,
    },
    Reposition {
        popup: usize,
        placement: RandomPlacement,
        token: u32,
    },
    AskToken {
        serial: Serial,
        surface: Option<Target>,
    },
    Activate {
        token: Token,
        target: Target,
    },
    Viewport {
        window: usize,
        destination: (i32, i32),
        source: Option<(i32, i32, i32, i32)>,
    },
    FractionalScale {
        window: usize,
    },
    SetSelection {
        primary: bool,
        serial: Serial,
    },
    /// A second role object for the surface of `target`: for a window's, a toplevel on its
    /// `xdg_surface` (0) or on a new one (1), or a popup (2); for a popup's, a popup. A popup
    /// hangs from `parent`, or from nothing. Allowed for a surface whose role object was
    /// destroyed, the client's protocol error otherwise.
    SecondRole {
        target: Target,
        kind: u8,
        parent: Option<Target>,
    },
    Destroy(Object),
    Disconnect,
}

/// A change of the layout, as a key binding of the owner's seat makes it.
#[derive(Debug, Clone, Copy)]
enum KeyBinding {
    Focus(Direction),
    FocusWorkspace(usize),
    MoveToWorkspace(usize),
    SetTiling(Tiling),
}

const WORKSPACES: [&str; 3] = ["1", "2", "3"];

/// One step of a sequence.
#[derive(Debug, Clone)]
enum Step {
    Client {
        who: Who,
        request: Request,
    },
    /// Moves the clock by this many milliseconds.
    Advance(u64),
    Layout(KeyBinding),
    /// A key binding focuses a window the output holds, by an index taken modulo their count,
    /// and the clock moves to the next boundary, which gives that window the keyboard.
    FocusWindow(usize),
}

/// How often the requests that need the keyboard's focus and a real serial took effect, as Yuna's
/// public state and the clients' own events show it.
#[derive(Debug, Default, Clone, Copy)]
struct Reach {
    /// Popup grabs asked; granted: the keyboard went to the popup.
    grabs: Tally,
    /// Activation tokens asked; valid: asked with the serial of the client's last `enter`, for
    /// the surface its keyboard is on, as Yuna asks of a valid token.
    tokens: Tally,
    /// Activations asked; done: the layout's focus moved to another window.
    activations: Tally,
    /// Selections set; taken: the client was told of its new selection.
    selections: Tally,
}

/// Of the requests of one kind, how many were sent and how many took effect.
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    asked: u64,
    took: u64,
}

/// What a request sent asked, of what [`Reach`] counts.
#[derive(Debug, Clone, Copy)]
enum Asked {
    Other,
    /// A popup grab, and the popup when it was created and committed with it, which may grant
    /// it at once.
    Grab {
        committed: Option<usize>,
    },
    /// A commit of a popup, which may grant a grab it asked before.
    PopupCommit(usize),
    Token {
        valid: bool,
    },
    Activation,
    Selection,
}

/// What [`Reach`] compares a step's outcome with, read before the step.
struct Before {
    keyboard_on: Option<WlSurface>,
    selections_told: usize,
    focused: Option<WindowId>,
}

/// A client slot: the first most often, so that one client lives long enough to reach deep
/// states, the others less, so that clients meet; or, at times, the client that has the keyboard
/// focus.
fn who() -> impl Strategy<Value = Who> {
    prop_oneof![
        6 => Just(Who::Slot(0)),
        3 => Just(Who::Slot(1)),
        1 => Just(Who::Slot(2)),
        2 => Just(Who::Focused),
    ]
}

fn index() -> impl Strategy<Value = usize> {
    0..8usize
}

/// A size as a request carries it, any `i32`: mostly what clients send, at times 0, negative,
/// the extremes, or around Yuna's bound on a positioner's values, 2^20.
fn size_value() -> impl Strategy<Value = i32> {
    prop_oneof![
        30 => 1..=2048i32,
        1 => -1..=0i32,
        1 => Just(i32::MAX),
        1 => Just(i32::MIN),
        1 => any::<i32>(),
        1 => ((1 << 20) - 1)..=((1 << 20) + 1),
    ]
}

/// A coordinate as a request carries it, any `i32`: mostly near a window, at times the extremes
/// or around 2^20 either way.
fn coordinate() -> impl Strategy<Value = i32> {
    prop_oneof![
        30 => -500..=2048i32,
        1 => Just(i32::MAX),
        1 => Just(i32::MIN),
        1 => any::<i32>(),
        1 => ((1 << 20) - 1)..=((1 << 20) + 1),
        1 => (-(1 << 20) - 1)..=(-(1 << 20) + 1),
    ]
}

fn size_pair() -> impl Strategy<Value = (i32, i32)> {
    (size_value(), size_value())
}

fn coordinate_pair() -> impl Strategy<Value = (i32, i32)> {
    (coordinate(), coordinate())
}

/// A minimum or maximum size: mostly within an output, often 0 (none), at times nonsense.
fn limit_pair() -> impl Strategy<Value = (i32, i32)> {
    let limit = prop_oneof![
        3 => Just(0),
        20 => 1..=2048i32,
        1 => -1..=-1i32,
        1 => Just(i32::MAX),
        1 => Just(i32::MIN),
        1 => any::<i32>(),
    ];
    (limit.clone(), limit)
}

fn serial() -> impl Strategy<Value = Serial> {
    prop_oneof![
        2 => Just(Serial::Absent),
        6 => Just(Serial::Real),
        1 => any::<u32>().prop_map(Serial::Any),
    ]
}

fn target() -> impl Strategy<Value = Target> {
    prop_oneof![
        3 => index().prop_map(Target::Window),
        1 => index().prop_map(Target::Popup),
        2 => Just(Target::Focused),
    ]
}

fn token() -> impl Strategy<Value = Token> {
    prop_oneof![
        1 => Just(Token::Newest),
        1 => index().prop_map(Token::Any),
    ]
}

fn buffer() -> impl Strategy<Value = Buffer> {
    prop_oneof![
        4 => (1..=2048i32, 1..=2048i32).prop_map(|(w, h)| Buffer::New(w, h)),
        1 => Just(Buffer::Remove),
        1 => Just(Buffer::Keep),
    ]
}

/// Any positioner, its values as [`size_value`] and [`coordinate`] give them: a third or so can
/// be placed, the others hold nonsense.
fn placement() -> impl Strategy<Value = RandomPlacement> {
    (
        size_pair(),
        (coordinate(), coordinate(), size_value(), size_value()),
        0..=8u32,
        0..=8u32,
        0..=63u32,
        coordinate_pair(),
    )
        .prop_map(
            |(size, anchor_rect, anchor, gravity, adjustment, offset)| RandomPlacement {
                size,
                anchor_rect,
                anchor,
                gravity,
                adjustment,
                offset,
            },
        )
}

/// A positioner a menu would have: a size and an anchor within a window, any rules.
fn menu_placement() -> impl Strategy<Value = RandomPlacement> {
    (
        (1..=600i32, 1..=600i32),
        (0..=400i32, 0..=400i32, 1..=50i32, 1..=50i32),
        0..=8u32,
        0..=8u32,
        0..=63u32,
        (-50..=50i32, -50..=50i32),
    )
        .prop_map(
            |(size, anchor_rect, anchor, gravity, adjustment, offset)| RandomPlacement {
                size,
                anchor_rect,
                anchor,
                gravity,
                adjustment,
                offset,
            },
        )
}

fn object() -> impl Strategy<Value = Object> {
    prop_oneof![
        3 => index().prop_map(Object::Window),
        1 => index().prop_map(Object::Decoration),
        1 => index().prop_map(Object::Toplevel),
        1 => index().prop_map(Object::XdgSurface),
        1 => index().prop_map(Object::Surface),
        2 => index().prop_map(Object::Popup),
        1 => index().prop_map(Object::PopupRole),
        1 => index().prop_map(Object::PopupXdgSurface),
        1 => index().prop_map(Object::PopupSurface),
        1 => index().prop_map(Object::Viewport),
        1 => index().prop_map(Object::FractionalScale),
        1 => index().prop_map(Object::Source),
    ]
}

fn request() -> impl Strategy<Value = Request> {
    let drawn_size = proptest::option::weighted(0.5, (1..=2048i32, 1..=2048i32));
    let popup_placement = prop_oneof![menu_placement(), placement()];
    prop_oneof![
        3 => Just(Request::OpenToplevel),
        6 => (target(), drawn_size, any::<bool>())
            .prop_map(|(target, size, frame)| Request::Draw { target, size, frame }),
        1 => (index(), serial()).prop_map(|(window, serial)| Request::Ack { window, serial }),
        2 => (target(), buffer(), any::<bool>())
            .prop_map(|(target, buffer, frame)| Request::Commit { target, buffer, frame }),
        1 => (index(), limit_pair())
            .prop_map(|(window, size)| Request::SetMinSize { window, size }),
        1 => (index(), limit_pair())
            .prop_map(|(window, size)| Request::SetMaxSize { window, size }),
        1 => (index(), coordinate_pair(), size_pair())
            .prop_map(|(window, at, size)| Request::SetGeometry { window, at, size }),
        1 => (index(), any::<bool>()).prop_map(|(window, on)| Request::Fullscreen { window, on }),
        1 => (index(), any::<bool>()).prop_map(|(window, on)| Request::Maximize { window, on }),
        1 => (index(), proptest::option::of(index()))
            .prop_map(|(window, parent)| Request::SetParent { window, parent }),
        3 => (proptest::option::weighted(0.95, target()), popup_placement, any::<bool>())
            .prop_map(|(parent, placement, grab)| Request::OpenPopup { parent, placement, grab }),
        1 => (index(), serial()).prop_map(|(popup, serial)| Request::Grab { popup, serial }),
        1 => (index(), placement(), any::<u32>())
            .prop_map(|(popup, placement, token)| Request::Reposition { popup, placement, token }),
        2 => (serial(), proptest::option::weighted(0.9, target()))
            .prop_map(|(serial, surface)| Request::AskToken { serial, surface }),
        2 => (token(), target()).prop_map(|(token, target)| Request::Activate { token, target }),
        1 => (
            index(),
            prop_oneof![4 => size_pair(), 1 => Just((-1, -1))],
            proptest::option::of((coordinate(), coordinate(), size_value(), size_value())),
        )
            .prop_map(|(window, destination, source)| Request::Viewport {
                window,
                destination,
                source
            }),
        1 => index().prop_map(|window| Request::FractionalScale { window }),
        1 => (any::<bool>(), serial())
            .prop_map(|(primary, serial)| Request::SetSelection { primary, serial }),
        1 => (target(), 0..3u8, proptest::option::of(target()))
            .prop_map(|(target, kind, parent)| Request::SecondRole { target, kind, parent }),
        2 => object().prop_map(Request::Destroy),
        1 => Just(Request::Disconnect),
    ]
}

/// A request the client that has the keyboard focus sends as a well-behaved client would: a
/// token for the surface its keyboard is on, with the serial of its last `enter`; an activation
/// of another of its windows with its newest token; a menu on the surface its keyboard is on, which
/// grabs the keyboard with that serial; a selection with that serial.
fn focused_request() -> impl Strategy<Value = Request> {
    prop_oneof![
        Just(Request::AskToken {
            serial: Serial::Real,
            surface: Some(Target::Focused),
        }),
        index().prop_map(|window| Request::Activate {
            token: Token::Newest,
            target: Target::OtherWindow(window),
        }),
        menu_placement().prop_map(|placement| Request::OpenPopup {
            parent: Some(Target::Focused),
            placement,
            grab: true,
        }),
        any::<bool>().prop_map(|primary| Request::SetSelection {
            primary,
            serial: Serial::Real,
        }),
    ]
}

fn key_binding() -> impl Strategy<Value = KeyBinding> {
    let direction = prop_oneof![
        Just(Direction::Left),
        Just(Direction::Right),
        Just(Direction::Up),
        Just(Direction::Down),
    ];
    prop_oneof![
        direction.prop_map(KeyBinding::Focus),
        (0..WORKSPACES.len()).prop_map(KeyBinding::FocusWorkspace),
        (0..WORKSPACES.len()).prop_map(KeyBinding::MoveToWorkspace),
        prop_oneof![Just(Tiling::Scrolling), Just(Tiling::Dwindle)].prop_map(KeyBinding::SetTiling),
    ]
}

/// A step: most often a client's request, any or one the client with the keyboard focus sends
/// as a well-behaved client would; else a move of the clock, a key binding, or a window focused.
fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        6 => (who(), request()).prop_map(|(who, request)| Step::Client { who, request }),
        2 => focused_request().prop_map(|request| Step::Client {
            who: Who::Focused,
            request,
        }),
        2 => (0..=2000u64).prop_map(Step::Advance),
        1 => key_binding().prop_map(Step::Layout),
        1 => index().prop_map(Step::FocusWindow),
    ]
}

fn steps() -> impl Strategy<Value = Vec<Step>> {
    prop::collection::vec(step(), 1..100)
}

/// `index` taken modulo `count`; none when there is nothing to name.
fn pick(index: usize, count: usize) -> Option<usize> {
    (count > 0).then(|| index % count)
}

/// The client's surface `target` names, if it has one of that kind; `keyboard_on` is the surface
/// its keyboard is on.
fn surface_of(client: &Client, keyboard_on: Option<&WlSurface>, target: Target) -> Option<Target> {
    match target {
        Target::Window(i) => pick(i, client.state.windows.len()).map(Target::Window),
        Target::Popup(i) => pick(i, client.popup_count()).map(Target::Popup),
        Target::Focused => {
            let focus = keyboard_on?;
            let window = (0..client.state.windows.len())
                .find(|&w| client.window(w).surface == *focus)
                .map(Target::Window);
            window.or_else(|| {
                (0..client.popup_count())
                    .find(|&p| client.popup(p).surface == *focus)
                    .map(Target::Popup)
            })
        }
        Target::OtherWindow(i) => {
            let others: Vec<usize> = (0..client.state.windows.len())
                .filter(|&w| Some(&client.window(w).surface) != keyboard_on)
                .collect();
            pick(i, others.len()).map(|o| Target::Window(others[o]))
        }
    }
}

/// The `wl_surface` of `target`, a window or a popup [`surface_of`] gave.
fn wl_surface(client: &Client, target: Target) -> Option<WlSurface> {
    match target {
        Target::Window(w) => Some(client.window(w).surface.clone()),
        Target::Popup(p) => Some(client.popup(p).surface.clone()),
        Target::Focused | Target::OtherWindow(_) => None,
    }
}

/// The parent a popup on `target` names: none when the client has no such surface.
fn popup_parent(
    client: &Client,
    keyboard_on: Option<&WlSurface>,
    target: Target,
) -> Option<Parent> {
    match surface_of(client, keyboard_on, target)? {
        Target::Window(w) => Some(Parent::Window(w)),
        Target::Popup(p) => Some(Parent::Popup(p)),
        Target::Focused | Target::OtherWindow(_) => None,
    }
}

impl RandomPlacement {
    fn placement(&self) -> Placement {
        Placement {
            size: self.size,
            anchor_rect: self.anchor_rect,
            anchor: Anchor::try_from(self.anchor).unwrap_or(Anchor::None),
            gravity: Gravity::try_from(self.gravity).unwrap_or(Gravity::None),
            adjustment: ConstraintAdjustment::from_bits_truncate(self.adjustment),
            offset: self.offset,
        }
    }
}

/// Sends `request`, without waiting for an answer, and says what it asked of what [`Reach`]
/// counts.
fn send(selection: &mut SelectionClient, request: &Request) -> Asked {
    let enter = selection.state.enters.last().copied();
    let keyboard_on = selection.state.keyboard_on.clone();
    let focus = keyboard_on.as_ref();
    let client = &mut selection.client;
    let windows = client.state.windows.len();
    let popups = client.popup_count();
    let window = |index: usize| pick(index, windows);
    let popup = |index: usize| pick(index, popups);
    match *request {
        Request::OpenToplevel => {
            let w = client.create_window();
            client.commit(w);
        }
        Request::Draw {
            target,
            size,
            frame,
        } => {
            let Some(target) = surface_of(client, focus, target) else {
                return Asked::Other;
            };
            let (asked, acked) = match target {
                Target::Window(w) => {
                    let window = client.window(w);
                    let asked = window
                        .configures
                        .last()
                        .map(|c| (c.serial, c.width, c.height));
                    (asked, window.acked)
                }
                Target::Popup(p) => {
                    let popup = client.popup(p);
                    let asked = popup
                        .configures
                        .last()
                        .map(|c| (c.serial, c.width, c.height));
                    (asked, popup.acked)
                }
                Target::Focused | Target::OtherWindow(_) => return Asked::Other,
            };
            // Before its first configure a well-behaved client draws nothing; it acknowledges
            // each configure once.
            let Some((serial, width, height)) = asked else {
                return Asked::Other;
            };
            let ack = acked != Some(serial);
            let asked = (width, height);
            let (width, height) = match size {
                Some(size) => size,
                None if asked.0 > 0 && asked.1 > 0 => asked,
                None => (640, 480),
            };
            let Some(surface) = wl_surface(client, target) else {
                return Asked::Other;
            };
            match target {
                Target::Window(w) => {
                    if ack {
                        client.ack(w);
                    }
                    if frame {
                        client.request_frame(w);
                    }
                }
                Target::Popup(p) => {
                    if ack {
                        client.ack_popup(p);
                    }
                    if frame {
                        client.request_popup_frame(p);
                    }
                }
                Target::Focused | Target::OtherWindow(_) => {}
            }
            // A size the configure asked may pass what `wl_shm` takes in a test's time.
            client.attach_black(&surface, width.min(2048), height.min(2048));
            surface.commit();
            if let Target::Popup(p) = target {
                return Asked::PopupCommit(p);
            }
        }
        Request::Ack { window: w, serial } => {
            let Some(w) = window(w) else {
                return Asked::Other;
            };
            match serial {
                Serial::Any(serial) => client.ack_serial(w, serial),
                Serial::Absent | Serial::Real if !client.window(w).configures.is_empty() => {
                    client.ack(w);
                }
                Serial::Absent | Serial::Real => {}
            }
        }
        Request::Commit {
            target,
            buffer,
            frame,
        } => {
            let Some(target) = surface_of(client, focus, target) else {
                return Asked::Other;
            };
            let Some(surface) = wl_surface(client, target) else {
                return Asked::Other;
            };
            match buffer {
                Buffer::New(width, height) => client.attach_black(&surface, width, height),
                Buffer::Remove => surface.attach(None, 0, 0),
                Buffer::Keep => {}
            }
            match (target, frame) {
                (Target::Window(w), true) => client.request_frame(w),
                (Target::Popup(p), true) => client.request_popup_frame(p),
                _ => {}
            }
            surface.commit();
            if let Target::Popup(p) = target {
                return Asked::PopupCommit(p);
            }
        }
        Request::SetMinSize { window: w, size } => {
            if let Some(w) = window(w) {
                client.set_min_size(w, size.0, size.1);
            }
        }
        Request::SetMaxSize { window: w, size } => {
            if let Some(w) = window(w) {
                client.set_max_size(w, size.0, size.1);
            }
        }
        Request::SetGeometry {
            window: w,
            at,
            size,
        } => {
            if let Some(w) = window(w) {
                client.set_window_geometry(w, at, size);
            }
        }
        Request::Fullscreen { window: w, on } => {
            if let Some(w) = window(w) {
                client.set_fullscreen(w, on);
            }
        }
        Request::Maximize { window: w, on } => match window(w) {
            Some(w) if on => client.set_maximized(w),
            Some(w) => client.unset_maximized(w),
            None => {}
        },
        Request::SetParent { window: w, parent } => {
            let Some(w) = window(w) else {
                return Asked::Other;
            };
            match parent.and_then(window) {
                Some(parent) => client.set_parent(w, parent),
                None => client.clear_parent(w),
            }
        }
        Request::OpenPopup {
            parent,
            placement,
            grab,
        } => {
            let parent = match parent {
                Some(target) => match popup_parent(client, focus, target) {
                    Some(parent) => parent,
                    // A popup on a surface the client does not have is not asked.
                    None => return Asked::Other,
                },
                None => Parent::Unset,
            };
            let p = client.create_popup(parent, placement.placement());
            let serial = enter.filter(|_| grab);
            if let Some(serial) = serial {
                client.grab_popup(p, serial);
            }
            client.commit_popup(p);
            if serial.is_some() {
                return Asked::Grab { committed: Some(p) };
            }
        }
        Request::Grab { popup: p, serial } => {
            let Some(p) = popup(p) else {
                return Asked::Other;
            };
            let serial = match serial {
                Serial::Any(serial) => Some(serial),
                Serial::Absent | Serial::Real => enter,
            };
            if let Some(serial) = serial {
                client.grab_popup(p, serial);
                return Asked::Grab { committed: None };
            }
        }
        Request::Reposition {
            popup: p,
            placement,
            token,
        } => {
            if let Some(p) = popup(p) {
                client.reposition_popup(p, placement.placement(), token);
            }
        }
        Request::AskToken { serial, surface } => {
            let serial = match serial {
                Serial::Absent => None,
                Serial::Real => enter,
                Serial::Any(serial) => Some(serial),
            };
            let surface = surface
                .and_then(|target| surface_of(client, focus, target))
                .and_then(|target| wl_surface(client, target));
            client.ask_token(serial, surface.as_ref());
            let valid =
                serial.is_some() && serial == enter && surface.is_some() && surface == keyboard_on;
            return Asked::Token { valid };
        }
        Request::Activate { token, target } => {
            let index = match token {
                Token::Newest => client.tokens_asked().checked_sub(1),
                Token::Any(index) => pick(index, client.tokens_asked()),
            };
            let token = index
                .and_then(|index| client.token(index))
                .map(str::to_owned);
            let surface =
                surface_of(client, focus, target).and_then(|target| wl_surface(client, target));
            if let (Some(token), Some(surface)) = (token, surface) {
                client.activate_surface(&token, &surface);
                return Asked::Activation;
            }
        }
        Request::Viewport {
            window: w,
            destination,
            source,
        } => {
            let Some(w) = window(w) else {
                return Asked::Other;
            };
            if client.window(w).viewport.is_none() {
                client.get_viewport(w);
            }
            client.set_viewport_destination(w, destination.0, destination.1);
            if let Some((x, y, width, height)) = source {
                client.set_viewport_source(
                    w,
                    (f64::from(x), f64::from(y)),
                    (f64::from(width), f64::from(height)),
                );
            }
        }
        Request::FractionalScale { window: w } => {
            if let Some(w) = window(w) {
                client.get_fractional_scale(w);
            }
        }
        Request::SetSelection { primary, serial } => {
            let serial = match serial {
                Serial::Any(serial) => serial,
                Serial::Absent | Serial::Real => enter.unwrap_or(0),
            };
            if primary {
                selection.set_primary(&[TEXT], serial);
            } else {
                selection.set_clipboard(&[TEXT], serial);
            }
            return Asked::Selection;
        }
        Request::SecondRole {
            target,
            kind,
            parent,
        } => {
            let parent = parent
                .and_then(|parent| popup_parent(client, focus, parent))
                .unwrap_or(Parent::Unset);
            match (surface_of(client, focus, target), kind) {
                (Some(Target::Window(w)), 0 | 1) => {
                    client.create_second_toplevel(w, kind == 0);
                }
                (Some(Target::Window(w)), _) => {
                    client.create_popup_on_window_surface(w, parent);
                }
                (Some(Target::Popup(p)), _) => {
                    client.create_popup_on_popup_surface(p, parent);
                }
                (Some(Target::Focused | Target::OtherWindow(_)) | None, _) => {}
            }
        }
        Request::Destroy(object) => destroy(selection, object),
        // The caller drops the client.
        Request::Disconnect => {}
    }
    Asked::Other
}

fn destroy(selection: &mut SelectionClient, object: Object) {
    let sources = selection.state.sources.len();
    let client = &mut selection.client;
    let windows = client.state.windows.len();
    let popups = client.popup_count();
    let (index, count) = match object {
        Object::Window(i)
        | Object::Decoration(i)
        | Object::Toplevel(i)
        | Object::XdgSurface(i)
        | Object::Surface(i)
        | Object::Viewport(i)
        | Object::FractionalScale(i) => (i, windows),
        Object::Popup(i)
        | Object::PopupRole(i)
        | Object::PopupXdgSurface(i)
        | Object::PopupSurface(i) => (i, popups),
        Object::Source(i) => (i, sources),
    };
    let Some(i) = pick(index, count) else { return };
    match object {
        Object::Window(_) => client.close(i),
        Object::Decoration(_) => client.destroy_decoration(i),
        Object::Toplevel(_) => client.destroy_toplevel(i),
        Object::XdgSurface(_) => client.destroy_xdg_surface(i),
        Object::Surface(_) => client.destroy_surface(i),
        Object::Viewport(_) => client.destroy_viewport(i),
        Object::FractionalScale(_) => client.destroy_fractional_scale(i),
        Object::Popup(_) => client.destroy_popup(i),
        Object::PopupRole(_) => client.destroy_popup_role(i),
        Object::PopupXdgSurface(_) => client.destroy_popup_xdg_surface(i),
        Object::PopupSurface(_) => client.destroy_popup_surface(i),
        Object::Source(_) => selection.destroy_source(i),
    }
}

/// Applies a key binding of the owner's seat; the layout may refuse it.
fn press(compositor: &mut Compositor, binding: KeyBinding) {
    compositor.change_layout(|layout| {
        let _ = match binding {
            KeyBinding::Focus(direction) => layout.focus(OWNER_SEAT, direction),
            KeyBinding::FocusWorkspace(name) => {
                layout.focus_workspace(OWNER_SEAT, WORKSPACES[name])
            }
            KeyBinding::MoveToWorkspace(name) => {
                layout.move_to_workspace(OWNER_SEAT, WORKSPACES[name])
            }
            KeyBinding::SetTiling(tiling) => layout.set_tiling(OWNER_SEAT, tiling),
        };
    });
}

/// Focuses the window of the output's arrangement that `index` names, taken modulo their count,
/// as a key binding would, and moves the clock past the next refresh, which gives it the
/// keyboard. Nothing happens when the output holds no window.
fn focus_window(compositor: &mut Compositor, start: Duration, index: usize) {
    let placed = compositor.layout().arrange(OUTPUT).unwrap_or_default();
    let Some(i) = pick(index, placed.len()) else {
        return;
    };
    let window = placed[i].window;
    // The layout may refuse it.
    let _ = compositor.change_layout(|layout| layout.focus_window(window));
    let next = next_frame_boundary(start, compositor.now());
    compositor
        .advance(next - compositor.now())
        .expect("the manual clock moves");
}

/// The test's compositor: manual clock, one small output.
fn compositor() -> Compositor {
    let mode = OutputMode::new(OUTPUT_SIZE.0, OUTPUT_SIZE.1, f64::from(RATE), SCALE)
        .expect("a valid mode");
    Compositor::new(Config {
        outputs: vec![mode],
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts")
}

/// Refresh boundary `k` of the output, created at `start`.
fn boundary(start: Duration, k: u64) -> Duration {
    start + Duration::from_nanos(k * 1_000_000_000 / u64::from(RATE))
}

/// The first refresh boundary at or after `time`.
fn first_boundary_at_or_after(start: Duration, time: Duration) -> u64 {
    let elapsed = time.saturating_sub(start).as_nanos();
    let mut k = u64::try_from(elapsed * u128::from(RATE) / 1_000_000_000).expect("a boundary");
    while boundary(start, k) < time {
        k += 1;
    }
    k
}

/// A boundary by which the refresh a change at `now` arms has run: the first after `now` plus
/// the lead. The refresh is due at once, or at a boundary minus the lead, the first after the
/// output's last refresh whose deadline is not past (ADR-0031); every boundary a refresh before
/// `now` aimed at is at most `now` plus the lead.
fn next_frame_boundary(start: Duration, now: Duration) -> Duration {
    let after = now + LEAD;
    let k = first_boundary_at_or_after(start, after);
    boundary(
        start,
        if boundary(start, k) == after {
            k + 1
        } else {
            k
        },
    )
}

/// The sentinel: a well-behaved client whose window, window 1, is drawn at boundary 1.
fn sentinel(compositor: &mut Compositor, start: Duration) -> Client {
    let mut sentinel = Client::connect(compositor);
    let w = sentinel.open_window(compositor);
    sentinel.ack(w);
    let asked = sentinel.window(w).configures[0].clone();
    let surface = sentinel.window(w).surface.clone();
    sentinel.attach_black(&surface, asked.width, asked.height);
    sentinel.request_frame(w);
    sentinel.commit(w);
    sentinel.roundtrip(compositor);
    compositor
        .advance(boundary(start, 1) - compositor.now())
        .expect("the manual clock moves");
    sentinel.roundtrip(compositor);
    assert_eq!(sentinel.window(w).frames.len(), 1);
    sentinel
}

/// A client in a busy state, from which short random tails reach deep states: a window and a
/// dialog of it, each drawn at the size it is asked, and a menu on the window, drawn.
fn busy_client(compositor: &mut Compositor) -> SelectionClient {
    let mut selection = SelectionClient::connect(compositor);
    let client = &mut selection.client;
    let window = client.create_window();
    client.commit(window);
    let dialog = client.create_window();
    client.set_parent(dialog, window);
    client.commit(dialog);
    selection.roundtrip(compositor);
    for target in [Target::Window(window), Target::Window(dialog)] {
        send(&mut selection, &draw(target));
    }
    selection.roundtrip(compositor);
    let menu = Placement::below_right((200, 100), (10, 10, 1, 1));
    let popup = selection.client.create_popup(Parent::Window(window), menu);
    selection.client.commit_popup(popup);
    selection.roundtrip(compositor);
    send(&mut selection, &draw(Target::Popup(popup)));
    selection.roundtrip(compositor);
    selection
}

/// A well-behaved frame of `target`, at the size it is asked, with a frame callback.
fn draw(target: Target) -> Request {
    Request::Draw {
        target,
        size: None,
        frame: true,
    }
}

/// Lets every client read what came and the compositor see who left; drops the clients whose
/// connection is over. The sentinel must stay.
fn settle(
    compositor: &mut Compositor,
    sentinel: &mut Client,
    clients: &mut [Option<SelectionClient>; CLIENTS],
) {
    for slot in clients.iter_mut() {
        if let Some(client) = slot
            && client.try_roundtrip(compositor).is_err()
        {
            *slot = None;
        }
    }
    sentinel.roundtrip(compositor);
}

/// The slot `who` names: for [`Who::Focused`], the client whose keyboard is on one of its
/// surfaces, or the first slot.
fn slot_of(who: Who, clients: &[Option<SelectionClient>; CLIENTS]) -> usize {
    match who {
        Who::Slot(slot) => slot,
        Who::Focused => clients
            .iter()
            .position(|client| {
                client
                    .as_ref()
                    .is_some_and(|c| c.state.keyboard_on.is_some())
            })
            .unwrap_or(0),
    }
}

/// How many times the client was told of a selection that holds something.
fn selections_told(client: &SelectionClient) -> usize {
    client
        .state
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                SelectionEvent::Clipboard(Some(_)) | SelectionEvent::Primary(Some(_))
            )
        })
        .count()
}

impl Before {
    fn read(client: &SelectionClient, compositor: &Compositor) -> Before {
        Before {
            keyboard_on: client.state.keyboard_on.clone(),
            selections_told: selections_told(client),
            focused: compositor.layout().focused(OWNER_SEAT),
        }
    }
}

impl Reach {
    /// Counts what a step's request asked, and whether it took effect: `client` is the client
    /// that sent it, after the step, if its connection lasted.
    fn count(
        &mut self,
        asked: Asked,
        before: &Before,
        client: Option<&SelectionClient>,
        compositor: &Compositor,
    ) {
        // Whether the keyboard went to popup `p` during the step: only a grab takes it there.
        let keyboard_went_to = |p: usize| {
            client.is_some_and(|client| {
                let on = client.state.keyboard_on.as_ref();
                p < client.client.popup_count()
                    && on == Some(&client.client.popup(p).surface)
                    && before.keyboard_on.as_ref() != on
            })
        };
        match asked {
            Asked::Other => {}
            Asked::Grab { committed } => {
                self.grabs.asked += 1;
                if committed.is_some_and(keyboard_went_to) {
                    self.grabs.took += 1;
                }
            }
            Asked::PopupCommit(p) => {
                if keyboard_went_to(p) {
                    self.grabs.took += 1;
                }
            }
            Asked::Token { valid } => {
                self.tokens.asked += 1;
                self.tokens.took += u64::from(valid);
            }
            Asked::Activation => {
                self.activations.asked += 1;
                let focused = compositor.layout().focused(OWNER_SEAT);
                if focused.is_some() && focused != before.focused {
                    self.activations.took += 1;
                }
            }
            Asked::Selection => {
                self.selections.asked += 1;
                if client.is_some_and(|client| selections_told(client) > before.selections_told) {
                    self.selections.took += 1;
                }
            }
        }
    }
}

impl fmt::Display for Tally {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let percent = if self.asked == 0 {
            0.0
        } else {
            100.0 * self.took as f64 / self.asked as f64
        };
        write!(f, "{}/{} ({percent:.0} %)", self.took, self.asked)
    }
}

impl fmt::Display for Reach {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "grabs granted {}, tokens valid {}, activations done {}, selections taken {}",
            self.grabs, self.tokens, self.activations, self.selections
        )
    }
}

/// Checks Yuna after a step: the layout's invariants, the windows, the popups and the keyboard.
fn check(compositor: &Compositor) -> Result<(), String> {
    compositor
        .layout()
        .check_invariants()
        .map_err(|error| format!("the layout's invariants: {error}"))?;
    compositor
        .check_windows()
        .map_err(|error| format!("the layout and the shell: {error}"))?;
    compositor
        .check_popups_and_keyboard()
        .map_err(|error| format!("the popups and the keyboard: {error}"))
}

/// Runs `steps`, checking after each, then checks the sentinel and rest, and adds to `reach`.
/// See the module's documentation.
fn run(busy: bool, steps: &[Step], reach: &mut Reach) -> Result<(), TestCaseError> {
    let mut compositor = compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut clients: [Option<SelectionClient>; CLIENTS] = Default::default();
    if busy {
        for slot in &mut clients {
            *slot = Some(busy_client(&mut compositor));
        }
        let next = next_frame_boundary(start, compositor.now());
        compositor
            .advance(next - compositor.now())
            .expect("the manual clock moves");
        settle(&mut compositor, &mut sentinel, &mut clients);
    }
    for (index, step) in steps.iter().enumerate() {
        let mut asked = None;
        match step {
            Step::Client { who, request } => {
                let slot = slot_of(*who, &clients);
                if matches!(request, Request::Disconnect) {
                    clients[slot] = None;
                } else {
                    let client = clients[slot]
                        .get_or_insert_with(|| SelectionClient::connect(&mut compositor));
                    let before = Before::read(client, &compositor);
                    asked = Some((slot, before, send(client, request)));
                }
            }
            Step::Advance(millis) => compositor
                .advance(Duration::from_millis(*millis))
                .expect("the manual clock moves"),
            Step::Layout(binding) => press(&mut compositor, *binding),
            Step::FocusWindow(window) => focus_window(&mut compositor, start, *window),
        }
        settle(&mut compositor, &mut sentinel, &mut clients);
        if let Some((slot, before, asked)) = asked {
            reach.count(asked, &before, clients[slot].as_ref(), &compositor);
        }
        if let Err(error) = check(&compositor) {
            return Err(TestCaseError::fail(format!(
                "step {index}, {step:?}: {error}"
            )));
        }
    }

    // A key binding focuses the sentinel's window, which shows it. Three periods later the
    // output is idle: the sentinel's frame callback, asked then, comes in the round that reads
    // it; asked again, at the next boundary's deadline exactly, not a nanosecond before
    // (ADR-0031).
    compositor
        .change_layout(|layout| layout.focus_window(WindowId(1)))
        .map_err(|error| TestCaseError::fail(format!("the sentinel cannot focus: {error}")))?;
    let quiet = compositor.now() + 3 * PERIOD;
    compositor
        .advance(quiet - compositor.now())
        .expect("the manual clock moves");
    settle(&mut compositor, &mut sentinel, &mut clients);
    let frames = sentinel.window(0).frames.len();
    sentinel.request_frame(0);
    sentinel.commit(0);
    sentinel.roundtrip(&mut compositor);
    prop_assert_eq!(
        &sentinel.window(0).frames[frames..],
        &[quiet.as_millis() as u32]
    );
    sentinel.request_frame(0);
    sentinel.commit(0);
    sentinel.roundtrip(&mut compositor);
    let aimed = first_boundary_at_or_after(start, quiet);
    let next = (aimed + 1).max(first_boundary_at_or_after(start, quiet + LEAD));
    let due = boundary(start, next) - LEAD;
    compositor
        .advance(due - Duration::from_nanos(1) - compositor.now())
        .expect("the manual clock moves");
    sentinel.roundtrip(&mut compositor);
    prop_assert_eq!(sentinel.window(0).frames.len(), frames + 1);
    compositor
        .advance(due - compositor.now())
        .expect("the manual clock moves");
    sentinel.roundtrip(&mut compositor);
    prop_assert_eq!(
        &sentinel.window(0).frames[frames + 1..],
        &[due.as_millis() as u32]
    );

    // Every client leaves: once the display has seen it and the next boundary has drawn it, Yuna
    // rests.
    drop(clients);
    sentinel.roundtrip(&mut compositor);
    drop(sentinel);
    compositor
        .dispatch(Some(Duration::ZERO))
        .expect("the compositor dispatches");
    run_armed_deadlines(&mut compositor);
    prop_assert_eq!(check(&compositor), Ok(()));
    prop_assert_eq!(
        compositor
            .layout()
            .arrange(OUTPUT)
            .map(|placed| placed.len()),
        Ok(0)
    );
    assert_rests(&mut compositor);
    Ok(())
}

/// Runs the cases of one of the two tests, those that start busy or not, then prints how far
/// they reached. Written out rather than with `proptest!`, so that the cases add to one
/// [`Reach`].
fn run_cases(busy: bool) {
    let mut config = contextualize_config(ProptestConfig {
        cases: CASES_PER_TEST,
        // Beside this file, as `proptest!` would set it.
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource(
            "proptest-regressions",
        ))),
        ..ProptestConfig::default()
    });
    config.source_file = Some(file!());
    let cases = config.cases;
    let reach = RefCell::new(Reach::default());
    let mut runner = TestRunner::new(config);
    let result = runner.run(&steps(), |steps| run(busy, &steps, &mut reach.borrow_mut()));
    if let Err(error) = result {
        panic!("{error}\n{runner}");
    }
    let start = if busy { "busy" } else { "fresh" };
    println!("{cases} cases from {start} starts: {}", reach.borrow());
}

#[test]
fn random_requests_from_busy_starts_keep_yuna_up() {
    run_cases(true);
}

#[test]
fn random_requests_from_fresh_starts_keep_yuna_up() {
    run_cases(false);
}

/// Sequences the random tests found failing, written out with whether they started busy: a seed
/// kept in the regressions file replays its case only while the generator stays the same.
#[test]
fn sequences_found_before() {
    let by_first = |request| Step::Client {
        who: Who::Slot(0),
        request,
    };
    let found = [
        // A toplevel again on an `xdg_surface` whose surface its client destroyed: Yuna kept a
        // record of it on the dead surface.
        (
            false,
            vec![
                by_first(Request::OpenToplevel),
                by_first(Request::Destroy(Object::Surface(0))),
                by_first(Request::SecondRole {
                    target: Target::Window(0),
                    kind: 0,
                    parent: None,
                }),
            ],
        ),
        // A window's surface destroyed before its menu: Yuna counted the menu, dismissed, as one
        // of a window's still.
        (true, vec![by_first(Request::Destroy(Object::Surface(2)))]),
        // A toplevel made again on a surface that kept its buffer: it has the buffer before its
        // first configure, which `check_windows` took for a window refused without reason.
        (
            false,
            vec![
                by_first(Request::OpenToplevel),
                by_first(draw(Target::Window(0))),
                by_first(Request::Destroy(Object::Window(0))),
                by_first(Request::SecondRole {
                    target: Target::Window(0),
                    kind: 1,
                    parent: None,
                }),
            ],
        ),
    ];
    for (busy, steps) in found {
        if let Err(error) = run(busy, &steps, &mut Reach::default()) {
            panic!("{error}");
        }
    }
}
