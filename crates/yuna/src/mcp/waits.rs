//! The `windows`, `events` and `rest` tools, which may wait, and what every call that waits
//! shares: the bound on them, their timers, and what ends one: its time up, its cancellation,
//! its seat closed.
//!
//! A `windows` or `events` call that waits is answered once the layout has placed the window, or
//! the round that recorded the event has ended. On a manual clock, the window is placed only once
//! the clock moves to its output's refresh. A `rest` call with `wait_ms` counts what wakes Yuna
//! until its time is up, its own timer left out (`crate::rest`). At most [`MAX_WAITING_CALLS`]
//! calls wait at once: `windows`, `events`, `rest`, `clipboard get`, `spawn` waiting for its
//! seat's accessibility bus and `accessibility` together.
//!
//! What happens on agents' seats is recorded once a round in each seat's log, `events.rs`: the
//! windows and outputs as the compositor describes them, compared with the last look; the
//! clients the display ended, with the program each belongs to and the protocol error it was
//! ended for, which the program keeps for `spawn list`; the programs reaped. An `events` call
//! reads it from the agent's cursor, or waits, as `windows` does, for a first event that
//! matches, answered at the end of the round that recorded it.
//!
//! The other calls that wait belong to their tools' modules: `clipboard get` to `clipboard.rs`,
//! `capture` to `captures.rs`, `accessibility` and `spawn start` with accessibility to
//! `spawn.rs`; this module ends them too when their time is up, they are cancelled or their seat
//! closes.

use std::time::Duration;

use calloop::RegistrationToken;
use calloop::timer::{TimeoutAction, Timer};
use serde_json::{Map, Value, json};
use yuna_types::SeatId;

use super::Server;
use super::captures::CaptureStage;
use super::clipboard::ReadEnd;
use super::events::{EventData, Kind};
use super::programs::ProgramWakes;
use super::protocol::Revision;
use super::tools::{
    self, EventsCall, MAX_WAIT, MAX_WAITING_CALLS, MAX_WINDOWS_PER_PAGE, RestCall, ToolResult,
    WindowsCall,
};
use crate::MAX_CLIENTS_PER_SEAT;
use crate::compositor::WindowDescription;
use crate::rest::{RestCounts, Source, Wake, insert_source};
use crate::state::OWNER_SEAT;

/// A `windows` call waiting for a window.
#[derive(Debug)]
pub(super) struct WindowWait {
    key: u64,
    id: Value,
    revision: Revision,
    call: WindowsCall,
    timer: RegistrationToken,
}

/// An `events` call waiting for a first event of `seat` after `after` that `kinds` take.
#[derive(Debug)]
pub(super) struct EventWait {
    key: u64,
    id: Value,
    revision: Revision,
    seat: SeatId,
    after: u64,
    kinds: Option<Vec<Kind>>,
    timer: RegistrationToken,
}

/// A `rest` call counting what wakes Yuna until its time is up: what was counted when it came,
/// its seat's or every seat's.
#[derive(Debug)]
pub(super) struct RestWait {
    key: u64,
    id: Value,
    revision: Revision,
    seat: Option<SeatId>,
    wait_ms: u64,
    since: RestCounts,
    programs: Vec<ProgramWakes>,
    timer: RegistrationToken,
}

/// A page of `windows`' reply: its windows, and the cursor of the next page while more wait.
#[derive(Debug, Default)]
struct WindowsPage {
    windows: Vec<Value>,
    next: Option<u64>,
}

impl WindowsPage {
    /// The reply to `call`: its windows, with `next` when the call pages.
    fn json(self, call: &WindowsCall) -> Value {
        let mut reply = json!({ "windows": self.windows });
        if call.pages() {
            reply["next"] = json!(self.next);
        }
        reply
    }
}

impl Server {
    /// `windows`: the result now, or none when the call waits.
    pub(super) fn windows(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call: WindowsCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        if let Err(failed) = self.agent_seat(call.seat) {
            return Some(failed);
        }
        let wait = Duration::from_millis(call.wait_ms);
        if wait > MAX_WAIT {
            return Some(ToolResult::failed(format!(
                "wait_ms is at most {}",
                MAX_WAIT.as_millis()
            )));
        }
        if call
            .limit
            .is_some_and(|limit| !(1..=MAX_WINDOWS_PER_PAGE).contains(&limit))
        {
            return Some(ToolResult::failed(format!(
                "limit is 1 to {MAX_WINDOWS_PER_PAGE}"
            )));
        }
        let page = self.page(&call, &self.compositor.windows());
        // A window that matches but waits for its place is answered once placed, as one that
        // opens later is.
        let placed = !self.compositor.arrangement_pending();
        if wait.is_zero() || (!page.windows.is_empty() && placed) {
            return Some(ToolResult::done(page.json(&call)));
        }
        if self.waiting_calls() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        let key = self.next_wait;
        let timer = match self.arm_wait_timer(key, wait, Source::WaitingCall) {
            Ok(timer) => timer,
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        self.next_wait += 1;
        self.waits.push(WindowWait {
            key,
            id: id.clone(),
            revision,
            call,
            timer,
        });
        None
    }

    /// The page of `windows`, which the compositor lists by id, that `call` asks for: the
    /// windows it matches, [`WindowsCall::page_size`] at most, and the last of them when more
    /// match after it.
    fn page(&self, call: &WindowsCall, windows: &[WindowDescription]) -> WindowsPage {
        let mut matching = windows.iter().filter(|window| call.matches(window));
        let listed: Vec<&WindowDescription> = matching.by_ref().take(call.page_size()).collect();
        let next = match (matching.next(), listed.last()) {
            (Some(_), Some(last)) => Some(last.id.0),
            _ => None,
        };
        WindowsPage {
            windows: listed
                .into_iter()
                .map(|window| self.window_json(call, window))
                .collect(),
            next,
        }
    }

    /// `window` as `call` asks for it: with its surfaces' colours when it asks for them.
    fn window_json(&self, call: &WindowsCall, window: &WindowDescription) -> Value {
        let colours = call
            .colour
            .then(|| self.compositor.window_colours(window.id));
        tools::window_json(window, colours.as_deref())
    }

    /// Answers each waiting call a window now matches.
    pub(super) fn answer_waits(&mut self) {
        let windows = self.compositor.windows();
        let mut index = 0;
        while index < self.waits.len() {
            let call = &self.waits[index].call;
            let page = self.page(call, &windows);
            if page.windows.is_empty() {
                index += 1;
                continue;
            }
            let result = ToolResult::done(page.json(call));
            let wait = self.waits.remove(index);
            self.compositor.loop_handle().remove(wait.timer);
            self.answer_waited(wait.id, wait.revision, result);
        }
    }

    /// `events`: the result now, or none when the call waits. What happened before it is
    /// recorded first, so that a call without `after` waits only for what comes after it.
    pub(super) fn events(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call: EventsCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        let seat = match self.agent_seat(call.seat) {
            Ok(seat) => seat,
            Err(failed) => return Some(failed),
        };
        let wait = Duration::from_millis(call.wait_ms);
        if wait > MAX_WAIT {
            return Some(ToolResult::failed(format!(
                "wait_ms is at most {}",
                MAX_WAIT.as_millis()
            )));
        }
        if call.kinds.as_ref().is_some_and(Vec::is_empty) {
            return Some(ToolResult::failed(
                "kinds names one kind at least: leave it out for every kind",
            ));
        }
        self.record_events();
        let newest = self.event_log.last_seq();
        let after = match call.after {
            Some(after) if after > newest => {
                return Some(ToolResult::failed(format!(
                    "after {after} is past the newest event, {newest}: give a reply's next"
                )));
            }
            Some(after) => after,
            None => newest,
        };
        let reading = self.event_log.read(seat, after, call.kinds.as_deref());
        // A cursor whose events the log dropped is told at once: what it waits for may be
        // among them.
        if wait.is_zero() || !reading.events.is_empty() || reading.missed > 0 {
            return Some(ToolResult::done(tools::events_json(&reading, false)));
        }
        if self.waiting_calls() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        let key = self.next_wait;
        let timer = match self.arm_wait_timer(key, wait, Source::WaitingCall) {
            Ok(timer) => timer,
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        self.next_wait += 1;
        self.event_waits.push(EventWait {
            key,
            id: id.clone(),
            revision,
            seat,
            after,
            kinds: call.kinds,
            timer,
        });
        None
    }

    /// Records in the seats' logs what happened since the last time: outputs and windows as the
    /// compositor describes them now, the clients the display ended, with the program each
    /// belonged to, as resolved when Yuna accepted it (`Server::resolve_arrivals`), which keeps
    /// the protocol error it was ended for while it is remembered, the connections the seats'
    /// sockets refused past their bound, as each seat told them, and the programs reaped.
    /// The windows and outputs are described only when the compositor's description generation
    /// moved since the last look: a round in which only buffers changed, a game drawing,
    /// describes nothing. Each agent seat has a log from its first look; a closed seat's went
    /// with it.
    pub(super) fn record_events(&mut self) {
        for seat in self.compositor.seats() {
            if seat != OWNER_SEAT {
                self.event_log.add_seat(seat);
            }
        }
        let time_ns = self.compositor.now().as_nanos() as u64;
        let generation = self.compositor.description_generation();
        if self.looked_at != Some(generation) {
            self.looked_at = Some(generation);
            self.looks += 1;
            let windows = self.compositor.windows();
            let outputs = self.compositor.output_descriptions();
            let arranged = !self.compositor.arrangement_pending();
            for (seat, data) in self.observer.look(&windows, &outputs, arranged) {
                self.event_log.record(seat, time_ns, data);
            }
        }
        self.resolve_arrivals();
        for departure in self.compositor.take_client_departures() {
            // Dropped for every client, those of seats without a log too.
            let program = self.client_programs.remove(&departure.number);
            if !self.event_log.has_seat(departure.seat) {
                continue;
            }
            if let (Some(program), Some(error), Some((programs, _))) =
                (program, &departure.error, &mut self.programs)
            {
                programs.note_protocol_error(program, error.clone());
            }
            let left = EventData::ClientLeft {
                pid: departure.process.pid,
                program,
                error: departure.error,
            };
            self.event_log.record(departure.seat, time_ns, left);
        }
        for refusal in self.compositor.take_client_refusals() {
            let refused = EventData::ClientRefused {
                bound: MAX_CLIENTS_PER_SEAT,
                refused: refusal.refused,
            };
            self.event_log.record(refusal.seat, time_ns, refused);
        }
        for (seat, program, end) in std::mem::take(&mut self.program_exits) {
            self.event_log
                .record(seat, time_ns, EventData::ProgramExited { program, end });
        }
    }

    /// Answers each `events` call waiting whose seat now has an event it takes, or whose
    /// cursor's events the log has begun to drop.
    pub(super) fn answer_event_waits(&mut self) {
        let mut index = 0;
        while index < self.event_waits.len() {
            let wait = &self.event_waits[index];
            let reading = self
                .event_log
                .read(wait.seat, wait.after, wait.kinds.as_deref());
            if reading.events.is_empty() && reading.missed == 0 {
                index += 1;
                continue;
            }
            let result = ToolResult::done(tools::events_json(&reading, false));
            let wait = self.event_waits.remove(index);
            self.compositor.loop_handle().remove(wait.timer);
            self.answer_waited(wait.id, wait.revision, result);
        }
    }

    /// `rest`: what woke Yuna and what it drew, since it started, or, with `wait_ms`, none now:
    /// the call counts until its time is up.
    pub(super) fn rest(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call: RestCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        let seat = match call.seat.map(|seat| self.agent_seat(seat)).transpose() {
            Ok(seat) => seat,
            Err(failed) => return Some(failed),
        };
        let counts = self.compositor.rest_counts();
        let programs = self.program_wakes();
        let Some(wait_ms) = call.wait_ms else {
            return Some(ToolResult::done(
                self.rest_json(&counts, seat, None, &programs),
            ));
        };
        let wait = Duration::from_millis(wait_ms);
        if wait.is_zero() || wait > MAX_WAIT {
            return Some(ToolResult::failed(format!(
                "wait_ms is from 1 to {}: leave it out for what was counted since Yuna started",
                MAX_WAIT.as_millis()
            )));
        }
        if self.waiting_calls() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        let key = self.next_wait;
        let timer = match self.arm_wait_timer(key, wait, Source::WaitingCall) {
            Ok(timer) => timer,
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        self.next_wait += 1;
        self.rest_waits.push(RestWait {
            key,
            id: id.clone(),
            revision,
            seat,
            wait_ms,
            since: counts,
            programs,
            timer,
        });
        None
    }

    /// What woke Yuna for each program remembered; none without programs.
    fn program_wakes(&self) -> Vec<ProgramWakes> {
        self.programs
            .as_ref()
            .map(|(programs, _)| programs.wakes())
            .unwrap_or_default()
    }

    /// `counts` as `rest` answers, with `programs` and each client's program named.
    fn rest_json(
        &self,
        counts: &RestCounts,
        seat: Option<SeatId>,
        wait_ms: Option<u64>,
        programs: &[ProgramWakes],
    ) -> Value {
        let program_of = |number: u64| self.client_programs.get(&number).copied();
        tools::rest_json(counts, seat, wait_ms, programs, &program_of)
    }

    /// The calls waiting, `windows`', `events`', `clipboard get`'s, the `spawn` starts waiting
    /// for their seat's accessibility bus, `accessibility`'s and `rest`'s together, which
    /// [`MAX_WAITING_CALLS`] bounds.
    pub(super) fn waiting_calls(&self) -> usize {
        self.waits.len()
            + self.event_waits.len()
            + self.clipboard_reads.len()
            + self.spawn_waits.len()
            + self.accessibility.calls_in_flight()
            + self.rest_waits.len()
    }

    /// Arms the timer that ends waiting call `key`'s time `after` from now, on real time:
    /// `source`, [`Source::WaitingCall`] or a capture's [`Source::CaptureTimeout`], names its
    /// wake.
    pub(super) fn arm_wait_timer(
        &self,
        key: u64,
        after: Duration,
        source: Source,
    ) -> Result<RegistrationToken, calloop::Error> {
        let events = self.events.clone();
        insert_source(
            &self.compositor.loop_handle(),
            Wake::of(source),
            Timer::from_duration(after),
            move |_, _, _| {
                events.borrow_mut().timed_out.push(key);
                TimeoutAction::Drop
            },
        )
        .map_err(|e| e.error)
    }

    /// Answers the call `key` whose time is up, if it still waits: no window matched, no event
    /// came, no frame came for a capture, a clipboard read's program did not close its pipe, or
    /// a `rest` call's time to count is over. Its timer woke Yuna alone when the round had
    /// `round_sources` 1.
    pub(super) fn answer_timed_out(&mut self, key: u64, round_sources: u32) {
        if let Some(index) = self.rest_waits.iter().position(|wait| wait.key == key) {
            // Its timer removed itself.
            let wait = self.rest_waits.remove(index);
            let mut window = self.compositor.rest_counts().since(&wait.since);
            window.leave_out_wake(Source::WaitingCall, round_sources == 1);
            let programs = programs_since(&self.program_wakes(), &wait.programs);
            let result =
                ToolResult::done(self.rest_json(&window, wait.seat, Some(wait.wait_ms), &programs));
            self.answer_waited(wait.id, wait.revision, result);
            return;
        }
        if let Some(index) = self.event_waits.iter().position(|wait| wait.key == key) {
            // Its timer removed itself.
            let wait = self.event_waits.remove(index);
            let reading = self
                .event_log
                .read(wait.seat, wait.after, wait.kinds.as_deref());
            // An event recorded in this round, not yet told, comes with it.
            let timed_out = reading.events.is_empty();
            let result = ToolResult::done(tools::events_json(&reading, timed_out));
            self.answer_waited(wait.id, wait.revision, result);
            return;
        }
        if let Some(index) = self.clipboard_reads.iter().position(|read| read.key == key) {
            // What came last is read first: a program that closed its pipe meanwhile is
            // answered whole.
            let end = self
                .drain_clipboard_read(index)
                .unwrap_or(ReadEnd::TimedOut);
            self.answer_clipboard_read(index, end);
            return;
        }
        if let Some(index) = self.captures.iter().position(|capture| {
            capture.key == key && matches!(capture.stage, CaptureStage::Copying(_))
        }) {
            // Its timer removed itself.
            let capture = self.captures.remove(index);
            self.compositor.cancel_capture(capture.ticket);
            let result = ToolResult::failed(format!(
                "no frame came in {} s: nothing changed on the output since a refresh was armed; \
                 on a manual clock, advance it",
                MAX_WAIT.as_secs()
            ));
            self.answer_waited(capture.id, capture.revision, result);
            return;
        }
        let Some(index) = self.waits.iter().position(|wait| wait.key == key) else {
            return;
        };
        // Its timer removed itself.
        let wait = self.waits.remove(index);
        let mut reply = WindowsPage::default().json(&wait.call);
        reply["timed_out"] = json!(true);
        self.answer_waited(wait.id, wait.revision, ToolResult::done(reply));
    }

    /// Drops the waiting calls of request `id`, unanswered, as MCP asks. A seat closing goes on,
    /// unanswered.
    pub(super) fn cancel(&mut self, id: &Value) {
        self.accessibility.cancel(id);
        self.spawn_waits.retain(|wait| &wait.id != id);
        for closing in &mut self.closings {
            if closing
                .reply
                .as_ref()
                .is_some_and(|(request, _)| request == id)
            {
                closing.reply = None;
            }
        }
        while let Some(index) = self.captures.iter().position(|capture| &capture.id == id) {
            self.drop_capture(index);
            tracing::debug!(request = %id, "a capture is cancelled");
        }
        while let Some(index) = self.clipboard_reads.iter().position(|read| &read.id == id) {
            self.drop_clipboard_read(index);
            tracing::debug!(request = %id, "a clipboard read is cancelled");
        }
        let handle = self.compositor.loop_handle();
        self.waits.retain(|wait| {
            let cancelled = &wait.id == id;
            if cancelled {
                handle.remove(wait.timer);
                tracing::debug!(request = %id, "a waiting call is cancelled");
            }
            !cancelled
        });
        self.event_waits.retain(|wait| {
            let cancelled = &wait.id == id;
            if cancelled {
                handle.remove(wait.timer);
                tracing::debug!(request = %id, "a waiting `events` call is cancelled");
            }
            !cancelled
        });
        self.rest_waits.retain(|wait| {
            let cancelled = &wait.id == id;
            if cancelled {
                handle.remove(wait.timer);
                tracing::debug!(request = %id, "a counting `rest` call is cancelled");
            }
            !cancelled
        });
    }

    /// Answers the `windows`, `events`, `rest`, `capture`, `clipboard`, `accessibility` calls and
    /// the `spawn` starts waiting for its bus that wait on `seat`, which closed, and forgets its
    /// captures, its events and its accessibility bus: a stale handle fails plainly
    /// ([ADR-0078](../../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)).
    pub(super) fn fail_waits_of(&mut self, seat: SeatId) {
        self.recent_captures.remove(&seat);
        self.event_log.remove_seat(seat);
        let handle = self.compositor.loop_handle();
        let (closed, counting): (Vec<RestWait>, Vec<RestWait>) =
            std::mem::take(&mut self.rest_waits)
                .into_iter()
                .partition(|wait| wait.seat == Some(seat));
        self.rest_waits = counting;
        for wait in closed {
            handle.remove(wait.timer);
            let result = ToolResult::failed(format!("seat {} was closed", seat.0));
            self.answer_waited(wait.id, wait.revision, result);
        }
        let (closed, waiting): (Vec<EventWait>, Vec<EventWait>) =
            std::mem::take(&mut self.event_waits)
                .into_iter()
                .partition(|wait| wait.seat == seat);
        self.event_waits = waiting;
        for wait in closed {
            handle.remove(wait.timer);
            let result = ToolResult::failed(format!("seat {} was closed", seat.0));
            self.answer_waited(wait.id, wait.revision, result);
        }
        while let Some(index) = self
            .clipboard_reads
            .iter()
            .position(|read| read.seat == seat)
        {
            let read = self.drop_clipboard_read(index);
            let result = ToolResult::failed(format!("seat {} was closed", seat.0));
            self.answer_waited(read.id, read.revision, result);
        }
        for (id, revision, result) in self.accessibility.remove_seat(seat) {
            self.answer_waited(id, revision, result);
        }
        self.fail_spawn_waits(seat, &format!("seat {} was closed", seat.0));
        while let Some(index) = self
            .captures
            .iter()
            .position(|capture| capture.seat == seat)
        {
            let capture = self.drop_capture(index);
            let result = ToolResult::failed(format!("seat {} was closed", seat.0));
            self.answer_waited(capture.id, capture.revision, result);
        }
        let mut index = 0;
        while index < self.waits.len() {
            if self.waits[index].call.seat != seat.0 {
                index += 1;
                continue;
            }
            let wait = self.waits.remove(index);
            handle.remove(wait.timer);
            let result = ToolResult::failed(format!("seat {} was closed", seat.0));
            self.answer_waited(wait.id, wait.revision, result);
        }
    }
}

/// What woke Yuna for each program of `now` since `earlier`: a program started since counts
/// whole.
fn programs_since(now: &[ProgramWakes], earlier: &[ProgramWakes]) -> Vec<ProgramWakes> {
    now.iter()
        .map(|program| {
            let before = earlier
                .iter()
                .find(|before| before.seat == program.seat && before.program == program.program);
            match before {
                Some(before) => ProgramWakes {
                    output: program.output.saturating_sub(before.output),
                    pace: program.pace.saturating_sub(before.pace),
                    ..*program
                },
                None => *program,
            }
        })
        .collect()
}
