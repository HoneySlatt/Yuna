//! A process a test starts ends with the test's thread, shared by `applications.rs` and
//! `consumers.rs`.

use std::os::unix::process::CommandExt;
use std::process::Command;

use rustix::io::Errno;
use rustix::process::{Pid, Signal, getppid, set_parent_process_death_signal};

/// Makes `command`'s process end with SIGKILL when the thread that starts it ends, so that a test
/// killed, or stopped at CI's time limit, leaves no Yuna or application running. Every run drops
/// its processes before its thread ends.
pub fn dies_with_test(command: &mut Command) {
    let parent = std::process::id();
    // SAFETY: the closure runs in the child between fork and exec, where only async-signal-safe
    // calls are sound: it makes two system calls, `prctl` and `getppid`, and allocates nothing,
    // an `Errno` turning into an `io::Error` without a box.
    unsafe {
        command.pre_exec(move || {
            set_parent_process_death_signal(Some(Signal::KILL))?;
            // The parent may have ended before the signal was set: then it never comes.
            if u32::try_from(Pid::as_raw(getppid())) != Ok(parent) {
                return Err(Errno::SRCH.into());
            }
            Ok(())
        });
    }
}
