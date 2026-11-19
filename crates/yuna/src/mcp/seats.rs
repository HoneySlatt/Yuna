//! The `seat` tool: an agent's seats created, listed and closed
//! ([ADR-0095](../../../../docs/adr/0095-an-agents-seat-through-mcp-its-tools-input-keyboard-layouts-and-closing.md)).
//!
//! `seat close` releases what the seat holds, removes its socket, ends its clients, its outputs
//! and its place in the layout at once, then ends its processes: SIGTERM and SIGCONT, then
//! SIGKILL [`END_GRACE`] later; it is answered once they are gone, [`KILL_WAIT`] after SIGKILL at
//! most.
//!
//! Apart because a closing outlives its call: its stages run in later rounds, on timers of their
//! own, and Yuna's stop answers those still running (`mod.rs`). Its neighbours: `mod.rs` checks
//! the seat every other tool names; `outputs.rs` reads the mode of a new seat's output;
//! `waits.rs` fails the calls waiting on a seat that closes.

use std::time::Duration;

use serde_json::{Map, Value, json};
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::rustix;
use smithay::reexports::rustix::process::{Pid, Signal};
use yuna_layout::SeatId;

use super::Server;
use super::programs::{self, END_GRACE, KILL_WAIT};
use super::protocol::Revision;
use super::tools::{self, SeatCall, ToolResult};
use crate::ClockMode;
use crate::output::OutputMode;
use crate::rest::{Source, Wake, insert_source};
use crate::state::OWNER_SEAT;

/// A `seat close` waiting for the seat's processes to end.
#[derive(Debug)]
pub(super) struct SeatClosing {
    key: u64,
    /// The request, unless cancelled: then the seat still closes, unanswered.
    pub(super) reply: Option<(Value, Revision)>,
    seat: SeatId,
    /// Its clients' process ids, which find its processes.
    client_pids: Vec<i32>,
    /// The processes signalled, in the order they were found.
    pub(super) signalled: Vec<Pid>,
    /// Whether SIGKILL was sent: the last stage.
    killed: bool,
    pub(super) timer: RegistrationToken,
}

impl Server {
    /// `seat`: the result now, or none when a closing waits for the seat's processes.
    pub(super) fn seat(
        &mut self,
        id: &Value,
        revision: Revision,
        arguments: Map<String, Value>,
    ) -> Option<ToolResult> {
        let call = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return Some(ToolResult::failed(reason)),
        };
        Some(match call {
            SeatCall::Create {
                width,
                height,
                rate,
                scale,
                vrr_min,
                hdr,
            } => match self.mode_of(width, height, rate, scale, vrr_min, hdr) {
                Ok(mode) => match self.check_output_size(&mode, None) {
                    Ok(()) => self.create_seat(mode),
                    Err(failed) => failed,
                },
                Err(failed) => failed,
            },
            SeatCall::List {} => {
                let seats: Vec<Value> = self
                    .compositor
                    .seats()
                    .into_iter()
                    .filter(|&seat| seat != OWNER_SEAT)
                    .map(|seat| self.seat_json(seat))
                    .collect();
                ToolResult::done(json!({ "seats": seats }))
            }
            SeatCall::Close { seat } => match self.agent_seat(seat) {
                Ok(seat) => return self.close_seat(id, revision, seat),
                Err(failed) => failed,
            },
        })
    }

    /// Creates a seat, its output and, with programs, its socket; nothing is left of it when a
    /// step fails.
    fn create_seat(&mut self, mode: OutputMode) -> ToolResult {
        let has_agent_seat = self
            .compositor
            .seats()
            .iter()
            .any(|&seat| seat != OWNER_SEAT);
        if self.compositor.clock_mode() == ClockMode::Manual && has_agent_seat {
            return ToolResult::failed(
                "a Yuna on a manual clock holds one seat, whose agent steps its clock: close \
                 it, or start another `yuna mcp`",
            );
        }
        let seat = match self.compositor.add_seat() {
            Ok(seat) => seat,
            Err(error) => return ToolResult::failed(error.to_string()),
        };
        self.last_seat = self.last_seat.max(seat);
        let made = self
            .compositor
            .add_output(seat, mode)
            .and_then(|_| match &self.programs {
                Some((programs, _)) => {
                    let path = programs.socket_path(seat);
                    self.compositor.listen_for_seat(seat, path.clone())?;
                    Ok(Some(path))
                }
                None => Ok(None),
            });
        match made {
            Ok(_) => ToolResult::done(self.seat_json(seat)),
            Err(error) => {
                if let Err(undo) = self.compositor.remove_seat(seat) {
                    tracing::error!(%undo, seat = seat.0, "a seat half made is left");
                }
                ToolResult::failed(error.to_string())
            }
        }
    }

    /// A seat, as the tools give it: its handle, its outputs, its socket, its windows' and
    /// running programs' counts.
    fn seat_json(&self, seat: SeatId) -> Value {
        let outputs: Vec<Value> = self
            .compositor
            .output_descriptions()
            .iter()
            .filter(|output| output.seat == seat)
            .map(tools::output_json)
            .collect();
        let windows = self
            .compositor
            .windows()
            .iter()
            .filter(|window| window.seat == seat)
            .count();
        let (socket, running) = match &self.programs {
            Some((programs, _)) => (
                Some(programs.socket_path(seat)),
                programs
                    .list(seat)
                    .iter()
                    .filter(|program| program.end.is_none())
                    .count(),
            ),
            None => (None, 0),
        };
        json!({
            "seat": seat.0,
            "outputs": outputs,
            "socket": socket,
            "windows": windows,
            "programs_running": running,
        })
    }

    /// Closes `seat`: what the compositor holds of it at once, then its processes, SIGTERM
    /// first. Answered now when none is left, else once they end (`answer_closings_done`).
    fn close_seat(&mut self, id: &Value, revision: Revision, seat: SeatId) -> Option<ToolResult> {
        let daemon = self.accessibility.daemon_pid(seat);
        let mut client_pids = match self.compositor.remove_seat(seat) {
            Ok(pids) => pids,
            Err(error) => return Some(ToolResult::failed(error.to_string())),
        };
        // The accessibility bus's daemon is one of the seat's processes.
        client_pids.extend(daemon.map(|pid| pid.as_raw_nonzero().get()));
        self.fail_waits_of(seat);
        let handle = self.compositor.loop_handle();
        let processes = match &mut self.programs {
            Some((programs, _)) => {
                programs.unpace_seat(&handle, seat);
                programs.seat_processes(seat, &client_pids, self.last_seat)
            }
            None => Vec::new(),
        };
        if processes.is_empty() {
            return Some(self.seat_closed(seat, &[], &[]));
        }
        for &pid in &processes {
            programs::send_ending_signal(pid, Signal::TERM);
        }
        tracing::info!(
            seat = seat.0,
            processes = processes.len(),
            "ending a seat's processes"
        );
        let key = self.next_wait;
        let timer = match self.arm_closing_stage(key, seat, END_GRACE) {
            Ok(timer) => timer,
            Err(error) => {
                // Without a timer, nothing would send SIGKILL: send it now.
                for &pid in &processes {
                    let _ = rustix::process::kill_process(pid, Signal::KILL);
                }
                tracing::warn!(%error, "a seat's processes are killed at once");
                return Some(self.seat_closed(seat, &processes, &[]));
            }
        };
        self.next_wait += 1;
        self.closings.push(SeatClosing {
            key,
            reply: Some((id.clone(), revision)),
            seat,
            client_pids,
            signalled: processes,
            killed: false,
            timer,
        });
        None
    }

    /// Arms the timer that ends `seat`'s closing's stage `after` from now.
    fn arm_closing_stage(
        &self,
        key: u64,
        seat: SeatId,
        after: Duration,
    ) -> Result<RegistrationToken, smithay::reexports::calloop::Error> {
        let events = self.events.clone();
        insert_source(
            &self.compositor.loop_handle(),
            Wake::of_seat(Source::SeatClosing, seat),
            Timer::from_duration(after),
            move |_, _, _| {
                events.borrow_mut().closings_due.push(key);
                TimeoutAction::Drop
            },
        )
        .map_err(|e| e.error)
    }

    /// A closing's stage is up: after SIGTERM, SIGKILL goes to every process of the seat still
    /// there, those it started meanwhile included; after SIGKILL, the closing is answered with
    /// what is left.
    pub(super) fn closing_stage_up(&mut self, key: u64) {
        let Some(index) = self.closings.iter().position(|closing| closing.key == key) else {
            return;
        };
        let living = self.living_under_yuna();
        if self.closings[index].killed {
            let closing = self.closings.remove(index);
            let left: Vec<Pid> = closing
                .signalled
                .iter()
                .copied()
                .filter(|pid| living.contains(pid))
                .collect();
            self.answer_closing(closing, &left);
            return;
        }
        let closing = &self.closings[index];
        // Only processes still under Yuna: a pid that left it may be another's now.
        let mut targets: Vec<Pid> = closing
            .signalled
            .iter()
            .copied()
            .filter(|pid| living.contains(pid))
            .collect();
        if let Some((programs, _)) = &self.programs {
            for pid in programs.seat_processes(closing.seat, &closing.client_pids, self.last_seat) {
                if !targets.contains(&pid) {
                    targets.push(pid);
                }
            }
        }
        for &pid in &targets {
            let _ = rustix::process::kill_process(pid, Signal::KILL);
        }
        let seat = self.closings[index].seat;
        let timer = self.arm_closing_stage(key, seat, KILL_WAIT);
        let closing = &mut self.closings[index];
        for pid in targets {
            if !closing.signalled.contains(&pid) {
                closing.signalled.push(pid);
            }
        }
        closing.killed = true;
        match timer {
            Ok(timer) => closing.timer = timer,
            Err(_) => {
                let closing = self.closings.remove(index);
                self.answer_closing(closing, &[]);
            }
        }
    }

    /// The processes living under Yuna; none without programs, in a test's process, whose
    /// children are not the server's.
    pub(super) fn living_under_yuna(&self) -> Vec<Pid> {
        if self.programs.is_some() {
            programs::living()
        } else {
            Vec::new()
        }
    }

    /// Answers each closing whose processes have all ended, or left Yuna.
    pub(super) fn answer_closings_done(&mut self) {
        if self.closings.is_empty() {
            return;
        }
        let living = self.living_under_yuna();
        let mut index = 0;
        while index < self.closings.len() {
            if self.closings[index]
                .signalled
                .iter()
                .any(|pid| living.contains(pid))
            {
                index += 1;
                continue;
            }
            let closing = self.closings.remove(index);
            self.compositor.loop_handle().remove(closing.timer);
            self.answer_closing(closing, &[]);
        }
    }

    pub(super) fn answer_closing(&mut self, closing: SeatClosing, left: &[Pid]) {
        let result = self.seat_closed(closing.seat, &closing.signalled, left);
        if let Some((id, revision)) = closing.reply {
            self.answer_waited(id, revision, result);
        }
    }

    /// A closed seat's result; its programs are forgotten, their logs deleted, and its home
    /// removed.
    fn seat_closed(&mut self, seat: SeatId, ended: &[Pid], left: &[Pid]) -> ToolResult {
        if let Some((programs, _)) = &mut self.programs {
            programs.forget_seat(seat);
        }
        let raw = |pid: &Pid| pid.as_raw_nonzero().get();
        let ended: Vec<i32> = ended
            .iter()
            .filter(|pid| !left.contains(pid))
            .map(raw)
            .collect();
        let left: Vec<i32> = left.iter().map(raw).collect();
        tracing::info!(seat = seat.0, left = left.len(), "seat closed");
        ToolResult::done(json!({
            "closed": seat.0,
            "ended": ended,
            "left_running": left,
        }))
    }
}
