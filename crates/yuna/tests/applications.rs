//! Real applications in `yuna --headless`, and Yuna's rest checked on the program, as agents run
//! it, not in the test's process ([ADR-0025](../../../docs/adr/0025-applications-in-ci.md)).
//!
//! Each application runs in a `yuna` of its own, all at once. Yuna logs at `debug`: the line
//! `window opened` when a window maps, `frame drawn` for each frame, and `counters` after each
//! round of its loop that woke. Rest is read from those lines and from `/proc`: once the
//! application has shown its window and gone quiet, 10 s with no context switch in any of Yuna's
//! threads, and no frame, frame callback, configure, commit or wake-up.
//!
//! Applications that animate without end, `vkcube` and SlimeEngine's `window` example, draw a
//! number of frames and leave; Yuna must then rest. Their rest with their window open waits for
//! M1.5's input ([ADR-0029](../../../docs/adr/0029-the-rest-of-animated-applications.md)).
//! `vkcube` left to run proves that a client that keeps drawing fails the check.
//!
//! What makes an application draw again is pinned, so that only Yuna's behaviour decides the
//! result. Yuna and each application get an environment of their own, nothing of the caller's:
//! no `DISPLAY` and no D-Bus, so that they reach neither the user's desktop nor an accessibility
//! bus; a `PATH` of the store alone; throwaway home and XDG folders, the system's XDG folders
//! included, so that no settings of the machine or the user are read; DejaVu alone, through a
//! fontconfig file that includes none of the machine's; Mesa on the CPU, lavapipe alone for
//! Vulkan, without the machine's implicit layers; cursors that never blink; GTK's animations off.
//! What still reads the machine, measured with strace on NixOS: Mesa reads `/etc/drirc` where a
//! machine has one, and lists `/dev/dri` without opening a device; Vulkan's loader reads the
//! machine's list of layers and loads none; the fontconfig of Alacritty and Qt looks for two
//! NixOS files in `/etc/fonts`, `local.conf` and `70-no-bitmaps-except-emoji.conf`.
//!
//! Every program comes from the development shell (`flake.nix`). Each process the test starts is
//! killed if the test's thread ends first, so that a test killed or out of time leaves nothing.

#[path = "common/application_environment.rs"]
mod application_environment;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, DirBuilder, File};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use application_environment::{application_environment, shell_variable};
use smithay::reexports::rustix::io::Errno;
use smithay::reexports::rustix::process::{
    Pid, Signal, getppid, kill_process, set_parent_process_death_signal,
};

/// How long Yuna must do nothing: the rest window of the bar (ROADMAP's quality bar).
const REST: Duration = Duration::from_secs(10);
/// How long Yuna must go without a round before its application counts as quiet.
const QUIET: Duration = Duration::from_secs(2);
/// The longest an application may take to show its window, to go quiet or to leave: CI's runners
/// are slow, and a debug build of Yuna draws on the CPU.
const PATIENCE: Duration = Duration::from_secs(60);
/// How often the test reads Yuna's log and an application's state; it never wakes Yuna.
const POLL: Duration = Duration::from_millis(50);
/// The frames that `vkcube` and SlimeEngine's example draw before they leave.
const FRAMES: u32 = 120;

/// Each application in its `yuna`; the animated ones draw [`FRAMES`] frames and leave. One test
/// for all, run at once, so that their 10 s of rest are waited for once.
#[test]
fn applications_run_and_yuna_rests() {
    let qml_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/applications/still.qml");
    let frames = FRAMES.to_string();
    let results = std::thread::scope(|scope| {
        let runs = [
            scope.spawn(|| {
                run_and_rest("Alacritty", |yuna| {
                    let mut command = application(yuna, "alacritty");
                    command.args([
                        "--option",
                        r#"cursor.style.blinking="Never""#,
                        "--command",
                        "sleep",
                        "3600",
                    ]);
                    command
                })
            }),
            scope.spawn(|| run_and_rest("gtk4-demo", gtk4_demo)),
            scope.spawn(|| {
                run_and_rest("qml", |yuna| {
                    let mut command = qml(yuna);
                    command.arg(&qml_file);
                    command
                })
            }),
            scope.spawn(|| {
                draw_leave_and_rest("vkcube", |yuna| {
                    let mut command = application(yuna, "vkcube");
                    command.args(["--wsi", "wayland", "--c", &frames]);
                    command
                })
            }),
        ];
        runs.map(|run| run.join().expect("a run does not panic"))
    });
    let mut failed = false;
    for result in results {
        match result {
            Ok(report) => eprintln!("{report}"),
            Err(failure) => {
                eprintln!("FAILED {failure}");
                failed = true;
            }
        }
    }
    assert!(!failed, "an application failed; see above");
}

/// `vkcube` left to draw: the same check fails, both waiting for quiet and over the rest window,
/// and says what Yuna did.
#[test]
fn a_client_that_keeps_drawing_fails_the_rest_check() {
    let yuna = Yuna::start();
    let mut command = application(&yuna, "vkcube");
    command.args(["--wsi", "wayland"]);
    let mut cube = Application::start("vkcube", &yuna, command);
    yuna.wait_for_window(&mut cube)
        .unwrap_or_else(|failure| panic!("{failure}"));

    let never_quiet = yuna.wait_until_quiet(Duration::from_secs(5));
    assert!(never_quiet.is_err(), "vkcube went quiet: {never_quiet:?}");
    let activity = yuna
        .measure_rest()
        .expect_err("10 s of rest while vkcube draws");
    eprintln!("vkcube, left to draw, fails the rest check: {activity}");
    let done = activity.counters;
    assert!(done.frames_drawn > 0, "{activity}");
    assert!(done.frame_callbacks > 0, "{activity}");
    assert!(done.commits > 0, "{activity}");
    assert!(done.wakeups > 0, "{activity}");
    assert!(activity.context_switches > 0, "{activity}");
    drop(cube);
    yuna.stop().unwrap_or_else(|failure| panic!("{failure}"));
}

/// SlimeEngine's `window` example, which draws through wgpu and Vulkan: its binary, built at the
/// revision `.github/workflows/slimeengine.yml` pins, is named by `YUNA_SLIMEENGINE_WINDOW`. That
/// weekly job runs it: `cargo test --test applications -- --ignored`.
#[test]
#[ignore = "needs SlimeEngine's window example, which the weekly job builds"]
fn slimeengine_window_runs_and_yuna_rests_after_it() {
    let window = std::env::var_os("YUNA_SLIMEENGINE_WINDOW")
        .expect("YUNA_SLIMEENGINE_WINDOW names SlimeEngine's window example");
    let frames = FRAMES.to_string();
    let report = draw_leave_and_rest("SlimeEngine's window", |yuna| {
        let mut command = application(yuna, &window);
        // winit and wgpu load these libraries at run time, beside the shell's EGL.
        let libraries = format!(
            "{}:{}",
            shell_variable("YUNA_CLIENT_LIBRARY_PATH"),
            shell_variable("LD_LIBRARY_PATH")
        );
        command
            .env("LD_LIBRARY_PATH", libraries)
            .args(["--frames", &frames]);
        command
    })
    .unwrap_or_else(|failure| panic!("{failure}"));
    eprintln!("{report}");
}

/// Runs an application that draws once and goes quiet: it shows its window, goes quiet, and Yuna
/// rests. Says what it measured, or why it failed.
fn run_and_rest(name: &str, command: impl FnOnce(&Yuna) -> Command) -> Result<String, String> {
    let started = Instant::now();
    let yuna = Yuna::start();
    let mut app = Application::start(name, &yuna, command(&yuna));
    let app_log = app.log.clone();
    let failure = |what: String| format!("{name}: {what}\n{}", yuna.diagnosis(&app_log));
    yuna.wait_for_window(&mut app).map_err(failure)?;
    let shown = started.elapsed();
    let quiet = yuna.wait_until_quiet(PATIENCE).map_err(failure)?;
    let rest = yuna
        .measure_rest()
        .map_err(|activity| failure(format!("Yuna did not rest: {activity}")))?;
    if let Some(status) = app.exited() {
        return Err(failure(format!("it left during the rest window: {status}")));
    }
    drop(app);
    yuna.stop()?;
    Ok(format!(
        "{name}: window shown after {:.1} s; quiet after {} frames drawn, {} frame callbacks, \
         {} configures, {} commits; then {rest}",
        shown.as_secs_f64(),
        quiet.frames_drawn,
        quiet.frame_callbacks,
        quiet.configures,
        quiet.commits,
    ))
}

/// Runs an application that animates until it has drawn [`FRAMES`] frames, then leaves: it shows
/// its window, Yuna draws its frames, it ends well, and Yuna rests.
fn draw_leave_and_rest(
    name: &str,
    command: impl FnOnce(&Yuna) -> Command,
) -> Result<String, String> {
    let yuna = Yuna::start();
    let mut app = Application::start(name, &yuna, command(&yuna));
    let app_log = app.log.clone();
    let failure = |what: String| format!("{name}: {what}\n{}", yuna.diagnosis(&app_log));
    yuna.wait_for_window(&mut app).map_err(failure)?;
    let status = app.wait(PATIENCE).map_err(failure)?;
    if !status.success() {
        return Err(failure(format!("it ended with {status}")));
    }
    let left = yuna.wait_until_quiet(PATIENCE).map_err(failure)?;
    // One commit per frame it drew; Yuna draws at most one frame per refresh, 60 a second by
    // default, and drew one for each in every run measured: half of them leaves room for a slow
    // runner.
    if left.commits < u64::from(FRAMES) || left.frames_drawn < u64::from(FRAMES / 2) {
        return Err(failure(format!(
            "Yuna got {} commits and drew {} frames, for {FRAMES} frames of its client",
            left.commits, left.frames_drawn
        )));
    }
    let rest = yuna
        .measure_rest()
        .map_err(|activity| failure(format!("Yuna did not rest after it left: {activity}")))?;
    yuna.stop()?;
    Ok(format!(
        "{name}: {} commits, {} frames drawn, {} frame callbacks; it left, then {rest}",
        left.commits, left.frames_drawn, left.frame_callbacks
    ))
}

/// GTK 4's demo, on its Vulkan renderer, GTK 4.24's default on Wayland, pinned so that a new GTK
/// does not change it unseen; its cursor does not blink and nothing animates.
fn gtk4_demo(yuna: &Yuna) -> Command {
    let settings = yuna.home().join("config/gtk-4.0");
    fs::create_dir_all(&settings).expect("GTK's settings folder");
    fs::write(
        settings.join("settings.ini"),
        "[Settings]\ngtk-cursor-blink=false\ngtk-enable-animations=false\n",
    )
    .expect("GTK's settings");
    let mut command = application(yuna, "gtk4-demo");
    command
        .env("GDK_BACKEND", "wayland")
        .env("GSK_RENDERER", "vulkan")
        // No accessibility bus is reachable anyway; this keeps GTK from looking for one.
        .env("GTK_A11Y", "none");
    command
}

/// Qt's QML runner on its Wayland platform and its OpenGL renderer, the default pinned.
fn qml(yuna: &Yuna) -> Command {
    let mut command = application(yuna, "qml");
    command
        .env("QT_QPA_PLATFORM", "wayland")
        .env("QT_PLUGIN_PATH", shell_variable("YUNA_QT_PLUGINS"))
        .env("QML_IMPORT_PATH", shell_variable("YUNA_QML_IMPORTS"))
        .env("QSG_RHI_BACKEND", "opengl");
    command
}

/// `program`, to run in `yuna`, with an environment of its own: only what this file gives it.
fn application(yuna: &Yuna, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .envs(application_environment(&yuna.home()))
        .env("XDG_RUNTIME_DIR", yuna.runtime_dir())
        // A path, so that nothing else in the runtime folder is ever reached.
        .env("WAYLAND_DISPLAY", &yuna.socket)
        .stdin(Stdio::null());
    dies_with_test(&mut command);
    command
}

/// Makes `command`'s process end with SIGKILL when the thread that starts it ends, so that a test
/// killed, or stopped at CI's time limit, leaves no Yuna or application running. Every run drops
/// its processes before its thread ends.
fn dies_with_test(command: &mut Command) {
    let parent = std::process::id();
    // SAFETY: the closure runs in the child between fork and exec, where only async-signal-safe
    // calls are sound: it makes two system calls, `prctl` and `getppid`, and allocates nothing,
    // an `Errno` turning into an `io::Error` without a box.
    unsafe {
        command.pre_exec(move || {
            set_parent_process_death_signal(Some(Signal::KILL))?;
            // The parent may have ended before the signal was set: then it never comes.
            if u32::try_from(Pid::as_raw(getppid())) != Ok(parent) {
                return Err(Errno::SRCH.into());
            }
            Ok(())
        });
    }
}

/// What Yuna counted, from its last `counters` line (`Compositor::counters`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counters {
    frames_drawn: u64,
    frame_callbacks: u64,
    configures: u64,
    commits: u64,
    wakeups: u64,
}

impl Counters {
    /// Reads `frames_drawn=3 frame_callbacks=5 configures=2 commits=6 wakeups=40` after the
    /// message `counters`: all five, nothing else.
    fn parse(line: &str) -> Option<Counters> {
        let fields: Vec<(&str, u64)> = line
            .split_once(" counters ")?
            .1
            .split_whitespace()
            .map(|field| {
                let (key, value) = field.split_once('=')?;
                Some((key, value.parse().ok()?))
            })
            .collect::<Option<_>>()?;
        match fields.as_slice() {
            [
                ("frames_drawn", frames_drawn),
                ("frame_callbacks", frame_callbacks),
                ("configures", configures),
                ("commits", commits),
                ("wakeups", wakeups),
            ] => Some(Counters {
                frames_drawn: *frames_drawn,
                frame_callbacks: *frame_callbacks,
                configures: *configures,
                commits: *commits,
                wakeups: *wakeups,
            }),
            _ => None,
        }
    }

    /// What was counted since `before`. Yuna's counters only grow: a smaller value is a reading
    /// gone wrong, never Yuna at rest.
    fn since(self, before: Counters) -> Counters {
        let less = |after: u64, before: u64| {
            after
                .checked_sub(before)
                .unwrap_or_else(|| panic!("a counter of Yuna fell from {before} to {after}"))
        };
        Counters {
            frames_drawn: less(self.frames_drawn, before.frames_drawn),
            frame_callbacks: less(self.frame_callbacks, before.frame_callbacks),
            configures: less(self.configures, before.configures),
            commits: less(self.commits, before.commits),
            wakeups: less(self.wakeups, before.wakeups),
        }
    }
}

/// What Yuna did during a rest window that should have had nothing.
#[derive(Debug)]
struct Activity {
    counters: Counters,
    context_switches: u64,
    /// Each thread that switched, or that came or went: its id, and its switches or what
    /// happened to it.
    threads: Vec<String>,
}

impl fmt::Display for Activity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.counters;
        write!(
            f,
            "in {} s, {} frames drawn, {} frame callbacks, {} configures, {} commits, {} \
             wake-ups; {} context switches ({})",
            REST.as_secs(),
            c.frames_drawn,
            c.frame_callbacks,
            c.configures,
            c.commits,
            c.wakeups,
            self.context_switches,
            self.threads.join(", ")
        )
    }
}

/// A `yuna --headless` process with one output, in a folder of its own: its runtime folder, its
/// log, and its application's home. The output is 1920 × 1080 at 60 Hz and scale 1, or the mode
/// `YUNA_APPLICATIONS_OUTPUT` names, as `--output` takes it, to measure rest on another output.
struct Yuna {
    child: Child,
    folder: PathBuf,
    log: PathBuf,
    /// Its Wayland socket's path.
    socket: PathBuf,
    /// Whether a failure was reported: the folder, with its logs, is then kept.
    failed: Cell<bool>,
}

impl Yuna {
    fn start() -> Yuna {
        static RUNS: AtomicU32 = AtomicU32::new(0);
        let folder = std::env::temp_dir().join(format!(
            "yuna-applications-{}-{}",
            std::process::id(),
            RUNS.fetch_add(1, Ordering::Relaxed)
        ));
        // Wayland asks that the runtime folder be the user's alone.
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder.join("runtime"))
            .expect("a runtime folder");
        let log = folder.join("yuna.log");
        let output = std::env::var("YUNA_APPLICATIONS_OUTPUT")
            .unwrap_or_else(|_| "1920x1080@60,1".to_owned());
        let mut command = Command::new(env!("CARGO_BIN_EXE_yuna"));
        command
            .args(["--headless", "--output", &output])
            // What it draws with, from the shell; a throwaway home, so that Mesa reads none of
            // the user's settings.
            .env_clear()
            .env("LD_LIBRARY_PATH", shell_variable("LD_LIBRARY_PATH"))
            .env(
                "__EGL_VENDOR_LIBRARY_FILENAMES",
                shell_variable("__EGL_VENDOR_LIBRARY_FILENAMES"),
            )
            .env("XKB_CONFIG_ROOT", shell_variable("XKB_CONFIG_ROOT"))
            .env("HOME", folder.join("yuna-home"))
            .env("XDG_RUNTIME_DIR", folder.join("runtime"))
            .env("YUNA_LOG", "debug")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(File::create(&log).expect("Yuna's log"));
        dies_with_test(&mut command);
        let child = command.spawn().expect("the yuna binary runs");
        let mut yuna = Yuna {
            child,
            folder,
            log,
            socket: PathBuf::new(),
            failed: Cell::new(false),
        };
        // Read on a thread of its own, so that a Yuna that never prints fails the test in time.
        let stdout = yuna.child.stdout.take().expect("its standard output");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = sender.send(line);
        });
        let line = receiver.recv_timeout(PATIENCE).unwrap_or_default();
        let Some(name) = line.trim_end().strip_prefix("WAYLAND_DISPLAY=") else {
            panic!("yuna printed no socket: {line:?}\n{}", yuna.log_tail());
        };
        yuna.socket = yuna.runtime_dir().join(name);
        yuna
    }

    fn runtime_dir(&self) -> PathBuf {
        self.folder.join("runtime")
    }

    fn home(&self) -> PathBuf {
        self.folder.join("home")
    }

    fn read_log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Its last `counters` line, or zeros before its first round. A last line without its end
    /// may be half written, and is left out.
    fn counters(&self) -> Counters {
        let log = self.read_log();
        let complete = log.rfind('\n').map_or("", |end| &log[..end]);
        complete
            .lines()
            .rev()
            .find_map(Counters::parse)
            .unwrap_or_default()
    }

    /// Waits until a window has mapped and a frame was drawn after it.
    fn wait_for_window(&self, app: &mut Application) -> Result<(), String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let log = self.read_log();
            if let Some(at) = log.find("window opened")
                && log[at..].contains("frame drawn")
            {
                return Ok(());
            }
            if let Some(status) = app.exited() {
                return Err(format!("it ended before showing a window: {status}"));
            }
            if Instant::now() > deadline {
                return Err(format!("no window shown in {} s", PATIENCE.as_secs()));
            }
            std::thread::sleep(POLL);
        }
    }

    /// Waits until Yuna has had no round for [`QUIET`], at most `patience`, and gives its
    /// counters then.
    fn wait_until_quiet(&self, patience: Duration) -> Result<Counters, String> {
        let deadline = Instant::now() + patience;
        let mut last = self.counters();
        let mut since = Instant::now();
        loop {
            std::thread::sleep(POLL);
            let now = self.counters();
            if now != last {
                last = now;
                since = Instant::now();
            } else if since.elapsed() >= QUIET {
                return Ok(now);
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "never quiet for {} s in {} s: {last:?}",
                    QUIET.as_secs(),
                    patience.as_secs()
                ));
            }
        }
    }

    /// Requires [`REST`] with no context switch in any of Yuna's threads and no round: no frame,
    /// frame callback, configure, commit or wake-up. Says what it measured.
    fn measure_rest(&self) -> Result<String, Activity> {
        let threads_before = self.context_switches();
        let counters_before = self.counters();
        std::thread::sleep(REST);
        let threads_after = self.context_switches();
        let counters = self.counters().since(counters_before);

        let mut context_switches = 0;
        let mut threads = Vec::new();
        for (thread, after) in &threads_after {
            match threads_before.get(thread) {
                Some(before) if before == after => {}
                Some(before) => {
                    context_switches += after - before;
                    threads.push(format!("thread {thread}: {}", after - before));
                }
                None => {
                    context_switches += after;
                    threads.push(format!("thread {thread} started"));
                }
            }
        }
        for thread in threads_before.keys() {
            if !threads_after.contains_key(thread) {
                threads.push(format!("thread {thread} ended"));
            }
        }
        if counters == Counters::default() && threads.is_empty() {
            Ok(format!(
                "{} s of rest: 0 context switches in its {} threads, 0 frames, frame \
                 callbacks, configures, commits and wake-ups",
                REST.as_secs(),
                threads_after.len()
            ))
        } else {
            Err(Activity {
                counters,
                context_switches,
                threads,
            })
        }
    }

    /// Each of Yuna's threads, and its context switches so far, voluntary and not.
    fn context_switches(&self) -> BTreeMap<u32, u64> {
        let tasks = PathBuf::from(format!("/proc/{}/task", self.child.id()));
        let mut threads = BTreeMap::new();
        for task in fs::read_dir(tasks).expect("Yuna's threads").flatten() {
            let Some(thread) = task.file_name().to_str().and_then(|id| id.parse().ok()) else {
                continue;
            };
            // A thread that ended between the listing and the read is left out.
            let Ok(status) = fs::read_to_string(task.path().join("status")) else {
                continue;
            };
            let switches = status
                .lines()
                .filter(|line| line.contains("ctxt_switches:"))
                .filter_map(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
                .sum();
            threads.insert(thread, switches);
        }
        threads
    }

    /// Stops Yuna with SIGTERM, as an agent does, and requires that it ends well.
    fn stop(mut self) -> Result<(), String> {
        let _ = kill_process(Pid::from_child(&self.child), Signal::TERM);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => {
                    self.failed.set(true);
                    return Err(format!("yuna ended with {status}\n{}", self.log_tail()));
                }
                Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
                _ => {
                    self.failed.set(true);
                    return Err(format!("yuna did not stop on SIGTERM\n{}", self.log_tail()));
                }
            }
        }
    }

    /// The end of Yuna's log, its warnings and errors first, for a failure's message.
    fn log_tail(&self) -> String {
        let log = self.read_log();
        let warnings: Vec<&str> = log
            .lines()
            .filter(|line| line.contains(" WARN ") || line.contains(" ERROR "))
            .take(10)
            .collect();
        let lines: Vec<&str> = log.lines().collect();
        let tail = &lines[lines.len().saturating_sub(10)..];
        format!(
            "yuna's warnings:\n{}\nyuna's last lines ({}):\n{}",
            warnings.join("\n"),
            self.log.display(),
            tail.join("\n")
        )
    }

    /// What a failure shows: the ends of Yuna's log and of its application's, `app_log`. The
    /// folder that holds both is then kept.
    fn diagnosis(&self, app_log: &Path) -> String {
        self.failed.set(true);
        let log = fs::read_to_string(app_log).unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        format!(
            "{}\nthe application's last lines ({}):\n{}",
            self.log_tail(),
            app_log.display(),
            lines[lines.len().saturating_sub(15)..].join("\n")
        )
    }
}

impl Drop for Yuna {
    /// Kills Yuna if it still runs. Its folder is kept when the run failed, for its logs.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.failed.get() || std::thread::panicking() {
            eprintln!("Yuna's folder is kept: {}", self.folder.display());
        } else {
            let _ = fs::remove_dir_all(&self.folder);
        }
    }
}

/// An application started in a [`Yuna`], its output in a log beside Yuna's; killed when dropped.
struct Application {
    child: Child,
    log: PathBuf,
}

impl Application {
    fn start(name: &str, yuna: &Yuna, mut command: Command) -> Application {
        let log = yuna.folder.join(format!(
            "{}.log",
            name.replace(|c: char| !c.is_alphanumeric(), "-")
        ));
        let output = File::create(&log).expect("the application's log");
        let child = command
            .stdout(output.try_clone().expect("the log, twice"))
            .stderr(output)
            .spawn()
            .unwrap_or_else(|error| panic!("{name} does not start: {error}"));
        Application { child, log }
    }

    /// Its exit status, if it has ended.
    fn exited(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Waits until it ends, at most `patience`.
    fn wait(&mut self, patience: Duration) -> Result<ExitStatus, String> {
        let deadline = Instant::now() + patience;
        loop {
            if let Some(status) = self.exited() {
                return Ok(status);
            }
            if Instant::now() > deadline {
                return Err(format!("it did not end in {} s", patience.as_secs()));
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for Application {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
