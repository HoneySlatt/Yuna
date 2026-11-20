//! The parity script's scenarios, and one run of one of them on one build.
//!
//! A run starts `yuna mcp --manual-clock` (under callgrind or DHAT, or alone: *native*), creates
//! its seat, and starts the lockstep client on the seat's socket itself, never through MCP's
//! `spawn`, so that the client is never a child of the measured process. Each *period* then
//! gives Yuna one refresh of work, always in this order: the scenario's MCP calls of that period
//! (pointer motions, a workspace's switch), the client's step (its commits, and a round trip so
//! that Yuna has read them), and one advance of the clock by the output's period, which draws
//! the frame. Nothing else runs: two builds given the same periods are given the same work, which
//! the run's counts check ([`Work`]).
//!
//! A run is a warm-up, `rest`'s counts, the measured periods, `rest`'s counts again; a native run
//! then lets Yuna come to rest and reads its wakes over [`REST_WINDOW`] of real time and its
//! memory.
//!
//! **Real time.** On the manual clock, Yuna's own deadlines run only when the script advances the
//! clock: what valgrind counts follows the periods, not the machine's speed. A timer on real time
//! would not: it fires as often as the run lasts, and valgrind's runs last 30 to 100 times longer
//! than native ones, longer still on a slower or busier machine, so its cost would grow with the
//! machine's slowness. No such timer runs in a parity run today (`rest` counts none at rest); the
//! `wake` canary, one each second, moved `full`'s and `motion`'s frame figures by 1 to 1.6 % on the
//! owner's desktop. A Yuna that adds one is flagged by its wakes at rest, and its frame figures
//! then measure the machine too.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::launch::UNSHARE_OPTIONS;
use super::lockstep::Counts;
use super::mcp::{CallError, Mcp};
use super::valgrind::{self, Tool};
use crate::procfs::{self, Proc};

/// One refresh at 240 Hz, the rate of every scenario's output, in nanoseconds, rounded up so that
/// an advance never stops short of a boundary.
pub const PERIOD_NS: u64 = 4_166_667;
/// The real time over which a native run counts Yuna's wakes once at rest.
pub const REST_WINDOW: Duration = Duration::from_secs(2);
/// llvmpipe's rasterizing threads: none, so that it draws on the thread that asks. valgrind runs
/// one thread at a time, and a thread that waits for another by spinning counts instructions that
/// depend on when it is switched out; and Yuna's main thread then never sleeps waiting for them.
/// The same work is drawn: the instructions do not depend on the threads (`m2.1-vg-trial`, 4
/// and 16).
pub const LLVMPIPE_THREADS: u32 = 0;
/// How long one MCP call or one step may take, under valgrind's slowest tool, before the run
/// fails: a first frame compiles llvmpipe's shaders, seconds natively, minutes under DHAT.
const CALL_TIMEOUT: Duration = Duration::from_secs(900);
/// How long Yuna may take to stop once its input closes: DHAT writes its file then.
const STOP_TIMEOUT: Duration = Duration::from_secs(900);
/// The signal that ends a DHAT run: one Yuna does not handle, which ends it at once.
const HEAP_KEPT_SIGNAL: rustix::process::Signal = rustix::process::Signal::USR1;
/// How long a native run waits for Yuna's main thread to sleep before a message.
const ASLEEP_TIMEOUT: Duration = Duration::from_secs(10);
/// The periods a native run gives Yuna to come to rest after its measured ones, at most: a motion
/// ends within them.
const SETTLE_PERIODS: u32 = 480;

/// What a scenario does each period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing but the advance: what an advance costs on its own.
    Idle,
    /// One window's 64 × 64 square changes.
    Square,
    /// Every window changes whole.
    Full,
    /// A workspace switch, then a window moved to the other workspace, both ways, on springs.
    Motion,
    /// The pointer moves [`POINTER_MOTIONS`] times across the windows.
    Input,
}

/// The pointer's motions in a period of `input`.
pub const POINTER_MOTIONS: u32 = 8;
/// The periods each step of `motion`'s cycle lasts: 100 ms at 240 Hz.
pub const MOTION_STEP: u32 = 24;

/// A scenario: an output, windows, and what each period does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scenario {
    /// Its name.
    pub name: &'static str,
    /// What it shows, in a few words.
    pub summary: &'static str,
    /// The output's width and height in pixels and its scale; its rate is 240 Hz.
    pub output: (u32, u32, f64),
    /// The lockstep client's windows.
    pub windows: usize,
    /// Whether Yuna moves windows on springs (`--animations`).
    pub animations: bool,
    /// Each period's work.
    pub action: Action,
    /// The measured periods of a native run, and of the shorter of the two runs each valgrind
    /// tool makes; the longer makes twice as many.
    pub periods: u32,
}

/// Every scenario, in the order the report gives them.
pub const SCENARIOS: [Scenario; 7] = [
    Scenario {
        name: "idle",
        summary: "two still windows; each period an advance alone, what the others subtract",
        output: (1920, 1080, 1.0),
        windows: 2,
        animations: false,
        action: Action::Idle,
        periods: 120,
    },
    Scenario {
        name: "partial",
        summary: "1080p, two windows, one 64 × 64 square changes each frame",
        output: (1920, 1080, 1.0),
        windows: 2,
        animations: false,
        action: Action::Square,
        periods: 120,
    },
    Scenario {
        name: "full",
        summary: "1080p, two windows, both change whole each frame",
        output: (1920, 1080, 1.0),
        windows: 2,
        animations: false,
        action: Action::Full,
        periods: 40,
    },
    Scenario {
        name: "motion",
        summary: "1080p, three windows on two workspaces: a switch and a window moved, both ways, \
                  on springs",
        output: (1920, 1080, 1.0),
        windows: 3,
        animations: true,
        action: Action::Motion,
        periods: 4 * MOTION_STEP,
    },
    Scenario {
        name: "scaled",
        summary: "4K at scale 1.5, two windows at the preferred scale, one 64 × 64 square \
                  changes each frame",
        output: (3840, 2160, 1.5),
        windows: 2,
        animations: false,
        action: Action::Square,
        periods: 120,
    },
    Scenario {
        name: "many",
        summary: "1080p, 30 windows, one 64 × 64 square changes each frame",
        output: (1920, 1080, 1.0),
        windows: 30,
        animations: false,
        action: Action::Square,
        periods: 120,
    },
    Scenario {
        name: "input",
        summary: "1080p, two still windows, the pointer moves 8 times a period across them",
        output: (1920, 1080, 1.0),
        windows: 2,
        animations: false,
        action: Action::Input,
        periods: 60,
    },
];

/// The scenario named `name`.
pub fn named(name: &str) -> Option<&'static Scenario> {
    SCENARIOS.iter().find(|scenario| scenario.name == name)
}

/// The periods of warm-up before the measured ones: the windows placed, a first frame drawn,
/// llvmpipe's shaders compiled; for `motion`, one whole cycle, after which each is the same.
fn warm_up(scenario: &Scenario) -> u32 {
    match scenario.action {
        Action::Motion => 4 * MOTION_STEP + 8,
        _ => 8,
    }
}

/// How a run measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Instrument {
    /// Yuna alone: memory, wakes, context switches.
    Native,
    /// Under a valgrind tool.
    Valgrind(Tool),
}

impl Instrument {
    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Instrument::Native => "native",
            Instrument::Valgrind(tool) => tool.name(),
        }
    }
}

/// What one side runs with.
#[derive(Debug, Clone)]
pub struct Build {
    /// Its `yuna`, copied out of its target folder.
    pub yuna: PathBuf,
    /// The variables of its own development shell that Yuna reads: its Mesa, its keymaps, its
    /// D-Bus daemon, its `PATH`.
    pub environment: Vec<(String, String)>,
    /// `YUNA_PARITY_CANARY`'s value, for a head built with a canary.
    pub canary: Option<String>,
}

/// What the runs share.
#[derive(Debug, Clone)]
pub struct Tools {
    /// valgrind.
    pub valgrind: PathBuf,
    /// This program, whose `client lockstep` is the client.
    pub program: PathBuf,
    /// Where the runs' sockets go: a short path, under Unix's 108 bytes.
    pub runtime: PathBuf,
    /// util-linux's `unshare`, which makes each run's namespaces.
    pub unshare: PathBuf,
}

/// The work a run did in its measured periods, which two builds must share for their costs to be
/// compared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Work {
    /// Frames Yuna drew.
    pub frames: u64,
    /// Frames by cause, from `rest`.
    pub causes: BTreeMap<String, u64>,
    /// The client's commits Yuna counted.
    pub commits: u64,
    /// What the client counted.
    pub client: Counts,
}

/// What one run measured.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// Its real time, in seconds.
    pub seconds: f64,
    /// Why it failed, if it did: its figures are then void.
    pub error: Option<String>,
    /// Its measured periods' work.
    pub work: Work,
    /// Its raw readings, by name: see [`run`].
    pub readings: BTreeMap<String, f64>,
    /// What a reader needs beside them.
    pub notes: Vec<String>,
    /// For an operation's run, its program's digest of the work it did.
    pub digest: Option<String>,
}

/// A run's readings, by name:
///
/// - callgrind: `instructions` (every traced Yuna process) and `instructions_yuna` (in the
///   build's `yuna` binary);
/// - DHAT: `blocks`, `blocks_own` (through the binary's own code), `bytes`, `end_bytes` (what
///   the heap held when Yuna was stopped);
/// - native: over the measured periods, `wakes` and `wakes:<source>` from `rest`, `switches`
///   (voluntary context switches of Yuna's own threads); at rest, over [`REST_WINDOW`],
///   `rest_wakes` and `rest_switches`; after it, `pss_anon_kib` and `pss_file_kib`.
pub fn run(
    scenario: &Scenario,
    instrument: Instrument,
    periods: u32,
    build: &Build,
    tools: &Tools,
    folder: &Path,
    runtime_index: usize,
) -> Outcome {
    let started = Instant::now();
    let mut outcome = Outcome::default();
    let result = Run::start(scenario, instrument, build, tools, folder, runtime_index).and_then(
        |mut run| {
            let measured = run.measure(scenario, instrument, periods, &mut outcome);
            let stopped = run.stop();
            measured.and(stopped)
        },
    );
    if let Err(error) = result {
        outcome.error = Some(error);
    } else if let Instrument::Valgrind(tool) = instrument
        && let Err(error) = read_valgrind(tool, folder, &build.yuna, &mut outcome)
    {
        outcome.error = Some(error);
    }
    outcome.seconds = started.elapsed().as_secs_f64();
    outcome
}

/// Sums the files `tool` wrote for every process that ran the build's `yuna`.
fn read_valgrind(
    tool: Tool,
    folder: &Path,
    yuna: &Path,
    outcome: &mut Outcome,
) -> Result<(), String> {
    let files = valgrind::files(folder, tool);
    let mut processes = 0;
    let mut add = |key: &str, value: u64| {
        *outcome.readings.entry(key.to_owned()).or_default() += value as f64;
    };
    for file in &files {
        let text = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let named = file.display();
        match tool {
            Tool::Callgrind => {
                let read = valgrind::read_callgrind(&text).map_err(|e| format!("{named}: {e}"))?;
                if !valgrind::runs(&read.command, yuna) {
                    outcome
                        .notes
                        .push(format!("not Yuna, left out: {}", read.command));
                    continue;
                }
                add("instructions", read.total);
                add(
                    "instructions_yuna",
                    read.by_object
                        .get(&*yuna.to_string_lossy())
                        .copied()
                        .unwrap_or(0),
                );
            }
            Tool::Dhat => {
                let read = valgrind::read_dhat(&text, yuna).map_err(|e| format!("{named}: {e}"))?;
                if !valgrind::runs(&read.command, yuna) {
                    outcome
                        .notes
                        .push(format!("not Yuna, left out: {}", read.command));
                    continue;
                }
                add("blocks", read.blocks);
                add("blocks_own", read.own_blocks);
                add("bytes", read.bytes);
                add("end_bytes", read.end_bytes);
            }
        }
        processes += 1;
    }
    if processes == 0 {
        return Err(format!("{} wrote no file of Yuna's", tool.name()));
    }
    outcome
        .readings
        .insert("processes".to_owned(), f64::from(processes));
    Ok(())
}

/// A running Yuna, its MCP client and its lockstep client.
struct Run {
    /// `unshare`, whose child, the launcher, runs Yuna and the client in the run's namespaces.
    namespace: Child,
    /// The build's `yuna`: the processes measured natively run it.
    yuna: PathBuf,
    mcp: Option<Mcp>,
    /// The client's socket, to the script.
    client: Option<UnixStream>,
    client_lines: Receiver<String>,
    client_counts: Counts,
    seat: u64,
    /// The client's windows by MCP id, in the client's order: `parity 1` first.
    window_ids: Vec<u64>,
    /// Which window the `square` steps change, from 1.
    square_window: usize,
    period: u64,
    /// How far the clock could advance at its last reply, in nanoseconds.
    room: u64,
    /// In a native run, Yuna's process, whose main thread the script lets sleep before each
    /// message.
    native_pid: Option<u32>,
    /// In a DHAT run, the process that runs Yuna under DHAT, which the script stops with
    /// [`HEAP_KEPT_SIGNAL`].
    dhat_pid: Option<u32>,
}

impl Run {
    fn start(
        scenario: &Scenario,
        instrument: Instrument,
        build: &Build,
        tools: &Tools,
        folder: &Path,
        runtime_index: usize,
    ) -> Result<Run, String> {
        let runtime = tools.runtime.join(format!("r{runtime_index}"));
        let _ = fs::remove_dir_all(&runtime);
        fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&runtime)
            .map_err(|e| format!("{}: {e}", runtime.display()))?;
        for sub in ["home", "tmp"] {
            fs::create_dir_all(folder.join(sub))
                .map_err(|e| format!("{}: {e}", folder.join(sub).display()))?;
        }
        let control_path = runtime.join("control");
        let control = UnixListener::bind(&control_path)
            .map_err(|e| format!("{}: {e}", control_path.display()))?;
        let mut command_line: Vec<String> = vec![tools.unshare.display().to_string()];
        command_line.extend(UNSHARE_OPTIONS.iter().map(|o| (*o).to_owned()));
        command_line.extend([
            "--".to_owned(),
            tools.program.display().to_string(),
            "parity".to_owned(),
            "launch".to_owned(),
            "--windows".to_owned(),
            scenario.windows.to_string(),
            "--control".to_owned(),
            control_path.display().to_string(),
            "--client-log".to_owned(),
            folder.join("client.log").display().to_string(),
            "--".to_owned(),
        ]);
        if let Instrument::Valgrind(tool) = instrument {
            command_line.extend(valgrind::command(&tools.valgrind, tool, folder));
        }
        command_line.extend([
            build.yuna.display().to_string(),
            "mcp".to_owned(),
            "--manual-clock".to_owned(),
        ]);
        if scenario.animations {
            command_line.push("--animations".to_owned());
        }
        let log = File::create(folder.join("yuna.log")).map_err(|e| format!("yuna.log: {e}"))?;
        let mut command = Command::new(&command_line[0]);
        command
            .args(&command_line[1..])
            .env_clear()
            .envs(build.environment.iter().map(|(k, v)| (k, v)))
            .env("HOME", folder.join("home"))
            .env("TMPDIR", folder.join("tmp"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("YUNA_LOG", "info")
            .env("LP_NUM_THREADS", LLVMPIPE_THREADS.to_string())
            // No shader read from an earlier run's cache. A filled cache does not spare a run
            // LLVM's work at Yuna's start, and its reads make the allocations vary
            // (`figures.rs`, the total's floors).
            .env("MESA_SHADER_CACHE_DISABLE", "true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log)
            .process_group(0);
        if let Some(canary) = &build.canary {
            command.env("YUNA_PARITY_CANARY", canary);
        }
        let mut namespace = command
            .spawn()
            .map_err(|e| format!("{}: {e}", command_line[0]))?;
        let (Some(input), Some(output)) = (namespace.stdin.take(), namespace.stdout.take()) else {
            kill_group(&mut namespace);
            return Err("Yuna's standard input or output".to_owned());
        };
        let mut run = Run {
            namespace,
            yuna: build.yuna.clone(),
            mcp: Some(Mcp::new(input, output)),
            client: None,
            client_lines: mpsc::channel().1,
            client_counts: Counts::default(),
            seat: 0,
            window_ids: Vec::new(),
            square_window: 1,
            period: PERIOD_NS,
            room: 0,
            native_pid: None,
            dhat_pid: None,
        };
        if let Err(error) = run.connect(scenario, &control) {
            run.kill();
            return Err(error);
        }
        if matches!(
            instrument,
            Instrument::Native | Instrument::Valgrind(Tool::Dhat)
        ) {
            let found = measured_process(
                &Proc::system(),
                run.namespace.id(),
                &build.yuna,
                &tools.program,
            );
            let Some(pid) = found else {
                run.kill();
                return Err("no process of the run runs its yuna".to_owned());
            };
            if instrument == Instrument::Native {
                run.native_pid = Some(pid);
            } else {
                run.dhat_pid = Some(pid);
            }
        }
        Ok(run)
    }

    /// Creates the seat, takes the client's connection to the script, names it the seat's
    /// socket, and waits for its windows to be ready.
    fn connect(&mut self, scenario: &Scenario, control: &UnixListener) -> Result<(), String> {
        let (width, height, scale) = scenario.output;
        let seat = self.call(
            "seat",
            json!({"action": "create", "width": width, "height": height, "rate": 240,
                   "scale": scale}),
        )?;
        let (Some(socket), Some(seat_id)) = (
            seat.get("socket").and_then(Value::as_str),
            seat.get("seat").and_then(Value::as_u64),
        ) else {
            return Err(format!("seat create gave no socket or seat: {seat}"));
        };
        self.seat = seat_id;
        let mut client = accept(control, CALL_TIMEOUT)?;
        writeln!(client, "wayland {socket}").map_err(|e| format!("the client's socket: {e}"))?;
        let reader = client
            .try_clone()
            .map_err(|e| format!("the client's socket: {e}"))?;
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        self.client = Some(client);
        self.client_lines = lines;
        self.wait_ready(scenario.windows)
    }

    fn mcp(&mut self) -> &mut Mcp {
        self.mcp
            .as_mut()
            .expect("the MCP client lives until the run stops")
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Result<Value, String> {
        self.wait_until_asleep()?;
        self.mcp()
            .call(tool, arguments, CALL_TIMEOUT)
            .map_err(|e| e.to_string())
    }

    /// In a native run, waits until Yuna's main thread sleeps, waiting for its next event: a
    /// message sent while it still runs would find it awake, and its context switches would count
    /// the race with the machine's load rather than its own work. Each message then wakes it once.
    fn wait_until_asleep(&self) -> Result<(), String> {
        match self.native_pid {
            Some(pid) => wait_until_asleep(pid),
            None => Ok(()),
        }
    }

    fn wait_ready(&mut self, windows: usize) -> Result<(), String> {
        let line = self.client_line()?;
        if line.trim() != format!("ready {windows}") {
            return Err(format!("the lockstep client said {line:?}, not ready"));
        }
        Ok(())
    }

    fn client_line(&mut self) -> Result<String, String> {
        match self.client_lines.recv_timeout(CALL_TIMEOUT) {
            Ok(line) => Ok(line),
            Err(RecvTimeoutError::Timeout) => Err(format!(
                "the lockstep client said nothing in {} s",
                CALL_TIMEOUT.as_secs()
            )),
            Err(RecvTimeoutError::Disconnected) => {
                Err("the lockstep client ended (client.log says why)".to_owned())
            }
        }
    }

    /// Tells the client `command` and reads its answer.
    fn step(&mut self, command: &str) -> Result<(), String> {
        self.wait_until_asleep()?;
        let client = self
            .client
            .as_mut()
            .ok_or("the lockstep client's socket is closed")?;
        writeln!(client, "{command}").map_err(|e| format!("the lockstep client: {e}"))?;
        let line = self.client_line()?;
        self.client_counts =
            Counts::parse(&line).ok_or_else(|| format!("the lockstep client answered {line:?}"))?;
        Ok(())
    }

    /// Advances the clock by one period. The clock never runs ahead of real time: natively, and
    /// in periods that draw nothing under valgrind, Yuna is faster than 240 Hz, and an advance
    /// asked too early would be refused, an MCP call more in one run than in another. So the wait
    /// comes first: from the room the last reply gave, real time less the clock's, which only
    /// grows while the script waits, until it holds a period. A refusal is then an error.
    fn advance(&mut self) -> Result<(), String> {
        let by = self.period;
        if self.room < by {
            std::thread::sleep(Duration::from_nanos(by - self.room + 500_000));
        }
        self.wait_until_asleep()?;
        let clock = match self.mcp().call(
            "clock",
            json!({"action": "advance", "by_ns": by}),
            CALL_TIMEOUT,
        ) {
            Ok(clock) => clock,
            Err(CallError::TooEarly(room)) => {
                return Err(format!(
                    "the clock refused an advance its room allowed: {room} ns of room"
                ));
            }
            Err(error) => return Err(error.to_string()),
        };
        self.note_room(&clock)
    }

    /// Keeps the room a `clock` reply gives: how far the clock may advance.
    fn note_room(&mut self, clock: &Value) -> Result<(), String> {
        let (Some(real), Some(now)) = (
            clock.get("real_now_ns").and_then(Value::as_u64),
            clock.get("now_ns").and_then(Value::as_u64),
        ) else {
            return Err(format!("clock gave no time: {clock}"));
        };
        self.room = real.saturating_sub(now);
        Ok(())
    }

    /// The client's windows' MCP ids, in its order, once Yuna lists them all.
    fn find_windows(&mut self, count: usize) -> Result<(), String> {
        let listed = self.call("windows", json!({"seat": self.seat}))?;
        let mut ids = vec![None; count];
        for window in listed
            .get("windows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let title = window.get("title").and_then(Value::as_str).unwrap_or("");
            let id = window.get("id").and_then(Value::as_u64);
            if let Some(n) = title
                .strip_prefix("parity ")
                .and_then(|n| n.parse::<usize>().ok())
                && let Some(slot) = n.checked_sub(1).and_then(|n| ids.get_mut(n))
            {
                *slot = id;
            }
        }
        self.window_ids = ids
            .into_iter()
            .collect::<Option<Vec<u64>>>()
            .ok_or("Yuna does not list every window of the client")?;
        // The square changes in the first window Yuna draws: one off the view would draw nothing.
        self.square_window = listed
            .get("windows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|window| window.get("rect").is_some_and(|rect| !rect.is_null()))
            .filter_map(|window| window.get("title").and_then(Value::as_str))
            .filter_map(|title| title.strip_prefix("parity ")?.parse::<usize>().ok())
            .min()
            .ok_or("Yuna draws none of the client's windows")?;
        Ok(())
    }

    /// One period of `scenario`: its MCP calls, the client's step, the advance. `index` counts the
    /// periods since the warm-up began.
    fn period(&mut self, scenario: &Scenario, index: u32) -> Result<(), String> {
        let step = match scenario.action {
            Action::Idle | Action::Motion => "step none".to_owned(),
            Action::Square => format!("step square {}", self.square_window),
            Action::Full => "step full".to_owned(),
            Action::Input => {
                let (width, height, scale) = scenario.output;
                let logical_width = f64::from(width) / scale;
                for motion in 0..POINTER_MOTIONS {
                    // Strides of 97 cross both windows, in and out, never twice the same place in a
                    // row.
                    let n = f64::from(index * POINTER_MOTIONS + motion);
                    let x = (n * 97.0) % logical_width + 0.5;
                    let y = f64::from(height) / scale / 2.0;
                    self.call(
                        "pointer",
                        json!({"action": "move", "seat": self.seat, "x": x, "y": y}),
                    )?;
                }
                "step none".to_owned()
            }
        };
        if scenario.action == Action::Motion && index.is_multiple_of(MOTION_STEP) {
            self.motion_step(index / MOTION_STEP)?;
        }
        self.step(&step)?;
        self.advance()
    }

    /// `motion`'s cycle of four steps, each [`MOTION_STEP`] periods: workspace `two` shown,
    /// window 1 moved to it, workspace `one` shown, window 1 moved back.
    fn motion_step(&mut self, step: u32) -> Result<(), String> {
        let first = self.window_ids[0];
        let (action, target) = match step % 4 {
            0 => ("focus", "two"),
            1 => ("move_window", "two"),
            2 => ("focus", "one"),
            _ => ("move_window", "one"),
        };
        let arguments = if action == "focus" {
            json!({"action": "focus", "seat": self.seat, "name": target})
        } else {
            json!({"action": "move_window", "seat": self.seat, "window": first,
                   "workspace": target})
        };
        self.call("workspaces", arguments).map(|_| ())
    }

    /// `motion`'s two workspaces: windows 1 and 2 on `one`, 3 on `two`, `one` shown.
    fn set_up_motion(&mut self) -> Result<(), String> {
        let seat = self.seat;
        for name in ["one", "two"] {
            self.call(
                "workspaces",
                json!({"action": "create", "seat": seat, "name": name}),
            )?;
        }
        let ids = self.window_ids.clone();
        for (n, id) in ids.iter().enumerate() {
            let workspace = if n < 2 { "one" } else { "two" };
            self.call(
                "workspaces",
                json!({"action": "move_window", "seat": seat, "window": id,
                       "workspace": workspace}),
            )?;
        }
        self.call(
            "workspaces",
            json!({"action": "focus", "seat": seat, "name": "one"}),
        )
        .map(|_| ())
    }

    fn measure(
        &mut self,
        scenario: &Scenario,
        instrument: Instrument,
        periods: u32,
        outcome: &mut Outcome,
    ) -> Result<(), String> {
        let clock = self.call("clock", json!({"action": "now"}))?;
        self.note_room(&clock)?;
        // Two periods with nothing asked, so that Yuna places and draws the windows it was given.
        for _ in 0..2 {
            self.step("step none")?;
            self.advance()?;
        }
        self.find_windows(scenario.windows)?;
        if scenario.action == Action::Motion {
            self.set_up_motion()?;
        }
        for index in 0..warm_up(scenario) {
            self.period(scenario, index)?;
        }
        let first = warm_up(scenario);
        let root = self.namespace.id();
        let yuna = self.yuna.clone();
        let switches = || own_switches(&Proc::system(), root, &yuna);
        let native = instrument == Instrument::Native;
        let rest_before = self.call("rest", json!({}))?;
        let client_before = self.client_counts;
        let switches_before = if native { switches() } else { 0 };
        for index in first..first + periods {
            self.period(scenario, index)?;
        }
        let switches_after = if native { switches() } else { 0 };
        let rest_after = self.call("rest", json!({}))?;
        outcome.work = work(&rest_before, &rest_after)?;
        outcome.work.client = difference(self.client_counts, client_before);
        // Motion's figures are compared only where motion drew: a cause `rest` does not list
        // drew nothing.
        if scenario.action == Action::Motion
            && outcome.work.causes.get("motion").copied().unwrap_or(0) == 0
        {
            return Err("no frame of the measured periods was drawn for a motion".to_owned());
        }
        if !native {
            return Ok(());
        }
        let readings = &mut outcome.readings;
        readings.insert(
            "switches".to_owned(),
            switches_after.saturating_sub(switches_before) as f64,
        );
        for (source, wakes) in wakes_by_source(&rest_before, &rest_after)? {
            readings.insert(format!("wakes:{source}"), wakes as f64);
        }
        readings.insert(
            "wakes".to_owned(),
            count(&rest_after, "wakes")?.saturating_sub(count(&rest_before, "wakes")?) as f64,
        );
        self.come_to_rest()?;
        let switches_before = switches();
        let window = self.call("rest", json!({"wait_ms": REST_WINDOW.as_millis() as u64}))?;
        let switches_after = switches();
        let readings = &mut outcome.readings;
        readings.insert("rest_wakes".to_owned(), count(&window, "wakes")? as f64);
        readings.insert(
            "rest_switches".to_owned(),
            switches_after.saturating_sub(switches_before) as f64,
        );
        let (anon, file) = memory(&Proc::system(), root, &yuna);
        readings.insert("pss_anon_kib".to_owned(), anon as f64);
        readings.insert("pss_file_kib".to_owned(), file as f64);
        Ok(())
    }

    /// Advances, a period at a time, until Yuna arms no deadline: at rest. Fails if it never does.
    fn come_to_rest(&mut self) -> Result<(), String> {
        for _ in 0..SETTLE_PERIODS {
            let clock = self.call("clock", json!({"action": "now"}))?;
            self.note_room(&clock)?;
            if clock.get("next_deadline_ns").is_some_and(Value::is_null) {
                return Ok(());
            }
            self.step("step none")?;
            self.advance()?;
        }
        Err(format!(
            "a deadline stays armed after {SETTLE_PERIODS} periods without work: not at rest"
        ))
    }

    /// Ends Yuna by closing its input, and waits for it: the launcher then ends the client, which
    /// stays connected and idle until Yuna has stopped, so that Yuna's last work is the same in
    /// every run.
    fn stop(mut self) -> Result<(), String> {
        let mut expected = 0;
        if let Some(pid) = self.dhat_pid {
            // A DHAT run ends by a signal Yuna does not handle, once Yuna sleeps after the last
            // `rest`: it then frees nothing, and DHAT's file holds what its heap still held, a
            // leak among it. Closing its input would end it cleanly, every byte freed first.
            wait_until_asleep(pid)?;
            if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
                rustix::process::kill_process(pid, HEAP_KEPT_SIGNAL)
                    .map_err(|e| format!("signalling Yuna: {e}"))?;
            }
            // The launcher's status for a command a signal ended.
            expected = 128 + HEAP_KEPT_SIGNAL.as_raw();
        }
        if let Some(mcp) = self.mcp.take() {
            mcp.close();
        }
        let ended = wait(&mut self.namespace, STOP_TIMEOUT);
        drop(self.client.take());
        match ended {
            Some(status) if status.code() == Some(expected) => Ok(()),
            Some(status) => Err(format!("Yuna ended with {status}")),
            None => {
                kill_group(&mut self.namespace);
                Err(format!(
                    "Yuna did not stop in {} s once its input closed",
                    STOP_TIMEOUT.as_secs()
                ))
            }
        }
    }

    fn kill(mut self) {
        kill_group(&mut self.namespace);
    }
}

/// Takes the one connection `listener` expects, within `timeout`.
fn accept(listener: &UnixListener, timeout: Duration) -> Result<UnixStream, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("the client's socket: {e}"))?;
    let until = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|e| format!("the client's socket: {e}"))?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > until {
                    return Err(format!(
                        "the lockstep client did not connect in {} s (client.log says why)",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("the client's socket: {error}")),
        }
    }
}

/// Waits until the main thread of process `pid` sleeps, for [`ASLEEP_TIMEOUT`] at most.
fn wait_until_asleep(pid: u32) -> Result<(), String> {
    let until = Instant::now() + ASLEEP_TIMEOUT;
    loop {
        let stat = fs::read_to_string(format!("/proc/{pid}/task/{pid}/stat"))
            .map_err(|e| format!("Yuna's main thread: {e}"))?;
        let state = stat
            .rfind(')')
            .and_then(|at| stat[at + 1..].split_whitespace().next());
        if state == Some("S") {
            return Ok(());
        }
        if Instant::now() > until {
            return Err(format!(
                "Yuna's main thread did not sleep in {} s",
                ASLEEP_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_micros(50));
    }
}

/// Waits up to `timeout` for `child` to end.
fn wait(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let until = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
            _ => return None,
        }
    }
}

/// Kills `child`'s process group, which it leads, and reaps it.
fn kill_group(child: &mut Child) {
    if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    let _ = child.wait();
}

/// A count of a `rest` reply; a reply without it fails the run, never reads as 0.
fn count(rest: &Value, key: &str) -> Result<u64, String> {
    rest.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("rest gave no {key}: {rest}"))
}

/// `rest`'s `sources` or `draw_causes`, by name; a list or an entry without its fields fails the
/// run. A source or a cause `rest` does not list has counted nothing.
fn named_counts(
    rest: &Value,
    list: &str,
    name: &str,
    value: &str,
) -> Result<BTreeMap<String, u64>, String> {
    rest.get(list)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("rest gave no {list}: {rest}"))?
        .iter()
        .map(|entry| {
            match (
                entry.get(name).and_then(Value::as_str),
                entry.get(value).and_then(Value::as_u64),
            ) {
                (Some(key), Some(count)) => Ok((key.to_owned(), count)),
                _ => Err(format!(
                    "an entry of rest's {list} without {name} or {value}: {entry}"
                )),
            }
        })
        .collect()
}

/// The wakes of each source between two `rest` replies, since Yuna started both.
fn wakes_by_source(before: &Value, after: &Value) -> Result<BTreeMap<String, u64>, String> {
    let before = named_counts(before, "sources", "source", "wakes")?;
    Ok(named_counts(after, "sources", "source", "wakes")?
        .into_iter()
        .map(|(source, wakes)| {
            let earlier = before.get(&source).copied().unwrap_or(0);
            (source, wakes.saturating_sub(earlier))
        })
        .filter(|(_, wakes)| *wakes > 0)
        .collect())
}

/// The work between two `rest` replies: frames, their causes, and the clients' commits. The
/// run's one client is the lockstep client; Yuna names it by its process id in the run's PID
/// namespace, which the script does not know.
fn work(before: &Value, after: &Value) -> Result<Work, String> {
    let causes_before = named_counts(before, "draw_causes", "cause", "frames")?;
    let causes = named_counts(after, "draw_causes", "cause", "frames")?
        .into_iter()
        .map(|(cause, frames)| {
            let earlier = causes_before.get(&cause).copied().unwrap_or(0);
            (cause, frames.saturating_sub(earlier))
        })
        .collect();
    let commits = |rest: &Value| -> Result<u64, String> {
        rest.get("clients")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("rest gave no clients: {rest}"))?
            .iter()
            .map(|entry| {
                entry
                    .get("commits")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("a client of rest's without commits: {entry}"))
            })
            .sum()
    };
    Ok(Work {
        frames: count(after, "frames")?.saturating_sub(count(before, "frames")?),
        causes,
        commits: commits(after)?.saturating_sub(commits(before)?),
        client: Counts::default(),
    })
}

fn difference(after: Counts, before: Counts) -> Counts {
    Counts {
        commits: after.commits.saturating_sub(before.commits),
        callbacks: after.callbacks.saturating_sub(before.callbacks),
        pointer: after.pointer.saturating_sub(before.pointer),
        configures: after.configures.saturating_sub(before.configures),
        buffers: after.buffers.saturating_sub(before.buffers),
    }
}

/// The process under `root` that runs `yuna`, natively or under valgrind: the one whose arguments
/// name it, but the launcher's and the client's, which run `program`.
fn measured_process(proc: &Proc, root: u32, yuna: &Path, program: &Path) -> Option<u32> {
    let yuna = yuna.to_string_lossy();
    descendants(proc, root).into_iter().find(|pid| {
        proc.executable(*pid).as_deref() != Some(program)
            && proc
                .command_line(*pid)
                .is_some_and(|arguments| arguments.iter().any(|a| *a == *yuna))
    })
}

/// Every process under `root`, `root` left out.
fn descendants(proc: &Proc, root: u32) -> Vec<u32> {
    let mut under = vec![root];
    let mut index = 0;
    while index < under.len() {
        let parent = under[index];
        for pid in proc.pids() {
            if !under.contains(&pid) && proc.stat(pid).is_some_and(|(ppid, _)| ppid == parent) {
                under.push(pid);
            }
        }
        index += 1;
    }
    under.remove(0);
    under
}

/// The processes under `root` that run `yuna`: at M2.1 one, the run's Yuna; at M2.12 a seat runs
/// in a sibling Yuna, which these figures then count too.
fn yuna_processes(proc: &Proc, root: u32, yuna: &Path) -> Vec<u32> {
    descendants(proc, root)
        .into_iter()
        .filter(|pid| proc.executable(*pid).as_deref() == Some(yuna))
        .collect()
}

/// The voluntary context switches, so far, of Yuna's own threads in every Yuna process under
/// `root`: each process's main thread and the threads Yuna names `yuna-…`. Mesa's threads, which
/// it does not name or names otherwise (`llvmpipe-0`), are left out.
fn own_switches(proc: &Proc, root: u32, yuna: &Path) -> u64 {
    yuna_processes(proc, root, yuna)
        .into_iter()
        .flat_map(|pid| {
            proc.threads(pid)
                .into_iter()
                .filter(move |thread| thread.tid == pid || thread.name.starts_with("yuna-"))
        })
        .map(|thread| thread.voluntary)
        .sum()
}

/// `Pss_Anon` and `Pss_File`, in KiB, of every Yuna process under `root`.
fn memory(proc: &Proc, root: u32, yuna: &Path) -> (u64, u64) {
    yuna_processes(proc, root, yuna)
        .into_iter()
        .filter_map(|pid| proc.smaps_rollup(pid))
        .map(|rollup| {
            (
                procfs::parse_kib(&rollup, "Pss_Anon").unwrap_or(0),
                procfs::parse_kib(&rollup, "Pss_File").unwrap_or(0),
            )
        })
        .fold((0, 0), |(a, f), (anon, file)| (a + anon, f + file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_work_between_two_rest_replies() {
        let before = json!({"frames": 10, "wakes": 30,
            "draw_causes": [{"cause": "commit", "frames": 8}, {"cause": "first_frame", "frames": 1}],
            "sources": [{"source": "mcp_input", "wakes": 20}, {"source": "clients", "wakes": 10}],
            "clients": [{"pid": 3, "commits": 12}]});
        let after = json!({"frames": 50, "wakes": 110,
            "draw_causes": [{"cause": "commit", "frames": 48}, {"cause": "first_frame", "frames": 1},
                            {"cause": "motion", "frames": 3}],
            "sources": [{"source": "mcp_input", "wakes": 60}, {"source": "clients", "wakes": 50}],
            "clients": [{"pid": 3, "commits": 52}]});
        let done = work(&before, &after).unwrap();
        assert_eq!(done.frames, 40);
        assert_eq!(done.commits, 40);
        assert_eq!(
            done.causes,
            BTreeMap::from([
                ("commit".to_owned(), 40),
                ("first_frame".to_owned(), 0),
                ("motion".to_owned(), 3)
            ])
        );
        assert_eq!(
            wakes_by_source(&before, &after).unwrap(),
            BTreeMap::from([("clients".to_owned(), 40), ("mcp_input".to_owned(), 40)])
        );
        // A field missing from a reply fails the run: it never reads as 0.
        assert!(work(&before, &json!({})).is_err());
        for missing in ["frames", "draw_causes", "clients"] {
            let mut cut = after.clone();
            cut.as_object_mut().unwrap().remove(missing);
            assert!(work(&before, &cut).is_err(), "{missing}");
        }
        let mut cut = after.clone();
        cut["clients"][0].as_object_mut().unwrap().remove("commits");
        assert!(work(&before, &cut).is_err());
        assert!(wakes_by_source(&before, &json!({"wakes": 1})).is_err());
        assert!(count(&json!({}), "wakes").is_err());
    }

    #[test]
    fn every_scenario_is_named_once_and_found() {
        for scenario in &SCENARIOS {
            assert_eq!(named(scenario.name), Some(scenario));
            assert!(scenario.periods > 0 && scenario.windows > 0);
        }
        assert_eq!(named("sleep"), None);
        // motion's measured periods are whole cycles, so that the longer run repeats them.
        let motion = named("motion").unwrap();
        assert_eq!(motion.periods % (4 * MOTION_STEP), 0);
        assert_eq!(warm_up(motion) % MOTION_STEP, 8);
    }

    #[test]
    fn yuna_own_threads_are_its_main_thread_and_those_it_names() {
        // The launcher, 9, runs Yuna, 10, and the client, 11.
        let root = crate::testing::folder("parity-switches");
        let process = |pid: u32, parent: u32, exe: &str| {
            fs::create_dir_all(root.join(format!("{pid}"))).unwrap();
            fs::write(
                root.join(format!("{pid}/stat")),
                format!("{pid} (x) S {parent} {pid} {pid} 0 -1 0 0 0 0 0 1 1 0 0\n"),
            )
            .unwrap();
            std::os::unix::fs::symlink(exe, root.join(format!("{pid}/exe"))).unwrap();
        };
        let thread = |pid: u32, tid: u32, name: &str, voluntary: u64| {
            let task = root.join(format!("{pid}/task/{tid}"));
            fs::create_dir_all(&task).unwrap();
            fs::write(task.join("comm"), format!("{name}\n")).unwrap();
            fs::write(task.join("schedstat"), "100 0 0\n").unwrap();
            fs::write(
                task.join("status"),
                format!("voluntary_ctxt_switches:\t{voluntary}\nnonvoluntary_ctxt_switches:\t1\n"),
            )
            .unwrap();
        };
        process(9, 1, "/w/bin/yuna-bench");
        process(10, 9, "/w/bin/yuna");
        process(11, 9, "/w/bin/yuna-bench");
        thread(9, 9, "yuna-bench", 300);
        thread(10, 10, "yuna", 5);
        thread(10, 11, "llvmpipe-0", 100);
        thread(10, 12, "yuna", 1000);
        thread(10, 13, "yuna-capture", 7);
        thread(11, 11, "yuna-bench", 50);
        for (pid, arguments) in [
            (
                9,
                "/w/bin/yuna-bench\0parity\0launch\0--\0/w/bin/yuna\0mcp\0",
            ),
            (10, "/w/bin/yuna\0mcp\0--manual-clock\0"),
            (11, "/w/bin/yuna-bench\0client\0lockstep\0"),
        ] {
            fs::write(root.join(format!("{pid}/cmdline")), arguments).unwrap();
        }
        let proc = Proc::at(&root);
        let yuna = Path::new("/w/bin/yuna");
        assert_eq!(yuna_processes(&proc, 9, yuna), [10]);
        assert_eq!(own_switches(&proc, 9, yuna), 12);
        // The launcher names Yuna in its arguments too, but runs the script's program.
        assert_eq!(
            measured_process(&proc, 1, yuna, Path::new("/w/bin/yuna-bench")),
            Some(10)
        );
    }
}
