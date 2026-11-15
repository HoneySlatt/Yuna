//! Measures what XWayland costs in `yuna --headless`: the figures M1.8 asks for (`docs/PLAN.md`,
//! [ADR-0067](../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)).
//!
//! 1. **Its start**, as an X11 program waits for it: from the program's `connect` to the
//!    display's socket to the reply to its first request, `GetInputFocus`; the connection's
//!    setup, which ends when Xwayland accepts it, comes between. *Cold*: the first X11 program
//!    after Yuna started, a new Yuna each time. *Warm*: in one Yuna, once the Xwayland before has
//!    ended when idle, `-terminate`'s 10 s after its last program. `--runs` of each, 30 by
//!    default: their median, 90th percentile and longest; beside them, Yuna's own time from
//!    Xwayland's spawn to its manager owning `WM_S0` (`took_ms` of `Xwayland ready`).
//! 2. **Its memory**: Xwayland's proportional set size (`Pss` in `/proc/<pid>/smaps_rollup`), at
//!    rest with one X11 window of 256 × 256 pixels shown; and Yuna's resident set (`VmRSS`)
//!    before the first X11 program, with that window shown, and once Xwayland has ended.
//!
//! With `--area-filter`, it measures instead what the X11 element's area filter costs a frame
//! ([ADR-0068](../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)), as
//! `x11_element_drawing_time_at_4k` does on llvmpipe in the test's process: a fullscreen X11
//! window of 5760 × 3240 pixels on a 3840 × 2160 output at scale 1, beside an output at 1.5 that
//! makes XWayland's scale 1.5; then, for the plain copy, one of 3840 × 2160 on a 3840 × 2160
//! output at 1.5. The window is filled with another colour without pause, so that each of Yuna's
//! frames draws it whole; for 10 s after 3 s of warm-up, it reads each frame's time on the
//! window's output from Yuna's `frame drawn` lines (`took_us`): from the start of the drawing to
//! the end of its wait for the renderer, as the test's figure. Then it prints what Yuna and
//! Xwayland, its client, used per frame: CPU time and GPU time, by the kernel's statistics.
//!
//! Beside each phase it reads what the machine's *other* programs used, every 2 s
//! ([ADR-0060](../../../docs/adr/0060-development-first-agents-in-their-own-yuna.md)): their
//! processor time, the machine's busy time in `/proc/stat` less this program's and its
//! descendants', Yuna and Xwayland among them, those reaped included; and their GPU time on the
//! engines Yuna draws on, `gfx` and `compute`, by the DRM statistics of every process this user
//! may read but those; a sandbox that hides other processes' files leaves it unknown, and says
//! so. A phase whose other programs used more than 2 cores or 10 % of that GPU time, on
//! average, is marked disturbed; their peak over one reading, this measure's own processor time
//! and the GPU's busy percentage (`gpu_busy_percent`) are printed beside it.
//!
//! Build it and Yuna for release, then run it in the development shell, alone among
//! measurements; it needs no screen of its own:
//!
//! ```sh
//! nix develop -c cargo build --release -p yuna --bin yuna --example xwayland_time
//! nix develop -c target/release/examples/xwayland_time
//! ```
//!
//! Options: `--gpu` draws on the GPU's render node (never in the tests), where Xwayland then draws
//! too; `--runs <n>`; `--output <mode>`, as Yuna takes it, Yuna's own by default, for the start
//! and the memory; `--area-filter`. Yuna finds Xwayland on the shell's `YUNA_XWAYLAND_PATH`. It
//! stops what it started when it ends, and keeps its folder of logs when it fails; killed
//! outright, it leaves Yuna running. It opens no display: run inside `agent-headless`, it cannot
//! read other processes' GPU files, and says so. The figures go to `docs/measurements.md` with
//! the machine and the commit.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{
    PATIENCE, Run, Usage, WARM_UP, field, gpu_engines, percentile, print_usage, shell_variable,
};
use smithay::reexports::rustix::process::{Pid, Signal, kill_process};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeGCAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, PropMode, Rectangle,
    WindowClass,
};
use x11rb::rust_connection::{DefaultStream, RustConnection};
use x11rb::wrapper::ConnectionExt as _;

/// How long Yuna's log must stay the same for Yuna to count as quiet.
const QUIET: Duration = Duration::from_millis(500);
/// How long the X11 window stands shown before the memory is read.
const SETTLE: Duration = Duration::from_secs(2);
/// The side of the X11 window of the memory's phase, in X11 pixels.
const WINDOW_SIDE: u16 = 256;
/// How long the area filter's frames are measured, after [`WARM_UP`].
const MEASURED: Duration = Duration::from_secs(10);
/// How often the other programs' load is read.
const LOAD_PERIOD: Duration = Duration::from_secs(2);
/// Above these, on average over a phase, the other programs disturb it: cores, and percent of the
/// time of the GPU's drawing engines.
const OTHERS_CPU_LIMIT: f64 = 2.0;
const OTHERS_GPU_LIMIT: f64 = 10.0;
/// The GPU's engines Yuna and Xwayland draw on, as the kernel's DRM statistics name them:
/// another program's video decoding, on engines of its own, takes nothing from them.
const DRAWING_ENGINES: [&str; 2] = ["gfx", "compute"];
/// The kernel's clock ticks a second in `/proc/stat` and `/proc/<pid>/stat`, `USER_HZ`: 100 on
/// x86_64, the only machines Yuna runs on (ADR-0006).
const TICKS_A_SECOND: f64 = 100.0;

/// What the command line asks for.
#[derive(Debug, PartialEq)]
struct Options {
    gpu: bool,
    runs: usize,
    output: Option<String>,
    area_filter: bool,
}

fn main() -> ExitCode {
    let options = match read_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(reason) => {
            eprintln!("xwayland_time: {reason}");
            return ExitCode::from(2);
        }
    };
    let run = match Run::new("yuna-xwayland-time") {
        Ok(run) => run,
        Err(reason) => {
            eprintln!("xwayland_time: {reason}");
            return ExitCode::FAILURE;
        }
    };
    match measure(&run, &options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("xwayland_time: {reason}; logs kept in {}", run.keep());
            ExitCode::FAILURE
        }
    }
}

fn read_options(arguments: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        gpu: false,
        runs: 30,
        output: None,
        area_filter: false,
    };
    let mut arguments = arguments;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} needs a value"))
        };
        match argument.as_str() {
            "--gpu" => options.gpu = true,
            "--area-filter" => options.area_filter = true,
            "--output" => options.output = Some(value()?),
            "--runs" => {
                options.runs = value()?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or("--runs takes a whole number above 0")?;
            }
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }
    if options.area_filter && options.output.is_some() {
        return Err("--area-filter chooses its own outputs: it takes no --output".to_owned());
    }
    Ok(options)
}

fn measure(run: &Run, options: &Options) -> Result<(), String> {
    let device = if options.gpu {
        "the GPU (--gpu)"
    } else {
        "llvmpipe"
    };
    if options.area_filter {
        println!("The X11 element's area filter in yuna --headless, drawn on {device}");
        return area_filter(run, options.gpu);
    }
    let output = options.output.as_deref().unwrap_or("Yuna's default");
    println!("XWayland in yuna --headless, drawn on {device}, output {output}");

    let watch = LoadWatch::start();
    let cold = cold_starts(run, options)?;
    report_starts("Cold start, a new Yuna each", &cold, &watch.finish());
    let watch = LoadWatch::start();
    let warm = warm_starts(run, options)?;
    report_starts("Warm start, after an idle exit", &warm, &watch.finish());
    let watch = LoadWatch::start();
    let memory = memory(run, options)?;
    let load = watch.finish();
    println!("Memory, one X11 window of {WINDOW_SIDE} × {WINDOW_SIDE} at rest:");
    println!(
        "  Xwayland: Pss {}, Rss {}",
        mib(memory.xwayland_pss_kib),
        mib(memory.xwayland_rss_kib)
    );
    println!(
        "  Yuna's VmRSS: {} before the first X11 program, {} with the window, {} once \
         Xwayland ended",
        mib(memory.yuna_before_kib),
        mib(memory.yuna_with_kib),
        mib(memory.yuna_after_kib)
    );
    println!("  {load}");
    Ok(())
}

/// The outputs `--output` asks for the start and the memory: Yuna's default without it.
fn outputs(options: &Options) -> &[String] {
    options.output.as_slice()
}

/// One X11 program's wait for XWayland, from its `connect`.
#[derive(Debug, Clone, Copy)]
struct Start {
    /// Until its connection's setup was accepted.
    set_up: Duration,
    /// Until the reply to its first request.
    replied: Duration,
    /// Yuna's own time, from Xwayland's spawn to its manager owning `WM_S0`.
    ready_ms: u64,
}

/// Each run in a new Yuna: the first X11 program after it started.
fn cold_starts(run: &Run, options: &Options) -> Result<Vec<Start>, String> {
    let mut starts = Vec::with_capacity(options.runs);
    for n in 0..options.runs {
        let yuna = Yuna::start(run, options.gpu, outputs(options), &format!("cold-{n}"))?;
        yuna.wait_quiet()?;
        let (mut start, connection) = time_a_start(&yuna)?;
        start.ready_ms = yuna.ready_ms(1)?;
        drop(connection);
        yuna.stop()?;
        starts.push(start);
    }
    Ok(starts)
}

/// In one Yuna, each run once the Xwayland before has ended when idle. The first start, cold,
/// is not counted.
fn warm_starts(run: &Run, options: &Options) -> Result<Vec<Start>, String> {
    let yuna = Yuna::start(run, options.gpu, outputs(options), "warm")?;
    yuna.wait_quiet()?;
    drop(time_a_start(&yuna)?);
    let mut starts = Vec::with_capacity(options.runs);
    for n in 1..=options.runs {
        yuna.wait_for_line("Xwayland stopped", n)?;
        yuna.wait_quiet()?;
        let (mut start, connection) = time_a_start(&yuna)?;
        start.ready_ms = yuna.ready_ms(n + 1)?;
        drop(connection);
        starts.push(start);
    }
    yuna.stop()?;
    Ok(starts)
}

/// Connects to Yuna's display as an X11 program does, through its abstract socket and with its
/// cookie, and times its wait; gives the connection, which the caller drops to leave.
fn time_a_start(yuna: &Yuna) -> Result<(Start, RustConnection), String> {
    let x11 = |error: &dyn std::fmt::Display| format!("X11: {error}");
    let cookie = cookie_of(&yuna.xauthority)?;
    let address = SocketAddr::from_abstract_name(format!("/tmp/.X11-unix/X{}", yuna.display))
        .map_err(|e| x11(&e))?;
    let began = Instant::now();
    let stream = UnixStream::connect_addr(&address).map_err(|e| x11(&e))?;
    let (stream, _) = DefaultStream::from_unix_stream(stream).map_err(|e| x11(&e))?;
    let connection = RustConnection::connect_to_stream_with_auth_info(
        stream,
        0,
        b"MIT-MAGIC-COOKIE-1".to_vec(),
        cookie,
    )
    .map_err(|e| x11(&e))?;
    let set_up = began.elapsed();
    connection
        .get_input_focus()
        .map_err(|e| x11(&e))?
        .reply()
        .map_err(|e| x11(&e))?;
    let replied = began.elapsed();
    let start = Start {
        set_up,
        replied,
        ready_ms: 0,
    };
    Ok((start, connection))
}

/// What the memory's phase read, in KiB.
struct Memory {
    xwayland_pss_kib: u64,
    xwayland_rss_kib: u64,
    yuna_before_kib: u64,
    yuna_with_kib: u64,
    yuna_after_kib: u64,
}

/// In a new Yuna: its resident set before any X11 program; then Xwayland's and Yuna's with one
/// window shown and at rest; then Yuna's once Xwayland has ended.
fn memory(run: &Run, options: &Options) -> Result<Memory, String> {
    let yuna = Yuna::start(run, options.gpu, outputs(options), "memory")?;
    yuna.wait_quiet()?;
    let yuna_before_kib = status_kib(yuna.pid(), "VmRSS")?;
    let (_, connection) = time_a_start(&yuna)?;
    open_window(&connection)?;
    yuna.wait_for_line("window opened", 1)?;
    yuna.wait_quiet()?;
    std::thread::sleep(SETTLE);
    let xwayland = yuna.xwayland_pid()?;
    let xwayland_pss_kib = smaps_rollup_kib(xwayland, "Pss")?;
    let xwayland_rss_kib = smaps_rollup_kib(xwayland, "Rss")?;
    let yuna_with_kib = status_kib(yuna.pid(), "VmRSS")?;
    drop(connection);
    yuna.wait_for_line("Xwayland stopped", 1)?;
    yuna.wait_quiet()?;
    let yuna_after_kib = status_kib(yuna.pid(), "VmRSS")?;
    yuna.stop()?;
    Ok(Memory {
        xwayland_pss_kib,
        xwayland_rss_kib,
        yuna_before_kib,
        yuna_with_kib,
        yuna_after_kib,
    })
}

/// Makes and maps a window of [`WINDOW_SIDE`] pixels, its background a colour the server
/// paints, so that it draws nothing itself.
fn open_window(connection: &RustConnection) -> Result<(), String> {
    let x11 = |error: &dyn std::fmt::Display| format!("X11: {error}");
    let screen = &connection.setup().roots[0];
    let window = connection.generate_id().map_err(|e| x11(&e))?;
    connection
        .create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            WINDOW_SIDE,
            WINDOW_SIDE,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().background_pixel(0x0033_6699),
        )
        .map_err(|e| x11(&e))?;
    connection.map_window(window).map_err(|e| x11(&e))?;
    connection.flush().map_err(|e| x11(&e))
}

/// A case of the area filter's phase: what it draws, Yuna's outputs, the first the window's, and
/// the size X11 gives the window there, fullscreen.
struct FilterCase {
    what: &'static str,
    outputs: &'static [&'static str],
    window: (u16, u16),
}

const FILTER_CASES: [FilterCase; 2] = [
    FilterCase {
        what: "5760 × 3240 to 3840 × 2160, area filter",
        outputs: &["3840x2160@60,1", "1920x1080@60,1.5"],
        window: (5760, 3240),
    },
    FilterCase {
        what: "3840 × 2160 copied at 1.5",
        outputs: &["3840x2160@60,1.5"],
        window: (3840, 2160),
    },
];

/// Each of [`FILTER_CASES`] in a new Yuna: a fullscreen X11 window filled again without pause,
/// and the time of each frame Yuna drew on its output.
fn area_filter(run: &Run, gpu: bool) -> Result<(), String> {
    for (n, case) in FILTER_CASES.iter().enumerate() {
        let watch = LoadWatch::start();
        let yuna = Yuna::start(run, gpu, case.outputs, &format!("filter-{n}"))?;
        yuna.wait_quiet()?;
        let (_, connection) = time_a_start(&yuna)?;
        let window = open_fullscreen_window(&connection, case.window)?;
        yuna.wait_for_line("window opened", 1)?;
        refill(&connection, window, case.window, WARM_UP)?;
        let xwayland = yuna.xwayland_pid()?;
        let measured_from = yuna.read_log().len();
        let before = usage_of(yuna.pid(), xwayland)?;
        refill(&connection, window, case.window, MEASURED)?;
        let after = usage_of(yuna.pid(), xwayland)?;
        let log = yuna.read_log();
        drop(connection);
        yuna.stop()?;
        let load = watch.finish();
        let mut times: Vec<u64> = log
            .get(measured_from..)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains("frame drawn") && field(line, "output") == Some(1))
            .filter_map(|line| field(line, "took_us"))
            .collect();
        if times.is_empty() {
            return Err(format!("{}: no frame drawn on its output", case.what));
        }
        times.sort_unstable();
        println!(
            "{}: {} frames in {} s; a frame: median {} µs, 90th percentile {} µs, longest {} µs",
            case.what,
            times.len(),
            MEASURED.as_secs(),
            percentile(&times, 50.0),
            percentile(&times, 90.0),
            times[times.len() - 1]
        );
        print_usage(&before, &after, times.len() as u64);
        println!("  {load}");
    }
    Ok(())
}

/// What Yuna, process `yuna`, and Xwayland, process `xwayland`, have used so far: a [`Usage`]
/// whose client is Xwayland, Yuna's own child rather than this program's.
fn usage_of(yuna: u32, xwayland: u32) -> Result<Usage, String> {
    let mut usage = Usage::take(yuna, &[])?;
    let xwayland = Usage::take(xwayland, &[])?;
    usage.clients_threads = xwayland
        .yuna_threads
        .into_iter()
        .map(|(thread, (_, ns))| (thread, ns))
        .collect();
    usage.clients_gpu = xwayland.yuna_gpu;
    Ok(usage)
}

/// Makes a window that asks to be fullscreen before it maps, maps it, and waits until X11 gives
/// it `size`: on Yuna's first output, which a new window opens on.
fn open_fullscreen_window(connection: &RustConnection, size: (u16, u16)) -> Result<u32, String> {
    let x11 = |error: &dyn std::fmt::Display| format!("X11: {error}");
    let atom = |name: &[u8]| {
        connection
            .intern_atom(false, name)
            .map_err(|e| x11(&e))?
            .reply()
            .map(|reply| reply.atom)
            .map_err(|e| x11(&e))
    };
    let (state, fullscreen) = (atom(b"_NET_WM_STATE")?, atom(b"_NET_WM_STATE_FULLSCREEN")?);
    let screen = &connection.setup().roots[0];
    let window = connection.generate_id().map_err(|e| x11(&e))?;
    connection
        .create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            WINDOW_SIDE,
            WINDOW_SIDE,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().background_pixel(0),
        )
        .map_err(|e| x11(&e))?;
    connection
        .change_property32(
            PropMode::REPLACE,
            window,
            state,
            AtomEnum::ATOM,
            &[fullscreen],
        )
        .map_err(|e| x11(&e))?;
    connection.map_window(window).map_err(|e| x11(&e))?;
    connection.flush().map_err(|e| x11(&e))?;
    let deadline = Instant::now() + PATIENCE;
    loop {
        let geometry = connection
            .get_geometry(window)
            .map_err(|e| x11(&e))?
            .reply()
            .map_err(|e| x11(&e))?;
        if (geometry.width, geometry.height) == size {
            return Ok(window);
        }
        if Instant::now() > deadline {
            return Err(format!(
                "the window is {} × {}, not the {} × {} of its output",
                geometry.width, geometry.height, size.0, size.1
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Fills `window`, of `size`, with another colour again and again for `how_long`, each fill
/// awaited, so that Xwayland always has the whole window to send at Yuna's next frame.
fn refill(
    connection: &RustConnection,
    window: u32,
    size: (u16, u16),
    how_long: Duration,
) -> Result<(), String> {
    let x11 = |error: &dyn std::fmt::Display| format!("X11: {error}");
    let gc = connection.generate_id().map_err(|e| x11(&e))?;
    connection
        .create_gc(gc, window, &CreateGCAux::new())
        .map_err(|e| x11(&e))?;
    let whole = Rectangle {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
    };
    let until = Instant::now() + how_long;
    let mut colour: u32 = 0;
    while Instant::now() < until {
        colour = colour.wrapping_add(0x0001_0203) & 0x00ff_ffff;
        connection
            .change_gc(gc, &ChangeGCAux::new().foreground(colour))
            .map_err(|e| x11(&e))?;
        connection
            .poly_fill_rectangle(window, gc, &[whole])
            .map_err(|e| x11(&e))?;
        connection
            .get_input_focus()
            .map_err(|e| x11(&e))?
            .reply()
            .map_err(|e| x11(&e))?;
        // About two fills a refresh at 60 Hz: Xwayland sends the window at Yuna's frame
        // callbacks, whatever the fills between.
        std::thread::sleep(Duration::from_millis(8));
    }
    connection.free_gc(gc).map_err(|e| x11(&e))?;
    Ok(())
}

/// The cookie of the Xauthority file at `path`: its one entry's last field, after the family,
/// the address, the display's number and the cookie's name, each but the family led by its
/// length.
fn cookie_of(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut at = 2;
    let mut field = Vec::new();
    for _ in 0..4 {
        let length = bytes
            .get(at..at + 2)
            .map(|length| usize::from(u16::from_be_bytes([length[0], length[1]])))
            .ok_or("a short Xauthority")?;
        field = bytes
            .get(at + 2..at + 2 + length)
            .ok_or("a short Xauthority")?
            .to_vec();
        at += 2 + length;
    }
    Ok(field)
}

/// A `key:  <n> kB` line of `/proc/<pid>/status`, in KiB.
fn status_kib(pid: u32, key: &str) -> Result<u64, String> {
    let status = fs::read_to_string(format!("/proc/{pid}/status"))
        .map_err(|e| format!("process {pid}'s status: {e}"))?;
    kib_of(&status, key).ok_or_else(|| format!("no {key} in process {pid}'s status"))
}

/// A `key:  <n> kB` line of `/proc/<pid>/smaps_rollup`, in KiB.
fn smaps_rollup_kib(pid: u32, key: &str) -> Result<u64, String> {
    let rollup = fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
        .map_err(|e| format!("process {pid}'s smaps_rollup: {e}"))?;
    kib_of(&rollup, key).ok_or_else(|| format!("no {key} in process {pid}'s smaps_rollup"))
}

/// The value of the line `key:  <n> kB` of `text`.
fn kib_of(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let value = line.strip_prefix(key)?.strip_prefix(':')?;
        value.trim().strip_suffix("kB")?.trim().parse().ok()
    })
}

fn mib(kib: u64) -> String {
    format!("{:.1} MiB", kib as f64 / 1024.0)
}

/// Prints the median, 90th percentile and longest of each figure of `starts`.
fn report_starts(title: &str, starts: &[Start], load: &Load) {
    let micros = |pick: fn(&Start) -> Duration| {
        let mut values: Vec<u64> = starts
            .iter()
            .map(|start| pick(start).as_micros() as u64)
            .collect();
        values.sort_unstable();
        values
    };
    let millis = |micros: u64| format!("{:.2} ms", micros as f64 / 1000.0);
    println!("{title}: {} runs", starts.len());
    for (what, values) in [
        ("connect to its first reply", micros(|start| start.replied)),
        (
            "connect to its setup accepted",
            micros(|start| start.set_up),
        ),
        (
            "Yuna's spawn to WM_S0",
            micros(|start| Duration::from_millis(start.ready_ms)),
        ),
    ] {
        println!(
            "  {what}: median {}, 90th percentile {}, longest {}",
            millis(percentile(&values, 50.0)),
            millis(percentile(&values, 90.0)),
            millis(values[values.len() - 1])
        );
    }
    println!("  {load}");
}

/// A `yuna --headless`, its log, and its X display.
struct Yuna {
    child: Child,
    log: PathBuf,
    display: u32,
    xauthority: PathBuf,
}

impl Yuna {
    /// Starts the `yuna` built beside this program, on the GPU or not, with `outputs`, logging at
    /// `debug` to `yuna-<name>.log`, with nothing of the caller's environment but what it draws
    /// with and Xwayland's folder; reads its display from what it prints.
    fn start(
        run: &Run,
        gpu: bool,
        outputs: &[impl AsRef<str>],
        name: &str,
    ) -> Result<Yuna, String> {
        let program = std::env::current_exe()
            .ok()
            .and_then(|me| Some(me.parent()?.parent()?.join("yuna")))
            .filter(|path| path.exists())
            .ok_or("no yuna beside this program: build it with --bin yuna")?;
        let log = run.folder().join(format!("yuna-{name}.log"));
        let mut command = Command::new(program);
        command.arg("--headless");
        if gpu {
            command.arg("--gpu");
        }
        for mode in outputs {
            command.args(["--output", mode.as_ref()]);
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
            .env("HOME", run.folder().join("yuna-home"))
            .env("XDG_RUNTIME_DIR", run.folder().join("runtime"))
            .env("YUNA_LOG", "debug")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(File::create(&log).map_err(|e| format!("Yuna's log: {e}"))?);
        let mut child = command
            .spawn()
            .map_err(|e| format!("yuna does not start: {e}"))?;
        let stdout = child.stdout.take().ok_or("Yuna's standard output")?;
        // Yuna writes its three lines at once, then nothing more.
        let mut lines = BufReader::new(stdout).lines();
        let mut variable = |name: &str| {
            let line = lines
                .next()
                .and_then(Result::ok)
                .ok_or_else(|| format!("yuna printed no {name}"))?;
            line.strip_prefix(name)
                .and_then(|line| line.strip_prefix('='))
                .map(str::to_owned)
                .ok_or_else(|| format!("yuna printed {line:?}, not {name}"))
        };
        variable("WAYLAND_DISPLAY")?;
        let display = variable("DISPLAY")?;
        let xauthority = PathBuf::from(variable("XAUTHORITY")?);
        let display = display
            .strip_prefix(':')
            .and_then(|number| number.parse().ok())
            .ok_or_else(|| format!("DISPLAY is {display}, not :<number>"))?;
        Ok(Yuna {
            child,
            log,
            display,
            xauthority,
        })
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn read_log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Waits until its log has not grown for [`QUIET`].
    fn wait_quiet(&self) -> Result<(), String> {
        let deadline = Instant::now() + PATIENCE;
        let mut length = self.read_log().len();
        let mut since = Instant::now();
        while since.elapsed() < QUIET {
            std::thread::sleep(Duration::from_millis(20));
            let now = self.read_log().len();
            if now != length {
                length = now;
                since = Instant::now();
            }
            if Instant::now() > deadline {
                return Err("Yuna never went quiet".to_owned());
            }
        }
        Ok(())
    }

    /// Waits until its log holds `count` lines that contain `text`, and gives the last.
    fn wait_for_line(&self, text: &str, count: usize) -> Result<String, String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let log = self.read_log();
            if let Some(line) = log
                .lines()
                .filter(|line| line.contains(text))
                .nth(count - 1)
            {
                return Ok(line.to_owned());
            }
            if Instant::now() > deadline {
                return Err(format!("Yuna did not log {text:?} {count} times"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Yuna's own time for its `count`-th Xwayland, from its spawn to `WM_S0`.
    fn ready_ms(&self, count: usize) -> Result<u64, String> {
        let line = self.wait_for_line("Xwayland ready", count)?;
        field(&line, "took_ms").ok_or_else(|| format!("no took_ms in {line:?}"))
    }

    /// The process id of its last Xwayland.
    fn xwayland_pid(&self) -> Result<u32, String> {
        let log = self.read_log();
        let line = log
            .lines()
            .rfind(|line| line.contains("Xwayland started"))
            .ok_or("no Xwayland started")?;
        field(line, "pid")
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(|| format!("no pid in {line:?}"))
    }

    /// Stops Yuna with SIGTERM, as an agent does, and requires that it ends well.
    fn stop(mut self) -> Result<(), String> {
        let terminated = kill_process(Pid::from_child(&self.child), Signal::TERM).is_ok();
        let deadline = Instant::now() + Duration::from_secs(10);
        while terminated && Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("yuna ended with {status}"))
                };
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err("yuna did not stop on SIGTERM".to_owned())
    }
}

impl Drop for Yuna {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What the machine's other programs use over a phase, read on a thread of its own every
/// [`LOAD_PERIOD`], and at the phase's start and end.
struct LoadWatch {
    readings: Arc<Mutex<Vec<Reading>>>,
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl LoadWatch {
    fn start() -> LoadWatch {
        let readings = Arc::new(Mutex::new(vec![Reading::take()]));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (readings, stop) = (Arc::clone(&readings), Arc::clone(&stop));
            std::thread::spawn(move || {
                let mut next = Instant::now() + LOAD_PERIOD;
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                    if Instant::now() >= next {
                        readings.lock().expect("readings").push(Reading::take());
                        next += LOAD_PERIOD;
                    }
                }
            })
        };
        LoadWatch {
            readings,
            stop,
            thread,
        }
    }

    fn finish(self) -> Load {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
        let mut readings = std::mem::take(&mut *self.readings.lock().expect("readings"));
        readings.push(Reading::take());
        Load::of(&readings)
    }
}

/// One reading of what the machine has done so far.
#[derive(Debug, Clone, PartialEq)]
struct Reading {
    at: Instant,
    /// Every processor's busy time, in clock ticks: `/proc/stat`'s first line but its idle and
    /// its wait for disks.
    busy_ticks: u64,
    /// This program's and its descendants' processor time, in clock ticks, the time of the
    /// children each has reaped included: Yuna, Xwayland and the X11 client are among them.
    own_ticks: u64,
    /// Each other process's GPU time, per engine, in nanoseconds; those this user may not read
    /// are missing.
    others_gpu: BTreeMap<u32, BTreeMap<String, u64>>,
    /// The busiest GPU's `gpu_busy_percent`, the measure's own work included.
    gpu_busy: u32,
}

impl Reading {
    fn take() -> Reading {
        let processes = processes();
        let mut own = BTreeSet::from([std::process::id()]);
        // Parents come before their children in no set order: walk until nothing is added.
        let mut grew = true;
        while grew {
            grew = false;
            for process in &processes {
                if own.contains(&process.parent) && own.insert(process.pid) {
                    grew = true;
                }
            }
        }
        let own_ticks = processes
            .iter()
            .filter(|process| own.contains(&process.pid))
            .map(|process| process.ticks)
            .sum();
        let others_gpu = processes
            .iter()
            .filter(|process| !own.contains(&process.pid))
            .map(|process| (process.pid, gpu_engines(process.pid)))
            .filter(|(_, engines)| !engines.is_empty())
            .collect();
        Reading {
            at: Instant::now(),
            busy_ticks: busy_ticks(),
            own_ticks,
            others_gpu,
            gpu_busy: gpu_busy(),
        }
    }
}

/// A process, as its `/proc/<pid>/stat` gives it: its parent, and its processor time and its
/// reaped children's, in clock ticks.
struct Process {
    pid: u32,
    parent: u32,
    ticks: u64,
}

/// Every process the machine runs now; those that end while they are read are left out.
fn processes() -> Vec<Process> {
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse().ok()?;
            let stat = fs::read_to_string(entry.path().join("stat")).ok()?;
            let (parent, ticks) = parse_stat(&stat)?;
            Some(Process { pid, parent, ticks })
        })
        .collect()
}

/// A process's parent and its processor time from its `stat`: `utime`, `stime`, `cutime` and
/// `cstime`, fields 14 to 17, read after the last `)`, which ends its name.
fn parse_stat(stat: &str) -> Option<(u32, u64)> {
    let fields: Vec<&str> = stat
        .get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .collect();
    // After the name come the state, field 3, and the parent, field 4.
    let parent = fields.get(1)?.parse().ok()?;
    let ticks = fields
        .get(11..15)?
        .iter()
        .map(|field| field.parse::<u64>().ok())
        .sum::<Option<u64>>()?;
    Some((parent, ticks))
}

/// Every processor's busy time so far, in clock ticks.
fn busy_ticks() -> u64 {
    fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|stat| parse_busy_ticks(&stat))
        .unwrap_or(0)
}

/// The busy time of `/proc/stat`'s first line: user, nice, system, irq, softirq and steal, its
/// idle time and its wait for disks left out.
fn parse_busy_ticks(stat: &str) -> Option<u64> {
    let fields: Vec<u64> = stat
        .lines()
        .next()?
        .strip_prefix("cpu ")?
        .split_whitespace()
        .map(|field| field.parse().ok())
        .collect::<Option<_>>()?;
    let [user, nice, system, _idle, _iowait, irq, softirq, steal, ..] = fields[..] else {
        return None;
    };
    Some(user + nice + system + irq + softirq + steal)
}

/// The busiest GPU's `gpu_busy_percent`; 0 where none can be read.
fn gpu_busy() -> u32 {
    fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|card| {
            fs::read_to_string(card.path().join("device/gpu_busy_percent"))
                .ok()?
                .trim()
                .parse()
                .ok()
        })
        .max()
        .unwrap_or(0)
}

/// What the other programs used over a phase, from its readings: on average, and at most over
/// one period between two readings.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Load {
    /// Their processor time, in cores.
    others_cpu: (f64, f64),
    /// Their GPU time on its drawing engines, [`DRAWING_ENGINES`], in percent of the time.
    others_gpu: (f64, f64),
    /// The most other processes whose GPU files a reading could read. None means their GPU
    /// time is unknown, not nil: a desktop's compositor holds one at least, and a sandbox may
    /// hide other processes' files.
    gpu_processes: usize,
    /// This measure's own processor time, in cores, on average.
    own_cpu: f64,
    /// The busiest GPU's busy percentage, at the first reading, the last, and at most.
    gpu_busy: (u32, u32, u32),
}

impl Load {
    /// The load of `readings`, which hold two at least, in order.
    fn of(readings: &[Reading]) -> Load {
        let (first, last) = (&readings[0], &readings[readings.len() - 1]);
        let others_cpu = |from: &Reading, to: &Reading| {
            let busy = to.busy_ticks.saturating_sub(from.busy_ticks);
            let own = to.own_ticks.saturating_sub(from.own_ticks);
            busy.saturating_sub(own) as f64 / TICKS_A_SECOND / seconds(from, to)
        };
        let others_gpu = |from: &Reading, to: &Reading| {
            let mut used = 0;
            for (pid, after) in &to.others_gpu {
                // A process that started between the readings counts from the next one.
                let Some(before) = from.others_gpu.get(pid) else {
                    continue;
                };
                for (engine, ns) in after {
                    if DRAWING_ENGINES.contains(&engine.as_str()) {
                        used += ns.saturating_sub(before.get(engine).copied().unwrap_or(*ns));
                    }
                }
            }
            used as f64 / 1e9 / seconds(from, to) * 100.0
        };
        let peak = |of: &dyn Fn(&Reading, &Reading) -> f64| {
            readings
                .windows(2)
                .map(|pair| of(&pair[0], &pair[1]))
                .fold(0.0, f64::max)
        };
        let own = last.own_ticks.saturating_sub(first.own_ticks) as f64 / TICKS_A_SECOND;
        Load {
            others_cpu: (others_cpu(first, last), peak(&others_cpu)),
            others_gpu: (others_gpu(first, last), peak(&others_gpu)),
            own_cpu: own / seconds(first, last),
            gpu_processes: readings
                .iter()
                .map(|reading| reading.others_gpu.len())
                .max()
                .unwrap_or(0),
            gpu_busy: (
                first.gpu_busy,
                last.gpu_busy,
                readings
                    .iter()
                    .map(|reading| reading.gpu_busy)
                    .max()
                    .unwrap_or(0),
            ),
        }
    }

    fn disturbed(&self) -> bool {
        self.others_cpu.0 > OTHERS_CPU_LIMIT || self.others_gpu.0 > OTHERS_GPU_LIMIT
    }

    /// What the other programs' GPU time was, or why it is not known.
    fn others_gpu(&self) -> String {
        if self.gpu_processes == 0 {
            return "no other process's GPU time could be read".to_owned();
        }
        format!(
            "{:.1} % of the GPU's drawing on average, {:.1} % at most, by {} processes",
            self.others_gpu.0, self.others_gpu.1, self.gpu_processes
        )
    }
}

/// The seconds between two readings, never 0.
fn seconds(from: &Reading, to: &Reading) -> f64 {
    (to.at - from.at).as_secs_f64().max(1e-3)
}

impl std::fmt::Display for Load {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "other programs: {:.2} cores on average, {:.2} at most, and {}; this measure: {:.2} \
             cores; the GPU {} % busy before, {} % after, {} % at most: {}",
            self.others_cpu.0,
            self.others_cpu.1,
            self.others_gpu(),
            self.own_cpu,
            self.gpu_busy.0,
            self.gpu_busy.1,
            self.gpu_busy.2,
            if self.disturbed() {
                "disturbed"
            } else {
                "quiet"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_to_thirty_runs_on_llvmpipe() {
        let read = |arguments: &[&str]| read_options(arguments.iter().map(|a| (*a).to_owned()));
        assert_eq!(
            read(&[]),
            Ok(Options {
                gpu: false,
                runs: 30,
                output: None,
                area_filter: false,
            })
        );
        assert_eq!(
            read(&["--gpu", "--runs", "5", "--output", "1920x1080@60,1.5"]),
            Ok(Options {
                gpu: true,
                runs: 5,
                output: Some("1920x1080@60,1.5".to_owned()),
                area_filter: false,
            })
        );
        assert!(read(&["--area-filter", "--gpu"]).is_ok_and(|options| options.area_filter));
        assert!(read(&["--area-filter", "--output", "1920x1080@60,1"]).is_err());
        assert!(read(&["--runs", "0"]).is_err());
        assert!(read(&["--runs"]).is_err());
        assert!(read(&["--frames"]).is_err());
    }

    #[test]
    fn memory_lines_are_read_in_kib() {
        let rollup = "Rss:               41236 kB\nPss:               20512 kB\nPss_Dirty:  9 kB\n";
        assert_eq!(kib_of(rollup, "Pss"), Some(20_512));
        assert_eq!(kib_of(rollup, "Rss"), Some(41_236));
        assert_eq!(kib_of(rollup, "Swap"), None);
    }

    #[test]
    fn a_process_stat_gives_its_parent_and_its_time_with_its_reaped_childrens() {
        // Its name holds a space and a parenthesis; utime 7, stime 3, cutime 20, cstime 5.
        let stat = "4242 (Web (Content) 1) S 4200 4242 4200 0 -1 4194560 100 0 0 0 7 3 20 5 20 0 \
                    1 0 1000 1000000 100 18446744073709551615";
        assert_eq!(parse_stat(stat), Some((4200, 35)));
        assert_eq!(parse_stat("4242 (cut"), None);
    }

    #[test]
    fn the_machines_busy_time_leaves_idle_and_disk_waits_out() {
        let stat = "cpu  100 2 30 5000 40 5 6 7 0 0\ncpu0 50 1 15 2500 20 2 3 3 0 0\n";
        assert_eq!(parse_busy_ticks(stat), Some(100 + 2 + 30 + 5 + 6 + 7));
        assert_eq!(parse_busy_ticks("intr 1 2 3\n"), None);
    }

    #[test]
    fn the_other_programs_load_leaves_this_measures_own_out() {
        let at = Instant::now();
        let reading = |seconds: u64, busy_ticks, own_ticks, gfx_ns: Option<u64>| Reading {
            at: at + Duration::from_secs(seconds),
            busy_ticks,
            own_ticks,
            // Process 7's video decoding, on an engine of its own, never counts.
            others_gpu: gfx_ns
                .map(|ns| {
                    let engines = [("gfx".to_owned(), ns), ("dec".to_owned(), ns * 3)];
                    BTreeMap::from([(7, BTreeMap::from(engines))])
                })
                .unwrap_or_default(),
            gpu_busy: 5,
        };
        // Over 4 s: 1,000 ticks busy, 600 of them this measure's; process 7 used the GPU's gfx
        // engine 0.2 s, all of it in the second half.
        let load = Load::of(&[
            reading(0, 0, 0, Some(0)),
            reading(2, 300, 200, Some(0)),
            reading(4, 1_000, 600, Some(200_000_000)),
        ]);
        assert!((load.others_cpu.0 - 1.0).abs() < 1e-9, "{load:?}");
        assert!((load.others_cpu.1 - 1.5).abs() < 1e-9, "{load:?}");
        assert!((load.own_cpu - 1.5).abs() < 1e-9, "{load:?}");
        assert!((load.others_gpu.0 - 5.0).abs() < 1e-9, "{load:?}");
        assert!((load.others_gpu.1 - 10.0).abs() < 1e-9, "{load:?}");
        assert!(!load.disturbed());
        assert_eq!(load.gpu_processes, 1);
        assert!(load.to_string().contains("by 1 processes"), "{load}");
        // A process that appears counts from its second reading.
        let appeared = Load::of(&[
            reading(0, 0, 0, None),
            reading(2, 0, 0, Some(5_000_000_000)),
        ]);
        assert_eq!(appeared.others_gpu, (0.0, 0.0));
        let busy = Load::of(&[reading(0, 0, 0, None), reading(1, 300, 0, None)]);
        assert!(busy.disturbed());
        // No other process's GPU files read: unknown, not nil.
        assert!(
            busy.to_string()
                .contains("no other process's GPU time could be read")
        );
    }
}
