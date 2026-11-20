//! A program's process group paced: frozen (SIGSTOP) until thawed (SIGCONT), or slowed, run a
//! share of each [`PACE_PERIOD`] by a timer on real time
//! ([ADR-0093](../../../../../docs/adr/0093-a-call-that-waits-for-the-seats-events.md)); and its
//! pace ended, before it is stopped or forgotten, and before its seat's processes or Yuna's are
//! ended.
//!
//! Its neighbours: `mod.rs` holds the programs, and ends a program's pace before it stops one;
//! `../spawn.rs` answers `spawn freeze`, `thaw` and `slow`.

use std::time::Duration;

use calloop::LoopHandle;
use calloop::timer::{TimeoutAction, Timer};
use rustix::process::Signal;
use yuna_layout::SeatId;

use super::{Program, Programs, signal_group};
use crate::rest::{Source, Wake, insert_source};
use crate::state::State;

/// How long a slowed program's period is: it runs its share of each, and is stopped the rest.
pub const PACE_PERIOD: Duration = Duration::from_millis(100);

/// How much of the time a program's process group may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    /// As it likes.
    Free,
    /// Stopped (SIGSTOP), until thawed.
    Frozen,
    /// Running this many hundredths of each [`PACE_PERIOD`], from 1 to 99, stopped the rest.
    Slow(u8),
}

impl Pace {
    /// The share of the time it may run, in hundredths: 100 free, 0 frozen.
    pub fn run_percent(self) -> u8 {
        match self {
            Pace::Free => 100,
            Pace::Frozen => 0,
            Pace::Slow(percent) => percent,
        }
    }
}

impl Programs {
    /// Stops program `id` of `seat`, its whole process group, until it is thawed: SIGSTOP.
    /// Refused as [`Programs::stop`] is.
    pub fn freeze(
        &mut self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        id: u64,
    ) -> Result<(), String> {
        let program = self.program_mut(seat, id)?;
        unpace(handle, program);
        signal_group(program, Signal::STOP)?;
        program.pace.set(Pace::Frozen);
        Ok(())
    }

    /// Lets program `id` of `seat` run as it likes again: its pace ends, and its whole group
    /// gets SIGCONT. Refused as [`Programs::stop`] is.
    pub fn thaw(
        &mut self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        id: u64,
    ) -> Result<(), String> {
        let program = self.program_mut(seat, id)?;
        unpace(handle, program);
        signal_group(program, Signal::CONT)
    }

    /// Slows program `id` of `seat`: its whole group runs `run_percent` hundredths of each
    /// [`PACE_PERIOD`], continued then stopped by a timer on real time, whatever the
    /// compositor's clock, until it is thawed, frozen, stopped, or its group has ended. Refused
    /// for a share outside 1 to 99, and as [`Programs::stop`] is.
    pub fn slow(
        &mut self,
        handle: &LoopHandle<'static, State>,
        seat: SeatId,
        id: u64,
        run_percent: u8,
    ) -> Result<(), String> {
        if !(1..=99).contains(&run_percent) {
            return Err("run_percent is from 1 to 99: freeze or thaw for 0 or 100".to_owned());
        }
        let program = self.program_mut(seat, id)?;
        unpace(handle, program);
        signal_group(program, Signal::CONT)?;
        let running_for = PACE_PERIOD * u32::from(run_percent) / 100;
        let stopped_for = PACE_PERIOD - running_for;
        let group = program.pid;
        let pace = program.pace.clone();
        let pace_wakes = program.pace_wakes.clone();
        let mut running = true;
        let timer = insert_source(
            handle,
            Wake::of_seat(Source::ProgramPace, seat),
            Timer::from_duration(running_for),
            move |_, _, _| {
                pace_wakes.set(pace_wakes.get() + 1);
                running.toggle();
                let signal = if running { Signal::CONT } else { Signal::STOP };
                match rustix::process::kill_process_group(group, signal) {
                    // Its whole group has ended: nothing is left to pace.
                    Err(rustix::io::Errno::SRCH) => {
                        pace.set(Pace::Free);
                        TimeoutAction::Drop
                    }
                    _ if running => TimeoutAction::ToDuration(running_for),
                    _ => TimeoutAction::ToDuration(stopped_for),
                }
            },
        )
        .map_err(|error| format!("program {id} cannot be paced: {}", error.error))?;
        program.pace_timer = Some(timer);
        program.pace.set(Pace::Slow(run_percent));
        Ok(())
    }

    /// Ends the pace of every program, without signalling them: Yuna is about to end them all,
    /// and no timer must stop one again between its SIGTERM and its SIGCONT.
    pub fn unpace_all(&mut self, handle: &LoopHandle<'static, State>) {
        for program in &mut self.list {
            unpace(handle, program);
        }
    }

    /// Ends the pace of `seat`'s programs, without signalling them, as [`Programs::unpace_all`].
    pub fn unpace_seat(&mut self, handle: &LoopHandle<'static, State>, seat: SeatId) {
        for program in self.list.iter_mut().filter(|program| program.seat == seat) {
            unpace(handle, program);
        }
    }
}

/// Ends `program`'s pace, without signalling it: its timer goes while it runs.
pub(super) fn unpace(handle: &LoopHandle<'static, State>, program: &mut Program) {
    // A timer that found the group ended has removed itself, and set the pace free: its token
    // is not removed again.
    if let (Some(timer), Pace::Slow(_)) = (program.pace_timer.take(), program.pace.get()) {
        handle.remove(timer);
    }
    program.pace.set(Pace::Free);
}
