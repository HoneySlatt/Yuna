//! The environment the applications' tests give `vkcube`, which the measures `frame_time`,
//! `motion_time` and `agent_time` give it too. `vkcube.rs` includes this file; `agent_time`, which
//! starts `vkcube` through `spawn`, includes it beside `common`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::common::{Run, shell_variable};

/// `vkcube` number `n`'s home, in the run's folder, made.
pub fn vkcube_home(run: &Run, n: u32) -> Result<PathBuf, String> {
    let home = run.folder().join(format!("vkcube-{n}"));
    fs::create_dir_all(&home).map_err(|e| format!("cannot create {}: {e}", home.display()))?;
    Ok(home)
}

/// The environment the applications' tests give `vkcube` (`crates/yuna/tests/applications.rs`)
/// at `home`, and the Vulkan driver `client_vulkan` names, lavapipe without one. How it reaches
/// Yuna, its runtime folder and `WAYLAND_DISPLAY` or `WAYLAND_SOCKET`, is the caller's to add.
pub fn vkcube_variables(
    home: &Path,
    client_vulkan: Option<&str>,
) -> Result<[(&'static str, String); 6], String> {
    let driver = match client_vulkan {
        Some(icd) => icd.to_owned(),
        None => shell_variable("YUNA_LAVAPIPE_ICD")?,
    };
    Ok([
        ("PATH", shell_variable("YUNA_APPLICATIONS_PATH")?),
        ("HOME", home.display().to_string()),
        ("XDG_CACHE_HOME", home.join("cache").display().to_string()),
        ("LD_LIBRARY_PATH", shell_variable("LD_LIBRARY_PATH")?),
        ("VK_DRIVER_FILES", driver),
        ("VK_LOADER_LAYERS_DISABLE", "~implicit~".to_owned()),
    ])
}
