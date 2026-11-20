//! Accessibility in `yuna mcp`: each agent's seat has an accessibility bus of its own, on which
//! Yuna is the AT-SPI registry and the one reader, so that an agent reads the accessibility tree
//! of the programs it started with `accessibility: true`, through MCP alone.
//!
//! - `bus.rs`: the seat's `dbus-daemon`, started by the seat's first `spawn start` with
//!   accessibility, never before; its stages up to Yuna owning `org.a11y.atspi.Registry`.
//! - `registry.rs`: Yuna's answers to what toolkits ask of the registry, every call listed.
//! - `reader.rs`: a walk of the applications' trees for one call.
//! - `names.rs`: AT-SPI's names of roles and states.
//!
//! Everything runs on Yuna's loop, through Yuna's own D-Bus connection (`crate::dbus`): the
//! sources of each bus (its daemon's pidfd, its address's pipe, its connection's socket) and the
//! calls' timers only note what came; after each round, [`Accessibility::service`] reads the
//! buses, answers the toolkits, sends the walks' calls and answers the calls that are done.
//!
//! **A call**, `accessibility {seat, window?, pid?, name?, role?, states?, max_nodes?,
//! wait_ms?}`, walks the trees of the applications embedded on the seat's bus, only those whose
//! process is the `pid` or `window`'s given, each application's process being its connection's,
//! as the bus gives it (`GetConnectionUnixProcessID`). A node is given when its role is `role`,
//! its name holds `name`, compared lowercased, and its states hold every one of `states`. Its
//! window is the seat's window of its application's process; when that process has several,
//! the one whose title is the name of the node's window node, the root's child above it. Its
//! rectangle is in its output's logical units from the output's corner, the units `pointer`
//! takes: where Yuna draws the window, plus the node's extents in window coordinates less its
//! window node's ([`reader::Walk::rect_in_window`]): GTK 4 counts them from its window widget's
//! origin, Qt from its window's corner, and Qt on X11 in its own logical pixels, Xwayland's
//! pixels divided by its device pixel ratio, which is Xwayland's scale (ADR-0068).
//!
//! Bounds: at most [`MAX_NODES`] nodes visited by a call, fewer with `max_nodes`;
//! [`reader::MAX_DEPTH`] levels; [`reader::IN_FLIGHT_PER_APP`] calls in flight to one
//! application, [`MAX_IN_FLIGHT`] on one bus; [`reader::MAX_MATCHES`] nodes given;
//! [`MAX_TEXT`] characters of a name or a description given; a reply is the bus's message size
//! at most (`crate::dbus::MAX_MESSAGE`). Each application of a walk has [`APP_DEADLINE`]: one that
//! does not answer by then costs that deadline, is told as such, and never holds the others.
//! A call with `wait_ms`, at most [`MAX_WAIT`] on real time, walks again while nothing matches:
//! once an application embeds, a client of its seat commits or a process is known, no sooner than
//! [`REWALK_AFTER`] after the last walk, and at least every [`REWALK_INTERVAL`]. Its first walk
//! always ends; a later one is cut by the wait's end. Calls count against
//! [`MAX_WAITING_CALLS`](super::tools::MAX_WAITING_CALLS) while in flight, fail when their seat
//! closes, and are dropped by `notifications/cancelled`. At rest, once every call is answered,
//! nothing runs: no timer, no poll.

mod bus;
mod names;
mod reader;
mod registry;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use rustix::process::{Pid, WaitStatus};
use serde_json::{Value, json};
use yuna_layout::{OutputId, SeatId, WindowId};

use super::protocol::Revision;
use super::tools::{AccessibilityCall, MAX_WAIT, ToolResult};
use crate::compositor::{Compositor, OutputDescription, WindowDescription};
use crate::dbus::{
    BUS_INTERFACE, BUS_NAME, BUS_PATH, Body, Connection, Kind, Message, unix_address,
};
use crate::process::{End, kill, reap};
use crate::rest::{Source, Wake, insert_source};
use crate::state::State;
use bus::{BusFiles, START_LIMIT};
use reader::{Filter, Query, Walk};
use registry::{REGISTRY_NAME, Registry};

pub(super) use bus::DAEMON;

/// The most nodes a call visits, all applications together, and its default.
pub const MAX_NODES: usize = 10_000;

/// How long each application of a walk has to answer all its calls: a walk of 10,000 nodes of
/// GTK 4's demo takes far less. An application that does not answer by then is told as such.
pub const APP_DEADLINE: Duration = Duration::from_secs(3);

/// The most calls of walks in flight on one seat's bus, all applications and calls together.
pub const MAX_IN_FLIGHT: usize = 512;

/// The most characters of a node's name or description given.
pub const MAX_TEXT: usize = 1000;

/// The least time between two walks of a waiting call.
pub const REWALK_AFTER: Duration = Duration::from_millis(100);

/// The most time between two walks of a waiting call, when nothing says a tree changed: a state
/// that changes without a commit is found then.
pub const REWALK_INTERVAL: Duration = Duration::from_secs(1);

/// What a bus's sources and the calls' timers noted during a round.
#[derive(Debug, Default)]
struct Notes {
    /// The buses, by number, whose daemon, address or connection has something.
    buses: Vec<u64>,
    /// The calls, by key, whose timer came.
    calls: Vec<u64>,
    /// The buses, by number, whose start's limit came: each is ended unless ready.
    late_starts: Vec<u64>,
}

/// A seat's bus, where it is.
#[derive(Debug)]
enum Stage {
    /// The daemon runs; its address is awaited on its pipe.
    Starting {
        address: OwnedFd,
        source: RegistrationToken,
        read: Vec<u8>,
    },
    /// Yuna's connection authenticates and asks for the registry's name.
    Connecting(Link),
    /// Yuna owns the name: programs may be given the address.
    Ready(Link),
    /// The daemon is killed, for the reason recorded; its end is awaited.
    Ending,
}

/// Yuna's connection to a seat's bus, and its two sources: readable always, writable while
/// bytes wait.
#[derive(Debug)]
struct Link {
    connection: Connection,
    readable: RegistrationToken,
    writable: RegistrationToken,
    writable_on: bool,
}

/// What Yuna waits for, by the serial of its call on a bus: each removed with its reply, which
/// the bus always sends, an error after its reply timeout at the latest.
#[derive(Debug, Clone)]
enum Pending {
    RegistryName,
    Match,
    /// The process of the application on this connection.
    Process(String),
    /// A walk's call: its call's key, its walk's serial, its application, its node and what it
    /// asks.
    Walk {
        call: u64,
        walk: u64,
        app: usize,
        node: usize,
        query: Query,
    },
}

/// One seat's accessibility bus.
#[derive(Debug)]
struct SeatBus {
    number: u64,
    seat: SeatId,
    files: BusFiles,
    address: String,
    pid: Pid,
    pidfd: OwnedFd,
    pidfd_source: RegistrationToken,
    /// The timer of its start's limit, until it is ready or the limit comes.
    start_timer: Option<RegistrationToken>,
    /// How its daemon ended, when `yuna mcp`'s reaping of every child took it first.
    reaped: Option<End>,
    stage: Stage,
    registry: Registry,
    pending: HashMap<u32, Pending>,
    /// Walks' calls in flight.
    in_flight: usize,
    /// Why it is ending, once it is.
    failure: Option<String>,
}

/// Where a seat's bus is, for `spawn start`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BusState {
    /// No bus: the next program with accessibility starts one.
    Absent,
    /// Starting: a program waits for it.
    Starting,
    /// Ready at this address.
    Ready(String),
    /// Being ended, its daemon killed and not yet reaped, for this reason.
    Ending(String),
}

/// What a round of [`Accessibility::service`] did that the server answers.
#[derive(Debug, Default)]
pub(super) struct Outcome {
    /// The seats whose bus became ready.
    pub ready: Vec<SeatId>,
    /// The seats whose bus ended or could not start, and why.
    pub ended: Vec<(SeatId, String)>,
    /// The calls answered.
    pub answers: Vec<(Value, Revision, ToolResult)>,
}

/// A call of `accessibility` until it is answered.
#[derive(Debug)]
struct TreeCall {
    key: u64,
    id: Value,
    revision: Revision,
    seat: SeatId,
    window: Option<WindowId>,
    pid: Option<i32>,
    filter: Filter,
    max_nodes: usize,
    /// When a wait ends; none without one.
    wait_until: Option<Instant>,
    /// Its walk under way.
    walk: Option<UnderWay>,
    /// When its last walk ended: none before the first ends.
    last_end: Option<Instant>,
    /// What its last walk saw when it started: the compositor's commits and the registry's
    /// generation. Either moving says a tree may have changed.
    seen: (u64, u64),
    timer: Option<(RegistrationToken, Instant)>,
}

/// A call's walk under way: its serial, which its calls carry, so that a late reply of an earlier
/// walk of the same call, cut at its deadline or its wait, never reaches it.
#[derive(Debug)]
struct UnderWay {
    serial: u64,
    walk: Walk,
    started: Instant,
}

/// The accessibility buses of the seats and the calls in flight.
pub(super) struct Accessibility {
    handle: LoopHandle<'static, State>,
    notes: Rc<RefCell<Notes>>,
    buses: HashMap<SeatId, SeatBus>,
    next_bus: u64,
    calls: Vec<TreeCall>,
    next_call: u64,
    /// The serial of the next walk, never reused.
    next_walk: u64,
    /// In tests, how many more sources [`Accessibility::watch`] makes before one fails, as one
    /// fails when its descriptor cannot be copied past the process's limit: at 0, the next.
    #[cfg(test)]
    watches_before_failure: std::cell::Cell<Option<usize>>,
}

impl Accessibility {
    pub(super) fn new(handle: LoopHandle<'static, State>) -> Accessibility {
        Accessibility {
            handle,
            notes: Rc::default(),
            buses: HashMap::new(),
            next_bus: 1,
            calls: Vec::new(),
            next_call: 1,
            next_walk: 1,
            #[cfg(test)]
            watches_before_failure: std::cell::Cell::new(None),
        }
    }

    /// Where `seat`'s bus is.
    pub(super) fn bus_state(&self, seat: SeatId) -> BusState {
        let Some(bus) = self.buses.get(&seat) else {
            return BusState::Absent;
        };
        match &bus.stage {
            Stage::Ready(_) => BusState::Ready(bus.address.clone()),
            Stage::Ending => BusState::Ending(bus.failure.clone().unwrap_or_default()),
            Stage::Starting { .. } | Stage::Connecting(_) => BusState::Starting,
        }
    }

    /// The process of `seat`'s daemon, which belongs to the seat's processes.
    pub(super) fn daemon_pid(&self, seat: SeatId) -> Option<Pid> {
        self.buses.get(&seat).map(|bus| bus.pid)
    }

    /// How many calls are in flight: they count among the waiting calls.
    pub(super) fn calls_in_flight(&self) -> usize {
        self.calls.len()
    }

    /// Starts `seat`'s bus in `folder`, its daemon with `environment`, its output to what `log`
    /// makes of its log's path; dbus-daemon is the one `crate::dbus::find_daemon` finds.
    /// Refused, with the reason, when the daemon is not there or cannot start.
    pub(super) fn start_bus(
        &mut self,
        seat: SeatId,
        folder: PathBuf,
        environment: BTreeMap<OsString, OsString>,
        log: impl FnOnce(&std::path::Path) -> std::io::Result<OwnedFd>,
    ) -> Result<(), String> {
        if self.buses.contains_key(&seat) {
            return Ok(());
        }
        let program = crate::dbus::find_daemon()?;
        let files = BusFiles::in_folder(folder);
        bus::check_socket_path(&files.socket)?;
        let address = unix_address(&files.socket);
        let started = bus::start(&program, &files, &address, environment, log)
            .map_err(|error| format!("{DAEMON} could not start: {error}"))?;
        let number = self.next_bus;
        self.next_bus += 1;
        let fail_start = |error: String| {
            kill(&started.pidfd);
            format!("the accessibility bus could not be watched: {error}")
        };
        let pidfd_source = self
            .watch(
                &started.pidfd,
                Interest::READ,
                number,
                Wake::of_seat(Source::AccessibilityDaemon, seat),
            )
            .map_err(fail_start)?;
        let address_source = match self.watch(
            &started.address,
            Interest::READ,
            number,
            Wake::of_seat(Source::AccessibilityAddress, seat),
        ) {
            Ok(source) => source,
            Err(error) => {
                self.handle.remove(pidfd_source);
                return Err(fail_start(error));
            }
        };
        // The limit's own timer says it came, whatever a clock read later would say.
        let notes = self.notes.clone();
        let start_timer = insert_source(
            &self.handle,
            Wake::of_seat(Source::AccessibilityStart, seat),
            Timer::from_duration(START_LIMIT),
            move |_, _, _| {
                notes.borrow_mut().late_starts.push(number);
                TimeoutAction::Drop
            },
        )
        .ok();
        tracing::info!(
            seat = seat.0,
            pid = started.pid.as_raw_nonzero().get(),
            "an accessibility bus starts"
        );
        self.buses.insert(
            seat,
            SeatBus {
                number,
                seat,
                files,
                address,
                pid: started.pid,
                pidfd: started.pidfd,
                pidfd_source,
                start_timer,
                reaped: None,
                stage: Stage::Starting {
                    address: started.address,
                    source: address_source,
                    read: Vec::new(),
                },
                registry: Registry::default(),
                pending: HashMap::new(),
                in_flight: 0,
                failure: None,
            },
        );
        Ok(())
    }

    /// A source on a copy of `fd` that notes bus `number`, named `wake`.
    fn watch(
        &self,
        fd: &impl AsFd,
        interest: Interest,
        number: u64,
        wake: Wake,
    ) -> Result<RegistrationToken, String> {
        #[cfg(test)]
        if let Some(left) = self.watches_before_failure.get() {
            self.watches_before_failure.set(left.checked_sub(1));
            if left == 0 {
                return Err("a source the test refused".to_owned());
            }
        }
        let fd = fd
            .as_fd()
            .try_clone_to_owned()
            .map_err(|error| error.to_string())?;
        let notes = self.notes.clone();
        insert_source(
            &self.handle,
            wake,
            Generic::new(fd, interest, Mode::Level),
            move |_, _, _| {
                notes.borrow_mut().buses.push(number);
                Ok(PostAction::Continue)
            },
        )
        .map_err(|error| error.error.to_string())
    }

    /// Starts a call of `accessibility`: a failure now, or none while it walks or waits.
    pub(super) fn call(
        &mut self,
        id: &Value,
        revision: Revision,
        call: AccessibilityCall,
        compositor: &Compositor,
    ) -> Option<ToolResult> {
        let seat = SeatId(call.seat);
        let wait = Duration::from_millis(call.wait_ms);
        if wait > MAX_WAIT {
            return Some(ToolResult::failed(format!(
                "wait_ms is at most {}",
                MAX_WAIT.as_millis()
            )));
        }
        if call.name.as_deref() == Some("") {
            return Some(ToolResult::failed(
                "name is empty: give a part of the name to find, or leave name out",
            ));
        }
        if let Some(role) = call.role.as_deref().filter(|role| !names::is_role(role)) {
            return Some(ToolResult::failed(format!(
                "{role:?} is no role AT-SPI names: give one as AT-SPI writes it, such as \
                 \"button\", \"check box\", \"text\" or \"frame\""
            )));
        }
        if let Some(state) = call.states.iter().find(|state| !names::is_state(state)) {
            return Some(ToolResult::failed(format!(
                "{state:?} is no state AT-SPI names: give them as AT-SPI writes them, such as \
                 \"focused\", \"checked\", \"sensitive\" or \"showing\""
            )));
        }
        let max_nodes = match call.max_nodes {
            None => MAX_NODES,
            Some(nodes) if (1..=MAX_NODES as u64).contains(&nodes) => nodes as usize,
            Some(_) => {
                return Some(ToolResult::failed(format!(
                    "max_nodes is from 1 to {MAX_NODES}"
                )));
            }
        };
        let window = call.window.map(WindowId);
        let pid = match window {
            None => call.pid,
            Some(window_id) => {
                let windows = compositor.windows();
                let Some(described) = windows
                    .iter()
                    .find(|described| described.id == window_id && described.seat == seat)
                else {
                    return Some(ToolResult::failed(format!(
                        "no window {} on seat {}",
                        window_id.0, seat.0
                    )));
                };
                if call.pid.is_some_and(|pid| described.pid != Some(pid)) {
                    return Some(ToolResult::failed(format!(
                        "window {} is not process {}'s",
                        window_id.0,
                        call.pid.unwrap_or_default()
                    )));
                }
                described.pid
            }
        };
        let ready = matches!(
            self.buses.get(&seat).map(|bus| &bus.stage),
            Some(Stage::Ready(_))
        );
        if !ready && wait.is_zero() {
            return Some(ToolResult::failed(no_bus_message(
                seat,
                self.buses.contains_key(&seat),
            )));
        }
        if let Some(pid) = pid
            && wait.is_zero()
            && !self.buses[&seat]
                .registry
                .apps()
                .iter()
                .any(|app| app.pid == Some(pid))
        {
            return Some(ToolResult::failed(not_on_bus_message(seat, pid, window)));
        }
        let key = self.next_call;
        self.next_call += 1;
        self.calls.push(TreeCall {
            key,
            id: id.clone(),
            revision,
            seat,
            window,
            pid,
            filter: Filter {
                name: call.name,
                role: call.role,
                states: call.states,
            },
            max_nodes,
            wait_until: (!wait.is_zero()).then(|| Instant::now() + wait),
            walk: None,
            last_end: None,
            seen: (0, 0),
            timer: None,
        });
        None
    }

    /// Records how a child that `yuna mcp`'s reaping took ended, when it is a seat's daemon:
    /// its pidfd then finds it reaped, its end unknown.
    pub(super) fn child_reaped(&mut self, pid: Pid, status: WaitStatus) {
        if let Some(bus) = self.buses.values_mut().find(|bus| bus.pid == pid) {
            bus.reaped = End::of(status);
        }
    }

    /// Drops the calls of request `id`, unanswered.
    pub(super) fn cancel(&mut self, id: &Value) {
        let handle = self.handle.clone();
        self.calls.retain(|call| {
            let cancelled = &call.id == id;
            if cancelled && let Some((token, _)) = call.timer {
                handle.remove(token);
            }
            !cancelled
        });
    }

    /// Forgets `seat`'s bus and fails its calls, the seat closing: its daemon is ended among the
    /// seat's processes, its folder removed.
    pub(super) fn remove_seat(&mut self, seat: SeatId) -> Vec<(Value, Revision, ToolResult)> {
        if let Some(bus) = self.buses.remove(&seat) {
            self.drop_sources(&bus);
            bus::remove_folder(&bus.files);
        }
        self.fail_calls(seat, &format!("seat {} was closed", seat.0))
    }

    /// Drops every bus's connection and source, Yuna stopping: its daemons end among every
    /// process under Yuna.
    pub(super) fn stop(&mut self) {
        for (_, bus) in std::mem::take(&mut self.buses) {
            self.drop_sources(&bus);
        }
        for call in std::mem::take(&mut self.calls) {
            if let Some((token, _)) = call.timer {
                self.handle.remove(token);
            }
        }
    }

    fn drop_sources(&self, bus: &SeatBus) {
        self.handle.remove(bus.pidfd_source);
        if let Some(timer) = bus.start_timer {
            self.handle.remove(timer);
        }
        match &bus.stage {
            Stage::Starting { source, .. } => self.handle.remove(*source),
            Stage::Connecting(link) | Stage::Ready(link) => {
                self.handle.remove(link.readable);
                self.handle.remove(link.writable);
            }
            Stage::Ending => {}
        }
    }

    /// Answers the calls of `seat` with `reason`, failed.
    fn fail_calls(&mut self, seat: SeatId, reason: &str) -> Vec<(Value, Revision, ToolResult)> {
        let mut answers = Vec::new();
        let mut index = 0;
        while index < self.calls.len() {
            if self.calls[index].seat != seat {
                index += 1;
                continue;
            }
            let call = self.calls.remove(index);
            if let Some((token, _)) = call.timer {
                self.handle.remove(token);
            }
            answers.push((call.id, call.revision, ToolResult::failed(reason)));
        }
        answers
    }

    /// Whether a round has something for it: a note, or a call whose trees may have changed.
    pub(super) fn has_work(&self) -> bool {
        !self.calls.is_empty() || {
            let notes = self.notes.borrow();
            !notes.buses.is_empty() || !notes.calls.is_empty() || !notes.late_starts.is_empty()
        }
    }

    /// Serves what came: each bus noted, then each call, against `compositor`'s windows.
    pub(super) fn service(&mut self, compositor: &Compositor) -> Outcome {
        let mut outcome = Outcome::default();
        let notes = std::mem::take(&mut *self.notes.borrow_mut());
        // A timer that came has dropped itself.
        for key in notes.calls {
            if let Some(call) = self.calls.iter_mut().find(|call| call.key == key) {
                call.timer = None;
            }
        }
        for number in notes.late_starts {
            let Some(bus) = self.buses.values_mut().find(|bus| bus.number == number) else {
                continue;
            };
            // It dropped itself.
            bus.start_timer = None;
            if !matches!(bus.stage, Stage::Ready(_) | Stage::Ending) {
                let seat = bus.seat;
                let reason = format!(
                    "the accessibility bus was not ready in {} s",
                    START_LIMIT.as_secs()
                );
                self.end_bus(seat, reason);
            }
        }
        let mut noted = notes.buses;
        noted.sort_unstable();
        noted.dedup();
        for number in noted {
            let Some(seat) = self
                .buses
                .values()
                .find(|bus| bus.number == number)
                .map(|bus| bus.seat)
            else {
                continue;
            };
            self.service_bus(seat, &mut outcome);
        }
        if !self.calls.is_empty() {
            self.advance_calls(compositor, &mut outcome);
            let seats: Vec<SeatId> = self.buses.keys().copied().collect();
            for seat in seats {
                self.send_walks(seat);
                self.flush(seat);
            }
        }
        outcome
    }

    /// Serves `seat`'s bus: its daemon's end, its start's limit, its address, its messages.
    fn service_bus(&mut self, seat: SeatId, outcome: &mut Outcome) {
        let Some(bus) = self.buses.get_mut(&seat) else {
            return;
        };
        if let Some(end) = reap(bus.pidfd.as_fd()) {
            self.bus_ended(seat, end, outcome);
            return;
        }
        let Some(bus) = self.buses.get_mut(&seat) else {
            return;
        };
        if let Stage::Starting { address, read, .. } = &mut bus.stage {
            match bus::read_address(address, read) {
                Ok(None) => return,
                Ok(Some(printed)) if bus::is_address(&printed, &bus.address) => {
                    if let Err(error) = self.connect(seat) {
                        self.end_bus(seat, error);
                    }
                }
                Ok(Some(printed)) => {
                    self.end_bus(
                        seat,
                        format!(
                            "{DAEMON} listens at {printed}, not at {}",
                            self.buses[&seat].address
                        ),
                    );
                    return;
                }
                Err(error) => {
                    self.end_bus(seat, error);
                    return;
                }
            }
        }
        self.read_bus(seat, outcome);
        self.flush(seat);
    }

    /// Connects to `seat`'s bus once its daemon listens, and asks for the registry's name and
    /// the bus's signals of names lost.
    fn connect(&mut self, seat: SeatId) -> Result<(), String> {
        let bus = self.buses.get_mut(&seat).ok_or("the bus is gone")?;
        let mut connection = Connection::connect(&bus.files.socket).map_err(|e| e.to_string())?;
        let request = Body::new("su", &(REGISTRY_NAME, 4_u32)).map_err(|e| e.to_string())?;
        let name = connection
            .send(Message::method_call(
                BUS_NAME,
                BUS_PATH,
                BUS_INTERFACE,
                "RequestName",
                request,
            ))
            .map_err(|e| e.to_string())?;
        let rule = Body::new(
            "s",
            "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',\
             member='NameOwnerChanged'",
        )
        .map_err(|e| e.to_string())?;
        let matched = connection
            .send(Message::method_call(
                BUS_NAME,
                BUS_PATH,
                BUS_INTERFACE,
                "AddMatch",
                rule,
            ))
            .map_err(|e| e.to_string())?;
        let number = bus.number;
        let wake = Wake::of_seat(Source::AccessibilityBus, seat);
        let readable = self.watch(&connection.fd(), Interest::READ, number, wake)?;
        // Left alone, the first source would watch its copy of the socket after the daemon is
        // ended: readable for ever, level-triggered, a loop that never rests.
        let writable = match self.watch(&connection.fd(), Interest::WRITE, number, wake) {
            Ok(source) => source,
            Err(error) => {
                self.handle.remove(readable);
                return Err(error);
            }
        };
        let _ = self.handle.disable(&writable);
        let Some(bus) = self.buses.get_mut(&seat) else {
            self.handle.remove(readable);
            self.handle.remove(writable);
            return Err("the bus is gone".to_owned());
        };
        if let Stage::Starting { source, .. } = &bus.stage {
            self.handle.remove(*source);
        }
        bus.pending.insert(name, Pending::RegistryName);
        bus.pending.insert(matched, Pending::Match);
        bus.stage = Stage::Connecting(Link {
            connection,
            readable,
            writable,
            writable_on: false,
        });
        Ok(())
    }

    /// Reads `seat`'s connection, and handles each message.
    fn read_bus(&mut self, seat: SeatId, outcome: &mut Outcome) {
        let Some(bus) = self.buses.get_mut(&seat) else {
            return;
        };
        let messages = match &mut bus.stage {
            Stage::Connecting(link) | Stage::Ready(link) => link.connection.read(),
            _ => return,
        };
        let messages = match messages {
            Ok(messages) => messages,
            Err(error) => {
                self.end_bus(
                    seat,
                    format!("Yuna's connection to the accessibility bus ended: {error}"),
                );
                return;
            }
        };
        for message in messages {
            self.handle_message(seat, message, outcome);
        }
    }

    /// One message from `seat`'s bus: a call to the registry, a reply to Yuna, or a signal.
    fn handle_message(&mut self, seat: SeatId, message: Message, outcome: &mut Outcome) {
        let Some(bus) = self.buses.get_mut(&seat) else {
            return;
        };
        match message.kind {
            Kind::MethodCall => {
                let own_name = match &bus.stage {
                    Stage::Connecting(link) | Stage::Ready(link) => {
                        link.connection.unique_name().unwrap_or_default().to_owned()
                    }
                    _ => return,
                };
                let answer = bus.registry.answer(&message, &own_name);
                if let Some(reply) = answer.reply {
                    send(bus, reply);
                }
                if let Some(name) = answer.embedded {
                    tracing::debug!(seat = seat.0, %name, "an application is embedded");
                    let body = Body::new("s", &name).unwrap_or_default();
                    let call = Message::method_call(
                        BUS_NAME,
                        BUS_PATH,
                        BUS_INTERFACE,
                        "GetConnectionUnixProcessID",
                        body,
                    );
                    match send(bus, call) {
                        Some(serial) => {
                            bus.pending.insert(serial, Pending::Process(name));
                        }
                        // Not asked, its process stays unknown: walked, its nodes without a
                        // window, rather than never walked.
                        None => bus.registry.set_pid(&name, None),
                    }
                }
            }
            Kind::Signal => {
                if let Some(name) = name_left(&message) {
                    bus.registry.name_lost(&name);
                }
            }
            Kind::MethodReturn | Kind::Error => {
                let Some(pending) = message
                    .reply_serial
                    .and_then(|serial| bus.pending.remove(&serial))
                else {
                    return;
                };
                self.take_reply(seat, pending, &message, outcome);
            }
        }
    }

    /// A reply to one of Yuna's calls on `seat`'s bus.
    fn take_reply(
        &mut self,
        seat: SeatId,
        pending: Pending,
        reply: &Message,
        outcome: &mut Outcome,
    ) {
        let Some(bus) = self.buses.get_mut(&seat) else {
            return;
        };
        match pending {
            Pending::RegistryName => match reply.body::<u32>("u") {
                // DBUS_REQUEST_NAME_REPLY_PRIMARY_OWNER.
                Ok(1) => {
                    if let Stage::Connecting(_) = bus.stage {
                        let Stage::Connecting(link) =
                            std::mem::replace(&mut bus.stage, Stage::Ending)
                        else {
                            return;
                        };
                        bus.stage = Stage::Ready(link);
                        if let Some(timer) = bus.start_timer.take() {
                            self.handle.remove(timer);
                        }
                        tracing::info!(seat = seat.0, "the accessibility bus is ready");
                        outcome.ready.push(seat);
                    }
                }
                _ => self.end_bus(
                    seat,
                    format!(
                        "Yuna could not own {REGISTRY_NAME} on the seat's bus: {}",
                        if reply.kind == Kind::Error {
                            reply.error_text()
                        } else {
                            "another process owns it".to_owned()
                        }
                    ),
                ),
            },
            Pending::Match => {
                if reply.kind == Kind::Error {
                    tracing::warn!(error = %reply.error_text(), "the bus refused Yuna's match rule");
                }
            }
            Pending::Process(name) => {
                let pid = reply
                    .body::<u32>("u")
                    .ok()
                    .and_then(|pid| i32::try_from(pid).ok());
                if pid.is_none() {
                    tracing::warn!(%name, error = %reply.error_text(), "an application's process is unknown");
                }
                bus.registry.set_pid(&name, pid);
            }
            Pending::Walk {
                call,
                walk,
                app,
                node,
                query,
            } => {
                bus.in_flight = bus.in_flight.saturating_sub(1);
                deliver(&mut self.calls, (call, walk), app, node, query, reply);
            }
        }
    }

    /// Kills `seat`'s daemon for `reason`: its end, through its pidfd, forgets the bus.
    fn end_bus(&mut self, seat: SeatId, reason: String) {
        let Some(bus) = self.buses.get_mut(&seat) else {
            return;
        };
        tracing::warn!(seat = seat.0, %reason, "the accessibility bus is ended");
        kill(&bus.pidfd);
        if bus.failure.is_none() {
            bus.failure = Some(reason);
        }
        let stage = std::mem::replace(&mut bus.stage, Stage::Ending);
        match stage {
            Stage::Starting { source, .. } => self.handle.remove(source),
            Stage::Connecting(link) | Stage::Ready(link) => {
                self.handle.remove(link.readable);
                self.handle.remove(link.writable);
            }
            Stage::Ending => {}
        }
    }

    /// `seat`'s daemon ended: the bus is forgotten, its socket removed, its calls failed.
    fn bus_ended(&mut self, seat: SeatId, end: End, outcome: &mut Outcome) {
        let Some(bus) = self.buses.remove(&seat) else {
            return;
        };
        self.drop_sources(&bus);
        bus::remove_socket(&bus.files);
        let end = match (end, bus.reaped) {
            (End::Unknown, Some(reaped)) => reaped,
            (end, _) => end,
        };
        let how = match end {
            End::Exited(code) => format!("{DAEMON} exited with status {code}"),
            End::Killed(signal) => format!("{DAEMON} was killed by signal {signal}"),
            End::Unknown => format!("{DAEMON} ended"),
        };
        let reason = match &bus.failure {
            Some(failure) => format!("{failure} ({how}); its log is {}", bus.files.log.display()),
            None => format!(
                "the seat's accessibility bus ended: {how}; its log is {}. The next program \
                 started with `accessibility: true` starts another",
                bus.files.log.display()
            ),
        };
        tracing::warn!(seat = seat.0, %reason, "the accessibility bus is gone");
        outcome.answers.extend(self.fail_calls(seat, &reason));
        outcome.ended.push((seat, reason));
    }

    /// Writes what waits on `seat`'s connection, and watches it for writing while some is left.
    fn flush(&mut self, seat: SeatId) {
        let Some(bus) = self.buses.get_mut(&seat) else {
            return;
        };
        let (Stage::Connecting(link) | Stage::Ready(link)) = &mut bus.stage else {
            return;
        };
        if let Err(error) = link.connection.flush() {
            let reason = format!("Yuna's connection to the accessibility bus ended: {error}");
            self.end_bus(seat, reason);
            return;
        }
        let wants = link.connection.wants_write();
        if wants != link.writable_on {
            let changed = if wants {
                self.handle.enable(&link.writable)
            } else {
                self.handle.disable(&link.writable)
            };
            if changed.is_ok() {
                link.writable_on = wants;
            }
        }
    }

    /// Moves each call on: its walk started, ended at its deadline, or ended and answered or
    /// followed by a wait; a wait ended.
    fn advance_calls(&mut self, compositor: &Compositor, outcome: &mut Outcome) {
        let now = Instant::now();
        let mut windows: Option<(Vec<WindowDescription>, Vec<OutputDescription>)> = None;
        let mut index = 0;
        while index < self.calls.len() {
            let seat = self.calls[index].seat;
            // Its own seat's drawing alone says its trees may have changed.
            let commits = compositor.seat_commits(seat);
            let bus = self.buses.get(&seat);
            let ready = matches!(bus.map(|bus| &bus.stage), Some(Stage::Ready(_)));
            let generation = bus.map_or(0, |bus| bus.registry.generation());
            let call = &mut self.calls[index];
            let wait_over = call.wait_until.is_some_and(|until| now >= until);
            let answer = if let Some(under_way) = &mut call.walk {
                if !under_way.walk.is_done() && now >= under_way.started + APP_DEADLINE {
                    under_way.walk.cut_at_deadline();
                }
                // A first walk always ends; a later one is cut by the wait's end.
                if wait_over && call.last_end.is_some() {
                    under_way.walk.cut_at_wait();
                }
                let ended = under_way.walk.is_done();
                if ended && let Some(UnderWay { walk, .. }) = call.walk.take() {
                    call.last_end = Some(now);
                    let (described, outputs) = windows.get_or_insert_with(|| {
                        (compositor.windows(), compositor.output_descriptions())
                    });
                    let found = call_result(call, &walk, described, outputs);
                    (found.matched || call.wait_until.is_none() || wait_over)
                        .then(|| ToolResult::done(found.value))
                } else {
                    None
                }
            } else if !ready {
                wait_over.then(|| ToolResult::failed(no_bus_message(seat, bus.is_some())))
            } else if wait_over && call.last_end.is_some() {
                let registry = &self.buses[&seat].registry;
                Some(timed_out_result(call, registry))
            } else {
                let due = match call.last_end {
                    None => true,
                    Some(ended) => {
                        let changed = call.seen != (commits, generation);
                        (changed && now >= ended + REWALK_AFTER) || now >= ended + REWALK_INTERVAL
                    }
                };
                if due {
                    let apps: Vec<registry::App> = self.buses[&seat]
                        .registry
                        .apps()
                        .iter()
                        .filter(|app| !app.pid_asked)
                        .filter(|app| call.pid.is_none_or(|pid| app.pid == Some(pid)))
                        .cloned()
                        .collect();
                    let walk = Walk::new(apps, call.filter.clone(), call.max_nodes);
                    let done = walk.is_done();
                    call.walk = Some(UnderWay {
                        serial: self.next_walk,
                        walk,
                        started: now,
                    });
                    self.next_walk += 1;
                    call.seen = (commits, generation);
                    if done {
                        // Nothing to walk, no application on the bus: it ends in this round,
                        // looked at again now rather than at its deadline.
                        continue;
                    }
                }
                None
            };
            match answer {
                Some(result) => {
                    let call = self.calls.remove(index);
                    if let Some((token, _)) = call.timer {
                        self.handle.remove(token);
                    }
                    outcome.answers.push((call.id, call.revision, result));
                }
                None => {
                    let changed = self.calls[index].seen != (commits, generation);
                    self.arm_timer(index, now, changed);
                    index += 1;
                }
            }
        }
    }

    /// Arms call `index`'s one timer at the next time it has to act: its walk's deadline while
    /// it walks; else its next walk, sooner when its trees may have `changed`; its wait's end.
    fn arm_timer(&mut self, index: usize, now: Instant, changed: bool) {
        let call = &self.calls[index];
        // A first walk always ends, its wait's end aside; every time here is still to come, or
        // this round would have acted on it.
        let wait = call
            .wait_until
            .filter(|_| call.last_end.is_some() || call.walk.is_none());
        let next = match (&call.walk, call.last_end) {
            (Some(under_way), _) => Some(under_way.started + APP_DEADLINE),
            (None, Some(ended)) if changed => Some((ended + REWALK_AFTER).max(now)),
            (None, Some(ended)) => Some(ended + REWALK_INTERVAL),
            (None, None) => None,
        };
        let next = match (next, wait) {
            (Some(next), Some(wait)) => Some(next.min(wait)),
            (next, wait) => next.or(wait),
        };
        if call.timer.map(|(_, at)| at) == next {
            return;
        }
        if let Some((token, _)) = call.timer {
            self.handle.remove(token);
        }
        let (key, seat) = (call.key, call.seat);
        let timer = next.and_then(|at| {
            let notes = self.notes.clone();
            insert_source(
                &self.handle,
                Wake::of_seat(Source::AccessibilityCall, seat),
                Timer::from_deadline(at),
                move |_, _, _| {
                    notes.borrow_mut().calls.push(key);
                    TimeoutAction::Drop
                },
            )
            .ok()
            .map(|token| (token, at))
        });
        self.calls[index].timer = timer;
    }

    /// Sends the walks' calls of `seat`, while the bus and each application have room.
    fn send_walks(&mut self, seat: SeatId) {
        let Some(bus) = self.buses.get_mut(&seat) else {
            return;
        };
        if !matches!(bus.stage, Stage::Ready(_)) {
            return;
        }
        for call in self.calls.iter_mut().filter(|call| call.seat == seat) {
            let Some(UnderWay {
                serial: walk_serial,
                walk,
                ..
            }) = &mut call.walk
            else {
                continue;
            };
            let mut progress = true;
            while progress && bus.in_flight < MAX_IN_FLIGHT {
                progress = false;
                for app in 0..walk.apps.len() {
                    if bus.in_flight >= MAX_IN_FLIGHT {
                        break;
                    }
                    let Some((node, query, message)) = walk.next_call(app) else {
                        continue;
                    };
                    progress = true;
                    match send(bus, message) {
                        Some(serial) => {
                            bus.in_flight += 1;
                            bus.pending.insert(
                                serial,
                                Pending::Walk {
                                    call: call.key,
                                    walk: *walk_serial,
                                    app,
                                    node,
                                    query,
                                },
                            );
                        }
                        None => walk.take_unsent(app, node, query),
                    }
                }
            }
        }
    }
}

/// The connection that `signal` says left the bus: `NameOwnerChanged` with no new owner, from the
/// bus itself alone, whose name the bus sets as its sender. Any program may send Yuna a signal of
/// that name; one from a program would make Yuna forget another program's application.
fn name_left(signal: &Message) -> Option<String> {
    let from_bus = signal.sender.as_deref() == Some(BUS_NAME)
        && signal.interface.as_deref() == Some(BUS_INTERFACE)
        && signal.member.as_deref() == Some("NameOwnerChanged");
    if !from_bus {
        return None;
    }
    let (name, _old, new) = signal.body::<(String, String, String)>("sss").ok()?;
    new.is_empty().then_some(name)
}

/// Sends `message` on `bus`'s connection; none when it cannot be, which the caller treats as
/// the call's failure.
fn send(bus: &mut SeatBus, message: Message) -> Option<u32> {
    let (Stage::Connecting(link) | Stage::Ready(link)) = &mut bus.stage else {
        return None;
    };
    match link.connection.send(message) {
        Ok(serial) => Some(serial),
        Err(error) => {
            tracing::warn!(%error, "a message to the accessibility bus is dropped");
            None
        }
    }
}

/// Gives `reply`, to a call of walk `walk` of call `key` (`(key, walk)`), to that walk: a reply
/// of a walk that ended, cut at its deadline or at its wait's end, is dropped, its call gone or a
/// later walk under way. Returns whether the walk took it.
fn deliver(
    calls: &mut [TreeCall],
    (key, walk): (u64, u64),
    app: usize,
    node: usize,
    query: Query,
    reply: &Message,
) -> bool {
    let Some(under_way) = calls
        .iter_mut()
        .find(|call| call.key == key)
        .and_then(|call| call.walk.as_mut())
        .filter(|under_way| under_way.serial == walk)
    else {
        return false;
    };
    under_way.walk.take_reply(app, node, query, reply);
    true
}

/// What a call's walk found, as the tool tells it.
struct Found {
    value: Value,
    matched: bool,
}

/// The result of `call`'s `walk`: the nodes that match, each with its window, its rectangle on
/// its output, and its application; and each application walked.
fn call_result(
    call: &TreeCall,
    walk: &Walk,
    windows: &[WindowDescription],
    outputs: &[OutputDescription],
) -> Found {
    let seat_windows: Vec<&WindowDescription> = windows
        .iter()
        .filter(|window| window.seat == call.seat)
        .collect();
    let windows_of = |pid: Option<i32>| -> Vec<&WindowDescription> {
        seat_windows
            .iter()
            .copied()
            .filter(|window| pid.is_some() && window.pid == pid)
            .collect()
    };
    let mut nodes = Vec::new();
    for (app_index, node_index) in walk.matches() {
        let app = &walk.apps[app_index];
        let node = &app.nodes[node_index];
        let candidates = windows_of(app.app.pid);
        let window = match candidates.as_slice() {
            [only] => Some(*only),
            [] => None,
            several => {
                // A window without a title is one whose node has no name; a title several
                // windows share names none of them.
                let title = walk
                    .window_node(app_index, node_index)
                    .and_then(|window_node| window_node.name.as_deref())
                    .unwrap_or_default();
                let titled: Vec<&WindowDescription> = several
                    .iter()
                    .copied()
                    .filter(|window| window.title.as_deref().unwrap_or_default() == title)
                    .collect();
                match titled.as_slice() {
                    [only] => Some(*only),
                    _ => None,
                }
            }
        };
        if call.window.is_some() && window.map(|window| window.id) != call.window {
            continue;
        }
        let rect = window.and_then(|window| {
            let (x, y, width, height) = walk.rect_in_window(app_index, node_index)?;
            let drawn = window.rect?;
            let output = outputs
                .iter()
                .find(|output| Some(output.id) == window.output)?;
            Some(json!({
                "x": drawn.x - output.rect.x + f64::from(x),
                "y": drawn.y - output.rect.y + f64::from(y),
                "width": width,
                "height": height,
            }))
        });
        nodes.push(json!({
            "window": window.map(|window| window.id.0),
            "app": app.app.pid,
            "role": node.role.map(names::role_name),
            "name": node.name.as_deref().map(clipped),
            "description": node.description.as_deref().map(clipped),
            "states": node.states.as_deref().map(names::state_names).unwrap_or_default(),
            "rect": rect,
            "output": window.and_then(|window| window.output).map(|output: OutputId| output.0),
        }));
    }
    let apps: Vec<Value> = walk
        .apps
        .iter()
        .map(|app| {
            let status = if let Some(failure) = &app.failed {
                format!("failed: {failure}")
            } else if app.timed_out {
                format!("no answer within {} s", APP_DEADLINE.as_secs())
            } else if app.waited_out || !app.is_done() {
                // Not done when its result is made: only the wait's end cuts a walk so.
                "cut at the wait's end".to_owned()
            } else if app.cut {
                "cut at max_nodes".to_owned()
            } else {
                "complete".to_owned()
            };
            json!({
                "pid": app.app.pid,
                "name": app.nodes.first().and_then(|root| root.name.as_deref()).map(clipped),
                "toolkit": app.toolkit,
                "windows": windows_of(app.app.pid).iter().map(|window| window.id.0).collect::<Vec<u64>>(),
                "nodes_visited": app.nodes.len(),
                "status": status,
            })
        })
        .collect();
    let matched = !nodes.is_empty();
    Found {
        value: json!({ "nodes": nodes, "more": walk.has_more(), "apps": apps }),
        matched,
    }
}

/// What a call whose wait ended answers: its plain failure when the process it names never
/// joined the bus, else no node, timed out.
fn timed_out_result(call: &TreeCall, registry: &Registry) -> ToolResult {
    if let Some(pid) = call.pid
        && !registry.apps().iter().any(|app| app.pid == Some(pid))
    {
        return ToolResult::failed(not_on_bus_message(call.seat, pid, call.window));
    }
    ToolResult::done(json!({ "nodes": [], "more": false, "apps": [], "timed_out": true }))
}

/// `text` cut at [`MAX_TEXT`] characters.
fn clipped(text: &str) -> String {
    match text.char_indices().nth(MAX_TEXT) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// What a call on a seat without a bus ready is told.
fn no_bus_message(seat: SeatId, starting: bool) -> String {
    if starting {
        format!(
            "seat {}'s accessibility bus is starting: wait for the program started with \
             `accessibility: true`, or give wait_ms",
            seat.0
        )
    } else {
        format!(
            "seat {} has no accessibility bus: no program was started on it with `spawn start` \
             and `accessibility: true`",
            seat.0
        )
    }
}

/// What a call naming a process not on the seat's bus is told.
fn not_on_bus_message(seat: SeatId, pid: i32, window: Option<WindowId>) -> String {
    let what = match window {
        Some(window) => format!("window {}'s process, {pid},", window.0),
        None => format!("process {pid}"),
    };
    format!(
        "{what} is not on seat {}'s accessibility bus: only programs started with `spawn start` \
         and `accessibility: true` are, once their toolkit has joined it; start it so, or give \
         wait_ms while it starts",
        seat.0
    )
}

#[cfg(test)]
mod tests;
