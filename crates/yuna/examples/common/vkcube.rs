//! The `vkcube` clients that the measures `frame_time` and `motion_time` start, with the
//! environment the applications' tests give them. Each includes this file beside `common`.

use std::fs::File;
use std::process::{Child, Command, Stdio};

use crate::common::Run;

#[path = "vkcube_environment.rs"]
mod environment;
use environment::{vkcube_home, vkcube_variables};

/// The `vkcube` processes, killed when dropped: a `Child` dropped alone keeps running.
pub struct Clients(pub Vec<Child>);

impl Drop for Clients {
    fn drop(&mut self) {
        for client in &mut self.0 {
            let _ = client.kill();
            let _ = client.wait();
        }
    }
}

/// `vkcube` number `n`, drawing without end, with that environment; its home and log in the
/// run's folder, and its runtime folder the run's. How it reaches Yuna, `WAYLAND_DISPLAY` or
/// `WAYLAND_SOCKET`, is the caller's to add.
pub fn vkcube_command(run: &Run, n: u32, client_vulkan: Option<&str>) -> Result<Command, String> {
    let home = vkcube_home(run, n)?;
    let output = File::create(home.join("log")).map_err(|e| format!("vkcube's log: {e}"))?;
    let mut command = Command::new("vkcube");
    command
        .args(["--wsi", "wayland"])
        .env_clear()
        .envs(vkcube_variables(&home, client_vulkan)?)
        .env("XDG_RUNTIME_DIR", run.folder().join("runtime"))
        .stdin(Stdio::null())
        .stdout(
            output
                .try_clone()
                .map_err(|e| format!("vkcube's log: {e}"))?,
        )
        .stderr(output);
    Ok(command)
}
