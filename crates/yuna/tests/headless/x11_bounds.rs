//! What one X11 client can make a seat's X11 window manager hold, and how a manager that stops
//! serving is ended
//! ([ADR-0070](../../../../docs/adr/0070-bounds-on-what-one-x11-client-may-ask.md),
//! [ADR-0080](../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)): a
//! made-up event that x11rb misreads, the commands Yuna owes a manager that no longer takes them,
//! the titles and atom names an X11 client makes Yuna keep, Xwayland's environment, and a panic
//! of the manager's thread.
//!
//! Each runs the development shell's Xwayland for one agent's seat, on the real clock. The
//! memory figures are the process's anonymous memory, Yuna's and its managers' threads': a story
//! compares its growth with what a bound allows. Such a story runs alone, in a process of its
//! own ([`in_its_own_process`]): the stories that run beside it in one process would change the
//! figure.

use std::time::{Duration, Instant};

use smithay::reexports::rustix::process::{Pid, Signal, kill_process};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, EventMask, KEYMAP_NOTIFY_EVENT, PropMode, Window,
};
use x11rb::wrapper::ConnectionExt as _;
use yuna::{ClockMode, Compositor, MAX_WINDOWS_PER_CLIENT};

use crate::selection_client::SelectionClient;
use crate::x11_client::{PATIENCE, WindowSpec, X11Client, step};
use crate::x11_misbehaving::assert_rests_real;
use crate::x11_selections::{atoms_of, notify, open_wayland_window, take_selection};
use crate::x11_windows::{X11Seat, at_1_5, off, open_drawn, pump, wait, window_titled};

/// The variable that marks the process a story runs alone in.
const ALONE: &str = "YUNA_STORY_ALONE";

/// Whether this is `story`'s own process: the test binary run again, filtered to it, with
/// `variables` and [`ALONE`]. When it is not, runs it there, waits for it, and checks that it ran
/// and passed: the caller then has nothing more to do.
fn in_its_own_process(story: &str, variables: &[(&str, &str)]) -> bool {
    if std::env::var_os(ALONE).is_some() {
        return true;
    }
    let filter = format!("x11_bounds::{story}");
    let run = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args([
            filter.as_str(),
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ALONE, "1")
        .envs(variables.iter().copied())
        .output()
        .expect("the story runs in a process of its own");
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    println!("{output}");
    assert!(run.status.success(), "{story} failed in its own process");
    assert!(output.contains("1 passed"), "{story} did not run");
    false
}

/// A field of a `/proc/<pid>/status` file, in KiB: `RssAnon:`, `VmRSS:`.
fn status_kib(pid: &str, field: &str) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix(field)?
                    .trim()
                    .strip_suffix("kB")?
                    .trim()
                    .parse()
                    .ok()
            })
        })
        .unwrap_or(0)
}

/// The anonymous memory of this process, which is Yuna's and its managers' threads, in KiB.
fn own_anon_kib() -> u64 {
    status_kib("self", "RssAnon:")
}

/// Runs the compositor's loop for `how_long`.
fn pump_for(compositor: &mut Compositor, how_long: Duration) {
    let until = Instant::now() + how_long;
    while Instant::now() < until {
        pump(compositor);
    }
}

/// Whether Yuna lists a window titled `title`.
fn listed(compositor: &Compositor, title: &str) -> bool {
    compositor
        .windows()
        .iter()
        .any(|window| window.title.as_deref() == Some(title))
}

/// The process id of the Xwayland serving `seat`'s display; none at rest.
fn xwayland_of(seat: &X11Seat) -> Option<i32> {
    seat.compositor
        .x11_display(seat.seat)
        .and_then(|display| display.server)
}

/// Sends `signal` to the Xwayland process `pid`.
fn signal(pid: i32, signal: Signal) {
    kill_process(Pid::from_raw(pid).expect("a process id"), signal).expect("Xwayland is signalled");
}

/// Makes the root's clients receive a `KeymapNotify` that `client` made up (`SendEvent`): its
/// bytes 2 and 3, where every other event carries its sequence number, half the 16-bit range
/// ahead of any the connection uses.
fn send_keymap_notify(client: &X11Client) -> Result<(), String> {
    let mut bytes = [0_u8; 32];
    bytes[0] = KEYMAP_NOTIFY_EVENT;
    bytes[2] = 0x00;
    bytes[3] = 0x80;
    client
        .connection
        .send_event(
            false,
            client.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            bytes,
        )
        .map_err(|e| e.to_string())?;
    client.sync()
}

/// debt.md, XWayland: "A `KeymapNotify` sent by an X11 client (`SendEvent`) stalls its seat's
/// manager". An X11 client sends one made-up `KeymapNotify` to the root, then maps a window. Yuna
/// shows that window as it showed the one before, served by the same Xwayland.
#[test]
fn a_window_mapped_after_a_sent_keymap_notify_is_shown() {
    let mut seat = X11Seat::new("keymap", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, _, _, _) = open_drawn(&mut seat.compositor, client, &WindowSpec::named("before"));
    let server = xwayland_of(&seat);
    let (client, ()) = step(&mut seat.compositor, client, |client| {
        send_keymap_notify(client)?;
        let spec = WindowSpec {
            background: Some(0x0040_8040),
            ..WindowSpec::named("after")
        };
        let window = client.create(&spec)?;
        client
            .connection
            .map_window(window)
            .map_err(|e| e.to_string())?;
        client.connection.flush().map_err(|e| e.to_string())
    });
    let started = Instant::now();
    let deadline = started + Duration::from_secs(10);
    while Instant::now() < deadline && !listed(&seat.compositor, "after") {
        pump(&mut seat.compositor);
    }
    let shown = listed(&seat.compositor, "after");
    let still = xwayland_of(&seat);
    println!(
        "after a sent KeymapNotify: the next window listed {shown} within {:?}; Xwayland \
         {server:?} then {still:?}",
        started.elapsed(),
    );
    drop(client);
    assert!(
        shown,
        "a window mapped after a made-up KeymapNotify is never shown: the seat's manager stalls"
    );
    assert_eq!(still, server, "the same Xwayland serves the seat");
}

/// What a Wayland client of the seat sets as its clipboard in
/// [`commands_to_a_stalled_manager_are_bounded`]: 64 MIME types of 255 bytes, the most X11 is
/// told of.
fn many_mime_types() -> Vec<String> {
    (0..64)
        .map(|index| {
            let mut name = format!("text/x-bounds-{index}-");
            name.extend(std::iter::repeat_n('x', 255 - name.len()));
            name
        })
        .collect()
}

/// Sets the Wayland client's clipboard `rounds` times, one round of Yuna's loop each, freeing
/// what the client and Smithay keep of each: the previous source and the offers it is given.
fn churn_clipboard(seat: &mut X11Seat, wl: &mut SelectionClient, rounds: usize) {
    let types = many_mime_types();
    let types: Vec<&str> = types.iter().map(String::as_str).collect();
    let mut previous: Option<usize> = None;
    for _ in 0..rounds {
        let serial = wl.enter_serial();
        let source = wl.set_clipboard(&types, serial);
        wl.roundtrip(&mut seat.compositor);
        if let Some(previous) = previous.replace(source) {
            wl.destroy_source(previous);
        }
        for offer in 0..wl.state.offers.len() {
            wl.destroy_offer(offer);
        }
        wl.state.offers.clear();
        wl.state.events.clear();
    }
}

/// debt.md, Architecture: "The X11 manager's commands go through an unbounded channel … Bounded
/// at Stage 2, or earlier if a client can fill it." The seat's manager stalls: its Xwayland is
/// stopped (SIGSTOP), as a hung one would be, and the manager waits for its answer to the first
/// clipboard the seat's Wayland client sets. That client then changes its clipboard at each
/// round. Yuna's memory does not grow with each change while the manager cannot take them: it
/// grows no more than 4 MiB past what the same changes cost with the manager serving.
#[test]
fn commands_to_a_stalled_manager_are_bounded() {
    const ROUNDS: usize = 1_500;
    if !in_its_own_process("commands_to_a_stalled_manager_are_bounded", &[]) {
        return;
    }
    let mut seat = X11Seat::new("stalled-commands", &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let client = seat.client();
    let (client, ()) = step(&mut seat.compositor, client, |client| client.sync());
    // Served: the changes go to the manager, which takes each.
    churn_clipboard(&mut seat, &mut wl, 200);
    pump_for(&mut seat.compositor, Duration::from_secs(1));
    let served_before = own_anon_kib();
    churn_clipboard(&mut seat, &mut wl, ROUNDS);
    pump_for(&mut seat.compositor, Duration::from_secs(2));
    let served_after = own_anon_kib();
    // Stalled.
    let xwayland = xwayland_of(&seat).expect("Xwayland runs");
    signal(xwayland, Signal::STOP);
    churn_clipboard(&mut seat, &mut wl, 1);
    pump_for(&mut seat.compositor, Duration::from_secs(1));
    let stalled_before = own_anon_kib();
    churn_clipboard(&mut seat, &mut wl, ROUNDS);
    pump_for(&mut seat.compositor, Duration::from_secs(2));
    let stalled_after = own_anon_kib();
    let served = served_after.saturating_sub(served_before);
    let stalled = stalled_after.saturating_sub(stalled_before);
    println!(
        "{ROUNDS} clipboard changes: served, Yuna's anonymous memory grew {served} KiB; manager \
         stalled, {stalled} KiB ({} bytes a change)",
        stalled * 1024 / ROUNDS as u64
    );
    // The client's connection goes with the Xwayland it waits on, stopped or ended meanwhile.
    drop(client);
    assert!(
        stalled < served + 4 * 1024,
        "{stalled} KiB more while the manager is stalled, {served} KiB while it is served"
    );
}

/// How long a manager may owe Yuna's commands before its Xwayland is ended:
/// `xwayland::MANAGER_STALL`, which the library does not export.
const MANAGER_STALL: Duration = Duration::from_secs(10);

/// A manager that stops taking Yuna's commands, its Xwayland stopped (SIGSTOP), is ended with
/// its Xwayland once it owed them [`MANAGER_STALL`], as a manager that fails is: the seat's next
/// X11 program then starts a new Xwayland, which serves it. The seat's Wayland client changes its
/// clipboard meanwhile, every tenth of a second: the manager waits for Xwayland's answer to one,
/// and owes Yuna those that follow.
#[test]
fn a_stalled_managers_xwayland_ends_and_the_next_program_gets_a_new_one() {
    let mut seat = X11Seat::new("stall-end", &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let client = seat.client();
    let (client, ()) = step(&mut seat.compositor, client, |client| client.sync());
    let first = xwayland_of(&seat).expect("Xwayland runs");
    signal(first, Signal::STOP);
    let stopped = Instant::now();
    let deadline = stopped + MANAGER_STALL + PATIENCE;
    let mut changed = stopped;
    churn_clipboard(&mut seat, &mut wl, 1);
    while xwayland_of(&seat) == Some(first) {
        assert!(
            Instant::now() < deadline,
            "the stalled manager's Xwayland still runs"
        );
        if changed.elapsed() >= Duration::from_millis(100) {
            churn_clipboard(&mut seat, &mut wl, 1);
            changed = Instant::now();
        }
        pump(&mut seat.compositor);
    }
    let ended_in = stopped.elapsed();
    println!("the stalled manager's Xwayland ended {ended_in:?} after it stopped");
    assert!(
        ended_in >= MANAGER_STALL,
        "ended after {ended_in:?}, before the manager owed its commands {MANAGER_STALL:?}"
    );
    drop(client);
    let again = seat.client();
    let (again, _, _, _) = open_drawn(&mut seat.compositor, again, &WindowSpec::named("again"));
    let second = xwayland_of(&seat).expect("a new Xwayland serves");
    assert_ne!(second, first, "the next X11 program gets a new Xwayland");
    drop(again);
}

/// The bytes of a title of `length` Latin-1 bytes, each 0xE9 (é): two bytes each in UTF-8.
fn latin1_title(length: usize) -> Vec<u8> {
    vec![0xE9; length]
}

/// debt.md, XWayland: "Unmapped X11 windows are tracked without a bound per client: each holds up
/// to about 192 KiB of text, three titles of 64 KiB, twice, in its manager and in Yuna". An X11
/// client makes 64 windows it never maps, each with its three texts at the 64 KiB a read takes,
/// in Latin-1 where X11 allows it. Yuna keeps 4 KiB of each text, decoded, in its manager and in
/// its state: its memory grows by no more than 40 KiB a window. That is those 24 KiB, the 4 KiB
/// Yuna and its manager keep of any window, measured with texts of a few bytes, and what the
/// allocator keeps of the 64 KiB each read takes: 35 KiB measured, where it was 647 KiB.
#[test]
fn unmapped_windows_hold_at_most_the_debts_figure() {
    const WINDOWS: usize = 64;
    if !in_its_own_process("unmapped_windows_hold_at_most_the_debts_figure", &[]) {
        return;
    }
    let mut seat = X11Seat::new("unmapped", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, ()) = step(&mut seat.compositor, client, |client| client.sync());
    pump_for(&mut seat.compositor, Duration::from_millis(500));
    let xwayland = xwayland_of(&seat).expect("Xwayland runs").to_string();
    let xwayland_before = status_kib(&xwayland, "VmRSS:");
    let before = own_anon_kib();
    let (client, ()) = step(&mut seat.compositor, client, |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let net_wm_name = vec![b'a'; 64 * 1024];
        let mut class = b"x\0".to_vec();
        class.extend(latin1_title(64 * 1024 - 3));
        class.push(0);
        for index in 0..WINDOWS {
            let window = client.create(&WindowSpec::named(&format!("unmapped-{index}")))?;
            let connection = &client.connection;
            connection
                .change_property8(
                    PropMode::REPLACE,
                    window,
                    AtomEnum::WM_NAME,
                    AtomEnum::STRING,
                    &latin1_title(64 * 1024),
                )
                .map_err(fail)?;
            connection
                .change_property8(
                    PropMode::REPLACE,
                    window,
                    client.atoms._NET_WM_NAME,
                    client.atoms.UTF8_STRING,
                    &net_wm_name,
                )
                .map_err(fail)?;
            connection
                .change_property8(
                    PropMode::REPLACE,
                    window,
                    AtomEnum::WM_CLASS,
                    AtomEnum::STRING,
                    &class,
                )
                .map_err(fail)?;
            connection.flush().map_err(fail)?;
        }
        client.sync()
    });
    pump_for(&mut seat.compositor, Duration::from_secs(3));
    let after = own_anon_kib();
    let xwayland_after = status_kib(&xwayland, "VmRSS:");
    let per_window = after.saturating_sub(before) / WINDOWS as u64;
    let xwayland_per_window = xwayland_after.saturating_sub(xwayland_before) / WINDOWS as u64;
    println!(
        "{WINDOWS} unmapped windows with their texts at the bound: Yuna's anonymous memory \
         {before} KiB, then {after} KiB, {per_window} KiB a window; Xwayland's resident set \
         {xwayland_before} KiB, then {xwayland_after} KiB, {xwayland_per_window} KiB a window"
    );
    drop(client);
    assert!(
        per_window <= 40,
        "{per_window} KiB a window, past 2 × 3 texts of 4 KiB and what any window costs"
    );
}

/// ADR-0070's figure for one X11 client's windows in the layout, 1,000, bounds the windows whose
/// properties Yuna follows: an X11 client makes 1,000 windows it does not map, then a thousand and
/// first, which it maps. That one is followed without its properties and counts as past its
/// client's limit: X11 maps it, and it never shows. The first of the thousand, mapped after it,
/// shows.
#[test]
fn a_window_past_its_clients_thousand_followed_never_shows() {
    let mut seat = X11Seat::new("past-thousand", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, first) = step(&mut seat.compositor, client, |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let mut first = None;
        for index in 0..MAX_WINDOWS_PER_CLIENT {
            let window = client.create(&WindowSpec::named(&format!("within-{index}")))?;
            first.get_or_insert(window);
            if index % 64 == 0 {
                client.connection.flush().map_err(fail)?;
            }
        }
        let past = client.create(&WindowSpec {
            background: Some(0x0080_4040),
            ..WindowSpec::named("past")
        })?;
        client.connection.map_window(past).map_err(fail)?;
        client.connection.flush().map_err(fail)?;
        client.wait_for(
            "the window past the limit mapped",
            |event| matches!(event, Event::MapNotify(mapped) if mapped.window == past),
        )?;
        first.ok_or_else(|| "no window made".to_owned())
    });
    // The manager handles X11's events in their order: once a window mapped after it shows, the
    // one past the limit had its turn.
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        client
            .connection
            .change_window_attributes(
                first,
                &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                    .background_pixel(0x0040_8040),
            )
            .map_err(|e| e.to_string())?;
        client
            .connection
            .map_window(first)
            .map_err(|e| e.to_string())?;
        client.sync()
    });
    window_titled(&mut seat.compositor, "within-0");
    let shown = listed(&seat.compositor, "past");
    drop(client);
    assert!(
        !shown,
        "a window past its client's limit of {MAX_WINDOWS_PER_CLIENT} shows"
    );
}

/// The atom named `index`th of a batch, `length` bytes long.
fn long_name(batch: usize, index: usize, length: usize) -> Vec<u8> {
    let mut name = format!("yuna-bounds-{batch}-{index}-").into_bytes();
    name.resize(length, b'x');
    name
}

/// Answers the manager's request for `owner`'s `TARGETS` with `targets`.
fn answer_targets(client: &mut X11Client, owner: Window, targets: &[Atom]) -> Result<(), String> {
    let atoms = atoms_of(client);
    loop {
        let event = client.wait_for(
            "a selection request",
            |event| matches!(event, Event::SelectionRequest(request) if request.owner == owner),
        )?;
        let Event::SelectionRequest(request) = event else {
            unreachable!("a selection request");
        };
        if request.target != atoms.TARGETS {
            notify(client, &request, x11rb::NONE)?;
            continue;
        }
        client
            .connection
            .change_property32(
                PropMode::REPLACE,
                request.requestor,
                request.property,
                AtomEnum::ATOM,
                targets,
            )
            .map_err(|e| e.to_string())?;
        return notify(client, &request, request.property);
    }
}

/// The bridge keeps the name of every atom an X11 owner's `TARGETS` lists, up to 4,096 names
/// (`MAX_NAMES`, `bridge.rs`), whatever their length: X11 allows 65,535 bytes. An X11 client
/// interns 512 atoms of 60,000 bytes and takes the clipboard twice, its `TARGETS` 256 of them each
/// time. None of them is a MIME type, which is 255 bytes at most: Yuna keeps next to nothing of
/// them.
#[test]
fn long_atom_names_in_targets_are_not_kept() {
    const LENGTH: usize = 60_000;
    if !in_its_own_process("long_atom_names_in_targets_are_not_kept", &[]) {
        return;
    }
    let mut seat = X11Seat::new("atom-names", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, ()) = step(&mut seat.compositor, client, |client| client.sync());
    pump_for(&mut seat.compositor, Duration::from_millis(500));
    let before = own_anon_kib();
    let (client, ()) = step(&mut seat.compositor, client, |client| {
        let clipboard = atoms_of(client).CLIPBOARD;
        for batch in 0..2 {
            let cookies = (0..256)
                .map(|index| {
                    client
                        .connection
                        .intern_atom(false, &long_name(batch, index, LENGTH))
                        .map_err(|e| e.to_string())
                })
                .collect::<Result<Vec<_>, String>>()?;
            let atoms = cookies
                .into_iter()
                .map(|cookie| cookie.reply().map(|reply| reply.atom))
                .collect::<Result<Vec<Atom>, _>>()
                .map_err(|e| e.to_string())?;
            let owner = take_selection(client, clipboard)?;
            answer_targets(client, owner, &atoms)?;
        }
        client.sync()
    });
    pump_for(&mut seat.compositor, Duration::from_secs(3));
    let after = own_anon_kib();
    let grown = after.saturating_sub(before);
    println!(
        "512 atoms of {LENGTH} bytes listed in two TARGETS: Yuna's anonymous memory {before} KiB, \
         then {after} KiB, {grown} KiB more"
    );
    drop(client);
    assert!(
        grown < 4 * 1024,
        "Yuna holds {grown} KiB more for names that are no MIME type"
    );
}

/// The variables of Yuna's environment Xwayland keeps (`xwayland/server.rs`), and the prefixes
/// of those it keeps by family.
const XWAYLAND_KEPT: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "LANGUAGE",
    "LOCALE_ARCHIVE",
    "XDG_CACHE_HOME",
    "LD_LIBRARY_PATH",
    "XKB_CONFIG_ROOT",
    "LIBGL_DRIVERS_PATH",
    "LIBGL_ALWAYS_SOFTWARE",
    "GBM_BACKENDS_PATH",
    "__EGL_VENDOR_LIBRARY_FILENAMES",
    "__EGL_VENDOR_LIBRARY_DIRS",
];
const XWAYLAND_KEPT_PREFIXES: &[&str] = &["LC_", "VK_", "MESA_"];

/// Xwayland gets of Yuna's environment only what it and its drivers read, and its Wayland
/// connection: no variable of the program that started Yuna, here the test's own cargo and Nix
/// ones, reaches it, nor what it runs.
#[test]
fn xwaylands_environment_holds_only_what_it_reads() {
    let mut seat = X11Seat::new("xwayland-env", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let xwayland = xwayland_of(&seat).expect("Xwayland runs");
    let environ =
        std::fs::read(format!("/proc/{xwayland}/environ")).expect("Xwayland's environment");
    drop(client);
    let names: Vec<String> = environ
        .split(|&byte| byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let name = entry.split(|&byte| byte == b'=').next().unwrap_or_default();
            String::from_utf8_lossy(name).into_owned()
        })
        .collect();
    let kept = |name: &str| {
        name == "WAYLAND_SOCKET"
            || XWAYLAND_KEPT.contains(&name)
            || XWAYLAND_KEPT_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
    };
    let outside: Vec<&String> = names.iter().filter(|name| !kept(name)).collect();
    println!("Xwayland's environment: {names:?}");
    assert!(
        outside.is_empty(),
        "Xwayland got variables outside its list: {outside:?}"
    );
    assert!(
        names.iter().any(|name| name == "WAYLAND_SOCKET"),
        "Xwayland got no Wayland connection: {names:?}"
    );
}

/// The variable that makes a debug build's X11 manager panic when Yuna asks it to close a
/// window: `PANIC_ON_X11_CLOSE` (`xwayland/manager/commands.rs`), which the library does not
/// export.
#[cfg(debug_assertions)]
const PANIC_ON_X11_CLOSE: &str = "YUNA_PANIC_ON_X11_CLOSE";

/// A panic of a seat's manager's thread is contained (ADR-0080): Yuna's loop goes on, the
/// manager's Xwayland ends, the seat's next X11 program gets a new Xwayland, which serves it,
/// and once that program leaves, Yuna rests. The panic is asked through
/// [`PANIC_ON_X11_CLOSE`], which a debug build alone has, and which holds for every manager of
/// the process it is given to: the story runs in a process of its own
/// ([`in_its_own_process`]).
#[cfg(debug_assertions)]
#[test]
fn a_panic_of_the_manager_ends_its_xwayland_alone() {
    let switch = [(PANIC_ON_X11_CLOSE, "1"), ("RUST_BACKTRACE", "0")];
    if !in_its_own_process("a_panic_of_the_manager_ends_its_xwayland_alone", &switch) {
        return;
    }
    let mut seat = X11Seat::new("manager-panic", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, _, doomed, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("doomed"));
    let first = xwayland_of(&seat).expect("Xwayland runs");
    seat.compositor
        .close_window(doomed.id)
        .expect("Yuna asks the window to close");
    let at = seat.seat;
    wait(
        &mut seat.compositor,
        "the panicked manager's Xwayland ends",
        |compositor| {
            let now = compositor
                .x11_display(at)
                .and_then(|display| display.server);
            (now != Some(first)).then_some(())
        },
    );
    drop(client);
    let again = seat.client();
    let (again, _, _, _) = open_drawn(&mut seat.compositor, again, &WindowSpec::named("again"));
    let second = xwayland_of(&seat).expect("a new Xwayland serves");
    println!("Xwayland {first} ended after its manager panicked; the next is {second}");
    assert_ne!(second, first);
    drop(again);
    wait(&mut seat.compositor, "Xwayland ends, idle", |compositor| {
        let display = compositor.x11_display(at).expect("the seat's display");
        display.server.is_none().then_some(())
    });
    assert_rests_real(&mut seat.compositor);
}
