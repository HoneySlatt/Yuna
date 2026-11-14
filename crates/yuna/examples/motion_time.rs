//! Measures what motion costs Yuna's frames, as
//! [ADR-0056](../../../docs/adr/0056-motions-budget-judged-on-the-gpu-against-a-still-baseline.md)
//! asks: the frames missed while the layout moves, against a still baseline on the same load, and
//! the heaviest frame's time.
//!
//! It runs Yuna in its own process, on the real clock, with animations on and their default
//! springs, on one output: by default 3840 × 2160 at 240 Hz and scale 1.5. Six `vkcube` connect to
//! it. Four are columns as wide as the output on the first workspace, so that each frame redraws
//! the whole output but the gaps around the window, and focusing the next column scrolls by a
//! whole output. One is on a second workspace, below the first, and one on the special
//! workspace. Then come two phases of `--frames` frames each, 10,000 by default:
//!
//! 1. **The still baseline**: the column on screen draws; the windows off screen are hidden, their
//!    frame callbacks once a second; no operation.
//! 2. **Motion**: the same, while a round of operations repeats, each given once the motion of the
//!    one before has ended, as `Compositor::windows` tells it (`motion_ends`). The round: focus
//!    right three times, then left three times; the focused column moved right, then back; a
//!    switch to the workspace below, then back; the special workspace shown, then hidden, its
//!    window fading. Last comes a retarget: focus right, and 30 ms into its motion, focus left.
//!
//! Each frame is read from Yuna's `frame drawn` log line and the line this program writes after
//! it with the windows the frame showed. A frame presented after the boundary it aimed at is
//! *missed* ([ADR-0031](../../../docs/adr/0031-frames-drawn-ahead-of-their-boundary.md)). For each
//! phase it prints the frames and those missed; a frame's time (`took_us`) at the median, the 99th
//! percentile and the longest; and Yuna's and its clients' CPU and GPU time per frame, read as
//! `frame_time` reads them
//! ([ADR-0030](../../../docs/adr/0030-a-frames-gpu-time-from-the-kernel.md)). For motion, it
//! also places each miss: in the first three frames after an operation (and of those, how many
//! as a window entered the output); later, as a window entered; or elsewhere. It names the
//! heaviest frame: its time, the operation before it and how many frames after it.
//!
//! Per phase it also prints the frames a second against the output's rate, and the frames drawn
//! at once (the output idle, so aimed at the first boundary in reach, with no lead) with how many
//! of them missed. In the baseline the clients set the pace: if they cannot keep the output's
//! rate, most frames are drawn at once and miss for a reason that motion does not share.
//!
//! Last comes ADR-0056's verdict: motion missed no more frames than the still baseline, over
//! 10,000 frames or more each. It names the mode it ran at, and is the ADR's only at the ADR's
//! mode, 3840 × 2160 at 240 Hz and scale 1.5, on `--gpu`; anywhere else it is recorded without a
//! pass mark. It gives no verdict when a phase has too few frames, or when the baseline ran below
//! 95 % of the output's rate: it would not be the output redrawn whole at its rate.
//!
//! Build it for release, then run it in the development shell, alone among measurements:
//!
//! ```sh
//! nix develop -c cargo build --release -p yuna --example motion_time
//! nix develop -c target/release/examples/motion_time --output 1920x1080@240,1
//! ```
//!
//! It needs no screen. On the owner's machine, agents run it inside `agent-headless`, which gives
//! it throwaway XDG folders and stops every process of its run, under the agents' lock of
//! exclusive runs (`run.sh --exclusive`), with no build, game or other measure meanwhile; `--gpu`
//! only with the owner's consent. The figures go to `docs/measurements.md` with the machine, its
//! load and the commit.
//!
//! Options: `--gpu` draws on the GPU's render node (never in the tests); `--client-vulkan
//! <icd.json>` gives `vkcube` that Vulkan driver rather than lavapipe; `--output
//! WIDTHxHEIGHT@RATE,SCALE`, a fixed rate; `--frames <n>`, each phase's frames;
//! `--reduced-motion`, animations under reduced motion, where nothing slides and fades are kept
//! ([ADR-0059](../../../docs/adr/0059-reduced-motion-nothing-slides-fades-stay.md)). The round
//! then has no retarget, since nothing is moving to turn back, and its load is layout jumps and
//! the special workspace's fades; a fixed pause of 250 ms separates its steps, which would
//! otherwise follow each other on every frame.
//!
//! Where it differs from `frame_time`, which measures the `yuna` program from outside: Yuna's
//! main thread here also runs the script and writes the log, and its CPU time counts in Yuna's.
//! The clients connect through `Compositor::connect_client`'s socket pairs, handed to `vkcube` as
//! `WAYLAND_SOCKET`, so that Yuna makes no socket file. Yuna here sees the caller's whole
//! environment. Mesa may keep a shader cache, in `XDG_CACHE_HOME` first: `agent-headless` makes
//! that a throwaway folder but keeps `HOME`, so outside it the cache may land in the caller's
//! own. It stops what it started when it ends, and keeps its folder of logs when it fails. If it
//! is killed outright, a `vkcube` may be left running: each should exit when its connection to
//! Yuna breaks, which was not checked.

mod common;

use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, ExitCode};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::{
    Clients, PATIENCE, Run, Usage, WARM_UP, field, percentile, print_usage, text_field,
    vkcube_command,
};
use smithay::reexports::rustix::io::{FdFlags, fcntl_setfd};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use yuna::yuna_layout::geometry::Rect;
use yuna::yuna_layout::{Direction, Layout, LayoutError, OutputId, WindowId};
use yuna::yuna_motion::Settings;
use yuna::{ClockMode, Compositor, Config, Device, OWNER_SEAT, OutputMode};

/// The output's mode without `--output`: Yuna's default, an agent's output, and the mode
/// ADR-0056 judges at.
const DEFAULT_OUTPUT: &str = "3840x2160@240,1.5";
/// Each phase's frames without `--frames`, and the fewest ADR-0056 judges on.
const MIN_FRAMES: usize = 10_000;
/// The columns of the first workspace, each as wide as the output.
const COLUMNS: usize = 4;
/// The workspace below the first, with a window of its own.
const SECOND_WORKSPACE: &str = "second";
/// The special workspace, with a window of its own.
const SPECIAL_WORKSPACE: &str = "special";
/// The frames after an operation counted as its first ones, where a miss is placed apart.
const FIRST_FRAMES: usize = 3;
/// How long after the operation before a retarget is given, its motion still running.
const RETARGET_AFTER: Duration = Duration::from_millis(30);
/// Under reduced motion, how long a step waits after the one before: with nothing moving, the
/// steps would follow each other on every frame.
const REDUCED_MOTION_PAUSE: Duration = Duration::from_millis(250);
/// The least share of the output's rate the baseline must keep for a verdict.
const BASELINE_MIN_SHARE: f64 = 0.95;
/// The longest the loop waits for Yuna before it looks again at the clients and the script.
const MAX_WAIT: Duration = Duration::from_millis(100);
/// How often a phase checks that every `vkcube` still runs.
const CLIENTS_CHECK: Duration = Duration::from_secs(1);
/// The target of the lines this program writes in the log, beside Yuna's.
const TARGET: &str = "motion_time";

/// What the command line asks for.
struct Options {
    gpu: bool,
    client_vulkan: Option<String>,
    /// The output's mode as given, for the report, and as read.
    output: String,
    mode: OutputMode,
    frames: u64,
    reduced_motion: bool,
}

fn main() -> ExitCode {
    let options = match read_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(reason) => {
            eprintln!("motion_time: {reason}");
            return ExitCode::from(2);
        }
    };
    let run = match Run::new("yuna-motion-time") {
        Ok(run) => run,
        Err(reason) => {
            eprintln!("motion_time: {reason}");
            return ExitCode::FAILURE;
        }
    };
    match measure(&run, &options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("motion_time: {reason}; logs kept in {}", run.keep());
            ExitCode::FAILURE
        }
    }
}

fn read_options(arguments: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut gpu = false;
    let mut client_vulkan = None;
    let mut output = DEFAULT_OUTPUT.to_owned();
    let mut frames = MIN_FRAMES as u64;
    let mut reduced_motion = false;
    let mut arguments = arguments;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        match argument.as_str() {
            "--gpu" => gpu = true,
            "--client-vulkan" => client_vulkan = Some(value()?),
            "--output" => output = value()?,
            "--frames" => {
                frames = value()?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or("--frames takes a whole number above 0")?;
            }
            "--reduced-motion" => reduced_motion = true,
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }
    Ok(Options {
        gpu,
        client_vulkan,
        mode: read_mode(&output)?,
        output,
        frames,
        reduced_motion,
    })
}

/// Reads `WIDTHxHEIGHT@RATE,SCALE`, a fixed rate: the missed frames ADR-0056 counts are those of
/// a fixed rate, so VRR and HDR are refused.
fn read_mode(text: &str) -> Result<OutputMode, String> {
    let bad = || {
        format!(
            "--output {text:?}: expected WIDTHxHEIGHT@RATE,SCALE at a fixed rate, such as \
             1920x1080@240,1"
        )
    };
    let (size, rest) = text.split_once('@').ok_or_else(bad)?;
    let (width, height) = size.split_once('x').ok_or_else(bad)?;
    let (rate, scale) = rest.split_once(',').ok_or_else(bad)?;
    let (Ok(width), Ok(height), Ok(rate), Ok(scale)) =
        (width.parse(), height.parse(), rate.parse(), scale.parse())
    else {
        return Err(bad());
    };
    OutputMode::new(width, height, rate, scale)
        .map_err(|reason| format!("--output {text}: {reason}"))
}

fn measure(run: &Run, options: &Options) -> Result<(), String> {
    let log = run.folder().join("yuna.log");
    start_logging(&log)?;
    let mut driver = Driver::start(options)?;
    // Declared after the driver, so dropped before it: the clients are killed first.
    let mut clients = Clients(Vec::new());
    set_up(&mut driver, run, options, &mut clients)?;
    let warmed = Instant::now() + WARM_UP;
    turn_until(&mut driver, &mut clients, "the warm-up", |_| {
        Instant::now() >= warmed
    })?;
    let baseline = run_phase(
        &mut driver,
        &mut clients,
        Phase::Baseline,
        options.frames,
        None,
    )?;
    let mut script = Script::new(options.reduced_motion);
    let motion = run_phase(
        &mut driver,
        &mut clients,
        Phase::Motion,
        options.frames,
        Some(&mut script),
    )?;
    check_clients(&mut clients)?;
    let text = fs::read_to_string(&log).map_err(|e| format!("the log: {e}"))?;
    let phases = read_phases(&text)?;
    let warnings: Vec<&str> = text
        .lines()
        .filter(|line| line.contains(" WARN ") || line.contains(" ERROR "))
        .take(5)
        .collect();
    report(options, &phases, &baseline, &motion, &warnings);
    Ok(())
}

/// Logs Yuna's lines and this program's to `path`, at `debug` as `frame_time` runs Yuna: a line
/// per frame drawn.
fn start_logging(path: &Path) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("the log: {e}"))?;
    tracing::subscriber::set_global_default(log_subscriber(Mutex::new(file)))
        .map_err(|e| format!("the log: {e}"))
}

/// The format of the log, the `yuna` program's at `debug`: what [`read_phases`] reads.
fn log_subscriber<W>(writer: W) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::DEBUG)
        .with_writer(writer)
        .finish()
}

// ----- Driving Yuna -----

/// Yuna in this process, and what the loop knows of it.
struct Driver {
    compositor: Compositor,
    output: OutputId,
    /// The output's place, in logical coordinates.
    output_rect: Rect,
    /// The frames drawn, as Yuna counted them after the last turn.
    frames_drawn: u64,
}

impl Driver {
    /// Starts Yuna on the real clock with one output, animations on, and no socket of its own.
    fn start(options: &Options) -> Result<Driver, String> {
        let compositor = Compositor::new(Config {
            outputs: vec![options.mode],
            device: if options.gpu {
                Device::Gpu
            } else {
                Device::Software
            },
            clock: ClockMode::Real,
            listen: false,
            announce_owner_seat: true,
            animations: Settings {
                off: false,
                reduced_motion: options.reduced_motion,
                ..Settings::default()
            },
        })
        .map_err(|e| format!("Yuna does not start: {e}"))?;
        let output = compositor
            .output_descriptions()
            .into_iter()
            .next()
            .ok_or("Yuna has no output")?;
        let frames_drawn = compositor.counters().frames_drawn;
        Ok(Driver {
            compositor,
            output: output.id,
            output_rect: output.rect,
            frames_drawn,
        })
    }

    /// Runs one round of Yuna's loop, waiting up to `timeout` for something to do. After a round
    /// that drew a frame, logs the windows it showed: the next line after its `frame drawn`.
    fn turn(&mut self, timeout: Duration) -> Result<(), String> {
        self.compositor
            .dispatch(Some(timeout))
            .map_err(|e| format!("Yuna stopped: {e}"))?;
        let frames_drawn = self.compositor.counters().frames_drawn;
        if frames_drawn != self.frames_drawn {
            self.frames_drawn = frames_drawn;
            let shown: Vec<WindowId> = self
                .compositor
                .drawn(self.output)
                .iter()
                .filter(|drawn| drawn.rect.overlaps(&self.output_rect))
                .map(|drawn| drawn.window)
                .collect();
            log_frame_shown(&shown);
        }
        Ok(())
    }

    /// Starts `vkcube` number `n`, connected to Yuna through a socket pair.
    fn start_vkcube(
        &mut self,
        run: &Run,
        n: u32,
        client_vulkan: Option<&str>,
    ) -> Result<Child, String> {
        let socket = self
            .compositor
            .connect_client()
            .map_err(|e| format!("a client cannot connect: {e}"))?;
        // `vkcube` alone inherits the socket: its close-on-exec flag is cleared just before the
        // spawn, and this process's copy closed just after, before any other spawn.
        fcntl_setfd(&socket, FdFlags::empty())
            .map_err(|e| format!("the client's socket cannot be inherited: {e}"))?;
        let child = vkcube_command(run, n, client_vulkan)?
            .env("WAYLAND_SOCKET", socket.as_raw_fd().to_string())
            .spawn()
            .map_err(|e| format!("vkcube does not start: {e}"));
        drop(socket);
        child
    }

    /// Changes the layout, as a key binding would: what moves is drawn moving.
    fn change(
        &mut self,
        what: &str,
        change: impl FnOnce(&mut Layout) -> Result<(), LayoutError>,
    ) -> Result<(), String> {
        self.compositor
            .change_layout(change)
            .map_err(|e| format!("{what}: the layout refused: {e}"))
    }

    /// Gives `operation`, and logs it first: the frames after its line are its motion's.
    fn apply(&mut self, operation: Operation) -> Result<(), String> {
        log_operation(operation);
        self.change(operation.name(), |layout| operation.apply(layout))
    }

    /// When the output's motion ends, on the clock's scale; none at rest.
    fn motion_end(&self) -> Option<Duration> {
        self.compositor
            .windows()
            .iter()
            .filter_map(|window| window.motion_ends)
            .max()
    }

    fn window_ids(&self) -> Vec<WindowId> {
        self.compositor
            .windows()
            .iter()
            .map(|window| window.id)
            .collect()
    }
}

/// Turns Yuna's loop until `done`; fails after [`PATIENCE`], or if a `vkcube` ended.
fn turn_until(
    driver: &mut Driver,
    clients: &mut Clients,
    what: &str,
    mut done: impl FnMut(&Driver) -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    while !done(driver) {
        if Instant::now() > deadline {
            return Err(format!("{what}: not done in {} s", PATIENCE.as_secs()));
        }
        driver.turn(MAX_WAIT)?;
        check_clients(clients)?;
    }
    Ok(())
}

fn check_clients(clients: &mut Clients) -> Result<(), String> {
    for client in &mut clients.0 {
        if let Ok(Some(status)) = client.try_wait() {
            return Err(format!("a vkcube ended: {status}"));
        }
    }
    Ok(())
}

/// Opens the six windows, sets the layout up, and waits until it rests.
fn set_up(
    driver: &mut Driver,
    run: &Run,
    options: &Options,
    clients: &mut Clients,
) -> Result<(), String> {
    let vulkan = options.client_vulkan.as_deref();
    let output = driver.output;
    for n in 0..COLUMNS as u32 {
        clients.0.push(driver.start_vkcube(run, n, vulkan)?);
    }
    wait_for_windows(driver, clients, COLUMNS)?;
    driver.change("the second workspace", |layout| {
        show_second_workspace(layout, output)
    })?;
    clients
        .0
        .push(driver.start_vkcube(run, COLUMNS as u32, vulkan)?);
    wait_for_windows(driver, clients, COLUMNS + 1)?;
    driver.change("the special workspace", show_special_workspace)?;
    clients
        .0
        .push(driver.start_vkcube(run, COLUMNS as u32 + 1, vulkan)?);
    wait_for_windows(driver, clients, COLUMNS + 2)?;
    let windows = driver.window_ids();
    driver.change("the set-up's end", |layout| {
        finish_set_up(layout, output, &windows)
    })?;
    let changed = driver.frames_drawn;
    turn_until(driver, clients, "the set-up's motion", |driver| {
        driver.frames_drawn > changed && driver.motion_end().is_none()
    })?;
    check_set_up(driver.compositor.layout(), output, &windows)
}

fn wait_for_windows(
    driver: &mut Driver,
    clients: &mut Clients,
    count: usize,
) -> Result<(), String> {
    turn_until(driver, clients, "the windows to open", |driver| {
        driver.compositor.windows().len() >= count
    })
}

/// Before the second workspace's client starts: that workspace, below the first, shown, so that
/// the client's window opens there.
fn show_second_workspace(layout: &mut Layout, output: OutputId) -> Result<(), LayoutError> {
    layout.add_workspace(OWNER_SEAT, SECOND_WORKSPACE, Some(output))?;
    layout.focus_workspace(OWNER_SEAT, SECOND_WORKSPACE)
}

/// Before the special workspace's client starts: that workspace shown, its window opens there.
fn show_special_workspace(layout: &mut Layout) -> Result<(), LayoutError> {
    layout.toggle_special(OWNER_SEAT, SPECIAL_WORKSPACE)
}

/// Once every window has opened: each one's column as wide as the output, the special workspace
/// hidden, the first workspace shown with its first column focused, where the round starts.
fn finish_set_up(
    layout: &mut Layout,
    output: OutputId,
    windows: &[WindowId],
) -> Result<(), LayoutError> {
    for &window in windows {
        layout.set_full_width(window, true)?;
    }
    if layout.shown_special(output)?.is_some() {
        layout.toggle_special(OWNER_SEAT, SPECIAL_WORKSPACE)?;
    }
    let first_column = layout
        .workspaces(output)?
        .first()
        .and_then(|workspace| workspace.windows.first().copied())
        .ok_or(LayoutError::Refused("the first workspace has no window"))?;
    layout.focus_window(first_column)
}

/// Checks that the layout is the one the round needs: the first workspace shown, with
/// [`COLUMNS`] columns, its first focused; the second below it with one window; one window on the
/// special workspace, hidden; every window tiled in a column as wide as the output.
fn check_set_up(layout: &Layout, output: OutputId, windows: &[WindowId]) -> Result<(), String> {
    let refused = |e: LayoutError| e.to_string();
    let workspaces = layout.workspaces(output).map_err(refused)?;
    let [first, second] = workspaces.as_slice() else {
        return Err(format!("not two workspaces: {workspaces:?}"));
    };
    let columns = layout
        .arrange(output)
        .map_err(refused)?
        .iter()
        .filter_map(|placed| placed.column)
        .collect::<std::collections::HashSet<_>>()
        .len();
    if !first.shown || first.windows.len() != COLUMNS || columns != COLUMNS {
        return Err(format!(
            "the first workspace is not shown with {COLUMNS} columns: {first:?}"
        ));
    }
    if second.name.as_deref() != Some(SECOND_WORKSPACE) || second.windows.len() != 1 {
        return Err(format!(
            "the second workspace has not one window: {second:?}"
        ));
    }
    if layout.focused(OWNER_SEAT) != first.windows.first().copied() {
        return Err("the first column is not focused".to_owned());
    }
    if layout.shown_special(output).map_err(refused)?.is_some() {
        return Err("the special workspace is shown".to_owned());
    }
    let infos: Vec<_> = windows
        .iter()
        .filter_map(|&window| layout.window(window))
        .collect();
    let special = infos.iter().filter(|info| info.special).count();
    if windows.len() != COLUMNS + 2 || infos.len() != windows.len() || special != 1 {
        return Err(format!(
            "not {} windows with one special: {infos:?}",
            COLUMNS + 2
        ));
    }
    if !infos.iter().all(|info| info.full_width && !info.floating) {
        return Err(format!(
            "a window is not a column of the output's width: {infos:?}"
        ));
    }
    Ok(())
}

/// The two phases measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The output redrawn by its client, no operation.
    Baseline,
    /// The same, while the round of operations repeats.
    Motion,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Baseline => "baseline",
            Phase::Motion => "motion",
        }
    }

    fn named(name: &str) -> Option<Phase> {
        [Phase::Baseline, Phase::Motion]
            .into_iter()
            .find(|phase| phase.name() == name)
    }
}

/// What a phase measured beside its frames, which are read from the log.
struct Measured {
    before: Usage,
    after: Usage,
    /// The frames drawn between the two samples: the phase's, and the one after the first sample.
    frames: u64,
    /// The operations the script gave.
    operations: usize,
}

/// Runs a phase of `frames` frames, with the round of operations if `script` is given, between
/// two samples of what Yuna and its clients used.
fn run_phase(
    driver: &mut Driver,
    clients: &mut Clients,
    phase: Phase,
    frames: u64,
    mut script: Option<&mut Script>,
) -> Result<Measured, String> {
    let pid = std::process::id();
    let before = Usage::take(pid, &clients.0)?;
    let frames_before = driver.frames_drawn;
    // Reading `/proc` held the loop a moment: the frame after it is left out of the phase.
    turn_until(driver, clients, "a frame", |driver| {
        driver.frames_drawn > frames_before
    })?;
    log_phase_started(phase);
    let start = driver.frames_drawn;
    let mut operations = 0;
    let (mut last_frames, mut last_frame_at) = (start, Instant::now());
    let mut clients_checked = Instant::now();
    while driver.frames_drawn - start < frames {
        let mut wait = MAX_WAIT;
        if let Some(script) = script.as_deref_mut() {
            let now = driver.compositor.now();
            if let Some(operation) = script.due(now, driver.frames_drawn, || driver.motion_end()) {
                driver.apply(operation)?;
                operations += 1;
            }
            wait = script.wait(driver.compositor.now());
        }
        driver.turn(wait)?;
        if driver.frames_drawn != last_frames {
            (last_frames, last_frame_at) = (driver.frames_drawn, Instant::now());
        } else if last_frame_at.elapsed() > PATIENCE {
            return Err(format!(
                "{} phase: no frame drawn in {} s",
                phase.name(),
                PATIENCE.as_secs()
            ));
        }
        if clients_checked.elapsed() >= CLIENTS_CHECK {
            check_clients(clients)?;
            clients_checked = Instant::now();
        }
    }
    log_phase_ended();
    let after = Usage::take(pid, &clients.0)?;
    Ok(Measured {
        before,
        after,
        frames: driver.frames_drawn - frames_before,
        operations,
    })
}

// ----- The round of operations -----

/// What the round does to the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    FocusRight,
    FocusLeft,
    MoveColumnRight,
    MoveColumnLeft,
    WorkspaceDown,
    WorkspaceUp,
    SpecialIn,
    SpecialOut,
}

impl Operation {
    /// Its name in the log and the report.
    fn name(self) -> &'static str {
        match self {
            Operation::FocusRight => "focus-right",
            Operation::FocusLeft => "focus-left",
            Operation::MoveColumnRight => "move-column-right",
            Operation::MoveColumnLeft => "move-column-left",
            Operation::WorkspaceDown => "workspace-down",
            Operation::WorkspaceUp => "workspace-up",
            Operation::SpecialIn => "special-in",
            Operation::SpecialOut => "special-out",
        }
    }

    fn apply(self, layout: &mut Layout) -> Result<(), LayoutError> {
        match self {
            Operation::FocusRight => layout.focus(OWNER_SEAT, Direction::Right),
            Operation::FocusLeft => layout.focus(OWNER_SEAT, Direction::Left),
            Operation::MoveColumnRight => layout.move_focused(OWNER_SEAT, Direction::Right),
            Operation::MoveColumnLeft => layout.move_focused(OWNER_SEAT, Direction::Left),
            Operation::WorkspaceDown => {
                layout.focus_workspace_relative(OWNER_SEAT, Direction::Down)
            }
            Operation::WorkspaceUp => layout.focus_workspace_relative(OWNER_SEAT, Direction::Up),
            Operation::SpecialIn | Operation::SpecialOut => {
                layout.toggle_special(OWNER_SEAT, SPECIAL_WORKSPACE)
            }
        }
    }
}

/// When a step of the round is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// Once the operation before was arranged, a frame drawn since, and its motion ended.
    AtRest,
    /// This long after the operation before was given, whether its motion ended or not.
    After(Duration),
}

#[derive(Debug, Clone, Copy)]
struct Step {
    operation: Operation,
    start: Start,
}

const fn at_rest(operation: Operation) -> Step {
    Step {
        operation,
        start: Start::AtRest,
    }
}

/// The round, which leaves the layout as it found it: the first workspace shown, its first
/// column focused, the special workspace hidden.
const ROUND: [Step; 14] = [
    at_rest(Operation::FocusRight),
    at_rest(Operation::FocusRight),
    at_rest(Operation::FocusRight),
    at_rest(Operation::FocusLeft),
    at_rest(Operation::FocusLeft),
    at_rest(Operation::FocusLeft),
    at_rest(Operation::MoveColumnRight),
    at_rest(Operation::MoveColumnLeft),
    at_rest(Operation::WorkspaceDown),
    at_rest(Operation::WorkspaceUp),
    at_rest(Operation::SpecialIn),
    at_rest(Operation::SpecialOut),
    at_rest(Operation::FocusRight),
    // The retarget: the scroll turns back mid-way, keeping its velocity.
    Step {
        operation: Operation::FocusLeft,
        start: Start::After(RETARGET_AFTER),
    },
];

/// Where the round is: which step comes next, and what the last one is waiting for.
#[derive(Debug, Default)]
struct Script {
    /// The operations given so far; the next is `ROUND[given % ROUND.len()]`.
    given: usize,
    last: Option<Given>,
    /// How long each step waits at least after the one before; none unless motion is reduced.
    pause: Duration,
}

/// The operation given last.
#[derive(Debug)]
struct Given {
    /// When it was given, on the clock's scale.
    at: Duration,
    /// The frames drawn when it was given: the next one arranges it.
    frames_drawn: u64,
    /// Before then, its motion runs, or the pause runs: the end it last announced.
    rest_not_before: Duration,
}

impl Script {
    /// A round that, under `reduced_motion`, pauses [`REDUCED_MOTION_PAUSE`] between steps.
    fn new(reduced_motion: bool) -> Script {
        Script {
            pause: if reduced_motion {
                REDUCED_MOTION_PAUSE
            } else {
                Duration::ZERO
            },
            ..Script::default()
        }
    }

    /// The next operation, if it is due at `now` with `frames_drawn` frames drawn; it then counts
    /// as given. `motion_end` says when the output's motion ends, none at rest; it is asked only
    /// once the operation before was arranged and the end it announced has passed.
    fn due(
        &mut self,
        now: Duration,
        frames_drawn: u64,
        motion_end: impl FnOnce() -> Option<Duration>,
    ) -> Option<Operation> {
        let step = ROUND[self.given % ROUND.len()];
        let due = match (&mut self.last, step.start) {
            (None, _) => true,
            // A frame must have drawn the step before, or both changes land in one layout pass and
            // cancel out.
            (Some(last), Start::After(delay)) => {
                now >= last.at + delay && frames_drawn > last.frames_drawn
            }
            (Some(last), Start::AtRest) => {
                if frames_drawn <= last.frames_drawn || now < last.rest_not_before {
                    false
                } else if let Some(end) = motion_end() {
                    last.rest_not_before = end;
                    false
                } else {
                    true
                }
            }
        };
        if !due {
            return None;
        }
        self.given += 1;
        self.last = Some(Given {
            at: now,
            frames_drawn,
            rest_not_before: now + self.pause,
        });
        Some(step.operation)
    }

    /// How long the loop may wait at `now` before the script must look again: until the time the
    /// next step waits for, or [`MAX_WAIT`] while it waits for frames, which wake the loop.
    fn wait(&self, now: Duration) -> Duration {
        let step = ROUND[self.given % ROUND.len()];
        let until = match (&self.last, step.start) {
            (None, _) => return Duration::ZERO,
            (Some(last), Start::After(delay)) if last.at + delay > now => last.at + delay,
            // Past its delay, it waits for a frame, which wakes the loop.
            (Some(_), Start::After(_)) => return MAX_WAIT,
            (Some(last), Start::AtRest) if last.rest_not_before > now => last.rest_not_before,
            (Some(_), Start::AtRest) => return MAX_WAIT,
        };
        until.saturating_sub(now).min(MAX_WAIT)
    }
}

// ----- The log -----

fn log_phase_started(phase: Phase) {
    tracing::info!(target: TARGET, phase = %phase.name(), "phase started");
}

fn log_phase_ended() {
    tracing::info!(target: TARGET, "phase ended");
}

fn log_operation(operation: Operation) {
    tracing::info!(target: TARGET, operation = %operation.name(), "operation started");
}

/// The windows on the output in the frame just drawn, by id, joined by commas.
fn log_frame_shown(windows: &[WindowId]) {
    let ids: Vec<String> = windows.iter().map(|window| window.0.to_string()).collect();
    tracing::debug!(target: TARGET, windows = %ids.join(","), "frame shown");
}

/// A frame drawn in a phase, as the log tells it.
#[derive(Debug, Clone, PartialEq)]
struct Frame {
    /// How long Yuna took to draw it, in microseconds (`took_us`).
    took_us: u64,
    /// Whether it was presented after the boundary it aimed at.
    missed: bool,
    /// Whether it was drawn at once: the output idle, no lead.
    at_once: bool,
    /// The operation given last in its phase, and this frame's number since: 1 for the first
    /// frame drawn after it, the one that arranged it.
    operation: Option<(String, usize)>,
    /// Whether a window is on the output in this frame that was not in the frame before.
    entering: bool,
}

/// Each phase's frames, in order.
#[derive(Debug, Default, PartialEq)]
struct Phases {
    baseline: Vec<Frame>,
    motion: Vec<Frame>,
}

impl Phases {
    fn of(&mut self, phase: Phase) -> &mut Vec<Frame> {
        match phase {
            Phase::Baseline => &mut self.baseline,
            Phase::Motion => &mut self.motion,
        }
    }
}

/// Reads each phase's frames from the log: Yuna's `frame drawn` lines, and this program's lines
/// around them. Refuses a line of a phase it cannot read.
fn read_phases(log: &str) -> Result<Phases, String> {
    let frame_shown = format!("{TARGET}: frame shown");
    let operation_started = format!("{TARGET}: operation started");
    let phase_started = format!("{TARGET}: phase started");
    let phase_ended = format!("{TARGET}: phase ended");
    let mut phases = Phases::default();
    let mut phase = None;
    let mut operation: Option<(String, usize)> = None;
    let mut shown_before: Option<Vec<u64>> = None;
    // The phase whose last frame waits for the line of the windows it showed.
    let mut awaiting = None;
    for line in log.lines() {
        if line.contains("frame drawn") {
            awaiting = None;
            let Some(current) = phase else {
                continue;
            };
            let number = |key| field(line, key).ok_or_else(|| format!("no {key} in {line:?}"));
            let (took_us, boundary, presented) = (
                number("took_us")?,
                number("boundary")?,
                number("presented")?,
            );
            if let Some((_, count)) = &mut operation {
                *count += 1;
            }
            phases.of(current).push(Frame {
                took_us,
                missed: presented > boundary,
                at_once: line.contains("at_once=true"),
                operation: operation.clone(),
                entering: false,
            });
            awaiting = Some(current);
        } else if line.contains(&frame_shown) {
            let shown = shown_windows(line)?;
            if let Some(current) = awaiting.take()
                && let Some(frame) = phases.of(current).last_mut()
            {
                frame.entering = shown_before
                    .as_ref()
                    .is_some_and(|before| shown.iter().any(|window| !before.contains(window)));
            }
            shown_before = Some(shown);
        } else if line.contains(&operation_started) {
            let name =
                text_field(line, "operation").ok_or_else(|| format!("no operation in {line:?}"))?;
            operation = Some((name.to_owned(), 0));
        } else if line.contains(&phase_started) {
            let name = text_field(line, "phase").unwrap_or_default();
            phase = Some(Phase::named(name).ok_or_else(|| format!("no phase in {line:?}"))?);
            operation = None;
        } else if line.contains(&phase_ended) {
            phase = None;
            operation = None;
        }
    }
    Ok(phases)
}

/// The windows a `frame shown` line lists.
fn shown_windows(line: &str) -> Result<Vec<u64>, String> {
    let list = text_field(line, "windows").ok_or_else(|| format!("no windows in {line:?}"))?;
    list.split(',')
        .filter(|id| !id.is_empty())
        .map(|id| {
            id.parse()
                .map_err(|_| format!("{id:?} is not a window in {line:?}"))
        })
        .collect()
}

// ----- The figures -----

/// A phase's frames in figures.
#[derive(Debug, PartialEq)]
struct Summary {
    frames: usize,
    missed: usize,
    /// The frames drawn at once, and how many of them missed.
    at_once: usize,
    at_once_missed: usize,
    /// A frame's time in microseconds: the median, the 99th percentile and the longest; none
    /// without a frame.
    took_us: Option<(u64, u64, u64)>,
}

fn summarise(frames: &[Frame]) -> Summary {
    let mut took: Vec<u64> = frames.iter().map(|frame| frame.took_us).collect();
    took.sort_unstable();
    Summary {
        frames: frames.len(),
        missed: frames.iter().filter(|frame| frame.missed).count(),
        at_once: frames.iter().filter(|frame| frame.at_once).count(),
        at_once_missed: frames
            .iter()
            .filter(|frame| frame.at_once && frame.missed)
            .count(),
        took_us: took
            .last()
            .map(|&longest| (percentile(&took, 50.0), percentile(&took, 99.0), longest)),
    }
}

/// Where the missed frames of the motion phase fall.
#[derive(Debug, Default, PartialEq)]
struct MissPlaces {
    /// In the first [`FIRST_FRAMES`] frames after an operation.
    first_frames: usize,
    /// Of those, the frames where a window entered the output.
    first_frames_entering: usize,
    /// Later after an operation, or before any, where a window entered the output.
    entering: usize,
    /// Everywhere else.
    elsewhere: usize,
}

fn place_misses(frames: &[Frame]) -> MissPlaces {
    let mut places = MissPlaces::default();
    for frame in frames.iter().filter(|frame| frame.missed) {
        let first = frame
            .operation
            .as_ref()
            .is_some_and(|(_, number)| *number <= FIRST_FRAMES);
        match (first, frame.entering) {
            (true, entering) => {
                places.first_frames += 1;
                places.first_frames_entering += usize::from(entering);
            }
            (false, true) => places.entering += 1,
            (false, false) => places.elsewhere += 1,
        }
    }
    places
}

/// The frame that took longest to draw; the first of equals.
fn heaviest(frames: &[Frame]) -> Option<&Frame> {
    frames.iter().rev().max_by_key(|frame| frame.took_us)
}

/// ADR-0056's verdict.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Motion missed no more frames than the still baseline.
    Met,
    /// Motion missed more.
    NotMet,
    /// A phase has fewer than [`MIN_FRAMES`] frames: too few to judge.
    TooFewFrames,
    /// The baseline ran below [`BASELINE_MIN_SHARE`] of the output's rate: it was not the output
    /// redrawn whole at its rate, which the comparison needs.
    BaselineBelowRate,
}

/// The baseline's pace against the output's.
#[derive(Debug, Clone, Copy)]
struct Pace {
    /// The baseline's frames drawn a second.
    baseline_per_second: f64,
    /// The output's rate, in hertz.
    output_rate: f64,
}

fn frames_per_second(frames: usize, seconds: f64) -> f64 {
    if seconds > 0.0 {
        frames as f64 / seconds
    } else {
        0.0
    }
}

fn verdict(baseline: &Summary, motion: &Summary, pace: Pace) -> Verdict {
    if baseline.frames < MIN_FRAMES || motion.frames < MIN_FRAMES {
        Verdict::TooFewFrames
    } else if pace.baseline_per_second < pace.output_rate * BASELINE_MIN_SHARE {
        Verdict::BaselineBelowRate
    } else if motion.missed <= baseline.missed {
        Verdict::Met
    } else {
        Verdict::NotMet
    }
}

/// Whether a run at `mode`, on the GPU or not, is the one ADR-0056 judges.
fn is_the_adrs_mode(gpu: bool, mode: &OutputMode) -> bool {
    gpu && read_mode(DEFAULT_OUTPUT).is_ok_and(|adrs| adrs == *mode)
}

/// The verdict as the report prints it, with the mode it ran at, and "no pass mark" unless that
/// is the mode ADR-0056 judges.
fn verdict_line(
    verdict: &Verdict,
    gpu: bool,
    output: &str,
    mode: &OutputMode,
    pace: Pace,
) -> String {
    let device = if gpu { "--gpu" } else { "llvmpipe" };
    let standing = if is_the_adrs_mode(gpu, mode) {
        "the ADR's mode".to_owned()
    } else {
        format!("recorded, no pass mark: the ADR judges only at {DEFAULT_OUTPUT} on --gpu")
    };
    let result = match verdict {
        Verdict::Met => "met: motion missed no more frames than the still output".to_owned(),
        Verdict::NotMet => "not met: motion missed more frames than the still output".to_owned(),
        Verdict::TooFewFrames => format!("no verdict: a phase has fewer than {MIN_FRAMES} frames"),
        Verdict::BaselineBelowRate => format!(
            "no verdict: the still baseline drew {:.0} frames a second, below {:.0} % of the \
             output's {:.0} Hz, so it was not the output redrawn whole at its rate",
            pace.baseline_per_second,
            BASELINE_MIN_SHARE * 100.0,
            pace.output_rate,
        ),
    };
    format!("ADR-0056's verdict, run at {output} on {device} ({standing}): {result}")
}

/// The heading of the motion phase's figures.
fn motion_heading(operations: usize, reduced_motion: bool) -> String {
    let rounds = operations / ROUND.len();
    if reduced_motion {
        format!(
            "Motion under reduced motion: {operations} operations, {rounds} whole rounds of {}, \
             {} ms apart; the round has no retarget, and its load is jumps and fades:",
            ROUND.len(),
            REDUCED_MOTION_PAUSE.as_millis(),
        )
    } else {
        format!(
            "Motion: {operations} operations, {rounds} whole rounds of {}, each with a retarget:",
            ROUND.len(),
        )
    }
}

fn report(
    options: &Options,
    phases: &Phases,
    baseline: &Measured,
    motion: &Measured,
    warnings: &[&str],
) {
    println!(
        "Yuna in this process on {}, output {}, animations on{}; {} vkcube on {}: {COLUMNS} \
         columns as wide as the output, a window on a second workspace and one on the special \
         workspace; {} frames a phase, after {} s of warm-up",
        if options.gpu {
            "the GPU (--gpu)"
        } else {
            "llvmpipe"
        },
        options.output,
        if options.reduced_motion {
            " under reduced motion"
        } else {
            ""
        },
        COLUMNS + 2,
        options.client_vulkan.as_deref().unwrap_or("lavapipe"),
        options.frames,
        WARM_UP.as_secs(),
    );
    let output_rate = options.mode.rate();
    let still = summarise(&phases.baseline);
    println!("\nThe still baseline, no operation:");
    let baseline_per_second = print_phase(&still, baseline, output_rate);

    let moving = summarise(&phases.motion);
    println!(
        "\n{}",
        motion_heading(motion.operations, options.reduced_motion)
    );
    print_phase(&moving, motion, output_rate);
    let places = place_misses(&phases.motion);
    println!(
        "  missed, by where they fall: {} in the first {FIRST_FRAMES} frames after an operation \
         ({} as a window entered the output), {} later as a window entered, {} elsewhere",
        places.first_frames, places.first_frames_entering, places.entering, places.elsewhere,
    );
    if let Some(frame) = heaviest(&phases.motion) {
        println!(
            "  the heaviest frame: {} µs, {}, {}, {}",
            frame.took_us,
            match &frame.operation {
                Some((name, number)) => format!("frame {number} after {name}"),
                None => "before any operation".to_owned(),
            },
            if frame.entering {
                "a window entering"
            } else {
                "no window entering"
            },
            if frame.missed { "missed" } else { "on time" },
        );
    }

    let pace = Pace {
        baseline_per_second,
        output_rate,
    };
    println!(
        "\n{}: motion missed {} of {} frames, the still baseline {} of {}",
        verdict_line(
            &verdict(&still, &moving, pace),
            options.gpu,
            &options.output,
            &options.mode,
            pace
        ),
        moving.missed,
        moving.frames,
        still.missed,
        still.frames,
    );
    if !warnings.is_empty() {
        println!("Yuna's first warnings:\n  {}", warnings.join("\n  "));
    }
}

/// Prints a phase's frames and their pace against the output's rate, the frames drawn at once,
/// the missed frames and frame times, then what Yuna and its clients used. Returns the frames a
/// second.
fn print_phase(summary: &Summary, measured: &Measured, output_rate: f64) -> f64 {
    let seconds = (measured.after.at - measured.before.at).as_secs_f64();
    let per_second = frames_per_second(summary.frames, seconds);
    println!(
        "  {} frames in about {seconds:.1} s: {per_second:.1} a second, {:.1} % of the output's \
         {output_rate:.0} Hz",
        summary.frames,
        per_second * 100.0 / output_rate,
    );
    if summary.frames > 0 {
        println!(
            "  drawn at once: {}, of which {} missed",
            summary.at_once, summary.at_once_missed,
        );
        println!(
            "  missed, presented after the boundary they aimed at: {} ({:.2} %)",
            summary.missed,
            summary.missed as f64 * 100.0 / summary.frames as f64,
        );
    }
    if let Some((median, p99, longest)) = summary.took_us {
        println!(
            "  a frame's time (took_us): median {median} µs, 99th percentile {p99} µs, longest \
             {longest} µs"
        );
    }
    if measured.frames > 0 {
        print_usage(&measured.before, &measured.after, measured.frames);
    }
    per_second
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, PoisonError};

    use yuna::yuna_layout::geometry::Point;
    use yuna::yuna_layout::{OpenOptions, OutputConfig};

    use super::*;

    fn frame(
        took_us: u64,
        missed: bool,
        operation: Option<(&str, usize)>,
        entering: bool,
    ) -> Frame {
        Frame {
            took_us,
            missed,
            at_once: false,
            operation: operation.map(|(name, number)| (name.to_owned(), number)),
            entering,
        }
    }

    #[test]
    fn options_take_a_fixed_rate_and_a_count_above_zero() {
        let read = |arguments: &[&str]| read_options(arguments.iter().map(|a| a.to_string()));
        let defaults = read(&[]).unwrap();
        assert_eq!(defaults.output, DEFAULT_OUTPUT);
        assert_eq!(defaults.frames, 10_000);
        assert!(!defaults.gpu && !defaults.reduced_motion && defaults.client_vulkan.is_none());
        let given = read(&[
            "--output",
            "1920x1080@240,1",
            "--frames",
            "300",
            "--reduced-motion",
        ])
        .unwrap();
        assert_eq!(given.mode, OutputMode::new(1920, 1080, 240.0, 1.0).unwrap());
        assert_eq!(given.frames, 300);
        assert!(given.reduced_motion);
        assert!(read(&["--frames", "0"]).is_err());
        assert!(read(&["--frames"]).is_err());
        assert!(read(&["--output", "1920x1080@240,1,vrr=48"]).is_err());
        assert!(read(&["--output", "1920x1080@240"]).is_err());
        assert!(read(&["--output", "0x1080@240,1"]).is_err());
        assert!(read(&["--seconds", "10"]).is_err());
    }

    #[test]
    fn frames_are_read_by_phase_with_their_operation_and_what_entered() {
        let log = "\
            t DEBUG yuna::frames: frame drawn output=1 took_us=1 boundary=1 presented=1\n\
            t DEBUG motion_time: frame shown windows=1\n\
            t  INFO motion_time: phase started phase=baseline\n\
            t DEBUG yuna::frames: frame drawn output=1 elements=1 took_us=900 lead_us=3000 at_once=false boundary=5 presented=5 presented_at_us=20\n\
            t DEBUG motion_time: frame shown windows=1\n\
            t DEBUG yuna: counters frames_drawn=7\n\
            t DEBUG yuna::frames: frame drawn output=1 elements=1 took_us=5000 lead_us=3000 at_once=false boundary=6 presented=7 presented_at_us=29\n\
            t DEBUG motion_time: frame shown windows=1\n\
            t  INFO motion_time: phase ended\n\
            t DEBUG yuna::frames: frame drawn output=1 took_us=1 boundary=8 presented=8\n\
            t DEBUG motion_time: frame shown windows=1\n\
            t  INFO motion_time: phase started phase=motion\n\
            t  INFO motion_time: operation started operation=focus-right\n\
            t DEBUG yuna::frames: frame drawn output=1 took_us=2000 boundary=10 presented=11\n\
            t DEBUG motion_time: frame shown windows=1,2\n\
            t DEBUG yuna::frames: frame drawn output=1 took_us=1000 boundary=11 presented=11\n\
            t DEBUG motion_time: frame shown windows=1,2\n\
            t DEBUG yuna::frames: frame drawn output=1 took_us=1100 boundary=12 presented=12\n\
            t DEBUG motion_time: frame shown windows=2\n\
            t  INFO motion_time: operation started operation=special-in\n\
            t DEBUG yuna::frames: frame drawn output=1 took_us=1200 boundary=13 presented=13\n\
            t DEBUG motion_time: frame shown windows=2,6\n\
            t  INFO motion_time: phase ended\n";
        let phases = read_phases(log).unwrap();
        assert_eq!(
            phases.baseline,
            vec![
                frame(900, false, None, false),
                frame(5000, true, None, false)
            ]
        );
        assert_eq!(
            phases.motion,
            vec![
                frame(2000, true, Some(("focus-right", 1)), true),
                frame(1000, false, Some(("focus-right", 2)), false),
                frame(1100, false, Some(("focus-right", 3)), false),
                frame(1200, false, Some(("special-in", 1)), true),
            ]
        );
    }

    #[test]
    fn a_line_of_a_phase_that_cannot_be_read_is_refused() {
        let started = "t  INFO motion_time: phase started phase=baseline\n";
        let no_time = "t DEBUG yuna::frames: frame drawn output=1 boundary=5 presented=5\n";
        assert!(read_phases(&format!("{started}{no_time}")).is_err());
        // Outside a phase, a frame is not read.
        assert_eq!(read_phases(no_time), Ok(Phases::default()));
        assert!(read_phases("t  INFO motion_time: phase started phase=other\n").is_err());
        assert!(read_phases("t DEBUG motion_time: frame shown windows=1,x\n").is_err());
        assert_eq!(
            shown_windows("t DEBUG motion_time: frame shown windows="),
            Ok(Vec::new())
        );
    }

    /// The log's writer in a test: a buffer the test reads afterwards.
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_lines_written_are_read_back_in_the_logs_format() {
        let buffer = Buffer::default();
        let writer = buffer.clone();
        let subscriber = log_subscriber(move || writer.clone());
        tracing::subscriber::with_default(subscriber, || {
            log_phase_started(Phase::Motion);
            log_operation(Operation::WorkspaceDown);
            // What Yuna writes for a frame (`frames.rs`), with the fields read.
            tracing::debug!(
                target: "yuna::frames",
                output = 1,
                elements = 2,
                took_us = 4200,
                lead_us = 3000,
                at_once = false,
                boundary = 8,
                presented = 9,
                presented_at_us = 1000,
                "frame drawn"
            );
            log_frame_shown(&[WindowId(1), WindowId(5)]);
            log_phase_ended();
        });
        let log = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        let phases = read_phases(&log).unwrap();
        assert_eq!(phases.baseline, Vec::new());
        assert_eq!(
            phases.motion,
            vec![frame(4200, true, Some(("workspace-down", 1)), false)]
        );
    }

    #[test]
    fn misses_are_placed_first_frames_then_entering_then_elsewhere() {
        let frames = [
            frame(1, true, Some(("focus-right", 1)), true),
            frame(1, true, Some(("focus-right", 3)), false),
            frame(1, true, Some(("focus-right", 4)), true),
            frame(1, true, Some(("focus-right", 9)), false),
            frame(1, true, None, false),
            frame(1, true, None, true),
            frame(1, false, Some(("focus-left", 1)), true),
            frame(1, false, Some(("focus-left", 8)), false),
        ];
        assert_eq!(
            place_misses(&frames),
            MissPlaces {
                first_frames: 2,
                first_frames_entering: 1,
                entering: 2,
                elsewhere: 2,
            }
        );
        assert_eq!(place_misses(&[]), MissPlaces::default());
    }

    #[test]
    fn a_phase_is_summed_up_by_nearest_rank_and_its_heaviest_frame_named() {
        let frames: Vec<Frame> = (1..=100)
            .map(|took_us| frame(took_us, took_us % 25 == 0, None, false))
            .collect();
        assert_eq!(
            summarise(&frames),
            Summary {
                frames: 100,
                missed: 4,
                at_once: 0,
                at_once_missed: 0,
                took_us: Some((50, 99, 100)),
            }
        );
        assert_eq!(
            summarise(&[]),
            Summary {
                frames: 0,
                missed: 0,
                at_once: 0,
                at_once_missed: 0,
                took_us: None,
            }
        );
        let two_heaviest = [
            frame(7, false, Some(("special-out", 2)), false),
            frame(9, false, Some(("workspace-down", 1)), true),
            frame(9, true, Some(("workspace-up", 1)), true),
        ];
        assert_eq!(heaviest(&two_heaviest), Some(&two_heaviest[1]));
        assert_eq!(heaviest(&[]), None);
    }

    fn summary(frames: usize, missed: usize) -> Summary {
        Summary {
            frames,
            missed,
            at_once: 0,
            at_once_missed: 0,
            took_us: None,
        }
    }

    /// A baseline that keeps the output's rate.
    const AT_RATE: Pace = Pace {
        baseline_per_second: 240.0,
        output_rate: 240.0,
    };

    #[test]
    fn the_verdict_asks_no_more_misses_over_enough_frames() {
        let enough = MIN_FRAMES;
        assert_eq!(
            verdict(&summary(enough, 5), &summary(enough, 5), AT_RATE),
            Verdict::Met
        );
        assert_eq!(
            verdict(&summary(enough, 5), &summary(enough, 4), AT_RATE),
            Verdict::Met
        );
        assert_eq!(
            verdict(&summary(enough, 5), &summary(enough, 6), AT_RATE),
            Verdict::NotMet
        );
        assert_eq!(
            verdict(&summary(enough, 5), &summary(enough - 1, 0), AT_RATE),
            Verdict::TooFewFrames
        );
        assert_eq!(
            verdict(&summary(enough - 1, 5), &summary(enough, 0), AT_RATE),
            Verdict::TooFewFrames
        );
    }

    #[test]
    fn a_baseline_below_ninety_five_percent_of_the_output_rate_gives_no_verdict() {
        let enough = MIN_FRAMES;
        let pace = |baseline_per_second| Pace {
            baseline_per_second,
            output_rate: 240.0,
        };
        // 95 % of 240 Hz is 228 frames a second.
        assert_eq!(
            verdict(&summary(enough, 0), &summary(enough, 0), pace(227.9)),
            Verdict::BaselineBelowRate
        );
        assert_eq!(
            verdict(&summary(enough, 0), &summary(enough, 0), pace(228.0)),
            Verdict::Met
        );
        // Even where motion would be judged not met.
        assert_eq!(
            verdict(&summary(enough, 0), &summary(enough, 9), pace(60.0)),
            Verdict::BaselineBelowRate
        );
        let line = verdict_line(
            &Verdict::BaselineBelowRate,
            true,
            DEFAULT_OUTPUT,
            &read_mode(DEFAULT_OUTPUT).unwrap(),
            pace(60.0),
        );
        assert!(
            line.contains("no verdict") && line.contains("60") && line.contains("240"),
            "{line}"
        );
    }

    #[test]
    fn the_verdict_names_its_mode_and_is_the_adrs_only_at_its_mode_on_the_gpu() {
        let line = |verdict, gpu, output: &str| {
            verdict_line(&verdict, gpu, output, &read_mode(output).unwrap(), AT_RATE)
        };
        let adr = line(Verdict::Met, true, "3840x2160@240,1.5");
        assert!(
            adr.contains("3840x2160@240,1.5") && adr.contains("--gpu"),
            "{adr}"
        );
        assert!(!adr.contains("no pass mark"), "{adr}");
        for other in [
            line(Verdict::Met, true, "1920x1080@240,1"),
            line(Verdict::Met, true, "3840x2160@120,1.5"),
            line(Verdict::Met, true, "3840x2160@240,1"),
            line(Verdict::Met, false, "3840x2160@240,1.5"),
        ] {
            assert!(other.contains("no pass mark"), "{other}");
            assert!(other.contains("3840x2160@240,1.5"), "{other}");
        }
        let small = line(Verdict::NotMet, true, "1920x1080@240,1");
        assert!(
            small.contains("1920x1080@240,1") && small.contains("no pass mark"),
            "{small}"
        );
    }

    #[test]
    fn frames_drawn_at_once_are_read_and_counted_with_their_misses() {
        let log = "\
            t  INFO motion_time: phase started phase=baseline\n\
            t DEBUG yuna::frames: frame drawn output=1 took_us=1 lead_us=0 at_once=true boundary=5 presented=6\n\
            t DEBUG motion_time: frame shown windows=1\n\
            t DEBUG yuna::frames: frame drawn output=1 took_us=1 lead_us=0 at_once=true boundary=6 presented=6\n\
            t DEBUG motion_time: frame shown windows=1\n\
            t DEBUG yuna::frames: frame drawn output=1 took_us=1 lead_us=3000 at_once=false boundary=7 presented=8\n\
            t DEBUG motion_time: frame shown windows=1\n\
            t  INFO motion_time: phase ended\n";
        let phases = read_phases(log).unwrap();
        let at_once: Vec<bool> = phases.baseline.iter().map(|f| f.at_once).collect();
        assert_eq!(at_once, [true, true, false]);
        let summed = summarise(&phases.baseline);
        assert_eq!((summed.frames, summed.missed), (3, 2));
        assert_eq!((summed.at_once, summed.at_once_missed), (2, 1));
    }

    #[test]
    fn frames_a_second_are_the_frames_over_the_seconds() {
        assert_eq!(frames_per_second(2400, 10.0), 240.0);
        assert_eq!(frames_per_second(0, 0.0), 0.0);
        assert_eq!(frames_per_second(5, 0.0), 0.0);
    }

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn an_operation_waits_for_its_arrangement_then_for_the_motion_before_to_end() {
        let mut script = Script::default();
        assert_eq!(script.wait(Duration::ZERO), Duration::ZERO);
        assert_eq!(
            script.due(Duration::ZERO, 0, || unreachable!()),
            Some(Operation::FocusRight)
        );
        // No frame drawn since: not arranged yet; the frames wake the loop.
        assert_eq!(script.due(MS, 0, || unreachable!()), None);
        assert_eq!(script.wait(MS), MAX_WAIT);
        // Arranged, its motion running until 400 ms: the end is asked once, then waited for.
        assert_eq!(script.due(2 * MS, 1, || Some(400 * MS)), None);
        assert_eq!(script.wait(350 * MS), 50 * MS);
        assert_eq!(script.wait(2 * MS), MAX_WAIT);
        assert_eq!(script.due(399 * MS, 90, || unreachable!()), None);
        // Asked again at the end, and longer: waited for again.
        assert_eq!(script.due(400 * MS, 91, || Some(410 * MS)), None);
        assert_eq!(
            script.due(410 * MS, 93, || None),
            Some(Operation::FocusRight)
        );
    }

    #[test]
    fn the_round_cycles_and_its_retarget_comes_thirty_milliseconds_in() {
        let mut script = Script::default();
        let mut now = Duration::ZERO;
        let mut frames = 0;
        let mut given = Vec::new();
        // Every motion ends at once: each step is given at the first frame after the one before,
        // but the retarget.
        while given.len() < ROUND.len() - 1 {
            if let Some(operation) = script.due(now, frames, || None) {
                given.push(operation);
            }
            now += MS;
            frames += 1;
        }
        let expected: Vec<Operation> = ROUND.iter().map(|step| step.operation).collect();
        assert_eq!(given, expected[..ROUND.len() - 1]);
        let focused_right_at = now - MS;
        // 30 ms after the focus right, whatever its motion: never asked.
        assert_eq!(script.wait(now), RETARGET_AFTER - MS);
        assert_eq!(
            script.due(
                focused_right_at + RETARGET_AFTER - MS,
                frames,
                || unreachable!()
            ),
            None
        );
        assert_eq!(
            script.due(focused_right_at + RETARGET_AFTER, frames, || unreachable!()),
            Some(Operation::FocusLeft)
        );
        // Then the round starts again, once the retarget's motion ended.
        let now = focused_right_at + RETARGET_AFTER;
        assert_eq!(
            script.due(now + MS, frames + 1, || None),
            Some(Operation::FocusRight)
        );
        assert_eq!(script.given, ROUND.len() + 1);
    }

    #[test]
    fn a_retarget_waits_for_a_frame_drawn_since_the_step_before() {
        let mut script = Script::default();
        let mut now = Duration::ZERO;
        let mut frames = 0;
        // Every motion ends at once: the steps before the retarget are given one a frame.
        for _ in 0..ROUND.len() - 1 {
            assert!(script.due(now, frames, || None).is_some());
            now += MS;
            frames += 1;
        }
        let given_at = now - MS;
        let given_frames = frames - 1;
        // The delay has passed, but no frame was drawn since the focus right: both changes would
        // land in one layout pass and cancel out.
        assert_eq!(
            script.due(given_at + RETARGET_AFTER, given_frames, || None),
            None
        );
        assert_eq!(
            script.due(given_at + 2 * RETARGET_AFTER, given_frames, || None),
            None
        );
        // The loop waits for the frame, which wakes it, rather than spinning.
        assert_eq!(script.wait(given_at + RETARGET_AFTER), MAX_WAIT);
        assert_eq!(
            script.due(given_at + 2 * RETARGET_AFTER, given_frames + 1, || None),
            Some(Operation::FocusLeft)
        );
    }

    #[test]
    fn under_reduced_motion_a_pause_separates_steps_that_move_nothing() {
        let mut script = Script::new(true);
        assert_eq!(
            script.due(Duration::ZERO, 0, || unreachable!()),
            Some(Operation::FocusRight)
        );
        // A frame drawn, nothing moving: the next step still waits for the pause, and does not
        // ask for the motion's end before it.
        assert_eq!(script.due(MS, 1, || unreachable!()), None);
        // The wait is cut at MAX_WAIT; near the end of the pause it is the rest of it.
        assert_eq!(script.wait(MS), MAX_WAIT);
        assert_eq!(script.wait(REDUCED_MOTION_PAUSE - 50 * MS), 50 * MS);
        assert_eq!(
            script.due(REDUCED_MOTION_PAUSE - MS, 5, || unreachable!()),
            None
        );
        assert_eq!(
            script.due(REDUCED_MOTION_PAUSE, 6, || None),
            Some(Operation::FocusRight)
        );
        // Without reduced motion there is no pause.
        let mut script = Script::new(false);
        assert!(script.due(Duration::ZERO, 0, || None).is_some());
        assert!(script.due(MS, 1, || None).is_some());
    }

    #[test]
    fn the_motion_heading_says_a_reduced_round_has_no_retarget() {
        let normal = motion_heading(28, false);
        assert!(normal.contains("28 operations") && normal.contains("2 whole rounds"));
        assert!(normal.contains("retarget"), "{normal}");
        let reduced = motion_heading(28, true);
        assert!(reduced.contains("reduced motion"), "{reduced}");
        assert!(reduced.contains("no retarget"), "{reduced}");
        assert!(reduced.contains("jumps and fades"), "{reduced}");
        assert!(reduced.contains("250 ms"), "{reduced}");
        assert!(!reduced.contains("each with a retarget"), "{reduced}");
    }

    /// The layout `set_up` makes, on a 4K output at scale 1.5, without Yuna.
    fn set_up_layout() -> (Layout, OutputId, Vec<WindowId>) {
        let output = OutputId(1);
        let mut layout = Layout::new(yuna::yuna_layout::Options::default()).unwrap();
        layout.add_seat(OWNER_SEAT).unwrap();
        let config = OutputConfig {
            position: Point { x: 0.0, y: 0.0 },
            physical_width: 3840,
            physical_height: 2160,
            scale: 1.5,
        };
        layout.add_output(OWNER_SEAT, output, config).unwrap();
        let open = |layout: &mut Layout, id| {
            layout
                .open_window(OWNER_SEAT, WindowId(id), OpenOptions::default())
                .unwrap();
        };
        for id in 1..=COLUMNS as u64 {
            open(&mut layout, id);
        }
        show_second_workspace(&mut layout, output).unwrap();
        open(&mut layout, COLUMNS as u64 + 1);
        show_special_workspace(&mut layout).unwrap();
        open(&mut layout, COLUMNS as u64 + 2);
        let windows: Vec<WindowId> = (1..=COLUMNS as u64 + 2).map(WindowId).collect();
        (layout, output, windows)
    }

    #[test]
    fn the_set_up_is_checked() {
        let (mut layout, output, windows) = set_up_layout();
        assert!(check_set_up(&layout, output, &windows).is_err());
        finish_set_up(&mut layout, output, &windows).unwrap();
        assert_eq!(check_set_up(&layout, output, &windows), Ok(()));
        assert!(check_set_up(&layout, output, &windows[..COLUMNS]).is_err());
    }

    #[test]
    fn each_operation_of_the_round_moves_the_view_and_the_round_comes_back() {
        let (mut layout, output, windows) = set_up_layout();
        finish_set_up(&mut layout, output, &windows).unwrap();
        let view = |layout: &Layout| {
            (
                layout.arrange(output).unwrap(),
                layout.shown_special(output).unwrap(),
                layout.focused(OWNER_SEAT),
            )
        };
        let start = view(&layout);
        for step in ROUND {
            let before = view(&layout);
            step.operation.apply(&mut layout).unwrap();
            assert_ne!(
                view(&layout),
                before,
                "{} moves nothing",
                step.operation.name()
            );
            layout.check_invariants().unwrap();
        }
        assert_eq!(view(&layout), start);
        assert_eq!(check_set_up(&layout, output, &windows), Ok(()));
    }
}
