//! The processes under Yuna, found through `/proc`: signalled, SIGTERM followed at once by
//! SIGCONT, as systemd does, so that a stopped process ends at once rather than at SIGKILL
//! ([`send_ending_signal`]); reaped once SIGCHLD has come, never polled; and all ended when Yuna
//! stops, SIGKILL following [`END_GRACE`] later, from its loop while it still serves its Wayland
//! clients (`../spawn.rs`), or from its panic hook, which has no loop ([`end_after_panic`]): a
//! panic of Yuna's state stops it at once
//! ([ADR-0080](../../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md)),
//! and the hook ends the programs and removes the folder and the seats' homes with the same
//! signals and the same walk through `/proc`, its waits slept. Every child is reaped at SIGCHLD;
//! the status of one that is not a program, an Xwayland's, goes back to the compositor, whose
//! own wait on it then finds it reaped.
//!
//! Its neighbours: `mod.rs` reaps the programs and finds a seat's processes; `../seats.rs` ends
//! them when their seat closes.

use std::collections::HashMap;
use std::fs;
use std::os::fd::BorrowedFd;
use std::path::Path;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, WaitOptions, WaitStatus};

use super::folder::remove_folder;
use super::homes::remove_linked_homes;
use crate::process::{ProcessStat, read_stat};

/// How long the programs have, once Yuna stops, between SIGTERM and SIGKILL.
pub const END_GRACE: Duration = Duration::from_secs(2);

/// How long Yuna waits, after SIGKILL, for the killed to be reaped.
pub const KILL_WAIT: Duration = Duration::from_secs(1);

/// How often the fatal path, which has no event loop to wake it, looks again whether the
/// processes it signalled have ended: each look reads `/proc`.
const ENDING_POLL: Duration = Duration::from_millis(10);

/// Sends `signal` to every living process under Yuna, found through `/proc`, as
/// [`send_ending_signal`] does, and returns how many there were. A child of Yuna's that ended
/// since it was read is not reaped yet, so its pid is not reused; a deeper process may be reaped
/// by its own parent in between, its pid then free for another
/// ([debt](../../../../../docs/debt.md#the-mcp-server)).
pub fn signal_all(signal: Signal) -> usize {
    let living = living();
    if !living.is_empty() {
        tracing::info!(processes = living.len(), ?signal, "ending the programs");
    }
    for &pid in &living {
        send_ending_signal(pid, signal);
    }
    living.len()
}

/// Sends `signal` to process `pid`; after SIGTERM, SIGCONT too, as systemd does: a stopped
/// process, a program frozen or slowed, acts on SIGTERM only once continued, and would otherwise
/// wait for SIGKILL.
pub fn send_ending_signal(pid: Pid, signal: Signal) {
    let _ = rustix::process::kill_process(pid, signal);
    if signal == Signal::TERM {
        let _ = rustix::process::kill_process(pid, Signal::CONT);
    }
}

/// The processes living under Yuna now.
pub fn living() -> Vec<Pid> {
    living_descendants(rustix::process::getpid())
}

/// Whether a process still runs under Yuna.
pub fn any_alive() -> bool {
    !living().is_empty()
}

/// Reaps every child of Yuna's that has ended, without waiting, and gives `ended` each one's pid
/// and status.
pub(super) fn reap_children(mut ended: impl FnMut(Pid, WaitStatus)) {
    loop {
        // `wait`, POSIX's `waitpid(-1)`: any child. rustix's `waitpid(None)` is `waitpid(0)`, the
        // children of Yuna's own process group alone, and each program leads its own.
        match rustix::process::wait(WaitOptions::NOHANG) {
            Ok(Some((pid, status))) => ended(pid, status),
            Ok(None) => return,
            Err(rustix::io::Errno::INTR) => continue,
            // ECHILD: no child left.
            Err(_) => return,
        }
    }
}

/// Ends every process under Yuna, then removes the seats' homes, through the link `folder`
/// holds, and `folder`, the programs' folder: what `yuna mcp` does when it stops on purpose, for
/// its fatal path, on the thread whose panic stops Yuna
/// ([ADR-0080](../../../../../docs/adr/0080-what-survives-a-failure-and-yunas-own-panics.md),
/// [ADR-0105](../../../../../docs/adr/0105-an-agents-programs-on-a-throwaway-home.md)).
/// It touches neither the compositor nor the server, which the panic may have left half
/// changed: with no event loop to serve the programs' clients or to wake it, it sleeps 10 ms
/// (`ENDING_POLL`) between looks. SIGTERM first, then SIGKILL [`END_GRACE`] later to those left, sent again at each look,
/// each reaped as it ends; it returns once none is left, [`KILL_WAIT`] after SIGKILL at most, at
/// once when none ran.
pub fn end_after_panic(folder: &Path) {
    'stages: for (signal, wait) in [(Signal::TERM, END_GRACE), (Signal::KILL, KILL_WAIT)] {
        if signal_all(signal) == 0 {
            break;
        }
        let until = Instant::now() + wait;
        loop {
            // A process may fork between the walk and its own SIGKILL: each look kills again.
            if signal == Signal::KILL {
                signal_all(Signal::KILL);
            }
            reap_children(|_, _| {});
            if !any_alive() {
                break 'stages;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(ENDING_POLL));
        }
    }
    remove_linked_homes(folder);
    remove_folder(folder);
}

/// Reads every byte waiting in a signal's pipe, which does not block.
pub fn drain(pipe: BorrowedFd<'_>) {
    let mut bytes = [0; 64];
    while let Ok(read) = rustix::io::read(pipe, &mut bytes) {
        if read < bytes.len() {
            return;
        }
    }
}

/// Every process under `root`, its parents followed through `/proc`, by pid.
pub(super) fn processes_under(root: Pid) -> HashMap<i32, ProcessStat> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return HashMap::new();
    };
    let mut all: HashMap<i32, ProcessStat> = HashMap::new();
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        if let Some(process) = read_stat(&stat) {
            children.entry(process.parent).or_default().push(pid);
            all.insert(pid, process);
        }
    }
    let mut under = HashMap::new();
    let mut to_visit = vec![root.as_raw_nonzero().get()];
    while let Some(parent) = to_visit.pop() {
        for &pid in children.get(&parent).into_iter().flatten() {
            to_visit.push(pid);
            if let Some(&process) = all.get(&pid) {
                under.insert(pid, process);
            }
        }
    }
    under
}

/// The living processes under `root`, zombies left out: those are dead, and waiting for their
/// parent.
pub(super) fn living_descendants(root: Pid) -> Vec<Pid> {
    processes_under(root)
        .into_iter()
        .filter(|(_, process)| !process.zombie)
        .filter_map(|(pid, _)| Pid::from_raw(pid))
        .collect()
}
