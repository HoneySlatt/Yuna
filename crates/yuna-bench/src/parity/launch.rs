//! The launcher of one run, `yuna-bench parity launch`: the first process of the run's own user,
//! PID, mount and network namespaces, which `unshare` makes. It starts the measured command,
//! `yuna mcp` alone or under valgrind, on its own standard input and output, which are the
//! script's MCP channel; and beside it, never under it, the lockstep client, which reaches the
//! script through a socket of the run's runtime folder.
//!
//! In its namespaces, Yuna sees only the run's own processes in `/proc`, at the same process ids
//! in every run, and the X displays of no other run, in a `/tmp` of its own: the work it does to
//! start and to stop, which scans its processes and looks for a free display, is the same in every
//! run, so that it cancels out of the difference between two runs. The folders the script shares
//! with the run, its work folder and its sockets', are therefore never under `/tmp`.
//!
//! Once the measured command ends, the client is killed: the run is over, and Yuna stopped with
//! it still connected, as in every run. The launcher exits with the command's status.

use std::fs::File;
use std::path::Path;
use std::process::{Child, Command, ExitCode, Stdio};

/// The options `unshare` takes to make a run's namespaces: a user namespace, in which the user
/// keeps its own id, and in it the PID, mount (for `/proc`) and network ones. The launcher keeps
/// the capabilities the user namespace grants (`--keep-caps`), to mount the run's `/tmp`, and
/// drops them before it starts anything.
pub const UNSHARE_OPTIONS: [&str; 8] = [
    "--user",
    "--map-current-user",
    "--keep-caps",
    "--pid",
    "--fork",
    "--mount-proc",
    "--net",
    "--kill-child",
];

/// Runs `yuna-bench parity launch --windows <n> --control <socket> --client-log <file> --
/// <command>...`.
pub fn main(arguments: &[String]) -> ExitCode {
    match launch(arguments) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("yuna-bench parity launch: {error}");
            ExitCode::from(2)
        }
    }
}

fn launch(arguments: &[String]) -> Result<ExitCode, String> {
    let usage = "launch --windows <n> --control <socket> --client-log <file> -- <command>...";
    let (options, command) = match arguments.iter().position(|a| a == "--") {
        Some(at) => (&arguments[..at], &arguments[at + 1..]),
        None => return Err(usage.to_owned()),
    };
    let [windows_flag, windows, control_flag, control, log_flag, log] = options else {
        return Err(usage.to_owned());
    };
    if windows_flag != "--windows"
        || control_flag != "--control"
        || log_flag != "--client-log"
        || command.is_empty()
    {
        return Err(usage.to_owned());
    }
    // A /tmp of the run's own, in its mount namespace: Yuna takes its X display's lock there,
    // and in a /tmp every run shares, which number it takes, and how many it tries first,
    // would depend on the runs beside it.
    rustix::mount::mount(
        "tmpfs",
        "/tmp",
        "tmpfs",
        rustix::mount::MountFlags::NOSUID | rustix::mount::MountFlags::NODEV,
        c"mode=1777",
    )
    .map_err(|e| format!("a /tmp of the run's own: {e}"))?;
    // Yuna and the client run with no capability, as anywhere else: without an ambient set, a
    // program this user runs gets none.
    rustix::thread::clear_ambient_capability_set()
        .map_err(|e| format!("the capabilities kept for the mount: {e}"))?;
    let program = std::env::current_exe().map_err(|e| format!("this program: {e}"))?;
    let mut measured = Command::new(&command[0])
        .args(&command[1..])
        .spawn()
        .map_err(|e| format!("{}: {e}", command[0]))?;
    let client_log = File::create(Path::new(log)).map_err(|e| format!("{log}: {e}"))?;
    let mut client = Command::new(program);
    client
        .args([
            "client",
            "lockstep",
            "--windows",
            windows,
            "--control",
            control,
        ])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(client_log);
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        client.env("XDG_RUNTIME_DIR", runtime);
    }
    let mut client = start_client(&mut client, &mut measured)?;
    let status = measured
        .wait()
        .map_err(|e| format!("{}: {e}", command[0]))?;
    let _ = client.kill();
    let _ = client.wait();
    use std::os::unix::process::ExitStatusExt;
    Ok(match (status.code(), status.signal()) {
        (Some(code), _) => ExitCode::from(code.clamp(0, 255) as u8),
        (None, Some(signal)) => ExitCode::from((128 + signal).clamp(0, 255) as u8),
        (None, None) => ExitCode::FAILURE,
    })
}

/// Starts `client` beside `measured`; if it cannot start, ends `measured` and says why. No run
/// goes on without its client: the run fails now, not when the script gives up waiting for a
/// client that never connects.
fn start_client(client: &mut Command, measured: &mut Child) -> Result<Child, String> {
    client.spawn().map_err(|error| {
        let _ = measured.kill();
        let _ = measured.wait();
        format!("the lockstep client: {error}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_that_cannot_start_ends_the_measured_command() {
        let mut measured = Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("sleep, in the development shell");
        let mut client = Command::new("/nonexistent/lockstep-client");
        let error = start_client(&mut client, &mut measured).unwrap_err();
        assert!(error.contains("the lockstep client"), "{error}");
        // Ended and reaped: nothing is left to wait for.
        assert!(measured.try_wait().expect("its status").is_some());
    }
}
