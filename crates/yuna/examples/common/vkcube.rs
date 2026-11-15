//! The `vkcube` clients that the measures `frame_time` and `motion_time` start, with the
//! environment the applications' tests give them. Each includes this file beside `common`.

use std::fs::{self, File};
use std::process::{Child, Command, Stdio};

use crate::common::{Run, shell_variable};

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

/// `vkcube` number `n`, drawing without end, with the environment the applications' tests give it
/// (`crates/yuna/tests/applications.rs`) and the Vulkan driver `client_vulkan` names, lavapipe
/// without one; its home and log in the run's folder. How it reaches Yuna, `WAYLAND_DISPLAY` or
/// `WAYLAND_SOCKET`, is the caller's to add.
pub fn vkcube_command(run: &Run, n: u32, client_vulkan: Option<&str>) -> Result<Command, String> {
    let home = run.folder().join(format!("vkcube-{n}"));
    fs::create_dir_all(&home).map_err(|e| format!("cannot create {}: {e}", home.display()))?;
    let output = File::create(home.join("log")).map_err(|e| format!("vkcube's log: {e}"))?;
    let driver = match client_vulkan {
        Some(icd) => icd.to_owned(),
        None => shell_variable("YUNA_LAVAPIPE_ICD")?,
    };
    let mut command = Command::new("vkcube");
    command
        .args(["--wsi", "wayland"])
        .env_clear()
        .env("PATH", shell_variable("YUNA_APPLICATIONS_PATH")?)
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", run.folder().join("runtime"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("LD_LIBRARY_PATH", shell_variable("LD_LIBRARY_PATH")?)
        .env("VK_DRIVER_FILES", driver)
        .env("VK_LOADER_LAYERS_DISABLE", "~implicit~")
        .stdin(Stdio::null())
        .stdout(
            output
                .try_clone()
                .map_err(|e| format!("vkcube's log: {e}"))?,
        )
        .stderr(output);
    Ok(command)
}
