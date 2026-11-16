//! The scenarios, each the same for every recipe, and the run of one case: a compositor started
//! in its sandbox, checked, measured through the scenario's windows, and stopped.
//!
//! - `rest`: 1, 15 or 150 still windows; [`Durations::settle`] to settle, then
//!   [`Durations::rest`] measured: wake-ups a second, CPU, GPU, frames drawn; memory after.
//! - `game`: mpv fullscreen, drawing on the GPU, with Vulkan on radv, a clip the bench makes at
//!   240 frames a second as fast as the compositor takes them (`--untimed`, a FIFO swap chain):
//!   each of its frames is one the compositor shows. [`Durations::window`] measured after
//!   [`Durations::warm_up`]: CPU and GPU a frame, its frames counted by mpv in the window itself.
//! - `film`: mpv fullscreen playing a clip the bench makes, 3840 × 2160 at 24 frames a second;
//!   its dropped and delayed frames from its own counters, over its IPC. Both clips are long
//!   enough never to loop inside a window ([`Clip::seconds`]).
//! - `pacing`: `weston-presentation-shm` in its small window; its presentation log.
//! - `motion`: 5 still windows, the recipe's actions once each, each once the last settled, after
//!   a round to warm up. A compositor driven by its keys alone gets the bench's virtual keyboard
//!   (`keys`), a client of its own.
//!
//! A window's figures are its costs over its length, exact. A cost a frame is given only where
//! the window's own frames were counted exactly in it: by KWin's frame log, by Umbriel's count of
//! its commits, or by mpv in `game`. A compositor counted by `ext-image-copy-capture` is counted
//! in a second window apart, the same scenario again, so that the copies never weigh on the
//! costs; that count is a lower bound, since the counter misses frames that come faster than its
//! copies, and no cost a frame comes from it. Mutter's are counted by mpv alone. Frames counted
//! headless compare only between compositors on the same backend (ADR-0075). Each cost window
//! records the rate the compositor really drew at ([`achieved_rate`]); no ranking depends on it.
//!
//! Before any window, once its clients are up, every scenario checks the sandbox: the
//! compositor's scope and each client's (`sandbox::check`). A case that breaks its rules, or whose
//! client is not fullscreen where the scenario asks it, is *refused*, not failed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rustix::process::Signal;

use crate::cgroup::Cgroup;
use crate::clock::monotonic_ns;
use crate::drm;
use crate::logs;
use crate::machine::{DRAWING_ENGINES, MachineWatch};
use crate::procfs::{self, Proc};
use crate::recipes::{self, Action, Configuration, Frames, Output, Plan, Prepared, Ready, Recipe};
use crate::record::{
    self, Case, CaseRun, ClientUsage, CompositorInfo, Figure, Figures, Memory, MemorySum, Phase,
    ProcessMemory, RawFdinfo,
};
use crate::sampler::Watch;
use crate::sandbox::{
    self, Folders, Launch, Owner, RENDER_NODE, Sandboxed, ShellPaths, client_environment,
    compositor_environment,
};
use crate::sockets::SocketTable;

/// The scenarios, by name.
pub const SCENARIOS: [&str; 5] = ["rest", "game", "film", "pacing", "motion"];
/// The still windows of `rest`.
pub const REST_WINDOWS: [u32; 3] = [1, 15, 150];
/// The still windows of `motion`.
const MOTION_WINDOWS: u32 = 5;
/// The longest a compositor or a client may take to be ready.
const PATIENCE: Duration = Duration::from_secs(60);

/// How long each part of a scenario lasts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Durations {
    /// For still windows to settle before `rest` is measured.
    pub settle: Duration,
    /// `rest`'s window.
    pub rest: Duration,
    /// For a client to warm up before it is measured.
    pub warm_up: Duration,
    /// `game`'s, `film`'s and `pacing`'s window.
    pub window: Duration,
    /// After each of `motion`'s actions, for its effects to end.
    pub action: Duration,
    /// From the frame counter's start to its window, for the frame it asks first, which any
    /// compositor draws, to pass.
    pub counter_lead: Duration,
}

impl Durations {
    /// The scenarios' own lengths: the only ones whose figures compare.
    pub const STANDARD: Durations = Durations {
        settle: Duration::from_secs(5),
        rest: Duration::from_secs(60),
        warm_up: Duration::from_secs(3),
        window: Duration::from_secs(10),
        action: Duration::from_millis(1500),
        counter_lead: Duration::from_secs(2),
    };

    /// The standard lengths with every measured window `seconds` long, to try the bench.
    pub fn with_windows(seconds: f64) -> Durations {
        let window = Duration::from_secs_f64(seconds);
        Durations {
            rest: window,
            window,
            ..Durations::STANDARD
        }
    }
}

/// What every case shares.
pub struct Bench {
    /// The shell's store paths.
    pub shell: ShellPaths,
    /// What the sandbox must never reach.
    pub owner: Owner,
    /// `/proc`.
    pub proc: Proc,
    /// This program, which runs the still client, the frame counter and `enter-core`.
    pub program: PathBuf,
    /// The run's folder: each case's folders, logs and configuration.
    pub folder: PathBuf,
    /// What was made before the cases.
    pub prepared: Prepared,
    /// The scenarios' lengths.
    pub durations: Durations,
    /// The clips made so far, by their names.
    clips: BTreeMap<&'static str, PathBuf>,
    units: usize,
}

impl Bench {
    /// A bench over `folder`, with nothing prepared yet.
    pub fn new(
        shell: ShellPaths,
        program: PathBuf,
        folder: PathBuf,
        durations: Durations,
    ) -> Bench {
        Bench {
            shell,
            owner: Owner::of_this_process(),
            proc: Proc::system(),
            program,
            folder,
            prepared: Prepared::default(),
            durations,
            clips: BTreeMap::new(),
            units: 0,
        }
    }

    /// A name for the next transient unit, unique on the machine.
    fn unit(&mut self, what: &str) -> String {
        self.units += 1;
        format!("yuna-bench-{}-{}-{what}", std::process::id(), self.units)
    }

    /// A program found in the applications' `PATH`.
    fn application(&self, name: &str) -> Result<PathBuf, String> {
        self.shell
            .applications_path
            .split(':')
            .map(|folder| Path::new(folder).join(name))
            .find(|path| path.exists())
            .ok_or_else(|| format!("no {name} in YUNA_APPLICATIONS_PATH"))
    }
}

/// A compositor running in its sandbox.
struct Running {
    recipe: &'static Recipe,
    folders: Folders,
    sandboxed: Sandboxed,
    wayland_display: String,
    sway_socket: Option<PathBuf>,
    output: Option<String>,
    /// The short link to its runtime folder it was given, if it was: removed at its stop.
    runtime_link: Option<PathBuf>,
}

impl Running {
    fn log(&self) -> String {
        fs::read_to_string(&self.sandboxed.log).unwrap_or_default()
    }

    fn bus(&self) -> PathBuf {
        self.folders.bus()
    }
}

/// One of the bench's clients, in a scope of its own.
struct Client {
    name: String,
    sandboxed: Sandboxed,
}

impl Client {
    /// Its log; none if it cannot be read.
    fn log(&self) -> Option<String> {
        fs::read_to_string(&self.sandboxed.log).ok()
    }

    /// Stops it with `signal`, and gives it 5 s.
    fn stop(mut self, signal: Signal) {
        let _ = sandbox::signal(self.sandboxed.child.id(), signal);
        self.sandboxed.wait_or_kill(Duration::from_secs(5));
    }
}

/// When a window ran, on CLOCK_MONOTONIC: where KWin's frame log and the frame counter's lines
/// are cut.
#[derive(Debug, Clone, Copy)]
struct Marks {
    from_ns: u64,
    to_ns: u64,
}

/// A window measured, and when it ran.
struct Measured {
    phase: Phase,
    marks: Marks,
}

/// Why a case stopped before its end.
#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    /// Something went wrong: the case failed.
    Failed(String),
    /// The sandbox broke its rules, or the scene was not the scenario's: the case is refused, and
    /// nothing of it is measured. Each line says why.
    Refused(Vec<String>),
}

impl From<String> for Stop {
    fn from(error: String) -> Stop {
        Stop::Failed(error)
    }
}

/// Records in `run` how a case's step ended: a failure as its error, a refusal as its refusals.
pub fn record_outcome<T>(run: &mut CaseRun, outcome: Result<T, Stop>) -> Option<T> {
    match outcome {
        Ok(value) => Some(value),
        Err(Stop::Failed(error)) => {
            run.error = Some(error);
            None
        }
        Err(Stop::Refused(lines)) => {
            run.refused.extend(lines);
            None
        }
    }
}

/// Runs `case`, its `repeat`-th run.
pub fn run_case(bench: &mut Bench, case: &Case, repeat: u32) -> CaseRun {
    let mut run = CaseRun {
        case: case.clone(),
        repeat,
        compositor: None,
        ready_after: Figure::new(None, "ms"),
        phases: Vec::new(),
        memory: Vec::new(),
        refused: Vec::new(),
        error: None,
        stopped: None,
        disturbed: None,
        notes: Vec::new(),
    };
    let (Some(recipe), Some(configuration)) = (
        recipes::recipe(&case.compositor),
        Configuration::named(&case.configuration),
    ) else {
        run.error = Some(format!("no recipe for {}", case.label()));
        return run;
    };
    if case.scenario == "motion" && recipe.actions.is_empty() {
        run.notes.push(format!(
            "not run: {}",
            recipe.no_actions.unwrap_or("the recipe has no actions")
        ));
        return run;
    }
    let label = case.label().replace('/', "-");
    let (mut running, plan, ready_after) =
        match start_compositor(bench, recipe, configuration, &format!("{label}-{repeat}")) {
            Ok(started) => started,
            Err(error) => {
                run.error = Some(error);
                return run;
            }
        };
    run.ready_after = Figure::new(Some(ready_after.as_secs_f64() * 1000.0), "ms");
    run.compositor = Some(compositor_info(bench, &running, &plan));
    if recipe.name == "kwin" {
        run.notes.push(
            "KWin is given two desktops in both configurations, for motion's switch".to_owned(),
        );
        if bench.prepared.kwin_output_config.is_none() {
            run.notes.push(
                "KWin's output configuration could not be prepared: it ran at the 60 Hz its \
                 virtual output starts at"
                    .to_owned(),
            );
        }
    }
    if !SocketTable::read(&bench.proc.unix_sockets()).peers_known {
        run.notes.push(
            "the kernel's socket diagnostics could not be read: sockets bound in the owner's \
             runtime folder were checked, not those connected to one"
                .to_owned(),
        );
    }
    let checked = refuse_if_broken(bench, &running, &[]);
    if record_outcome(&mut run, checked).is_none() {
        run.stopped = Some(stop_compositor(bench, &mut running));
        return run;
    }
    run.memory.push(memory(bench, &running, "ready"));
    // Hyprland 0.57.1 asks for SCHED_RR on its main thread (src/init/initHelpers.cpp).
    let realtime_at_ready = realtime_threads(bench, &running);
    run.notes.push(format!(
        "the core's threads under a realtime policy, at ready: {}",
        listed(&realtime_at_ready)
    ));
    let measured = match case.scenario.as_str() {
        "rest" => rest(bench, &running, case.windows.unwrap_or(1), &mut run),
        "game" => game(bench, &running, &mut run),
        "film" => film(bench, &running, &mut run),
        "pacing" => pacing(bench, &running, &mut run),
        "motion" => motion(bench, &running, &mut run),
        other => Err(Stop::Failed(format!("no scenario {other}"))),
    };
    let realtime_at_end = realtime_threads(bench, &running);
    if realtime_at_end != realtime_at_ready {
        run.notes.push(format!(
            "the core's threads under a realtime policy, at the end: {}",
            listed(&realtime_at_end)
        ));
    }
    run.stopped = Some(stop_compositor(bench, &mut running));
    // A compositor restarted during the case is not the one measured from its start.
    let measured = match restarted(recipe, &running.log()) {
        Some(error) => Err(Stop::Failed(error)),
        None => measured,
    };
    if let Some(measured) = record_outcome(&mut run, measured) {
        let output = finish_frames(recipe, &running, measured, &mut run);
        if let (Some(output), Some(info)) = (output, run.compositor.as_mut()) {
            info.output = Some(output);
        }
    }
    run.disturbed = run
        .phases
        .iter()
        .map(|phase| phase.load.disturbed)
        .reduce(|a, b| match (a, b) {
            (Some(a), Some(b)) => Some(a || b),
            (a, b) => a.or(b),
        })
        .flatten();
    run
}

/// Why the case fails if `log`, the compositor's, says it was restarted, as the recipe tells.
fn restarted(recipe: &Recipe, log: &str) -> Option<String> {
    let line = recipe.restarted.filter(|line| log.contains(line))?;
    Some(format!(
        "{} was restarted during the case: its log says \"{line}\"",
        recipe.name
    ))
}

/// The threads of the compositor's core under a realtime policy, as `Hyprland's Hyprland:
/// SCHED_RR, priority 1`, `× 2` after those that are several.
fn realtime_threads(bench: &Bench, running: &Running) -> Vec<String> {
    let mut threads: BTreeMap<String, usize> = BTreeMap::new();
    for pid in running.sandboxed.core().processes() {
        let process = bench.proc.name(pid).unwrap_or_default();
        for (thread, scheduling) in bench.proc.scheduling(pid) {
            if procfs::REALTIME_POLICIES.contains(&scheduling.policy) {
                *threads
                    .entry(format!(
                        "{process}'s {thread}: {}, priority {}",
                        scheduling.policy, scheduling.priority
                    ))
                    .or_default() += 1;
            }
        }
    }
    threads
        .into_iter()
        .map(|(thread, count)| match count {
            1 => thread,
            count => format!("{thread} × {count}"),
        })
        .collect()
}

/// `items` in a sentence; `none` when there are none.
fn listed(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items.join("; ")
    }
}

/// Starts `recipe` in `configuration`, in a sandbox of its own under the run's folder, and waits
/// until it is ready.
fn start_compositor(
    bench: &mut Bench,
    recipe: &'static Recipe,
    configuration: Configuration,
    name: &str,
) -> Result<(Running, Plan, Duration), String> {
    let folders = Folders::create(&bench.folder.join(name))?;
    let plan = recipes::plan(
        recipe,
        configuration,
        &bench.shell,
        &folders,
        &bench.prepared,
    )?;
    for (path, text) in &plan.files {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(link) = &plan.runtime_link {
        std::os::unix::fs::symlink(folders.runtime(), link)
            .map_err(|e| format!("{}: {e}", link.display()))?;
    }
    let mut command = vec![plan.program.to_string_lossy().into_owned()];
    command.extend(plan.arguments.iter().cloned());
    let launch = Launch {
        unit: bench.unit(recipe.name),
        command,
        environment: compositor_environment(&folders, &bench.shell, &plan.path, &plan.variables),
        folder: folders.root.clone(),
        log: folders.root.join("compositor.log"),
        compositor: true,
        parent: plan.parent.clone(),
        input: false,
    };
    let started = Instant::now();
    let sandboxed = match Sandboxed::start(&launch, &bench.shell, &bench.program) {
        Ok(sandboxed) => sandboxed,
        Err(error) => {
            if let Some(link) = &plan.runtime_link {
                let _ = fs::remove_file(link);
            }
            return Err(error);
        }
    };
    let mut running = Running {
        recipe,
        folders,
        sandboxed,
        wayland_display: String::new(),
        sway_socket: None,
        output: None,
        runtime_link: plan.runtime_link.clone(),
    };
    let ready = wait_ready(bench, &mut running).and_then(|()| {
        let ready_after = started.elapsed();
        for action in recipe.setup {
            send(&bench.shell, &running, None, *action)?;
        }
        running.output = wait_output(bench, &running)?;
        Ok(ready_after)
    });
    match ready {
        Ok(ready_after) => Ok((running, plan, ready_after)),
        Err(error) => {
            let stopped = stop_compositor(bench, &mut running);
            let log = running.log();
            let tail: Vec<&str> = log.lines().rev().take(15).collect();
            Err(format!(
                "{} was not ready: {error}; it {stopped}; its log ends:\n{}",
                recipe.name,
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            ))
        }
    }
}

/// Waits until the compositor's Wayland socket is there and its recipe says it is ready.
fn wait_ready(bench: &Bench, running: &mut Running) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    let runtime = running.folders.runtime();
    loop {
        if let Some(status) = running.sandboxed.ended() {
            return Err(format!("it ended with {status}"));
        }
        let socket = wayland_socket(&runtime);
        let ready = match running.recipe.ready {
            Ready::Swaymsg => {
                running.sway_socket = file_in(&runtime, "sway-ipc.", ".sock");
                running
                    .sway_socket
                    .as_ref()
                    .and_then(|ipc| sway_output(&bench.shell, ipc))
                    .is_some()
            }
            Ready::BusName(name) => bus_has(&bench.shell, &running.bus(), name),
            Ready::Line(prefix) => running.log().lines().any(|line| line.starts_with(prefix)),
            Ready::Answers(action) => {
                socket.is_some() && send(&bench.shell, running, None, action).is_ok()
            }
        };
        if ready && let Some(socket) = socket {
            running.wayland_display = socket;
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("not ready after {} s", PATIENCE.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What reading a compositor's output gave.
enum Reading {
    /// The output, as `3840x2160@240.000,1.5`.
    Found(String),
    /// Not there yet, and why.
    NotYet(String),
    /// Read once the case has ended, from the compositor's frame log.
    Later,
}

/// The output the compositor made, read back as its recipe says once its setup is done; waits
/// up to 15 s for one it makes after it is ready, then fails.
fn wait_output(bench: &Bench, running: &Running) -> Result<Option<String>, String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match read_output(bench, running) {
            Reading::Found(output) => return Ok(Some(output)),
            Reading::Later => return Ok(None),
            Reading::NotYet(why) if Instant::now() > deadline => {
                return Err(format!("its output: {why}"));
            }
            Reading::NotYet(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// Reads the output the compositor made, once.
fn read_output(bench: &Bench, running: &Running) -> Reading {
    let shell = &bench.shell;
    let answer = |action: Action| -> Result<String, String> {
        let output = output_within(tool(shell, running, action)?, TOOL_PATIENCE)?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let found = |output: Option<String>, what: &str| match output {
        Some(output) => Reading::Found(output),
        None => Reading::NotYet(format!("{what} lists no such output")),
    };
    match running.recipe.output {
        Output::Swaymsg => found(
            running
                .sway_socket
                .as_ref()
                .and_then(|ipc| sway_output(shell, ipc)),
            "swaymsg",
        ),
        Output::YunaLog => found(logs::yuna_output(&running.log()), "its log"),
        Output::KwinLog => Reading::Later,
        Output::WlrRandr => match answer(Action::WlrRandr(&["--json"])) {
            Ok(text) => found(logs::wlr_randr_output(&text, "HEADLESS-1"), "wlr-randr"),
            Err(error) => Reading::NotYet(error),
        },
        Output::Umbriel => match answer(Action::Umbriel(&["outputs", "--json"])) {
            Ok(text) => found(logs::umbriel_output(&text, "HEADLESS-1"), "umbriel outputs"),
            Err(error) => Reading::NotYet(error),
        },
        Output::Hyprctl => match answer(Action::Hyprctl(&["-j", "monitors", "all"])) {
            Ok(text) => found(
                logs::hyprctl_output(&text, "HEADLESS-1"),
                "hyprctl monitors",
            ),
            Err(error) => Reading::NotYet(error),
        },
        Output::Gdctl => {
            let mut command = Command::new(shell.mutter.join("bin/gdctl"));
            command.arg("show").env_clear().env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", running.bus().display()),
            );
            match output_within(command, TOOL_PATIENCE) {
                Ok(output) => found(
                    logs::gdctl_output(&String::from_utf8_lossy(&output.stdout)),
                    "gdctl show",
                ),
                Err(error) => Reading::NotYet(error),
            }
        }
    }
}

/// Umbriel's count of its output's commits now, from `umbriel outputs --json`.
fn umbriel_commits(bench: &Bench, running: &Running) -> Option<u64> {
    let command = tool(
        &bench.shell,
        running,
        Action::Umbriel(&["outputs", "--json"]),
    )
    .ok()?;
    let output = output_within(command, TOOL_PATIENCE).ok()?;
    logs::umbriel_commits(&String::from_utf8_lossy(&output.stdout), "HEADLESS-1")
}

/// The name of the Wayland socket in `runtime`.
fn wayland_socket(runtime: &Path) -> Option<String> {
    sandbox::wayland_socket_in(runtime)?
        .file_name()?
        .to_str()
        .map(str::to_owned)
}

/// A file of `folder` whose name starts with `prefix` and ends with `suffix`, not a lock.
fn file_in(folder: &Path, prefix: &str, suffix: &str) -> Option<PathBuf> {
    let mut names: Vec<PathBuf> = fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| {
                    name.starts_with(prefix) && name.ends_with(suffix) && !name.ends_with(".lock")
                })
        })
        .collect();
    names.sort();
    names.into_iter().next()
}

/// sway's output, as `swaymsg -t get_outputs` reports it, once it is active.
fn sway_output(shell: &ShellPaths, ipc: &Path) -> Option<String> {
    let mut command = Command::new(shell.sway.join("bin/swaymsg"));
    command
        .arg("-s")
        .arg(ipc)
        .args(["-r", "-t", "get_outputs"])
        .env_clear();
    let output = output_within(command, TOOL_PATIENCE).ok()?;
    let outputs: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let headless = outputs
        .as_array()?
        .iter()
        .find(|output| output["name"] == "HEADLESS-1" && output["active"] == true)?;
    let mode = &headless["current_mode"];
    Some(format!(
        "{}x{}@{:.3},{}",
        mode["width"],
        mode["height"],
        mode["refresh"].as_f64()? / 1000.0,
        headless["scale"]
    ))
}

/// Whether `name` is owned on the bus at `bus`.
fn bus_has(shell: &ShellPaths, bus: &Path, name: &str) -> bool {
    let mut command = Command::new(shell.dbus.join("bin/dbus-send"));
    command
        .arg(format!("--bus=unix:path={}", bus.display()))
        .args([
            "--print-reply",
            "--dest=org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.NameHasOwner",
        ])
        .arg(format!("string:{name}"))
        .env_clear();
    output_within(command, TOOL_PATIENCE)
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("boolean true"))
}

/// The longest one of a compositor's own tools may take to answer.
const TOOL_PATIENCE: Duration = Duration::from_secs(10);

/// Runs `command`, its standard input closed, and gives what it wrote once it has ended; kills
/// it if it has not within `patience`, so that a tool that hangs never stops the bench.
fn output_within(mut command: Command, patience: Duration) -> Result<std::process::Output, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    // Read apart, so that a tool that writes much never blocks on a full pipe.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + patience;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!(
                    "{program} gave no answer in {} s",
                    patience.as_secs()
                ));
            }
            Err(error) => break Err(format!("{program}: {error}")),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    Ok(std::process::Output {
        status: status?,
        stdout,
        stderr,
    })
}

/// Reads `pipe` to its end on a thread of its own.
fn drain<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

/// The command that runs `action`'s tool against the compositor of `running`, its environment
/// cleared but for the socket it needs, which is in the case's runtime folder; an error for keys,
/// which no tool types.
fn tool(shell: &ShellPaths, running: &Running, action: Action) -> Result<Command, String> {
    tool_at(
        shell,
        &Reach {
            folders: &running.folders,
            sway_socket: running.sway_socket.as_deref(),
            runtime_link: running.runtime_link.as_deref(),
        },
        action,
    )
}

/// Where a compositor's tools reach it: its folders, and the sockets the bench found or made.
struct Reach<'a> {
    folders: &'a Folders,
    sway_socket: Option<&'a Path>,
    runtime_link: Option<&'a Path>,
}

/// The command of [`tool`] for the compositor `reach` finds.
fn tool_at(shell: &ShellPaths, reach: &Reach, action: Action) -> Result<Command, String> {
    let runtime = reach.folders.runtime();
    let socket = |prefix: &str, suffix: &str| {
        file_in(&runtime, prefix, suffix)
            .ok_or_else(|| format!("no {prefix}*{suffix} socket in the runtime folder"))
    };
    let cleared = |program: PathBuf| {
        let mut command = Command::new(program);
        command.env_clear();
        command
    };
    let mut command;
    match action {
        Action::Sway(words) => {
            let ipc = reach.sway_socket.ok_or("no sway socket")?;
            command = cleared(shell.sway.join("bin/swaymsg"));
            command.arg("-s").arg(ipc).arg(words);
        }
        Action::KwinBus(method, arguments) => {
            command = cleared(shell.dbus.join("bin/dbus-send"));
            command
                .arg(format!("--bus=unix:path={}", reach.folders.bus().display()))
                .args(["--print-reply", "--dest=org.kde.KWin", "/KWin"])
                .arg(format!("org.kde.KWin.{method}"))
                .args(arguments);
        }
        Action::Mmsg(arguments) => {
            command = cleared(shell.mango.join("bin/mmsg"));
            command
                .args(arguments)
                .env("MANGO_INSTANCE_SIGNATURE", socket("mango-", ".sock")?);
        }
        Action::Umbriel(arguments) => {
            command = cleared(shell.umbriel.join("bin/umbriel"));
            command
                .args(arguments)
                .env("UMBRIEL_SOCKET", socket("umbriel-", ".sock")?);
        }
        // Its instances are the folders of `$XDG_RUNTIME_DIR/hypr`: the case's one alone,
        // through the short link Hyprland was given.
        Action::Hyprctl(arguments) => {
            command = cleared(shell.hyprland.join("bin/hyprctl"));
            command
                .args(["--instance", "0"])
                .args(arguments)
                .env("XDG_RUNTIME_DIR", reach.runtime_link.unwrap_or(&runtime));
        }
        Action::WlrRandr(arguments) => {
            let name = wayland_socket(&runtime).ok_or("no Wayland socket")?;
            command = cleared(shell.wlr_randr.join("bin/wlr-randr"));
            command
                .args(arguments)
                .env("WAYLAND_DISPLAY", runtime.join(name));
        }
        Action::Keys(_) => return Err("keys are typed by the keyboard client".to_owned()),
    }
    Ok(command)
}

/// Sends `action` to the compositor: through its own tool, or for keys, through `keyboard`.
fn send(
    shell: &ShellPaths,
    running: &Running,
    keyboard: Option<&mut Keyboard>,
    action: Action,
) -> Result<(), String> {
    if let Action::Keys(chord) = action {
        return keyboard
            .ok_or("keys need the keyboard client")?
            .type_chord(&chord.text());
    }
    let output = output_within(tool(shell, running, action)?, TOOL_PATIENCE)
        .map_err(|e| format!("{action:?}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{action:?}: {}{}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The compositor's own process: the one of `core` whose parent is outside it.
fn compositor_pid(bench: &Bench, running: &Running) -> Option<u32> {
    let core = running.sandboxed.core().processes();
    core.iter().copied().find(|pid| {
        bench
            .proc
            .stat(*pid)
            .is_some_and(|(parent, _)| !core.contains(&parent))
    })
}

/// Stops the compositor as its recipe says, then waits for its sandbox to empty; says how.
fn stop_compositor(bench: &Bench, running: &mut Running) -> String {
    let asked = match running.recipe.stop {
        Some(action) => send(&bench.shell, running, None, action),
        None => match compositor_pid(bench, running) {
            Some(pid) => sandbox::signal(pid, Signal::TERM).map_err(|e| e.to_string()),
            None => Err("no compositor process left".to_owned()),
        },
    };
    let ended = running.sandboxed.wait_or_kill(Duration::from_secs(10));
    if let Some(link) = running.runtime_link.take() {
        let _ = fs::remove_file(link);
    }
    match asked {
        Ok(()) => ended,
        // `swaymsg exit` finds no answer: sway has gone before it could send one.
        Err(_) if ended.starts_with("ended: exit status: 0") => ended,
        Err(error) => format!("{ended}, its stop failed: {error}"),
    }
}

/// Checks the sandbox's rules on the compositor's scope and each client's; a break refuses the
/// case.
fn refuse_if_broken(bench: &Bench, running: &Running, clients: &[&Client]) -> Result<(), Stop> {
    let mut groups: Vec<&Cgroup> = vec![&running.sandboxed.scope];
    groups.extend(clients.iter().map(|client| &client.sandboxed.scope));
    let sockets = SocketTable::read(&bench.proc.unix_sockets());
    let found = sandbox::check(&bench.proc, &groups, &bench.owner, &sockets);
    if found.is_empty() {
        Ok(())
    } else {
        Err(Stop::Refused(found))
    }
}

/// What the run's file says of the compositor.
fn compositor_info(bench: &Bench, running: &Running, plan: &Plan) -> CompositorInfo {
    let mesa_mapped: BTreeSet<String> = running
        .sandboxed
        .core()
        .processes()
        .into_iter()
        .flat_map(|pid| bench.proc.mapped_store_folders(pid))
        .filter(|folder| folder.contains("-mesa-"))
        .collect();
    CompositorInfo {
        name: running.recipe.name.to_owned(),
        version: plan.version.clone(),
        program: plan.program.to_string_lossy().into_owned(),
        backend: running.recipe.backend.to_owned(),
        renderer: running.recipe.renderer.to_owned(),
        settings: plan.settings.clone(),
        output_asked: recipes::OUTPUT.to_owned(),
        output: running.output.clone(),
        frames_counted_by: running.recipe.frames.describe().to_owned(),
        mesa_mapped: mesa_mapped.into_iter().collect(),
    }
}

/// Starts a client of `command` in a scope of its own, beside the compositor; with `input`, its
/// standard input is a pipe the bench writes to.
fn start_client(
    bench: &mut Bench,
    running: &Running,
    name: &str,
    command: Vec<String>,
    path: &str,
    input: bool,
) -> Result<Client, String> {
    let launch = Launch {
        unit: bench.unit(name),
        command,
        environment: client_environment(
            &running.folders,
            &bench.shell,
            path,
            &running.wayland_display,
        ),
        folder: running.folders.root.clone(),
        log: running.folders.root.join(format!("{name}.log")),
        compositor: false,
        parent: None,
        input,
    };
    let sandboxed = Sandboxed::start(&launch, &bench.shell, &bench.program)?;
    Ok(Client {
        name: name.to_owned(),
        sandboxed,
    })
}

/// Waits until `client`'s log holds a line that starts with `prefix`.
fn wait_for_line(client: &mut Client, prefix: &str) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if client
            .log()
            .is_some_and(|log| log.lines().any(|line| line.starts_with(prefix)))
        {
            return Ok(());
        }
        if let Some(status) = client.sandboxed.ended() {
            return Err(format!(
                "{} ended with {status}: {}",
                client.name,
                client.log().unwrap_or_default().trim()
            ));
        }
        if Instant::now() > deadline {
            return Err(format!("{} never printed {prefix:?}", client.name));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The still client with `windows` windows, once all of them show.
fn still_client(bench: &mut Bench, running: &Running, windows: u32) -> Result<Client, String> {
    let command = vec![
        bench.program.to_string_lossy().into_owned(),
        "client".to_owned(),
        "still".to_owned(),
        "--windows".to_owned(),
        windows.to_string(),
    ];
    let mut client = start_client(bench, running, "still", command, "", false)?;
    wait_for_line(&mut client, "ready")?;
    Ok(client)
}

/// The frame counter, once its session is set up and its first frame has passed.
fn frame_counter(bench: &mut Bench, running: &Running) -> Result<Client, String> {
    let command = vec![
        bench.program.to_string_lossy().into_owned(),
        "client".to_owned(),
        "frames".to_owned(),
    ];
    let mut client = start_client(bench, running, "frames", command, "", false)?;
    wait_for_line(&mut client, "session")?;
    std::thread::sleep(bench.durations.counter_lead);
    Ok(client)
}

/// The bench's keyboard, `yuna-bench client keys`, as the bench types into it: the pipe to its
/// standard input and its log, where it says each chord was handled.
struct Keyboard {
    input: std::process::ChildStdin,
    log: PathBuf,
    sent: u64,
}

impl Keyboard {
    /// Starts the keyboard in a scope of its own, connected to the compositor; gives it as a
    /// client, measured and checked as any other, and as the bench types into it.
    fn start(bench: &mut Bench, running: &Running) -> Result<(Client, Keyboard), String> {
        let command = vec![
            bench.program.to_string_lossy().into_owned(),
            "client".to_owned(),
            "keys".to_owned(),
        ];
        let mut client = start_client(bench, running, "keys", command, "", true)?;
        wait_for_line(&mut client, "ready")?;
        let input = client
            .sandboxed
            .child
            .stdin
            .take()
            .ok_or("the keyboard's standard input is not a pipe")?;
        let log = client.sandboxed.log.clone();
        Ok((
            client,
            Keyboard {
                input,
                log,
                sent: 0,
            },
        ))
    }

    /// Types `chord`, then waits until the keyboard says the compositor handled it.
    fn type_chord(&mut self, chord: &str) -> Result<(), String> {
        writeln!(self.input, "{chord}")
            .and_then(|()| self.input.flush())
            .map_err(|e| format!("the keyboard: {e}"))?;
        self.sent += 1;
        let handled = format!("sent {}", self.sent);
        let deadline = Instant::now() + TOOL_PATIENCE;
        loop {
            let log = fs::read_to_string(&self.log).unwrap_or_default();
            if log.lines().any(|line| line == handled) {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "the keyboard never said {handled:?}: {}",
                    log.trim()
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Sleeps `length`: a window in which the bench does nothing.
fn idle(length: Duration) -> impl FnMut() -> Result<(), String> {
    move || {
        std::thread::sleep(length);
        Ok(())
    }
}

/// Measures one window named `name` while `activity` runs: the compositor's core and stack,
/// each client apart, the machine; refuses it if any process held a forbidden node meanwhile,
/// whatever the activity's own end.
fn measure(
    bench: &Bench,
    running: &Running,
    clients: &[&Client],
    name: &str,
    activity: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Measured, Stop> {
    let core = running.sandboxed.core();
    let mut groups = vec![
        ("core".to_owned(), core.clone()),
        ("stack".to_owned(), running.sandboxed.scope.clone()),
    ];
    groups.extend(
        clients
            .iter()
            .map(|client| (client.name.clone(), client.sandboxed.scope.clone())),
    );
    let fdinfo_raw = |when: &str| -> Vec<RawFdinfo> {
        crate::sampler::drm_now(&bench.proc, &core)
            .into_iter()
            .map(|file| RawFdinfo {
                when: when.to_owned(),
                pid: file.pid,
                fd: file.fd,
                node: file.node,
                text: file.raw,
            })
            .collect()
    };
    let mut raw = fdinfo_raw("start");
    let machine = MachineWatch::start(RENDER_NODE);
    let watch = Watch::start(&bench.proc, groups.clone());
    // The exact counters are read right at the activity's ends: the watches' readings take
    // milliseconds, which the window's length would not count.
    let cpu_before: Vec<_> = groups.iter().map(|(_, group)| group.cpu()).collect();
    let from_ns = monotonic_ns();
    let started = Instant::now();
    let done = activity();
    let to_ns = monotonic_ns();
    let length = started.elapsed();
    let cpu_after: Vec<_> = groups.iter().map(|(_, group)| group.cpu()).collect();
    let forbidden = watch.forbidden();
    let windows = watch.finish(&bench.proc, &groups);
    let load = machine.finish();
    raw.extend(fdinfo_raw("end"));
    // A refusal first: a sandbox that broke its rules is the case's story, not the error it led
    // its activity into.
    if !forbidden.is_empty() {
        return Err(Stop::Refused(
            forbidden
                .iter()
                .map(|(pid, node)| {
                    format!("during the {name} window, process {pid} held {node} open")
                })
                .collect(),
        ));
    }
    done?;
    let mut usages = windows.iter().zip(cpu_before.iter().zip(&cpu_after)).map(
        |((name, window), (before, after))| (name.clone(), record::usage(*before, *after, window)),
    );
    let core_usage = usages
        .next()
        .map(|(_, usage)| usage)
        .ok_or_else(|| Stop::Failed("no core window".to_owned()))?;
    let stack_usage = usages
        .next()
        .map(|(_, usage)| usage)
        .ok_or_else(|| Stop::Failed("no stack window".to_owned()))?;
    let clients = usages
        .map(|(name, usage)| ClientUsage { name, usage })
        .collect();
    Ok(Measured {
        phase: Phase {
            name: name.to_owned(),
            seconds: Figure::new(Some(length.as_secs_f64()), "s"),
            figures: Figures::new(),
            core: core_usage,
            stack: stack_usage,
            clients,
            load,
            frames_counted_by: None,
            lower_bounds: Vec::new(),
            fdinfo_raw: raw,
        },
        marks: Marks { from_ns, to_ns },
    })
}

/// The scenario's windows: the cost window, and the frame counter's when the recipe counts by
/// capture, with the frames it counted, if its log could be read.
struct Windows {
    cost: Measured,
    frames: Option<(Measured, Option<u64>)>,
    /// The frames the compositor counted itself in the cost window, read at its ends: Umbriel's
    /// commits.
    own_count: Option<u64>,
    /// The frames the client counted itself in each window, in order, where the client's frames
    /// are the compositor's: mpv's in `game`.
    client_frames: Vec<Option<f64>>,
}

/// Checks the sandbox with the clients up, measures the cost window, then, for a recipe that
/// counts by capture, the same activity again with the frame counter.
fn cost_and_frames(
    bench: &mut Bench,
    running: &Running,
    clients: &[&Client],
    activity: &mut dyn FnMut() -> Result<(), String>,
    run: &mut CaseRun,
) -> Result<Windows, Stop> {
    refuse_if_broken(bench, running, clients)?;
    let mut own_count = None;
    let cost = if running.recipe.frames == Frames::UmbrielCommits {
        // Read inside the window, at its very ends: two answers of Umbriel's in its costs.
        let bench = &*bench;
        let mut counted = || {
            let before = umbriel_commits(bench, running);
            let done = activity();
            let after = umbriel_commits(bench, running);
            own_count = before
                .zip(after)
                .map(|(before, after)| after.saturating_sub(before));
            done
        };
        measure(bench, running, clients, "cost", &mut counted)?
    } else {
        measure(bench, running, clients, "cost", activity)?
    };
    run.memory.push(memory(bench, running, "after"));
    if running.recipe.frames != Frames::Capture {
        return Ok(Windows {
            cost,
            frames: None,
            own_count,
            client_frames: Vec::new(),
        });
    }
    let counter = frame_counter(bench, running)?;
    let mut with_counter: Vec<&Client> = clients.to_vec();
    with_counter.push(&counter);
    let frames = refuse_if_broken(bench, running, &with_counter)
        .and_then(|()| measure(bench, running, &with_counter, "frames", activity));
    let log = counter.log();
    counter.stop(Signal::TERM);
    let frames = frames?;
    let count = log.map(|log| logs::counted_frames(&log, frames.marks.from_ns, frames.marks.to_ns));
    Ok(Windows {
        cost,
        frames: Some((frames, count)),
        own_count,
        client_frames: Vec::new(),
    })
}

fn rest(
    bench: &mut Bench,
    running: &Running,
    windows: u32,
    run: &mut CaseRun,
) -> Result<Windows, Stop> {
    let still = still_client(bench, running, windows)?;
    std::thread::sleep(bench.durations.settle);
    let windows = cost_and_frames(
        bench,
        running,
        &[&still],
        &mut idle(bench.durations.rest),
        run,
    );
    still.stop(Signal::TERM);
    windows
}

/// A clip the bench makes with ffmpeg's test pattern, H.264.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clip {
    /// Its name, and its file's.
    pub name: &'static str,
    /// Its size, as ffmpeg takes it.
    pub size: &'static str,
    /// Its frames a second.
    pub fps: u32,
}

impl Clip {
    /// Its length in seconds: as long as mpv may play before its last window ends, so that it
    /// never loops inside one. That is the wait for its window to be fullscreen, the warm-up,
    /// the cost window, the frame counter's lead and the frames window, at up to 1.1 times the
    /// clip's rate: wlroots' headless backend takes `game`'s frames at up to 250 a second. mpv
    /// is not sought back to its start instead: on Mutter 51.0, mpv with `--untimed` and a FIFO
    /// swap chain sometimes stays at its first frames, with or without a seek, and a seek is one
    /// more chance of it.
    pub fn seconds(durations: &Durations) -> u32 {
        let played = FULLSCREEN_PATIENCE
            + durations.warm_up
            + durations.window
            + durations.counter_lead
            + durations.window;
        (played.as_secs_f64() * 1.1).ceil() as u32
    }
}

/// The film: 3840 × 2160 at 24 frames a second.
pub const FILM: Clip = Clip {
    name: "film",
    size: "3840x2160",
    fps: 24,
};

/// The game's scene: 1920 × 1080 at 240 frames a second, which mpv decodes on the CPU faster than
/// that, and draws scaled to the whole output on the GPU.
pub const GAME: Clip = Clip {
    name: "game",
    size: "1920x1080",
    fps: 240,
};

/// The longest mpv's window may take to be fullscreen at the output's size.
const FULLSCREEN_PATIENCE: Duration = Duration::from_secs(15);

/// Makes `clip` into the run's folder, once a run.
pub fn make_clip(bench: &mut Bench, clip: Clip) -> Result<PathBuf, String> {
    if let Some(path) = bench.clips.get(clip.name) {
        return Ok(path.clone());
    }
    let path = bench
        .folder
        .join(format!("clip-{}-{}x{}.mkv", clip.name, clip.size, clip.fps));
    let output = Command::new(bench.shell.ffmpeg.join("bin/ffmpeg"))
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg(format!("testsrc2=size={}:rate={}", clip.size, clip.fps))
        .args(["-t", &Clip::seconds(&bench.durations).to_string()])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&path)
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ffmpeg: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffmpeg: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    bench.clips.insert(clip.name, path.clone());
    Ok(path)
}

/// mpv playing `clip` fullscreen, drawing with Vulkan on radv, with `extra` options, its IPC
/// at `socket`; once its IPC answers.
fn mpv_client(
    bench: &mut Bench,
    running: &Running,
    clip: &Path,
    extra: &[&str],
    socket: &Path,
) -> Result<Client, String> {
    let mpv = bench.application("mpv")?;
    let mut command = vec![
        mpv.to_string_lossy().into_owned(),
        "--no-config".to_owned(),
        "--fs".to_owned(),
        "--no-audio".to_owned(),
        "--hwdec=no".to_owned(),
        "--loop-file=inf".to_owned(),
        "--vo=gpu-next".to_owned(),
        "--gpu-api=vulkan".to_owned(),
        format!("--input-ipc-server={}", socket.display()),
    ];
    command.extend(extra.iter().map(|option| (*option).to_owned()));
    command.push(clip.to_string_lossy().into_owned());
    let path = bench.shell.applications_path.clone();
    let mut client = start_client(bench, running, "mpv", command, &path, false)?;
    let deadline = Instant::now() + PATIENCE;
    while logs::mpv_value(socket, "pid").is_err() {
        if let Some(status) = client.sandboxed.ended() {
            return Err(format!(
                "mpv ended with {status}: {}",
                client.log().unwrap_or_default().trim()
            ));
        }
        if Instant::now() > deadline {
            client.stop(Signal::TERM);
            return Err("mpv's IPC never answered".to_owned());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(client)
}

/// Waits until mpv's window is fullscreen at the output's logical size, as mpv sees it: the
/// compositor told it fullscreen, and its size divided by its scale is the output's. Refuses the
/// case if it never is: the scene would not be the scenario's. An mpv that ended meanwhile fails
/// the case.
fn wait_fullscreen(socket: &Path, mpv: &mut Client, run: &mut CaseRun) -> Result<(), Stop> {
    let (logical_width, logical_height) = recipes::output_logical_size();
    let deadline = Instant::now() + FULLSCREEN_PATIENCE;
    loop {
        if let Some(status) = mpv.sandboxed.ended() {
            return Err(Stop::Failed(format!(
                "mpv ended with {status} before its window was fullscreen: {}",
                mpv.log().unwrap_or_default().trim()
            )));
        }
        let fullscreen = logs::mpv_value(socket, "fullscreen").ok();
        let size = logs::mpv_value(socket, "osd-dimensions").ok();
        let scale = logs::mpv_property(socket, "display-hidpi-scale")
            .ok()
            .flatten();
        let (width, height) = size.as_ref().map_or((None, None), |size| {
            (size["w"].as_f64(), size["h"].as_f64())
        });
        let logical = match (width, height, scale) {
            (Some(w), Some(h), Some(scale)) if scale > 0.0 => {
                Some(((w / scale).round() as u32, (h / scale).round() as u32))
            }
            _ => None,
        };
        let seen = format!(
            "fullscreen {}, {} × {} pixels at scale {}",
            fullscreen
                .as_ref()
                .map_or("unknown".to_owned(), |v| v.to_string()),
            width.map_or("?".to_owned(), |w| w.to_string()),
            height.map_or("?".to_owned(), |h| h.to_string()),
            scale.map_or("?".to_owned(), |s| s.to_string()),
        );
        if fullscreen.as_ref().and_then(|v| v.as_bool()) == Some(true)
            && logical == Some((logical_width, logical_height))
        {
            run.notes.push(format!("mpv's window: {seen}"));
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(Stop::Refused(vec![format!(
                "mpv's window is not fullscreen at the output's {logical_width} × \
                 {logical_height}: {seen}"
            )]));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Lets mpv play for `length`, and fails the case if its clip did not: a stalled mpv is no scene
/// to measure. mpv with `--untimed` and a FIFO swap chain sometimes stays at its first frames on
/// Mutter 51.0, sought or not: four of the bench's six starts of `game` there on 2026-11-16.
fn warm_up(socket: &Path, length: Duration) -> Result<(), Stop> {
    let position = || logs::mpv_property(socket, "time-pos").ok().flatten();
    let before = position();
    std::thread::sleep(length);
    played(before, position(), length).map_err(Stop::Failed)
}

/// Whether mpv's position moved from `before` to `after` by half of `length` at least: its clip
/// played, at its own rate or near it.
fn played(before: Option<f64>, after: Option<f64>, length: Duration) -> Result<(), String> {
    let moved = after.zip(before).map(|(after, before)| after - before);
    match moved {
        Some(moved) if moved >= length.as_secs_f64() / 2.0 => Ok(()),
        _ => Err(format!(
            "mpv's clip did not play in its {} s of warm-up: its position went from {} to {}",
            length.as_secs_f64(),
            before.map_or("unknown".to_owned(), |s| format!("{s:.3} s")),
            after.map_or("unknown".to_owned(), |s| format!("{s:.3} s")),
        )),
    }
}

/// What mpv's IPC gives of the frames it showed: its position and its counters of drops.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MpvFrames {
    /// Its position in the clip, in seconds: `time-pos`, the time of the frame shown.
    pub position_s: Option<f64>,
    /// Frames its output dropped, `frame-drop-count`.
    pub dropped: Option<f64>,
    /// Frames its decoder dropped, `decoder-frame-drop-count`.
    pub decoder_dropped: Option<f64>,
}

impl MpvFrames {
    fn read(socket: &Path) -> MpvFrames {
        let number = |name: &str| logs::mpv_property(socket, name).ok().flatten();
        MpvFrames {
            position_s: number("time-pos"),
            dropped: number("frame-drop-count"),
            decoder_dropped: number("decoder-frame-drop-count"),
        }
    }

    /// The frames shown from `before` to `self` of a clip at `fps`: the frames its position
    /// moved through, a constant-rate clip's timestamps being exact, less those dropped. None if
    /// a reading is missing, or the clip looped between them.
    pub fn shown_since(&self, before: &MpvFrames, fps: f64) -> Option<f64> {
        let moved = self.position_s? - before.position_s?;
        if moved < 0.0 {
            return None;
        }
        let dropped =
            (self.dropped? - before.dropped?) + (self.decoder_dropped? - before.decoder_dropped?);
        Some((moved * fps).round() - dropped)
    }
}

fn game(bench: &mut Bench, running: &Running, run: &mut CaseRun) -> Result<Windows, Stop> {
    let clip = make_clip(bench, GAME)?;
    let socket = running.folders.runtime().join("mpv.sock");
    // Every frame decoded is shown, as soon as the compositor takes it: a FIFO swap chain paces
    // mpv by the compositor's frames, as a game's.
    let mut client = mpv_client(
        bench,
        running,
        &clip,
        &["--untimed", "--vulkan-swap-mode=fifo"],
        &socket,
    )?;
    let windows = (|| {
        wait_fullscreen(&socket, &mut client, run)?;
        warm_up(&socket, bench.durations.warm_up)?;
        run.notes.push(format!(
            "game: mpv fullscreen, --untimed, a FIFO swap chain, drawing with Vulkan on radv a \
             {}-frame-a-second clip of {}, decoded on the CPU",
            GAME.fps, GAME.size
        ));
        let window = bench.durations.window;
        let mut shown = Vec::new();
        let mut activity = || {
            let before = MpvFrames::read(&socket);
            std::thread::sleep(window);
            shown.push(MpvFrames::read(&socket).shown_since(&before, f64::from(GAME.fps)));
            Ok(())
        };
        let mut windows = cost_and_frames(bench, running, &[&client], &mut activity, run)?;
        windows.client_frames = shown;
        Ok(windows)
    })();
    client.stop(Signal::TERM);
    windows
}

fn film(bench: &mut Bench, running: &Running, run: &mut CaseRun) -> Result<Windows, Stop> {
    let clip = make_clip(bench, FILM)?;
    let socket = running.folders.runtime().join("mpv.sock");
    let mut client = mpv_client(bench, running, &clip, &[], &socket)?;
    let windows = (|| {
        wait_fullscreen(&socket, &mut client, run)?;
        warm_up(&socket, bench.durations.warm_up)?;
        run.notes.push(
            "film: mpv fullscreen, software decoding (--hwdec=no), drawn with Vulkan on radv \
             (--vo=gpu-next)"
                .to_owned(),
        );
        let counters = |socket: &Path| -> Vec<Option<f64>> {
            logs::MPV_COUNTERS
                .iter()
                .map(|name| logs::mpv_property(socket, name).ok().flatten())
                .collect()
        };
        let window = bench.durations.window;
        let mut readings = Vec::new();
        let mut activity = || {
            let (before, frames_before) = (counters(&socket), MpvFrames::read(&socket));
            std::thread::sleep(window);
            let shown = MpvFrames::read(&socket).shown_since(&frames_before, f64::from(FILM.fps));
            readings.push((before, counters(&socket), shown));
            Ok(())
        };
        let mut windows = cost_and_frames(bench, running, &[&client], &mut activity, run)?;
        let phases = std::iter::once(&mut windows.cost)
            .chain(windows.frames.as_mut().map(|(measured, _)| measured));
        for (measured, (before, after, shown)) in phases.zip(readings) {
            let figures = &mut measured.phase.figures;
            for ((name, before), after) in logs::MPV_COUNTERS.iter().zip(before).zip(after) {
                let value = after.zip(before).map(|(a, b)| a - b);
                figures.insert(
                    format!("mpv_{}", name.replace('-', "_")),
                    Figure::new(value, "count"),
                );
            }
            // The clip's frames mpv showed: its cadence, not the compositor's frames, which may
            // be more.
            figures.insert("mpv_shown".to_owned(), Figure::new(shown, "count"));
        }
        Ok(windows)
    })();
    client.stop(Signal::TERM);
    windows
}

fn pacing(bench: &mut Bench, running: &Running, run: &mut CaseRun) -> Result<Windows, Stop> {
    let command = vec![
        // Line-buffered, so that the lines of a window are in the log at its end.
        bench
            .shell
            .coreutils
            .join("bin/stdbuf")
            .to_string_lossy()
            .into_owned(),
        "-oL".to_owned(),
        bench
            .shell
            .weston
            .join("bin/weston-presentation-shm")
            .to_string_lossy()
            .into_owned(),
    ];
    let client = start_client(bench, running, "presentation", command, "", false)?;
    std::thread::sleep(bench.durations.warm_up);
    let window = bench.durations.window;
    let log = client.sandboxed.log.clone();
    let mut stretches: Vec<Option<String>> = Vec::new();
    let mut activity = || {
        let from = fs::read_to_string(&log).map(|text| text.len());
        std::thread::sleep(window);
        let stretch = match (from, fs::read_to_string(&log)) {
            (Ok(from), Ok(text)) => text.get(from..).map(str::to_owned),
            _ => None,
        };
        stretches.push(stretch);
        Ok(())
    };
    let windows = cost_and_frames(bench, running, &[&client], &mut activity, run);
    client.stop(Signal::INT);
    let mut windows = windows?;
    run.notes.push(
        "pacing: weston-presentation-shm in its feedback mode, a 250 × 250 window".to_owned(),
    );
    let phases = std::iter::once(&mut windows.cost)
        .chain(windows.frames.as_mut().map(|(measured, _)| measured));
    for (measured, stretch) in phases.zip(stretches) {
        let read = stretch.as_deref().map(logs::parse_presentation_log);
        let mut between: Vec<f64> = read
            .as_ref()
            .map(|read| read.between_us.iter().map(|us| *us as f64).collect())
            .unwrap_or_default();
        between.sort_by(f64::total_cmp);
        let count = |pick: fn(&logs::PresentationLog) -> u64| {
            Figure::new(read.as_ref().map(|read| pick(read) as f64), "count")
        };
        let figures = &mut measured.phase.figures;
        figures.insert(
            "presented".to_owned(),
            count(|read| read.between_us.len() as u64),
        );
        figures.insert("discarded".to_owned(), count(|read| read.discarded));
        figures.insert("unread_lines".to_owned(), count(|read| read.unread));
        for (name, p) in [("median", 50.0), ("p99", 99.0), ("max", 100.0)] {
            figures.insert(
                format!("p2p_{name}"),
                Figure::new(logs::percentile(&between, p), "us"),
            );
        }
        // A presentation more than half a period after the usual one missed a refresh.
        let late = logs::percentile(&between, 50.0)
            .map(|median| between.iter().filter(|p| **p > median * 1.5).count() as f64);
        figures.insert("p2p_over_1.5_median".to_owned(), Figure::new(late, "count"));
    }
    Ok(windows)
}

fn motion(bench: &mut Bench, running: &Running, run: &mut CaseRun) -> Result<Windows, Stop> {
    let still = still_client(bench, running, MOTION_WINDOWS)?;
    let actions = running.recipe.actions;
    // A compositor driven by its keys alone gets the bench's keyboard, a client of its own.
    let typed = actions
        .iter()
        .any(|action| matches!(action, Action::Keys(_)));
    let (keys, mut keyboard) = if typed {
        match Keyboard::start(bench, running) {
            Ok((client, keyboard)) => (Some(client), Some(keyboard)),
            Err(error) => {
                still.stop(Signal::TERM);
                return Err(Stop::Failed(error));
            }
        }
    } else {
        (None, None)
    };
    std::thread::sleep(bench.durations.settle);
    let settle = bench.durations.action;
    let shell = bench.shell.clone();
    let mut round = || -> Result<(), String> {
        for action in actions {
            send(&shell, running, keyboard.as_mut(), *action)?;
            std::thread::sleep(settle);
        }
        Ok(())
    };
    // A round to warm up: each action's first run loads what it needs.
    let windows = match round() {
        Ok(()) => {
            run.notes.push(format!(
                "motion: {} actions, {} ms after each: {}",
                actions.len(),
                settle.as_millis(),
                actions
                    .iter()
                    .map(|action| action.text())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            let mut clients = vec![&still];
            clients.extend(keys.as_ref());
            cost_and_frames(bench, running, &clients, &mut round, run)
        }
        Err(error) => Err(Stop::Failed(error)),
    };
    still.stop(Signal::TERM);
    if let Some(keys) = keys {
        keys.stop(Signal::TERM);
    }
    windows
}

/// How a window's frames were counted.
#[derive(Debug, Clone, PartialEq)]
pub struct Count {
    /// The frames.
    pub frames: Option<f64>,
    /// How they were counted.
    pub by: String,
    /// Whether the count is exact and the window's own: only then is a cost a frame given.
    pub exact: bool,
}

/// Counts each window's frames now that the compositor has stopped and flushed its logs, and
/// derives the figures; gives the output the compositor's frames report, if they report one.
fn finish_frames(
    recipe: &Recipe,
    running: &Running,
    windows: Windows,
    run: &mut CaseRun,
) -> Option<String> {
    let mut output = None;
    let Windows {
        mut cost,
        frames,
        own_count,
        client_frames,
    } = windows;
    let kwin_frames = |marks: &Marks, output: &mut Option<String>| -> Option<u64> {
        let csv = file_in(&running.folders.root, "kwin perf statistics", ".csv")
            .and_then(|path| fs::read_to_string(path).ok())?;
        let (frames, refresh_ns) = logs::kwin_frames(&csv, marks.from_ns, marks.to_ns);
        if let Some(refresh_ns) = refresh_ns {
            *output = Some(format!(
                "a refresh of {refresh_ns:.0} ns by its frame log: {:.2} Hz",
                1e9 / refresh_ns
            ));
        }
        Some(frames)
    };
    // The compositor's own exact count of the cost window, where it keeps one.
    let (own, own_by) = match recipe.frames {
        Frames::KwinLog => (
            kwin_frames(&cost.marks, &mut output),
            "KWin's frame log, in this window",
        ),
        Frames::UmbrielCommits => (
            own_count,
            "Umbriel's count of its output's commits, read at this window's ends",
        ),
        Frames::Capture | Frames::None(_) => (None, ""),
    };
    let counts_itself = matches!(recipe.frames, Frames::KwinLog | Frames::UmbrielCommits);
    let mpv_by = "mpv's frames shown in this window: its position and drops, over its IPC";
    let count = match client_frames.first() {
        // The client's frames are the compositor's: mpv in `game`, paced one for one.
        Some(Some(shown)) => {
            if let Some(own) = own {
                cost.phase.figures.insert(
                    "compositor_frames".to_owned(),
                    Figure::new(Some(own as f64), "count"),
                );
            }
            Some(Count {
                frames: Some(*shown),
                by: mpv_by.to_owned(),
                exact: true,
            })
        }
        Some(None) => {
            run.notes.push(
                "mpv's count of the cost window is missing: one of its readings failed, or its \
                 clip looped"
                    .to_owned(),
            );
            match own {
                Some(own) => {
                    run.notes.push(format!(
                        "the cost window's frames are the compositor's own count instead: {own_by}"
                    ));
                    Some(Count {
                        frames: Some(own as f64),
                        by: own_by.to_owned(),
                        exact: true,
                    })
                }
                None => Some(Count {
                    frames: None,
                    by: mpv_by.to_owned(),
                    exact: false,
                }),
            }
        }
        None if counts_itself => Some(Count {
            frames: own.map(|own| own as f64),
            by: own_by.to_owned(),
            exact: own.is_some(),
        }),
        None => None,
    };
    derive(&mut cost.phase, count);
    let counted = frames.map(|(mut counted, captured)| {
        derive(
            &mut counted.phase,
            Some(Count {
                frames: captured.map(|frames| frames as f64),
                by: "ext-image-copy-capture in this window, whose copies weigh on the costs: it \
                     misses frames that come faster than its copies"
                    .to_owned(),
                exact: false,
            }),
        );
        if let Some(shown) = client_frames.get(1) {
            counted
                .phase
                .figures
                .insert("client_frames".to_owned(), Figure::new(*shown, "count"));
        }
        counted.phase
    });
    achieved_rate(&mut cost.phase, counted.as_ref());
    run.phases.push(cost.phase);
    if let Some(counted) = counted {
        run.phases.push(counted);
        run.notes.push(
            "frames counted by capture in a window apart, a lower bound: no cost a frame comes \
             from them"
                .to_owned(),
        );
    }
    output
}

/// Records in the cost window the rate the compositor really drew at, `achieved_frames_per_s`,
/// which its costs a second compare by: its own exact count; else, in `pacing`, the frames it
/// presented to the client, each one in the log; else the frames window's, a lower bound. The
/// summary ranks by the recipe's pace instead ([`recipes::Pace`]): wlroots' headless backend
/// draws a frame 4 ms after each commit, up to 250 frames a second for a 240 Hz mode, fewer
/// under load.
pub fn achieved_rate(cost: &mut Phase, frames: Option<&Phase>) {
    let rate = |phase: &Phase| {
        let value = phase.figures.get("frames_per_s")?.value?;
        Some((
            value,
            phase.lower_bounds.iter().any(|key| key == "frames_per_s"),
        ))
    };
    let presented = || {
        let seconds = cost.seconds.value.filter(|seconds| *seconds > 0.0)?;
        Some((cost.figures.get("presented")?.value? / seconds, false))
    };
    let achieved = rate(cost)
        .filter(|(_, lower_bound)| !lower_bound)
        .or_else(presented)
        .or_else(|| frames.and_then(rate));
    if let Some((value, lower_bound)) = achieved {
        cost.figures.insert(
            "achieved_frames_per_s".to_owned(),
            Figure::new(Some(value), "per_s"),
        );
        if lower_bound {
            cost.lower_bounds.push("achieved_frames_per_s".to_owned());
        }
    }
}

/// A figure in ms a second from `ns` over `seconds`.
fn per_second_ms(ns: Option<f64>, seconds: f64) -> Figure {
    Figure::new(ns.map(|ns| ns / 1e6 / seconds), "ms_per_s")
}

/// Derives a window's figures: each cost a second, its wake-ups a second, which are lower bounds
/// where processes escaped the readings, its frames as `count` gives them, and each cost a frame
/// only where that count is exact.
pub fn derive(phase: &mut Phase, count: Option<Count>) {
    let seconds = phase.seconds.value.unwrap_or(0.0).max(1e-3);
    let drawing = |usage: &record::Usage| -> Option<f64> {
        let engines: Vec<f64> = DRAWING_ENGINES
            .iter()
            .filter_map(|engine| usage.gpu.get(*engine)?.value)
            .collect();
        (!engines.is_empty()).then(|| engines.iter().sum())
    };
    let mut figures = Figures::new();
    let mut lower_bounds = Vec::new();
    let mut costs: Vec<(String, Option<f64>)> = vec![
        ("core_cpu".to_owned(), phase.core.cpu.value),
        ("core_gpu".to_owned(), drawing(&phase.core)),
        ("stack_cpu".to_owned(), phase.stack.cpu.value),
        ("stack_gpu".to_owned(), drawing(&phase.stack)),
    ];
    for client in &phase.clients {
        costs.push((
            format!("client_{}_cpu", client.name),
            client.usage.cpu.value,
        ));
        costs.push((
            format!("client_{}_gpu", client.name),
            drawing(&client.usage),
        ));
    }
    for (name, ns) in &costs {
        figures.insert(name.clone(), per_second_ms(*ns, seconds));
    }
    for (engine, figure) in &phase.core.gpu {
        figures.insert(
            format!("core_gpu_{engine}"),
            per_second_ms(figure.value, seconds),
        );
    }
    let mut woken: Vec<(String, &record::Usage)> = vec![
        ("core".to_owned(), &phase.core),
        ("stack".to_owned(), &phase.stack),
    ];
    woken.extend(
        phase
            .clients
            .iter()
            .map(|client| (format!("client_{}", client.name), &client.usage)),
    );
    for (name, usage) in woken {
        let per_s = |figure: &Figure| Figure::new(figure.value.map(|v| v / seconds), "per_s");
        figures.insert(format!("{name}_wakeups"), per_s(&usage.wakeups));
        figures.insert(format!("{name}_preemptions"), per_s(&usage.preemptions));
        if usage.wakeups_lower_bound {
            lower_bounds.push(format!("{name}_wakeups"));
            lower_bounds.push(format!("{name}_preemptions"));
        }
    }
    let frames = count.as_ref().and_then(|count| count.frames);
    figures.insert("frames".to_owned(), Figure::new(frames, "count"));
    figures.insert(
        "frames_per_s".to_owned(),
        Figure::new(frames.map(|f| f / seconds), "per_s"),
    );
    match &count {
        Some(Count {
            frames: Some(frames),
            exact: true,
            ..
        }) if *frames > 0.0 => {
            for (name, ns) in &costs {
                figures.insert(
                    format!("{name}_per_frame"),
                    Figure::new(ns.map(|ns| ns / 1e3 / frames), "us"),
                );
            }
        }
        Some(Count { exact: false, .. }) => {
            lower_bounds.push("frames".to_owned());
            lower_bounds.push("frames_per_s".to_owned());
        }
        _ => {}
    }
    phase.frames_counted_by = count.map(|count| count.by);
    phase.figures.extend(figures);
    phase.lower_bounds.extend(lower_bounds);
}

/// The memory of the sandbox's processes now, from `smaps_rollup` and the DRM statistics.
fn memory(bench: &Bench, running: &Running, when: &str) -> Memory {
    let core: BTreeSet<u32> = running.sandboxed.core().processes().into_iter().collect();
    let stack = running.sandboxed.scope.processes();
    let mut processes = Vec::new();
    for &pid in &stack {
        let Some(rollup) = bench.proc.smaps_rollup(pid) else {
            continue;
        };
        let files = drm::drm_files(&bench.proc, &[pid]);
        processes.push(ProcessMemory {
            pid,
            name: bench.proc.name(pid).unwrap_or_default(),
            core: core.contains(&pid),
            memory: memory_of(&[rollup], &files),
        });
    }
    let sum = |pids: &[u32]| {
        let rollups: Vec<String> = pids
            .iter()
            .filter_map(|pid| bench.proc.smaps_rollup(*pid))
            .collect();
        memory_of(&rollups, &drm::drm_files(&bench.proc, pids))
    };
    let core_pids: Vec<u32> = core.iter().copied().collect();
    Memory {
        when: when.to_owned(),
        core: sum(&core_pids),
        stack: sum(&stack),
        processes,
    }
}

/// The memory of `rollups`, each a process's `smaps_rollup`, and of the DRM clients of `files`.
fn memory_of(rollups: &[String], files: &[drm::DrmFile]) -> MemorySum {
    let total = |key: &str| -> Figure {
        let values: Vec<u64> = rollups
            .iter()
            .filter_map(|r| procfs::parse_kib(r, key))
            .collect();
        Figure::new(
            (!values.is_empty()).then(|| values.iter().sum::<u64>() as f64),
            "KiB",
        )
    };
    let (anon, file, shmem) = (total("Pss_Anon"), total("Pss_File"), total("Pss_Shmem"));
    let pss = match (anon.value, file.value, shmem.value) {
        (Some(a), Some(f), Some(s)) => Some(a + f + s),
        _ => None,
    };
    MemorySum {
        pss: Figure::new(pss, "KiB"),
        pss_anon: anon,
        pss_file: file,
        pss_shmem: shmem,
        rss: total("Rss"),
        drm: drm::memory_kib(files)
            .into_iter()
            .map(|(key, kib)| (key, Figure::new(Some(kib as f64), "KiB")))
            .collect(),
    }
}

/// Makes what the cases need before any runs: Yuna built, KWin's output configuration at
/// 240 Hz, the clips. Each failure is written into the run's notes rather than stopping the
/// other compositors.
pub fn prepare(
    bench: &mut Bench,
    compositors: &[&str],
    scenarios: &[&str],
    notes: &mut Vec<String>,
) {
    if compositors.contains(&"yuna") {
        match build_yuna() {
            Ok((program, version)) => {
                bench.prepared.yuna = Some(program);
                bench.prepared.yuna_version = Some(version);
            }
            Err(error) => notes.push(format!("yuna could not be built: {error}")),
        }
    }
    if compositors.contains(&"kwin") {
        match prepare_kwin(bench) {
            Ok(config) => bench.prepared.kwin_output_config = Some(config),
            Err(error) => notes.push(format!("KWin's output configuration: {error}")),
        }
    }
    for clip in [FILM, GAME] {
        if scenarios.contains(&clip.name)
            && let Err(error) = make_clip(bench, clip)
        {
            notes.push(format!("the {} clip: {error}", clip.name));
        }
    }
}

/// The workspace's root.
pub fn workspace() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&root).unwrap_or(root)
}

/// The commit of the workspace's `HEAD`, with `-dirty` if any of `paths` differ from it.
pub fn commit(paths: &[&str]) -> Option<String> {
    let git = |arguments: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(workspace())
            .args(arguments)
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let head = git(&["rev-parse", "HEAD"])?;
    let mut status = vec!["status", "--porcelain", "--"];
    status.extend(paths);
    let dirty = git(&status).is_none_or(|changes| !changes.is_empty());
    Some(if dirty { format!("{head}-dirty") } else { head })
}

/// Builds Yuna for release from the workspace; gives its program and its commit.
fn build_yuna() -> Result<(PathBuf, String), String> {
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--locked",
            "-p",
            "yuna",
            "--bin",
            "yuna",
        ])
        .current_dir(workspace())
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err(format!("cargo build ended with {status}"));
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace().join("target"));
    let program = fs::canonicalize(target.join("release/yuna"))
        .map_err(|e| format!("target/release/yuna: {e}"))?;
    let version = commit(&["crates/yuna", "crates/yuna-layout", "crates/yuna-motion"])
        .ok_or("the workspace's commit cannot be read")?;
    Ok((program, version))
}

/// Starts KWin once so that it writes its own output configuration, and gives it back edited to
/// 240 Hz.
fn prepare_kwin(bench: &mut Bench) -> Result<String, String> {
    let recipe = recipes::recipe("kwin").ok_or("no kwin recipe")?;
    let (mut running, _, _) =
        start_compositor(bench, recipe, Configuration::AsShipped, "kwin-prepare")?;
    let checked = refuse_if_broken(bench, &running, &[]);
    let written = running.folders.config().join("kwinoutputconfig.json");
    let deadline = Instant::now() + Duration::from_secs(15);
    while checked.is_ok() && !written.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    // Written at once with the output's first configuration, but give it a moment to end.
    std::thread::sleep(Duration::from_millis(500));
    stop_compositor(bench, &mut running);
    if let Err(Stop::Refused(lines)) = checked {
        return Err(format!("refused: {}", lines.join("; ")));
    }
    let text = fs::read_to_string(&written)
        .map_err(|e| format!("KWin wrote no {}: {e}", written.display()))?;
    recipes::kwin_output_at_240(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_recorded_as_one_and_a_failure_as_one() {
        let mut run = CaseRun::default();
        let refused: Result<(), Stop> = Err(Stop::Refused(vec![
            "during the cost window, process 10 held /dev/dri/card1 open".to_owned(),
        ]));
        assert_eq!(record_outcome(&mut run, refused), None);
        assert_eq!(run.refused.len(), 1);
        assert_eq!(run.error, None);
        let mut run = CaseRun::default();
        let failed: Result<(), Stop> = Err(Stop::from("mpv ended".to_owned()));
        assert_eq!(record_outcome(&mut run, failed), None);
        assert!(run.refused.is_empty());
        assert_eq!(run.error.as_deref(), Some("mpv ended"));
        assert_eq!(record_outcome(&mut run, Ok::<u8, Stop>(7)), Some(7));
    }

    fn phase_of(core_cpu_ns: f64) -> Phase {
        Phase {
            name: "cost".to_owned(),
            seconds: Figure::new(Some(10.0), "s"),
            core: record::Usage {
                cpu: Figure::new(Some(core_cpu_ns), "ns"),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn a_cost_a_frame_comes_only_from_an_exact_count_of_the_same_window() {
        let exact = Count {
            frames: Some(2_400.0),
            by: "mpv".to_owned(),
            exact: true,
        };
        let mut phase = phase_of(480_000_000.0);
        derive(&mut phase, Some(exact));
        assert_eq!(phase.figures["core_cpu_per_frame"].value, Some(200.0));
        assert_eq!(phase.figures["core_cpu"].value, Some(48.0));
        assert!(phase.lower_bounds.is_empty());
        // Counted by capture: frames a lower bound, and no cost a frame.
        let captured = Count {
            frames: Some(2_220.0),
            by: "capture".to_owned(),
            exact: false,
        };
        let mut phase = phase_of(480_000_000.0);
        derive(&mut phase, Some(captured));
        assert!(!phase.figures.contains_key("core_cpu_per_frame"));
        assert!(phase.lower_bounds.contains(&"frames".to_owned()));
        assert_eq!(phase.figures["core_cpu"].value, Some(48.0));
        // Not counted in this window: no frames, and no cost a frame.
        let mut phase = phase_of(480_000_000.0);
        derive(&mut phase, None);
        assert_eq!(phase.figures["frames"].value, None);
        assert!(!phase.figures.contains_key("core_cpu_per_frame"));
        assert_eq!(phase.frames_counted_by, None);
    }

    #[test]
    fn the_achieved_rate_is_the_exact_count_then_the_presented_then_the_captured() {
        let with = |figures: &[(&str, f64)], lower_bounds: &[&str]| Phase {
            name: "cost".to_owned(),
            seconds: Figure::new(Some(10.0), "s"),
            figures: figures
                .iter()
                .map(|(key, value)| ((*key).to_owned(), Figure::new(Some(*value), "per_s")))
                .collect(),
            lower_bounds: lower_bounds.iter().map(|key| (*key).to_owned()).collect(),
            ..Default::default()
        };
        // An exact count of its own: KWin at 240 Hz.
        let mut cost = with(&[("frames_per_s", 240.0)], &[]);
        achieved_rate(&mut cost, None);
        assert_eq!(cost.figures["achieved_frames_per_s"].value, Some(240.0));
        assert!(cost.lower_bounds.is_empty());
        // pacing on wlroots: the frames presented, not the capture's lower bound.
        let captured = with(&[("frames_per_s", 231.0)], &["frames_per_s"]);
        let mut cost = with(&[("presented", 2_495.0)], &[]);
        achieved_rate(&mut cost, Some(&captured));
        assert_eq!(cost.figures["achieved_frames_per_s"].value, Some(249.5));
        // rest on wlroots: the frames window's capture, a lower bound.
        let mut cost = with(&[], &[]);
        achieved_rate(&mut cost, Some(&captured));
        assert_eq!(cost.figures["achieved_frames_per_s"].value, Some(231.0));
        assert_eq!(cost.lower_bounds, ["achieved_frames_per_s"]);
        // Mutter at rest: no count at all, no rate.
        let mut cost = with(&[], &[]);
        achieved_rate(&mut cost, None);
        assert!(!cost.figures.contains_key("achieved_frames_per_s"));
    }

    #[test]
    fn each_tool_reaches_the_compositor_of_the_case_alone() {
        let root = crate::testing::folder("tools");
        let folders = Folders::create(&root).unwrap();
        for socket in [
            "wayland-1",
            "wayland-1.lock",
            "mango-42.sock",
            "umbriel-wayland-1.sock",
        ] {
            fs::write(folders.runtime().join(socket), "").unwrap();
        }
        let shell = crate::testing::shell();
        let link = PathBuf::from("/tmp/yb-1-0");
        let reach = Reach {
            folders: &folders,
            sway_socket: None,
            runtime_link: Some(&link),
        };
        let line = |action: Action| {
            let command = tool_at(&shell, &reach, action).unwrap();
            let words: Vec<String> = std::iter::once(command.get_program())
                .chain(command.get_args())
                .map(|word| word.to_string_lossy().into_owned())
                .collect();
            let variables: Vec<(String, String)> = command
                .get_envs()
                .map(|(key, value)| {
                    (
                        key.to_string_lossy().into_owned(),
                        value
                            .map(|v| v.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    )
                })
                .collect();
            (words.join(" "), variables)
        };
        let runtime = folders.runtime().to_string_lossy().into_owned();
        assert_eq!(
            line(Action::Mmsg(&["dispatch", "view,2,0"])),
            (
                "/nix/store/aaaa-mango-0.18.4/bin/mmsg dispatch view,2,0".to_owned(),
                vec![(
                    "MANGO_INSTANCE_SIGNATURE".to_owned(),
                    format!("{runtime}/mango-42.sock")
                )]
            )
        );
        assert_eq!(
            line(Action::Umbriel(&["msg", "workspace-switch", "2"])).1,
            [(
                "UMBRIEL_SOCKET".to_owned(),
                format!("{runtime}/umbriel-wayland-1.sock")
            )]
        );
        assert_eq!(
            line(Action::Hyprctl(&[
                "dispatch",
                "hl.dsp.focus({ workspace = 2 })"
            ])),
            (
                "/nix/store/aaaa-hyprland-0.57.1/bin/hyprctl --instance 0 dispatch \
                 hl.dsp.focus({ workspace = 2 })"
                    .to_owned(),
                vec![("XDG_RUNTIME_DIR".to_owned(), "/tmp/yb-1-0".to_owned())]
            )
        );
        let (words, variables) = line(Action::WlrRandr(&["--json"]));
        assert_eq!(
            words,
            "/nix/store/aaaa-wlr-randr-0.5.0/bin/wlr-randr --json"
        );
        assert_eq!(
            variables,
            [("WAYLAND_DISPLAY".to_owned(), format!("{runtime}/wayland-1"))]
        );
        // Every tool starts from an empty environment: none of the bench's own reaches it.
        let command = tool_at(&shell, &reach, Action::WlrRandr(&[])).unwrap();
        assert!(format!("{command:?}").starts_with("env -i"), "{command:?}");
        // Keys are no tool's; sway's command needs its socket.
        assert!(tool_at(&shell, &reach, Action::Sway("exit")).is_err());
        assert!(tool_at(&shell, &reach, recipes::recipe("dwl").unwrap().actions[0]).is_err());
    }

    #[test]
    fn a_compositor_restarted_during_the_case_fails_it() {
        let hyprland = recipes::recipe("hyprland").unwrap();
        // As start-hyprland logs it when Hyprland ends badly, before it starts it in safe mode.
        let log = "[ERR] Hyprland exit not-cleanly, restarting\n";
        let error = restarted(hyprland, log).unwrap();
        assert!(
            error.starts_with("hyprland was restarted during the case"),
            "{error}"
        );
        assert_eq!(restarted(hyprland, "[LOG] Hyprland started\n"), None);
        // A compositor without such a line is never told restarted.
        assert_eq!(restarted(recipes::recipe("sway").unwrap(), log), None);
    }

    #[test]
    fn a_clip_that_did_not_play_in_its_warm_up_fails_the_case() {
        let warm_up = Duration::from_secs(3);
        assert!(played(Some(0.2), Some(3.3), warm_up).is_ok());
        // Stalled at its first frame, as on Mutter: 0.013 s, then 0.013 s.
        let stalled = played(Some(0.013), Some(0.013), warm_up).unwrap_err();
        assert!(stalled.contains("from 0.013 s to 0.013 s"), "{stalled}");
        assert!(played(None, Some(3.0), warm_up).is_err());
        assert!(played(Some(1.0), None, warm_up).is_err());
    }

    #[test]
    fn a_clip_outlasts_every_window_mpv_plays_it_in() {
        // 15 s to be fullscreen, 3 s of warm-up, two windows of 10 s, the counter's 2 s.
        assert_eq!(Clip::seconds(&Durations::STANDARD), 44);
        // Longer windows, to try the bench, make a longer clip.
        assert_eq!(Clip::seconds(&Durations::with_windows(25.0)), 77);
    }

    #[test]
    fn mpvs_frames_shown_are_its_moves_less_its_drops() {
        let reading = |position_s, dropped| MpvFrames {
            position_s: Some(position_s),
            dropped: Some(dropped),
            decoder_dropped: Some(0.0),
        };
        let shown = reading(13.0, 2.0).shown_since(&reading(3.0, 0.0), 240.0);
        assert_eq!(shown, Some(2_398.0));
        // Looped back, or a reading missing: unknown.
        assert_eq!(
            reading(1.0, 0.0).shown_since(&reading(3.0, 0.0), 240.0),
            None
        );
        assert_eq!(
            MpvFrames::default().shown_since(&reading(3.0, 0.0), 240.0),
            None
        );
    }
}
