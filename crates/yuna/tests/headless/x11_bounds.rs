//! What one X11 client can make a seat's X11 window manager hold, and how a manager that stops
//! serving is ended
//! ([ADR-0131](../../../../docs/adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md),
//! [ADR-0080](../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)): a
//! made-up event that x11rb misreads, the commands Yuna owes a manager that no longer takes them,
//! the slice of each round Yuna's loop gives a burst of a manager's events, the windows whose
//! properties a manager follows, the titles and atom names an X11 client makes Yuna keep,
//! Xwayland's environment, and a panic of the manager's thread.
//!
//! Each runs the development shell's Xwayland for one agent's seat, on the real clock. The
//! memory figures are the process's anonymous memory, Yuna's and its managers' threads': a story
//! compares its growth with what a bound allows. Such a story runs alone, in a process of its
//! own ([`in_its_own_process`]): the stories that run beside it in one process would change the
//! figure.

use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConfigureWindowAux, ConnectionExt as _, EventMask, KEYMAP_NOTIFY_EVENT,
    PropMode, Window,
};
use x11rb::wrapper::ConnectionExt as _;
use yuna::rest::{RestCounts, Source};
use yuna::{
    ClockMode, Compositor, MANAGER_SLICE, MANAGER_STALL, MAX_PROPERTIES_PER_CLIENT, STALL_RECHECK,
};
#[cfg(debug_assertions)]
use yuna::{PANIC_ON_X11_CLOSE, X11_OWED_BOUND};
use yuna_ipc::{Caller, Command};

use crate::client::Client;
use crate::selection_client::SelectionClient;
use crate::x11_client::{PATIENCE, WindowSpec, X11Client, step};
use crate::x11_misbehaving::assert_rests_real;
use crate::x11_selections::{atoms_of, notify, open_wayland_window, take_selection};
use crate::x11_windows::{
    TILE, X11Seat, at_1_5, button_press_at, click_at, off, open_drawn, pump, rest, wait,
    window_titled,
};

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

/// Sets the Wayland client's clipboard `rounds` times, one round of Yuna's loop each, to
/// [`many_mime_types`]: [`churn_clipboard_as`].
fn churn_clipboard(seat: &mut X11Seat, wl: &mut SelectionClient, rounds: usize) {
    let types = many_mime_types();
    let types: Vec<&str> = types.iter().map(String::as_str).collect();
    churn_clipboard_as(seat, wl, rounds, &types);
}

/// Sets the Wayland client's clipboard `rounds` times, one round of Yuna's loop each, offering
/// `types`, freeing what the client and Smithay keep of each: the previous source and the offers
/// it is given. Each is one command Yuna owes the seat's manager, which takes it, or which
/// replaces the one it owes.
fn churn_clipboard_as(seat: &mut X11Seat, wl: &mut SelectionClient, rounds: usize, types: &[&str]) {
    let mut previous: Option<usize> = None;
    for _ in 0..rounds {
        let serial = wl.enter_serial();
        let source = wl.set_clipboard(types, serial);
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
    // Measured while the stalled manager lives: its end, which a machine slow enough would
    // bring past `MANAGER_STALL`, frees what it was owed, and the figure would prove nothing.
    assert_eq!(
        xwayland_of(&seat),
        Some(xwayland),
        "the stalled manager's Xwayland was ended before its memory was read"
    );
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

/// A manager that stops taking Yuna's commands, its Xwayland stopped (SIGSTOP), is ended with
/// its Xwayland once it owed them [`MANAGER_STALL`], and still did [`STALL_RECHECK`] later, as a
/// manager that fails is: the seat's next X11 program then starts a new Xwayland, which serves
/// it. The seat's Wayland client changes its clipboard meanwhile, every tenth of a second: the
/// manager waits for Xwayland's answer to one, and owes Yuna those that follow.
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
        ended_in >= MANAGER_STALL + STALL_RECHECK,
        "ended after {ended_in:?}, before the manager owed its commands {MANAGER_STALL:?} and \
         was looked at again {STALL_RECHECK:?} later"
    );
    drop(client);
    let again = seat.client();
    let (again, _, _, _) = open_drawn(&mut seat.compositor, again, &WindowSpec::named("again"));
    let second = xwayland_of(&seat).expect("a new Xwayland serves");
    assert_ne!(second, first, "the next X11 program gets a new Xwayland");
    drop(again);
}

/// A seat whose manager owes Yuna commands, its watchdog armed: its Xwayland is stopped
/// (SIGSTOP), and the seat's Wayland client sets its clipboard five times, a tenth of a second
/// apart. The manager waits for Xwayland's answer to the first it took, and owes Yuna those that
/// follow. Returns the seat, its Wayland client, the X11 program that keeps Xwayland serving,
/// and Xwayland's process id.
fn manager_owing_commands(story: &str) -> (X11Seat, SelectionClient, X11Client, i32) {
    let mut seat = X11Seat::new(story, &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let client = seat.client();
    let (client, ()) = step(&mut seat.compositor, client, |client| client.sync());
    let xwayland = xwayland_of(&seat).expect("Xwayland runs");
    signal(xwayland, Signal::STOP);
    for _ in 0..5 {
        churn_clipboard(&mut seat, &mut wl, 1);
        pump_for(&mut seat.compositor, Duration::from_millis(100));
    }
    (seat, wl, client, xwayland)
}

/// Each source that woke Yuna in `woke`, with its count.
fn wakes_by_source(woke: &RestCounts) -> Vec<(&'static str, u64)> {
    Source::ALL
        .into_iter()
        .map(|source| (source.name(), woke.whole.wakes_by(source)))
        .filter(|&(_, wakes)| wakes > 0)
        .collect()
}

/// A manager that owes Yuna commands at a round's start has its watchdog armed; once it takes
/// them, it says so, and the watchdog is disarmed: Yuna, sleeping as its loop does until
/// something comes, is woken by no watchdog [`MANAGER_STALL`] later, and rests. The manager owes
/// them while its Xwayland is stopped (SIGSTOP), and takes them once Xwayland runs again.
#[test]
fn a_manager_that_took_what_it_owed_leaves_no_watchdog_armed() {
    let (mut seat, wl, client, xwayland) = manager_owing_commands("watchdog-rest");
    signal(xwayland, Signal::CONT);
    // Yuna's loop as it runs, asleep until something comes, rather than `pump`, whose rounds
    // every few milliseconds would each look again at what the manager owes.
    let before = seat.compositor.rest_counts();
    let until = Instant::now() + MANAGER_STALL + Duration::from_secs(2);
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        seat.compositor
            .dispatch(Some(left))
            .expect("Yuna dispatches");
    }
    let woke = seat.compositor.rest_counts().since(&before);
    println!(
        "once the manager could take what it owed, Yuna woke {} times in {:?}: {:?}",
        woke.woke,
        MANAGER_STALL + Duration::from_secs(2),
        wakes_by_source(&woke)
    );
    assert_eq!(
        woke.whole.wakes_by(Source::XwaylandTimeout),
        0,
        "a watchdog stayed armed once its manager took what it owed, and woke Yuna"
    );
    assert_eq!(
        xwayland_of(&seat),
        Some(xwayland),
        "the same Xwayland serves"
    );
    assert_rests_real(&mut seat.compositor);
    drop(client);
    drop(wl);
}

/// When Yuna's whole process is stopped while a manager owes it commands (a debugger, SIGSTOP,
/// a machine swapping hard), the manager's watchdog fires as soon as Yuna runs again, before the
/// manager's thread, which runs again at the same moment, has taken them: Yuna looks once more
/// [`STALL_RECHECK`] later, and the same Xwayland serves on. Here Yuna's loop does not run for
/// longer than [`MANAGER_STALL`] while the manager waits on its stopped Xwayland; Yuna then runs
/// a round, and Xwayland runs again just after.
#[test]
fn a_manager_that_runs_again_with_yuna_is_not_taken_as_stalled() {
    let (mut seat, wl, client, xwayland) = manager_owing_commands("watchdog-resume");
    std::thread::sleep(MANAGER_STALL + Duration::from_secs(1));
    pump(&mut seat.compositor);
    signal(xwayland, Signal::CONT);
    pump_for(&mut seat.compositor, 3 * STALL_RECHECK);
    assert_eq!(
        xwayland_of(&seat),
        Some(xwayland),
        "a manager that ran again with Yuna was taken as stalled, and its Xwayland ended"
    );
    drop(client);
    drop(wl);
}

/// What a manager is owed past its bound ends its Xwayland at once, as a manager that fails is,
/// before its watchdog would: the seat's next X11 program then starts a new Xwayland, which
/// serves it. The bound is lowered to 4 KiB for the story ([`X11_OWED_BOUND`], a debug build's
/// alone), so that a stalled manager reaches it in a few dozen clipboard changes, each of which
/// leaves its place in what is owed; the manager is stalled as in
/// [`a_stalled_managers_xwayland_ends_and_the_next_program_gets_a_new_one`]. The switch holds
/// for every manager of the process it is given to: the story runs in a process of its own.
#[cfg(debug_assertions)]
#[test]
fn a_manager_owed_past_its_bound_ends_its_xwayland_and_the_next_program_gets_a_new_one() {
    const BOUND: usize = 4 * 1024;
    let bound = BOUND.to_string();
    if !in_its_own_process(
        "a_manager_owed_past_its_bound_ends_its_xwayland_and_the_next_program_gets_a_new_one",
        &[(X11_OWED_BOUND, bound.as_str())],
    ) {
        return;
    }
    let mut seat = X11Seat::new("owed-bound", &[at_1_5()], ClockMode::Real, off());
    let mut wl = SelectionClient::connect_to(&mut seat.compositor, seat.seat);
    open_wayland_window(&mut seat, &mut wl);
    let client = seat.client();
    let (client, ()) = step(&mut seat.compositor, client, |client| client.sync());
    let first = xwayland_of(&seat).expect("Xwayland runs");
    signal(first, Signal::STOP);
    let stopped = Instant::now();
    // A change leaves tens of bytes owed: far fewer than `BOUND` changes reach the bound.
    let mut changes = 0;
    while xwayland_of(&seat) == Some(first) {
        assert!(
            changes < BOUND,
            "the stalled manager's Xwayland still runs after {changes} clipboard changes"
        );
        churn_clipboard_as(&mut seat, &mut wl, 1, &["text/plain"]);
        changes += 1;
    }
    let ended_in = stopped.elapsed();
    println!(
        "a manager owed past {BOUND} bytes: its Xwayland ended {ended_in:?} after it stopped, \
         {changes} clipboard changes later"
    );
    assert!(
        ended_in < MANAGER_STALL,
        "ended after {ended_in:?}: by its watchdog, not by its bound"
    );
    drop(client);
    let again = seat.client();
    let (again, _, _, _) = open_drawn(&mut seat.compositor, again, &WindowSpec::named("again"));
    let second = xwayland_of(&seat).expect("a new Xwayland serves");
    assert_ne!(second, first, "the next X11 program gets a new Xwayland");
    drop(again);
}

/// How many requests the story of the managers' slice sends at once: well within the channel that
/// carries a manager's events to Yuna (`EVENT_QUEUE`, 256), so that all of them wait for one
/// round of Yuna's loop.
const BURST: u32 = 200;

/// How many bursts the story of the managers' slice sends at most, while none is answered whole
/// in its second round. Under 32 copies of the story at once, a load of about 60 on 16 threads,
/// about one second round in five was slowed past its slice (2026-11-19): three in a row failed
/// one run in 96.
const BURST_TRIES: usize = 8;

/// What the two rounds of Yuna's loop after a burst of [`BURST`] requests did: how many requests
/// each answered, and how long the second took.
#[derive(Clone, Copy)]
struct BurstRounds {
    first: u64,
    second: u64,
    second_took: Duration,
}

impl BurstRounds {
    /// The two rounds in words, for the story's output.
    fn described(&self) -> String {
        let slice = if self.second_took >= MANAGER_SLICE {
            "past its slice"
        } else {
            "within its slice"
        };
        format!(
            "{} answered, then {} of the {} left in {:?}, {slice}",
            self.first,
            self.second,
            BURST - 1,
            self.second_took
        )
    }
}

/// Runs the compositor's loop until `wayland`'s window `hidden` has had one more frame callback
/// than it had read so far.
fn called_back(seat: &mut X11Seat, wayland: &mut Client, hidden: usize) {
    let frames = wayland.window(hidden).frames.len();
    let deadline = Instant::now() + PATIENCE;
    while wayland.window(hidden).frames.len() == frames {
        assert!(
            Instant::now() < deadline,
            "the hidden window is never called back"
        );
        pump(&mut seat.compositor);
        wayland.read_events();
    }
}

/// Once the seat rests, `wayland`'s window `hidden`, which no output shows, is called back, then
/// asks its next frame callback, due [`yuna::HIDDEN_FRAME_INTERVAL`] later. Yuna's loop then stands
/// still while `client` asks [`BURST`] times that its window `asking`, never mapped, be resized,
/// and its manager posts each request, until the callback is overdue. Then two rounds of the loop
/// run, and the loop runs on until every request is answered. Each answer is one of Yuna's
/// configures.
fn burst_after_a_deadline_due(
    seat: &mut X11Seat,
    client: &X11Client,
    asking: Window,
    wayland: &mut Client,
    hidden: usize,
) -> BurstRounds {
    rest(seat);
    wayland.request_frame(hidden);
    wayland.commit(hidden);
    wayland.send();
    called_back(seat, wayland, hidden);
    wayland.request_frame(hidden);
    wayland.commit(hidden);
    wayland.roundtrip(&mut seat.compositor);
    let due = seat
        .compositor
        .next_deadline()
        .expect("the hidden window's callback is due later");
    for n in 0..BURST {
        client
            .connection
            .configure_window(asking, &ConfigureWindowAux::new().width(100 + n))
            .expect("the request is sent");
    }
    client.sync().expect("X11 handled the requests");
    std::thread::sleep(due.saturating_sub(seat.compositor.now()) + Duration::from_millis(300));

    let answered = |seat: &X11Seat| seat.compositor.counters().configures;
    let before = answered(seat);
    let frames = wayland.window(hidden).frames.len();
    seat.compositor
        .dispatch(Some(Duration::ZERO))
        .expect("Yuna dispatches");
    let first = answered(seat) - before;
    wayland.read_events();
    assert_eq!(
        wayland.window(hidden).frames.len(),
        frames + 1,
        "the overdue callback is sent in the first round"
    );
    assert_eq!(
        seat.compositor.next_deadline(),
        None,
        "no deadline is armed for the second round"
    );
    let started = Instant::now();
    seat.compositor
        .dispatch(Some(Duration::ZERO))
        .expect("Yuna dispatches");
    let second_took = started.elapsed();
    let second = answered(seat) - before - first;
    let deadline = Instant::now() + PATIENCE;
    while answered(seat) - before < u64::from(BURST) {
        assert!(Instant::now() < deadline, "the requests are never answered");
        pump(&mut seat.compositor);
    }
    BurstRounds {
        first,
        second,
        second_took,
    }
}

/// debt.md, XWayland: "no story sees the whole flood handled in one round, nor tests
/// `manager_slice_end`'s wiring". A round of Yuna's loop handles its managers' events in a slice
/// that ends [`MANAGER_SLICE`] after it starts, or once a deadline armed is near
/// (`manager_slice_end`). An X11 client asks [`BURST`] times at once that a window be resized,
/// while a frame callback of a hidden Wayland window is overdue: the next round answers one
/// request, its slice over at once, and sends the callback; the round after it, nothing armed,
/// answers the [`BURST`] − 1 others, well within its slice. A second round that answers fewer was
/// slowed past its slice by a loaded machine, or met a manager that had not posted every request
/// when the first round read its channel: the burst runs again, [`BURST_TRIES`] times at most. A
/// slice that Yuna ends too soon leaves every burst short, and the story fails; one that ignores
/// the deadline answers the whole burst in the first round, and the story fails at once.
#[test]
fn a_burst_of_x11_requests_waits_for_a_deadline_due_then_goes_whole_in_one_round() {
    let mut seat = X11Seat::new("slice", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, asking) = step(&mut seat.compositor, client, |client| {
        let window = client.create(&WindowSpec::named("asking"))?;
        client.sync()?;
        Ok(window)
    });
    // The owner's seat has no output: its window is hidden.
    let mut wayland = Client::connect(&mut seat.compositor);
    let hidden = wayland.open_window(&mut seat.compositor);
    wayland.ack(hidden);
    wayland.attach(hidden, 64, 64, 0xFF_80_80_80);
    wayland.commit(hidden);
    wayland.roundtrip(&mut seat.compositor);

    let mut bursts = Vec::new();
    loop {
        let rounds = burst_after_a_deadline_due(&mut seat, &client, asking, &mut wayland, hidden);
        bursts.push(rounds);
        assert_eq!(
            rounds.first, 1,
            "the round that met an overdue deadline answered {} requests, not one",
            rounds.first
        );
        if rounds.second == u64::from(BURST) - 1 {
            break;
        }
        let described: Vec<String> = bursts.iter().map(BurstRounds::described).collect();
        assert!(
            bursts.len() < BURST_TRIES,
            "{BURST_TRIES} bursts, none answered whole in its second round: {described:?}"
        );
    }
    let described: Vec<String> = bursts.iter().map(BurstRounds::described).collect();
    println!(
        "{BURST} X11 requests at once, one round's slice ended by a deadline due: {described:?}"
    );
    drop(client);
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

/// [`MAX_PROPERTIES_PER_CLIENT`], ADR-0131's figure for one X11 client's windows in the layout,
/// 1,000, bounds the windows whose properties Yuna follows: an X11 client makes 1,000 windows it
/// does not map, then a thousand and first, which it maps. That one is followed without its
/// properties and counts as past its client's limit: X11 maps it, and it never shows. The first
/// of the thousand, mapped after it, shows.
#[test]
fn a_window_past_its_clients_thousand_followed_never_shows() {
    let mut seat = X11Seat::new("past-thousand", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, first) = step(&mut seat.compositor, client, |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let mut first = None;
        for index in 0..MAX_PROPERTIES_PER_CLIENT {
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
        "a window past its client's limit of {MAX_PROPERTIES_PER_CLIENT} shows"
    );
}

/// A window past its client's thousand, which the manager maps itself and Yuna is never told
/// of, lies below every window Yuna stacks, as a window past its client's limit in the layout
/// does, and is not in `_NET_CLIENT_LIST`: a click where it lies reaches the X11 window Yuna
/// shows there, another program's. Two such windows cover the whole output: one mapped before
/// that window opens, which Yuna then stacks, and one made and mapped after Yuna stacked it.
#[test]
fn a_window_past_its_clients_thousand_takes_no_click_from_a_window_shown() {
    let mut seat = X11Seat::new("past-click", &[at_1_5()], ClockMode::Real, off());
    let output = seat.outputs[0];
    // The whole output, 2880 × 1620 X11 pixels at 1.5.
    let covering = |title: &str| WindowSpec {
        background: Some(0x0080_4040),
        rect: (0, 0, 2880, 1620),
        ..WindowSpec::named(title)
    };
    let hostile = seat.client();
    let mapped_before = covering("past-before");
    let (hostile, (first_within, before)) = step(&mut seat.compositor, hostile, move |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let mut first = None;
        for index in 0..MAX_PROPERTIES_PER_CLIENT {
            let window = client.create(&WindowSpec::named(&format!("within-{index}")))?;
            first.get_or_insert(window);
            if index % 64 == 0 {
                client.connection.flush().map_err(fail)?;
            }
        }
        let before = client.open(&mapped_before)?;
        Ok((first.ok_or("no window made")?, before))
    });
    let shown = seat.client();
    let (shown, window, _, _) =
        open_drawn(&mut seat.compositor, shown, &WindowSpec::named("shown"));
    // Yuna stacks the window it shows from the bottom up, below the windows it does not list,
    // which a window made earlier, its client's first, shows in X11's own stacking.
    let (hostile, ()) = step(&mut seat.compositor, hostile, move |client| {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let children = client
                .connection
                .query_tree(client.root)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?
                .children;
            let place = |of: Window| children.iter().position(|&child| child == of);
            if matches!((place(window), place(first_within)), (Some(a), Some(b)) if a < b) {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err("Yuna never stacked the window it shows".to_owned());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    let mapped_after = covering("past-after");
    let (hostile, after) = step(&mut seat.compositor, hostile, move |client| {
        client.open(&mapped_after)
    });
    click_at(&mut seat, output, TILE.x + 100.25, TILE.y + 50.25);
    let (shown, _) = button_press_at(&mut seat.compositor, shown, window);
    let (hostile, events) = step(&mut seat.compositor, hostile, |client| client.take_events());
    let pressed: Vec<Window> = events
        .iter()
        .filter_map(|event| match event {
            Event::ButtonPress(press) => Some(press.event),
            _ => None,
        })
        .collect();
    assert!(
        pressed.is_empty(),
        "windows past the limit, {before} and {after}, got clicks: {pressed:?}"
    );
    let (shown, listed) = step(&mut seat.compositor, shown, |client| client.client_list());
    assert_eq!(
        listed,
        vec![window],
        "_NET_CLIENT_LIST lists the window shown alone"
    );
    drop(shown);
    drop(hostile);
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
        .command(
            Caller::on_seat(seat.seat),
            Command::CloseWindow(Some(doomed.id)),
        )
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
