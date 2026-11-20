//! Measures what the rest auditor costs Yuna (`rest.rs`), at 10 and 100 clients: naming the
//! clients that woke it, a wait of no time on the epoll of their sockets' second descriptors,
//! alone, a whole round of Yuna's loop in which one client sent a request, and rounds in which a
//! client committed a surface, every count the round feeds included: the figures
//! `docs/measurements.md` keeps.
//!
//! It runs the compositor in its own process, on a manual clock, with one agent seat and its
//! output, 640 × 480 at 60 Hz and scale 1, which nothing draws on. Its clients connect through
//! socket pairs and speak Wayland's wire themselves: each request is a `wl_display.sync`, 12
//! bytes, which Yuna answers with `wl_callback.done` and `wl_display.delete_id`. For each count
//! it times, `--rounds` times each, 10,000 by default:
//!
//! - **naming, one ready**: the naming alone, one client's request waiting, as at a wake by one
//!   client among many: a game, or a program that animates;
//! - **naming, all ready**: the same, each client's request waiting;
//! - **round, one ready**: `Compositor::dispatch` while one client's request waits, each client
//!   in turn: the loop's wake, the naming, the display reading and answering the request, and
//!   the round's end;
//! - **round, one commit**: the same while one client's `wl_surface.commit` of a surface without
//!   a role waits: Yuna's commit, its client named and its commit counted;
//! - **round, 100 commits**: the same with 100 commits waiting, in one write: what a commit
//!   costs is its difference with one commit's, divided by 99.
//!
//! It checks that the round's wakes and commits were each named, to the client. It prints the median, the
//! 99th percentile and the longest of each, in microseconds, and the load of the machine before
//! and after, which a figure taken while other programs load the processor reports beside it.
//! Build it for release, then run it in the development shell, alone among measurements; it
//! needs no screen:
//!
//! ```sh
//! nix develop -c cargo build --release -p yuna --example rest_cost
//! nix develop -c target/release/examples/rest_cost
//! ```

#[path = "common/timing.rs"]
mod timing;

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use yuna::rest::Source;
use yuna::{ClockMode, Compositor, Config, OutputMode};
use yuna_types::SeatId;

/// The client counts measured.
const COUNTS: [usize; 2] = [10, 100];

fn main() -> std::process::ExitCode {
    let rounds = match timing::read_rounds(std::env::args().skip(1), 10_000, "rest_cost") {
        Ok(rounds) => rounds,
        Err(reason) => {
            eprintln!("rest_cost: {reason}");
            return std::process::ExitCode::from(2);
        }
    };
    println!("load before: {}", timing::load_average());
    println!("clients  case                median_us  p99_us  longest_us");
    for count in COUNTS {
        if let Err(reason) = measure(count, rounds) {
            eprintln!("rest_cost: {reason}");
            return std::process::ExitCode::FAILURE;
        }
    }
    println!("load after: {}", timing::load_average());
    std::process::ExitCode::SUCCESS
}

/// Connects `count` clients to a seat of a compositor of its own, then times the naming and the
/// rounds.
fn measure(count: usize, rounds: usize) -> Result<(), String> {
    let failed = |what: &str, error: &dyn std::fmt::Display| format!("{what}: {error}");
    let mut compositor = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Manual,
        listen: false,
        announce_owner_seat: false,
        ..Config::default()
    })
    .map_err(|e| failed("the compositor", &e))?;
    let seat = compositor.add_seat().map_err(|e| failed("a seat", &e))?;
    let mode = OutputMode::new(640, 480, 60.0, 1.0).map_err(|e| failed("a mode", &e))?;
    compositor
        .add_output(seat, mode)
        .map_err(|e| failed("an output", &e))?;
    let mut clients = Vec::with_capacity(count);
    for _ in 0..count {
        clients.push(RawClient::connect(&mut compositor, seat)?);
    }
    for client in &mut clients {
        client.sync()?;
        answer(&mut compositor, client)?;
    }

    let mut one = Vec::with_capacity(rounds);
    clients[0].sync()?;
    for _ in 0..rounds {
        one.push(timing::time(|| compositor.count_readable_clients_now()));
    }
    answer(&mut compositor, &mut clients[0])?;
    let mut all = Vec::with_capacity(rounds);
    for client in &mut clients {
        client.sync()?;
    }
    for _ in 0..rounds {
        all.push(timing::time(|| compositor.count_readable_clients_now()));
    }
    for client in &mut clients {
        answer(&mut compositor, client)?;
    }

    let before = compositor.rest_counts();
    let mut round = Vec::with_capacity(rounds);
    for index in 0..rounds {
        let client = &mut clients[index % count];
        client.sync()?;
        let mut dispatched = Ok(());
        round.push(timing::time(|| {
            dispatched = compositor.dispatch(Some(Duration::ZERO));
        }));
        dispatched.map_err(|e| failed("the compositor", &e))?;
        client.read_answer()?;
    }
    let window = compositor.rest_counts().since(&before);
    let named: u64 = window
        .clients
        .values()
        .map(|client| client.tally.wakes)
        .sum();
    let woke = window.whole.wakes_by(Source::Clients);
    if named != rounds as u64 || woke != rounds as u64 || window.unnamed_client_wakes != 0 {
        return Err(format!(
            "{rounds} rounds woke Yuna {woke} times for clients, {named} named, {} unnamed",
            window.unnamed_client_wakes
        ));
    }
    let commits = commit_rounds(&mut compositor, &mut clients[0], rounds)?;
    for (case, times) in [
        ("naming, one ready", one),
        ("naming, all ready", all),
        ("round, one ready", round),
        ("round, one commit", commits.0),
        ("round, 100 commits", commits.1),
    ] {
        let [median, p99, longest] = timing::summary(times);
        println!("{count:>7}  {case:<18}  {median:>9.2}  {p99:>6.2}  {longest:>10.2}");
    }
    Ok(())
}

/// How many commits the batched case sends a round.
const BATCH: usize = 100;

/// Times rounds in which `client`, given a surface, sent one commit, then [`BATCH`] commits;
/// checks that each commit was counted to it.
fn commit_rounds(
    compositor: &mut Compositor,
    client: &mut RawClient,
    rounds: usize,
) -> Result<(Vec<Duration>, Vec<Duration>), String> {
    let surface = client.make_surface(compositor)?;
    let before = compositor.rest_counts();
    let mut times = (Vec::with_capacity(rounds), Vec::with_capacity(rounds));
    for batch in [1, BATCH] {
        for _ in 0..rounds {
            client.commit(surface, batch)?;
            let mut dispatched = Ok(());
            let took = timing::time(|| {
                dispatched = compositor.dispatch(Some(Duration::ZERO));
            });
            dispatched.map_err(|e| format!("the compositor: {e}"))?;
            if batch == 1 {
                times.0.push(took);
            } else {
                times.1.push(took);
            }
        }
    }
    let window = compositor.rest_counts().since(&before);
    let counted: u64 = window
        .clients
        .values()
        .map(|client| client.tally.commits)
        .sum();
    let sent = (rounds * (1 + BATCH)) as u64;
    if counted != sent {
        return Err(format!("{sent} commits sent, {counted} counted"));
    }
    Ok(times)
}

/// Lets the compositor read what `client` sent and answer it, then reads the answer.
fn answer(compositor: &mut Compositor, client: &mut RawClient) -> Result<(), String> {
    compositor
        .dispatch(Some(Duration::ZERO))
        .map_err(|e| format!("the compositor: {e}"))?;
    client.read_answer()
}

/// A client of a seat that speaks Wayland's wire itself: `wl_display.sync`, and nothing else.
struct RawClient {
    stream: UnixStream,
    next_callback: u32,
}

impl RawClient {
    fn connect(compositor: &mut Compositor, seat: SeatId) -> Result<RawClient, String> {
        let stream = compositor
            .connect_client_to(seat)
            .map_err(|e| format!("a client: {e}"))?;
        stream
            .set_nonblocking(true)
            .map_err(|e| format!("a client: {e}"))?;
        Ok(RawClient {
            stream,
            next_callback: 2,
        })
    }

    /// A new object's id: ids are never reused.
    fn new_id(&mut self) -> u32 {
        let id = self.next_callback;
        self.next_callback += 1;
        id
    }

    /// Sends `wl_display.sync` with a new callback: object 1, opcode 0, 12 bytes.
    fn sync(&mut self) -> Result<(), String> {
        let callback = self.new_id();
        self.send(&message(1, 0, &[callback]))
    }

    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.stream
            .write_all(bytes)
            .map_err(|e| format!("a client's request: {e}"))
    }

    /// Binds `wl_compositor` and makes a surface with no role, whose commits Yuna handles and
    /// draws nothing for; returns its id.
    fn make_surface(&mut self, compositor: &mut Compositor) -> Result<u32, String> {
        let registry = self.new_id();
        let callback = self.new_id();
        self.send(&[message(1, 1, &[registry]), message(1, 0, &[callback])].concat())?;
        compositor
            .dispatch(Some(Duration::ZERO))
            .map_err(|e| format!("the compositor: {e}"))?;
        let mut events = Vec::new();
        let mut bytes = [0; 4096];
        loop {
            match self.stream.read(&mut bytes) {
                Ok(0) => return Err("a client's connection closed".to_owned()),
                Ok(read) => events.extend_from_slice(&bytes[..read]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(format!("a client's events: {error}")),
            }
        }
        let name = global_name(&events, registry, "wl_compositor")
            .ok_or_else(|| "no wl_compositor among the globals".to_owned())?;
        let bound = self.new_id();
        let surface = self.new_id();
        let mut bind = vec![name];
        bind.extend(wire_string("wl_compositor"));
        bind.extend([4, bound]);
        self.send(&[message(registry, 0, &bind), message(bound, 0, &[surface])].concat())?;
        compositor
            .dispatch(Some(Duration::ZERO))
            .map_err(|e| format!("the compositor: {e}"))?;
        Ok(surface)
    }

    /// Sends `count` commits of `surface` in one write: opcode 6, 8 bytes each.
    fn commit(&mut self, surface: u32, count: usize) -> Result<(), String> {
        self.send(&message(surface, 6, &[]).repeat(count))
    }

    /// Reads every byte Yuna sent, which holds its answer once it has dispatched.
    fn read_answer(&mut self) -> Result<(), String> {
        let mut bytes = [0; 4096];
        let mut read = 0;
        loop {
            match self.stream.read(&mut bytes) {
                Ok(0) => return Err("a client's connection closed".to_owned()),
                Ok(more) => read += more,
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(format!("a client's answer: {error}")),
            }
        }
        if read == 0 {
            return Err("Yuna did not answer a client".to_owned());
        }
        Ok(())
    }
}

/// A Wayland request or event: its object, its size and opcode, its arguments as words.
fn message(object: u32, opcode: u32, arguments: &[u32]) -> Vec<u8> {
    let size = 8 + 4 * arguments.len() as u32;
    let mut bytes = Vec::with_capacity(size as usize);
    for word in [object, size << 16 | opcode].iter().chain(arguments) {
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    bytes
}

/// A string as Wayland's wire carries it, in words: its length with its NUL, then its bytes,
/// padded to a word.
fn wire_string(text: &str) -> Vec<u32> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    let length = bytes.len() as u32;
    bytes.resize(bytes.len().div_ceil(4) * 4, 0);
    std::iter::once(length)
        .chain(
            bytes
                .chunks(4)
                .map(|word| u32::from_ne_bytes([word[0], word[1], word[2], word[3]])),
        )
        .collect()
}

/// The name of the global `interface` among `registry`'s `global` events in `events`.
fn global_name(events: &[u8], registry: u32, interface: &str) -> Option<u32> {
    let word = |at: usize| -> Option<u32> {
        let bytes = events.get(at..at + 4)?;
        Some(u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    };
    let mut at = 0;
    while at + 8 <= events.len() {
        let (object, header) = (word(at)?, word(at + 4)?);
        let size = (header >> 16) as usize;
        if size < 8 {
            return None;
        }
        if object == registry && header & 0xffff == 0 {
            let length = word(at + 12)? as usize;
            let text = events.get(at + 16..at + 16 + length.saturating_sub(1))?;
            if text == interface.as_bytes() {
                return word(at + 8);
            }
        }
        at += size;
    }
    None
}
