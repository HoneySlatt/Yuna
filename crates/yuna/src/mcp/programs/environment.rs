//! The environment a program gets, and a seat's `dbus-daemon`; and the seat a living process's
//! environment names.
//!
//! A program gets the few variables of Yuna's own that it needs to find itself, its libraries,
//! its locale and its drivers ([`KEPT_VARIABLES`], [`KEPT_PREFIXES`]), the system's
//! `XDG_CONFIG_DIRS` and `XDG_DATA_DIRS` among them, with the caller's own values of those
//! Yuna's package set for Yuna alone ([`WRAPPED_VARIABLES`]); `WAYLAND_DISPLAY`, the full path of
//! its seat's socket, and `XDG_SESSION_TYPE=wayland`; `DISPLAY` and `XAUTHORITY`, its seat's X
//! display and cookie, when the seat has one
//! ([ADR-0067](../../../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md));
//! and an `XDG_RUNTIME_DIR` of its own, empty, so that it finds neither the user's D-Bus nor
//! their sound server there. With a `DISPLAY`, `DBUS_SESSION_BUS_ADDRESS=disabled:`, so that
//! D-Bus's library does not launch a bus of its own through X11, which would hold the seat's
//! Xwayland. Nothing else: not Yuna's own `DISPLAY`, not the user's bus, not the user's `HOME`
//! nor their XDG base folders. The agent's own variables come next and win, but for
//! `YUNA_SEAT`, its seat's id, which comes last. Then its seat's throwaway home, `HOME` and the
//! XDG base folders in it, each one the agent did not give (`homes.rs`,
//! [ADR-0105](../../../../../docs/adr/0105-an-agents-programs-on-a-throwaway-home.md)); then
//! the accessibility variables, each one the agent did not give ([`add_accessibility_variables`]):
//! with `accessibility: true`, its seat's accessibility bus and the switches that turn every
//! toolkit's accessibility on ([`ACCESSIBILITY_VARIABLES`]); without, the two that keep GTK and
//! the ATK bridge from looking for a bus ([`NO_ACCESSIBILITY_VARIABLES`]). A seat's
//! `dbus-daemon` gets Yuna's kept variables alone ([`daemon_environment`]).
//!
//! The functions that build an environment are given Yuna's variables rather than read them, so
//! that the tests give them their own (`tests.rs`). Its neighbours: `mod.rs` starts each program
//! with one; `../spawn.rs` starts a seat's bus with another.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::Path;

use yuna_types::SeatId;

use crate::xwayland::X11Display;

/// The variables of Yuna's environment a program keeps. `HOME` and the XDG base folders are not
/// among them: a program gets its seat's throwaway home instead (`homes.rs`).
pub const KEPT_VARIABLES: &[&str] = &[
    "PATH",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TZDIR",
    "LOCALE_ARCHIVE",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_DIRS",
    "LD_LIBRARY_PATH",
    "XKB_CONFIG_ROOT",
    "FONTCONFIG_FILE",
    "FONTCONFIG_PATH",
    "LIBGL_DRIVERS_PATH",
    "LIBGL_ALWAYS_SOFTWARE",
    "GBM_BACKENDS_PATH",
    "__EGL_VENDOR_LIBRARY_FILENAMES",
    "__EGL_VENDOR_LIBRARY_DIRS",
];

/// The variable in which Yuna's package names, separated by spaces, the variables it set for
/// Yuna alone: its own libraries and Xwayland (`flake.nix`).
pub const WRAPPED_VARIABLES: &str = "YUNA_WRAPPED_VARIABLES";

/// The prefix of the variables in which Yuna's package keeps the caller's own value of each
/// variable [`WRAPPED_VARIABLES`] names, when the caller had one: a program gets it back.
pub const CALLER_PREFIX: &str = "YUNA_CALLER_";

/// The prefixes of the variables a program keeps: the locale's categories, and the settings of
/// Vulkan's loader and of Mesa.
pub const KEPT_PREFIXES: &[&str] = &["LC_", "VK_", "MESA_"];

/// The variables a program started with `accessibility: true` gets, beside
/// `AT_SPI_BUS_ADDRESS`, its seat's bus: GTK 4 and AccessKit find the bus through the address
/// alone; Qt turns on with it, `QT_LINUX_ACCESSIBILITY_ALWAYS_ON` past it; the ATK bridge of
/// GTK 3, Firefox, Chromium and Electron, which ask whether accessibility is on, read
/// `GNOME_ACCESSIBILITY` and `ACCESSIBILITY_ENABLED`. Chromium and Electron also need
/// `--force-renderer-accessibility` in their command for a page's own tree.
pub const ACCESSIBILITY_VARIABLES: &[(&str, &str)] = &[
    ("GNOME_ACCESSIBILITY", "1"),
    ("ACCESSIBILITY_ENABLED", "1"),
    ("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1"),
];

/// The variable that names an accessibility bus.
pub const BUS_VARIABLE: &str = "AT_SPI_BUS_ADDRESS";

/// The variables a program started without accessibility gets, so that no toolkit even looks
/// for a bus: the ATK bridge's switch and GTK 4's.
pub const NO_ACCESSIBILITY_VARIABLES: &[(&str, &str)] =
    &[("NO_AT_BRIDGE", "1"), ("GTK_A11Y", "none")];

/// A program's environment: the variables of `inherited` that [`KEPT_VARIABLES`] and
/// [`KEPT_PREFIXES`] name, then its own runtime folder, its seat's socket, the session's type,
/// `wayland`, and its seat's X display, `x11`, when it has one, then the agent's `added`, which
/// win, then `YUNA_SEAT`, which names its seat for its closing.
pub fn program_environment(
    inherited: impl Iterator<Item = (OsString, OsString)>,
    runtime_dir: &Path,
    wayland_display: &Path,
    x11: Option<&X11Display>,
    seat: SeatId,
    added: &BTreeMap<String, String>,
) -> BTreeMap<OsString, OsString> {
    let mut environment = kept_environment(inherited);
    environment.insert("XDG_RUNTIME_DIR".into(), runtime_dir.into());
    environment.insert("WAYLAND_DISPLAY".into(), wayland_display.into());
    environment.insert("XDG_SESSION_TYPE".into(), "wayland".into());
    if let Some(x11) = x11 {
        environment.insert("DISPLAY".into(), x11.name().into());
        environment.insert("XAUTHORITY".into(), x11.xauthority.clone().into());
        // Without a bus's address and with a DISPLAY, D-Bus's library launches a bus of its own
        // through X11 (`dbus-launch --autolaunch`), the machine's, which stays connected to the
        // seat's Xwayland and keeps it from ever stopping. `disabled:` says there is no bus.
        environment.insert("DBUS_SESSION_BUS_ADDRESS".into(), "disabled:".into());
    }
    for (name, value) in added {
        environment.insert(name.into(), value.into());
    }
    environment.insert(SEAT_VARIABLE.into(), seat.0.to_string().into());
    environment
}

/// Adds a program's accessibility variables to its `environment`, each but those the agent's
/// `added` give: with `bus`, its seat's accessibility bus's address, those that point every
/// toolkit at it and turn it on; without, those that keep GTK and the ATK bridge from looking for
/// one.
pub fn add_accessibility_variables(
    environment: &mut BTreeMap<OsString, OsString>,
    bus: Option<&str>,
    added: &BTreeMap<String, String>,
) {
    let mut add = |name: &str, value: &str| {
        if !added.contains_key(name) {
            environment.insert(name.into(), value.into());
        }
    };
    match bus {
        Some(address) => {
            add(BUS_VARIABLE, address);
            for (name, value) in ACCESSIBILITY_VARIABLES {
                add(name, value);
            }
        }
        None => {
            for (name, value) in NO_ACCESSIBILITY_VARIABLES {
                add(name, value);
            }
        }
    }
}

/// The environment of a seat's `dbus-daemon`: Yuna's kept variables, no session bus, and its
/// seat's `YUNA_SEAT`, which finds it among the seat's processes.
pub fn daemon_environment(
    inherited: impl Iterator<Item = (OsString, OsString)>,
    seat: SeatId,
) -> BTreeMap<OsString, OsString> {
    let mut environment = kept_environment(inherited);
    environment.insert("DBUS_SESSION_BUS_ADDRESS".into(), "disabled:".into());
    environment.insert(SEAT_VARIABLE.into(), seat.0.to_string().into());
    environment
}

/// The variables of `inherited` that [`KEPT_VARIABLES`] and [`KEPT_PREFIXES`] name, those Yuna's
/// package set for Yuna alone given back their caller's values.
fn kept_environment(
    inherited: impl Iterator<Item = (OsString, OsString)>,
) -> BTreeMap<OsString, OsString> {
    let kept = |name: &OsStr| {
        name.to_str().is_some_and(|name| {
            KEPT_VARIABLES.contains(&name) || KEPT_PREFIXES.iter().any(|p| name.starts_with(p))
        })
    };
    let inherited: BTreeMap<OsString, OsString> = inherited.collect();
    let mut environment: BTreeMap<OsString, OsString> = inherited
        .iter()
        .filter(|(name, _)| kept(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    // The variables Yuna's package set for Yuna alone (flake.nix) go back to the caller's own.
    let wrapped = inherited
        .get(OsStr::new(WRAPPED_VARIABLES))
        .and_then(|names| names.to_str())
        .unwrap_or_default();
    for name in wrapped.split_whitespace() {
        match inherited.get(OsStr::new(&format!("{CALLER_PREFIX}{name}"))) {
            Some(value) => environment.insert(name.into(), value.clone()),
            None => environment.remove(OsStr::new(name)),
        };
    }
    environment
}

/// The variable that names a program's seat, which its processes inherit.
pub const SEAT_VARIABLE: &str = "YUNA_SEAT";

/// The seat process `pid`'s environment names, as it was when the process started: its
/// `YUNA_SEAT`.
pub(super) fn seat_in_environment(pid: i32) -> Option<SeatId> {
    let environment = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let prefix = format!("{SEAT_VARIABLE}=");
    environment
        .split(|&byte| byte == 0)
        .find_map(|entry| entry.strip_prefix(prefix.as_bytes()))
        .and_then(|value| std::str::from_utf8(value).ok()?.parse().ok())
        .map(SeatId)
}
