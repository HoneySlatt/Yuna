//! The owner's other projects drive a program in `yuna --headless --agent-seat` as in a headless
//! sway, their own code unchanged, at the revisions `flake.nix` pins
//! ([ADR-0089](../../../docs/adr/0089-yuna-for-the-owners-other-projects.md),
//! [ADR-0079](../../../docs/adr/0079-yunas-contracts-in-0-x-changes-announced-never-silent.md)):
//!
//! - Pluto's `pluto-input`, its bench's virtual keyboard and pointer, built from Pluto's own
//!   lock, clicks and types into a window, which gets each event;
//! - uwui's `scripts/measure.py --display`, its measure of an application from outside, runs its
//!   steps to the end against Yuna: `scripts/wayland.py`'s virtual pointer is made and moves and
//!   clicks without an error, and `vkcube`'s frames are counted through `wlr-screencopy`'s
//!   copies with damage. That its clicks land is shown by Pluto's test and by the stories of
//!   `tests/headless/virtual_input.rs`, which take the same path.
//!
//! The window that records what it gets is a client of this test, on a thread of its own. Two
//! things stand between `measure.py` and Yuna, neither in its measure: it refuses to start while
//! a game seems to run, for the GPU's figures, which a functional test does not take, so the
//! test loads it with that check answering none; and it checks that `sway` and `swaymsg` are on
//! the `PATH`, which `--display` never runs, so the test puts two that fail if run.

#![allow(
    clippy::disallowed_methods,
    reason = "a test's threads are not Yuna's tasks (ADR-0152)"
)]

#[path = "common/application_environment.rs"]
mod application_environment;
#[path = "common/dies_with_test.rs"]
mod dies_with_test;
#[path = "common/folders.rs"]
mod folders;

use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use application_environment::{application_environment, shell_variable};
use dies_with_test::dies_with_test;
use folders::fresh_folder;
use rustix::fs::{MemfdFlags, memfd_create};
use serde_json::Value;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

/// The longest a step may take: a debug build of Yuna starts llvmpipe, and CI's runners are slow.
const PATIENCE: Duration = Duration::from_secs(60);
/// The output both drive: 1920 × 1080 at 60 Hz, scale 1. Its one window fills it, less the
/// gaps of 16: 1888 × 1048 at (16, 16).
const OUTPUT: &str = "1920x1080@60,1";
const BUTTON_LEFT: u32 = 0x110;

/// A `yuna --headless --agent-seat` of the test's own, in a runtime folder of its own.
struct Yuna {
    child: Child,
    folder: PathBuf,
    socket: String,
}

impl Yuna {
    fn start(name: &str) -> Yuna {
        let folder = fresh_folder(&format!("consumers-{name}"));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(folder.join("runtime"))
            .expect("a runtime folder");
        let mut command = Command::new(env!("CARGO_BIN_EXE_yuna"));
        command
            .args(["--headless", "--agent-seat", "--output", OUTPUT])
            .env("XDG_RUNTIME_DIR", folder.join("runtime"))
            .env("YUNA_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(File::create(folder.join("yuna.log")).expect("Yuna's log"));
        dies_with_test(&mut command);
        let mut child = command.spawn().expect("the yuna binary runs");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("its output"))
            .read_line(&mut line)
            .expect("Yuna prints its socket");
        let socket = line
            .trim_end()
            .strip_prefix("WAYLAND_DISPLAY=")
            .unwrap_or_else(|| panic!("no socket printed: {line:?}"))
            .to_owned();
        Yuna {
            child,
            folder,
            socket,
        }
    }

    fn runtime_dir(&self) -> PathBuf {
        self.folder.join("runtime")
    }

    fn log(&self) -> String {
        fs::read_to_string(self.folder.join("yuna.log")).unwrap_or_default()
    }
}

impl Drop for Yuna {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.folder);
    }
}

/// Waits until `ready` holds, or fails with `what` and Yuna's log.
fn wait_for(yuna: &Yuna, what: &str, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + PATIENCE;
    while !ready() {
        assert!(
            Instant::now() < until,
            "{what} did not come\n{}",
            yuna.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// What the test's window got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Got {
    PointerEnter,
    Button { button: u32, pressed: bool },
    Key { key: u32, pressed: bool },
}

/// The test's window: one `xdg_toplevel` of `wl_shm`, which records its seat's input.
#[derive(Default)]
struct Window {
    shm: Option<wl_shm::WlShm>,
    surface: Option<wl_surface::WlSurface>,
    size: (i32, i32),
    got: Arc<Mutex<Vec<Got>>>,
    shown: Arc<Mutex<bool>>,
}

impl Window {
    /// Opens the window on `yuna`'s socket, its events read on a thread until Yuna goes: what it
    /// gets, and whether it has shown.
    fn open(yuna: &Yuna) -> (Arc<Mutex<Vec<Got>>>, Arc<Mutex<bool>>) {
        let stream = std::os::unix::net::UnixStream::connect(yuna.runtime_dir().join(&yuna.socket))
            .expect("Yuna's socket");
        let connection = Connection::from_socket(stream).expect("a Wayland connection");
        let (globals, mut queue) = registry_queue_init::<Window>(&connection).expect("the globals");
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor =
            globals.bind(&qh, 4..=6, ()).expect("wl_compositor");
        let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).expect("xdg_wm_base");
        let _seat: wl_seat::WlSeat = globals.bind(&qh, 5..=8, ()).expect("wl_seat");
        let mut window = Window {
            shm: Some(globals.bind(&qh, 1..=1, ()).expect("wl_shm")),
            ..Window::default()
        };
        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        toplevel.set_title("yuna consumers' window".to_owned());
        surface.commit();
        window.surface = Some(surface);
        let (got, shown) = (window.got.clone(), window.shown.clone());
        std::thread::spawn(move || {
            let _keep = (wm_base, xdg_surface, toplevel, compositor);
            while queue.blocking_dispatch(&mut window).is_ok() {}
        });
        (got, shown)
    }

    fn draw(&mut self, qh: &QueueHandle<Window>) {
        let (width, height) = match self.size {
            (0, _) | (_, 0) => (640, 480),
            size => size,
        };
        let stride = width * 4;
        let fd = memfd_create("yuna-consumers-window", MemfdFlags::CLOEXEC).expect("a memfd");
        let file = File::from(fd);
        let pixels: Vec<u8> = [0x99, 0x66, 0x33, 0xFF].repeat((width * height) as usize);
        (&file).write_all(&pixels).expect("the pixels are written");
        let shm = self.shm.as_ref().expect("wl_shm");
        let pool = shm.create_pool(file.as_fd(), stride * height, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, ());
        pool.destroy();
        let surface = self.surface.as_ref().expect("its surface");
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
        *self.shown.lock().expect("the flag") = true;
    }

    fn record(&self, got: Got) {
        self.got.lock().expect("the events").push(got);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Window {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Window {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Window {
    fn event(
        window: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            window.draw(qh);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Window {
    fn event(
        window: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            window.size = (width, height);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Window {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            if capabilities.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(qh, ());
            }
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Window {
    fn event(
        window: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { .. } => window.record(Got::PointerEnter),
            wl_pointer::Event::Button { button, state, .. } => window.record(Got::Button {
                button,
                pressed: state == WEnum::Value(wl_pointer::ButtonState::Pressed),
            }),
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Window {
    fn event(
        window: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key { key, state, .. } = event {
            window.record(Got::Key {
                key,
                pressed: state == WEnum::Value(wl_keyboard::KeyState::Pressed),
            });
        }
    }
}

delegate_noop!(Window: ignore wl_compositor::WlCompositor);
delegate_noop!(Window: ignore wl_surface::WlSurface);
delegate_noop!(Window: ignore wl_shm::WlShm);
delegate_noop!(Window: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Window: ignore wl_buffer::WlBuffer);

/// A pinned consumer's sources, which the development shell names in `variable`.
fn consumer(variable: &str) -> PathBuf {
    PathBuf::from(shell_variable(variable))
}

/// What the development shell sets that changes how `pluto-input` builds: the pinned source and
/// the paths its libraries are found by. Cargo does not see a library change under a build it
/// kept, so a build kept from before a raise of `nixpkgs` would be used as it was.
const BUILD_ENVIRONMENT: [&str; 4] = [
    "YUNA_PLUTO_SOURCE",
    "PATH",
    "LD_LIBRARY_PATH",
    "PKG_CONFIG_PATH",
];

/// The folder name's key of `pluto-input`'s build: a hash of the Nix store paths the variables
/// of [`BUILD_ENVIRONMENT`] name, so that a shell with other libraries builds afresh in a folder
/// of its own. Only the store's entries: cargo adds its own target folder to `LD_LIBRARY_PATH`,
/// and a caller's `PATH` holds its own folders, which would give one shell many keys. The hash is
/// std's, enough to tell shells apart, as nothing reads it back.
fn build_key() -> String {
    let mut hasher = DefaultHasher::new();
    for name in BUILD_ENVIRONMENT {
        let Some(value) = std::env::var(name).ok() else {
            continue;
        };
        let store_paths: Vec<&str> = value
            .split(':')
            .filter(|entry| entry.starts_with("/nix/store/"))
            .collect();
        (name, store_paths).hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// Builds Pluto's `pluto-input` from its own lock, in a target folder of its own beside Yuna's,
/// named by [`build_key`]. The folders of older keys stay, as `target/` is throwaway.
fn build_pluto_input() -> PathBuf {
    let pluto = consumer("YUNA_PLUTO_SOURCE");
    let target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/consumers")
        .join(format!("pluto-{}", build_key()));
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()))
        .arg("build")
        .arg("--manifest-path")
        .arg(pluto.join("Cargo.toml"))
        .args(["--package", "pluto-input", "--locked", "--quiet"])
        .arg("--target-dir")
        .arg(&target)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "pluto-input does not build: {status}");
    target.join("debug/pluto-input")
}

/// Pluto's bench's driver, `pluto-input`, clicks in a window of an agent's seat and types into
/// it, as it does in a headless sway: the window gets the press and the release of the button,
/// then `h`, `i` and Return, pressed in that order.
#[test]
fn plutos_bench_driver_drives_a_window_in_yuna() {
    let pluto_input = build_pluto_input();
    let yuna = Yuna::start("pluto");
    let (got, shown) = Window::open(&yuna);
    wait_for(&yuna, "the window", || *shown.lock().expect("the flag"));

    let commands = yuna.folder.join("commands");
    let made = Command::new("mkfifo")
        .arg(&commands)
        .status()
        .expect("mkfifo runs");
    assert!(made.success());
    let mut command = Command::new(&pluto_input);
    command
        .arg("--commands")
        .arg(&commands)
        .args(["--size", "1920x1080"])
        .env_clear()
        .env("XDG_RUNTIME_DIR", yuna.runtime_dir())
        .env("WAYLAND_DISPLAY", &yuna.socket)
        .stderr(Stdio::piped());
    dies_with_test(&mut command);
    let mut driver = command.spawn().expect("pluto-input runs");
    let mut line = String::new();
    BufReader::new(driver.stderr.take().expect("its errors"))
        .read_line(&mut line)
        .expect("a line");
    assert_eq!(line.trim_end(), "pluto-input: ready", "{}", yuna.log());

    let done = yuna.folder.join("done");
    let mut pipe = fs::OpenOptions::new()
        .write(true)
        .open(&commands)
        .expect("the commands' pipe");
    writeln!(
        pipe,
        "move 500 400\nclick left\ntype hi\nkey Return\nmark {}",
        done.display()
    )
    .expect("the commands are sent");
    wait_for(&yuna, "pluto-input's mark", || done.exists());
    let expected = [
        Got::Button {
            button: BUTTON_LEFT,
            pressed: true,
        },
        Got::Button {
            button: BUTTON_LEFT,
            pressed: false,
        },
        Got::Key {
            key: 35,
            pressed: true,
        },
        Got::Key {
            key: 23,
            pressed: true,
        },
        Got::Key {
            key: 28,
            pressed: true,
        },
    ];
    let received = || {
        let got = got.lock().expect("the events").clone();
        let kept: Vec<Got> = got
            .into_iter()
            .filter(|got| matches!(got, Got::Button { .. } | Got::Key { pressed: true, .. }))
            .collect();
        kept
    };
    wait_for(&yuna, "the window's input", || received() == expected);
    assert!(got.lock().expect("the events").contains(&Got::PointerEnter));
    let _ = driver.kill();
    let _ = driver.wait();
}

/// Writes `name` into `folder`, an executable that fails and leaves the file `<name>.ran` there
/// if it is ever run, which `--display` must not do.
fn stub(folder: &Path, name: &str) {
    let path = folder.join(name);
    let mark = folder.join(format!("{name}.ran"));
    fs::write(
        &path,
        format!(
            "#!/bin/sh\n: > '{}'\necho \"{name} is not run by measure.py --display\" >&2\nexit 1\n",
            mark.display()
        ),
    )
    .expect("the stub is written");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("the stub runs");
}

/// uwui's `measure.py --display` measures `vkcube` in Yuna as on the session's own display:
/// `wayland.py`'s virtual pointer moves and clicks without an error, its copies with damage
/// count the frames `vkcube` draws, and it reads the window's processes and memory; `sway` and
/// `swaymsg` are never run. The figures are not kept:
/// llvmpipe and a debug build measure nothing of Yuna's.
#[test]
fn uwuis_measure_drives_and_counts_frames_in_yuna() {
    let uwui = consumer("YUNA_UWUI_SOURCE");
    let yuna = Yuna::start("uwui");
    let work = yuna.folder.join("measure");
    let stubs = work.join("stubs");
    fs::create_dir_all(&stubs).expect("the measure's folder");
    stub(&stubs, "sway");
    stub(&stubs, "swaymsg");
    let steps = work.join("steps");
    fs::write(
        &steps,
        "origin echo 1888x1048+16+16\npointer move 300 300\nsleep 0.5\npointer click\n\
         measure draw 3\nmemory end\n",
    )
    .expect("the steps are written");

    let python = which("python3");
    let path = format!(
        "{}:{}:{}",
        stubs.display(),
        python.parent().expect("python's folder").display(),
        shell_variable("YUNA_APPLICATIONS_PATH")
    );
    let load = "import importlib.util, sys\n\
                path = sys.argv[1]\n\
                spec = importlib.util.spec_from_file_location('measure', path)\n\
                measure = importlib.util.module_from_spec(spec)\n\
                spec.loader.exec_module(measure)\n\
                measure.games_running = lambda: []\n\
                sys.argv = sys.argv[1:]\n\
                measure.main()\n";
    let mut command = Command::new(&python);
    command
        .args(["-I", "-B", "-c", load])
        .arg(uwui.join("scripts/measure.py"))
        .args(["--display", "--runs", "0", "--steps"])
        .arg(&steps)
        .arg(work.join("results"))
        .args(["--", "vkcube", "--wsi", "wayland"])
        .env_clear()
        .envs(application_environment(&work.join("home")))
        .env("PATH", path)
        .env("XDG_RUNTIME_DIR", yuna.runtime_dir())
        .env("WAYLAND_DISPLAY", &yuna.socket)
        .stdout(File::create(work.join("measure.log")).expect("the measure's log"))
        .stderr(Stdio::piped());
    dies_with_test(&mut command);
    let output = command
        .spawn()
        .expect("python runs")
        .wait_with_output()
        .expect("the measure ends");
    let printed = fs::read_to_string(work.join("measure.log")).unwrap_or_default();
    assert!(
        output.status.success(),
        "measure.py failed: {}\n{printed}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
        yuna.log()
    );
    let results: Value = serde_json::from_str(
        &fs::read_to_string(work.join("results/results.json")).expect("the results"),
    )
    .expect("the results are JSON");
    let steps = &results["steps"];
    for tool in ["sway", "swaymsg"] {
        assert!(
            !stubs.join(format!("{tool}.ran")).exists(),
            "measure.py --display ran {tool}"
        );
    }
    let frames = steps["draw"]["frames"].as_u64().unwrap_or(0);
    assert!(
        frames >= 10,
        "vkcube's frames were not counted: {results}\n{printed}"
    );
    assert!(steps["memory end"].is_object(), "{results}");
    eprintln!(
        "measure.py counted {frames} frames of vkcube in 3 s: {}",
        steps["draw"]
    );
}

/// `program`'s path on the `PATH`.
fn which(program: &str) -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|folder| folder.join(program))
                .find(|candidate| candidate.is_file())
        })
        .unwrap_or_else(|| panic!("{program} is not on the PATH"))
}
