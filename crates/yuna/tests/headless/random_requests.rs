//! Random sequences of requests (ADR-0007, ADR-0025), with proptest: up to three clients send any
//! request in any order, with any values, extremes and nonsense included, while the test moves
//! the clock and changes the layout as key bindings would. A request that breaks the protocol
//! ends its client, which is allowed; its slot connects a new client at its next request.
//!
//! After every step: Yuna dispatches; the layout keeps its invariants; and the layout and the
//! shell agree on windows ([`Compositor::check_windows`]). After the sequence, a sentinel client
//! that joined first and stayed well-behaved, focused by a key binding, gets its frame callback
//! at the next refresh boundary; once every client has gone, Yuna rests.
//!
//! A failure shrinks to the shortest sequence that still fails; proptest keeps its seed in
//! `random_requests.proptest-regressions`, committed beside this file so that the case runs at
//! every check. The case count fits about 10 s of the check in a debug build
//! (`docs/ARCHITECTURE.md`); `PROPTEST_CASES=<n>` runs more.

use std::time::Duration;

use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use yuna::yuna_layout::{Direction, OutputId, Tiling, WindowId};
use yuna::{ClockMode, Compositor, Config, OWNER_SEAT, OutputMode};

use crate::assert_rests;
use crate::client::{Anchor, Client, ConstraintAdjustment, Gravity, Parent, Placement};
use crate::selection_client::SelectionClient;

/// The clients that send random requests, beside the sentinel.
const CLIENTS: usize = 3;
/// The output: 1280 × 720 pixels at 60 Hz and scale 1.5, small so that frames cost little.
const OUTPUT_SIZE: (u32, u32) = (1280, 720);
const RATE: u32 = 60;
const SCALE: f64 = 1.5;
const TEXT: &str = "text/plain;charset=utf-8";

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

/// A surface of the client: a window's or a popup's, by an index taken modulo their count.
#[derive(Debug, Clone, Copy)]
enum Target {
    Window(usize),
    Popup(usize),
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
    /// A popup on a window, on a popup, or on nothing; its initial commit.
    OpenPopup {
        parent: Option<Target>,
        placement: RandomPlacement,
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
        token: usize,
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
    /// A second role object for the surface of `target`, of the kind it has or not: a toplevel
    /// on its `xdg_surface` (0) or on a new one (1), or a popup (2). Allowed for a surface whose
    /// role object was destroyed, the client's protocol error otherwise.
    SecondRole {
        target: Target,
        kind: u8,
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
        client: usize,
        request: Request,
    },
    /// Moves the clock by this many milliseconds.
    Advance(u64),
    Layout(KeyBinding),
}

/// A client slot: the first most often, so that one client lives long enough to reach deep
/// states, the others less, so that clients meet.
fn client() -> impl Strategy<Value = usize> {
    prop_oneof![6 => Just(0), 3 => Just(1), 1 => Just(2)]
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
    ]
}

fn buffer() -> impl Strategy<Value = Buffer> {
    prop_oneof![
        4 => (1..=2048i32, 1..=2048i32).prop_map(|(w, h)| Buffer::New(w, h)),
        1 => Just(Buffer::Remove),
        1 => Just(Buffer::Keep),
    ]
}

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
        2 => (proptest::option::weighted(0.95, target()), placement())
            .prop_map(|(parent, placement)| Request::OpenPopup { parent, placement }),
        1 => (index(), serial()).prop_map(|(popup, serial)| Request::Grab { popup, serial }),
        1 => (index(), placement(), any::<u32>())
            .prop_map(|(popup, placement, token)| Request::Reposition { popup, placement, token }),
        1 => (serial(), proptest::option::of(target()))
            .prop_map(|(serial, surface)| Request::AskToken { serial, surface }),
        1 => (index(), target()).prop_map(|(token, target)| Request::Activate { token, target }),
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
        1 => (target(), 0..3u8).prop_map(|(target, kind)| Request::SecondRole { target, kind }),
        2 => object().prop_map(Request::Destroy),
        1 => Just(Request::Disconnect),
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

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        8 => (client(), request()).prop_map(|(client, request)| Step::Client { client, request }),
        2 => (0..=2000u64).prop_map(Step::Advance),
        1 => key_binding().prop_map(Step::Layout),
    ]
}

/// `index` taken modulo `count`; none when there is nothing to name.
fn pick(index: usize, count: usize) -> Option<usize> {
    (count > 0).then(|| index % count)
}

/// The client's surface `target` names, if it has one of that kind.
fn surface_of(client: &Client, target: Target) -> Option<Target> {
    match target {
        Target::Window(i) => pick(i, client.state.windows.len()).map(Target::Window),
        Target::Popup(i) => pick(i, client.popup_count()).map(Target::Popup),
    }
}

fn wl_surface(client: &Client, target: Target) -> wayland_client::protocol::wl_surface::WlSurface {
    match target {
        Target::Window(w) => client.window(w).surface.clone(),
        Target::Popup(p) => client.popup(p).surface.clone(),
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

/// Sends `request`, without waiting for an answer.
fn send(selection: &mut SelectionClient, request: &Request) {
    let enter = selection.state.enters.last().copied();
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
            let Some(target) = surface_of(client, target) else {
                return;
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
            };
            // Before its first configure a well-behaved client draws nothing; it acknowledges
            // each configure once.
            let Some((serial, width, height)) = asked else {
                return;
            };
            let ack = acked != Some(serial);
            let asked = (width, height);
            let (width, height) = match size {
                Some(size) => size,
                None if asked.0 > 0 && asked.1 > 0 => asked,
                None => (640, 480),
            };
            let surface = wl_surface(client, target);
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
            }
            // A size the configure asked may pass what `wl_shm` takes in a test's time.
            client.attach_black(&surface, width.min(2048), height.min(2048));
            surface.commit();
        }
        Request::Ack { window: w, serial } => {
            let Some(w) = window(w) else { return };
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
            let Some(target) = surface_of(client, target) else {
                return;
            };
            let surface = wl_surface(client, target);
            match buffer {
                Buffer::New(width, height) => client.attach_black(&surface, width, height),
                Buffer::Remove => surface.attach(None, 0, 0),
                Buffer::Keep => {}
            }
            match (target, frame) {
                (Target::Window(w), true) => client.request_frame(w),
                (Target::Popup(p), true) => client.request_popup_frame(p),
                (_, false) => {}
            }
            surface.commit();
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
            let Some(w) = window(w) else { return };
            match parent.and_then(window) {
                Some(parent) => client.set_parent(w, parent),
                None => client.clear_parent(w),
            }
        }
        Request::OpenPopup { parent, placement } => {
            let parent = match parent.map(|parent| surface_of(client, parent)) {
                Some(Some(Target::Window(w))) => Parent::Window(w),
                Some(Some(Target::Popup(p))) => Parent::Popup(p),
                // A popup on a surface the client does not have is not asked.
                Some(None) => return,
                None => Parent::Unset,
            };
            let p = client.create_popup(parent, placement.placement());
            client.commit_popup(p);
        }
        Request::Grab { popup: p, serial } => {
            let Some(p) = popup(p) else { return };
            let serial = match serial {
                Serial::Any(serial) => Some(serial),
                Serial::Absent | Serial::Real => enter,
            };
            if let Some(serial) = serial {
                client.grab_popup(p, serial);
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
                .and_then(|target| surface_of(client, target))
                .map(|target| wl_surface(client, target));
            client.ask_token(serial, surface.as_ref());
        }
        Request::Activate { token, target } => {
            let token = pick(token, client.tokens_asked())
                .and_then(|index| client.token(index))
                .map(str::to_owned);
            let surface = surface_of(client, target).map(|target| wl_surface(client, target));
            if let (Some(token), Some(surface)) = (token, surface) {
                client.activate_surface(&token, &surface);
            }
        }
        Request::Viewport {
            window: w,
            destination,
            source,
        } => {
            let Some(w) = window(w) else { return };
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
        }
        Request::SecondRole { target, kind } => match (surface_of(client, target), kind) {
            (Some(Target::Window(w)), 0 | 1) => {
                client.create_second_toplevel(w, kind == 0);
            }
            (Some(Target::Window(w)), _) => {
                client.create_popup_on_window_surface(w, Parent::Window(w));
            }
            (Some(Target::Popup(p)), _) => {
                client.create_popup_on_popup_surface(p, Parent::Unset);
            }
            (None, _) => {}
        },
        Request::Destroy(object) => destroy(selection, object),
        // The caller drops the client.
        Request::Disconnect => {}
    }
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

/// The first refresh boundary after `now`.
fn next_boundary(start: Duration, now: Duration) -> Duration {
    let elapsed = now.saturating_sub(start).as_nanos();
    let mut k = u64::try_from(elapsed * u128::from(RATE) / 1_000_000_000).expect("a boundary");
    while boundary(start, k) <= now {
        k += 1;
    }
    boundary(start, k)
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

/// Runs `steps`, checking after each, then checks the sentinel and rest. See the module's
/// documentation.
fn run(busy: bool, steps: &[Step]) -> Result<(), TestCaseError> {
    let mut compositor = compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut clients: [Option<SelectionClient>; CLIENTS] = Default::default();
    if busy {
        for slot in &mut clients {
            *slot = Some(busy_client(&mut compositor));
        }
        let next = next_boundary(start, compositor.now());
        compositor
            .advance(next - compositor.now())
            .expect("the manual clock moves");
        settle(&mut compositor, &mut sentinel, &mut clients);
    }
    for (index, step) in steps.iter().enumerate() {
        match step {
            Step::Client { client, request } => {
                let slot = &mut clients[*client];
                if matches!(request, Request::Disconnect) {
                    *slot = None;
                } else {
                    let client =
                        slot.get_or_insert_with(|| SelectionClient::connect(&mut compositor));
                    send(client, request);
                }
            }
            Step::Advance(millis) => compositor
                .advance(Duration::from_millis(*millis))
                .expect("the manual clock moves"),
            Step::Layout(binding) => press(&mut compositor, *binding),
        }
        settle(&mut compositor, &mut sentinel, &mut clients);
        if let Err(error) = compositor.layout().check_invariants() {
            return Err(TestCaseError::fail(format!(
                "step {index}, {step:?}: the layout's invariants: {error}"
            )));
        }
        if let Err(error) = compositor.check_windows() {
            return Err(TestCaseError::fail(format!(
                "step {index}, {step:?}: the layout and the shell: {error}"
            )));
        }
    }

    // A key binding focuses the sentinel's window, which shows it; its next frame callback comes
    // at the next boundary, not a nanosecond before.
    compositor
        .change_layout(|layout| layout.focus_window(WindowId(1)))
        .map_err(|error| TestCaseError::fail(format!("the sentinel cannot focus: {error}")))?;
    let next = next_boundary(start, compositor.now());
    compositor
        .advance(next - compositor.now())
        .expect("the manual clock moves");
    settle(&mut compositor, &mut sentinel, &mut clients);
    let frames = sentinel.window(0).frames.len();
    sentinel.request_frame(0);
    sentinel.commit(0);
    sentinel.roundtrip(&mut compositor);
    let next = next_boundary(start, compositor.now());
    compositor
        .advance(next - Duration::from_nanos(1) - compositor.now())
        .expect("the manual clock moves");
    sentinel.roundtrip(&mut compositor);
    prop_assert_eq!(sentinel.window(0).frames.len(), frames);
    compositor
        .advance(next - compositor.now())
        .expect("the manual clock moves");
    sentinel.roundtrip(&mut compositor);
    prop_assert_eq!(
        &sentinel.window(0).frames[frames..],
        &[next.as_millis() as u32]
    );

    // Every client leaves: once the display has seen it and the next boundary has drawn it, Yuna
    // rests.
    drop(clients);
    sentinel.roundtrip(&mut compositor);
    drop(sentinel);
    compositor
        .dispatch(Some(Duration::ZERO))
        .expect("the compositor dispatches");
    let next = next_boundary(start, compositor.now());
    compositor
        .advance(next - compositor.now())
        .expect("the manual clock moves");
    prop_assert_eq!(compositor.check_windows(), Ok(()));
    prop_assert_eq!(
        compositor
            .layout()
            .arrange(OutputId(1))
            .map(|placed| placed.len()),
        Ok(0)
    );
    assert_rests(&mut compositor);
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 160,
        // Beside this file: the default would look for the folder of `main.rs`, and write
        // elsewhere.
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource(
            "proptest-regressions"
        ))),
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_requests_keep_yuna_up(
        busy in any::<bool>(),
        steps in prop::collection::vec(step(), 1..100),
    ) {
        run(busy, &steps)?;
    }
}
