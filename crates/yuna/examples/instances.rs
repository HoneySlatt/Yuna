//! Measures what agents' Yunas cost: one `yuna mcp` alone, and N `yuna mcp` with a seat each
//! against one `yuna mcp` with N seats, the figures M1.11 asks for (`docs/PLAN.md`). Stage 2's
//! core chooses between the two on them
//! ([ADR-0078](../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)),
//! and the ROADMAP's budgets for `yuna mcp` rest on them
//! ([ADR-0082](../../../docs/adr/0082-the-requirements-of-stage-2s-core-its-bets-made-rules-or-deferred.md),
//! item 25).
//!
//! It starts the `yuna` built beside it as `yuna mcp`, each seat's output of `--output`'s mode,
//! 1920 × 1080 at 60 Hz and scale 1 by default, and runs three phases:
//!
//! 1. **One start** (`--phase start`): `yuna mcp` spawned and its `seat create` written at once,
//!    then the time from the spawn to the reply, read as it arrives, and to the first frame drawn
//!    on the seat's output, read from the time of Yuna's `frame drawn` log line; then, once its
//!    log has stayed the same for [`QUIET`], its memory at rest: `Pss_Anon` + `Pss_File` of its
//!    `smaps_rollup`, its own ([ADR-0077](../../../docs/adr/0077-the-budgets-of-a-frame-memory-and-rest-set-from-the-rivals.md)),
//!    and with `--gpu` its GPU memory, the kernel's DRM `resident-vram` + `resident-gtt`.
//!    `--starts` starts, 20 by default, each a new Yuna: *cold*, each with a new home, so with
//!    Mesa's shader cache empty, as an agent's thrown-away folders leave it (ADR-0082, item 27);
//!    then *warm*, every one with the home a first start, not counted, filled. Their median and
//!    longest.
//! 2. **Many together** (`--phase together`): for N = 1, 2, 4, 6, 8 and 10, N seats started
//!    together, first as N `yuna mcp` with a seat each, then as one `yuna mcp` with N seats. Each
//!    seat starts [`CLIENT`], Alacritty, through `spawn` once its `seat create` has replied: a
//!    Wayland client that draws its window and rests, with the environment the applications'
//!    tests give it (`crates/yuna/tests/common/application_environment.rs`), on llvmpipe in
//!    both cases and on both of Yuna's devices, so that it is the same client in all. From the
//!    first spawn: when every seat's `seat create` replied, when every seat's output drew its
//!    first frame, and when every seat drew its first frame with its client's window. Then, at
//!    rest, the memory of everything under the Yunas, Yuna, its Xwayland if any, the client and
//!    its child, and of the Yunas alone; with `--gpu`, their GPU memory, each DRM client counted
//!    once. The same after `--rest` seconds, 60 by default, with the frames drawn and the CPU
//!    time used meanwhile, which rest makes nil.
//! 3. **A soak** (`--phase soak`): 10 `yuna mcp`, a seat and a client each, held `--soak`
//!    seconds at rest, 300 by default, their memory read every [`SOAK_READING`]: none may grow
//!    (ADR-0082, item 25).
//!
//! Without `--phase`, all three run in turn. Beside each phase it reads what the machine's other
//! programs used, and marks a phase they disturb (`common/load.rs`). It ends with the phase's
//! figures as Markdown tables, for `docs/measurements.md`.
//!
//! Build it and Yuna for release, then run it in the development shell, alone among
//! measurements; it opens no display, and needs no screen of its own:
//!
//! ```sh
//! nix develop -c cargo build --release -p yuna --bin yuna --example instances
//! nix develop -c target/release/examples/instances
//! ```
//!
//! `--gpu` draws on the GPU's render node (`yuna --gpu`), never in the tests. Yuna's and the
//! clients' folders go in the temporary folder, `TMPDIR`, which `nix develop` sets to a folder of
//! its own inside the caller's: set it inside the shell (`nix develop -c env TMPDIR=… …`). The
//! seats' sockets are in it, and a socket's path holds at most 107 bytes: `seat create` refuses a
//! folder deeper than about 50. It stops what it started when it ends, and keeps its folder of
//! logs when it fails. Killed outright, it leaves nothing running: each `yuna mcp` stops when its
//! input closes, and ends its programs.

#![allow(
    clippy::disallowed_methods,
    reason = "an example's threads are not Yuna's tasks (ADR-0152)"
)]

mod common;
#[path = "common/load.rs"]
mod load;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common::{PATIENCE, Run, field, percentile, shell_variable};
use load::LoadWatch;
use serde_json::{Value, json};
use yuna_bench::drm;
use yuna_bench::machine::MachineInfo;
use yuna_bench::procfs::{self, Proc};
use yuna_bench::sandbox::RENDER_NODE;

/// How long every Yuna's log must stay the same for it to count as at rest.
const QUIET: Duration = Duration::from_secs(1);
/// How often a log is read while a frame or the rest is waited for; reading it never wakes Yuna.
const POLL: Duration = Duration::from_millis(20);
/// The counts of seats started together.
const COUNTS: [usize; 6] = [1, 2, 4, 6, 8, 10];
/// The instances the soak holds.
const SOAK_INSTANCES: usize = 10;
/// How often the soak reads the memory.
const SOAK_READING: Duration = Duration::from_secs(30);
/// Each seat's client: Alacritty, its cursor still, with a child that waits for ever, so that
/// after its first frames it draws nothing more.
const CLIENT: [&str; 6] = [
    "alacritty",
    "--option",
    "cursor.style.blinking=\"Never\"",
    "--command",
    "sleep",
    "infinity",
];

/// What the command line asks for.
#[derive(Debug, PartialEq)]
struct Options {
    gpu: bool,
    output: String,
    /// One phase, or all three without it.
    phase: Option<Phase>,
    starts: usize,
    rest: Duration,
    soak: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Start,
    Together,
    Soak,
}

/// How N seats are served.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Layout {
    /// N `yuna mcp`, a seat each.
    Instances,
    /// One `yuna mcp`, N seats.
    Seats,
}

impl Layout {
    fn describe(self, n: usize) -> String {
        match self {
            Layout::Instances => format!("{n} yuna mcp, a seat each"),
            Layout::Seats if n == 1 => "1 yuna mcp, 1 seat".to_owned(),
            Layout::Seats => format!("1 yuna mcp, {n} seats"),
        }
    }
}

fn main() -> ExitCode {
    let options = match read_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(reason) => {
            eprintln!("instances: {reason}");
            return ExitCode::from(2);
        }
    };
    let run = match Run::new("yuna-instances") {
        Ok(run) => run,
        Err(reason) => {
            eprintln!("instances: {reason}");
            return ExitCode::FAILURE;
        }
    };
    match measure(&run, &options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("instances: {reason}; logs kept in {}", run.keep());
            ExitCode::FAILURE
        }
    }
}

fn read_options(arguments: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        gpu: false,
        output: "1920x1080@60,1".to_owned(),
        phase: None,
        starts: 20,
        rest: Duration::from_secs(60),
        soak: Duration::from_secs(300),
    };
    let mut arguments = arguments;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        let whole = |value: String, what: &str| {
            value
                .parse::<u64>()
                .ok()
                .filter(|&n| n > 0)
                .ok_or(format!("{what} takes a whole number above 0"))
        };
        match argument.as_str() {
            "--gpu" => options.gpu = true,
            "--output" => options.output = value()?,
            "--phase" => {
                options.phase = Some(match value()?.as_str() {
                    "start" => Phase::Start,
                    "together" => Phase::Together,
                    "soak" => Phase::Soak,
                    other => {
                        return Err(format!("--phase is start, together or soak, not {other:?}"));
                    }
                });
            }
            "--starts" => options.starts = whole(value()?, "--starts")? as usize,
            "--rest" => options.rest = Duration::from_secs(whole(value()?, "--rest")?),
            "--soak" => options.soak = Duration::from_secs(whole(value()?, "--soak")?),
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }
    Ok(options)
}

fn measure(run: &Run, options: &Options) -> Result<(), String> {
    print_machine(options)?;
    let phases = match options.phase {
        Some(phase) => vec![phase],
        None => vec![Phase::Start, Phase::Together, Phase::Soak],
    };
    for phase in phases {
        match phase {
            Phase::Start => starts(run, options)?,
            Phase::Together => together(run, options)?,
            Phase::Soak => soak(run, options)?,
        }
    }
    Ok(())
}

/// The machine, Yuna's device and output, and the client's version, as the measurements name them.
fn print_machine(options: &Options) -> Result<(), String> {
    let machine = MachineInfo::read(RENDER_NODE, Some(&shell_mesa()?));
    let unknown = || "?".to_owned();
    println!(
        "Machine: {}, {} threads, {} MiB; kernel {}; GPU {} ({}); Mesa {}",
        machine.cpu_model.unwrap_or_else(unknown),
        machine.logical_cpus.map_or_else(unknown, |n| n.to_string()),
        machine
            .memory_kib
            .map_or_else(unknown, |kib| (kib / 1024).to_string()),
        machine.kernel.unwrap_or_else(unknown),
        machine
            .gpu
            .as_ref()
            .and_then(|gpu| gpu.pci_slot.clone())
            .unwrap_or_else(unknown),
        machine
            .gpu
            .as_ref()
            .and_then(|gpu| gpu.driver.clone())
            .unwrap_or_else(unknown),
        machine.mesa_version.unwrap_or_else(unknown),
    );
    let applications = shell_variable("YUNA_APPLICATIONS_PATH")?;
    let client = applications
        .split(':')
        .find(|folder| folder.contains("-alacritty-"))
        .and_then(procfs::store_folder)
        .and_then(procfs::store_version)
        .unwrap_or("?");
    println!(
        "yuna mcp drawn on {}, each seat's output {}; the client Alacritty {client}, on llvmpipe",
        if options.gpu {
            "the GPU (--gpu)"
        } else {
            "llvmpipe"
        },
        options.output
    );
    Ok(())
}

/// What one start took, and the memory after it.
struct Start {
    /// From the spawn to the reply of `seat create`.
    replied: Duration,
    /// From the spawn to the end of the seat's output's first frame.
    drawn: Duration,
    /// Yuna's memory at rest after it.
    memory: Memory,
    /// The processes of others that mapped the shell's Mesa when it was read.
    others_on_mesa: usize,
}

/// Phase 1: cold starts, a new home each, then warm ones, one home they share.
fn starts(run: &Run, options: &Options) -> Result<(), String> {
    println!("\nOne yuna mcp's start, {} starts each:", options.starts);
    let mut rows = Vec::new();
    for warm in [false, true] {
        let kind = if warm { "warm" } else { "cold" };
        let watch = LoadWatch::start();
        if warm {
            // Fills the shared home's shader cache.
            one_start(run, options, "start-warm-fill", "start-warm")?;
        }
        let mut starts = Vec::with_capacity(options.starts);
        for n in 0..options.starts {
            let name = format!("start-{kind}-{n}");
            let home = if warm {
                "start-warm".to_owned()
            } else {
                name.clone()
            };
            starts.push(one_start(run, options, &name, &home)?);
        }
        let load = watch.finish();
        let ms = |pick: fn(&Start) -> Duration| {
            sorted(starts.iter().map(|start| pick(start).as_micros() as u64))
        };
        let kib = |pick: fn(&Memory) -> Option<u64>| {
            sorted(starts.iter().filter_map(|start| pick(&start.memory)))
        };
        let shared = starts
            .iter()
            .filter(|start| start.others_on_mesa > 0)
            .count();
        let most_others = starts.iter().map(|start| start.others_on_mesa).max();
        let replied = ms(|start| start.replied);
        let drawn = ms(|start| start.drawn);
        let own = kib(|memory| Some(memory.own));
        let anon = kib(|memory| Some(memory.anon));
        let swapped = kib(|memory| Some(memory.swapped));
        let gpu = kib(|memory| memory.gpu);
        println!("  {kind}:");
        println!("    spawn to seat create's reply: {}", spread_ms(&replied));
        println!(
            "    spawn to the output's first frame: {}",
            spread_ms(&drawn)
        );
        println!("    Yuna's own memory at rest: {}", spread_mib(&own));
        println!("    of it anonymous: {}", spread_mib(&anon));
        println!("    swapped out: {}", spread_mib(&swapped));
        println!("    its GPU memory: {}", spread_mib(&gpu));
        println!(
            "    read while other processes mapped the shell's Mesa, sharing its pages: {shared} \
             of {} starts, {} processes at most",
            starts.len(),
            most_others.unwrap_or(0)
        );
        println!("    {load}");
        rows.push(format!(
            "| {kind} | {} | {} | {} | {} | {} | {} | {} | {} | {shared} |",
            ms_text(percentile(&replied, 50.0)),
            ms_text(replied[replied.len() - 1]),
            ms_text(percentile(&drawn, 50.0)),
            ms_text(drawn[drawn.len() - 1]),
            mib(percentile(&own, 50.0)),
            mib(own[own.len() - 1]),
            gpu.first()
                .map_or("none".to_owned(), |_| mib(percentile(&gpu, 50.0))),
            gpu.last().map_or("none".to_owned(), |kib| mib(*kib)),
        ));
    }
    println!(
        "\n| Start | Reply, median | Longest | First frame, median | Longest | Own memory, \
         median | Most | GPU memory, median | Most | Starts read beside others on its Mesa |\n\
         |---|---|---|---|---|---|---|---|---|---|"
    );
    for row in rows {
        println!("{row}");
    }
    Ok(())
}

/// One `yuna mcp`, home `home`: its start to its seat's reply and first frame, then its memory at
/// rest.
fn one_start(run: &Run, options: &Options, name: &str, home: &str) -> Result<Start, String> {
    let mut yuna = Yuna::start(run, options, name, home)?;
    let asked = yuna.send("seat", json!({"action": "create"}))?;
    let reply = yuna.wait(asked)?;
    let output = output_of(&reply.result)?;
    let replied = reply.at - yuna.spawned;
    let drawn = yuna.wait_for_frame(output, Frame::Any)?;
    let drawn = drawn
        .duration_since(yuna.spawned_at)
        .map_err(|_| "a frame drawn before Yuna was spawned: the clock stepped".to_owned())?;
    wait_quiet(std::slice::from_ref(&yuna))?;
    let proc = Proc::system();
    let memory = Memory::of(&proc, &[yuna.pid()]);
    let others_on_mesa = others_on_mesa(&proc, &[yuna.pid()], &shell_mesa()?);
    stop_all(vec![yuna])?;
    Ok(Start {
        replied,
        drawn,
        memory,
        others_on_mesa,
    })
}

/// A seat of the seats started together: its Yuna and output, and when its first frames came.
struct Seat {
    /// Its Yuna's index.
    yuna: usize,
    /// Its output's id.
    output: u64,
    /// From the first spawn to its `seat create`'s reply.
    replied: Duration,
    /// From the first spawn to its output's first frame, and to its first frame with its client's
    /// window.
    drawn: Duration,
    shown: Duration,
}

/// N seats started together, each with its client shown.
struct Started {
    yunas: Vec<Yuna>,
    seats: Vec<Seat>,
    /// The shell's Mesa's store folder.
    mesa: String,
}

/// The last and first of the seats' times, from the first spawn.
struct SeatsSummary {
    replied_last: u64,
    drawn_last: u64,
    shown_last: u64,
    shown_first: u64,
}

impl Started {
    /// Starts `n` seats, as `layout` serves them, each with its client, and waits until every
    /// client's window is drawn and every Yuna is at rest. Their folders and logs in the run's
    /// folder are named after `tag`.
    fn start(
        run: &Run,
        options: &Options,
        n: usize,
        layout: Layout,
        tag: &str,
    ) -> Result<Started, String> {
        let began_at = SystemTime::now();
        let began = Instant::now();
        let mut yunas = Vec::new();
        let mut asked = Vec::new();
        match layout {
            Layout::Instances => {
                for i in 0..n {
                    let name = format!("{tag}-{i}");
                    let mut yuna = Yuna::start(run, options, &name, &name)?;
                    asked.push((i, yuna.send("seat", json!({"action": "create"}))?));
                    yunas.push(yuna);
                }
            }
            Layout::Seats => {
                let mut yuna = Yuna::start(run, options, tag, tag)?;
                for _ in 0..n {
                    asked.push((0, yuna.send("seat", json!({"action": "create"}))?));
                }
                yunas.push(yuna);
            }
        }
        let mut seats = Vec::with_capacity(n);
        let mut spawns = Vec::with_capacity(n);
        for (index, (yuna, id)) in asked.into_iter().enumerate() {
            let reply = yunas[yuna].wait(id)?;
            let seat = reply.result["structuredContent"]["seat"]
                .as_u64()
                .ok_or_else(|| format!("seat create gave no seat: {}", reply.result))?;
            let output = output_of(&reply.result)?;
            let home = run.folder().join(format!("{tag}-client-{index}"));
            let spawned = yunas[yuna].send(
                "spawn",
                json!({"action": "start", "seat": seat, "command": CLIENT,
                       "env": client_environment(&home)?}),
            )?;
            spawns.push((yuna, spawned));
            seats.push(Seat {
                yuna,
                output,
                replied: reply.at - began,
                drawn: Duration::ZERO,
                shown: Duration::ZERO,
            });
        }
        for (yuna, id) in spawns {
            yunas[yuna].wait(id)?;
        }
        let since_began = |at: SystemTime| {
            at.duration_since(began_at)
                .map_err(|_| "a frame drawn before the start: the clock stepped".to_owned())
        };
        for seat in &mut seats {
            let yuna = &yunas[seat.yuna];
            seat.drawn = since_began(yuna.wait_for_frame(seat.output, Frame::Any)?)?;
            seat.shown = since_began(yuna.wait_for_frame(seat.output, Frame::WithWindow)?)?;
        }
        wait_quiet(&yunas)?;
        Ok(Started {
            yunas,
            seats,
            mesa: shell_mesa()?,
        })
    }

    /// The last and first of the seats' times.
    fn seats_summary(&self) -> SeatsSummary {
        let micros = |pick: fn(&Seat) -> Duration| {
            sorted(self.seats.iter().map(|seat| pick(seat).as_micros() as u64))
        };
        let (replied, drawn, shown) = (
            micros(|seat| seat.replied),
            micros(|seat| seat.drawn),
            micros(|seat| seat.shown),
        );
        SeatsSummary {
            replied_last: replied[replied.len() - 1],
            drawn_last: drawn[drawn.len() - 1],
            shown_last: shown[shown.len() - 1],
            shown_first: shown[0],
        }
    }

    fn pids(&self) -> Vec<u32> {
        self.yunas.iter().map(Yuna::pid).collect()
    }

    /// What everything under the Yunas, and the Yunas alone, hold now.
    fn read(&self) -> Reading {
        let proc = Proc::system();
        let yunas = self.pids();
        let everything: Vec<u32> = yunas.iter().flat_map(|&pid| tree(&proc, pid)).collect();
        let mut processes = BTreeMap::new();
        for &pid in &everything {
            let name = proc.name(pid).unwrap_or_else(|| "?".to_owned());
            *processes.entry(name).or_insert(0) += 1;
        }
        Reading {
            everything: Memory::of(&proc, &everything),
            yunas: Memory::of(&proc, &yunas),
            each_own: yunas
                .iter()
                .map(|&pid| Memory::of(&proc, &[pid]).own)
                .collect(),
            processes,
            cpu_ns: everything
                .iter()
                .flat_map(|&pid| proc.threads(pid))
                .map(|thread| (thread.tid, thread.cpu_ns))
                .collect(),
            log_lengths: self
                .yunas
                .iter()
                .map(|yuna| yuna.read_log().len())
                .collect(),
            others_on_mesa: others_on_mesa(&proc, &everything, &self.mesa),
        }
    }

    /// The frames drawn in every Yuna's log since `reading`.
    fn frames_since(&self, reading: &Reading) -> usize {
        self.yunas
            .iter()
            .zip(&reading.log_lengths)
            .map(|(yuna, &start)| {
                yuna.read_log()
                    .get(start..)
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| line.contains("frame drawn"))
                    .count()
            })
            .sum()
    }
}

/// The memory and the CPU time of everything under the Yunas at one moment.
struct Reading {
    everything: Memory,
    yunas: Memory,
    /// Each Yuna's own memory.
    each_own: Vec<u64>,
    /// How many processes of each name were under them.
    processes: BTreeMap<String, usize>,
    /// Each of their threads' CPU time, in nanoseconds.
    cpu_ns: BTreeMap<u32, u64>,
    /// Each Yuna's log's length.
    log_lengths: Vec<usize>,
    /// The processes of others that mapped the shell's Mesa.
    others_on_mesa: usize,
}

impl Reading {
    /// The CPU time everything under the Yunas used since `before`, each thread taken apart, in
    /// milliseconds.
    fn cpu_ms_since(&self, before: &Reading) -> f64 {
        let used: u64 = self
            .cpu_ns
            .iter()
            .map(|(tid, ns)| ns.saturating_sub(before.cpu_ns.get(tid).copied().unwrap_or(0)))
            .sum();
        used as f64 / 1e6
    }

    fn describe(&self) -> String {
        let processes: Vec<String> = self
            .processes
            .iter()
            .map(|(name, count)| format!("{name} {count}"))
            .collect();
        let each = sorted(self.each_own.iter().copied());
        format!(
            "everything: {} in {} processes ({}); the Yunas: {}, each's own {} to {}; GPU: \
             everything {}, the Yunas {}; {} other processes mapped the shell's Mesa",
            self.everything.describe_pss(),
            self.processes.values().sum::<usize>(),
            processes.join(", "),
            self.yunas.describe_pss(),
            mib(each[0]),
            mib(each[each.len() - 1]),
            self.everything.gpu.map_or("none".to_owned(), mib),
            self.yunas.gpu.map_or("none".to_owned(), mib),
            self.others_on_mesa,
        )
    }
}

/// Phase 2: N seats started together, as N Yunas and as one, for each N of [`COUNTS`].
fn together(run: &Run, options: &Options) -> Result<(), String> {
    println!(
        "\nN seats started together, each with Alacritty; {} s at rest after:",
        options.rest.as_secs()
    );
    let mut times = Vec::new();
    let mut memory = Vec::new();
    for n in COUNTS {
        for layout in [Layout::Instances, Layout::Seats] {
            let watch = LoadWatch::start();
            let tag = match layout {
                Layout::Instances => format!("instances-{n}"),
                Layout::Seats => format!("seats-{n}"),
            };
            let started = Started::start(run, options, n, layout, &tag)?;
            let at_rest = started.read();
            std::thread::sleep(options.rest);
            let after = started.read();
            let frames = started.frames_since(&at_rest);
            let cpu_ms = after.cpu_ms_since(&at_rest);
            let seats = started.seats_summary();
            stop_all(started.yunas)?;
            let load = watch.finish();
            println!("  {}:", layout.describe(n));
            println!(
                "    from the first spawn, every seat's reply by {}, its output's first frame by \
                 {}, its client shown by {} (the first by {})",
                ms_text(seats.replied_last),
                ms_text(seats.drawn_last),
                ms_text(seats.shown_last),
                ms_text(seats.shown_first),
            );
            println!("    at rest: {}", at_rest.describe());
            println!(
                "    after {} s: {}; {frames} frames drawn and {cpu_ms:.1} ms of CPU meanwhile",
                options.rest.as_secs(),
                after.describe()
            );
            println!("    {load}");
            times.push(format!(
                "| {n} | {} | {} | {} | {} |",
                layout.describe(n),
                ms_text(seats.replied_last),
                ms_text(seats.drawn_last),
                ms_text(seats.shown_last),
            ));
            memory.push(format!(
                "| {n} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {frames}, {cpu_ms:.1} ms | \
                 {}, {} |",
                layout.describe(n),
                mib(at_rest.everything.pss),
                mib(at_rest.yunas.pss),
                mib(at_rest.yunas.own),
                at_rest.everything.gpu.map_or("none".to_owned(), mib),
                mib(after.everything.pss),
                mib(after.yunas.pss),
                mib(after.yunas.own),
                after.everything.gpu.map_or("none".to_owned(), mib),
                at_rest.others_on_mesa,
                after.others_on_mesa,
            ));
        }
    }
    println!(
        "\n| N | Served by | Every seat's reply | Every output's first frame | Every client shown |\n\
         |---|---|---|---|---|"
    );
    for row in times {
        println!("{row}");
    }
    println!(
        "\n| N | Served by | Everything, Pss | The Yunas, Pss | Their own | GPU | After the rest: \
         everything | The Yunas | Their own | GPU | Frames, CPU in the rest | Others on its Mesa |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|"
    );
    for row in memory {
        println!("{row}");
    }
    Ok(())
}

/// Phase 3: [`SOAK_INSTANCES`] Yunas held at rest, their memory read every [`SOAK_READING`].
fn soak(run: &Run, options: &Options) -> Result<(), String> {
    println!(
        "\nA soak: {SOAK_INSTANCES} yuna mcp, a seat and Alacritty each, {} s at rest:",
        options.soak.as_secs()
    );
    let watch = LoadWatch::start();
    let started = Started::start(run, options, SOAK_INSTANCES, Layout::Instances, "soak")?;
    let first = started.read();
    let began = Instant::now();
    let mut rows = vec![soak_row(Duration::ZERO, &first, &first)];
    println!("  0 s: {}", first.describe());
    let mut last = None;
    let mut next = SOAK_READING;
    while next <= options.soak {
        std::thread::sleep((began + next).saturating_duration_since(Instant::now()));
        let reading = started.read();
        println!("  {} s: {}", next.as_secs(), reading.describe());
        rows.push(soak_row(next, &first, &reading));
        last = Some(reading);
        next += SOAK_READING;
    }
    let frames = started.frames_since(&first);
    let cpu_ms = last.as_ref().map_or(0.0, |last| last.cpu_ms_since(&first));
    stop_all(started.yunas)?;
    let load = watch.finish();
    if let Some(last) = &last {
        println!(
            "  growth over {} s: everything {}, the Yunas' own {}, GPU {}; {frames} frames drawn \
             and {cpu_ms:.1} ms of CPU",
            options.soak.as_secs(),
            growth(first.everything.pss, last.everything.pss),
            growth(first.yunas.own, last.yunas.own),
            match (first.everything.gpu, last.everything.gpu) {
                (Some(before), Some(after)) => growth(before, after),
                _ => "none".to_owned(),
            },
        );
    }
    println!("  {load}");
    println!(
        "\n| At | Everything, Pss | The Yunas' own | Each Yuna's own | GPU | Since the start | \
         Others on its Mesa |\n|---|---|---|---|---|---|---|"
    );
    for row in rows {
        println!("{row}");
    }
    Ok(())
}

fn soak_row(at: Duration, first: &Reading, reading: &Reading) -> String {
    let each = sorted(reading.each_own.iter().copied());
    format!(
        "| {} s | {} | {} | {} to {} | {} | everything {}, own {} | {} |",
        at.as_secs(),
        mib(reading.everything.pss),
        mib(reading.yunas.own),
        mib(each[0]),
        mib(each[each.len() - 1]),
        reading.everything.gpu.map_or("none".to_owned(), mib),
        growth(first.everything.pss, reading.everything.pss),
        growth(first.yunas.own, reading.yunas.own),
        reading.others_on_mesa,
    )
}

/// The environment of a client whose home is `home`, as the applications' tests give it
/// (`crates/yuna/tests/common/application_environment.rs`): the shell's applications, throwaway
/// home and XDG folders, DejaVu alone, Mesa on the CPU.
fn client_environment(home: &Path) -> Result<Value, String> {
    fs::create_dir_all(home).map_err(|e| format!("cannot create {}: {e}", home.display()))?;
    Ok(json!({
        "PATH": shell_variable("YUNA_APPLICATIONS_PATH")?,
        "HOME": home,
        "XDG_CONFIG_HOME": home.join("config"),
        "XDG_DATA_HOME": home.join("data"),
        "XDG_CACHE_HOME": home.join("cache"),
        "XDG_STATE_HOME": home.join("state"),
        "XDG_CONFIG_DIRS": home.join("config-dirs"),
        "XDG_DATA_DIRS": home.join("data-dirs"),
        "FONTCONFIG_FILE": shell_variable("YUNA_FONTS_CONF")?,
        "LD_LIBRARY_PATH": shell_variable("LD_LIBRARY_PATH")?,
        "__EGL_VENDOR_LIBRARY_FILENAMES": shell_variable("__EGL_VENDOR_LIBRARY_FILENAMES")?,
        "LIBGL_ALWAYS_SOFTWARE": "1",
        "VK_DRIVER_FILES": shell_variable("YUNA_LAVAPIPE_ICD")?,
        "VK_LOADER_LAYERS_DISABLE": "~implicit~",
    }))
}

/// The store folder of the shell's Mesa, which Yuna and its clients draw with.
fn shell_mesa() -> Result<String, String> {
    let egl = shell_variable("__EGL_VENDOR_LIBRARY_FILENAMES")?;
    procfs::store_folder(&egl)
        .map(str::to_owned)
        .ok_or_else(|| format!("__EGL_VENDOR_LIBRARY_FILENAMES is not in the Nix store: {egl}"))
}

/// How many processes but `ours` map files of `mesa`, a store folder. Each takes a share of the
/// pages that Yuna and its clients map from it, which lowers their `Pss_File`: another session's
/// Yuna, or its tests, may draw with the same Mesa.
fn others_on_mesa(proc: &Proc, ours: &[u32], mesa: &str) -> usize {
    proc.pids()
        .into_iter()
        .filter(|pid| !ours.contains(pid))
        .filter(|&pid| proc.mapped_store_folders(pid).contains(mesa))
        .count()
}

/// The id of the first output of the seat a `seat create` reply describes.
fn output_of(result: &Value) -> Result<u64, String> {
    result["structuredContent"]["outputs"][0]["id"]
        .as_u64()
        .ok_or_else(|| format!("seat create gave no output: {result}"))
}

/// Memory of a set of processes, in KiB, from their `smaps_rollup` and their DRM files.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Memory {
    /// `Pss`: everything they map, their share of it.
    pss: u64,
    /// `Pss_Anon` + `Pss_File`: their own, without the shared memory of clients' buffers.
    own: u64,
    /// `Pss_Anon`, of `own`: what the kernel cannot drop and read again from a file, which the
    /// page cache's state therefore moves least.
    anon: u64,
    /// `Pss_Shmem`: the shared memory they map, a compositor's clients' `wl_shm` buffers.
    shmem: u64,
    /// `SwapPss`: their share of what the kernel swapped out, which `pss` no longer counts. A
    /// machine short of memory swaps an idle Yuna's pages, and its `pss` falls without its having
    /// freed anything.
    swapped: u64,
    /// The kernel's DRM `resident-vram` + `resident-gtt`, each client counted once; none when
    /// they hold no DRM file that gives either.
    gpu: Option<u64>,
}

impl Memory {
    fn of(proc: &Proc, pids: &[u32]) -> Memory {
        let mut memory = Memory::default();
        for &pid in pids {
            let Some(rollup) = proc.smaps_rollup(pid) else {
                continue;
            };
            memory.add_rollup(&rollup);
        }
        let drm = drm::memory_kib(&drm::drm_files(proc, pids));
        let resident: Vec<u64> = ["resident-vram", "resident-gtt"]
            .iter()
            .filter_map(|key| drm.get(*key).copied())
            .collect();
        memory.gpu = (!resident.is_empty()).then(|| resident.iter().sum());
        memory
    }

    fn add_rollup(&mut self, rollup: &str) {
        let kib = |key| procfs::parse_kib(rollup, key).unwrap_or(0);
        self.pss += kib("Pss");
        self.own += kib("Pss_Anon") + kib("Pss_File");
        self.anon += kib("Pss_Anon");
        self.shmem += kib("Pss_Shmem");
        self.swapped += kib("SwapPss");
    }

    fn describe_pss(&self) -> String {
        format!(
            "{} Pss (own {}, of it anonymous {}; shared memory {}), {} swapped out",
            mib(self.pss),
            mib(self.own),
            mib(self.anon),
            mib(self.shmem),
            mib(self.swapped)
        )
    }
}

/// `root` and every process under it.
fn tree(proc: &Proc, root: u32) -> Vec<u32> {
    let parents: Vec<(u32, u32)> = proc
        .pids()
        .into_iter()
        .filter_map(|pid| Some((pid, proc.stat(pid)?.0)))
        .collect();
    let mut tree = vec![root];
    let mut next = 0;
    while let Some(&parent) = tree.get(next) {
        tree.extend(
            parents
                .iter()
                .filter(|(_, of)| *of == parent)
                .map(|(pid, _)| *pid),
        );
        next += 1;
    }
    tree
}

/// Which of a Yuna's frames is waited for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Frame {
    /// Its output's first.
    Any,
    /// Its output's first with a window in it.
    WithWindow,
}

/// The time of the first `frame drawn` line of `log` on output `output` that `frame` asks for.
fn frame_time(log: &str, output: u64, frame: Frame) -> Option<SystemTime> {
    log.lines()
        .filter(|line| line.contains("frame drawn") && field(line, "output") == Some(output))
        .find(|line| frame == Frame::Any || field(line, "elements").is_some_and(|n| n > 0))
        .and_then(log_time)
}

/// The time a log line begins with, as Yuna's logger writes it, in UTC: such as
/// `2026-11-18T04:12:58.637080Z`.
fn log_time(line: &str) -> Option<SystemTime> {
    let stamp = line.split_whitespace().next()?.strip_suffix('Z')?;
    let (date, time) = stamp.split_once('T')?;
    let numbers = |text: &str, separator: char| -> Option<Vec<u64>> {
        text.split(separator)
            .map(|number| number.parse().ok())
            .collect()
    };
    let [year, month, day] = numbers(date, '-')?[..] else {
        return None;
    };
    let (clock, fraction) = time.split_once('.')?;
    let [hours, minutes, seconds] = numbers(clock, ':')?[..] else {
        return None;
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hours > 23 || minutes > 59 {
        return None;
    }
    if seconds > 60 || fraction.is_empty() || fraction.len() > 9 {
        return None;
    }
    let nanos = fraction.parse::<u64>().ok()? * 10u64.pow(9 - fraction.len() as u32);
    let days = u64::try_from(days_from_civil(
        i64::try_from(year).ok()?,
        month as i64,
        day as i64,
    ))
    .ok()?;
    let seconds = days * 86_400 + hours * 3_600 + minutes * 60 + seconds;
    Some(UNIX_EPOCH + Duration::from_secs(seconds) + Duration::from_nanos(nanos))
}

/// The days from 1970-01-01 to a date of the Gregorian calendar: Howard Hinnant's
/// `days_from_civil`, the inverse of the `civil_from_days` of `yuna_bench::cli::utc`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    // Months from March, so that February, and its leap day, ends the year.
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// A reply of Yuna's, and when this program read it.
struct Reply {
    at: Instant,
    result: Value,
}

/// A `yuna mcp` process: its input, its replies, read on a thread as they come, and its log.
struct Yuna {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<(Instant, Value)>,
    /// Replies read while another was waited for, by their id.
    early: BTreeMap<u64, Reply>,
    next_id: u64,
    log: PathBuf,
    /// When it was spawned, on the clock of `Instant` and on the wall clock its log's times are
    /// written on.
    spawned: Instant,
    spawned_at: SystemTime,
}

impl Yuna {
    /// Starts the `yuna` built beside this program as `yuna mcp`, logging at `debug` to
    /// `<name>.log`, its home the run's folder `home`, with nothing of the caller's environment
    /// but what it draws with and Xwayland's folder.
    fn start(run: &Run, options: &Options, name: &str, home: &str) -> Result<Yuna, String> {
        let program = std::env::current_exe()
            .ok()
            .and_then(|me| Some(me.parent()?.parent()?.join("yuna")))
            .filter(|path| path.exists())
            .ok_or("no yuna beside this program: build it with --bin yuna")?;
        let log = run.folder().join(format!("{name}.log"));
        let mut command = Command::new(program);
        command.args(["mcp", "--output", &options.output]);
        if options.gpu {
            command.arg("--gpu");
        }
        command
            .env_clear()
            .env("PATH", shell_variable("YUNA_XWAYLAND_PATH")?)
            .env("LD_LIBRARY_PATH", shell_variable("LD_LIBRARY_PATH")?)
            .env(
                "__EGL_VENDOR_LIBRARY_FILENAMES",
                shell_variable("__EGL_VENDOR_LIBRARY_FILENAMES")?,
            )
            .env("XKB_CONFIG_ROOT", shell_variable("XKB_CONFIG_ROOT")?)
            .env("HOME", run.folder().join(home))
            .env("XDG_RUNTIME_DIR", run.folder().join("runtime"))
            .env("YUNA_LOG", "debug")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(File::create(&log).map_err(|e| format!("Yuna's log: {e}"))?);
        let spawned_at = SystemTime::now();
        let spawned = Instant::now();
        let mut child = command
            .spawn()
            .map_err(|e| format!("yuna does not start: {e}"))?;
        let stdout = child.stdout.take().ok_or("Yuna's standard output")?;
        let input = child.stdin.take().ok_or("Yuna's standard input")?;
        let (sender, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let at = Instant::now();
                let Ok(line) = line else { return };
                let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                    return;
                };
                if sender.send((at, reply)).is_err() {
                    return;
                }
            }
        });
        Ok(Yuna {
            child,
            input: Some(input),
            replies,
            early: BTreeMap::new(),
            next_id: 1,
            log,
            spawned,
            spawned_at,
        })
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Writes a call of `tool`, and gives its id, without waiting for its reply.
    fn send(&mut self, tool: &str, arguments: Value) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
                "name": tool,
                "arguments": arguments,
            }
        });
        let input = self.input.as_mut().ok_or("Yuna's input is closed")?;
        writeln!(input, "{request}").map_err(|e| format!("{tool}: {e}"))?;
        Ok(id)
    }

    /// Waits for the reply to call `id`, and gives its result, or what it refused.
    fn wait(&mut self, id: u64) -> Result<Reply, String> {
        let reply = loop {
            if let Some(reply) = self.early.remove(&id) {
                break reply;
            }
            let (at, message) = self
                .replies
                .recv_timeout(PATIENCE)
                .map_err(|_| format!("no reply to call {id}"))?;
            let Some(replied) = message["id"].as_u64() else {
                continue;
            };
            self.early.insert(
                replied,
                Reply {
                    at,
                    result: message["result"].clone(),
                },
            );
        };
        if reply.result["isError"] == json!(false) {
            Ok(reply)
        } else {
            Err(format!("call {id} failed: {}", reply.result))
        }
    }

    fn read_log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Waits until its log has a frame drawn on `output` that `frame` asks for, and gives the
    /// time its line was written.
    fn wait_for_frame(&self, output: u64, frame: Frame) -> Result<SystemTime, String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(at) = frame_time(&self.read_log(), output, frame) {
                return Ok(at);
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "no frame {frame:?} on output {output} in {}",
                    self.log.display()
                ));
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for Yuna {
    /// Closes Yuna's input, so that it ends its programs as it stops; kills it if it has not
    /// stopped 10 s later.
    fn drop(&mut self) {
        drop(self.input.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(POLL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Waits until every Yuna's log has stayed the same for [`QUIET`].
fn wait_quiet(yunas: &[Yuna]) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    let lengths = || -> Vec<u64> {
        yunas
            .iter()
            .map(|yuna| fs::metadata(&yuna.log).map_or(0, |m| m.len()))
            .collect()
    };
    let mut last = lengths();
    let mut since = Instant::now();
    while since.elapsed() < QUIET {
        if Instant::now() > deadline {
            return Err("the Yunas never went quiet".to_owned());
        }
        std::thread::sleep(POLL);
        let now = lengths();
        if now != last {
            last = now;
            since = Instant::now();
        }
    }
    Ok(())
}

/// Closes every Yuna's input at once, as their agents' clients do, and requires that each ends
/// well within 10 s.
fn stop_all(mut yunas: Vec<Yuna>) -> Result<(), String> {
    for yuna in &mut yunas {
        drop(yuna.input.take());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ended = BTreeSet::new();
    while ended.len() < yunas.len() {
        for (index, yuna) in yunas.iter_mut().enumerate() {
            match yuna.child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    ended.insert(index);
                }
                Ok(Some(status)) => return Err(format!("yuna ended with {status}")),
                Ok(None) => {}
                Err(error) => return Err(format!("yuna's status: {error}")),
            }
        }
        if Instant::now() > deadline {
            return Err("a yuna did not stop when its input closed".to_owned());
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}

fn sorted(values: impl Iterator<Item = u64>) -> Vec<u64> {
    let mut values: Vec<u64> = values.collect();
    values.sort_unstable();
    values
}

fn mib(kib: u64) -> String {
    format!("{:.1} MiB", kib as f64 / 1024.0)
}

fn ms_text(micros: u64) -> String {
    format!("{:.1} ms", micros as f64 / 1000.0)
}

/// A difference of two figures in KiB, signed, in MiB.
fn growth(before: u64, after: u64) -> String {
    format!("{:+.1} MiB", (after as f64 - before as f64) / 1024.0)
}

/// The median and longest of `sorted`, in milliseconds.
fn spread_ms(sorted: &[u64]) -> String {
    format!(
        "median {}, longest {}",
        ms_text(percentile(sorted, 50.0)),
        ms_text(sorted[sorted.len() - 1])
    )
}

/// The median and most of `sorted`, in MiB; none when it is empty.
fn spread_mib(sorted: &[u64]) -> String {
    if sorted.is_empty() {
        return "none".to_owned();
    }
    format!(
        "median {}, most {}",
        mib(percentile(sorted, 50.0)),
        mib(sorted[sorted.len() - 1])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(arguments: &[&str]) -> Result<Options, String> {
        read_options(arguments.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn options_default_to_every_phase_at_1080p_on_llvmpipe() {
        assert_eq!(
            read(&[]),
            Ok(Options {
                gpu: false,
                output: "1920x1080@60,1".to_owned(),
                phase: None,
                starts: 20,
                rest: Duration::from_secs(60),
                soak: Duration::from_secs(300),
            })
        );
        assert_eq!(
            read(&[
                "--gpu",
                "--phase",
                "soak",
                "--output",
                "3840x2160@240,1.5",
                "--starts",
                "3",
                "--rest",
                "5",
                "--soak",
                "60"
            ]),
            Ok(Options {
                gpu: true,
                output: "3840x2160@240,1.5".to_owned(),
                phase: Some(Phase::Soak),
                starts: 3,
                rest: Duration::from_secs(5),
                soak: Duration::from_secs(60),
            })
        );
        assert!(read(&["--phase", "all"]).is_err());
        assert!(read(&["--phase"]).is_err());
        assert!(read(&["--starts", "0"]).is_err());
        assert!(read(&["--rest", "-1"]).is_err());
        assert!(read(&["--frames"]).is_err());
    }

    #[test]
    fn log_times_are_read_to_the_microsecond() {
        let line = "2026-11-18T04:12:58.637080Z DEBUG yuna::frames: frame drawn output=1";
        let at = log_time(line).expect("a time");
        assert_eq!(yuna_bench::cli::utc(at), "2026-11-18T04:12:58Z");
        assert_eq!(
            at.duration_since(UNIX_EPOCH)
                .expect("after 1970")
                .subsec_micros(),
            637_080
        );
        assert_eq!(log_time("1970-01-01T00:00:00.000000Z x"), Some(UNIX_EPOCH));
        // The leap day, and the day after it.
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(log_time("2000-02-29T00:00:00.0Z"), Some(leap));
        assert_eq!(
            log_time("2000-03-01T00:00:00.5Z"),
            Some(leap + Duration::from_millis(86_400_500))
        );
    }

    #[test]
    fn malformed_log_times_are_none() {
        for line in [
            "",
            "frame drawn",
            "2026-11-18T04:12:58Z",
            "2026-11-18T04:12:58.637080",
            "2026-13-18T04:12:58.637080Z",
            "2026-11-18T24:12:58.637080Z",
            "2026-11-18T04:12:58.6370801234Z",
            "2026-11-18 04:12:58.637080Z",
            "1969-12-31T23:59:59.000000Z",
        ] {
            assert_eq!(log_time(line), None, "{line:?}");
        }
    }

    #[test]
    fn a_seats_first_frames_are_found_on_its_output() {
        let log = "2026-11-18T04:12:58.100000Z DEBUG yuna::frames: frame drawn output=2 \
                   elements=0 took_us=1\n\
                   2026-11-18T04:12:58.200000Z DEBUG yuna::frames: frame drawn output=1 \
                   elements=0 took_us=1\n\
                   2026-11-18T04:12:58.300000Z DEBUG yuna::shell: window opened window=1\n\
                   2026-11-18T04:12:58.400000Z DEBUG yuna::frames: frame drawn output=1 \
                   elements=1 took_us=1\n";
        let at = |micros: u64| {
            log_time("2026-11-18T04:12:58.000000Z").map(|t| t + Duration::from_micros(micros))
        };
        assert_eq!(frame_time(log, 1, Frame::Any), at(200_000));
        assert_eq!(frame_time(log, 1, Frame::WithWindow), at(400_000));
        assert_eq!(frame_time(log, 2, Frame::Any), at(100_000));
        assert_eq!(frame_time(log, 2, Frame::WithWindow), None);
        assert_eq!(frame_time(log, 3, Frame::Any), None);
    }

    #[test]
    fn memory_sums_its_rollups() {
        let mut memory = Memory::default();
        memory.add_rollup(
            "Rss: 100 kB\nPss: 90 kB\nPss_Anon: 50 kB\nPss_File: 30 kB\nPss_Shmem: 10 kB\n",
        );
        memory.add_rollup(
            "Pss: 9 kB\nPss_Anon: 5 kB\nPss_File: 4 kB\nPss_Shmem: 0 kB\nSwapPss: 7 kB\n",
        );
        assert_eq!(
            memory,
            Memory {
                pss: 99,
                own: 89,
                anon: 55,
                shmem: 10,
                swapped: 7,
                gpu: None
            }
        );
    }

    #[test]
    fn a_tree_holds_the_root_and_everything_under_it() {
        let folder = std::env::temp_dir().join(format!("instances-tree-{}", std::process::id()));
        // 1 → 2 → 3, and 4 apart; each `stat` with its parent as field 4.
        for (pid, parent) in [(1, 0), (2, 1), (3, 2), (4, 0)] {
            let process = folder.join(pid.to_string());
            fs::create_dir_all(&process).expect("a fake process");
            fs::write(
                process.join("stat"),
                format!("{pid} (p) S {parent} 0 0 0 0 0 0 0 0 0 1 2 3 4 0"),
            )
            .expect("its stat");
        }
        let proc = Proc::at(&folder);
        let mut found = tree(&proc, 1);
        found.sort_unstable();
        assert_eq!(found, [1, 2, 3]);
        assert_eq!(tree(&proc, 4), [4]);
        fs::remove_dir_all(&folder).expect("the fake /proc removed");
    }
}
