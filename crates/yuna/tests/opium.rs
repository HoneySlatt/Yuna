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
//! namespace is made by util-linux's `unshare`, which the development shell names in
//! `YUNA_UNSHARE`: it is the program `spawn` starts, and becomes Opium. Opium's profile, its
//! settings, caches and data, is a folder of the
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
#[path = "common/base64.rs"]
mod base64;
#[path = "common/capture_image.rs"]
mod capture_image;
#[path = "common/dies_with_test.rs"]
mod dies_with_test;
#[path = "common/folders.rs"]
mod folders;
#[path = "common/mcp_client.rs"]
mod mcp_client;
#[path = "common/rest.rs"]
mod rest;
#[path = "common/yuna_run.rs"]
mod yuna_run;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, File};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use application_environment::{application_environment, shell_variable};
use base64::decode_base64;
use capture_image::Image;
use dies_with_test::dies_with_test;
use folders::fresh_folder;
use mcp_client::McpClient;
use rest::{Activity, Counters};
use serde_json::{Value, json};
use yuna_run::POLL;

/// How long Yuna must go without a round before Opium counts as idle.
const QUIET: Duration = Duration::from_secs(2);
/// The longest a step may take: Opium's first frames on lavapipe, in a debug build of Yuna on
/// CI's slow runners.
const PATIENCE: Duration = Duration::from_secs(60);
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
    // Yuna answers once the seat's processes have ended, before it may have reaped them: a
    // zombie (`Z`), or a process being reaped (`X`), which `/proc` shows for a moment.
    if closed_seat["ended"] != json!([pid])
        || closed_seat["left_running"] != json!([])
        || state_of(pid).is_some_and(|state| !matches!(state, 'Z' | 'X'))
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
    let (row_left, row_top, ..) = HOME_ROW;
    let icon = (row_left + 20, row_top + 10, ICON_PIXELS as u32, 1);
    let without: [(Area, [u8; 3]); 6] = [
        ((300, 0, 600, WINDOW.1), SIDEBAR),
        (EMPTY_SIDEBAR, PAGE),
        (HOME_ROW, SIDEBAR),
        // The active row's background, without its icon in the accent.
        (icon, ACTIVE_ROW),
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

/// The development shell names the `unshare` that makes Opium's network namespace, util-linux's
/// at the revision the shell pins, so that the test never takes the host's: it runs without Opium,
/// at every check.
#[test]
fn the_shell_names_the_unshare_that_makes_the_network_namespace() {
    let unshare = PathBuf::from(shell_variable("YUNA_UNSHARE"));
    assert!(
        unshare.starts_with("/nix/store") && unshare.is_file(),
        "{}",
        unshare.display()
    );
}

/// A folder a failed run kept is removed before the next run uses its name, which a process id
/// that comes round again gives: Opium would otherwise start on that run's profile. It runs
/// without Opium, at every check.
#[test]
fn a_folder_a_failed_run_kept_is_not_reused() {
    let kept = fresh_folder("opium-kept");
    let old_profile = kept.join("home/config/opium/instance.lock");
    fs::create_dir_all(old_profile.parent().expect("a folder")).expect("an old profile");
    fs::write(&old_profile, "").expect("an old lock");
    let folder = fresh_folder("opium-kept");
    // Kept until the check: dropping it removes the folder, which would hide one reused.
    let _yuna = Yuna::start_in(folder);
    assert!(!old_profile.exists(), "the old profile is still there");
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

/// A `yuna mcp` process, its output 1920 × 1080 at 60 Hz, in a folder of its own: its runtime
/// folder, its log, the PNGs of the captures the test kept, and Opium's profile.
struct Yuna {
    child: Child,
    folder: PathBuf,
    log: PathBuf,
    mcp: McpClient,
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
    /// Starts `yuna mcp` in this process's folder.
    fn start() -> Yuna {
        Yuna::start_in(fresh_folder("opium"))
    }

    /// Starts `yuna mcp` at the `debug` level in `folder`, a [`fresh_folder`], with an
    /// environment of its own: the shell's Mesa, Xwayland and `dbus-daemon`, and a throwaway
    /// home.
    fn start_in(folder: PathBuf) -> Yuna {
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
        let mcp = McpClient::connect(&mut child);
        Yuna {
            child,
            folder,
            log,
            mcp,
            failed: Cell::new(false),
        }
    }

    /// Calls an MCP tool and gives its result, or what it refused.
    fn call(&self, tool: &str, arguments: Value) -> Result<Value, String> {
        self.mcp.call(tool, arguments, PATIENCE)
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
        let unshare = shell_variable("YUNA_UNSHARE");
        let opium = opium();
        let command = [
            unshare.as_str(),
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

    /// Waits until Yuna has had no round for `quiet`, at most `patience`, and gives its counters
    /// then.
    fn wait_until_quiet(&self, quiet: Duration, patience: Duration) -> Result<Counters, String> {
        rest::wait_until_quiet(&self.log, quiet, patience)
    }

    /// Requires [`rest::REST`] with no context switch in any of Yuna's threads and no round: no
    /// frame, frame callback, configure, commit or wake-up. Says what it measured.
    fn measure_rest(&self) -> Result<String, Activity> {
        rest::measure_rest(self.child.id(), &self.log)
    }

    /// Stops Yuna with SIGTERM, as an agent does, and requires that it ends well.
    fn stop(mut self) -> Result<(), String> {
        yuna_run::stop_on_sigterm(&mut self.child, &self.log).inspect_err(|_| self.failed.set(true))
    }

    /// What a failure shows: the ends of Yuna's log and of the program's, `program_log`. The
    /// folder that holds both is then kept.
    fn diagnosis(&self, program_log: &Path) -> String {
        self.failed.set(true);
        yuna_run::diagnosis(&self.log, program_log, "program")
    }
}

impl Drop for Yuna {
    /// Kills Yuna if it still runs, which ends its programs. Its folder is kept when the run
    /// failed, for its logs and captures.
    fn drop(&mut self) {
        yuna_run::end_run(&mut self.child, &self.folder, self.failed.get());
    }
}

impl Image {
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
