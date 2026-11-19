//! Opium, the owner's music player, in `yuna mcp`, driven as an agent drives it, through MCP
//! alone: Stage 1's exit
//! ([ADR-0104](../../../docs/adr/0104-stage-1s-exit-opium-in-aeriths-place-and-its-exceptions-written.md)).
//!
//! The test is ignored unless `YUNA_OPIUM` names Opium's `bin/opium`, which the weekly job
//! (`.github/workflows/opium.yml`) builds from Opium's Nix package at the revision it pins. An
//! agent creates a seat, starts Opium there on a throwaway profile and waits for its window. It
//! captures the window once it stands still and checks its pixels: Opium's Home page in its
//! Stock theme, which a blank window, another program's or another page fails. It clicks Songs
//! in Opium's sidebar, on the pixels of that capture, and checks by capture that Opium shows
//! its Songs page. Once Opium is idle, Yuna must rest: 10 s with no context switch in any of its
//! threads and no frame, commit or wake-up, as `tests/applications.rs` measures it
//! ([ADR-0025](../../../docs/adr/0025-applications-in-ci.md)). Closing the seat ends Opium,
//! which wrote its profile in the test's folder alone. A test beside it, without Opium, at every
//! check, shows that the check of a capture refuses a blank window and another one.
//!
//! Opium runs in a network namespace of its own, with no interface but its loopback, which stays
//! down. Without an account it reaches no service: Apple Music, SoundCloud and Navidrome find no
//! sign-in and stay off. But Opium Connect, its link to the owner's other devices, would listen
//! on a TCP port, announce the machine over mDNS and reach iroh's relays as it starts. The
//! namespace is made by `unshare`, found in the test's own `PATH`: it is the program `spawn`
//! starts, and becomes Opium. Opium's profile, its settings, caches and data, is a folder of the
//! test's (`HOME` and the XDG folders, given through `spawn`'s `env`), and its Widevine module
//! none, so that it loads no browser's, Chrome's on CI's runners among them. Its environment is
//! otherwise the one `tests/applications.rs` gives every application: Mesa on the CPU, lavapipe
//! alone for Vulkan, DejaVu alone. Opium's package puts its own Wayland, xkbcommon, Vulkan
//! loader and libglvnd first in its libraries; they load the shell's Mesa, which holds while that
//! Mesa needs no newer glibc than Opium's nixpkgs gives: 2.44 in both at the revisions pinned on
//! 2026-11-18. Opium has no session bus in `yuna mcp`: it prints that its media controls and its
//! downloads' keyring are unavailable, and that Opium Connect cannot look for devices, the
//! network being unreachable.

#[path = "common/application_environment.rs"]
mod application_environment;
#[path = "common/dies_with_test.rs"]
mod dies_with_test;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, DirBuilder, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use application_environment::{application_environment, shell_variable};
use dies_with_test::dies_with_test;
use serde_json::{Value, json};
use smithay::reexports::rustix::process::{Pid, Signal, kill_process};

/// How long Yuna must do nothing: the rest window of the bar (ROADMAP's quality bar).
const REST: Duration = Duration::from_secs(10);
/// How long Yuna must go without a round before Opium counts as idle.
const QUIET: Duration = Duration::from_secs(2);
/// The longest a step may take: Opium's first frames on lavapipe, in a debug build of Yuna on
/// CI's slow runners.
const PATIENCE: Duration = Duration::from_secs(60);
/// How often the test reads Yuna's log, captures or lists programs; reading the log never wakes
/// Yuna.
const POLL: Duration = Duration::from_millis(50);
/// How long a capture must stay the same to count as still.
const STILL: Duration = Duration::from_secs(1);

/// The seat's one output: 1920 × 1080 at 60 Hz and scale 1, where a window is placed on whole
/// pixels and a capture's pixel is a logical unit.
const OUTPUT: (u32, u32) = (1920, 1080);

/// The size of Opium's window as the seat's output tiles it, half the output, which the places
/// below assume: Opium then folds its sidebar to a column of icons.
const WINDOW: (u32, u32) = (936, 1048);

/// A rectangle of a capture of Opium's window: its left, top, width and height, in pixels from
/// the window's top-left corner.
type Area = (u32, u32, u32, u32);

/// Where Opium's window shows what the story checks, at the pinned revision: the rows of Home
/// and Songs in its sidebar, a part of the sidebar's column below its rows, which holds nothing,
/// and the page's heading, "Home", then "Songs".
const HOME_ROW: Area = (9, 55, 57, 33);
const SONGS_ROW: Area = (9, 231, 57, 33);
const EMPTY_SIDEBAR: Area = (0, 420, 72, 540);
const HEADING: Area = (95, 55, 205, 60);
/// The middle of Songs' row, where the agent clicks.
const SONGS: (u32, u32) = (37, 247);

/// The colours of Opium's Stock theme, which a new profile shows (Opium's
/// `crates/opium-core/src/theme.rs`): the page, the sidebar, the text and the accent; and the
/// active row's background, the text at 11 % over the sidebar (`crates/app-desktop/src/ui/
/// sidebar.rs`), where the active row's icon is drawn in the accent.
const PAGE: [u8; 3] = [0x16, 0x16, 0x16];
const SIDEBAR: [u8; 3] = [0x10, 0x10, 0x10];
const TEXT: [u8; 3] = [0xf2, 0xf4, 0xf8];
const ACCENT: [u8; 3] = [0xfa, 0x2d, 0x48];
const ACTIVE_ROW: [u8; 3] = [0x29, 0x29, 0x29];
/// How far a pixel's channel may be from a theme's colour and still be it: Opium fills its
/// areas with exact colours; this leaves room for a renderer that rounds otherwise.
const TOLERANCE: u8 = 2;
/// The fewest pixels of the accent in an active row's icon, and of the text in a heading: about
/// 30, and 400 for Home's heading and 900 for Songs', at the pinned revision.
const ICON_PIXELS: usize = 10;
const HEADING_PIXELS: usize = 100;

/// The `opium` program, its package's `bin/opium`. The weekly job builds it and names it in
/// `YUNA_OPIUM`, then runs the ignored test: `cargo test --test opium -- --ignored`.
fn opium() -> PathBuf {
    std::env::var_os("YUNA_OPIUM")
        .map(PathBuf::from)
        .expect("YUNA_OPIUM names Opium's bin/opium")
}

/// An agent opens Opium on its seat and drives it through MCP alone (ADR-0104).
#[test]
#[ignore = "needs Opium, which the weekly job builds and names in YUNA_OPIUM"]
fn an_agent_opens_opium_clicks_in_it_checks_it_by_capture_and_yuna_rests() {
    let report = opium_story().unwrap_or_else(|failure| panic!("{failure}"));
    eprintln!("{report}");
}

fn opium_story() -> Result<String, String> {
    let yuna = Yuna::start();
    let (width, height) = OUTPUT;
    let seat = yuna.create_seat(
        json!({"action": "create", "width": width, "height": height, "rate": 60, "scale": 1}),
    )?;
    let started = Instant::now();
    let program = yuna.start_opium(seat)?;
    let failure = |what: String| format!("Opium: {what}\n{}", yuna.diagnosis(&program.log));
    let window = yuna.opium_window(seat).map_err(failure)?;
    let shown = started.elapsed();
    let size = (
        window["rect"]["width"].as_f64(),
        window["rect"]["height"].as_f64(),
    );
    if size != (Some(f64::from(WINDOW.0)), Some(f64::from(WINDOW.1))) {
        return Err(failure(format!(
            "its window is {size:?}, not the {WINDOW:?} the story's places assume"
        )));
    }
    let pid = window["pid"].as_i64().ok_or("no pid")?;
    no_network_but_loopback(pid).map_err(failure)?;

    let whole_window = json!({"seat": seat, "window": window["id"]});
    let home = yuna.still_capture(&whole_window).map_err(failure)?;
    yuna.keep(&home.png, "opium-home.png").map_err(failure)?;
    let home_page = Image::from_png(&home.png)
        .and_then(|image| shows_page(&image, HOME_ROW, SONGS_ROW))
        .map_err(|what| failure(format!("its first capture is not its Home page: {what}")))?;

    // The agent clicks what it saw, in the pixels of that capture.
    let clicked = yuna
        .call(
            "pointer",
            json!({
                "action": "click", "seat": seat, "capture": home.id,
                "x": SONGS.0, "y": SONGS.1,
            }),
        )
        .map_err(failure)?;
    if clicked["structuredContent"]["pointer"]["window"] != window["id"] {
        return Err(failure(format!(
            "the click was not on its window: {clicked}"
        )));
    }
    yuna.capture_changed(&whole_window, &home.png)
        .map_err(failure)?;
    let songs = yuna.still_capture(&whole_window).map_err(failure)?;
    yuna.keep(&songs.png, "opium-songs.png").map_err(failure)?;
    let songs_page = Image::from_png(&songs.png)
        .and_then(|image| shows_page(&image, SONGS_ROW, HOME_ROW))
        .map_err(|what| failure(format!("the click did not show its Songs page: {what}")))?;

    let quiet = yuna.wait_until_quiet(QUIET, PATIENCE).map_err(failure)?;
    let rest = yuna
        .measure_rest()
        .map_err(|activity| failure(format!("Yuna did not rest: {activity}")))?;
    if !yuna.program_runs(seat, &program).map_err(failure)? {
        return Err(failure("it left during the rest window".to_owned()));
    }

    let output = yuna.program_log(seat, &program).map_err(failure)?;
    let closing = Instant::now();
    let closed_seat = yuna.close_seat(seat).map_err(failure)?;
    let closed = closing.elapsed();
    // Yuna answers once the seat's processes have ended, before it may have reaped them.
    if closed_seat["ended"] != json!([pid])
        || closed_seat["left_running"] != json!([])
        || state_of(pid).is_some_and(|state| state != 'Z')
    {
        return Err(failure(format!(
            "it still runs once its seat closed: {closed_seat}"
        )));
    }
    yuna.profile_kept_in_its_home().map_err(failure)?;
    yuna.stop()?;
    Ok(format!(
        "Opium through MCP: its window shown {:.1} s after spawn; its Home page captured \
         ({home_page}); Songs clicked, its Songs page captured ({songs_page}); quiet after {} \
         frames drawn, {} commits; then {rest}; its seat closed in {} ms\nOpium's output:\n{output}",
        shown.as_secs_f64(),
        quiet.frames_drawn,
        quiet.commits,
        closed.as_millis(),
    ))
}

/// Requires that `image`, a capture of Opium's window, shows one of its pages in its Stock
/// theme: the page's colour over at least half the window, the sidebar's over the empty part of
/// its column, `active`'s row drawn as the active one, its icon in the accent, `inactive`'s
/// not, and text in the page's heading. A blank window, another program's, or another page of
/// Opium's fails. Says what it counted.
fn shows_page(image: &Image, active: Area, inactive: Area) -> Result<String, String> {
    if (image.width, image.height) != WINDOW {
        return Err(format!(
            "a capture of {} × {} pixels, not of the {WINDOW:?} window",
            image.width, image.height
        ));
    }
    let page = image.count((0, 0, image.width, image.height), PAGE);
    if page * 2 < image.pixels.len() {
        return Err(format!(
            "{page} pixels of the page's colour, less than half the window"
        ));
    }
    let sidebar = image.count(EMPTY_SIDEBAR, SIDEBAR);
    if sidebar != pixels_in(EMPTY_SIDEBAR) {
        return Err(format!(
            "{sidebar} of the {} pixels of the sidebar's empty part are its colour",
            pixels_in(EMPTY_SIDEBAR)
        ));
    }
    let (row, icon) = (image.count(active, ACTIVE_ROW), image.count(active, ACCENT));
    if row * 2 < pixels_in(active) || icon < ICON_PIXELS {
        return Err(format!(
            "the row at {active:?} is not the active one: {row} pixels of an active row, {icon} \
             of the accent"
        ));
    }
    let (other, other_icon) = (
        image.count(inactive, SIDEBAR),
        image.count(inactive, ACCENT),
    );
    if other * 2 < pixels_in(inactive) || other_icon > 0 {
        return Err(format!(
            "the row at {inactive:?} is active too: {other} pixels of the sidebar, {other_icon} \
             of the accent"
        ));
    }
    let heading = image.count(HEADING, TEXT);
    if heading < HEADING_PIXELS {
        return Err(format!("{heading} pixels of text in the page's heading"));
    }
    Ok(format!(
        "{page} pixels of the page's colour, the active row's {row} and its icon's {icon}, the \
         heading's text {heading}"
    ))
}

/// The state of process `pid`, as `/proc` gives it: `Z` for one that ended and was not yet
/// reaped; none once it is gone.
fn state_of(pid: i64) -> Option<char> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat[stat.rfind(')')? + 1..].trim_start().chars().next()
}

/// How many pixels `area` holds.
fn pixels_in((_, _, width, height): Area) -> usize {
    (width * height) as usize
}

/// The check of a capture takes a drawing of what it looks for, Opium's Home page reduced to the
/// parts it reads, and refuses a blank window whatever its colour, a window of another size,
/// that drawing without each of its parts, and its Songs page: the story cannot pass on a
/// window that shows nothing, or something else. It runs without Opium, at every check.
#[test]
fn a_blank_or_another_window_is_not_opiums_page() {
    let home = || {
        let mut image = Image::filled(WINDOW, PAGE);
        image.fill((0, 0, 75, WINDOW.1), SIDEBAR);
        image.fill(HOME_ROW, ACTIVE_ROW);
        let (left, top, ..) = HOME_ROW;
        image.fill((left + 20, top + 10, ICON_PIXELS as u32, 1), ACCENT);
        let (left, top, ..) = HEADING;
        image.fill((left + 8, top + 10, 60, 2), TEXT);
        image
    };
    assert!(shows_page(&home(), HOME_ROW, SONGS_ROW).is_ok());
    assert!(
        shows_page(&home(), SONGS_ROW, HOME_ROW).is_err(),
        "the Home page taken for the Songs page"
    );
    for colour in [PAGE, SIDEBAR, ACTIVE_ROW, [0; 3], [255; 3]] {
        let blank = Image::filled(WINDOW, colour);
        assert!(
            shows_page(&blank, HOME_ROW, SONGS_ROW).is_err(),
            "a blank window of {colour:?}"
        );
    }
    let (width, height) = WINDOW;
    let larger = Image::filled((width + 1, height), PAGE);
    assert!(shows_page(&larger, HOME_ROW, SONGS_ROW).is_err());
    let without: [(Area, [u8; 3]); 5] = [
        ((300, 0, 600, WINDOW.1), SIDEBAR),
        (EMPTY_SIDEBAR, PAGE),
        (HOME_ROW, SIDEBAR),
        (SONGS_ROW, ACCENT),
        (HEADING, PAGE),
    ];
    for (area, colour) in without {
        let mut image = home();
        image.fill(area, colour);
        assert!(
            shows_page(&image, HOME_ROW, SONGS_ROW).is_err(),
            "{area:?} filled with {colour:?}"
        );
    }
}

/// Requires that process `pid` sees no network interface but its loopback: it runs in a
/// network namespace of its own.
fn no_network_but_loopback(pid: i64) -> Result<(), String> {
    let path = format!("/proc/{pid}/net/dev");
    let devices = fs::read_to_string(&path).map_err(|error| format!("{path}: {error}"))?;
    // Two lines of headings, then one line per interface, its name before a colon.
    let interfaces: Vec<&str> = devices
        .lines()
        .skip(2)
        .filter_map(|line| Some(line.split_once(':')?.0.trim()))
        .collect();
    if interfaces != ["lo"] {
        return Err(format!(
            "it sees the network interfaces {interfaces:?}, not its loopback alone"
        ));
    }
    Ok(())
}

/// A folder short enough that the sockets Yuna makes under it, an accessibility bus's the
/// longest, fit a Unix socket's 107 bytes, as in `tests/mcp.rs`.
fn short_temp_dir() -> PathBuf {
    let temp = std::env::temp_dir();
    if temp.as_os_str().len() <= 16 {
        temp
    } else {
        PathBuf::from("/tmp")
    }
}

/// A program found in the test's own `PATH`, by its full path.
fn in_path(program: &str) -> Result<PathBuf, String> {
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|folder| folder.join(program))
        .find(|path| path.is_file())
        .ok_or_else(|| format!("{program} is not in PATH"))
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

/// A `yuna mcp` process, its output 1920 × 1080 at 60 Hz, in a folder of its own: its runtime
/// folder, its log, the PNGs of the captures the test kept, and Opium's profile.
struct Yuna {
    child: Child,
    folder: PathBuf,
    log: PathBuf,
    /// Its standard input, which takes the test's requests.
    input: RefCell<ChildStdin>,
    /// Its replies, read from its standard output on a thread.
    replies: mpsc::Receiver<Value>,
    next_id: Cell<u64>,
    /// Whether a failure was reported: the folder, with its logs and captures, is then kept.
    failed: Cell<bool>,
}

/// A program the agent started through `spawn`: its id and its log.
struct Program {
    id: u64,
    log: PathBuf,
}

/// A capture's PNG, and its id, by which `pointer` takes its pixels.
struct Captured {
    png: Vec<u8>,
    id: u64,
}

impl Yuna {
    /// Starts `yuna mcp` at the `debug` level, with an environment of its own: the shell's Mesa,
    /// Xwayland and `dbus-daemon`, and a throwaway home.
    fn start() -> Yuna {
        let folder = short_temp_dir().join(format!("yuna-opium-{}", std::process::id()));
        // Wayland asks that the runtime folder be the user's alone.
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder.join("runtime"))
            .expect("a runtime folder");
        let log = folder.join("yuna.log");
        let (width, height) = OUTPUT;
        let mut command = Command::new(env!("CARGO_BIN_EXE_yuna"));
        command
            .args(["mcp", "--output", &format!("{width}x{height}@60,1")])
            .env_clear()
            .env("PATH", shell_variable("YUNA_XWAYLAND_PATH"))
            .env("YUNA_DBUS_DAEMON", shell_variable("YUNA_DBUS_DAEMON"))
            .env("LD_LIBRARY_PATH", shell_variable("LD_LIBRARY_PATH"))
            .env(
                "__EGL_VENDOR_LIBRARY_FILENAMES",
                shell_variable("__EGL_VENDOR_LIBRARY_FILENAMES"),
            )
            .env("XKB_CONFIG_ROOT", shell_variable("XKB_CONFIG_ROOT"))
            .env("HOME", folder.join("yuna-home"))
            .env("XDG_RUNTIME_DIR", folder.join("runtime"))
            .env("YUNA_LOG", "debug")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(File::create(&log).expect("Yuna's log"));
        dies_with_test(&mut command);
        let mut child = command.spawn().expect("the yuna binary runs");
        let stdout = child.stdout.take().expect("its standard output");
        let input = child.stdin.take().expect("its standard input");
        let (sender, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                    return;
                };
                if sender.send(reply).is_err() {
                    return;
                }
            }
        });
        Yuna {
            child,
            folder,
            log,
            input: RefCell::new(input),
            replies,
            next_id: Cell::new(1),
            failed: Cell::new(false),
        }
    }

    /// Calls an MCP tool and gives its result, or what it refused.
    fn call(&self, tool: &str, arguments: Value) -> Result<Value, String> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
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
        writeln!(self.input.borrow_mut(), "{request}")
            .map_err(|error| format!("{tool}: {error}"))?;
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let reply = self
                .replies
                .recv_timeout(left)
                .map_err(|_| format!("no reply to {tool} in {} s", PATIENCE.as_secs()))?;
            if reply["id"] == json!(id) {
                let result = reply["result"].clone();
                if result["isError"] == json!(false) {
                    return Ok(result);
                }
                return Err(format!("{tool} {arguments} failed: {reply}"));
            }
        }
    }

    /// Creates an agent's seat with `arguments`, and gives its handle.
    fn create_seat(&self, arguments: Value) -> Result<u64, String> {
        let created = self.call("seat", arguments)?;
        created["structuredContent"]["seat"]
            .as_u64()
            .ok_or_else(|| format!("no handle in {created}"))
    }

    /// Starts Opium on `seat`, in a network namespace of its own, on a throwaway profile in
    /// this Yuna's folder.
    fn start_opium(&self, seat: u64) -> Result<Program, String> {
        let unshare = in_path("unshare")?;
        let opium = opium();
        let command = [
            unshare.to_str().ok_or("a path")?,
            "--user",
            "--map-current-user",
            "--net",
            opium.to_str().ok_or("a path")?,
        ];
        let mut env: BTreeMap<String, String> = application_environment(&self.home())
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_string_lossy().into_owned()))
            .collect();
        // A file that does not exist: Opium then loads no Widevine module, where it would
        // otherwise look in the browsers' folders, Chrome's on CI's runners among them.
        env.insert("WIDEVINE_CDM".to_owned(), "/nonexistent".to_owned());
        let started = self.call(
            "spawn",
            json!({"action": "start", "seat": seat, "command": command, "env": env}),
        )?;
        let program = &started["structuredContent"]["program"];
        Ok(Program {
            id: program["id"].as_u64().ok_or("no program id")?,
            log: PathBuf::from(program["log"].as_str().ok_or("no log")?),
        })
    }

    /// Waits through MCP for Opium's window on `seat`, placed, and gives it.
    fn opium_window(&self, seat: u64) -> Result<Value, String> {
        // `windows` waits 50 s at most.
        let wait_ms = PATIENCE.as_millis().min(50_000);
        let found = self.call(
            "windows",
            json!({"seat": seat, "app_id": "opium", "wait_ms": wait_ms}),
        )?;
        let window = found["structuredContent"]["windows"][0].clone();
        if window.is_null() {
            return Err(format!("no window of opium in {wait_ms} ms"));
        }
        if window["title"] != json!("Opium") || window["rect"].is_null() {
            return Err(format!("a window not titled Opium, or not shown: {window}"));
        }
        Ok(window)
    }

    /// Captures through MCP with `arguments`, and gives its PNG and its id.
    fn capture(&self, arguments: &Value) -> Result<Captured, String> {
        let captured = self.call("capture", arguments.clone())?;
        let data = captured["content"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|content| content["type"] == json!("image"))
            .and_then(|image| image["data"].as_str())
            .ok_or_else(|| format!("no image in {captured}"))?;
        Ok(Captured {
            png: decode_base64(data)?,
            id: captured["structuredContent"]["capture"]
                .as_u64()
                .ok_or_else(|| format!("no capture id in {captured}"))?,
        })
    }

    /// Captures with `arguments` again and again until the image differs from `before`, as an
    /// agent waits for its application to draw what its input changed.
    fn capture_changed(&self, arguments: &Value, before: &[u8]) -> Result<(), String> {
        let deadline = Instant::now() + PATIENCE;
        while self.capture(arguments)?.png == before {
            if Instant::now() > deadline {
                return Err(format!(
                    "{arguments} did not change in {} s",
                    PATIENCE.as_secs()
                ));
            }
            std::thread::sleep(POLL);
        }
        Ok(())
    }

    /// Captures with `arguments` every [`POLL`] until the image has stayed the same for
    /// [`STILL`], and gives the last capture: sampled through the whole second, so that an
    /// animation is not taken for still.
    fn still_capture(&self, arguments: &Value) -> Result<Captured, String> {
        let deadline = Instant::now() + PATIENCE;
        let mut last = self.capture(arguments)?;
        let mut since = Instant::now();
        loop {
            std::thread::sleep(POLL);
            let now = self.capture(arguments)?;
            if now.png != last.png {
                since = Instant::now();
            } else if since.elapsed() >= STILL {
                return Ok(now);
            }
            last = now;
            if Instant::now() > deadline {
                return Err(format!("{arguments} never stood still"));
            }
        }
    }

    /// Writes `png` to this Yuna's folder as `name`, which a failure keeps.
    fn keep(&self, png: &[u8], name: &str) -> Result<(), String> {
        fs::write(self.folder.join(name), png).map_err(|error| format!("{name}: {error}"))
    }

    /// Whether `program` of `seat` still runs, as `spawn list` says.
    fn program_runs(&self, seat: u64, program: &Program) -> Result<bool, String> {
        let listed = self.call("spawn", json!({"action": "list", "seat": seat}))?;
        Ok(listed["structuredContent"]["programs"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|listed| listed["id"] == json!(program.id) && listed["state"] == json!("running")))
    }

    /// `program`'s output so far, as `spawn log` gives it.
    fn program_log(&self, seat: u64, program: &Program) -> Result<String, String> {
        let read = self.call(
            "spawn",
            json!({"action": "log", "seat": seat, "program": program.id}),
        )?;
        read["structuredContent"]["text"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("no text in {read}"))
    }

    /// Closes `seat`, which ends its programs: Yuna answers once they have ended, and says
    /// which it ended and which still run.
    fn close_seat(&self, seat: u64) -> Result<Value, String> {
        let closed = self.call("seat", json!({"action": "close", "seat": seat}))?;
        Ok(closed["structuredContent"].clone())
    }

    /// Opium's home: its settings, caches and data, in this Yuna's folder.
    fn home(&self) -> PathBuf {
        self.folder.join("home")
    }

    /// Requires that Opium kept its profile in its home, [`Yuna::home`]: its lock, which it
    /// takes before anything else, is in the settings' folder there, and nothing of Opium's is
    /// in Yuna's own home, which a program gets when the agent gives it none.
    fn profile_kept_in_its_home(&self) -> Result<(), String> {
        let lock = self.home().join("config/opium/instance.lock");
        if !lock.is_file() {
            return Err(format!("no {}", lock.display()));
        }
        let yunas_home = self.folder.join("yuna-home");
        for folder in [".config/opium", ".cache/opium", ".local/share/opium"] {
            if yunas_home.join(folder).exists() {
                return Err(format!("it wrote {} in Yuna's own home", folder));
            }
        }
        Ok(())
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

    /// Waits until Yuna has had no round for `quiet`, at most `patience`, and gives its counters
    /// then.
    fn wait_until_quiet(&self, quiet: Duration, patience: Duration) -> Result<Counters, String> {
        let deadline = Instant::now() + patience;
        let mut last = self.counters();
        let mut since = Instant::now();
        loop {
            std::thread::sleep(POLL);
            let now = self.counters();
            if now != last {
                last = now;
                since = Instant::now();
            } else if since.elapsed() >= quiet {
                return Ok(now);
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "never quiet for {} s in {} s: {last:?}",
                    quiet.as_secs(),
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

    /// What a failure shows: the ends of Yuna's log and of the program's, `program_log`. The
    /// folder that holds both is then kept.
    fn diagnosis(&self, program_log: &Path) -> String {
        self.failed.set(true);
        let log = fs::read_to_string(program_log).unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        format!(
            "{}\nthe program's last lines ({}):\n{}",
            self.log_tail(),
            program_log.display(),
            lines[lines.len().saturating_sub(15)..].join("\n")
        )
    }
}

impl Drop for Yuna {
    /// Kills Yuna if it still runs, which ends its programs. Its folder is kept when the run
    /// failed, for its logs and captures.
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

/// An image of R, G, B pixels, row by row.
struct Image {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 3]>,
}

impl Image {
    /// A capture's PNG, as Yuna encodes it: 8-bit RGB.
    fn from_png(png: &[u8]) -> Result<Image, String> {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .map_err(|error| format!("a capture's PNG: {error}"))?;
        let mut bytes = vec![0; reader.output_buffer_size().ok_or("a PNG too large")?];
        let info = reader
            .next_frame(&mut bytes)
            .map_err(|error| format!("a capture's pixels: {error}"))?;
        if info.color_type != png::ColorType::Rgb || info.bit_depth != png::BitDepth::Eight {
            return Err(format!(
                "a capture's PNG is {:?} of {:?}, not 8-bit RGB",
                info.color_type, info.bit_depth
            ));
        }
        bytes.truncate(info.buffer_size());
        Ok(Image {
            width: info.width,
            height: info.height,
            pixels: bytes.as_chunks::<3>().0.to_vec(),
        })
    }

    /// An image of `width` × `height` pixels, all `colour`.
    fn filled((width, height): (u32, u32), colour: [u8; 3]) -> Image {
        Image {
            width,
            height,
            pixels: vec![colour; (width * height) as usize],
        }
    }

    /// Paints `area`, which the image holds whole, in `colour`.
    fn fill(&mut self, (left, top, width, height): Area, colour: [u8; 3]) {
        for y in top..top + height {
            for x in left..left + width {
                self.pixels[(y * self.width + x) as usize] = colour;
            }
        }
    }

    /// How many pixels of `area`, which the image holds whole, are `colour`, each channel
    /// within [`TOLERANCE`].
    fn count(&self, (left, top, width, height): Area, colour: [u8; 3]) -> usize {
        (top..top + height)
            .flat_map(|y| (left..left + width).map(move |x| (x, y)))
            .map(|(x, y)| self.pixels[(y * self.width + x) as usize])
            .filter(|pixel| (0..3).all(|c| pixel[c].abs_diff(colour[c]) <= TOLERANCE))
            .count()
    }
}

/// The bytes of base64 `text`, as RFC 4648 writes them, padded.
fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Ok(c - b'A'),
        b'a'..=b'z' => Ok(c - b'a' + 26),
        b'0'..=b'9' => Ok(c - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(format!("not base64: {c}")),
    };
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    for chunk in text.as_bytes().chunks(4) {
        let digits = chunk
            .iter()
            .take_while(|&&c| c != b'=')
            .map(|&c| value(c))
            .collect::<Result<Vec<u8>, String>>()?;
        if digits.len() < 2 {
            return Err("base64 cut short".to_owned());
        }
        let n = digits.iter().fold(0u32, |n, &d| n << 6 | u32::from(d)) << (6 * (4 - digits.len()));
        bytes.extend_from_slice(&n.to_be_bytes()[1..digits.len()]);
    }
    Ok(bytes)
}
