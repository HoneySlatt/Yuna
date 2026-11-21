//! XWayland: each seat's X display, its Xwayland started by the first X11 connection, behind the
//! seat's cookie, ended when idle or killed
//! ([ADR-0067](../../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md),
//! [ADR-0068](../../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)).
//!
//! Each story runs the development shell's Xwayland. Its X11 client, x11rb's, runs on a thread of
//! its own, since it waits for Xwayland, while the test runs the compositor's loop
//! (`Compositor::dispatch`) until the client is answered. Xwayland runs on real time; the
//! compositor's manual clock moves only where a story checks that Yuna rests. Clients connect
//! through a display's abstract socket, as libxcb does first, and give their cookie themselves:
//! tests share one process, whose `XAUTHORITY` they cannot set.

use std::fs;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};
use x11rb::connection::Connection;
use x11rb::errors::ConnectError;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
use x11rb::rust_connection::{DefaultStream, RustConnection};
use yuna::rest::Source;
use yuna::{
    ClockMode, Compositor, Config, FIRST_PAUSE, IDLE_SECONDS, MAX_DISPLAYS, OWNER_SEAT, OutputMode,
    X11Display, XwaylandConfig,
};
use yuna_types::SeatId;

use crate::x11_client::xauthority::entry_fields;
use crate::x11_windows::Folder;
use crate::{assert_rests, run_armed_deadlines};

/// How long a story waits for Xwayland: a start takes a fraction of a second, longer while every
/// test runs at once.
const PATIENCE: Duration = Duration::from_secs(30);

/// A compositor on the manual clock, XWayland on with `idle_seconds`, the owner's seat given a
/// display, its Xauthority in `folder`, and `outputs`.
fn compositor(folder: &Folder, outputs: Vec<OutputMode>, idle_seconds: u32) -> Compositor {
    Compositor::new(Config {
        outputs,
        clock: ClockMode::Manual,
        listen: false,
        xwayland: Some(XwaylandConfig {
            idle_seconds,
            owner_display_folder: Some(folder.0.join("owner.x11")),
            ..XwaylandConfig::default()
        }),
        ..Config::default()
    })
    .expect("the compositor starts")
}

/// The owner's seat's display.
fn display_of(compositor: &Compositor, seat: SeatId) -> X11Display {
    compositor
        .x11_display(seat)
        .expect("the seat has a display")
}

/// The process id of the Xwayland serving `seat`'s display; none at rest.
fn server_of(compositor: &Compositor, seat: SeatId) -> Option<i32> {
    display_of(compositor, seat).server
}

/// The cookie of `display`'s Xauthority file, which must hold one entry: `FamilyWild`, no
/// address, the display's number, `MIT-MAGIC-COOKIE-1` and 16 bytes.
fn checked_cookie_of(display: &X11Display) -> Vec<u8> {
    let bytes = fs::read(&display.xauthority).expect("the Xauthority file");
    assert_eq!(&bytes[..2], &[0xFF, 0xFF], "FamilyWild");
    let (fields, end) = entry_fields(&bytes).expect("an entry");
    assert_eq!(end, bytes.len(), "one entry");
    assert_eq!(fields[0], b"");
    assert_eq!(fields[1], display.number.to_string().as_bytes());
    assert_eq!(fields[2], b"MIT-MAGIC-COOKIE-1");
    assert_eq!(fields[3].len(), 16);
    fields[3].clone()
}

/// Connects to display `number` through its abstract socket, giving `cookie`, or nothing.
fn connect(number: u32, cookie: Option<&[u8]>) -> Result<RustConnection, ConnectError> {
    let address = SocketAddr::from_abstract_name(format!("/tmp/.X11-unix/X{number}"))?;
    let stream = UnixStream::connect_addr(&address)?;
    let (stream, _) = DefaultStream::from_unix_stream(stream)?;
    let (name, data) = match cookie {
        Some(cookie) => (b"MIT-MAGIC-COOKIE-1".to_vec(), cookie.to_vec()),
        None => (Vec::new(), Vec::new()),
    };
    RustConnection::connect_to_stream_with_auth_info(stream, 0, name, data)
}

/// An X11 client on a thread of its own: connects to display `number` with `cookie`, and waits
/// for a reply to `GetInputFocus`. Returns its connection, and the time from its connection to
/// that reply.
fn answered_client(
    number: u32,
    cookie: Vec<u8>,
) -> JoinHandle<Result<(RustConnection, Duration), String>> {
    thread::spawn(move || {
        let start = Instant::now();
        let connection = connect(number, Some(&cookie)).map_err(|e| e.to_string())?;
        connection
            .get_input_focus()
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok((connection, start.elapsed()))
    })
}

/// Runs the compositor's loop until `client`'s thread has finished, and returns what it
/// returned.
fn serve<T>(compositor: &mut Compositor, client: JoinHandle<T>) -> T {
    let deadline = Instant::now() + PATIENCE;
    while !client.is_finished() {
        assert!(
            Instant::now() < deadline,
            "the X11 client was not answered within {PATIENCE:?}"
        );
        compositor
            .dispatch(Some(Duration::from_millis(5)))
            .expect("Yuna dispatches");
    }
    match client.join() {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// Runs the compositor's loop until `done` holds.
fn dispatch_until(compositor: &mut Compositor, what: &str, done: impl Fn(&Compositor) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done(compositor) {
        assert!(Instant::now() < deadline, "{what} within {PATIENCE:?}");
        compositor
            .dispatch(Some(Duration::from_millis(5)))
            .expect("Yuna dispatches");
    }
}

/// Whether process `pid` exists, a zombie included: once reaped, it does not.
fn exists(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// A file's permissions.
fn mode(path: &Path) -> u32 {
    fs::metadata(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o7777
}

/// The integer `name` of an XSETTINGS property, read as the XSETTINGS specification, 0.5,
/// encodes it, in either byte order.
fn xsettings_integer(blob: &[u8], name: &str) -> Option<i32> {
    let big = blob[0] == 1;
    let u16_at = |at: usize| {
        let bytes = [blob[at], blob[at + 1]];
        if big {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        }
    };
    let u32_at = |at: usize| {
        let bytes = [blob[at], blob[at + 1], blob[at + 2], blob[at + 3]];
        if big {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        }
    };
    let padded = |length: usize| length.next_multiple_of(4);
    let count = u32_at(8);
    let mut at = 12;
    for _ in 0..count {
        let kind = blob[at];
        let length = usize::from(u16_at(at + 2));
        let found = &blob[at + 4..at + 4 + length];
        at += 4 + padded(length) + 4;
        let value_at = at;
        at += match kind {
            0 => 4,
            1 => 4 + padded(u32_at(at) as usize),
            _ => 8,
        };
        if kind == 0 && found == name.as_bytes() {
            return Some(u32_at(value_at) as i32);
        }
    }
    None
}

/// The first X11 connection starts Xwayland, which serves it; nothing runs before it. The
/// display's files are the user's alone: its Xauthority file 0600 in a folder 0700; its lock
/// holds Yuna's process id, and its file socket is beside the abstract one. The compositor gone,
/// its lock is gone: the lock file it made is no longer linked. Another story's compositor of
/// this process may take the number at once and make its own lock, naming the same process: the
/// path alone cannot tell the two apart.
#[test]
fn xwayland_starts_on_the_first_connection_and_serves_it() {
    let folder = Folder::new("xwayland-first");
    let mut compositor = compositor(&folder, vec![OutputMode::default()], IDLE_SECONDS);
    let display = display_of(&compositor, OWNER_SEAT);
    assert!(display.number < MAX_DISPLAYS);
    assert_eq!(display.name(), format!(":{}", display.number));
    assert_eq!(display.xauthority, folder.0.join("owner.x11/Xauthority"));
    assert_eq!(mode(&display.xauthority), 0o600);
    assert_eq!(mode(&folder.0.join("owner.x11")), 0o700);
    let lock_path = format!("/tmp/.X{}-lock", display.number);
    let lock = fs::read_to_string(&lock_path).expect("its lock");
    assert_eq!(lock, format!("{:>10}\n", std::process::id()));
    let own_lock = fs::File::open(&lock_path).expect("its lock");
    let file_socket =
        fs::metadata(format!("/tmp/.X11-unix/X{}", display.number)).expect("its file socket");
    assert!(file_socket.file_type().is_socket());

    // Nothing but a connection starts it.
    compositor
        .dispatch(Some(Duration::from_millis(100)))
        .expect("Yuna dispatches");
    assert_eq!(server_of(&compositor, OWNER_SEAT), None);

    let client = answered_client(display.number, checked_cookie_of(&display));
    let (connection, took) = serve(&mut compositor, client).expect("the client is answered");
    eprintln!(
        "an X11 client's first reply came {} ms after it connected",
        took.as_millis()
    );
    let pid = server_of(&compositor, OWNER_SEAT).expect("Xwayland runs");
    let name = fs::read_to_string(format!("/proc/{pid}/comm")).expect("Xwayland's process");
    assert_eq!(name.trim_end(), "Xwayland");
    drop(connection);

    drop(compositor);
    // Ended and reaped with the compositor, its files removed.
    assert!(!exists(pid), "Xwayland outlived the compositor");
    assert!(!display.xauthority.exists());
    assert!(!folder.0.join("owner.x11").exists());
    let links = own_lock.metadata().expect("the lock's metadata").nlink();
    assert_eq!(links, 0, "the lock was left");
}

/// Yuna on llvmpipe, Xwayland touches no GPU either: it loads no DRI driver, which is the
/// machine's own (`/run/opengl-driver` on NixOS), and Mesa's GLX for Xwayland, which opens the
/// machine's display device to draw even on the CPU, then closes it.
#[test]
fn xwayland_on_the_software_device_loads_no_gpu_driver() {
    let folder = Folder::new("xwayland-software");
    let mut compositor = compositor(&folder, vec![OutputMode::default()], IDLE_SECONDS);
    let display = display_of(&compositor, OWNER_SEAT);
    let client = answered_client(display.number, checked_cookie_of(&display));
    let (connection, _) = serve(&mut compositor, client).expect("answered");
    let pid = server_of(&compositor, OWNER_SEAT).expect("Xwayland runs");
    let maps = fs::read_to_string(format!("/proc/{pid}/maps")).expect("Xwayland's mappings");
    let drivers: Vec<&str> = maps
        .lines()
        .filter_map(|line| line.split_whitespace().nth(5))
        .filter(|file| file.ends_with("_dri.so") || file.contains("/libgallium"))
        .collect();
    assert_eq!(drivers, Vec::<&str>::new());
    drop(connection);
}

/// With outputs at 1.5 and 1, the very first client reads `Xft.dpi` of the larger, 144, in
/// `RESOURCE_MANAGER`, and `Xft/DPI` 147,456, 144 × 1024, in XSETTINGS: both are set before
/// Xwayland lets a client in.
#[test]
fn the_first_client_reads_xft_dpi_of_the_largest_scale() {
    let folder = Folder::new("xwayland-dpi");
    let outputs = vec![
        OutputMode::default(),
        OutputMode::new(1920, 1080, 60.0, 1.0).expect("a valid mode"),
    ];
    let mut compositor = compositor(&folder, outputs, IDLE_SECONDS);
    let display = display_of(&compositor, OWNER_SEAT);
    let (number, cookie) = (display.number, checked_cookie_of(&display));
    let client = thread::spawn(move || -> Result<(Vec<u8>, Vec<u8>, bool), String> {
        let connection = connect(number, Some(&cookie)).map_err(|e| e.to_string())?;
        let ask = |e: x11rb::errors::ReplyError| e.to_string();
        let root = connection.setup().roots[0].root;
        let resources = connection
            .get_property(
                false,
                root,
                AtomEnum::RESOURCE_MANAGER,
                AtomEnum::STRING,
                0,
                1024,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(ask)?
            .value;
        let atom = |name: &[u8]| -> Result<u32, String> {
            Ok(connection
                .intern_atom(false, name)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(ask)?
                .atom)
        };
        let owner_of = |selection: u32| -> Result<u32, String> {
            Ok(connection
                .get_selection_owner(selection)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(ask)?
                .owner)
        };
        let settings_owner = owner_of(atom(b"_XSETTINGS_S0")?)?;
        let settings_atom = atom(b"_XSETTINGS_SETTINGS")?;
        let settings = connection
            .get_property(false, settings_owner, settings_atom, settings_atom, 0, 1024)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(ask)?
            .value;
        let managed = owner_of(atom(b"WM_S0")?)? != x11rb::NONE;
        Ok((resources, settings, managed))
    });
    let (resources, settings, managed) = serve(&mut compositor, client).expect("answered");
    assert_eq!(String::from_utf8_lossy(&resources), "Xft.dpi:\t144\n");
    assert_eq!(xsettings_integer(&settings, "Xft/DPI"), Some(147_456));
    assert_eq!(
        xsettings_integer(&settings, "Gdk/WindowScalingFactor"),
        Some(1)
    );
    assert!(managed, "WM_S0 has no owner");
}

/// A client that gives no cookie, or a wrong one, is refused; one with the seat's is served.
#[test]
fn a_client_without_the_cookie_is_refused() {
    let folder = Folder::new("xwayland-refused");
    let mut compositor = compositor(&folder, vec![OutputMode::default()], IDLE_SECONDS);
    let display = display_of(&compositor, OWNER_SEAT);
    let number = display.number;
    let refusals = thread::spawn(move || {
        let refused = |result: Result<RustConnection, ConnectError>| match result {
            Err(ConnectError::SetupFailed(_) | ConnectError::SetupAuthenticate(_)) => Ok(()),
            Err(error) => Err(format!("not refused for its cookie: {error}")),
            Ok(_) => Err("let in".to_owned()),
        };
        refused(connect(number, None))?;
        refused(connect(number, Some(&[7; 16])))
    });
    serve(&mut compositor, refusals).expect("both are refused");
    let client = answered_client(number, checked_cookie_of(&display));
    serve(&mut compositor, client).expect("the seat's cookie is let in");
}

/// Two agents' seats, each with its socket, get a display each, with a cookie each: a seat's
/// cookie opens its own display, never the other's.
#[test]
fn a_seat_s_cookie_does_not_open_another_seat_s_display() {
    let folder = Folder::new("xwayland-two-seats");
    let mut compositor = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Manual,
        listen: false,
        xwayland: Some(XwaylandConfig::default()),
        ..Config::default()
    })
    .expect("the compositor starts");
    assert_eq!(compositor.x11_display(OWNER_SEAT), None);
    let mut seats = Vec::new();
    for _ in 0..2 {
        let seat = compositor.add_seat().expect("a seat");
        compositor
            .add_output(
                seat,
                OutputMode::new(1920, 1080, 60.0, 1.0).expect("a mode"),
            )
            .expect("an output");
        let socket = folder.0.join(format!("seat-{}", seat.0));
        compositor
            .listen_for_seat(seat, socket.clone())
            .expect("the seat listens");
        let display = display_of(&compositor, seat);
        assert_eq!(
            display.xauthority,
            folder.0.join(format!("seat-{}.x11/Xauthority", seat.0))
        );
        seats.push((seat, display.number, checked_cookie_of(&display)));
    }
    let [(a, number_a, cookie_a), (b, number_b, cookie_b)] = &seats[..] else {
        unreachable!("two seats");
    };
    assert_ne!(number_a, number_b);
    assert_ne!(cookie_a, cookie_b);
    let (number_a, number_b) = (*number_a, *number_b);
    let crossed = {
        let (cookie_a, cookie_b) = (cookie_a.clone(), cookie_b.clone());
        thread::spawn(move || {
            let refused = |number: u32, cookie: &[u8]| match connect(number, Some(cookie)) {
                Err(ConnectError::SetupFailed(_) | ConnectError::SetupAuthenticate(_)) => Ok(()),
                Err(error) => Err(format!(":{number} failed otherwise: {error}")),
                Ok(_) => Err(format!(":{number} let another seat's cookie in")),
            };
            refused(number_a, &cookie_b)?;
            refused(number_b, &cookie_a)
        })
    };
    serve(&mut compositor, crossed).expect("each display refuses the other's cookie");
    for (number, cookie) in [(number_a, cookie_a), (number_b, cookie_b)] {
        let client = answered_client(number, cookie.clone());
        serve(&mut compositor, client).expect("each display lets its own cookie in");
    }
    let server_a = server_of(&compositor, *a).expect("seat a's Xwayland");
    let server_b = server_of(&compositor, *b).expect("seat b's Xwayland");
    assert_ne!(server_a, server_b, "one Xwayland per seat");

    // A seat removed takes its Xwayland and its files with it; the other's stays.
    let removed_xauthority = folder.0.join(format!("seat-{}.x11/Xauthority", a.0));
    compositor.remove_seat(*a).expect("seat a is removed");
    dispatch_until(&mut compositor, "seat a's Xwayland is reaped", |_| {
        !exists(server_a)
    });
    assert_eq!(compositor.x11_display(*a), None);
    assert!(!removed_xauthority.exists());
    assert_eq!(server_of(&compositor, *b), Some(server_b));
}

/// With a delay of 1 s, Xwayland ends 1 s after its last X11 client left, its manager still
/// connected, and is reaped; a new connection starts another.
#[test]
fn xwayland_stops_when_idle_and_starts_again() {
    let folder = Folder::new("xwayland-idle");
    let mut compositor = compositor(&folder, vec![OutputMode::default()], 1);
    let display = display_of(&compositor, OWNER_SEAT);
    let client = answered_client(display.number, checked_cookie_of(&display));
    let (connection, _) = serve(&mut compositor, client).expect("answered");
    let first = server_of(&compositor, OWNER_SEAT).expect("Xwayland runs");
    drop(connection);
    let left = Instant::now();
    dispatch_until(&mut compositor, "Xwayland stops when idle", |compositor| {
        server_of(compositor, OWNER_SEAT).is_none()
    });
    let idle = left.elapsed();
    eprintln!(
        "Xwayland stopped {} ms after its last client left",
        idle.as_millis()
    );
    assert!(
        idle >= Duration::from_millis(900),
        "it stopped after {idle:?}"
    );
    assert!(!exists(first), "Xwayland was not reaped");

    let client = answered_client(display.number, checked_cookie_of(&display));
    serve(&mut compositor, client).expect("a new connection is answered");
    let second = server_of(&compositor, OWNER_SEAT).expect("Xwayland runs again");
    assert_ne!(first, second);
}

/// The rest auditor names Xwayland's idle stop: its process's end woke Yuna once, the owner's
/// seat's, and its Wayland client, the seat's Xwayland, is among the clients gone. The X11
/// connection that started it was named too, and the Xwayland that served it.
#[test]
fn xwaylands_idle_stop_is_named_by_the_rest_auditor() {
    let folder = Folder::new("xwayland-rest-audit");
    let mut compositor = compositor(&folder, vec![OutputMode::default()], 1);
    let display = display_of(&compositor, OWNER_SEAT);
    let client = answered_client(display.number, checked_cookie_of(&display));
    let (connection, _) = serve(&mut compositor, client).expect("answered");
    let before = compositor.rest_counts();
    let owners = &before.seats[&OWNER_SEAT];
    assert!(owners.wakes_by(Source::X11Connection) >= 1, "{before:?}");
    // Once a piece of its display's number came: in one write, or more.
    assert!(owners.wakes_by(Source::XwaylandDisplay) >= 1, "{before:?}");
    let xwayland = before
        .clients
        .values()
        .find(|client| client.xwayland)
        .expect("Xwayland's Wayland client is named");
    assert_eq!(xwayland.seat, OWNER_SEAT);
    assert!(xwayland.tally.wakes >= 1, "{before:?}");

    drop(connection);
    dispatch_until(&mut compositor, "Xwayland stops when idle", |compositor| {
        server_of(compositor, OWNER_SEAT).is_none()
    });
    let window = compositor.rest_counts().since(&before);
    assert_eq!(
        window.whole.wakes_by(Source::XwaylandExited),
        1,
        "{window:?}"
    );
    assert_eq!(
        window.seats[&OWNER_SEAT].wakes_by(Source::XwaylandExited),
        1,
        "{window:?}"
    );
    assert_eq!(window.whole.gone_clients, 1, "{window:?}");
    assert!(window.clients.values().all(|client| !client.xwayland));
}

/// Xwayland killed leaves Yuna running: its client's connection ends, it is reaped, and a new
/// connection starts another at once.
#[test]
fn a_crash_of_xwayland_leaves_yuna_running() {
    let folder = Folder::new("xwayland-crash");
    let mut compositor = compositor(&folder, vec![OutputMode::default()], IDLE_SECONDS);
    let display = display_of(&compositor, OWNER_SEAT);
    let client = answered_client(display.number, checked_cookie_of(&display));
    let (connection, _) = serve(&mut compositor, client).expect("answered");
    let first = server_of(&compositor, OWNER_SEAT).expect("Xwayland runs");
    let pid = Pid::from_raw(first).expect("a process id");
    kill_process(pid, Signal::KILL).expect("Xwayland is killed");
    dispatch_until(
        &mut compositor,
        "the killed Xwayland is reaped",
        |compositor| server_of(compositor, OWNER_SEAT).is_none(),
    );
    assert!(!exists(first), "Xwayland was not reaped");
    // Its socket is closed: the reply fails at once.
    let reply = connection.get_input_focus().map(|cookie| cookie.reply());
    assert!(!matches!(reply, Ok(Ok(_))), "a dead Xwayland answered");

    let client = answered_client(display.number, checked_cookie_of(&display));
    serve(&mut compositor, client).expect("a new connection is answered");
    assert_ne!(server_of(&compositor, OWNER_SEAT), Some(first));
}

/// Xwayland killed twice in a row: the first time it starts again at once, the second its
/// display pauses [`FIRST_PAUSE`] first, a program connecting meanwhile waiting in the backlog
/// until it is served. Whether Yuna paused is read from what it did, its pause's timer waking it
/// (the rest auditor's `XwaylandPause`), not from how long a program waited: on a loaded machine
/// Xwayland's start alone may take longer than the pause.
#[test]
fn a_second_crash_in_a_row_pauses_before_xwayland_starts_again() {
    let folder = Folder::new("xwayland-crash-loop");
    let mut compositor = compositor(&folder, vec![OutputMode::default()], IDLE_SECONDS);
    let display = display_of(&compositor, OWNER_SEAT);
    let pauses = |compositor: &Compositor| {
        compositor
            .rest_counts()
            .seats
            .get(&OWNER_SEAT)
            .map_or(0, |seat| seat.wakes_by(Source::XwaylandPause))
    };
    let mut killed = Instant::now();
    for start in 0..2 {
        let client = answered_client(display.number, checked_cookie_of(&display));
        let (connection, _) = serve(&mut compositor, client).expect("answered");
        assert_eq!(pauses(&compositor), 0, "a pause before start {start}");
        let pid = server_of(&compositor, OWNER_SEAT).expect("Xwayland runs");
        killed = Instant::now();
        kill_process(Pid::from_raw(pid).expect("a pid"), Signal::KILL).expect("killed");
        dispatch_until(
            &mut compositor,
            "the killed Xwayland is reaped",
            |compositor| server_of(compositor, OWNER_SEAT).is_none(),
        );
        drop(connection);
    }
    let client = answered_client(display.number, checked_cookie_of(&display));
    let (_, waited) = serve(&mut compositor, client).expect("answered after the pause");
    let after_crash = killed.elapsed();
    eprintln!(
        "after the second crash: answered {} ms after it, the program waited {} ms",
        after_crash.as_millis(),
        waited.as_millis()
    );
    assert_eq!(pauses(&compositor), 1, "the second crash paused once");
    assert!(
        after_crash >= FIRST_PAUSE,
        "served {after_crash:?} after the second crash"
    );
}

/// A number whose abstract socket another process holds is skipped, its lock left alone; Yuna's
/// display, another number, serves.
#[test]
fn a_display_whose_abstract_name_is_taken_is_skipped() {
    let folder = Folder::new("xwayland-taken");
    // The lowest numbers free now, which Yuna would take first.
    let mut held = Vec::new();
    for number in 0..MAX_DISPLAYS {
        let address = SocketAddr::from_abstract_name(format!("/tmp/.X11-unix/X{number}"))
            .expect("an abstract name");
        if let Ok(listener) = UnixListener::bind_addr(&address) {
            held.push((number, listener));
            if held.len() == 2 {
                break;
            }
        }
    }
    assert_eq!(held.len(), 2, "two free numbers");
    let mut compositor = compositor(&folder, vec![OutputMode::default()], IDLE_SECONDS);
    let display = display_of(&compositor, OWNER_SEAT);
    for (number, listener) in &held {
        assert_ne!(display.number, *number, "Yuna took a number held");
        let lock = fs::read_to_string(format!("/tmp/.X{number}-lock"));
        assert!(
            lock.is_err() || lock.is_ok_and(|text| text.trim() != std::process::id().to_string()),
            "Yuna locked :{number}"
        );
        // Still the test's.
        let address = listener.local_addr().expect("its address");
        UnixStream::connect_addr(&address).expect("the held socket accepts");
    }
    let client = answered_client(display.number, checked_cookie_of(&display));
    serve(&mut compositor, client).expect("Yuna's display is answered");
}

/// With XWayland off, no seat gets a display, and no file is made.
#[test]
fn no_xwayland_when_it_is_off() {
    let folder = Folder::new("xwayland-off");
    let mut compositor = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts");
    let seat = compositor.add_seat().expect("a seat");
    let socket = folder.0.join("seat");
    compositor
        .listen_for_seat(seat, socket.clone())
        .expect("the seat listens");
    assert_eq!(compositor.x11_display(OWNER_SEAT), None);
    assert_eq!(compositor.x11_display(seat), None);
    assert!(!folder.0.join("seat.x11").exists());
}

/// XWayland on, but no Xwayland on its search path: no seat gets a display, and no file is
/// made.
#[test]
fn xwayland_missing_from_path_gives_no_display() {
    let folder = Folder::new("xwayland-missing");
    let empty = folder.0.join("bin");
    fs::create_dir(&empty).expect("an empty folder");
    let mut compositor = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Manual,
        listen: false,
        xwayland: Some(XwaylandConfig {
            search_path: Some(empty.into_os_string()),
            owner_display_folder: Some(folder.0.join("owner.x11")),
            ..XwaylandConfig::default()
        }),
        ..Config::default()
    })
    .expect("the compositor starts without Xwayland");
    let seat = compositor.add_seat().expect("a seat");
    compositor
        .listen_for_seat(seat, folder.0.join("seat"))
        .expect("the seat listens");
    assert_eq!(compositor.x11_display(OWNER_SEAT), None);
    assert_eq!(compositor.x11_display(seat), None);
    assert!(!folder.0.join("owner.x11").exists());
    assert!(!folder.0.join("seat.x11").exists());
}

/// A display waiting for its first connection costs nothing: no process, no timer, no wake-up.
/// Once Xwayland has stopped, idle, Yuna rests again.
#[test]
fn yuna_rests_with_xwayland_idle() {
    let folder = Folder::new("xwayland-rest");
    let mut compositor = compositor(&folder, vec![OutputMode::default()], 1);
    let rests = |compositor: &mut Compositor| {
        run_armed_deadlines(compositor);
        assert_rests(compositor);
        // Real time too, on which Xwayland's sources wake Yuna.
        let before = compositor.counters().wakeups;
        compositor
            .dispatch(Some(Duration::from_millis(300)))
            .expect("Yuna dispatches");
        assert_eq!(compositor.counters().wakeups, before, "Yuna woke");
        assert_eq!(server_of(compositor, OWNER_SEAT), None);
    };
    rests(&mut compositor);

    let display = display_of(&compositor, OWNER_SEAT);
    let client = answered_client(display.number, checked_cookie_of(&display));
    let (connection, _) = serve(&mut compositor, client).expect("answered");
    let pid = server_of(&compositor, OWNER_SEAT).expect("Xwayland runs");
    drop(connection);
    dispatch_until(&mut compositor, "Xwayland stops when idle", |compositor| {
        server_of(compositor, OWNER_SEAT).is_none()
    });
    assert!(!exists(pid));
    rests(&mut compositor);
}
