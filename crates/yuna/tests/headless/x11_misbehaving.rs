//! Hostile and broken X11 clients (ADR-0080, ADR-0028,
//! [ADR-0066](../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md)): huge and
//! malformed properties, messages naming atoms never interned, nonsense sizes, cycles of
//! parents, floods of windows, windows gone before they pair, a client that dies in the middle of
//! a request, menus far off every output, and random sequences of all of them. None takes Yuna
//! down or stalls it.
//!
//! Each story runs the development shell's Xwayland for one agent's seat, the hostile client's,
//! and a Wayland program, the sentinel, on another agent's seat. Each ends the same way
//! ([`Hostile::end`]): Yuna's state is sound, the sentinel's window is where it was and gets its
//! frame callbacks, and once the hostile client has gone and its windows with it, Yuna rests.
//! They run on the real clock, as Xwayland does.

use std::cell::RefCell;
use std::io::Write as _;
use std::net::Shutdown;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use proptest::test_runner::{FileFailurePersistence, TestRunner, contextualize_config};
use smithay::reexports::rustix::time::{ClockId, clock_gettime};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageData, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _,
    DESTROY_NOTIFY_EVENT, DestroyNotifyEvent, EventMask, KEYMAP_NOTIFY_EVENT, PropMode, StackMode,
    UNMAP_NOTIFY_EVENT, UnmapNotifyEvent, Window,
};
use x11rb::wrapper::ConnectionExt as _;
use yuna::yuna_layout::geometry::Rect;
use yuna::yuna_layout::{OutputId, SeatId, WindowId};
use yuna::{
    ClockMode, Compositor, Config, Counters, MAX_MENUS_PER_CLIENT, MAX_WINDOWS_PER_CLIENT,
    OutputMode, X11Display, XwaylandConfig,
};

use crate::client::Client;
use crate::x11_client::{Image, PATIENCE, WindowSpec, X11Client, connect, step, step_pumping};
use crate::x11_windows::{Folder, at_1, at_1_5, pump, wait, window_titled};

/// A hostile client's seat beside a sentinel's.
struct Hostile {
    compositor: Compositor,
    /// The hostile client's seat, which has an X display.
    seat: SeatId,
    output: OutputId,
    display: X11Display,
    sentinel: Client,
    sentinel_window: usize,
    sentinel_id: WindowId,
    /// The sentinel's place, output and focus, as they must stay.
    sentinel_seen: (Option<Rect>, Option<OutputId>, bool),
    _folder: Folder,
}

impl Hostile {
    /// A compositor whose first agent seat, of an output of `mode`, has an X display, and whose
    /// second has the sentinel's window drawn.
    fn new(story: &str, mode: OutputMode) -> Hostile {
        let folder = Folder::new(story);
        let mut compositor = Compositor::new(Config {
            outputs: Vec::new(),
            clock: ClockMode::Real,
            listen: false,
            xwayland: Some(XwaylandConfig::default()),
            ..Config::default()
        })
        .expect("the compositor starts");
        let seat = compositor.add_seat().expect("a seat");
        let output = compositor.add_output(seat, mode).expect("an output");
        compositor
            .listen_for_seat(seat, folder.0.join("hostile"))
            .expect("the seat listens");
        let display = compositor.x11_display(seat).expect("the seat's display");
        let other = compositor.add_seat().expect("another seat");
        compositor
            .add_output(other, at_1())
            .expect("the other seat's output");
        let mut sentinel = Client::connect_to(&mut compositor, other);
        let sentinel_window = sentinel.open_window(&mut compositor);
        sentinel.ack(sentinel_window);
        let configure = sentinel
            .window(sentinel_window)
            .configures
            .last()
            .cloned()
            .expect("a first configure");
        sentinel.attach(
            sentinel_window,
            configure.width,
            configure.height,
            0xFF_30_90_30,
        );
        sentinel.commit(sentinel_window);
        sentinel.roundtrip(&mut compositor);
        let described = wait(&mut compositor, "the sentinel opens", |compositor| {
            compositor
                .windows()
                .into_iter()
                .find(|window| window.seat == other && window.rect.is_some())
        });
        Hostile {
            compositor,
            seat,
            output,
            display,
            sentinel,
            sentinel_window,
            sentinel_id: described.id,
            sentinel_seen: (described.place, described.output, described.focused),
            _folder: folder,
        }
    }

    fn client(&mut self) -> X11Client {
        connect(&mut self.compositor, &self.display)
    }

    /// Runs the compositor's loop for `duration`.
    fn pump_for(&mut self, duration: Duration) {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            pump(&mut self.compositor);
        }
    }

    /// The windows of the hostile client's seat.
    fn hostile_windows(&self) -> usize {
        self.compositor
            .windows()
            .iter()
            .filter(|window| window.seat == self.seat)
            .count()
    }

    /// Yuna's state is sound, and the sentinel is untouched: its place, its output and its
    /// focus are as they were, and it gets the frame callback it asks.
    fn others_untouched(&mut self) {
        assert_sound(&self.compositor);
        let described = self
            .compositor
            .windows()
            .into_iter()
            .find(|window| window.id == self.sentinel_id)
            .expect("the sentinel's window stays");
        assert_eq!(
            (described.place, described.output, described.focused),
            self.sentinel_seen,
            "the sentinel's window is untouched"
        );
        let w = self.sentinel_window;
        let frames = self.sentinel.window(w).frames.len();
        self.sentinel.request_frame(w);
        self.sentinel.commit(w);
        self.sentinel.send();
        let deadline = Instant::now() + PATIENCE;
        while self.sentinel.window(w).frames.len() == frames {
            assert!(
                Instant::now() < deadline,
                "the sentinel got no frame callback"
            );
            pump(&mut self.compositor);
            self.sentinel.read_events();
        }
    }

    /// The end of every story: the others go on; then the hostile clients leave, their windows
    /// close, their menus go, and Yuna rests.
    fn end(mut self, clients: Vec<X11Client>) {
        self.others_untouched();
        drop(clients);
        let (seat, output) = (self.seat, self.output);
        wait(
            &mut self.compositor,
            "the hostile windows close",
            |compositor| {
                let windows = compositor
                    .windows()
                    .iter()
                    .filter(|window| window.seat == seat)
                    .count();
                let menus = compositor.drawn_x11_menus(output).len();
                (windows == 0 && menus == 0).then_some(())
            },
        );
        self.others_untouched();
        assert_rests_real(&mut self.compositor);
    }
}

/// The layout keeps its invariants, agrees with the shells, and the popups and the keyboard
/// are sound.
fn assert_sound(compositor: &Compositor) {
    compositor
        .layout()
        .check_invariants()
        .expect("the layout keeps its invariants");
    compositor
        .check_windows()
        .expect("the layout and the shells agree");
    compositor
        .check_popups_and_keyboard()
        .expect("the popups and the keyboard are sound");
}

/// What changed in the counters, wake-ups included.
fn activity(before: Counters, after: Counters) -> (u64, u64, u64, u64, u64) {
    (
        after.frames_drawn - before.frames_drawn,
        after.frame_callbacks - before.frame_callbacks,
        after.configures - before.configures,
        after.commits - before.commits,
        after.wakeups - before.wakeups,
    )
}

/// On the real clock: once what is due has run, half a second of the loop draws, calls back,
/// configures, commits and wakes nothing, and leaves no deadline armed.
fn assert_rests_real(compositor: &mut Compositor) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        assert!(Instant::now() < deadline, "Yuna never rests");
        let before = compositor.counters();
        let quiet = Instant::now() + Duration::from_millis(300);
        while Instant::now() < quiet {
            compositor
                .dispatch(Some(Duration::from_millis(5)))
                .expect("Yuna dispatches");
        }
        if compositor.next_deadline().is_none() && compositor.counters() == before {
            break;
        }
    }
    let before = compositor.counters();
    let quiet = Instant::now() + Duration::from_millis(500);
    while Instant::now() < quiet {
        compositor
            .dispatch(Some(Duration::from_millis(50)))
            .expect("Yuna dispatches");
    }
    assert_eq!(activity(before, compositor.counters()), (0, 0, 0, 0, 0));
    assert_eq!(compositor.next_deadline(), None);
}

/// An atom no client interned: X11 numbers its atoms from 1, a few thousand of them.
const NEVER_INTERNED: u32 = 0x7FFF_0000;

#[test]
fn a_title_of_16_mib_is_read_to_its_bound() {
    let mut hostile = Hostile::new("x11-huge-title", at_1_5());
    let client = hostile.client();
    let (client, ()) = step(&mut hostile.compositor, client, |client| {
        let window = client.create(&WindowSpec::named("huge"))?;
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        client
            .connection
            .delete_property(window, client.atoms._NET_WM_NAME)
            .map_err(fail)?;
        // 16 MiB of Latin-1, appended in pieces that fit any request.
        let piece = vec![b'a'; 64 * 1024];
        client
            .connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                &[],
            )
            .map_err(fail)?;
        for _ in 0..256 {
            client
                .connection
                .change_property8(
                    PropMode::APPEND,
                    window,
                    AtomEnum::WM_NAME,
                    AtomEnum::STRING,
                    &piece,
                )
                .map_err(fail)?;
        }
        client.sync()?;
        client.connection.map_window(window).map_err(fail)?;
        client.connection.flush().map_err(fail)?;
        client.wait_for(
            "the window mapped",
            |event| matches!(event, Event::MapNotify(mapped) if mapped.window == window),
        )?;
        let geometry = client.geometry(window)?;
        client.draw(window, &Image::sharp(geometry.width, geometry.height))
    });
    let described = wait(&mut hostile.compositor, "the window opens", |compositor| {
        compositor
            .windows()
            .into_iter()
            .find(|window| window.app_id.as_deref() == Some("X11Test") && window.rect.is_some())
    });
    let title = described.title.expect("a title");
    assert_eq!(title.len(), 64 * 1024, "the title is cut at its bound");
    assert!(title.bytes().all(|byte| byte == b'a'));
    hostile.end(vec![client]);
}

#[test]
fn titles_in_invalid_utf8_and_compound_text_are_ignored_or_read() {
    let mut hostile = Hostile::new("x11-encodings", at_1_5());
    let client = hostile.client();
    let (client, window, _) =
        open_window(&mut hostile.compositor, client, &WindowSpec::named("valid"));
    // Its `_NET_WM_NAME` becomes invalid UTF-8, ignored as if it were not set: its title is
    // its `WM_NAME` then, in COMPOUND_TEXT, read as Latin-1.
    let (client, ()) = step(&mut hostile.compositor, client, move |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let compound = client
            .connection
            .intern_atom(false, b"COMPOUND_TEXT")
            .map_err(fail)?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        client
            .connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                compound,
                b"compound \xE9",
            )
            .map_err(fail)?;
        client
            .connection
            .change_property8(
                PropMode::REPLACE,
                window,
                client.atoms._NET_WM_NAME,
                client.atoms.UTF8_STRING,
                b"bad \xFF\xFE utf-8 \xC3",
            )
            .map_err(fail)?;
        client.sync()
    });
    window_titled(&mut hostile.compositor, "compound é");
    hostile.end(vec![client]);
}

/// Sends `client`'s message of `type_` and `data` to `window`, as `SendEvent` lets any client.
fn send_message(
    client: &X11Client,
    to: Window,
    window: Window,
    type_: u32,
    format: u8,
    data: [u32; 5],
) -> Result<(), String> {
    let data = match format {
        8 => {
            let mut bytes = [0_u8; 20];
            for (at, byte) in bytes.iter_mut().enumerate() {
                *byte = data[at % 5] as u8;
            }
            ClientMessageData::from(bytes)
        }
        16 => ClientMessageData::from([
            data[0] as u16,
            data[1] as u16,
            data[2] as u16,
            data[3] as u16,
            data[4] as u16,
            0,
            0,
            0,
            0,
            0,
        ]),
        _ => ClientMessageData::from(data),
    };
    let message = ClientMessageEvent {
        response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
        format,
        sequence: 0,
        window,
        type_,
        data,
    };
    client
        .connection
        .send_event(
            false,
            to,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            message,
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
fn messages_and_states_naming_atoms_never_interned_change_nothing() {
    let mut hostile = Hostile::new("x11-atoms", at_1_5());
    let client = hostile.client();
    let spec = WindowSpec::named("atoms");
    let (client, window, _) = open_window(&mut hostile.compositor, client, &spec);
    let (client, ()) = step(&mut hostile.compositor, client, move |client| {
        let atoms = client.atoms;
        let root = client.root;
        let unknown = |n: u32| NEVER_INTERNED + n;
        // A message of an unknown type, to the root and to the window, in each format.
        for format in [8, 16, 32] {
            for to in [root, window] {
                send_message(
                    client,
                    to,
                    window,
                    unknown(format.into()),
                    format,
                    [1, 2, 3, 4, 5],
                )?;
            }
        }
        // `_NET_WM_STATE` naming atoms never interned, with every action, and none known.
        for action in 0..4 {
            send_message(
                client,
                root,
                window,
                atoms._NET_WM_STATE,
                32,
                [action, unknown(10), unknown(11), 1, 0],
            )?;
        }
        // `_NET_ACTIVE_WINDOW` and `WM_PROTOCOLS` naming windows and atoms that are none.
        send_message(
            client,
            root,
            0xDEAD,
            atoms._NET_ACTIVE_WINDOW,
            32,
            [1, 0, 0, 0, 0],
        )?;
        send_message(
            client,
            root,
            root,
            atoms.WM_PROTOCOLS,
            32,
            [atoms._NET_WM_PING, 0, unknown(12), 0, 0],
        )?;
        send_message(
            client,
            root,
            window,
            atoms.WM_PROTOCOLS,
            32,
            [unknown(13), 0, 0, 0, 0],
        )?;
        // Its `_NET_WM_STATE` and window type list atoms never interned.
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        for property in [
            atoms._NET_WM_STATE,
            atoms._NET_WM_WINDOW_TYPE,
            atoms.WM_PROTOCOLS,
        ] {
            client
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    property,
                    AtomEnum::ATOM,
                    &[unknown(20), unknown(21), atoms._NET_WM_STATE_FULLSCREEN, 0],
                )
                .map_err(fail)?;
        }
        // A property of a type never interned.
        client
            .connection
            .change_property32(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                unknown(30),
                &[1, 2, 3],
            )
            .map_err(fail)?;
        client.sync()
    });
    hostile.pump_for(Duration::from_millis(300));
    let listed = hostile.compositor.windows();
    let window = listed
        .iter()
        .find(|listed| listed.seat == hostile.seat)
        .expect("the window stays");
    assert!(!window.fullscreen, "no message made it fullscreen");
    hostile.end(vec![client]);
}

/// Every core event type, its bytes made up, sent to the root through `SendEvent`, as any X11
/// client of the seat may: the manager, which the root's redirection makes receive them, takes
/// each as nothing it trusts, and goes on serving, the same Xwayland, a window opened after them
/// shown as any.
#[test]
fn made_up_events_of_every_type_leave_the_manager_serving() {
    let mut hostile = Hostile::new("x11-made-up-events", at_1_5());
    let client = hostile.client();
    let spec = WindowSpec::named("target");
    let (client, target, _) = open_window(&mut hostile.compositor, client, &spec);
    let server = hostile
        .compositor
        .x11_display(hostile.seat)
        .and_then(|display| display.server);
    let (client, ()) = step(&mut hostile.compositor, client, move |client| {
        let root = client.root;
        // A linear congruential sequence: the same made-up bytes at each run.
        let mut state: u32 = 0x1234_5678;
        let mut next = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        };
        // Every core event but `KeymapNotify`, which x11rb 0.14 misreads once sent (debt.md).
        for event_type in (2..=34_u8).filter(|&event_type| event_type != KEYMAP_NOTIFY_EVENT) {
            for _ in 0..4 {
                let mut bytes = [0_u8; 32];
                bytes.iter_mut().for_each(|byte| *byte = next());
                bytes[0] = event_type;
                client
                    .connection
                    .send_event(
                        false,
                        root,
                        EventMask::SUBSTRUCTURE_REDIRECT
                            | EventMask::SUBSTRUCTURE_NOTIFY
                            | EventMask::STRUCTURE_NOTIFY
                            | EventMask::PROPERTY_CHANGE,
                        bytes,
                    )
                    .map_err(|e| e.to_string())?;
            }
        }
        // Made up to name the program's own window, as if it were gone.
        let redirect = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        let destroyed = DestroyNotifyEvent {
            response_type: DESTROY_NOTIFY_EVENT,
            sequence: 0,
            event: root,
            window: target,
        };
        let unmapped = UnmapNotifyEvent {
            response_type: UNMAP_NOTIFY_EVENT,
            sequence: 0,
            event: root,
            window: target,
            from_configure: false,
        };
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        client
            .connection
            .send_event(false, root, redirect, unmapped)
            .map_err(fail)?;
        client
            .connection
            .send_event(false, root, redirect, destroyed)
            .map_err(fail)?;
        client.sync()
    });
    hostile.pump_for(Duration::from_millis(300));
    assert!(
        hostile
            .compositor
            .windows()
            .iter()
            .any(|window| window.seat == hostile.seat && window.title.as_deref() == Some("target")),
        "a made-up DestroyNotify closed the window"
    );
    let (client, _, _) = open_window(&mut hostile.compositor, client, &WindowSpec::named("after"));
    assert_eq!(
        hostile
            .compositor
            .x11_display(hostile.seat)
            .and_then(|display| display.server),
        server,
        "the seat's Xwayland is the same"
    );
    hostile.end(vec![client]);
}

/// Opens a window of `spec`, draws into it, and waits until Yuna shows it.
fn open_window(
    compositor: &mut Compositor,
    client: X11Client,
    spec: &WindowSpec,
) -> (X11Client, Window, WindowId) {
    let (client, window, described, _) = crate::x11_windows::open_drawn(compositor, client, spec);
    (client, window, described.id)
}

/// A `WM_SIZE_HINTS` of 18 values whose minimum, maximum, increments and base are `values`,
/// all flagged.
fn size_hints(values: [i32; 8]) -> Vec<u32> {
    let mut hints = vec![0_u32; 18];
    hints[0] = (1 << 4) | (1 << 5) | (1 << 6) | (1 << 8);
    for (at, value) in [5, 6, 7, 8, 9, 10, 15, 16].into_iter().zip(values) {
        hints[at] = value as u32;
    }
    hints
}

#[test]
fn size_hints_negative_and_huge_still_open() {
    let mut hostile = Hostile::new("x11-hints", at_1_5());
    let client = hostile.client();
    let nonsense = [
        [-5, -5, -1, -1, -3, -3, -7, -7],
        [
            i32::MAX,
            i32::MAX,
            i32::MAX,
            i32::MAX,
            i32::MAX,
            i32::MAX,
            i32::MAX,
            i32::MAX,
        ],
        [i32::MIN, 10, 0, 0, 0, 0, i32::MIN, 3],
        [40_000, 40_000, 10, 10, 0, -1, 0, 0],
        [0, 0, 1, 1, 0, 0, 0, 0],
    ];
    let (client, ()) = step(&mut hostile.compositor, client, move |client| {
        for (index, values) in nonsense.into_iter().enumerate() {
            let spec = WindowSpec {
                background: Some(0x0060_6060),
                ..WindowSpec::named(&format!("hints-{index}"))
            };
            let window = client.create(&spec)?;
            client
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    AtomEnum::WM_NORMAL_HINTS,
                    AtomEnum::WM_SIZE_HINTS,
                    &size_hints(values),
                )
                .map_err(|e| e.to_string())?;
            client
                .connection
                .map_window(window)
                .map_err(|e| e.to_string())?;
        }
        client.sync()
    });
    // Malformed hints are ignored, a maximum below the minimum dropped: those windows open. A
    // minimum of 32,767 X11 pixels is honoured, and Xwayland draws no buffer that large: those
    // never show, and Yuna goes on.
    for title in ["hints-0", "hints-2", "hints-4"] {
        window_titled(&mut hostile.compositor, title);
    }
    hostile.pump_for(Duration::from_millis(500));
    assert_sound(&hostile.compositor);
    hostile.end(vec![client]);
}

#[test]
fn windows_transient_for_themselves_or_each_other_open() {
    let mut hostile = Hostile::new("x11-cycles", at_1_5());
    let client = hostile.client();
    let (client, ()) = step(&mut hostile.compositor, client, |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let itself = client.create(&WindowSpec::named("itself"))?;
        let one = client.create(&WindowSpec::named("one"))?;
        let other = client.create(&WindowSpec::named("other"))?;
        let menu = client.create(&WindowSpec::menu("menu-itself", 30, 30, 100, 100))?;
        for (window, parent) in [(itself, itself), (one, other), (other, one), (menu, menu)] {
            client
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    AtomEnum::WM_TRANSIENT_FOR,
                    AtomEnum::WINDOW,
                    &[parent],
                )
                .map_err(fail)?;
        }
        for window in [itself, one, other, menu] {
            client.connection.map_window(window).map_err(fail)?;
        }
        client.connection.flush().map_err(fail)?;
        for window in [itself, one, other, menu] {
            client.wait_for(
                "the window mapped",
                |event| matches!(event, Event::MapNotify(mapped) if mapped.window == window),
            )?;
            let geometry = client.geometry(window)?;
            client.draw(window, &Image::sharp(geometry.width, geometry.height))?;
        }
        Ok(())
    });
    for title in ["itself", "one", "other"] {
        window_titled(&mut hostile.compositor, title);
    }
    let output = hostile.output;
    wait(&mut hostile.compositor, "the menu is drawn", |compositor| {
        compositor.drawn_x11_menus(output).into_iter().next()
    });
    assert_sound(&hostile.compositor);
    hostile.end(vec![client]);
}

/// The CPU time of the calling thread: Yuna's loop runs on the test's.
fn thread_cpu_time() -> Duration {
    let time = clock_gettime(ClockId::ThreadCPUTime);
    Duration::new(
        time.tv_sec.unsigned_abs(),
        time.tv_nsec.unsigned_abs() as u32,
    )
}

/// One round of the loop, the sentinel's window `w` asking its next frame at each callback: an
/// animation on another seat. Returns the CPU time the round took Yuna.
fn animate(compositor: &mut Compositor, sentinel: &mut Client, w: usize) -> Duration {
    let before = thread_cpu_time();
    pump(compositor);
    let took = thread_cpu_time().saturating_sub(before);
    let frames = sentinel.window(w).frames.len();
    sentinel.read_events();
    if sentinel.window(w).frames.len() > frames {
        sentinel.request_frame(w);
        sentinel.commit(w);
        sentinel.send();
    }
    took
}

#[test]
fn five_thousand_windows_and_menus_hold_the_caps_and_stall_no_frame() {
    // A small output, so that thousands of windows' buffers stay small: 200 × 100 at 1.
    let small = OutputMode::new(200, 100, 60.0, 1.0).expect("a valid mode");
    let mut hostile = Hostile::new("x11-flood", small);
    let client = hostile.client();
    // The sentinel animates throughout: a frame asked at each callback.
    let w = hostile.sentinel_window;
    let first = hostile.sentinel.window(w).frames.len();
    hostile.sentinel.request_frame(w);
    hostile.sentinel.commit(w);
    hostile.sentinel.send();
    let flood_started = Instant::now();
    let sentinel = &mut hostile.sentinel;
    let mut longest_round = Duration::ZERO;
    let (client, ()) = step_pumping(
        &mut hostile.compositor,
        client,
        |compositor| longest_round = longest_round.max(animate(compositor, sentinel, w)),
        |client| {
            let fail = |e: x11rb::errors::ConnectionError| e.to_string();
            for index in 0..5_000 {
                let spec = WindowSpec {
                    background: Some(0x0080_8080),
                    rect: (0, 0, 16, 16),
                    ..WindowSpec::named(&format!("flood-{index}"))
                };
                let window = client.create(&spec)?;
                client.connection.map_window(window).map_err(fail)?;
                if index % 64 == 0 {
                    client.connection.flush().map_err(fail)?;
                }
            }
            client.sync()
        },
    );
    let sent_in = flood_started.elapsed();
    // Yuna takes them while the sentinel animates on: until a thousand are open, then half a
    // second more.
    let deadline = Instant::now() + PATIENCE;
    while hostile.hostile_windows() < MAX_WINDOWS_PER_CLIENT {
        assert!(Instant::now() < deadline, "a thousand windows never opened");
        let took = animate(&mut hostile.compositor, &mut hostile.sentinel, w);
        longest_round = longest_round.max(took);
    }
    let opened_in = flood_started.elapsed();
    let quiet = Instant::now() + Duration::from_millis(500);
    while Instant::now() < quiet {
        let took = animate(&mut hostile.compositor, &mut hostile.sentinel, w);
        longest_round = longest_round.max(took);
    }
    assert_eq!(
        hostile.hostile_windows(),
        MAX_WINDOWS_PER_CLIENT,
        "the cap holds"
    );

    // Another program maps 5,000 override-redirect windows of 8 × 8 over the output: its first
    // 64 show, the cap of menus, and Yuna keeps its frames as before.
    let menus = hostile.client();
    let sentinel = &mut hostile.sentinel;
    let (menus, ()) = step_pumping(
        &mut hostile.compositor,
        menus,
        |compositor| longest_round = longest_round.max(animate(compositor, sentinel, w)),
        |client| {
            let fail = |e: x11rb::errors::ConnectionError| e.to_string();
            for index in 0..5_000_u16 {
                let (x, y) = ((index % 25) * 8, (index / 25 % 12) * 8);
                let spec = WindowSpec {
                    background: Some(0x00C0_4040),
                    ..WindowSpec::menu(&format!("menu-{index}"), x as i16, y as i16, 8, 8)
                };
                let window = client.create(&spec)?;
                client.connection.map_window(window).map_err(fail)?;
                if index % 64 == 0 {
                    client.connection.flush().map_err(fail)?;
                }
            }
            client.sync()
        },
    );
    let output = hostile.output;
    let deadline = Instant::now() + PATIENCE;
    while hostile.compositor.drawn_x11_menus(output).len() < MAX_MENUS_PER_CLIENT {
        assert!(Instant::now() < deadline, "the menus never showed");
        let took = animate(&mut hostile.compositor, &mut hostile.sentinel, w);
        longest_round = longest_round.max(took);
    }
    let menus_in = flood_started.elapsed();
    let quiet = Instant::now() + Duration::from_millis(500);
    while Instant::now() < quiet {
        let took = animate(&mut hostile.compositor, &mut hostile.sentinel, w);
        longest_round = longest_round.max(took);
    }
    assert_eq!(
        hostile.compositor.drawn_x11_menus(output).len(),
        MAX_MENUS_PER_CLIENT,
        "the cap of menus holds"
    );
    // While the manager's channel was full, the sentinel kept being called back, four times a
    // second at least, whatever the machine's load: never starved. In a release build no round
    // of Yuna's loop works a period at 60 Hz, in CPU time of its thread, which other programs
    // lengthen far less than the time between two callbacks: 7 ms measured, the callbacks then
    // two periods apart at most. An unoptimised build arranges a thousand windows a frame in
    // tens of milliseconds, 90 ms a round measured alone, three times that with every test at
    // once: its rounds are printed, not checked.
    let frames = hostile.sentinel.window(w).frames[first..].to_vec();
    let longest = frames
        .windows(2)
        .map(|pair| pair[1].wrapping_sub(pair[0]))
        .max()
        .unwrap_or(0);
    println!(
        "5,000 X11 windows sent in {sent_in:?}, 1,000 open in {opened_in:?}, 5,000 menus mapped \
         and 64 shown by {menus_in:?}: {} sentinel frames, the longest gap {longest} ms, the \
         longest round {longest_round:?} of CPU",
        frames.len()
    );
    let starved = frames.len() < 10 || frames.len() as f64 / menus_in.as_secs_f64() < 4.0;
    assert!(!starved, "{} frames in {menus_in:?}", frames.len());
    if !cfg!(debug_assertions) {
        assert!(
            longest_round <= Duration::from_millis(16),
            "a round of {longest_round:?} of CPU"
        );
    }
    hostile.end(vec![client, menus]);
}

#[test]
fn a_client_shows_at_most_its_cap_of_menus() {
    let mut hostile = Hostile::new("x11-menu-cap", at_1_5());
    let client = hostile.client();
    let count = MAX_MENUS_PER_CLIENT + 16;
    let (client, ()) = step(&mut hostile.compositor, client, move |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let mut menus = Vec::new();
        for index in 0..count {
            let spec = WindowSpec {
                background: Some(0x00C0_2020),
                ..WindowSpec::menu(&format!("menu-{index}"), (index * 10) as i16, 10, 8, 8)
            };
            let menu = client.create(&spec)?;
            client.connection.map_window(menu).map_err(fail)?;
            menus.push(menu);
        }
        client.connection.flush().map_err(fail)?;
        for menu in menus {
            client.wait_for(
                "a menu mapped",
                |event| matches!(event, Event::MapNotify(mapped) if mapped.window == menu),
            )?;
        }
        Ok(())
    });
    let output = hostile.output;
    wait(
        &mut hostile.compositor,
        "the menus are drawn",
        |compositor| {
            (compositor.drawn_x11_menus(output).len() == MAX_MENUS_PER_CLIENT).then_some(())
        },
    );
    hostile.pump_for(Duration::from_millis(300));
    assert_eq!(
        hostile.compositor.drawn_x11_menus(output).len(),
        MAX_MENUS_PER_CLIENT
    );
    hostile.end(vec![client]);
}

#[test]
fn windows_destroyed_between_map_and_pairing_leave_nothing() {
    let mut hostile = Hostile::new("x11-destroyed", at_1_5());
    let client = hostile.client();
    let (client, ()) = step(&mut hostile.compositor, client, |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        for index in 0..200 {
            let spec = WindowSpec {
                override_redirect: index % 2 == 0,
                background: Some(0x0020_20C0),
                ..WindowSpec::named(&format!("gone-{index}"))
            };
            let window = client.create(&spec)?;
            client.connection.map_window(window).map_err(fail)?;
            if index % 3 == 0 {
                client.sync()?;
            }
            client.connection.destroy_window(window).map_err(fail)?;
        }
        client.sync()
    });
    hostile.pump_for(Duration::from_millis(500));
    assert_eq!(hostile.hostile_windows(), 0);
    // A window after them opens as any does.
    let (client, _, _) = open_window(&mut hostile.compositor, client, &WindowSpec::named("after"));
    hostile.end(vec![client]);
}

#[test]
fn an_x11_client_killed_mid_request_leaves_the_rest() {
    let mut hostile = Hostile::new("x11-killed", at_1_5());
    let survivor = hostile.client();
    let (survivor, _, survivor_id) = open_window(
        &mut hostile.compositor,
        survivor,
        &WindowSpec::named("survivor"),
    );
    let victim = hostile.client();
    let (victim, _, victim_id) = open_window(
        &mut hostile.compositor,
        victim,
        &WindowSpec::named("victim"),
    );
    // The victim's connection takes the first bytes of a ChangeProperty of 4,000 bytes, then
    // ends, as a program killed while it writes.
    let socket = victim
        .connection
        .stream()
        .as_fd()
        .try_clone_to_owned()
        .expect("a copy of the socket");
    let mut socket = UnixStream::from(socket);
    socket
        .write_all(&[18, 0, 0xE8, 0x03, 0, 0, 0, 0, 0, 0])
        .expect("half a request written");
    socket.shutdown(Shutdown::Both).expect("the socket ends");
    drop(socket);
    drop(victim);
    wait(
        &mut hostile.compositor,
        "the victim's window closes",
        |compositor| {
            (!compositor
                .windows()
                .iter()
                .any(|window| window.id == victim_id))
            .then_some(())
        },
    );
    assert!(
        hostile
            .compositor
            .windows()
            .iter()
            .any(|window| window.id == survivor_id),
        "the other X11 client's window stays"
    );
    let (survivor, ()) = step(&mut hostile.compositor, survivor, |client| client.sync());
    hostile.end(vec![survivor]);
}

#[test]
fn menus_at_huge_or_negative_positions_are_drawn_where_they_show() {
    let mut hostile = Hostile::new("x11-far-menus", at_1_5());
    // Menus of a window, at offsets that take them off every output.
    let client = hostile.client();
    let (client, parent, parent_id) =
        open_window(&mut hostile.compositor, client, &WindowSpec::named("near"));
    let with_parent = [(32_000, -32_000), (-32_768, 32_767)];
    let client = map_menus(
        &mut hostile.compositor,
        client,
        "far",
        Some(parent),
        &with_parent,
    );
    // Menus of a program without a window, drawn alone at their own place: two show on the
    // output, cut to it, at (-100, 50) and at its far corner.
    let lone = hostile.client();
    let alone = [
        (-32_000, -32_000),
        (32_000, 32_000),
        (-100, 50),
        (2_850, 1_600),
    ];
    let lone = map_menus(&mut hostile.compositor, lone, "alone", None, &alone);
    hostile.pump_for(Duration::from_millis(500));
    let output = hostile.output;
    let drawn = wait(
        &mut hostile.compositor,
        "the menus that show",
        |compositor| {
            let drawn = compositor.drawn_x11_menus(output);
            (drawn.len() == 2).then_some(drawn)
        },
    );
    assert!(drawn.iter().all(|menu| menu.parent.is_none()), "{drawn:?}");
    assert_eq!(
        drawn
            .iter()
            .map(|menu| (menu.rect.0, menu.rect.1))
            .collect::<Vec<_>>(),
        vec![(2_850, 1_600), (-100, 50)],
        "topmost first"
    );
    assert!(
        hostile
            .compositor
            .windows()
            .iter()
            .any(|window| window.id == parent_id)
    );
    hostile.end(vec![client, lone]);
}

/// Maps an override-redirect window of 200 × 100 at each of `places`, named `name` and its index,
/// for `parent` if any, and waits until X11 maps them.
fn map_menus(
    compositor: &mut Compositor,
    client: X11Client,
    name: &str,
    parent: Option<Window>,
    places: &[(i16, i16)],
) -> X11Client {
    let name = name.to_owned();
    let places = places.to_vec();
    step(compositor, client, move |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let mut menus = Vec::new();
        for (index, (x, y)) in places.into_iter().enumerate() {
            let spec = WindowSpec {
                transient_for: parent,
                background: Some(0x00C0_C020),
                ..WindowSpec::menu(&format!("{name}-{index}"), x, y, 200, 100)
            };
            let menu = client.create(&spec)?;
            client.connection.map_window(menu).map_err(fail)?;
            menus.push(menu);
        }
        client.connection.flush().map_err(fail)?;
        for menu in menus {
            client.wait_for(
                "a menu mapped",
                |event| matches!(event, Event::MapNotify(mapped) if mapped.window == menu),
            )?;
        }
        Ok(())
    })
    .0
}

// ----- Random sequences -----

/// The atoms a random property or message names: those the manager reads, of every kind, and
/// any other.
#[derive(Debug, Clone, Copy)]
enum AtomChoice {
    Known(usize),
    Raw(u32),
}

/// A value of a random property or message: a number, one of the case's windows, or an atom.
#[derive(Debug, Clone, Copy)]
enum Word {
    Raw(u32),
    Window(usize),
    Atom(AtomChoice),
}

/// A rectangle of X11: x, y, width, height.
type X11Rect = (i16, i16, u16, u16);

/// One X11 request of a random sequence, on the case's windows by index.
#[derive(Debug, Clone)]
enum Op {
    Create {
        override_redirect: bool,
        rect: X11Rect,
    },
    Map(usize),
    Unmap(usize),
    Destroy(usize),
    Configure(usize, X11Rect),
    Raise(usize),
    SetProperty {
        window: usize,
        property: AtomChoice,
        type_: AtomChoice,
        format: u8,
        values: Vec<Word>,
    },
    DeleteProperty(usize, AtomChoice),
    Message {
        window: usize,
        to_root: bool,
        type_: AtomChoice,
        format: u8,
        data: [Word; 5],
    },
}

/// The cases of the random story: each starts an X11 client, runs up to [`MAX_OPS`] requests and
/// lets it go, with the checks after each; 24 of them take about 10 s of the check in a debug
/// build. `PROPTEST_CASES=<n>` runs `n`.
const CASES: u32 = 24;

/// The most requests of one case.
const MAX_OPS: usize = 32;

fn atom_choice() -> impl Strategy<Value = AtomChoice> {
    prop_oneof![
        4 => (0..KNOWN_ATOMS).prop_map(AtomChoice::Known),
        1 => any::<u32>().prop_map(AtomChoice::Raw),
    ]
}

fn word() -> impl Strategy<Value = Word> {
    prop_oneof![
        2 => prop_oneof![Just(0_u32), Just(1), Just(2), Just(u32::MAX), any::<u32>()]
            .prop_map(Word::Raw),
        1 => (0..8_usize).prop_map(Word::Window),
        2 => atom_choice().prop_map(Word::Atom),
    ]
}

fn rect() -> impl Strategy<Value = X11Rect> {
    let coordinate = prop_oneof![
        3 => -200_i16..3_000,
        1 => any::<i16>(),
    ];
    let side = prop_oneof![
        3 => 1_u16..400,
        1 => Just(0_u16),
        1 => any::<u16>(),
    ];
    (coordinate.clone(), coordinate, side.clone(), side)
}

fn op() -> impl Strategy<Value = Op> {
    let window = 0..8_usize;
    prop_oneof![
        3 => (any::<bool>(), rect())
            .prop_map(|(override_redirect, rect)| Op::Create { override_redirect, rect }),
        3 => window.clone().prop_map(Op::Map),
        1 => window.clone().prop_map(Op::Unmap),
        1 => window.clone().prop_map(Op::Destroy),
        2 => (window.clone(), rect()).prop_map(|(window, rect)| Op::Configure(window, rect)),
        1 => window.clone().prop_map(Op::Raise),
        4 => (
            window.clone(),
            atom_choice(),
            atom_choice(),
            prop_oneof![Just(8_u8), Just(16), Just(32)],
            prop::collection::vec(word(), 0..24),
        )
            .prop_map(|(window, property, type_, format, values)| Op::SetProperty {
                window,
                property,
                type_,
                format,
                values,
            }),
        1 => (window.clone(), atom_choice())
            .prop_map(|(window, property)| Op::DeleteProperty(window, property)),
        2 => (
            window,
            any::<bool>(),
            atom_choice(),
            prop_oneof![Just(32_u8), Just(8), Just(16)],
            [word(), word(), word(), word(), word()],
        )
            .prop_map(|(window, to_root, type_, format, data)| Op::Message {
                window,
                to_root,
                type_,
                format,
                data,
            }),
    ]
}

/// How many atoms [`known_atom`] names.
const KNOWN_ATOMS: usize = 26;

/// The `index`th atom the manager reads, a type it reads them in, or a value it looks for.
fn known_atom(client: &X11Client, index: usize) -> u32 {
    let atoms = client.atoms;
    let known: [u32; KNOWN_ATOMS] = [
        AtomEnum::WM_NAME.into(),
        AtomEnum::WM_CLASS.into(),
        AtomEnum::WM_NORMAL_HINTS.into(),
        AtomEnum::WM_SIZE_HINTS.into(),
        AtomEnum::WM_HINTS.into(),
        AtomEnum::WM_TRANSIENT_FOR.into(),
        AtomEnum::STRING.into(),
        AtomEnum::CARDINAL.into(),
        AtomEnum::ATOM.into(),
        AtomEnum::WINDOW.into(),
        atoms.UTF8_STRING,
        atoms._NET_WM_NAME,
        atoms.WM_PROTOCOLS,
        atoms.WM_DELETE_WINDOW,
        atoms._NET_WM_STATE,
        atoms._NET_WM_STATE_FULLSCREEN,
        atoms._NET_WM_STATE_HIDDEN,
        atoms._NET_ACTIVE_WINDOW,
        atoms._NET_WM_PING,
        atoms._NET_WM_SYNC_REQUEST,
        atoms._NET_WM_SYNC_REQUEST_COUNTER,
        atoms._NET_STARTUP_ID,
        atoms._NET_WM_WINDOW_TYPE,
        atoms._NET_WM_WINDOW_TYPE_POPUP_MENU,
        atoms.WL_SURFACE_SERIAL,
        atoms.WM_STATE,
    ];
    known[index % KNOWN_ATOMS]
}

/// The atom `choice` names.
fn atom_of(client: &X11Client, choice: AtomChoice) -> u32 {
    match choice {
        AtomChoice::Known(index) => known_atom(client, index),
        AtomChoice::Raw(atom) => atom,
    }
}

/// The value `word` names: a window of the case, or 0 when it has none of that index.
fn value_of(client: &X11Client, windows: &[Window], word: Word) -> u32 {
    match word {
        Word::Raw(value) => value,
        Word::Window(index) => windows
            .get(index % windows.len().max(1))
            .copied()
            .unwrap_or(0),
        Word::Atom(choice) => atom_of(client, choice),
    }
}

/// Sends `op` on `client`, its windows `windows`. Requests on windows gone fail in X11's
/// errors, which the client leaves.
fn apply(client: &mut X11Client, windows: &mut Vec<Window>, op: &Op) -> Result<(), String> {
    let fail = |e: x11rb::errors::ConnectionError| e.to_string();
    let pick = |index: usize| windows.get(index % windows.len().max(1)).copied();
    match op {
        Op::Create {
            override_redirect,
            rect,
        } => {
            let spec = WindowSpec {
                override_redirect: *override_redirect,
                rect: *rect,
                background: Some(0x0050_A050),
                ..WindowSpec::named("random")
            };
            // X11 refuses a side of 0; the window is not made, and the case goes on.
            if rect.2 > 0 && rect.3 > 0 {
                let window = client.create(&spec)?;
                windows.push(window);
            }
        }
        Op::Map(index) => {
            if let Some(window) = pick(*index) {
                client.connection.map_window(window).map_err(fail)?;
            }
        }
        Op::Unmap(index) => {
            if let Some(window) = pick(*index) {
                client.connection.unmap_window(window).map_err(fail)?;
            }
        }
        Op::Destroy(index) => {
            if let Some(window) = pick(*index) {
                client.connection.destroy_window(window).map_err(fail)?;
            }
        }
        Op::Configure(index, (x, y, width, height)) => {
            if let Some(window) = pick(*index) {
                let aux = ConfigureWindowAux::new()
                    .x(i32::from(*x))
                    .y(i32::from(*y))
                    .width(u32::from((*width).max(1)))
                    .height(u32::from((*height).max(1)));
                client
                    .connection
                    .configure_window(window, &aux)
                    .map_err(fail)?;
            }
        }
        Op::Raise(index) => {
            if let Some(window) = pick(*index) {
                let aux = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
                client
                    .connection
                    .configure_window(window, &aux)
                    .map_err(fail)?;
            }
        }
        Op::SetProperty {
            window,
            property,
            type_,
            format,
            values,
        } => {
            if let Some(window) = pick(*window) {
                let property = atom_of(client, *property);
                let type_ = atom_of(client, *type_);
                let values: Vec<u32> = values
                    .iter()
                    .map(|&word| value_of(client, windows, word))
                    .collect();
                match format {
                    8 => {
                        let bytes: Vec<u8> = values.iter().map(|&value| value as u8).collect();
                        client
                            .connection
                            .change_property8(PropMode::REPLACE, window, property, type_, &bytes)
                            .map_err(fail)?;
                    }
                    16 => {
                        let shorts: Vec<u16> = values.iter().map(|&value| value as u16).collect();
                        client
                            .connection
                            .change_property16(PropMode::REPLACE, window, property, type_, &shorts)
                            .map_err(fail)?;
                    }
                    _ => {
                        client
                            .connection
                            .change_property32(PropMode::REPLACE, window, property, type_, &values)
                            .map_err(fail)?;
                    }
                }
            }
        }
        Op::DeleteProperty(index, property) => {
            if let Some(window) = pick(*index) {
                let property = atom_of(client, *property);
                client
                    .connection
                    .delete_property(window, property)
                    .map_err(fail)?;
            }
        }
        Op::Message {
            window,
            to_root,
            type_,
            format,
            data,
        } => {
            if let Some(window) = pick(*window) {
                let type_ = atom_of(client, *type_);
                let data = data.map(|word| value_of(client, windows, word));
                let to = if *to_root { client.root } else { window };
                send_message(client, to, window, type_, *format, data)?;
            }
        }
    }
    Ok(())
}

/// Runs one case: an X11 client connects, sends `ops`, each followed by the checks, and leaves;
/// its windows close, and the sentinel goes on.
fn run_case(hostile: &mut Hostile, ops: &[Op]) -> Result<(), TestCaseError> {
    let mut client = hostile.client();
    let mut windows = Vec::new();
    for op in ops {
        let op = op.clone();
        let (back, kept) = step(&mut hostile.compositor, client, move |client| {
            apply(client, &mut windows, &op)?;
            client.sync()?;
            Ok(windows)
        });
        client = back;
        windows = kept;
        for _ in 0..3 {
            pump(&mut hostile.compositor);
        }
        prop_assert_eq!(hostile.compositor.layout().check_invariants(), Ok(()));
        prop_assert_eq!(hostile.compositor.check_windows(), Ok(()));
        prop_assert_eq!(hostile.compositor.check_popups_and_keyboard(), Ok(()));
    }
    drop(client);
    let seat = hostile.seat;
    let output = hostile.output;
    wait(
        &mut hostile.compositor,
        "the case's windows close",
        |compositor| {
            let windows = compositor
                .windows()
                .iter()
                .filter(|window| window.seat == seat)
                .count();
            (windows == 0 && compositor.drawn_x11_menus(output).is_empty()).then_some(())
        },
    );
    hostile.others_untouched();
    Ok(())
}

#[test]
fn random_x11_requests_keep_yuna_up() {
    let hostile = RefCell::new(Hostile::new("x11-random", at_1_5()));
    let mut config = contextualize_config(ProptestConfig {
        cases: CASES,
        // Beside this file, as `proptest!` would set it.
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource(
            "proptest-regressions",
        ))),
        ..ProptestConfig::default()
    });
    config.source_file = Some(file!());
    let cases = config.cases;
    let started = Instant::now();
    let mut runner = TestRunner::new(config);
    let result = runner.run(&prop::collection::vec(op(), 1..=MAX_OPS), |ops| {
        run_case(&mut hostile.borrow_mut(), &ops)
    });
    if let Err(error) = result {
        panic!("{error}\n{runner}");
    }
    println!(
        "{cases} random X11 cases of up to {MAX_OPS} requests in {:?}",
        started.elapsed()
    );
    hostile.into_inner().end(Vec::new());
}
