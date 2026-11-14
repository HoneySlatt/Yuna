//! What every program Yuna starts shares: the descriptors it does not inherit. MCP's programs
//! (`mcp/programs.rs`) and Xwayland (`xwayland/server.rs`) start through it.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

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
