//! Protocol traces of scripted clients, held as files
//! ([ADR-0139](../../../../docs/adr/0139-stage-2-in-three-gated-parts-2as-path-revised.md), step 1
//! and *Parity at every step*): what test clients send and receive in eight scenarios, recorded
//! on the clients' side, is compared with `tests/traces/` at every run, so that a change of Yuna's
//! protocol behaviour shows as a failing test and a diff of the lines that changed. Recorded on
//! the clients' side, a trace holds nothing of how the server is written, and stays valid once
//! Smithay is gone.
//!
//! A trace is recorded in a process of its own: this test binary run again on one test,
//! [`traced_process`], with `WAYLAND_DEBUG=client`, under which `wayland-backend`'s client side
//! prints on standard error every message it sends and every event it dispatches. It reads the
//! variable when a connection is made, so the variable must be set before the process starts:
//! set in this process, it would race with the other tests' threads, and their clients'
//! messages would mix with the scenario's. The test reads the child's standard error into typed
//! messages and writes them in the traces' own format ([`normalise::normalise`]): each client's
//! objects, each with its own messages in order, and only the orders between objects that the
//! protocol requires, for the interleaving of unrelated objects is the server's own.
//!
//! Each scenario is a script that waits for each answer it needs, so that every object's messages
//! are fixed. All eight run on the manual clock, in-process as the stories do, their outputs
//! created at a whole millisecond, so that times from the first frame callback are exact
//! ([`manual`]). Three also run against `yuna --headless` on the real clock, their times masked
//! ([`real`]).
//!
//! A change of behaviour made on purpose rewrites the files with
//! `YUNA_WRITE_TRACES=1 cargo test -p yuna --test headless traces::`, and the files' diff shows
//! it.

mod diff;
mod manual;
mod normalise;
mod output_client;
mod real;

#[path = "../common/application_environment.rs"]
mod application_environment;
#[path = "../common/dies_with_test.rs"]
mod dies_with_test;
#[path = "../common/yuna_run.rs"]
mod yuna_run;

use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use application_environment::{application_environment, shell_variable};
use dies_with_test::dies_with_test;
use diff::unified_diff;
use normalise::{Clock, normalise};

pub use normalise::mark_client;

/// One scenario: its name in its files' names, and its scripts on each clock.
struct Scenario {
    name: &'static str,
    manual: fn(),
    /// Against `yuna --headless`, for the scenarios that run there.
    real: Option<fn()>,
}

const WINDOW: Scenario = Scenario {
    name: "window",
    manual: manual::window,
    real: Some(real::window),
};
const POPUPS: Scenario = Scenario {
    name: "popups",
    manual: manual::popups,
    real: None,
};
const INPUT: Scenario = Scenario {
    name: "input",
    manual: manual::input,
    real: Some(real::input),
};
const SUBSURFACES: Scenario = Scenario {
    name: "subsurfaces",
    manual: manual::subsurfaces,
    real: None,
};
const SCALE: Scenario = Scenario {
    name: "scale",
    manual: manual::scale,
    real: None,
};
const OUTPUTS: Scenario = Scenario {
    name: "outputs",
    manual: manual::outputs,
    real: Some(real::outputs),
};
const CAPTURE: Scenario = Scenario {
    name: "capture",
    manual: manual::capture,
    real: None,
};
const CLIPBOARD: Scenario = Scenario {
    name: "clipboard",
    manual: manual::clipboard,
    real: None,
};

const SCENARIOS: [Scenario; 8] = [
    WINDOW,
    POPUPS,
    INPUT,
    SUBSURFACES,
    SCALE,
    OUTPUTS,
    CAPTURE,
    CLIPBOARD,
];

/// The variable that tells a [`traced_process`] which scenario to run: `<scenario>.<clock>`.
const RUN: &str = "YUNA_TRACE_RUN";
/// The variable that gives a [`traced_process`] its folder: its home, and the folder of the
/// `yuna --headless` it starts ([`yuna_folder`]).
const FOLDER: &str = "YUNA_TRACE_FOLDER";
/// The variable that rewrites the files held rather than compare with them.
const WRITE: &str = "YUNA_WRITE_TRACES";
/// The name of [`traced_process`] as the harness knows it.
const TRACED_PROCESS: &str = "traces::traced_process";

/// Not a check: the entry of the process a trace is recorded in, which runs the scenario
/// [`RUN`] names. Run by the harness without it, as every test is, it does nothing.
#[test]
fn traced_process() {
    let Some(run) = std::env::var_os(RUN) else {
        return;
    };
    let run = run.to_str().expect("the run's name is UTF-8");
    let (name, clock) = run.split_once('.').expect("<scenario>.<clock>");
    let scenario = SCENARIOS
        .iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("no scenario {name}"));
    match clock {
        "manual" => (scenario.manual)(),
        "real" => (scenario.real.expect("a scenario on the real clock"))(),
        _ => panic!("no clock {clock}"),
    }
}

/// The folder of the `yuna --headless` a [`traced_process`] starts, in its own folder.
fn yuna_folder() -> PathBuf {
    PathBuf::from(std::env::var_os(FOLDER).expect("a traced process's folder")).join("yuna")
}

/// Runs `scenario` on `clock` in a process of its own and gives its trace, after two lines that
/// say what it is. The process gets an application's environment, so that it reads neither the
/// machine's settings nor the user's, and a folder of its own, kept when it fails. A trace that
/// holds no message is refused, so that a printout that stopped coming, `WAYLAND_DEBUG` read no
/// more or written elsewhere, is never held as a file.
fn record(scenario: &Scenario, clock: Clock) -> String {
    static RUNS: AtomicU32 = AtomicU32::new(0);
    let folder = std::env::temp_dir().join(format!(
        "yuna-trace-{}-{}",
        std::process::id(),
        RUNS.fetch_add(1, Ordering::Relaxed)
    ));
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder.join("home"))
        .expect("the traced process's folder");
    let executable = std::env::current_exe().expect("the test binary's path");
    let mut command = Command::new(executable);
    command
        .args(["--exact", TRACED_PROCESS, "--nocapture", "--test-threads=1"])
        .env_clear()
        .envs(application_environment(&folder.join("home")))
        // The agents' seats' keymaps, for the input scenarios.
        .env("XKB_CONFIG_ROOT", shell_variable("XKB_CONFIG_ROOT"))
        .env(RUN, format!("{}.{}", scenario.name, clock.name()))
        .env(FOLDER, &folder)
        .env("WAYLAND_DEBUG", "client")
        .stdin(Stdio::null());
    dies_with_test(&mut command);
    let output = command.output().expect("the test binary runs again");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let log = folder.join("scenario.log");
        fs::write(&log, &output.stderr).expect("the scenario's log is written");
        panic!(
            "the scenario {} failed on the {} clock: {}; its folder is kept: {}\n{}",
            scenario.name,
            clock.name(),
            output.status,
            folder.display(),
            yuna_run::diagnosis(&folder.join("yuna/yuna.log"), &log, "scenario")
        );
    }
    let _ = fs::remove_dir_all(&folder);
    let trace = normalise(&stderr, clock).unwrap_or_else(|why| {
        panic!(
            "the trace of {} on the {} clock cannot be normalised: {why}",
            scenario.name,
            clock.name()
        )
    });
    assert!(
        trace
            .lines()
            .any(|line| line.starts_with("  -> ") || line.starts_with("  <- ")),
        "the trace of {} on the {} clock holds no message: did `wayland-backend` print none?",
        scenario.name,
        clock.name()
    );
    format!("{}{trace}", header(scenario, clock))
}

/// A trace's first lines: what it is, and how to read it.
fn header(scenario: &Scenario, clock: Clock) -> String {
    let times = match clock {
        Clock::Manual => "times in ms from the earliest frame callback",
        Clock::Real => "times masked",
    };
    format!(
        "# {} on the {} clock: each client's objects by interface and rank, each with its own \
         messages in order, and the event\n# of another object that follows one where the \
         protocol requires it; serials named by the event that carried them; {times}\n",
        scenario.name,
        clock.name()
    )
}

/// The file `scenario`'s trace on `clock` is held in.
fn held_path(scenario: &Scenario, clock: Clock) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/traces")
        .join(format!("{}.{}.trace", scenario.name, clock.name()))
}

/// Records `scenario` on `clock` and compares its trace with the one held, or writes it there
/// when [`WRITE`] is set. A difference fails with the diff of the lines that differ.
fn check(scenario: &Scenario, clock: Clock) {
    let trace = record(scenario, clock);
    let path = held_path(scenario, clock);
    if std::env::var_os(WRITE).is_some() {
        fs::write(&path, &trace).expect("the trace's file is written");
        return;
    }
    let held = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        held == trace,
        "the trace of {} on the {} clock differs from the one held in {}:\n{}\n\
         If the change is meant, rewrite the traces with \
         `YUNA_WRITE_TRACES=1 cargo test -p yuna --test headless traces::`, and read their diff.",
        scenario.name,
        clock.name(),
        path.display(),
        unified_diff(&held, &trace)
    );
}

/// Scenario (1): a window's life: mapped, configured and acknowledged, drawn with damage, called
/// back with presentation feedback, resized, fullscreen and back, closed.
#[test]
fn a_windows_life_on_the_manual_clock() {
    check(&WINDOW, Clock::Manual);
}

#[test]
fn a_windows_life_on_the_real_clock() {
    check(&WINDOW, Clock::Real);
}

/// Scenario (2): a popup with a grab, placed again, dismissed.
#[test]
fn popups_on_the_manual_clock() {
    check(&POPUPS, Clock::Manual);
}

/// Scenario (3): a seat's pointer and keyboard: enter, motion, button, key, modifiers, leave.
#[test]
fn pointer_and_keyboard_on_the_manual_clock() {
    check(&INPUT, Clock::Manual);
}

#[test]
fn pointer_and_keyboard_on_the_real_clock() {
    check(&INPUT, Clock::Real);
}

/// Scenario (4): subsurfaces.
#[test]
fn subsurfaces_on_the_manual_clock() {
    check(&SUBSURFACES, Clock::Manual);
}

/// Scenario (5): an output at scale 1.5: preferred and fractional scale, a viewport.
#[test]
fn scale_on_the_manual_clock() {
    check(&SCALE, Clock::Manual);
}

/// Scenario (6): outputs, as `wl_output` and `xdg-output` tell them; one added, then removed.
#[test]
fn outputs_on_the_manual_clock() {
    check(&OUTPUTS, Clock::Manual);
}

#[test]
fn outputs_on_the_real_clock() {
    check(&OUTPUTS, Clock::Real);
}

/// Scenario (7): an output captured through `ext-image-copy-capture-v1` into `wl_shm`.
#[test]
fn capture_on_the_manual_clock() {
    check(&CAPTURE, Clock::Manual);
}

/// Scenario (8): the clipboard from one client to another.
#[test]
fn clipboard_on_the_manual_clock() {
    check(&CLIPBOARD, Clock::Manual);
}

/// Each scenario on each of its clocks gives the same trace five times in a row: a trace that
/// changed with the machine's load or the time it ran at would fail here before it failed a
/// change. The scenarios run side by side, each on a thread of its own.
#[test]
fn every_trace_is_the_same_five_times_in_a_row() {
    let runs: Vec<(&Scenario, Clock)> = SCENARIOS
        .iter()
        .flat_map(|scenario| {
            let real = scenario.real.map(|_| (scenario, Clock::Real));
            std::iter::once((scenario, Clock::Manual)).chain(real)
        })
        .collect();
    let differences: Vec<String> = std::thread::scope(|scope| {
        let threads: Vec<_> = runs
            .iter()
            .map(|&(scenario, clock)| {
                scope.spawn(move || {
                    let first = record(scenario, clock);
                    (2..=5).find_map(|run| {
                        let again = record(scenario, clock);
                        (again != first).then(|| {
                            format!(
                                "{} on the {} clock, run {run} against run 1:\n{}",
                                scenario.name,
                                clock.name(),
                                unified_diff(&first, &again)
                            )
                        })
                    })
                })
            })
            .collect();
        threads
            .into_iter()
            .filter_map(|thread| thread.join().expect("a run does not panic"))
            .collect()
    });
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
