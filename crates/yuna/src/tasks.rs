//! The table of tasks, and the one spawner of Yuna's threads
//! ([ADR-0152](../../../docs/adr/0152-the-table-of-tasks-one-spawner-the-x11-manager-inline-at-m2-17.md)).
//!
//! Every thread Yuna starts is a [`Task`] of the table below and goes through [`spawn`], which
//! names it `yuna-…` within the 15 bytes Linux keeps of a thread's name: the parity script finds
//! Yuna's own threads by that name. Clippy's `disallowed-methods` (`clippy.toml`) refuses
//! `std::thread`'s spawns anywhere else in Yuna's sources; tests, examples and `yuna-bench` allow
//! it at their roots, and test modules inside `src/` expect it at theirs: their threads are not
//! Yuna's. Programs Yuna starts are not tasks: their spawner, which clears their capabilities,
//! comes at M2.18.
//!
//! | Task | Owner | Inbox, its bound, and when full | Stop order at exit | Failure | Class | Clock |
//! |---|---|---|---|---|---|---|
//! | The state thread, the thread that made the [`Compositor`](crate::Compositor) | The program (`main.rs`), or the test that made it | The event loop's sources, each bounded where it reads ([ADR-0131](../../../docs/adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)): a client past its 1 MiB of unread events is ended ([ADR-0136](../../../docs/adr/0136-a-client-that-stops-reading-1-mib-and-its-motion-merged.md)) | Last: it stops every other task, then returns | Fatal: the panic hook ends the process, and the keeper starts Yuna again ([ADR-0080](../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md), `fatal.rs`) | Normal | Yuna's [`Clock`](crate::Clock), real or manual |
//! | [`Task::X11Manager`], `yuna-x11-<seat>`, one per running Xwayland (`xwayland/manager/`) | Its seat's Xwayland, in `xwayland/mod.rs` | What Yuna owes it (`owed.rs`), `MAX_OWED_BYTES`, 8 MiB of commands: past it, or once it has taken none for `MANAGER_STALL`, Yuna ends that Xwayland. Its events to Yuna: a channel of `EVENT_QUEUE` (256), where the thread waits when it is full | Released, its Xwayland killed and reaped within `REAP_BOUND` (1 s), then joined within its bound; past a bound, left, with a warning. While Yuna serves, it is never waited on: its handle goes when its Xwayland is reaped | Contained: its end ends its Xwayland and that seat's X11 programs; the seat starts another | Normal | The real monotonic clock, also when Yuna's is manual ([debt](../../../docs/debt.md#xwayland)) |
//! | [`Task::Encoder`], `yuna-capture`, in `yuna mcp` (`mcp/captures.rs`) | `yuna mcp`'s server | None: it takes its frame when it starts, and sends its one image back on a channel. At most `MAX_ENCODERS` (4), or the cores less one if fewer, at least one, run at once: a capture copied meanwhile waits for one, and one asked meanwhile is refused. Its channel back holds `MAX_ENCODERS` images, in its type | Joined when the server finishes, within its bound; past it, detached | Contained: its panic fails its capture alone | Normal | None: it reads the real clock only to log how long it took |
//!
//! Every task runs at the normal scheduling class: an agent's Yuna is never lowered for a game
//! ([ADR-0082](../../../docs/adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md)
//! item 24). At a session pause and at a GPU loss, the stop order is none until Stage 2b brings
//! both.
//!
//! Outside the table, since they are not Yuna's core:
//!
//! - `yuna-wlcs-server`, the thread of WLCS's module (`crates/yuna-wlcs/src/server.rs`), a test
//!   harness on the real clock: 16 bytes, of which Linux keeps `yuna-wlcs-serve`, which the
//!   `yuna-…` prefix still matches. Its one spawn expects clippy's lint.
//! - The threads that Mesa starts inside Yuna's process for EGL: on llvmpipe, `llvmpipe-<n>`,
//!   `yuna:disk$0` for its shader cache, and its compute pool, which names its threads not at
//!   all, so that they show the process's name, `yuna`; on a GPU, its driver's queues. Clippy
//!   sees Yuna's sources alone, never a library's threads; the parity script counts each
//!   process's main thread and its `yuna-…` threads, which leaves Mesa's out.
//! - The threads of tests, examples and `yuna-bench`.

use std::convert::Infallible;
use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use yuna_types::SeatId;

/// The most bytes of a thread's name that Linux keeps (`TASK_COMM_LEN`, less its final zero).
pub(crate) const MAX_THREAD_NAME: usize = 15;

/// A task of the table: what one of Yuna's threads does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Task {
    /// The X11 window manager of the seat's Xwayland.
    X11Manager(SeatId),
    /// One capture's PNG encoder, in `yuna mcp`.
    Encoder,
}

impl Task {
    /// The name of its thread: `yuna-…`, cut to [`MAX_THREAD_NAME`] bytes. An X11 manager's
    /// holds its seat's number whole up to seat 999,999; past it, the number's last digits are
    /// cut, as Linux would cut them.
    pub(crate) fn thread_name(self) -> String {
        let mut name = match self {
            Task::X11Manager(seat) => format!("yuna-x11-{}", seat.0),
            Task::Encoder => "yuna-capture".to_owned(),
        };
        // Every name is ASCII, so any byte is a character's boundary.
        name.truncate(MAX_THREAD_NAME);
        name
    }

    /// How long a stop waits for its thread to end, at most, before it is detached.
    pub(crate) fn join_bound(self) -> Duration {
        match self {
            // Its Xwayland killed, its connection closes and it returns at once: it waits longer
            // only blocked on its full channel of events, which nothing reads any more.
            Task::X11Manager(_) => Duration::from_secs(1),
            // A 4K image encodes in under 0.1 s in a release build, and in about 1.2 s in a
            // debug build at its logical size; an image no one will read is left past it.
            Task::Encoder => Duration::from_secs(1),
        }
    }
}

/// A task's thread, until it is joined. Dropping it detaches the thread, which ends on its own.
#[derive(Debug)]
pub(crate) struct TaskThread {
    name: String,
    thread: JoinHandle<()>,
    /// Disconnected once the thread's work has returned or unwound: its sender drops then.
    finished: Receiver<Infallible>,
}

/// How a join within a bound ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Joined {
    /// The thread's work returned.
    Ended,
    /// The thread's work panicked; the panic hook has logged it.
    Panicked,
    /// The bound passed first: the thread was detached, and a warning logged.
    Detached,
}

/// Starts `work` on a thread of its own, named for `task`. Fails as `std::thread` does, when the
/// system refuses a thread.
pub(crate) fn spawn(task: Task, work: impl FnOnce() + Send + 'static) -> io::Result<TaskThread> {
    let name = task.thread_name();
    let (done, finished) = mpsc::sync_channel::<Infallible>(0);
    #[expect(
        clippy::disallowed_methods,
        reason = "the one spawner of Yuna's threads (ADR-0152)"
    )]
    let thread = std::thread::Builder::new()
        .name(name.clone())
        .spawn(move || {
            // Dropped as `work` returns or unwinds, which tells `join_by` the work is done.
            let _done = done;
            work();
        })?;
    Ok(TaskThread {
        name,
        thread,
        finished,
    })
}

impl TaskThread {
    /// Whether the thread's work has ended. Never waits.
    pub(crate) fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// Waits until the thread's work ends, at most until `deadline`, then joins it. Past the
    /// deadline, the thread is detached and a warning names it: a stop never waits without end.
    pub(crate) fn join_by(self, deadline: Instant) -> Joined {
        let wait = deadline.saturating_duration_since(Instant::now());
        match self.finished.recv_timeout(wait) {
            Err(RecvTimeoutError::Disconnected) => match self.thread.join() {
                Ok(()) => Joined::Ended,
                Err(_) => Joined::Panicked,
            },
            Err(RecvTimeoutError::Timeout) => {
                tracing::warn!(
                    thread = self.name,
                    "a thread did not end within its bound: it is left to end alone"
                );
                Joined::Detached
            }
            Ok(never) => match never {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_is_yunas_within_fifteen_bytes() {
        assert_eq!(Task::Encoder.thread_name(), "yuna-capture");
        assert_eq!(Task::X11Manager(SeatId(1)).thread_name(), "yuna-x11-1");
        assert_eq!(
            Task::X11Manager(SeatId(999_999)).thread_name(),
            "yuna-x11-999999"
        );
        let longest = Task::X11Manager(SeatId(u64::MAX)).thread_name();
        assert_eq!(longest, "yuna-x11-184467");
        for task in [Task::Encoder, Task::X11Manager(SeatId(u64::MAX))] {
            let name = task.thread_name();
            assert!(name.len() <= MAX_THREAD_NAME, "{name}");
            assert!(name.starts_with("yuna-"), "{name}");
        }
    }

    #[test]
    fn the_kernel_keeps_the_whole_name() {
        let (sender, named) = mpsc::channel();
        let thread = spawn(Task::X11Manager(SeatId(999_999)), move || {
            let _ = sender.send(std::fs::read_to_string("/proc/thread-self/comm"));
        })
        .expect("a thread");
        let comm = named.recv().expect("its name").expect("read");
        assert_eq!(comm, "yuna-x11-999999\n");
        assert_eq!(
            thread.join_by(Instant::now() + Duration::from_secs(10)),
            Joined::Ended
        );
    }

    #[test]
    fn a_join_tells_a_panic_apart() {
        let thread = spawn(Task::Encoder, || panic!("a test's task panics")).expect("a thread");
        assert_eq!(
            thread.join_by(Instant::now() + Duration::from_secs(10)),
            Joined::Panicked
        );
    }

    #[test]
    fn a_thread_past_its_bound_is_detached() {
        let (release, held) = mpsc::channel::<()>();
        let thread = spawn(Task::Encoder, move || {
            let _ = held.recv();
        })
        .expect("a thread");
        let began = Instant::now();
        assert_eq!(
            thread.join_by(began + Duration::from_millis(50)),
            Joined::Detached
        );
        assert!(began.elapsed() >= Duration::from_millis(50));
        drop(release);
    }

    #[test]
    fn a_deadline_already_passed_still_joins_a_thread_that_ended() {
        let thread = spawn(Task::Encoder, || {}).expect("a thread");
        while !thread.is_finished() {
            std::thread::yield_now();
        }
        assert_eq!(thread.join_by(Instant::now()), Joined::Ended);
    }
}
