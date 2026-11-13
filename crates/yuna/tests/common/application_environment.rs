//! The environment the tests give a real application: only what is listed here, so that it reads
//! neither the machine's settings nor the user's (`tests/applications.rs` says what still reads
//! the machine). Shared by the tests that start applications, `applications.rs` and `mcp.rs`.

use std::ffi::OsString;
use std::path::Path;

/// An application's variables, with `home` as its home and XDG folders, without its runtime
/// folder and its Wayland socket, which depend on who starts it.
pub fn application_environment(home: &Path) -> Vec<(&'static str, OsString)> {
    vec![
        // The store alone: nixpkgs' Qt also looks for plugins beside each folder of the PATH.
        ("PATH", shell_variable("YUNA_APPLICATIONS_PATH").into()),
        ("HOME", home.into()),
        ("XDG_CONFIG_HOME", home.join("config").into()),
        ("XDG_DATA_HOME", home.join("data").into()),
        ("XDG_CACHE_HOME", home.join("cache").into()),
        ("XDG_STATE_HOME", home.join("state").into()),
        // Unset, they would be /etc/xdg and /usr/share: the machine's settings, GTK's among them.
        ("XDG_CONFIG_DIRS", home.join("config-dirs").into()),
        ("XDG_DATA_DIRS", home.join("data-dirs").into()),
        ("FONTCONFIG_FILE", shell_variable("YUNA_FONTS_CONF").into()),
        // The shell's Mesa, on the CPU, never the machine's GPU (ADR-0022).
        ("LD_LIBRARY_PATH", shell_variable("LD_LIBRARY_PATH").into()),
        (
            "__EGL_VENDOR_LIBRARY_FILENAMES",
            shell_variable("__EGL_VENDOR_LIBRARY_FILENAMES").into(),
        ),
        ("LIBGL_ALWAYS_SOFTWARE", "1".into()),
        (
            "VK_DRIVER_FILES",
            shell_variable("YUNA_LAVAPIPE_ICD").into(),
        ),
        // The machine's own layers, such as its Mesa's device_select, stay out.
        ("VK_LOADER_LAYERS_DISABLE", "~implicit~".into()),
    ]
}

/// A variable the development shell sets.
pub fn shell_variable(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("{name} is not set: run the tests in the development shell, `nix develop`")
    })
}
