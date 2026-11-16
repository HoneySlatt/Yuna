//! The sandbox each compositor and each client runs in.
//!
//! - **A transient systemd user scope of its own** (`systemd-run --user --scope`): the whole tree
//!   of processes it starts is one control group, whose `cpu.stat` counts everything they ever
//!   ran. A compositor's scope is delegated: the compositor's own process moves itself into a
//!   child group, `core` ([`enter_core`]), before it runs, so that its descendants are there too,
//!   and its session bus stays outside. A compositor that runs inside another, its parent, is
//!   started by [`run_parent`], which keeps the parent outside `core` too.
//! - **An environment built from nothing** ([`compositor_environment`], [`client_environment`]):
//!   `env -i` clears what `systemd-run` passes on, then the bench sets only store paths and the
//!   run's own folders: its own `XDG_RUNTIME_DIR` and `XDG_*_HOME` under the run's folder, no
//!   `WAYLAND_DISPLAY`, `DISPLAY`, `SWAYSOCK`, `NIRI_SOCKET`, `HYPRLAND_INSTANCE_SIGNATURE` or
//!   `DBUS_SESSION_BUS_ADDRESS` of the owner's.
//! - **A private session bus**: `dbus-run-session` with a configuration of the bench's, which
//!   listens in the run's runtime folder and activates no service of the machine's.
//! - **Refusals** ([`check`]): if any process of the scope holds a `/dev/dri/card*` node or an
//!   input device open ([`FORBIDDEN_NODES`]), is bound or connected to a socket in the owner's
//!   runtime folder, or was started with a variable naming that folder, the bench stops the
//!   compositor and measures nothing.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::cgroup::{self, Cgroup};
use crate::procfs::Proc;
use crate::sockets::{SocketTable, socket_inode};

/// The variables that would lead a program to the owner's own session; none of them is ever set
/// from the bench's environment.
pub const OWNER_VARIABLES: [&str; 13] = [
    "WAYLAND_DISPLAY",
    "WAYLAND_SOCKET",
    "DISPLAY",
    "XAUTHORITY",
    "SWAYSOCK",
    "I3SOCK",
    "NIRI_SOCKET",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_SESSION_ID",
    "XDG_SEAT",
    "XDG_VTNR",
    "PIPEWIRE_REMOTE",
];

/// The render node every compositor and client draws on: never a card node, which drives the
/// owner's display.
pub const RENDER_NODE: &str = "/dev/dri/renderD128";

/// The store paths the bench's shell gives (`flake.nix`, `devShells.bench`).
#[derive(Debug, Clone, PartialEq)]
pub struct ShellPaths {
    /// sway's package, unwrapped.
    pub sway: PathBuf,
    /// KWin's package.
    pub kwin: PathBuf,
    /// dwl as nixpkgs ships it, with XWayland.
    pub dwl: PathBuf,
    /// dwl built without XWayland, for equal work.
    pub dwl_no_xwayland: PathBuf,
    /// mango as nixpkgs ships it, with XWayland.
    pub mango: PathBuf,
    /// mango built without XWayland, for equal work.
    pub mango_no_xwayland: PathBuf,
    /// Umbriel, at the revision the flake pins.
    pub umbriel: PathBuf,
    /// Hyprland's package.
    pub hyprland: PathBuf,
    /// Mutter's package.
    pub mutter: PathBuf,
    /// wlr-randr, a client of wlr-output-management.
    pub wlr_randr: PathBuf,
    /// weston's package, for `weston-presentation-shm`.
    pub weston: PathBuf,
    /// ffmpeg's binaries.
    pub ffmpeg: PathBuf,
    /// D-Bus's package: `dbus-run-session`, `dbus-daemon`, `dbus-send`.
    pub dbus: PathBuf,
    /// The Mesa every program draws with.
    pub mesa: PathBuf,
    /// Xwayland's package.
    pub xwayland: PathBuf,
    /// coreutils.
    pub coreutils: PathBuf,
    /// bash.
    pub bash: PathBuf,
    /// The applications' `PATH`: `mpv` among them.
    pub applications_path: String,
    /// A fontconfig file naming DejaVu alone.
    pub fonts_conf: PathBuf,
    /// The keyboard's layouts.
    pub xkb_config_root: PathBuf,
    /// libglvnd's folder of libraries, EGL's dispatcher.
    pub library_path: String,
    /// The EGL vendor file of the shell's Mesa.
    pub egl_vendor: PathBuf,
}

impl ShellPaths {
    /// Reads them from the shell's variables; each must be in the Nix store, so that nothing of
    /// the machine's own or the owner's reaches a compositor through them.
    pub fn from_environment() -> Result<ShellPaths, String> {
        let variable = |name: &str| -> Result<String, String> {
            let value = std::env::var(name).map_err(|_| {
                format!("{name} is not set: run in the bench's shell, `nix develop .#bench`")
            })?;
            if value.split(':').all(|part| part.starts_with("/nix/store/")) {
                Ok(value)
            } else {
                Err(format!("{name} is {value:?}, outside the Nix store"))
            }
        };
        let path = |name: &str| variable(name).map(PathBuf::from);
        Ok(ShellPaths {
            sway: path("YUNA_BENCH_SWAY")?,
            kwin: path("YUNA_BENCH_KWIN")?,
            dwl: path("YUNA_BENCH_DWL")?,
            dwl_no_xwayland: path("YUNA_BENCH_DWL_NO_XWAYLAND")?,
            mango: path("YUNA_BENCH_MANGO")?,
            mango_no_xwayland: path("YUNA_BENCH_MANGO_NO_XWAYLAND")?,
            umbriel: path("YUNA_BENCH_UMBRIEL")?,
            hyprland: path("YUNA_BENCH_HYPRLAND")?,
            mutter: path("YUNA_BENCH_MUTTER")?,
            wlr_randr: path("YUNA_BENCH_WLR_RANDR")?,
            weston: path("YUNA_BENCH_WESTON")?,
            ffmpeg: path("YUNA_BENCH_FFMPEG")?,
            dbus: path("YUNA_BENCH_DBUS")?,
            mesa: path("YUNA_BENCH_MESA")?,
            xwayland: path("YUNA_BENCH_XWAYLAND")?,
            coreutils: path("YUNA_BENCH_COREUTILS")?,
            bash: path("YUNA_BENCH_BASH")?,
            applications_path: variable("YUNA_APPLICATIONS_PATH")?,
            fonts_conf: path("YUNA_FONTS_CONF")?,
            xkb_config_root: path("XKB_CONFIG_ROOT")?,
            library_path: variable("YUNA_BENCH_LIBRARY_PATH")?,
            egl_vendor: path("__EGL_VENDOR_LIBRARY_FILENAMES")?,
        })
    }

    /// The Vulkan driver of the shell's Mesa for AMD GPUs, radv.
    pub fn radv(&self) -> PathBuf {
        self.mesa.join("share/vulkan/icd.d/radeon_icd.x86_64.json")
    }
}

/// What belongs to the owner and must never be reached: their runtime folders.
#[derive(Debug, Clone, PartialEq)]
pub struct Owner {
    /// `/run/user/<uid>`, and the bench's own runtime folder if it differs.
    pub runtime_folders: Vec<PathBuf>,
}

/// This process's user's runtime folder, `/run/user/<uid>`, which systemd makes at their login:
/// the owner's, where their display's socket and session bus are. An agents' service runs as the
/// same user with an `XDG_RUNTIME_DIR` of its own beneath it.
pub fn user_runtime_folder() -> PathBuf {
    PathBuf::from(format!("/run/user/{}", rustix::process::getuid().as_raw()))
}

impl Owner {
    /// The owner of this process: its user's runtime folders.
    pub fn of_this_process() -> Owner {
        let mut runtime_folders = vec![user_runtime_folder()];
        for name in ["OWNER_RUNTIME_DIR", "XDG_RUNTIME_DIR"] {
            if let Some(folder) = std::env::var_os(name).map(PathBuf::from)
                && !runtime_folders.contains(&folder)
            {
                runtime_folders.push(folder);
            }
        }
        Owner { runtime_folders }
    }

    /// Whether `text`, a path, a list of paths or a bus address, names one of the owner's runtime
    /// folders or a file in one: the folder's path, bounded on both sides by the text's ends or
    /// a separator (`:`, `=`, `,`, `;`, a space; and `/` after it).
    pub fn names_runtime_folder(&self, text: &str) -> bool {
        self.runtime_folders.iter().any(|folder| {
            let folder = folder.to_string_lossy();
            text.match_indices(folder.as_ref()).any(|(at, _)| {
                let before = text[..at].chars().next_back();
                let after = text[at + folder.len()..].chars().next();
                before.is_none_or(|c| ":=,; ".contains(c))
                    && after.is_none_or(|c| "/:,; ".contains(c))
            })
        })
    }
}

/// The folders one compositor's or client's sandbox gets, under the run's folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Folders {
    /// The case's folder: its logs and configuration.
    pub root: PathBuf,
    /// A short path for its runtime folder, `/tmp/yb-<pid>-<n>`, which a compositor whose
    /// sockets' paths would pass Unix's 108 bytes under the run's folder is given: a link to the
    /// runtime folder, made with the compositor's start, removed at its end. Hyprland's instance
    /// folder alone is 62 characters long.
    pub short_runtime: PathBuf,
}

impl Folders {
    /// Makes `root` and its folders; the runtime folder is the user's alone, as Wayland asks.
    pub fn create(root: &Path) -> Result<Folders, String> {
        static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let made = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let folders = Folders {
            root: root.to_owned(),
            short_runtime: PathBuf::from(format!("/tmp/yb-{}-{made}", std::process::id())),
        };
        for folder in [
            folders.home(),
            folders.config(),
            folders.data(),
            folders.cache(),
            folders.state(),
            folders.config_dirs(),
            folders.data_dirs(),
        ] {
            fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folders.runtime())
            .map_err(|e| format!("{}: {e}", folders.runtime().display()))?;
        Ok(folders)
    }

    /// Its `XDG_RUNTIME_DIR`, where the compositor's sockets go.
    pub fn runtime(&self) -> PathBuf {
        self.root.join("runtime")
    }
    /// Its `HOME`.
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    /// Its `XDG_CONFIG_HOME`.
    pub fn config(&self) -> PathBuf {
        self.root.join("config")
    }
    /// Its `XDG_DATA_HOME`.
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }
    /// Its `XDG_CACHE_HOME`.
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
    /// Its `XDG_STATE_HOME`.
    pub fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    /// An empty `XDG_CONFIG_DIRS`, so that nothing of the machine's `/etc/xdg` is read.
    pub fn config_dirs(&self) -> PathBuf {
        self.root.join("config-dirs")
    }
    /// An empty `XDG_DATA_DIRS`, so that nothing of the machine's `/usr/share` is read.
    pub fn data_dirs(&self) -> PathBuf {
        self.root.join("data-dirs")
    }
    /// The private session bus's socket.
    pub fn bus(&self) -> PathBuf {
        self.runtime().join("bus")
    }
}

/// An environment, as ordered pairs.
pub type Environment = Vec<(String, String)>;

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// What every program the bench starts gets: its own folders, the shell's Mesa, fonts and
/// keyboard, and nothing of the owner's.
fn common_environment(folders: &Folders, shell: &ShellPaths) -> Environment {
    vec![
        ("HOME".to_owned(), text(&folders.home())),
        ("XDG_RUNTIME_DIR".to_owned(), text(&folders.runtime())),
        ("XDG_CONFIG_HOME".to_owned(), text(&folders.config())),
        ("XDG_DATA_HOME".to_owned(), text(&folders.data())),
        ("XDG_CACHE_HOME".to_owned(), text(&folders.cache())),
        ("XDG_STATE_HOME".to_owned(), text(&folders.state())),
        ("XDG_CONFIG_DIRS".to_owned(), text(&folders.config_dirs())),
        ("XDG_DATA_DIRS".to_owned(), text(&folders.data_dirs())),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ("LD_LIBRARY_PATH".to_owned(), shell.library_path.clone()),
        (
            "__EGL_VENDOR_LIBRARY_FILENAMES".to_owned(),
            text(&shell.egl_vendor),
        ),
        // libgbm looks for its backend in the host's /run/opengl-driver otherwise.
        (
            "GBM_BACKENDS_PATH".to_owned(),
            text(&shell.mesa.join("lib/gbm")),
        ),
        ("VK_DRIVER_FILES".to_owned(), text(&shell.radv())),
        // The host's implicit layers (overlays, capture tools) stay out.
        (
            "VK_LOADER_LAYERS_DISABLE".to_owned(),
            "~implicit~".to_owned(),
        ),
        ("FONTCONFIG_FILE".to_owned(), text(&shell.fonts_conf)),
        ("XKB_CONFIG_ROOT".to_owned(), text(&shell.xkb_config_root)),
    ]
}

/// A compositor's environment: the common one, its `PATH`, and the recipe's own variables,
/// which replace the common ones of the same name: one value a variable.
pub fn compositor_environment(
    folders: &Folders,
    shell: &ShellPaths,
    path: &str,
    recipe_variables: &[(String, String)],
) -> Environment {
    let mut environment = common_environment(folders, shell);
    environment.push(("PATH".to_owned(), path.to_owned()));
    environment.retain(|(key, _)| !recipe_variables.iter().any(|(own, _)| own == key));
    environment.extend(recipe_variables.iter().cloned());
    environment
}

/// A client's environment: the common one, its `PATH`, and the compositor's Wayland socket,
/// which is in the compositor's runtime folder, shared with the client.
pub fn client_environment(
    folders: &Folders,
    shell: &ShellPaths,
    path: &str,
    wayland_display: &str,
) -> Environment {
    let mut environment = common_environment(folders, shell);
    environment.push(("PATH".to_owned(), path.to_owned()));
    environment.push(("WAYLAND_DISPLAY".to_owned(), wayland_display.to_owned()));
    environment
}

/// The configuration of a private session bus listening at `socket`, with no service to
/// activate: none of the machine's or the owner's can start in the sandbox.
pub fn bus_configuration(socket: &Path) -> String {
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        socket.display()
    )
}

/// What to start in a sandbox.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    /// The transient unit's name, `<name>.scope`.
    pub unit: String,
    /// The program and its arguments.
    pub command: Vec<String>,
    /// Its whole environment.
    pub environment: Environment,
    /// Its working folder.
    pub folder: PathBuf,
    /// Where its standard output and error go.
    pub log: PathBuf,
    /// Whether it gets a private session bus and its own `core` group: a compositor does, a
    /// client does not.
    pub compositor: bool,
    /// The compositor it runs inside, started first in the same sandbox, if it needs one.
    pub parent: Option<Parent>,
    /// Whether its standard input is a pipe the bench writes to; else it reads nothing.
    pub input: bool,
}

/// A compositor that another runs inside, in the same sandbox: `yuna-bench parent` starts it
/// first, outside `core`, so that its costs count in the stack alone, waits for its Wayland
/// socket, then starts the compositor into `core`, connected to it, and stops it once the
/// compositor has ended. Hyprland needs one: its headless backend cannot allocate buffers
/// alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parent {
    /// Its program and arguments.
    pub command: Vec<String>,
    /// Its own variables, which the compositor does not get, beside the sandbox's.
    pub variables: Vec<(String, String)>,
    /// Its runtime folder, inside the compositor's: its sockets stay apart from the
    /// compositor's. The Wayland socket it makes there is the compositor's `WAYLAND_DISPLAY`.
    pub runtime: PathBuf,
    /// Where its standard output and error go.
    pub log: PathBuf,
}

/// What `yuna-bench parent` reads: the parent, and the compositor to start inside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParentLaunch {
    /// The parent.
    pub parent: Parent,
    /// The compositor's program and arguments.
    pub compositor: Vec<String>,
}

/// The file a launch with a parent writes its [`ParentLaunch`] to, in its folder.
fn parent_launch_file(launch: &Launch) -> PathBuf {
    launch.folder.join("parent-launch.json")
}

/// The command line that starts `launch`: `systemd-run` makes the scope, `env -i` clears what it
/// passed on and sets `launch`'s environment alone; a compositor then gets its session bus, and
/// moves into its `core` group through this program's `enter-core`, or, inside a parent, is
/// started by this program's `parent`, which reads the rest from the launch's file.
pub fn command_line(
    launch: &Launch,
    shell: &ShellPaths,
    bench: &Path,
    bus_configuration: &Path,
) -> Vec<String> {
    let mut line: Vec<String> = [
        "systemd-run",
        "--user",
        "--scope",
        "--quiet",
        "--collect",
        "--unit",
        &launch.unit,
    ]
    .iter()
    .map(|word| (*word).to_owned())
    .collect();
    if launch.compositor {
        line.extend(["--property".to_owned(), "Delegate=yes".to_owned()]);
    }
    line.push("--".to_owned());
    line.push(text(&shell.coreutils.join("bin/env")));
    line.push("-i".to_owned());
    line.extend(
        launch
            .environment
            .iter()
            .map(|(key, value)| format!("{key}={value}")),
    );
    if launch.compositor {
        line.push(text(&shell.dbus.join("bin/dbus-run-session")));
        line.push(format!("--config-file={}", bus_configuration.display()));
        line.push(format!(
            "--dbus-daemon={}",
            shell.dbus.join("bin/dbus-daemon").display()
        ));
        line.push("--".to_owned());
        line.push(text(bench));
        if launch.parent.is_some() {
            line.push("parent".to_owned());
            line.push(text(&parent_launch_file(launch)));
            return line;
        }
        line.push("enter-core".to_owned());
        line.push("--".to_owned());
    }
    line.extend(launch.command.iter().cloned());
    line
}

/// A program running in its scope.
pub struct Sandboxed {
    /// `systemd-run`'s process, which became the program, or the session bus's runner.
    pub child: Child,
    /// The scope's group.
    pub scope: Cgroup,
    /// Its log.
    pub log: PathBuf,
}

impl Sandboxed {
    /// Starts `launch` in a new scope, and waits until the scope holds it.
    pub fn start(launch: &Launch, shell: &ShellPaths, bench: &Path) -> Result<Sandboxed, String> {
        let bus_conf = launch.folder.join("session-bus.conf");
        if launch.compositor {
            let socket = launch
                .environment
                .iter()
                .find(|(key, _)| key == "XDG_RUNTIME_DIR")
                .map(|(_, folder)| Path::new(folder).join("bus"))
                .ok_or("a compositor's environment names no XDG_RUNTIME_DIR")?;
            fs::write(&bus_conf, bus_configuration(&socket))
                .map_err(|e| format!("{}: {e}", bus_conf.display()))?;
            if let Some(parent) = &launch.parent {
                let file = parent_launch_file(launch);
                let text = serde_json::to_string_pretty(&ParentLaunch {
                    parent: parent.clone(),
                    compositor: launch.command.clone(),
                })
                .map_err(|e| e.to_string())?;
                fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))?;
            }
        }
        let line = command_line(launch, shell, bench, &bus_conf);
        let log =
            File::create(&launch.log).map_err(|e| format!("{}: {e}", launch.log.display()))?;
        let mut child = Command::new(&line[0])
            .args(&line[1..])
            .current_dir(&launch.folder)
            // systemd-run, then env, become the program: the pipe reaches it.
            .stdin(if launch.input {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(log.try_clone().map_err(|e| format!("a log: {e}"))?)
            .stderr(log)
            .spawn()
            .map_err(|e| format!("systemd-run does not start: {e}"))?;
        let proc = Proc::system();
        let deadline = Instant::now() + Duration::from_secs(10);
        let suffix = format!("/{}.scope", launch.unit);
        loop {
            if let Some(group) = proc.cgroup(child.id()).filter(|g| g.ends_with(&suffix)) {
                return Ok(Sandboxed {
                    child,
                    scope: Cgroup::named(Path::new(cgroup::ROOT), &group),
                    log: launch.log.clone(),
                });
            }
            if let Ok(Some(status)) = child.try_wait() {
                let log = fs::read_to_string(&launch.log).unwrap_or_default();
                return Err(format!(
                    "{} ended with {status} before its scope held it: {}",
                    launch.unit,
                    log.trim()
                ));
            }
            if Instant::now() > deadline {
                // Still outside any scope of its own: systemd-run itself, the only process.
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} never reached its scope", launch.unit));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The compositor's own group, `core`, once the compositor moved into it.
    pub fn core(&self) -> Cgroup {
        self.scope.child("core")
    }

    /// Whether `systemd-run`'s process has ended, and how.
    pub fn ended(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            _ => None,
        }
    }

    /// Waits up to `patience` for every process of the scope to end, then kills those left;
    /// says how it ended.
    pub fn wait_or_kill(&mut self, patience: Duration) -> String {
        let deadline = Instant::now() + patience;
        while Instant::now() < deadline {
            if !self.scope.is_populated() {
                let status = self.child.wait().map(|s| s.to_string()).unwrap_or_default();
                return format!("ended: {status}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let killed = self.scope.kill();
        let status = self.child.wait().map(|s| s.to_string()).unwrap_or_default();
        match killed {
            Ok(()) => format!("killed after {} s: {status}", patience.as_secs()),
            Err(error) => format!("could not be killed ({error}): {status}"),
        }
    }
}

impl Drop for Sandboxed {
    fn drop(&mut self) {
        // A sandbox left behind by an error ends with the bench, whatever runs in it.
        if self.scope.is_populated() {
            let _ = self.scope.kill();
        }
        let _ = self.child.wait();
    }
}

/// `yuna-bench enter-core -- <program> <arguments>`: moves this process into a child group
/// `core` of its own group, then becomes the program. Run by the compositor's scope, after the
/// session bus started, so that only the compositor and its descendants are in `core`.
pub fn enter_core(command: &[String]) -> Result<(), String> {
    let (program, arguments) = command.split_first().ok_or("enter-core needs a program")?;
    let group = Proc::system()
        .cgroup(std::process::id())
        .ok_or("this process has no control group")?;
    let core = Cgroup::named(Path::new(cgroup::ROOT), &group).child("core");
    fs::create_dir_all(core.folder()).map_err(|e| format!("{}: {e}", core.folder().display()))?;
    fs::write(
        core.folder().join("cgroup.procs"),
        std::process::id().to_string(),
    )
    .map_err(|e| format!("cannot enter {}: {e}", core.folder().display()))?;
    let error = Command::new(program).args(arguments).exec();
    Err(format!("{program} does not start: {error}"))
}

/// `yuna-bench parent <file>`: starts the parent of `file`'s [`ParentLaunch`], waits for its
/// Wayland socket, then starts the compositor through `enter-core`, connected to it; once the
/// compositor has ended, stops the parent. Gives whether the compositor ended well. Run by the
/// compositor's scope, after the session bus started: this process and the parent stay outside
/// `core`.
pub fn run_parent(file: &Path) -> Result<bool, String> {
    let text = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let launch: ParentLaunch =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let parent = &launch.parent;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&parent.runtime)
        .map_err(|e| format!("{}: {e}", parent.runtime.display()))?;
    let log = File::create(&parent.log).map_err(|e| format!("{}: {e}", parent.log.display()))?;
    let (program, arguments) = parent
        .command
        .split_first()
        .ok_or("a parent needs a program")?;
    let mut started = Command::new(program)
        .args(arguments)
        .envs(parent.variables.iter().map(|(key, value)| (key, value)))
        .env("XDG_RUNTIME_DIR", &parent.runtime)
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| format!("a log: {e}"))?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("{program} does not start: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let socket = loop {
        if let Some(socket) = wayland_socket_in(&parent.runtime) {
            break socket;
        }
        if let Ok(Some(status)) = started.try_wait() {
            return Err(format!("the parent {program} ended with {status}"));
        }
        if Instant::now() > deadline {
            let _ = started.kill();
            let _ = started.wait();
            return Err(format!("the parent {program} made no socket in 60 s"));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let bench = std::env::current_exe().map_err(|e| format!("this program: {e}"))?;
    let compositor = Command::new(bench)
        .arg("enter-core")
        .arg("--")
        .args(&launch.compositor)
        .env("WAYLAND_DISPLAY", &socket)
        .stdin(Stdio::null())
        .status();
    let _ = signal(started.id(), rustix::process::Signal::TERM);
    let deadline = Instant::now() + Duration::from_secs(5);
    while started.try_wait().ok().flatten().is_none() {
        if Instant::now() > deadline {
            let _ = started.kill();
            let _ = started.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    compositor
        .map(|status| status.success())
        .map_err(|e| format!("the compositor does not start: {e}"))
}

/// The Wayland socket a compositor made in `runtime`, `wayland-<n>`, its lock left out.
pub fn wayland_socket_in(runtime: &Path) -> Option<PathBuf> {
    let mut sockets: Vec<PathBuf> = fs::read_dir(runtime)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("wayland-") && !name.ends_with(".lock"))
        })
        .collect();
    sockets.sort();
    sockets.into_iter().next()
}

/// The device nodes no sandboxed process may hold: the display's card nodes, which drive the
/// owner's monitors, and every input device, through which it could read or make the owner's
/// input.
pub const FORBIDDEN_NODES: [&str; 4] =
    ["/dev/dri/card", "/dev/input/", "/dev/uinput", "/dev/hidraw"];

/// The X11 displays' sockets, abstract and in the file system. A sandboxed process may serve one,
/// as sway's lazy XWayland does; it may never be connected to one served outside its sandbox,
/// such as the owner's display.
pub const X11_SOCKETS: [&str; 2] = ["@/tmp/.X11-unix/X", "/tmp/.X11-unix/X"];

/// What a check of a sandbox found against its rules, one line each; empty when it holds. The
/// sandbox is `groups`: the compositor's scope and its clients'. A process breaks them if it holds
/// a node of [`FORBIDDEN_NODES`] open, is bound or connected to a socket in the owner's runtime
/// folders, is connected to an X11 display served outside the sandbox, or was started with a
/// variable naming the owner's runtime folders.
pub fn check(proc: &Proc, groups: &[&Cgroup], owner: &Owner, sockets: &SocketTable) -> Vec<String> {
    /// A process of the sandbox and what it holds open.
    struct Held {
        pid: u32,
        name: String,
        files: Vec<(u32, PathBuf)>,
    }
    let processes: Vec<Held> = groups
        .iter()
        .flat_map(|group| group.processes())
        .map(|pid| Held {
            pid,
            name: proc.name(pid).unwrap_or_default(),
            files: proc.open_files(pid),
        })
        .collect();
    // The sockets the sandbox itself holds: a peer among them is inside it.
    let inside: BTreeSet<u64> = processes
        .iter()
        .flat_map(|held| &held.files)
        .filter_map(|(_, target)| socket_inode(&target.to_string_lossy()))
        .collect();
    let mut found = Vec::new();
    for Held { pid, name, files } in &processes {
        for (fd, target) in files {
            let target = target.to_string_lossy();
            if FORBIDDEN_NODES.iter().any(|node| target.starts_with(node)) {
                found.push(format!(
                    "process {pid} ({name}) holds {target} open, fd {fd}"
                ));
                continue;
            }
            let Some(inode) = socket_inode(&target) else {
                continue;
            };
            if let Some(path) = sockets.reaches(inode)
                && owner.names_runtime_folder(path)
            {
                found.push(format!(
                    "process {pid} ({name}) reaches the owner's socket {path}, fd {fd}"
                ));
            } else if let Some(peer) = sockets.peers.get(&inode)
                && !inside.contains(peer)
                && let Some(path) = sockets.paths.get(peer)
                && X11_SOCKETS.iter().any(|x11| path.starts_with(x11))
            {
                found.push(format!(
                    "process {pid} ({name}) is connected to the X11 display {path}, served \
                     outside its sandbox, fd {fd}"
                ));
            }
        }
        for (key, value) in proc.environment(*pid).unwrap_or_default() {
            if owner.names_runtime_folder(&value) {
                found.push(format!(
                    "process {pid} ({name}) was started with {key}={value}, the owner's"
                ));
            }
        }
    }
    found
}

/// Marks close-on-exec every descriptor above 2 this process inherited without it, such as the
/// lock `run.sh --exclusive` holds on descriptor 9 or `cargo run`'s jobserver pipes: none may
/// reach a compositor or a client. Gives the descriptors marked. The bench calls it first, before
/// it starts any thread; every descriptor the standard library opens is close-on-exec already.
pub fn close_inherited_on_exec() -> Vec<i32> {
    let mut marked = Vec::new();
    let descriptors: Vec<i32> = fs::read_dir("/proc/self/fd")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .filter(|fd| *fd > 2)
        .collect();
    for fd in descriptors {
        let Ok(info) = fs::read_to_string(format!("/proc/self/fdinfo/{fd}")) else {
            // Closed since the listing, such as the listing's own descriptor.
            continue;
        };
        if !inheritable(&info) {
            continue;
        }
        // SAFETY: `fd` is open: it was listed in /proc/self/fd and its fdinfo read just now, and
        // nothing in this process closes it meanwhile. No `File` or `OwnedFd` of this process
        // owns it: the standard library opens every descriptor close-on-exec, so one without the
        // flag was inherited, and the bench calls this before any thread of its own exists. The
        // borrow lasts for the one `fcntl` call.
        let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
        if rustix::io::fcntl_setfd(borrowed, rustix::io::FdFlags::CLOEXEC).is_ok() {
            marked.push(fd);
        }
    }
    marked
}

/// Whether an `fdinfo` says its descriptor is inherited by a program it executes: its `flags`,
/// in octal, lack `O_CLOEXEC`.
fn inheritable(fdinfo: &str) -> bool {
    const O_CLOEXEC: u32 = 0o2_000_000;
    fdinfo
        .lines()
        .find_map(|line| line.strip_prefix("flags:"))
        .and_then(|flags| u32::from_str_radix(flags.trim(), 8).ok())
        .is_some_and(|flags| flags & O_CLOEXEC == 0)
}

/// Kills the scopes a bench left behind when it was killed itself, before it could stop them:
/// those of `app.slice` named `yuna-bench-<pid>-…` whose bench, process `<pid>`, is gone. Gives
/// their names.
pub fn sweep_left_behind(proc: &Proc) -> Vec<String> {
    let Some(own) = proc.cgroup(std::process::id()) else {
        return Vec::new();
    };
    // The user manager's app.slice: `/user.slice/user-<uid>.slice/user@<uid>.service/app.slice`.
    let Some(manager) = own
        .find(".service/")
        .map(|end| &own[..end + ".service".len()])
    else {
        return Vec::new();
    };
    let slice = Cgroup::named(Path::new(cgroup::ROOT), &format!("{manager}/app.slice"));
    let mut swept = Vec::new();
    for entry in fs::read_dir(slice.folder()).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(bench) = name
            .strip_prefix("yuna-bench-")
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let alive = proc
            .name(bench)
            .is_some_and(|program| program.starts_with("yuna-bench"));
        if !alive && slice.child(&name).kill().is_ok() {
            swept.push(name);
        }
    }
    swept
}

/// Removes the short runtime links ([`Folders::short_runtime`]) in `folder` that a bench killed
/// earlier left: `yb-<pid>-<n>`, whose bench, process `<pid>`, is gone. Gives their names.
pub fn sweep_runtime_links(proc: &Proc, folder: &Path) -> Vec<String> {
    let mut swept = Vec::new();
    for entry in fs::read_dir(folder).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(bench) = name
            .strip_prefix("yb-")
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let is_link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
        let alive = proc
            .name(bench)
            .is_some_and(|program| program.starts_with("yuna-bench"));
        if is_link && !alive && fs::remove_file(entry.path()).is_ok() {
            swept.push(name);
        }
    }
    swept
}

/// Sends `signal` to process `pid`.
pub fn signal(pid: u32, signal: rustix::process::Signal) -> io::Result<()> {
    let pid = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or_else(|| io::Error::other(format!("no process {pid}")))?;
    rustix::process::kill_process(pid, signal).map_err(io::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> ShellPaths {
        crate::testing::shell()
    }

    fn owner() -> Owner {
        Owner {
            runtime_folders: vec![
                PathBuf::from("/run/user/1000"),
                PathBuf::from("/run/user/1000/agents"),
            ],
        }
    }

    #[test]
    fn the_sandboxs_environment_holds_none_of_the_owners() {
        let folders = Folders {
            root: PathBuf::from("/tmp/yuna-bench-1/sway-1"),
            short_runtime: PathBuf::from("/tmp/yb-1-0"),
        };
        let variables = [("WLR_BACKENDS".to_owned(), "headless".to_owned())];
        let compositor =
            compositor_environment(&folders, &shell(), "/nix/store/aaaa-sway/bin", &variables);
        let client = client_environment(&folders, &shell(), "/nix/store/aaaa-mpv/bin", "wayland-1");
        let owner = owner();
        for (environment, which) in [(&compositor, "compositor"), (&client, "client")] {
            for (key, value) in environment {
                assert!(
                    !owner.names_runtime_folder(value),
                    "{which}: {key}={value} names the owner's runtime folder"
                );
                assert!(
                    !value.contains("/home/"),
                    "{which}: {key}={value} names a home"
                );
                let allowed = which == "client" && key == "WAYLAND_DISPLAY";
                assert!(
                    allowed || !OWNER_VARIABLES.contains(&key.as_str()),
                    "{which}: {key} is the owner's variable"
                );
            }
            let runtime = environment.iter().find(|(key, _)| key == "XDG_RUNTIME_DIR");
            assert_eq!(
                runtime.map(|(_, value)| value.as_str()),
                Some("/tmp/yuna-bench-1/sway-1/runtime")
            );
        }
        // The client's display is the compositor's socket, in the run's own runtime folder.
        assert!(client.contains(&("WAYLAND_DISPLAY".to_owned(), "wayland-1".to_owned())));
        // A recipe's own value replaces the sandbox's: one value a variable.
        let short = [("XDG_RUNTIME_DIR".to_owned(), "/tmp/yb-1-0".to_owned())];
        let own =
            compositor_environment(&folders, &shell(), "/nix/store/aaaa-hyprland/bin", &short);
        let runtimes: Vec<&String> = own
            .iter()
            .filter(|(key, _)| key == "XDG_RUNTIME_DIR")
            .map(|(_, value)| value)
            .collect();
        assert_eq!(runtimes, ["/tmp/yb-1-0"]);
    }

    #[test]
    fn the_command_line_clears_the_environment_before_setting_it() {
        let launch = Launch {
            unit: "yuna-bench-1-sway-1".to_owned(),
            command: vec!["/nix/store/aaaa-sway/bin/sway".to_owned()],
            environment: vec![("HOME".to_owned(), "/tmp/run/home".to_owned())],
            folder: PathBuf::from("/tmp/run"),
            log: PathBuf::from("/tmp/run/log"),
            compositor: true,
            parent: None,
            input: false,
        };
        let line = command_line(
            &launch,
            &shell(),
            Path::new("/bench"),
            Path::new("/tmp/run/bus.conf"),
        );
        let env = line
            .iter()
            .position(|word| word.ends_with("/bin/env"))
            .unwrap();
        assert_eq!(line[env + 1], "-i");
        assert_eq!(line[env + 2], "HOME=/tmp/run/home");
        let separator = line.iter().position(|word| word == "--").unwrap();
        assert!(separator < env, "systemd-run's own options end before env");
        assert!(line.contains(&"Delegate=yes".to_owned()));
        assert!(line.contains(&"enter-core".to_owned()));
        assert_eq!(line.last().unwrap(), "/nix/store/aaaa-sway/bin/sway");
        // A client gets no bus and stays in its scope's own group.
        let client = Launch {
            compositor: false,
            ..launch
        };
        let line = command_line(&client, &shell(), Path::new("/bench"), Path::new("/x"));
        assert!(!line.contains(&"enter-core".to_owned()));
        assert!(!line.iter().any(|word| word.contains("dbus-run-session")));
        // A compositor inside a parent is started by `parent`, which reads its launch's file.
        let inside = Launch {
            compositor: true,
            parent: Some(Parent {
                command: vec!["/nix/store/aaaa-dwl/bin/dwl".to_owned()],
                variables: vec![("WLR_BACKENDS".to_owned(), "headless".to_owned())],
                runtime: PathBuf::from("/tmp/run/runtime/parent"),
                log: PathBuf::from("/tmp/run/parent.log"),
            }),
            ..client
        };
        let line = command_line(&inside, &shell(), Path::new("/bench"), Path::new("/x"));
        assert!(!line.contains(&"enter-core".to_owned()));
        assert!(line.iter().any(|word| word.contains("dbus-run-session")));
        assert_eq!(
            line[line.len() - 3..],
            ["/bench", "parent", "/tmp/run/parent-launch.json"]
        );
    }

    #[test]
    fn the_links_a_killed_bench_left_are_swept_and_no_others() {
        let root = crate::testing::folder("links");
        let (proc_root, links) = (root.join("proc"), root.join("tmp"));
        fs::create_dir_all(proc_root.join("77")).unwrap();
        fs::write(proc_root.join("77/comm"), "yuna-bench\n").unwrap();
        fs::create_dir_all(&links).unwrap();
        // 77's bench runs; 88's is gone; a folder of that name and another file are not links.
        std::os::unix::fs::symlink(&root, links.join("yb-77-0")).unwrap();
        std::os::unix::fs::symlink(&root, links.join("yb-88-3")).unwrap();
        fs::create_dir_all(links.join("yb-99-1")).unwrap();
        fs::write(links.join("yb-notes"), "").unwrap();
        let swept = sweep_runtime_links(&Proc::at(&proc_root), &links);
        assert_eq!(swept, ["yb-88-3"]);
        assert!(links.join("yb-77-0").exists());
        assert!(links.join("yb-99-1").exists());
    }

    #[test]
    fn a_parents_launch_is_read_back_as_written() {
        let launch = ParentLaunch {
            parent: Parent {
                command: vec!["/nix/store/aaaa-dwl/bin/dwl".to_owned()],
                variables: vec![("WLR_BACKENDS".to_owned(), "headless".to_owned())],
                runtime: PathBuf::from("/tmp/run/runtime/parent"),
                log: PathBuf::from("/tmp/run/parent.log"),
            },
            compositor: vec!["/nix/store/aaaa-hyprland/bin/start-hyprland".to_owned()],
        };
        let text = serde_json::to_string(&launch).unwrap();
        assert_eq!(serde_json::from_str::<ParentLaunch>(&text).unwrap(), launch);
        // Its Wayland socket is found by its name, its lock left out.
        let runtime = crate::testing::folder("parent-socket");
        assert_eq!(wayland_socket_in(&runtime), None);
        fs::write(runtime.join("wayland-0.lock"), "").unwrap();
        fs::write(runtime.join("wayland-0"), "").unwrap();
        assert_eq!(wayland_socket_in(&runtime), Some(runtime.join("wayland-0")));
        assert!(run_parent(&runtime.join("missing.json")).is_err());
    }

    #[test]
    fn the_owners_runtime_folder_is_named_in_paths_and_addresses() {
        let owner = owner();
        assert!(owner.names_runtime_folder("/run/user/1000/wayland-1"));
        assert!(owner.names_runtime_folder("unix:path=/run/user/1000/bus"));
        assert!(owner.names_runtime_folder("/nix/store/x:/run/user/1000"));
        assert!(!owner.names_runtime_folder("/run/user/10000/bus"));
        assert!(!owner.names_runtime_folder("/tmp/yuna-bench-1/sway-1/runtime/wayland-1"));
    }

    #[test]
    fn a_card_node_an_input_device_or_an_owners_socket_is_refused() {
        // A fake scope: 10 holds the render node, a card node and three input devices; 11 is
        // connected to the owner's Wayland socket and was started with the owner's bus; 12 to
        // the sandbox's own Wayland socket; 13 to an X11 display served outside the sandbox; 14
        // serves an X11 display, which 15, inside, is connected to.
        let root = crate::testing::folder("sandbox-check");
        let (proc_root, cgroups) = (root.join("proc"), root.join("cgroup"));
        let scope = Cgroup::named(&cgroups, "/app.slice/bench.scope");
        fs::create_dir_all(scope.folder()).unwrap();
        fs::write(
            scope.folder().join("cgroup.procs"),
            "10\n11\n12\n13\n14\n15\n",
        )
        .unwrap();
        for pid in ["10", "11", "12", "13", "14", "15"] {
            fs::create_dir_all(proc_root.join(pid).join("fd")).unwrap();
            fs::write(proc_root.join(pid).join("comm"), "compositor\n").unwrap();
        }
        let link = |pid: &str, fd: &str, target: &str| {
            std::os::unix::fs::symlink(target, proc_root.join(pid).join("fd").join(fd)).unwrap();
        };
        link("10", "3", "/dev/dri/renderD128");
        link("10", "4", "/dev/dri/card1");
        link("10", "5", "/dev/input/event3");
        link("10", "6", "/dev/uinput");
        link("10", "7", "/dev/hidraw0");
        link("11", "5", "socket:[500]");
        link("12", "5", "socket:[600]");
        link("13", "5", "socket:[700]");
        link("14", "5", "socket:[800]");
        link("14", "6", "socket:[801]");
        link("15", "5", "socket:[802]");
        fs::write(
            proc_root.join("11/environ"),
            b"HOME=/tmp/run/home\0DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus\0",
        )
        .unwrap();
        fs::write(
            proc_root.join("12/environ"),
            b"XDG_RUNTIME_DIR=/tmp/run/runtime\0",
        )
        .unwrap();
        let mut sockets = SocketTable::default();
        sockets
            .paths
            .insert(501, "/run/user/1000/wayland-1".to_owned());
        sockets.peers.insert(500, 501);
        sockets
            .paths
            .insert(601, "/tmp/run/runtime/wayland-1".to_owned());
        sockets.peers.insert(600, 601);
        // The owner's display, served outside: 701 is no process's of the scope.
        sockets.paths.insert(701, "@/tmp/.X11-unix/X0".to_owned());
        sockets.peers.insert(700, 701);
        // The sandbox's own display: 800 listens, 801 accepted 802's connection.
        sockets.paths.insert(800, "@/tmp/.X11-unix/X1".to_owned());
        sockets.paths.insert(801, "@/tmp/.X11-unix/X1".to_owned());
        sockets.peers.insert(801, 802);
        sockets.peers.insert(802, 801);
        let found = check(&Proc::at(&proc_root), &[&scope], &owner(), &sockets);
        assert_eq!(found.len(), 7, "{found:#?}");
        for (line, what) in found.iter().zip([
            "/dev/dri/card1",
            "/dev/input/event3",
            "/dev/uinput",
            "/dev/hidraw0",
            "/run/user/1000/wayland-1",
            "DBUS_SESSION_BUS_ADDRESS",
            "@/tmp/.X11-unix/X0",
        ]) {
            assert!(line.contains(what), "{line} is not about {what}");
        }
        // Without them, the scope holds.
        for file in [
            "10/fd/4", "10/fd/5", "10/fd/6", "10/fd/7", "11/fd/5", "13/fd/5",
        ] {
            fs::remove_file(proc_root.join(file)).unwrap();
        }
        fs::write(proc_root.join("11/environ"), b"HOME=/tmp/run/home\0").unwrap();
        assert!(check(&Proc::at(&proc_root), &[&scope], &owner(), &sockets).is_empty());
    }

    /// Set in the process that runs the close-on-exec test alone.
    const ALONE: &str = "YUNA_BENCH_TEST_ALONE";

    #[test]
    fn a_child_holds_no_inherited_descriptor_above_2() {
        // `close_inherited_on_exec` holds only in a process whose threads open no descriptor
        // meanwhile, as the bench's own start: this test runs again alone in a process of its
        // own, one test on one thread, not beside the others.
        if std::env::var_os(ALONE).is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "sandbox::tests::a_child_holds_no_inherited_descriptor_above_2",
                    "--test-threads=1",
                ])
                .env(ALONE, "1")
                .status()
                .unwrap();
            assert!(status.success(), "the test alone failed: {status}");
            return;
        }
        // A descriptor opened without close-on-exec, as run.sh's lock is inherited.
        let folder = crate::testing::folder("inherited");
        let path = folder.join("lock");
        fs::write(&path, "").unwrap();
        let fd =
            rustix::fs::open(&path, rustix::fs::OFlags::RDONLY, rustix::fs::Mode::empty()).unwrap();
        let number = std::os::fd::AsRawFd::as_raw_fd(&fd);
        let child_holds = || {
            Command::new("sh")
                .arg("-c")
                .arg(format!("[ -e /proc/$$/fd/{number} ]"))
                .status()
                .unwrap()
                .success()
        };
        assert!(
            child_holds(),
            "the child should inherit descriptor {number} before the fix"
        );
        assert!(close_inherited_on_exec().contains(&number));
        assert!(
            !child_holds(),
            "the child still inherits descriptor {number}"
        );
        assert!(inheritable("pos:\t0\nflags:\t0100002\n"));
        assert!(!inheritable("pos:\t0\nflags:\t02100002\n"));
        assert!(!inheritable("pos:\t0\n"));
    }
}
