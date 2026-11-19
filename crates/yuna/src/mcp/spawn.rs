//! The `spawn` and `accessibility` tools, and the programs' life in the server's loop: their
//! output read again once its throttle ends, and their end when Yuna stops.
//!
//! A program started with `accessibility: true` joins its seat's accessibility bus, which the
//! seat's first such program starts, and on which Yuna is the AT-SPI registry; `accessibility`
//! reads the trees there (`accessibility/`). A `spawn start` with accessibility waits for its
//! seat's bus, and is answered once the program has started, or with the bus's failure.
//!
//! The programs themselves, their environment, logs, pace and processes, are `programs/`'s;
//! this module is what the server does with them.

use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::time::Instant;

use rustix::process::Signal;
use serde_json::{Map, Value, json};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use yuna_layout::SeatId;

use super::Server;
use super::accessibility::{BusState, Outcome};
use super::programs::{self, END_GRACE, KILL_WAIT, Programs, Start, THROTTLE};
use super::protocol::Revision;
use super::tools::{self, AccessibilityCall, MAX_WAITING_CALLS, SpawnCall, ToolResult};
use crate::rest::{Source, Wake, insert_source};
use crate::state::OWNER_SEAT;

/// A `spawn start` with accessibility, waiting for its seat's bus.
#[derive(Debug)]
pub(super) struct SpawnWait {
    pub(super) id: Value,
    revision: Revision,
    seat: SeatId,
    start: Start,
}

impl Server {
    /// `spawn`: the result now, or none when a start waits for its seat's accessibility bus.
    pub(super) fn spawn(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        if let SpawnCall::Start {
            seat,
            accessibility: true,
            ..
        } = &call
        {
            let seat = SeatId(*seat);
            if seat != OWNER_SEAT && self.compositor.seats().contains(&seat) {
                return self.start_accessible(id, revision, seat, call);
            }
        }
        Some(self.spawn_now(call))
    }

    /// A `spawn start` with accessibility on `seat`: started now when its bus is ready; else it
    /// waits for the bus, started first when there is none.
    fn start_accessible(
        &mut self,
        id: &Value,
        revision: Revision,
        seat: SeatId,
        call: SpawnCall,
    ) -> Option<ToolResult> {
        let SpawnCall::Start {
            command, env, cwd, ..
        } = call
        else {
            return Some(ToolResult::failed("not a start"));
        };
        let Some((programs, _)) = &self.programs else {
            return Some(ToolResult::failed(
                "this Yuna starts no program: run `yuna mcp`",
            ));
        };
        let mut start = Start {
            command,
            env,
            cwd: cwd.map(PathBuf::from),
            accessibility: None,
        };
        let state = self.accessibility.bus_state(seat);
        if let BusState::Ready(address) = state {
            start.accessibility = Some(address);
            return Some(self.start_program(seat, start));
        }
        if let BusState::Ending(reason) = state {
            return Some(ToolResult::failed(format!(
                "seat {}'s accessibility bus is being ended ({reason}): start the program again \
                 in a moment, and a new bus starts for it",
                seat.0
            )));
        }
        // Refused before a bus starts for it.
        if self.waiting_calls() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        if state == BusState::Absent {
            let folder = programs.accessibility_folder(seat);
            let environment = programs::daemon_environment(std::env::vars_os(), seat);
            let handle = self.compositor.loop_handle();
            let log = |path: &std::path::Path| programs.bounded_log(&handle, seat, path);
            if let Err(reason) = self.accessibility.start_bus(seat, folder, environment, log) {
                return Some(ToolResult::failed(reason));
            }
        }
        self.spawn_waits.push(SpawnWait {
            id: id.clone(),
            revision,
            seat,
            start,
        });
        None
    }

    /// Starts `start` on `seat` and tells it.
    fn start_program(&mut self, seat: SeatId, start: Start) -> ToolResult {
        let Some((programs, _)) = &mut self.programs else {
            return ToolResult::failed("this Yuna starts no program: run `yuna mcp`");
        };
        let handle = self.compositor.loop_handle();
        let socket = programs.socket_path(seat);
        let x11 = self.compositor.x11_display(seat);
        match programs.start(&handle, seat, &socket, x11.as_ref(), start) {
            Ok(program) => ToolResult::done(json!({ "program": tools::program_json(program) })),
            Err(reason) => ToolResult::failed(reason),
        }
    }

    /// Answers what the accessibility buses and calls did this round: the programs waiting for a
    /// bus that became ready are started, those of a bus that ended fail.
    pub(super) fn take_accessibility(&mut self, outcome: Outcome) {
        for (id, revision, result) in outcome.answers {
            self.answer_waited(id, revision, result);
        }
        for seat in outcome.ready {
            let BusState::Ready(address) = self.accessibility.bus_state(seat) else {
                continue;
            };
            let mut index = 0;
            while index < self.spawn_waits.len() {
                if self.spawn_waits[index].seat != seat {
                    index += 1;
                    continue;
                }
                let mut wait = self.spawn_waits.remove(index);
                wait.start.accessibility = Some(address.clone());
                let result = self.start_program(seat, wait.start);
                self.answer_waited(wait.id, wait.revision, result);
            }
        }
        for (seat, reason) in outcome.ended {
            self.fail_spawn_waits(seat, &reason);
        }
    }

    /// Answers the programs waiting for `seat`'s accessibility bus with `reason`, failed.
    pub(super) fn fail_spawn_waits(&mut self, seat: SeatId, reason: &str) {
        let mut index = 0;
        while index < self.spawn_waits.len() {
            if self.spawn_waits[index].seat != seat {
                index += 1;
                continue;
            }
            let wait = self.spawn_waits.remove(index);
            let result = ToolResult::failed(reason);
            self.answer_waited(wait.id, wait.revision, result);
        }
    }

    /// `accessibility`: a failure now, or none while it walks or waits.
    pub(super) fn accessibility_call(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call: AccessibilityCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        if let Err(failed) = self.agent_seat(call.seat) {
            return Some(failed);
        }
        if self.programs.is_none() {
            return Some(ToolResult::failed(
                "this Yuna starts no program: run `yuna mcp`",
            ));
        }
        if self.waiting_calls() >= MAX_WAITING_CALLS {
            return Some(ToolResult::failed(format!(
                "{MAX_WAITING_CALLS} calls wait already"
            )));
        }
        self.accessibility
            .call(id, revision, call, &self.compositor)
    }

    /// `spawn`'s list, stop, and start without accessibility: answered now.
    fn spawn_now(&mut self, call: SpawnCall) -> ToolResult {
        let Some((programs, _)) = &mut self.programs else {
            return ToolResult::failed("this Yuna starts no program: run `yuna mcp`");
        };
        let seat = match &call {
            SpawnCall::Start { seat, .. }
            | SpawnCall::List { seat }
            | SpawnCall::Stop { seat, .. }
            | SpawnCall::Log { seat, .. }
            | SpawnCall::Freeze { seat, .. }
            | SpawnCall::Thaw { seat, .. }
            | SpawnCall::Slow { seat, .. } => *seat,
        };
        let seat = SeatId(seat);
        if seat == OWNER_SEAT || !self.compositor.seats().contains(&seat) {
            return ToolResult::failed(format!(
                "no seat {}: create one with `seat create`",
                seat.0
            ));
        }
        let handle = self.compositor.loop_handle();
        // A paced program, as its result tells it.
        let paced = |programs: &Programs, program: u64| {
            let listed = programs.program(seat, program).map(tools::program_json);
            ToolResult::done(json!({ "program": listed }))
        };
        match call {
            SpawnCall::Start {
                command, env, cwd, ..
            } => {
                let start = Start {
                    command,
                    env,
                    cwd: cwd.map(PathBuf::from),
                    accessibility: None,
                };
                self.start_program(seat, start)
            }
            SpawnCall::List { .. } => {
                let list: Vec<Value> = programs
                    .list(seat)
                    .into_iter()
                    .map(tools::program_json)
                    .collect();
                ToolResult::done(json!({ "programs": list }))
            }
            SpawnCall::Stop { program, force, .. } => {
                match programs.stop(&handle, seat, program, force) {
                    Ok(()) => ToolResult::done(json!({
                        "stopped": program,
                        "signal": if force { "SIGKILL" } else { "SIGTERM" },
                    })),
                    Err(reason) => ToolResult::failed(reason),
                }
            }
            SpawnCall::Log {
                program,
                from,
                max_bytes,
                ..
            } => {
                let max_bytes = max_bytes.unwrap_or(tools::DEFAULT_LOG_READ);
                if !(1..=tools::MAX_LOG_READ).contains(&max_bytes) {
                    return ToolResult::failed(format!(
                        "max_bytes is from 1 to {}",
                        tools::MAX_LOG_READ
                    ));
                }
                let read = programs
                    .program(seat, program)
                    .ok_or_else(|| format!("no program {program} on seat {}", seat.0))
                    .and_then(|found| found.read_log(from, max_bytes));
                match read {
                    Ok(read) => ToolResult::done(tools::log_json(&read)),
                    Err(reason) => ToolResult::failed(reason),
                }
            }
            SpawnCall::Freeze { program, .. } => match programs.freeze(&handle, seat, program) {
                Ok(()) => paced(programs, program),
                Err(reason) => ToolResult::failed(reason),
            },
            SpawnCall::Thaw { program, .. } => match programs.thaw(&handle, seat, program) {
                Ok(()) => paced(programs, program),
                Err(reason) => ToolResult::failed(reason),
            },
            SpawnCall::Slow {
                program,
                run_percent,
                ..
            } => match programs.slow(&handle, seat, program, run_percent) {
                Ok(()) => paced(programs, program),
                Err(reason) => ToolResult::failed(reason),
            },
        }
    }

    /// Lets the programs' output be read again when their throttle has ended, and arms the next
    /// throttle while some output waits for one: one timer for all, and none otherwise.
    pub(super) fn throttle_programs(&mut self, ended: bool) {
        let Some((programs, _)) = &self.programs else {
            return;
        };
        let handle = self.compositor.loop_handle();
        if ended {
            self.throttle_armed = false;
            programs.resume_throttled(&handle);
        }
        if self.throttle_armed || !programs.has_throttled() {
            return;
        }
        let events = self.events.clone();
        let armed = insert_source(
            &handle,
            Wake::of(Source::ProgramsThrottle),
            Timer::from_duration(THROTTLE),
            move |_, _, _| {
                events.borrow_mut().throttle_ended = true;
                TimeoutAction::Drop
            },
        );
        match armed {
            Ok(_) => self.throttle_armed = true,
            // Without a timer, the output is read again at once rather than never.
            Err(_) => programs.resume_throttled(&handle),
        }
    }

    /// Ends every process under Yuna: SIGTERM and SIGCONT, then SIGKILL to those left after
    /// [`END_GRACE`], each wait cut short once none is left. No program is paced any more, so
    /// that none is stopped again before it acts on SIGTERM. Meanwhile the compositor still
    /// serves its clients, so that a program answering SIGTERM with a last roundtrip gets its
    /// answer.
    pub(super) fn end_programs(&mut self, programs: &mut Programs, children: &OwnedFd) {
        programs.unpace_all(&self.compositor.loop_handle());
        for (signal, wait) in [(Signal::TERM, END_GRACE), (Signal::KILL, KILL_WAIT)] {
            if programs::signal_all(signal) == 0 {
                return;
            }
            let until = Instant::now() + wait;
            loop {
                // A process may fork between the walk and its own SIGKILL: each look kills again.
                if signal == Signal::KILL {
                    programs::signal_all(Signal::KILL);
                }
                let compositor = &mut self.compositor;
                programs.reap(|pid, status| compositor.child_reaped(pid, status));
                if !programs::any_alive() {
                    return;
                }
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() || self.compositor.dispatch(Some(left)).is_err() {
                    break;
                }
                if std::mem::take(&mut *self.events.borrow_mut()).children_ended {
                    programs::drain(children.as_fd());
                }
            }
        }
    }
}
