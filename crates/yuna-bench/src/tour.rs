//! The owner's tour, `yuna-bench tour`: each rival in turn on the real display, the same rounds
//! with the same clients, for what only a real display measures: frames missed on its refresh,
//! input to present end to end, and watts
//! ([ADR-0071](../../../docs/adr/0071-the-rivals-measured-headless-by-agents-and-on-a-real-display-by-the-owner.md)).
//!
//! The owner runs it from a virtual terminal where they logged in and no compositor runs. It
//! refuses to start otherwise ([`refuse_a_running_compositor`], [`own_seat`]), and when a GPU's
//! performance level is not `auto` ([`refuse_gpu_levels`]): it prints the commands that set it and
//! put the owner's back, and never writes it itself.
//!
//! Each case starts its rival on DRM in a sandbox of its own (`sandbox`): a transient scope, an
//! environment built from nothing but the tour terminal's own logind session (`sandbox::Seat`), a
//! private session bus, and a time limit after which systemd stops the scope whatever the bench
//! does ([`time_limit`]), so that a hung rival never keeps the display. Its rounds are
//! `scenarios`' own, marked `tour` in the run's file. A rival that crashes or is refused fails its
//! case, and the tour goes on to the next.
//!
//! `--dry-run` runs the same code with each rival headless, as `headless` runs it, or nested in a
//! headless dwl where it cannot run headless (niri, cosmic-comp): everything but the start on the
//! real display, for agents and CI. Its cases are marked `tour-dry-run`, and its figures are never
//! the display's.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::cli;
use crate::lock;
use crate::machine::{self, GpuLevel, MachineInfo};
use crate::procfs::Proc;
use crate::recipes::{self, Configuration, Frames};
use crate::record::{Case, CaseRun, FORMAT, RunFile};
use crate::report;
use crate::sandbox::{self, RENDER_NODE, STOP_PATIENCE, Seat, ShellPaths};
use crate::scenarios::{
    self, Bench, Durations, PRESSES, Place, TOUR_REST_WINDOWS, TOUR_SCENARIOS, WARM_UP_PRESSES,
};

/// The machines the tour knows, which set its default rivals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Machine {
    /// The owner's desktop: every rival.
    Desktop,
    /// A laptop: the peers, dwl and mango for power, and its own compositor.
    Laptop,
    /// The Steam Deck: the same, its own compositor gamescope.
    Deck,
}

impl Machine {
    /// Its name on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Machine::Desktop => "desktop",
            Machine::Laptop => "laptop",
            Machine::Deck => "deck",
        }
    }

    /// The machine named `name`.
    pub fn named(name: &str) -> Option<Machine> {
        [Machine::Desktop, Machine::Laptop, Machine::Deck]
            .into_iter()
            .find(|machine| machine.name() == name)
    }

    /// The machine this is, from sysfs at `sys`: the Steam Deck by its board's product name
    /// (`Jupiter` for the LCD model, `Galileo` for the OLED), a laptop by a battery, else a
    /// desktop.
    pub fn detect(sys: &Path) -> Machine {
        let product = fs::read_to_string(sys.join("class/dmi/id/product_name")).unwrap_or_default();
        if ["Jupiter", "Galileo"].contains(&product.trim()) {
            return Machine::Deck;
        }
        match machine::battery(sys) {
            (Some(_), _) => Machine::Laptop,
            (None, _) => Machine::Desktop,
        }
    }

    /// Its default rivals, in the tour's order ([`recipes::TOUR_RIVALS`]). The peers in features
    /// are niri, Hyprland, Umbriel and KWin (ADR-0072); dwl and mango are the rivals for power;
    /// the laptop's own compositor is niri, a peer already, and the Deck's are gamescope, its
    /// game mode, and KWin, its desktop mode.
    pub fn default_rivals(self) -> Vec<&'static str> {
        let rivals: &[&str] = match self {
            Machine::Desktop => &recipes::TOUR_RIVALS,
            Machine::Laptop => &["dwl", "mango", "umbriel", "hyprland", "niri", "kwin"],
            Machine::Deck => &[
                "dwl",
                "mango",
                "umbriel",
                "hyprland",
                "niri",
                "kwin",
                "gamescope",
            ],
        };
        rivals.to_vec()
    }
}

/// The options of `tour`.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The rivals; none for the machine's defaults.
    pub compositors: Option<Vec<String>>,
    /// The machine, when not detected.
    pub machine: Option<Machine>,
    /// The rounds.
    pub scenarios: Vec<String>,
    /// The configurations.
    pub configurations: Vec<Configuration>,
    /// Runs of each case.
    pub runs: u32,
    /// The keys `latency` measures.
    pub presses: u32,
    /// Every window's length, to try the tour.
    pub seconds: Option<f64>,
    /// Where the run's file goes.
    pub out: Option<PathBuf>,
    /// Each rival headless instead of on the real display.
    pub dry_run: bool,
    /// Whether to keep the run's folder of logs.
    pub keep: bool,
}

/// Reads `tour`'s options.
pub fn read_options(arguments: &[String]) -> Result<Options, String> {
    let mut options = Options {
        compositors: None,
        machine: None,
        scenarios: TOUR_SCENARIOS.iter().map(|s| (*s).to_owned()).collect(),
        configurations: vec![Configuration::EqualWork],
        runs: 1,
        presses: PRESSES,
        seconds: None,
        out: None,
        dry_run: false,
        keep: false,
    };
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .cloned()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        let whole = |text: String, what: &str| {
            text.parse::<u32>()
                .ok()
                .filter(|&n| n > 0)
                .ok_or_else(|| format!("{what} takes a whole number above 0"))
        };
        match argument.as_str() {
            "--compositor" => options.compositors = Some(cli::list(&value()?)),
            "--machine" => {
                options.machine = Some(
                    Machine::named(&value()?).ok_or("--machine takes desktop, laptop or deck")?,
                );
            }
            "--scenario" => options.scenarios = cli::list(&value()?),
            "--configuration" => {
                options.configurations = cli::list(&value()?)
                    .iter()
                    .map(|name| Configuration::named(name))
                    .collect::<Option<_>>()
                    .ok_or("--configuration takes as-shipped and equal-work")?;
            }
            "--runs" => options.runs = whole(value()?, "--runs")?,
            "--presses" => options.presses = whole(value()?, "--presses")?,
            "--seconds" => {
                options.seconds = Some(
                    value()?
                        .parse()
                        .ok()
                        .filter(|s: &f64| *s > 0.0)
                        .ok_or("--seconds takes a number above 0")?,
                );
            }
            "--out" => options.out = Some(PathBuf::from(value()?)),
            "--dry-run" => options.dry_run = true,
            "--keep" => options.keep = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if let Some(compositors) = &options.compositors {
        for name in compositors {
            if recipes::tour_recipe(name).is_none() {
                return Err(format!(
                    "no rival {name:?} on the tour; it runs {}",
                    recipes::TOUR_RIVALS.join(", ")
                ));
            }
        }
    }
    for name in &options.scenarios {
        if !TOUR_SCENARIOS.contains(&name.as_str()) {
            return Err(format!(
                "no round {name:?}; the tour runs {}",
                TOUR_SCENARIOS.join(", ")
            ));
        }
    }
    if options.compositors.as_ref().is_some_and(Vec::is_empty)
        || options.scenarios.is_empty()
        || options.configurations.is_empty()
    {
        return Err(
            "--compositor, --scenario and --configuration need one value at least".to_owned(),
        );
    }
    Ok(options)
}

/// Refuses to start under a compositor of the tour's own session: one runs where
/// `WAYLAND_DISPLAY`, `WAYLAND_SOCKET` or `DISPLAY` is set, and the tour would measure it beside
/// each rival, or fight it for the display. `variable` reads the environment.
pub fn refuse_a_running_compositor(
    variable: &dyn Fn(&str) -> Option<String>,
) -> Result<(), String> {
    let set: Vec<String> = ["WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DISPLAY"]
        .into_iter()
        .filter_map(|name| variable(name).map(|value| format!("{name}={value}")))
        .collect();
    if set.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "a compositor runs in this session ({}): run the tour from a virtual terminal where \
             you logged in and none runs, such as Ctrl+Alt+F3",
            set.join(", ")
        ))
    }
}

/// Refuses to start unless every GPU that has a performance level is at `auto`, the level the
/// tour measures at (ADR-0071); gives the commands the owner runs to set it, and to put theirs
/// back after. The tour never writes it.
pub fn refuse_gpu_levels(levels: &[GpuLevel]) -> Result<(), String> {
    let other: Vec<&GpuLevel> = levels
        .iter()
        .filter(|level| level.level != "auto")
        .collect();
    if other.is_empty() {
        return Ok(());
    }
    let mut text = String::from(
        "the tour measures with every GPU's performance level at `auto` (ADR-0071), and it \
         never changes it itself. Set it, run the tour, then put yours back:\n",
    );
    for level in &other {
        text.push_str(&format!(
            "  echo auto | sudo tee {}\n",
            level.file.display()
        ));
    }
    text.push_str("  (the tour)\n");
    for level in &other {
        text.push_str(&format!(
            "  echo {} | sudo tee {}\n",
            level.level,
            level.file.display()
        ));
    }
    Err(text.trim_end().to_owned())
}

/// The logind session of the process whose `/proc/<pid>/cgroup` is `cgroup`, `session-<id>.scope`
/// in it; none outside a session (a service, a scope of the user's manager).
pub fn session_of(cgroup: &str) -> Option<String> {
    cgroup.lines().find_map(|line| {
        line.rsplit('/')
            .next()?
            .strip_prefix("session-")?
            .strip_suffix(".scope")
            .map(str::to_owned)
    })
}

/// A value of a logind session's file, `KEY=VALUE` lines.
fn session_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .map(str::to_owned)
}

/// Where logind writes its sessions' files.
pub const SESSIONS: &str = "/run/systemd/sessions";

/// The tour terminal's own logind session, which every rival takes: the session of this process
/// (`cgroup`, its `/proc/self/cgroup`), whose file in `sessions` says it is the user `uid`'s, on
/// a virtual terminal of a seat (`TYPE=tty`, `SEAT`), with no compositor in it yet, and shown
/// (`ACTIVE=1`). Refuses otherwise, saying how to run the tour.
pub fn own_seat(cgroup: &str, sessions: &Path, uid: u32) -> Result<Seat, String> {
    let how = "run the tour from a virtual terminal where you logged in, such as Ctrl+Alt+F3";
    let session = session_of(cgroup).ok_or_else(|| {
        format!(
            "this process is in no logind session (its cgroup: {}): {how}",
            cgroup.trim()
        )
    })?;
    let file = sessions.join(&session);
    let text = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let value = |key: &str| session_value(&text, key);
    if value("UID").and_then(|owner| owner.parse::<u32>().ok()) != Some(uid) {
        return Err(format!("session {session} is not this user's: {how}"));
    }
    let seat = value("SEAT").ok_or_else(|| {
        format!("session {session} has no seat, as over SSH: no display to take; {how}")
    })?;
    match value("TYPE").as_deref() {
        Some("tty") => {}
        Some(kind) => {
            return Err(format!(
                "session {session} is of type {kind}: a compositor runs in it already; {how}"
            ));
        }
        None => return Err(format!("{} says no TYPE", file.display())),
    }
    if value("ACTIVE").as_deref() != Some("1") {
        return Err(format!(
            "session {session} is not the one shown on its seat: switch to its terminal first"
        ));
    }
    Ok(Seat {
        session,
        seat,
        vt: value("VTNR").and_then(|vt| vt.parse().ok()),
    })
}

/// Whether the session `seat` names is the one shown on its seat now.
pub fn is_active(sessions: &Path, seat: &Seat) -> bool {
    fs::read_to_string(sessions.join(&seat.session))
        .is_ok_and(|text| session_value(&text, "ACTIVE").as_deref() == Some("1"))
}

/// What a start, its readiness and its stop take, beside the rounds: a little more than the
/// dry run of sway, KWin and Hyprland took on 2026-11-16, whose 12 cases took 6 min 57 s where
/// these figures give 7 min 50 s. A start on the real display sets a mode, which takes longer.
const START_AND_STOP: Duration = Duration::from_secs(6);
/// What a client takes to start and show.
const CLIENT_START: Duration = Duration::from_secs(1);
/// What the tour waits before it takes the display, for the owner to read what it will run.
const BEFORE_THE_DISPLAY: Duration = Duration::from_secs(5);
/// What each key of `latency` takes: its pause, half its random part on average, and its answer.
const PRESS: Duration = Duration::from_millis(70);

/// Why `rival` does not run `scenario` on the tour, if it does not: gamescope shows one window at
/// a time, and a rival without actions has no `motion`.
pub fn not_run(rival: &str, scenario: &str) -> Option<&'static str> {
    let recipe = recipes::tour_recipe(rival)?;
    match scenario {
        "rest" if rival == "gamescope" => {
            Some("gamescope shows one window at a time, its game's: no still windows to rest with")
        }
        "motion" => recipe.no_actions,
        _ => None,
    }
}

/// About how long one case of `rival` in `scenario` takes, its start and stop included; zero for
/// a case it does not run.
pub fn case_time(rival: &str, scenario: &str, durations: &Durations, presses: u32) -> Duration {
    let Some(recipe) = recipes::tour_recipe(rival) else {
        return Duration::ZERO;
    };
    if not_run(rival, scenario).is_some() {
        return Duration::ZERO;
    }
    let round = |actions: usize| durations.action * actions as u32;
    let rounds = match scenario {
        "rest" => CLIENT_START + durations.settle + durations.rest,
        "game" => CLIENT_START * 2 + durations.warm_up + durations.window,
        "motion" => {
            let actions = recipe.actions.len();
            // A round to warm up, the cost round, and the frames' round apart where frames are
            // counted by capture.
            let apart = if recipe.frames == Frames::Capture {
                durations.counter_lead + round(actions)
            } else {
                Duration::ZERO
            };
            CLIENT_START * 2 + durations.settle + round(actions) * 2 + apart
        }
        "latency" => CLIENT_START * 2 + PRESS * (presses + WARM_UP_PRESSES),
        _ => Duration::ZERO,
    };
    START_AND_STOP + rounds
}

/// The longest one case may keep the display: twice its time, and a minute more, the scope's
/// `RuntimeMaxSec`.
pub fn time_limit(case: Duration) -> Duration {
    case * 2 + Duration::from_secs(60)
}

/// About how long the tour of `rivals` takes, each in `configurations` and `scenarios` `runs`
/// times.
pub fn tour_time(
    rivals: &[&str],
    scenarios: &[&str],
    configurations: usize,
    runs: u32,
    durations: &Durations,
    presses: u32,
) -> Duration {
    rivals
        .iter()
        .flat_map(|rival| {
            scenarios
                .iter()
                .map(move |scenario| case_time(rival, scenario, durations, presses))
        })
        .sum::<Duration>()
        * configurations as u32
        * runs
}

/// A duration as a person reads it: `2 min 30 s`.
pub fn minutes(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match (seconds / 60, seconds % 60) {
        (0, seconds) => format!("{seconds} s"),
        (minutes, 0) => format!("{minutes} min"),
        (minutes, seconds) => format!("{minutes} min {seconds} s"),
    }
}

/// Each case of the tour, in its order: rival by rival, as the owner watches them, each in its
/// configurations and rounds.
pub fn cases(rivals: &[&str], options: &Options, mark: Option<&str>) -> Vec<(Case, u32)> {
    let mut cases = Vec::new();
    for rival in rivals {
        for configuration in &options.configurations {
            for scenario in &options.scenarios {
                for repeat in 1..=options.runs {
                    cases.push((
                        Case {
                            compositor: (*rival).to_owned(),
                            configuration: configuration.name().to_owned(),
                            scenario: scenario.clone(),
                            windows: (scenario == "rest").then_some(TOUR_REST_WINDOWS),
                            tour: mark.map(str::to_owned),
                        },
                        repeat,
                    ));
                }
            }
        }
    }
    cases
}

/// This process's user id, from `/proc/self/status`.
fn own_uid() -> Option<u32> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("Uid:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}

/// How long the tour waits for the owner to come back to its terminal, when they left it.
const WAIT_FOR_THE_TERMINAL: Duration = Duration::from_secs(600);

/// Runs `yuna-bench tour`; gives whether every case ended well.
pub fn run(arguments: &[String]) -> Result<bool, String> {
    // First, before any thread or child, as `headless` does: no inherited descriptor reaches a
    // compositor.
    let inherited = sandbox::close_inherited_on_exec();
    let options = read_options(arguments)?;
    refuse_a_running_compositor(&|name| std::env::var(name).ok())?;
    let sys = Path::new(machine::SYS);
    let levels = machine::gpu_levels(sys);
    let mut notes = Vec::new();
    let place = if options.dry_run {
        if let Err(refusal) = refuse_gpu_levels(&levels) {
            // Agents never change it, and a dry run's figures are no display's.
            notes.push(format!(
                "the dry run ran at a GPU level that the tour on the display would refuse: {}",
                refusal.lines().next().unwrap_or_default()
            ));
        }
        Place::DryRun
    } else {
        refuse_gpu_levels(&levels)?;
        let cgroup = fs::read_to_string("/proc/self/cgroup")
            .map_err(|e| format!("/proc/self/cgroup: {e}"))?;
        let uid = own_uid().ok_or("this process's user cannot be read")?;
        Place::Display(own_seat(&cgroup, Path::new(SESSIONS), uid)?)
    };
    let machine = options.machine.unwrap_or_else(|| Machine::detect(sys));
    let rivals: Vec<&str> = match &options.compositors {
        Some(names) => names.iter().map(String::as_str).collect(),
        None => machine.default_rivals(),
    };
    let shell = ShellPaths::from_environment()?;
    let program = std::env::current_exe().map_err(|e| format!("this program: {e}"))?;
    if !inherited.is_empty() {
        notes.push(format!(
            "descriptors inherited and kept from every child: {inherited:?}"
        ));
    }
    let swept = sandbox::sweep_left_behind(&Proc::system());
    if !swept.is_empty() {
        notes.push(format!(
            "scopes left by a bench killed earlier, killed: {}",
            swept.join(", ")
        ));
    }
    let links = sandbox::sweep_runtime_links(&Proc::system(), Path::new("/tmp"));
    if !links.is_empty() {
        notes.push(format!(
            "runtime links left by a bench killed earlier, removed: {}",
            links.join(", ")
        ));
    }
    let durations = options
        .seconds
        .map_or(Durations::STANDARD, Durations::with_windows);
    let scenario_names: Vec<&str> = options.scenarios.iter().map(String::as_str).collect();
    let total = tour_time(
        &rivals,
        &scenario_names,
        options.configurations.len(),
        options.runs,
        &durations,
        options.presses,
    );
    eprintln!(
        "yuna-bench: the tour of the {}{}: {}; {} round(s) each, {}; about {}",
        machine.name(),
        if options.dry_run { ", dry run" } else { "" },
        rivals.join(", "),
        scenario_names.len(),
        options
            .configurations
            .iter()
            .map(|c| c.name())
            .collect::<Vec<_>>()
            .join(" and "),
        minutes(total)
    );
    let exclusive = lock::take()?;
    if place.on_display() {
        eprintln!(
            "yuna-bench: each rival takes the screen in turn, from {} s from now; Ctrl+C now to \
             stop. A rival that hangs is stopped at the time each case says.",
            BEFORE_THE_DISPLAY.as_secs()
        );
        std::thread::sleep(BEFORE_THE_DISPLAY);
    }
    let started = SystemTime::now();
    let started_s = started
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let folder = Path::new("/tmp").join(format!("yuna-bench-{}-{started_s}", std::process::id()));
    std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700)
        .create(&folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    let mut bench = Bench::new(shell.clone(), program, folder.clone(), durations);
    bench.place = place.clone();
    bench.presses = options.presses;
    scenarios::prepare(&mut bench, &rivals, &scenario_names, &mut notes);
    notes.push(format!(
        "the tour of the {}, {}: {}",
        machine.name(),
        match &place {
            Place::Display(seat) => format!(
                "on the real display, through logind session {} of {}{}",
                seat.session,
                seat.seat,
                seat.vt
                    .map_or(String::new(), |vt| format!(", virtual terminal {vt}"))
            ),
            _ => "its dry run: each rival headless, or nested in a headless dwl".to_owned(),
        },
        rivals.join(", ")
    ));
    let cases = cases(&rivals, &options, place.mark());
    let commit =
        scenarios::commit(&[]).or_else(|| option_env!("YUNA_BENCH_COMMIT").map(str::to_owned));
    let mut tour_arguments = vec!["tour".to_owned()];
    tour_arguments.extend(arguments.iter().cloned());
    let mut file = RunFile {
        format: FORMAT,
        commit: commit
            .as_ref()
            .map(|c| c.trim_end_matches("-dirty").to_owned()),
        dirty: commit.as_ref().map(|c| c.ends_with("-dirty")),
        started_utc: cli::utc(started),
        arguments: tour_arguments,
        seed: 0,
        seconds_override: options.seconds,
        exclusive_lock: exclusive.how.clone(),
        machine: MachineInfo::read(RENDER_NODE, Some(&shell.mesa.to_string_lossy())),
        notes,
        order: cases
            .iter()
            .map(|(case, repeat)| format!("{} #{repeat}", case.label()))
            .collect(),
        runs: Vec::new(),
    };
    for (n, (case, repeat)) in cases.iter().enumerate() {
        let time = case_time(
            &case.compositor,
            &case.scenario,
            &durations,
            options.presses,
        );
        if let Some(why) = not_run(&case.compositor, &case.scenario) {
            file.runs.push(CaseRun {
                case: case.clone(),
                repeat: *repeat,
                notes: vec![format!("not run: {why}")],
                ..CaseRun::default()
            });
            continue;
        }
        let limit = time_limit(time);
        eprintln!(
            "yuna-bench: [{}/{}] {} run {repeat}: about {}, stopped after {} at most",
            n + 1,
            cases.len(),
            case.label(),
            minutes(time),
            minutes(limit + STOP_PATIENCE)
        );
        if let Place::Display(seat) = &place
            && !wait_for_the_terminal(seat)
        {
            file.runs.push(CaseRun {
                case: case.clone(),
                repeat: *repeat,
                error: Some(format!(
                    "not started: the tour's terminal was not shown for {}",
                    minutes(WAIT_FOR_THE_TERMINAL)
                )),
                ..CaseRun::default()
            });
            continue;
        }
        bench.time_limit = Some(limit);
        let mut run = scenarios::run_case(&mut bench, case, *repeat);
        if let Place::Display(seat) = &place
            && !is_active(Path::new(SESSIONS), seat)
            && run.error.is_none()
        {
            run.error = Some(
                "the tour's terminal was not shown at the case's end: another terminal was \
                 switched to, and the rival paused"
                    .to_owned(),
            );
        }
        if let Some(error) = &run.error {
            eprintln!(
                "yuna-bench:   failed: {}",
                error.lines().next().unwrap_or("")
            );
        }
        for refused in &run.refused {
            eprintln!("yuna-bench:   refused: {refused}");
        }
        file.runs.push(run);
    }
    let out = options.out.unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let stem = format!(
        "{}-{}",
        if options.dry_run {
            "tour-dry-run"
        } else {
            "tour"
        },
        file.started_utc.replace([':', '-'], "")
    );
    let json = out.join(format!("{stem}.json"));
    let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    cli::write_atomically(&json, &text)?;
    let summary = out.join(format!("{stem}.md"));
    cli::write_atomically(
        &summary,
        &report::summary(&[(format!("{stem}.json"), file.clone())]),
    )?;
    let clean = file
        .runs
        .iter()
        .all(|run| run.error.is_none() && run.refused.is_empty());
    if options.keep || !clean {
        eprintln!(
            "yuna-bench: the run's logs are kept in {}",
            folder.display()
        );
    } else {
        let _ = fs::remove_dir_all(&folder);
    }
    println!("{}", json.display());
    println!("{}", summary.display());
    Ok(clean)
}

/// Waits until the tour's terminal is the one shown, for [`WAIT_FOR_THE_TERMINAL`] at most: a
/// rival started on a terminal not shown gets no display. Gives whether it is.
fn wait_for_the_terminal(seat: &Seat) -> bool {
    let sessions = Path::new(SESSIONS);
    if is_active(sessions, seat) {
        return true;
    }
    eprintln!(
        "yuna-bench: waiting for the tour's terminal{} to be shown again",
        seat.vt.map_or(String::new(), |vt| format!(", {vt}"))
    );
    let deadline = std::time::Instant::now() + WAIT_FOR_THE_TERMINAL;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        if is_active(sessions, seat) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(arguments: &[&str]) -> Result<Options, String> {
        read_options(
            &arguments
                .iter()
                .map(|a| (*a).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn options_default_to_every_round_at_equal_work_once() {
        let options = read(&[]).unwrap();
        assert_eq!(options.compositors, None);
        assert_eq!(options.scenarios, ["rest", "game", "motion", "latency"]);
        assert_eq!(options.configurations, [Configuration::EqualWork]);
        assert_eq!((options.runs, options.presses), (1, 300));
        assert!(!options.dry_run);
        let options = read(&["--compositor", "sway,kwin,gamescope", "--dry-run"]).unwrap();
        assert_eq!(options.compositors.unwrap(), ["sway", "kwin", "gamescope"]);
        assert!(options.dry_run);
        // Yuna enters the tour at Stage 2; sway-vulkan is headless's alone.
        assert!(read(&["--compositor", "yuna"]).is_err());
        assert!(read(&["--compositor", "sway-vulkan"]).is_err());
        assert!(read(&["--scenario", "film"]).is_err());
        assert!(read(&["--machine", "server"]).is_err());
        assert_eq!(
            read(&["--machine", "deck"]).unwrap().machine,
            Some(Machine::Deck)
        );
        assert!(read(&["--presses", "0"]).is_err());
        assert!(read(&["--compositor", ""]).is_err());
    }

    #[test]
    fn a_compositor_of_the_tours_own_session_refuses_it() {
        let none = |_: &str| None;
        assert!(refuse_a_running_compositor(&none).is_ok());
        for variable in ["WAYLAND_DISPLAY", "DISPLAY", "WAYLAND_SOCKET"] {
            let set = |name: &str| (name == variable).then(|| "wayland-1".to_owned());
            let refusal = refuse_a_running_compositor(&set).unwrap_err();
            assert!(refusal.contains(variable), "{refusal}");
            assert!(refusal.contains("virtual terminal"), "{refusal}");
        }
    }

    #[test]
    fn a_gpu_level_other_than_auto_refuses_the_tour_with_the_commands() {
        let level = |card: &str, level: &str| GpuLevel {
            file: PathBuf::from(format!(
                "/sys/class/drm/{card}/device/power_dpm_force_performance_level"
            )),
            level: level.to_owned(),
        };
        // No GPU with the file, as an Intel one: nothing to refuse.
        assert!(refuse_gpu_levels(&[]).is_ok());
        assert!(refuse_gpu_levels(&[level("card0", "auto")]).is_ok());
        let refusal =
            refuse_gpu_levels(&[level("card0", "auto"), level("card1", "high")]).unwrap_err();
        assert!(
            refusal.contains(
                "echo auto | sudo tee /sys/class/drm/card1/device/power_dpm_force_performance_level"
            ),
            "{refusal}"
        );
        // The owner's own level, to put back after.
        assert!(
            refusal.contains(
                "echo high | sudo tee /sys/class/drm/card1/device/power_dpm_force_performance_level"
            ),
            "{refusal}"
        );
        assert!(!refusal.contains("card0"), "{refusal}");
    }

    #[test]
    fn the_tour_takes_its_own_terminals_session_alone() {
        let sessions = crate::testing::folder("tour-sessions");
        let write = |id: &str, text: &str| fs::write(sessions.join(id), text).unwrap();
        write(
            "3",
            "UID=1000\nUSER=someone\nACTIVE=1\nSTATE=active\nTYPE=tty\nCLASS=user\nSEAT=seat0\nVTNR=3\n",
        );
        let cgroup = "0::/user.slice/user-1000.slice/session-3.scope\n";
        assert_eq!(
            own_seat(cgroup, &sessions, 1000),
            Ok(Seat {
                session: "3".to_owned(),
                seat: "seat0".to_owned(),
                vt: Some(3),
            })
        );
        // Another user's session, or a scope of the user's manager, is no session of the tour's.
        assert!(own_seat(cgroup, &sessions, 1001).is_err());
        let in_a_scope = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/x.scope\n";
        assert!(
            own_seat(in_a_scope, &sessions, 1000)
                .unwrap_err()
                .contains("no logind session")
        );
        // A session over SSH has no seat; one where a compositor runs is of its type.
        write("4", "UID=1000\nACTIVE=1\nTYPE=tty\nCLASS=user\n");
        let ssh = "0::/user.slice/user-1000.slice/session-4.scope\n";
        assert!(
            own_seat(ssh, &sessions, 1000)
                .unwrap_err()
                .contains("no seat")
        );
        write(
            "5",
            "UID=1000\nACTIVE=1\nTYPE=wayland\nSEAT=seat0\nVTNR=2\n",
        );
        let wayland = "0::/user.slice/user-1000.slice/session-5.scope\n";
        assert!(
            own_seat(wayland, &sessions, 1000)
                .unwrap_err()
                .contains("type wayland")
        );
        // A terminal not shown.
        write("6", "UID=1000\nACTIVE=0\nTYPE=tty\nSEAT=seat0\nVTNR=4\n");
        let hidden = "0::/user.slice/user-1000.slice/session-6.scope\n";
        assert!(own_seat(hidden, &sessions, 1000).is_err());
        let seat = own_seat(cgroup, &sessions, 1000).unwrap();
        assert!(is_active(&sessions, &seat));
        write(
            "3",
            "UID=1000\nACTIVE=0\nTYPE=wayland\nSEAT=seat0\nVTNR=3\n",
        );
        assert!(!is_active(&sessions, &seat));
    }

    #[test]
    fn each_rival_takes_the_tours_session_on_the_display_and_nothing_of_the_owners() {
        let seat = Seat {
            session: "3".to_owned(),
            seat: "seat0".to_owned(),
            vt: Some(3),
        };
        let mut shell = crate::testing::shell();
        // The recipes that read their package's shipped configuration.
        let root = crate::testing::folder("tour-plans-shell");
        for (field, file, text) in [
            ("sway", "etc/sway/config", "set $mod Mod4\nbar {\n}\n"),
            ("mango", "etc/mango/config.conf", "animations=1\n"),
            ("umbriel", "share/umbriel/config.toml", "[general]\n"),
            ("hyprland", "share/hypr/hyprland.lua", "hl.config({})\n"),
        ] {
            let package = root.join(field);
            fs::create_dir_all(package.join(file).parent().unwrap()).unwrap();
            fs::write(package.join(file), text).unwrap();
            match field {
                "sway" => shell.sway = package,
                "mango" => shell.mango = package,
                "umbriel" => shell.umbriel = package,
                _ => shell.hyprland = package,
            }
        }
        let folders = sandbox::Folders::create(&crate::testing::folder("tour-plans")).unwrap();
        let owner = sandbox::Owner {
            runtime_folders: vec![PathBuf::from("/run/user/1000")],
        };
        let expected_program = [
            ("sway", "bin/sway", vec![]),
            ("dwl", "bin/dwl", vec![]),
            ("mango", "bin/mango", vec![]),
            ("umbriel", "bin/umbriel", vec![]),
            ("hyprland", "bin/start-hyprland", vec![]),
            ("niri", "bin/niri", vec![]),
            ("kwin", "bin/kwin_wayland", vec!["--drm"]),
            (
                "mutter",
                "bin/mutter",
                vec!["--display-server", "--wayland"],
            ),
            ("cosmic-comp", "bin/cosmic-comp", vec![]),
            ("gamescope", "bin/gamescope", vec!["--backend", "drm"]),
        ];
        for (rival, program, arguments) in expected_program {
            let recipe = recipes::tour_recipe(rival).unwrap();
            for configuration in Configuration::ALL {
                let plan = recipes::plan(
                    recipe,
                    configuration,
                    recipes::Display::Drm(&seat),
                    &shell,
                    &folders,
                    &recipes::Prepared::default(),
                )
                .unwrap_or_else(|e| panic!("{rival}: {e}"));
                assert!(
                    plan.program.ends_with(program),
                    "{rival}: {:?}",
                    plan.program
                );
                for argument in &arguments {
                    assert!(
                        plan.arguments.contains(&(*argument).to_owned()),
                        "{rival}: {argument}"
                    );
                }
                assert!(
                    plan.parent.is_none(),
                    "{rival} needs no parent on the display"
                );
                let environment =
                    sandbox::compositor_environment(&folders, &shell, &plan.path, &plan.variables);
                let value = |key: &str| {
                    environment
                        .iter()
                        .filter(|(name, _)| name == key)
                        .map(|(_, value)| value.as_str())
                        .collect::<Vec<_>>()
                };
                // The tour's own session, once, for libseat, KWin and Mutter alike.
                assert_eq!(value("XDG_SESSION_ID"), ["3"], "{rival}");
                assert_eq!(value("LIBSEAT_BACKEND"), ["logind"], "{rival}");
                assert_eq!(value("XDG_SEAT"), ["seat0"], "{rival}");
                // No headless backend, no render node forced, no display of the owner's.
                for key in [
                    "WLR_BACKENDS",
                    "WLR_RENDER_DRM_DEVICE",
                    "KWIN_DRM_DEVICES",
                    "WAYLAND_DISPLAY",
                    "DISPLAY",
                    "DBUS_SESSION_BUS_ADDRESS",
                    "DBUS_SYSTEM_BUS_ADDRESS",
                ] {
                    assert!(value(key).is_empty(), "{rival} has {key}");
                }
                for (key, value) in &environment {
                    assert!(
                        !owner.names_runtime_folder(value),
                        "{rival}: {key}={value} names the owner's runtime folder"
                    );
                }
                assert!(
                    !plan
                        .arguments
                        .iter()
                        .any(|a| a.contains("HEADLESS") || a == "--headless"),
                    "{rival}: {:?}",
                    plan.arguments
                );
                for (_, text) in &plan.files {
                    assert!(
                        !text.contains("HEADLESS-1"),
                        "{rival}'s configuration: {text}"
                    );
                }
            }
        }
        // Headless, as `headless` and the dry run run them, the tour-only rivals nest in dwl.
        for rival in ["niri", "cosmic-comp"] {
            let plan = recipes::plan(
                recipes::tour_recipe(rival).unwrap(),
                Configuration::EqualWork,
                recipes::Display::Headless,
                &shell,
                &folders,
                &recipes::Prepared::default(),
            )
            .unwrap();
            assert!(plan.parent.is_some(), "{rival}");
            assert!(
                !plan
                    .variables
                    .iter()
                    .any(|(key, _)| key == "XDG_SESSION_ID")
            );
        }
    }

    #[test]
    fn each_machine_has_its_rivals_and_the_tour_says_how_long_it_takes() {
        assert_eq!(Machine::Desktop.default_rivals().len(), 10);
        assert_eq!(
            Machine::Laptop.default_rivals(),
            ["dwl", "mango", "umbriel", "hyprland", "niri", "kwin"]
        );
        assert!(Machine::Deck.default_rivals().contains(&"gamescope"));
        let durations = Durations::STANDARD;
        // sway's rest: its start and stop, the still client, 5 s to settle, 60 s measured.
        assert_eq!(
            case_time("sway", "rest", &durations, PRESSES),
            Duration::from_secs(6 + 1 + 5 + 60)
        );
        // 310 keys at 70 ms each.
        assert_eq!(
            case_time("kwin", "latency", &durations, PRESSES),
            Duration::from_millis(6_000 + 2_000 + 310 * 70)
        );
        assert_eq!(
            case_time("gamescope", "rest", &durations, PRESSES),
            Duration::ZERO
        );
        assert_eq!(
            case_time("mutter", "motion", &durations, PRESSES),
            Duration::ZERO
        );
        assert!(not_run("gamescope", "motion").is_some());
        let all = TOUR_SCENARIOS;
        let desktop = tour_time(
            &Machine::Desktop.default_rivals(),
            &all,
            1,
            1,
            &durations,
            PRESSES,
        );
        let laptop = tour_time(
            &Machine::Laptop.default_rivals(),
            &all,
            1,
            1,
            &durations,
            PRESSES,
        );
        // The figures docs/owner-runs.md gives for each machine's default tour.
        let deck = tour_time(
            &Machine::Deck.default_rivals(),
            &all,
            1,
            1,
            &durations,
            PRESSES,
        );
        assert_eq!(
            [minutes(desktop), minutes(laptop), minutes(deck)],
            ["25 min 3 s", "16 min 22 s", "17 min 12 s"]
        );
        assert_eq!(
            tour_time(&["sway"], &all, 2, 3, &durations, PRESSES),
            tour_time(&["sway"], &all, 1, 1, &durations, PRESSES) * 6
        );
        // A hung case keeps the display twice its time and a minute at most.
        assert_eq!(
            time_limit(Duration::from_secs(77)),
            Duration::from_secs(214)
        );
        assert_eq!(minutes(Duration::from_secs(150)), "2 min 30 s");
        assert_eq!(minutes(Duration::from_secs(45)), "45 s");
    }

    #[test]
    fn the_machine_is_told_by_its_board_or_its_battery() {
        let sys = crate::testing::folder("tour-machine");
        assert_eq!(Machine::detect(&sys), Machine::Desktop);
        // A wireless mouse's battery makes no laptop.
        fs::create_dir_all(sys.join("class/power_supply/hidpp_battery_0")).unwrap();
        fs::write(
            sys.join("class/power_supply/hidpp_battery_0/type"),
            "Battery\n",
        )
        .unwrap();
        fs::write(
            sys.join("class/power_supply/hidpp_battery_0/scope"),
            "Device\n",
        )
        .unwrap();
        assert_eq!(Machine::detect(&sys), Machine::Desktop);
        fs::create_dir_all(sys.join("class/power_supply/BAT1")).unwrap();
        fs::write(sys.join("class/power_supply/BAT1/type"), "Battery\n").unwrap();
        assert_eq!(Machine::detect(&sys), Machine::Laptop);
        fs::create_dir_all(sys.join("class/dmi/id")).unwrap();
        fs::write(sys.join("class/dmi/id/product_name"), "Galileo\n").unwrap();
        assert_eq!(Machine::detect(&sys), Machine::Deck);
    }

    #[test]
    fn the_tours_cases_go_rival_by_rival_marked_as_the_tours() {
        let options = read(&["--compositor", "sway,kwin", "--scenario", "rest,latency"]).unwrap();
        let cases = cases(
            &["sway", "kwin"],
            &options,
            Some(crate::record::ON_THE_DISPLAY),
        );
        let labels: Vec<String> = cases.iter().map(|(case, _)| case.label()).collect();
        assert_eq!(
            labels,
            [
                "tour/sway/equal-work/rest-15",
                "tour/sway/equal-work/latency",
                "tour/kwin/equal-work/rest-15",
                "tour/kwin/equal-work/latency"
            ]
        );
    }
}
