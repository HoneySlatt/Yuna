//! What every program Yuna starts shares: the descriptors it does not inherit. MCP's programs
//! (`mcp/programs.rs`) and Xwayland (`xwayland/server.rs`) start through it. And what Yuna reads
//! of a process in `/proc`: its parent, its group, and the processes above it up to Yuna, which
//! a client's connection records (`state.rs`) and a seat's closing follows (`mcp/programs.rs`).

use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

use smithay::reexports::rustix;

/// How many processes above a client its lineage follows before giving up: deeper trees are not
/// under Yuna, or not worth the reads.
const MAX_LINEAGE: usize = 64;

/// Marks every descriptor of the program above 2 close-on-exec, in the child just before it runs
/// the program, so that it inherits none of Yuna's: llvmpipe opens some without
/// `O_CLOEXEC`, `/dev/udmabuf` and the memory of its buffers among them. Marked, not closed:
/// the pipe through which the standard library reports a failed `exec` must stay open. Xwayland's
/// start uses it too, then clears the flag of the descriptors it gives (`xwayland/server.rs`).
pub(crate) fn keep_descriptors_from_the_program(command: &mut Command) {
    // SAFETY: the closure runs in the child between fork and exec, where only async-signal-safe
    // calls are sound: it makes one system call, `close_range`, and allocates nothing, an errno
    // turning into an `io::Error` without a box.
    unsafe {
        command.pre_exec(|| {
            if close_range(3, u32::MAX, CLOSE_RANGE_CLOEXEC) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
}

/// `close_range`'s flag that marks the descriptors close-on-exec rather than closing them.
const CLOSE_RANGE_CLOEXEC: u32 = 1 << 2;

// SAFETY: this declaration matches the C library's own, glibc's since 2.34, which the standard
// library links on Linux; its one call is `unsafe`, with its reason.
unsafe extern "C" {
    /// Linux's `close_range(2)`: `int close_range(unsigned int first, unsigned int last, int
    /// flags)`, its flags passed as the `unsigned int` they are built from.
    fn close_range(first: u32, last: u32, flags: u32) -> i32;
}

/// A process as `/proc/<pid>/stat` gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessStat {
    pub parent: i32,
    pub group: i32,
    pub zombie: bool,
}

/// A `/proc/<pid>/stat` line: `pid (name) state ppid pgrp …`, the name possibly holding spaces
/// and parentheses, so read from its last `)`.
pub(crate) fn read_stat(stat: &str) -> Option<ProcessStat> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    let mut fields = after_name.split_whitespace();
    let state = fields.next()?;
    let parent = fields.next()?.parse().ok()?;
    let group = fields.next()?.parse().ok()?;
    Some(ProcessStat {
        parent,
        group,
        zombie: state == "Z",
    })
}

/// Process `pid` as `/proc` gives it now; none once it is gone and reaped.
fn stat_of(pid: i32) -> Option<ProcessStat> {
    read_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// Process `pid` and the processes above it up to Yuna, Yuna left out, nearest first, with
/// `pid`'s process group, as `/proc` gives them now. Empty, with no group, when `pid` is not
/// under Yuna: Yuna itself, a process elsewhere in the machine, or one gone already.
pub(crate) fn lineage_under_yuna(pid: i32) -> (Vec<i32>, Option<i32>) {
    let yuna = rustix::process::getpid().as_raw_nonzero().get();
    let mut lineage = Vec::new();
    let mut group = None;
    let mut current = pid;
    while lineage.len() < MAX_LINEAGE && current > 1 && current != yuna {
        let Some(stat) = stat_of(current) else {
            break;
        };
        group.get_or_insert(stat.group);
        lineage.push(current);
        if stat.parent == yuna {
            return (lineage, group);
        }
        current = stat.parent;
    }
    (Vec::new(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuna_and_processes_outside_it_have_no_lineage() {
        let me = rustix::process::getpid().as_raw_nonzero().get();
        assert_eq!(lineage_under_yuna(me), (Vec::new(), None));
        // The test's parent is above it, never under it.
        let parent = stat_of(me).expect("this process").parent;
        assert_eq!(lineage_under_yuna(parent), (Vec::new(), None));
        assert_eq!(lineage_under_yuna(i32::MAX), (Vec::new(), None));
    }

    #[test]
    fn a_child_and_its_child_are_found_under_yuna_nearest_first() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 30 & echo $!; wait"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("sh starts");
        let mut line = String::new();
        io::BufRead::read_line(
            &mut io::BufReader::new(child.stdout.take().expect("its output")),
            &mut line,
        )
        .expect("the grandchild's pid");
        let grandchild: i32 = line.trim().parse().expect("a pid");
        let shell = child.id() as i32;
        let (lineage, group) = lineage_under_yuna(grandchild);
        // The shell was started without a group of its own: its group is the test's.
        let shells_group = stat_of(shell).map(|stat| stat.group);
        let _ = rustix::process::kill_process(
            rustix::process::Pid::from_raw(grandchild).expect("a pid"),
            rustix::process::Signal::KILL,
        );
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(lineage, [grandchild, shell]);
        assert_eq!(group, shells_group);
    }
}
