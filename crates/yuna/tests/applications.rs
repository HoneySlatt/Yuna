//! Real applications in `yuna --headless` and in `yuna mcp`, and Yuna's rest checked on the
//! program, as agents run it, not in the test's process
//! ([ADR-0025](../../../docs/adr/0025-applications-in-ci.md)).
//!
//! Each application runs in a `yuna` of its own, all at once. Yuna logs at `debug`: the line
//! `window opened` when a window maps, `frame drawn` for each frame, and `counters` after each
//! round of its loop that woke. Rest is read from those lines and from `/proc`: once the
//! application has shown its window and gone quiet, 10 s with no context switch in any of Yuna's
//! threads, and no frame, frame callback, configure, commit or wake-up.
//!
//! Applications that animate without end draw a number of frames and leave; Yuna must then rest.
//! `vkcube` left to run proves that a client that keeps drawing fails the check. Through
//! `yuna mcp`, as an agent drives it, `vkcube` paused by its space key rests with its window open
//! ([ADR-0029](../../../docs/adr/0029-the-rest-of-animated-applications.md)).
//!
//! Through `yuna mcp` too, mpv shows a picture it makes itself, an SDR one on an HDR output and a
//! PQ one on an SDR output, until it has gone quiet. The test reads the colours mpv's surface
//! declared through `color-management-v1`: values that hold only if mpv read the description Yuna
//! prefers for it. mpv then quits by its `q` key, and Yuna must rest
//! ([ADR-0050](../../../docs/adr/0050-mpv-in-the-applications-tests.md)).
//!
//! X11 programs run through `yuna mcp` on agents' seats, whose programs get the seat's
//! `DISPLAY`; Yuna finds Xwayland on the `PATH` the test gives it. Qt's `qml` on X11, at
//! XWayland's scale 1.5, draws at a device pixel ratio of 1.5, and its capture is exactly what
//! X11 holds of its window and what Qt draws on Wayland at 1.5; at 1, it is its own area average.
//! Wine's notepad, from a fresh prefix at 144 dots per inch, draws its menu bar and scroll bar
//! 1.5 times larger than at 96, and its capture is exactly what X11 holds of it. Each leaves, and
//! Yuna rests once Xwayland has ended
//! ([ADR-0068](../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)).
//!
//! SlimeEngine's examples run in ignored tests, which its weekly job runs: `window` draws its
//! frames and leaves, `editor` rests with its window open, and an agent types and clicks in
//! `interface` and checks it by capture, M1.5's done-when
//! ([ADR-0043](../../../docs/adr/0043-text-input-and-the-rest-of-slimeengines-examples.md)).
//!
//! What makes an application draw again is pinned, so that only Yuna's behaviour decides the
//! result. Yuna and each application get an environment of their own, nothing of the caller's:
//! no `DISPLAY` but an agent's seat's, and no D-Bus, its start through X11 included, which Yuna
//! disables for its programs, so that they reach neither the user's desktop nor an
//! accessibility bus; a `PATH` of the store alone; throwaway home and XDG folders, the system's
//! XDG folders included, so that no settings of the machine or the user are read; DejaVu alone,
//! through a fontconfig file that includes none of the machine's; Mesa on the CPU, lavapipe
//! alone for Vulkan, without the machine's implicit layers; cursors that never blink; GTK's
//! animations off.
//! What still reads the machine, measured with strace on NixOS: Mesa reads `/etc/drirc` where a
//! machine has one, and lists `/dev/dri` without opening a device; Vulkan's loader reads the
//! machine's list of layers and loads none; the fontconfig of Alacritty and Qt looks for two
//! NixOS files in `/etc/fonts`, `local.conf` and `70-no-bitmaps-except-emoji.conf`; lavapipe, in
//! mpv, opens `/dev/udmabuf`; mpv reads `/etc/localtime`, and looks for each of its libraries in
//! the machine's `/run/opengl-driver/lib` first, where the machine measured has none of them.
//! Qt on X11 reads `/etc/localtime` and the GPU's description in `/sys`, without opening a
//! device. Wine reads `/etc/localtime`, `/etc/passwd`, `/etc/nsswitch.conf`, `/etc/hosts`,
//! `/etc/host.conf`, `/etc/resolv.conf`, `/proc/net/dev`, the processor's description in `/sys`
//! and the Vulkan loader's layer manifests in `/run/opengl-driver`, loading no layer; it asks
//! the user's name through glibc's cache, `nscd`, on its local socket; a new prefix links the
//! machine's serial ports as COM ports without opening them. Its drivers that open input and USB
//! devices are off, and its print server is a socket that does not exist, so that it opens no
//! network connection (checked with `strace -e trace=connect`). Each wineserver leaves a folder
//! in `/tmp/.wine-<uid>`, Wine's own place, which the test removes once it has ended; a failed
//! run leaves it there, with its empty lock.
//!
//! Every program comes from the development shell (`flake.nix`). Each process the test starts is
//! killed if the test's thread ends first, so that a test killed or out of time leaves nothing.

#[path = "common/application_environment.rs"]
mod application_environment;
#[path = "common/dies_with_test.rs"]
mod dies_with_test;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, DirBuilder, File};
use std::io::{BufRead, BufReader, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use application_environment::{application_environment, shell_variable};
use dies_with_test::dies_with_test;
use serde_json::{Value, json};
use smithay::reexports::rustix::process::{Pid, Signal, kill_process};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder};
use x11rb::rust_connection::{DefaultStream, RustConnection};

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

    let never_quiet = yuna.wait_until_quiet(QUIET, Duration::from_secs(5));
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

/// `vkcube`, paused by its space key through MCP, rests with its window open: Yuna has nothing
/// to do while it shows a cube that does not turn
/// ([ADR-0029](../../../docs/adr/0029-the-rest-of-animated-applications.md)).
#[test]
fn vkcube_paused_by_its_space_key_rests_with_its_window_open() {
    let report = pause_vkcube_and_rest().unwrap_or_else(|failure| panic!("{failure}"));
    eprintln!("{report}");
}

fn pause_vkcube_and_rest() -> Result<String, String> {
    let mut yuna = Yuna::start_mcp();
    let seat = yuna.create_seat()?;
    let cube = yuna.start_program(seat, &["vkcube", "--wsi", "wayland"], &[])?;
    let failure = |what: String| format!("vkcube: {what}\n{}", yuna.diagnosis(&cube.log));
    yuna.agent_window(seat).map_err(failure)?;
    yuna.wait_for_frames().map_err(failure)?;
    if yuna.wait_until_quiet(QUIET, QUIET * 2).is_ok() {
        return Err(failure("it went quiet before its pause".to_owned()));
    }
    yuna.call(
        "keyboard",
        json!({"action": "tap", "seat": seat, "keys": "space"}),
    )
    .map_err(failure)?;
    let paused = yuna.wait_until_quiet(QUIET, PATIENCE).map_err(failure)?;
    let rest = yuna
        .measure_rest()
        .map_err(|activity| failure(format!("Yuna did not rest: {activity}")))?;
    if !yuna.program_runs(seat, &cube).map_err(failure)? {
        return Err(failure("it left during the rest window".to_owned()));
    }
    if !yuna.window_shown(seat).map_err(failure)? {
        return Err(failure("its window was not shown at rest".to_owned()));
    }
    yuna.stop()?;
    Ok(format!(
        "vkcube, paused by its space key: quiet after {} frames drawn, {} commits; then {rest}",
        paused.frames_drawn, paused.commits
    ))
}

/// mpv shows an SDR picture on an HDR output, which announces the PG32UCDM's colours. Its
/// target is the description Yuna prefers for its surface: it declares its picture in PQ over
/// BT.2020, the output's, aimed at the monitor's own volume, the panel's primaries and 0.0002
/// to 1015 cd/m², which only that description gave it. Its own luminances are PQ's, with
/// libplacebo's reference white of 203 cd/m²; it leaves `max_fall` unset, as it reads a
/// display's as a full frame's maximum, not an average
/// ([ADR-0050](../../../docs/adr/0050-mpv-in-the-applications-tests.md)).
#[test]
fn mpv_aims_an_sdr_picture_at_the_hdr_outputs_monitor() {
    let declared = json!([{
        "role": "window",
        "transfer_function": "st2084_pq",
        "primaries": "bt2020",
        "luminances": {"min": 0.0, "max": 10_000, "reference": 203},
        // The PG32UCDM's own values, `colour/description.rs`, written again here on purpose:
        // an independent check of what Yuna announced.
        "target_primaries": {
            "red": [0.6835, 0.3046],
            "green": [0.2441, 0.7089],
            "blue": [0.1435, 0.0556],
            "white": [0.3134, 0.3291],
        },
        "target_luminance": {"min": 0.0002, "max": 1015},
        "max_cll": 1015,
        "max_fall": null,
        "intent": "perceptual",
    }]);
    let report = mpv_declares(true, &[], &declared).unwrap_or_else(|failure| panic!("{failure}"));
    eprintln!("{report}");
}

/// mpv shows a PQ picture on an SDR output, which announces gamma 2.2 over sRGB, 0.2 to 80
/// cd/m² with a reference white of 80. Its target is that description: it tone-maps the
/// picture and declares gamma 2.2 over sRGB, not the picture's PQ. It scales the luminances so
/// that the reference white is libplacebo's 203 cd/m² and keeps the output's minimum: 0.2 to
/// 203, no headroom above white, as the output announced. Gamma 2.2 carries no HDR metadata
/// ([ADR-0050](../../../docs/adr/0050-mpv-in-the-applications-tests.md)).
#[test]
fn mpv_tone_maps_a_pq_picture_to_the_sdr_outputs_description() {
    let declared = json!([{
        "role": "window",
        "transfer_function": "gamma22",
        "primaries": "srgb",
        "luminances": {"min": 0.2, "max": 203, "reference": 203},
        "target_primaries": {
            "red": [0.64, 0.33],
            "green": [0.3, 0.6],
            "blue": [0.15, 0.06],
            "white": [0.3127, 0.329],
        },
        "target_luminance": {"min": 0.2, "max": 203},
        "max_cll": null,
        "max_fall": null,
        "intent": "perceptual",
    }]);
    let pq = ["--vf=format:primaries=bt.2020:gamma=pq"];
    let report = mpv_declares(false, &pq, &declared).unwrap_or_else(|failure| panic!("{failure}"));
    eprintln!("{report}");
}

/// The picture mpv makes itself, so that nothing is fetched: one second of FFmpeg's test
/// pattern, whose last frame `--keep-open` holds.
const MPV_PICTURE: &str = "av://lavfi:testsrc2=size=640x360:rate=24:duration=1";

/// Runs mpv through MCP on a seat of its own, its output HDR or SDR, with `options` before its
/// picture, and requires that its window's surfaces declared the colours `declared`, as
/// `windows` gives them with `colour`, read once mpv and Yuna have gone quiet: mpv sets its
/// colours before its first frame, so a pause in its playback would read the same. mpv then quits by its
/// `q` key, and Yuna must rest.
fn mpv_declares(hdr: bool, options: &[&str], declared: &Value) -> Result<String, String> {
    let mut yuna = Yuna::start_mcp();
    let seat = yuna.create_seat_with(json!({"action": "create", "hdr": hdr}))?;
    // Vulkan on lavapipe, which mpv refuses as a software device without `--gpu-sw`; no
    // configuration file and no sound; the context pinned, so that a new mpv does not pick
    // another unseen. Its `select` script would list the machine's sound cards for its menu,
    // opening their ALSA control devices: off, as mpv's own `libmpv` profile sets it. Its
    // Wayland module's verbose lines say what it read and declared.
    let mut command = vec![
        "mpv",
        "--no-config",
        "--vo=gpu-next",
        "--gpu-api=vulkan",
        "--gpu-context=waylandvk",
        "--gpu-sw",
        "--audio=no",
        "--script-opts=select-populate_menu_data=no",
        "--keep-open=yes",
        "--msg-level=vo/gpu-next/wayland=v",
    ];
    command.extend(options);
    command.push(MPV_PICTURE);
    let mpv = yuna.start_program(seat, &command, &[])?;
    let failure = |what: String| format!("mpv: {what}\n{}", yuna.diagnosis(&mpv.log));
    yuna.agent_window(seat).map_err(failure)?;
    yuna.wait_for_frames().map_err(failure)?;
    let shown = yuna.wait_until_quiet(QUIET, PATIENCE).map_err(failure)?;
    if !yuna.program_runs(seat, &mpv).map_err(failure)? {
        return Err(failure("it left before its colours were read".to_owned()));
    }
    let listed = yuna
        .call("windows", json!({"seat": seat, "colour": true}))
        .map_err(failure)?;
    let colours = &listed["structuredContent"]["windows"][0]["colours"];
    if colours != declared {
        let log = fs::read_to_string(&mpv.log).unwrap_or_default();
        let wayland: Vec<&str> = log
            .lines()
            .filter(|line| line.starts_with("[vo/gpu-next/wayland]"))
            .collect();
        return Err(failure(format!(
            "its surface declared\n{colours:#}\nnot\n{declared:#}\nwhat it read and \
             declared:\n{}",
            wayland.join("\n")
        )));
    }

    yuna.call(
        "keyboard",
        json!({"action": "tap", "seat": seat, "keys": "q"}),
    )
    .map_err(failure)?;
    let ended = yuna.wait_for_end(seat, &mpv).map_err(failure)?;
    if ended["state"] != json!("exited") || ended["exit_code"] != json!(0) {
        return Err(failure(format!("it did not end well: {ended}")));
    }
    yuna.wait_until_quiet(QUIET, PATIENCE).map_err(failure)?;
    let rest = yuna
        .measure_rest()
        .map_err(|activity| failure(format!("Yuna did not rest after it quit: {activity}")))?;
    yuna.stop()?;
    let output = if hdr { "an HDR" } else { "an SDR" };
    Ok(format!(
        "mpv on {output} output: quiet after {} frames drawn, {} commits, its colours as \
         expected; it quit, then {rest}",
        shown.frames_drawn, shown.commits
    ))
}

/// The QML scene of the X11 test, written to its folder: on a plain background, a line one
/// physical pixel thick across and one down at scale 1.5, two thirds of a unit, from a corner on
/// even units, which fall on whole pixels; and text. Nothing in it moves, blinks or takes text.
/// It logs its screen's device pixel ratio, which on X11 is its window's, read from `Xft.dpi`;
/// on Wayland the window's own comes later, with `wp_fractional_scale_v1`. Ctrl+Q quits it.
const SHARP_QML: &str = r##"import QtQuick
import QtQuick.Window

Window {
    visible: true
    title: "Yuna, sharp"
    color: "#203040"
    Component.onCompleted: console.log("device pixel ratio", Screen.devicePixelRatio)
    Shortcut {
        sequence: "Ctrl+Q"
        onActivated: Qt.quit()
    }
    Rectangle { x: 20; y: 20; width: 200; height: 2 / 3; color: "white" }
    Rectangle { x: 20; y: 20; width: 2 / 3; height: 100; color: "white" }
    Text {
        x: 30
        y: 30
        text: "Yuna draws X11 at 1.5"
        color: "white"
        font.pixelSize: 16
        renderType: Text.NativeRendering
    }
}
"##;
/// [`SHARP_QML`]'s colours, and where its lines start at scale 1.5, in physical pixels from its
/// window's corner, and how long they are.
const SHARP_BACKGROUND: [u8; 3] = [0x20, 0x30, 0x40];
const SHARP_LINE: [u8; 3] = [0xff, 0xff, 0xff];
const SHARP_CORNER: u32 = 30;
const SHARP_ACROSS: u32 = 300;
const SHARP_DOWN: u32 = 150;
/// The line Qt logs on X11 at XWayland's scale 1.5.
const QML_AT_ONE_AND_A_HALF: &str = "qml: device pixel ratio 1.5";
/// An agent's output at scale 1.5, 1280 × 720 logical units, and one at 1 of the same logical
/// size, so that a window moved from one to the other keeps its size.
const OUTPUT_AT_ONE_AND_A_HALF: (u32, u32, f64) = (1920, 1080, 1.5);
const OUTPUT_AT_ONE: (u32, u32, f64) = (1280, 720, 1.0);

/// Qt's `qml` on X11, on an agent's seat at 1.5, through MCP as an agent runs it: it reads
/// `Xft.dpi` 144 and draws at a device pixel ratio of 1.5, and Yuna shows it pixel for pixel.
/// Its capture at the physical size is what X11 holds of the window, exactly; its one-pixel
/// lines are one physical pixel; and it equals the same scene drawn by Qt on Wayland at 1.5,
/// through `wp_fractional_scale_v1`, on another seat. Moved to an output at 1 of the same logical
/// size, it is drawn through the area filter: within a level of its logical capture at 1.5,
/// which averages the same pixels (ADR-0068). Yuna rests with it open; typed Ctrl+Q, it quits,
/// and Yuna rests again once Xwayland has ended.
#[test]
fn qml_on_x11_at_one_and_a_half_is_drawn_as_qml_on_wayland() {
    let report = qml_on_x11().unwrap_or_else(|failure| panic!("{failure}"));
    eprintln!("{report}");
}

fn qml_on_x11() -> Result<String, String> {
    let mut yuna = Yuna::start_mcp();
    let scene = yuna.folder.join("sharp.qml");
    fs::write(&scene, SHARP_QML).map_err(|error| format!("the scene: {error}"))?;
    let scene = scene.to_str().ok_or("a path")?;
    let reference_seat = yuna.create_seat_with(output_mode(OUTPUT_AT_ONE_AND_A_HALF))?;
    let seat = yuna.create_seat_with(output_mode(OUTPUT_AT_ONE_AND_A_HALF))?;
    let reference =
        yuna.start_program(reference_seat, &["qml", scene], &qt_variables("wayland"))?;
    let qml = yuna.start_program(seat, &["qml", scene], &qt_variables("xcb"))?;
    let failure = |what: String| {
        format!(
            "qml on X11: {what}\n{}\nqml on Wayland's log: {}",
            yuna.diagnosis(&qml.log),
            reference.log.display()
        )
    };
    yuna.agent_window(seat).map_err(failure)?;
    yuna.agent_window(reference_seat).map_err(failure)?;
    let shown = yuna.wait_until_quiet(QUIET, PATIENCE).map_err(failure)?;
    let log = fs::read_to_string(&qml.log).unwrap_or_default();
    if !log.contains(QML_AT_ONE_AND_A_HALF) {
        return Err(failure(format!(
            "Qt did not log \"{QML_AT_ONE_AND_A_HALF}\""
        )));
    }
    let window = yuna.agent_window(seat).map_err(failure)?;
    let reference_window = yuna.agent_window(reference_seat).map_err(failure)?;
    if (&window["shell"], &reference_window["shell"]) != (&json!("x11"), &json!("xdg")) {
        return Err(failure(format!(
            "the windows are of shells {} and {}, not x11 and xdg",
            window["shell"], reference_window["shell"]
        )));
    }
    let size = |window: &Value| {
        (
            window["rect"]["width"].clone(),
            window["rect"]["height"].clone(),
        )
    };
    if size(&window) != size(&reference_window) {
        return Err(failure(format!(
            "on X11 its window is {}, on Wayland {}: they cannot be compared",
            window["rect"], reference_window["rect"]
        )));
    }

    let x11 = yuna.x11_window(seat).map_err(failure)?;
    if x11.dpi != 144 {
        return Err(failure(format!("X11 gives Xft.dpi {}, not 144", x11.dpi)));
    }
    let physical = |seat: u64, window: &Value| {
        yuna.capture_image(json!({"seat": seat, "window": window["id"], "size": "physical"}))
    };
    let drawn = physical(seat, &window).map_err(failure)?;
    let corner = yuna.corner_on_output(seat, &window).map_err(failure)?;
    let drawn = drawn.window_part(corner, &x11.image).map_err(failure)?;
    let mismatches = drawn.differences(&x11.image, 0);
    if mismatches.count > 0 {
        return Err(failure(format!(
            "its capture at 1.5 is not what X11 holds of it: {mismatches}"
        )));
    }
    sharp_lines(&drawn).map_err(failure)?;
    let reference_drawn = physical(reference_seat, &reference_window).map_err(failure)?;
    let reference_corner = yuna
        .corner_on_output(reference_seat, &reference_window)
        .map_err(failure)?;
    let reference_drawn = reference_drawn
        .window_part(reference_corner, &x11.image)
        .map_err(failure)?;
    let unlike_wayland = drawn.differences(&reference_drawn, 0);
    if unlike_wayland.count > 0 {
        return Err(failure(format!(
            "its capture at 1.5 is not Qt's on Wayland: {unlike_wayland}"
        )));
    }
    let logical = yuna
        .capture_image(json!({"seat": seat, "window": window["id"]}))
        .map_err(failure)?;
    yuna.call("seat", json!({"action": "close", "seat": reference_seat}))
        .map_err(failure)?;

    let moved = yuna
        .move_to_new_output(seat, &window, OUTPUT_AT_ONE)
        .map_err(failure)?;
    if size(&moved) != size(&window) {
        return Err(failure(format!(
            "moved to the output at 1, its window is {}, not {}'s size",
            moved["rect"], window["rect"]
        )));
    }
    let at_one = yuna
        .capture_image(json!({"seat": seat, "window": window["id"]}))
        .map_err(failure)?;
    let unlike_its_average = at_one.image.differences(&logical.image, 1);
    if unlike_its_average.count > 0 {
        return Err(failure(format!(
            "at 1, it is not its logical capture at 1.5 within a level: {unlike_its_average}"
        )));
    }
    let open = yuna
        .measure_rest()
        .map_err(|activity| failure(format!("Yuna did not rest with it open: {activity}")))?;
    let at_one_difference = at_one.image.differences(&logical.image, 0);

    yuna.call(
        "keyboard",
        json!({"action": "tap", "seat": seat, "keys": "ctrl+q"}),
    )
    .map_err(failure)?;
    let ended = yuna.wait_for_end(seat, &qml).map_err(failure)?;
    if ended["state"] != json!("exited") || ended["exit_code"] != json!(0) {
        return Err(failure(format!("it did not end well: {ended}")));
    }
    yuna.wait_until_logged("Xwayland stopped", 1)
        .map_err(failure)?;
    yuna.wait_until_quiet(QUIET, PATIENCE).map_err(failure)?;
    let after = yuna.measure_rest().map_err(|activity| {
        failure(format!("Yuna did not rest once Xwayland ended: {activity}"))
    })?;
    yuna.stop()?;
    Ok(format!(
        "qml on X11 at 1.5: quiet after {} frames drawn, {} commits; its window of {} × {} X11 \
         pixels drawn as X11 holds it and as Qt draws it on Wayland, exactly; at 1, within a \
         level of its average ({} pixels a level off); with it open, {open}; it quit by Ctrl+Q, \
         Xwayland ended, then {after}",
        shown.frames_drawn,
        shown.commits,
        x11.image.width,
        x11.image.height,
        at_one_difference.count
    ))
}

/// Requires that [`SHARP_QML`]'s lines be one physical pixel, on the background on both sides,
/// in `drawn`, its window's pixels at 1.5.
fn sharp_lines(drawn: &Image) -> Result<(), String> {
    let corner = SHARP_CORNER;
    let colour = |on_a_line: bool| {
        if on_a_line {
            SHARP_LINE
        } else {
            SHARP_BACKGROUND
        }
    };
    let mut expected = Vec::new();
    // The line across, the row above it and the row below, which the line down crosses.
    for x in 0..=corner + SHARP_ACROSS {
        expected.push((x, corner - 1, SHARP_BACKGROUND));
        let across = (corner..corner + SHARP_ACROSS).contains(&x);
        expected.push((x, corner, colour(across)));
        expected.push((x, corner + 1, colour(x == corner)));
    }
    // The line down, and the columns on its left and its right.
    for y in corner + 1..=corner + SHARP_DOWN {
        expected.push((corner - 1, y, SHARP_BACKGROUND));
        expected.push((corner, y, colour(y < corner + SHARP_DOWN)));
        expected.push((corner + 1, y, SHARP_BACKGROUND));
    }
    for (x, y, colour) in expected {
        let found = drawn.at(x, y);
        if found != colour {
            return Err(format!(
                "its lines are not one sharp pixel: ({x}, {y}) is {found:?}, not {colour:?}"
            ));
        }
    }
    Ok(())
}

/// Wine's own notepad on X11, Wine's X11 driver, through MCP as an agent runs it: one with a
/// fresh prefix at 144 dots per inch on a seat at 1.5, one at 96 on a seat at 1 beside it, each
/// prefix made before its first run. Both are tiled, so their windows' sizes are Yuna's; Wine's
/// own scaling shows in what it draws inside: at 144, its menu bar's height and its scroll bar's
/// width, which Wine scales from whole pixels at 96, are 1.5 times theirs at 96, rounded either
/// way, from at least [`NOTEPAD_BAR_AT_96`] pixels. Yuna shows it pixel for pixel: its capture at
/// the physical size is what X11 holds of the window, exactly. Yuna rests with both open; closed
/// by Alt+F4, they leave, and Yuna rests again once both Xwaylands have ended. The folders their
/// wineservers kept in `/tmp/.wine-<uid>` are then removed.
#[test]
fn wine_notepad_on_x11_scales_by_its_log_pixels_and_is_drawn_sharp() {
    let report = notepad_on_x11().unwrap_or_else(|failure| panic!("{failure}"));
    eprintln!("{report}");
}

/// The fewest pixels a bar of notepad has at 96 dots per inch: Wine's are 17 to 19, so that 1.5
/// times of them cannot pass by accident, as 1 against 1 would.
const NOTEPAD_BAR_AT_96: u32 = 10;

fn notepad_on_x11() -> Result<String, String> {
    let mut yuna = Yuna::start_mcp();
    let prefix_96 = yuna.folder.join("wine-96");
    let prefix_144 = yuna.folder.join("wine-144");
    let made = make_wine_prefixes(&yuna, &[(&prefix_96, 96), (&prefix_144, 144)])?;
    let seat_96 = yuna.create_seat_with(output_mode(OUTPUT_AT_ONE))?;
    let seat_144 = yuna.create_seat_with(output_mode(OUTPUT_AT_ONE_AND_A_HALF))?;
    let command = ["wine", "notepad"];
    let notepad_96 = yuna.start_program(seat_96, &command, &wine_variables(&prefix_96))?;
    let notepad_144 = yuna.start_program(seat_144, &command, &wine_variables(&prefix_144))?;
    let failure = |what: String| {
        format!(
            "notepad: {what}\n{}\nat 96 DPI, its log: {}",
            yuna.diagnosis(&notepad_144.log),
            notepad_96.log.display()
        )
    };
    yuna.agent_window(seat_144).map_err(failure)?;
    yuna.agent_window(seat_96).map_err(failure)?;
    let shown = yuna.wait_until_quiet(QUIET, PATIENCE).map_err(failure)?;
    let window = yuna.agent_window(seat_144).map_err(failure)?;
    for listed in [&window, &yuna.agent_window(seat_96).map_err(failure)?] {
        if listed["shell"] != json!("x11") || listed["app_id"] != json!("notepad.exe") {
            return Err(failure(format!(
                "its window is not X11's notepad.exe: {listed}"
            )));
        }
    }

    let x11 = yuna.x11_window(seat_144).map_err(failure)?;
    let parts = NotepadParts::of(&x11.image).map_err(failure)?;
    let parts_96 =
        NotepadParts::of(&yuna.x11_window(seat_96).map_err(failure)?.image).map_err(failure)?;
    for (part, at_144, at_96) in [
        ("menu bar's height", parts.menu_bar, parts_96.menu_bar),
        ("scroll bar's width", parts.scroll_bar, parts_96.scroll_bar),
    ] {
        if at_96 < NOTEPAD_BAR_AT_96 || (f64::from(at_144) - 1.5 * f64::from(at_96)).abs() > 0.5 {
            return Err(failure(format!(
                "its {part} is {at_144} pixels at 144 DPI, {at_96} at 96: not 1.5 times, from \
                 {NOTEPAD_BAR_AT_96} at least"
            )));
        }
    }
    let drawn = yuna
        .capture_image(json!({"seat": seat_144, "window": window["id"], "size": "physical"}))
        .map_err(failure)?;
    let corner = yuna.corner_on_output(seat_144, &window).map_err(failure)?;
    let drawn = drawn.window_part(corner, &x11.image).map_err(failure)?;
    let mismatches = drawn.differences(&x11.image, 0);
    if mismatches.count > 0 {
        return Err(failure(format!(
            "its capture at 1.5 is not what X11 holds of it: {mismatches}"
        )));
    }
    let open = yuna
        .measure_rest()
        .map_err(|activity| failure(format!("Yuna did not rest with it open: {activity}")))?;

    for (seat, notepad) in [(seat_144, &notepad_144), (seat_96, &notepad_96)] {
        yuna.call(
            "keyboard",
            json!({"action": "tap", "seat": seat, "keys": "alt+F4"}),
        )
        .map_err(failure)?;
        let ended = yuna.wait_for_end(seat, notepad).map_err(failure)?;
        if ended["state"] != json!("exited") || ended["exit_code"] != json!(0) {
            return Err(failure(format!("it did not end well: {ended}")));
        }
    }
    yuna.wait_until_logged("Xwayland stopped", 2)
        .map_err(failure)?;
    yuna.wait_until_quiet(QUIET, PATIENCE).map_err(failure)?;
    let after = yuna.measure_rest().map_err(|activity| {
        failure(format!("Yuna did not rest once Xwayland ended: {activity}"))
    })?;
    for prefix in [&prefix_96, &prefix_144] {
        remove_wineserver_folder(&yuna, prefix).map_err(failure)?;
    }
    yuna.stop()?;
    Ok(format!(
        "notepad on X11: both prefixes made in {:.1} s; quiet after {} frames drawn, {} \
         commits; at 144 DPI, its menu bar {} pixels high against {} at 96, its scroll bar {} \
         wide against {}; its window of {} × {} X11 pixels drawn as X11 holds it, exactly; with \
         both open, {open}; closed by Alt+F4, both Xwaylands ended, then {after}",
        made.as_secs_f64(),
        shown.frames_drawn,
        shown.commits,
        parts.menu_bar,
        parts_96.menu_bar,
        parts.scroll_bar,
        parts_96.scroll_bar,
        x11.image.width,
        x11.image.height,
    ))
}

/// The parts of notepad's window that Wine sizes by its dots per inch, in X11 pixels, measured
/// on its pixels: Wine draws them in flat colours.
#[derive(Debug)]
struct NotepadParts {
    /// The rows of the menu bar's colour from the top, down the window's middle column, past
    /// the menu's text.
    menu_bar: u32,
    /// The scroll bar's track across the window's middle row: the last run of one colour at
    /// least 4 pixels long, before the window's right border.
    scroll_bar: u32,
}

impl NotepadParts {
    fn of(window: &Image) -> Result<NotepadParts, String> {
        let middle_column: Vec<[u8; 3]> = (0..window.height)
            .map(|y| window.at(window.width / 2, y))
            .collect();
        let middle_row: Vec<[u8; 3]> = (0..window.width)
            .map(|x| window.at(x, window.height / 2))
            .collect();
        let column_runs = runs(&middle_column);
        let menu_bar = match column_runs.as_slice() {
            [menu_bar, _, ..] => *menu_bar,
            _ => return Err(format!("no menu bar in {column_runs:?}")),
        };
        let scroll_bar = runs(&middle_row)
            .into_iter()
            .rev()
            .find(|&run| run >= 4)
            .ok_or("no scroll bar")?;
        Ok(NotepadParts {
            menu_bar,
            scroll_bar,
        })
    }
}

/// The lengths of the runs of equal pixels in `pixels`, in order.
fn runs(pixels: &[[u8; 3]]) -> Vec<u32> {
    pixels
        .chunk_by(|a, b| a == b)
        .map(|run| u32::try_from(run.len()).expect("a run of an image's side"))
        .collect()
}

/// Makes fresh Wine prefixes, each in its folder at its dots per inch, all at once, before any
/// of their programs runs: the test runs Wine itself, with no display. Each prefix's registry
/// says its dots per inch, `LogPixels`, Wine's X11 driver, and a caret that never blinks,
/// `CursorBlinkRate` -1 as on Windows, so that notepad, once drawn, has no reason to draw
/// again. Gives how long Wine took to make them, its wineserver's end not counted.
fn make_wine_prefixes(yuna: &Yuna, prefixes: &[(&Path, u32)]) -> Result<Duration, String> {
    let started = Instant::now();
    let mut imports = Vec::new();
    for &(prefix, dpi) in prefixes {
        let registry = prefix.with_extension("reg");
        fs::write(&registry, wine_registry(dpi))
            .map_err(|error| format!("{}: {error}", registry.display()))?;
        let mut command = wine_tool(yuna, prefix, "wine");
        command.args(["reg", "import"]).arg(&registry);
        imports.push(Application::start(
            &format!("wine-prefix-{dpi}"),
            yuna,
            command,
        ));
    }
    for import in &mut imports {
        let status = import.wait(PATIENCE)?;
        if !status.success() {
            return Err(format!(
                "Wine's reg import ended with {status}: {}",
                import.log.display()
            ));
        }
    }
    let made = started.elapsed();
    // Its wineserver writes the registry when it ends, 3 s after its last program: notepad
    // then starts its own, with the seat's display.
    for &(prefix, _) in prefixes {
        wait_for_wineserver(yuna, prefix)?;
    }
    Ok(made)
}

/// Waits until `prefix`'s wineserver, if one runs, has ended.
fn wait_for_wineserver(yuna: &Yuna, prefix: &Path) -> Result<(), String> {
    let name = format!(
        "wineserver-{}",
        prefix.file_name().unwrap_or_default().to_string_lossy()
    );
    let mut command = wine_tool(yuna, prefix, "wineserver");
    command.arg("--wait");
    let status = Application::start(&name, yuna, command).wait(PATIENCE)?;
    if !status.success() {
        return Err(format!("wineserver --wait ended with {status}"));
    }
    Ok(())
}

/// Removes the folder `prefix`'s wineservers kept beside the machine's own, once the last has
/// ended: Wine names it after the prefix's device and inode, in hexadecimal, always in `/tmp`,
/// and leaves its empty lock file in it. Anything else in it makes the removal fail.
fn remove_wineserver_folder(yuna: &Yuna, prefix: &Path) -> Result<(), String> {
    wait_for_wineserver(yuna, prefix)?;
    let prefix = fs::metadata(prefix).map_err(|error| format!("{}: {error}", prefix.display()))?;
    let folder = PathBuf::from(format!(
        "/tmp/.wine-{}/server-{:x}-{:x}",
        prefix.uid(),
        prefix.dev(),
        prefix.ino()
    ));
    let removed = |error: std::io::Error| format!("{}: {error}", folder.display());
    match fs::remove_file(folder.join("lock")) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(removed(error)),
        _ => {}
    }
    fs::remove_dir(&folder).map_err(removed)
}

/// The registry file that sets a prefix's dots per inch, its X11 driver and its caret.
fn wine_registry(dpi: u32) -> String {
    format!(
        "REGEDIT4\n\n\
         [HKEY_CURRENT_USER\\Control Panel\\Desktop]\n\
         \"LogPixels\"=dword:{dpi:08x}\n\
         \"CursorBlinkRate\"=\"-1\"\n\n\
         [HKEY_CURRENT_USER\\Software\\Wine\\Drivers]\n\
         \"Graphics\"=\"x11\"\n"
    )
}

/// What Wine's programs get: their prefix; no Mono and no Gecko, which Wine would offer to
/// download in a dialog; none of its drivers for input devices and USB, `winebus.sys` and
/// `wineusb.sys`, which open the machine's `/dev/input`, `/dev/hidraw` and `/dev/bus/usb`
/// read-write; a print server that does not exist, a socket beside the prefix, where CUPS's
/// library would otherwise reach the machine's over TCP, `localhost:631`; no system bus, through
/// which Wine would list the machine's disks; and none of its debugging lines but errors.
fn wine_variables(prefix: &Path) -> Vec<(&'static str, String)> {
    vec![
        ("WINEPREFIX", prefix.to_string_lossy().into_owned()),
        (
            "CUPS_SERVER",
            prefix.with_extension("cups").to_string_lossy().into_owned(),
        ),
        (
            "WINEDLLOVERRIDES",
            "mscoree,mshtml=;winebus.sys,wineusb.sys=d".to_owned(),
        ),
        ("DBUS_SYSTEM_BUS_ADDRESS", "disabled:".to_owned()),
        ("WINEDEBUG", "fixme-all,warn-all".to_owned()),
    ]
}

/// `program`, one of Wine's, run by the test itself on `prefix`, with the applications'
/// environment and no display.
fn wine_tool(yuna: &Yuna, prefix: &Path, program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .envs(application_environment(&yuna.home()))
        .envs(wine_variables(prefix))
        .stdin(Stdio::null());
    dies_with_test(&mut command);
    command
}

/// An agent's output of `mode`, physical width, height and scale, at 60 Hz, as `seat create`
/// takes it.
fn output_mode((width, height, scale): (u32, u32, f64)) -> Value {
    json!({"action": "create", "width": width, "height": height, "rate": 60, "scale": scale})
}

/// The folder of SlimeEngine's examples, built at the revision
/// `.github/workflows/slimeengine.yml` pins: the `window`, `interface` and `editor` binaries,
/// `cooked/`, the examples' cooked data, and `assets/`, their sources, the editor's yard among
/// them. That weekly job names it in `YUNA_SLIMEENGINE_EXAMPLES` and runs the ignored tests:
/// `cargo test --test applications -- --ignored`.
fn slimeengine_examples() -> PathBuf {
    std::env::var_os("YUNA_SLIMEENGINE_EXAMPLES")
        .map(PathBuf::from)
        .expect("YUNA_SLIMEENGINE_EXAMPLES names the folder of SlimeEngine's examples")
}

/// What SlimeEngine's examples load at run time beside the shell's EGL: winit's and wgpu's
/// libraries, and fontconfig, through which uwui finds DejaVu.
fn slimeengine_libraries() -> (&'static str, String) {
    (
        "LD_LIBRARY_PATH",
        format!(
            "{}:{}",
            shell_variable("YUNA_CLIENT_LIBRARY_PATH"),
            shell_variable("LD_LIBRARY_PATH")
        ),
    )
}

/// SlimeEngine's `window` example, which draws through wgpu and Vulkan, draws its frames and
/// leaves. It cannot rest with its window shown: it draws at its frame rate while it shows
/// ([ADR-0043](../../../docs/adr/0043-text-input-and-the-rest-of-slimeengines-examples.md)).
#[test]
#[ignore = "needs SlimeEngine's examples, which the weekly job builds"]
fn slimeengine_window_runs_and_yuna_rests_after_it() {
    let window = slimeengine_examples().join("window");
    let frames = FRAMES.to_string();
    let report = draw_leave_and_rest("SlimeEngine's window", |yuna| {
        let mut command = application(yuna, &window);
        let (name, libraries) = slimeengine_libraries();
        command.env(name, libraries).args(["--frames", &frames]);
        command
    })
    .unwrap_or_else(|failure| panic!("{failure}"));
    eprintln!("{report}");
}

/// SlimeEngine's `editor` example, on wgpu and Vulkan, runs its frames on demand: opened on a
/// copy of its yard, through MCP, it rests with its window shown.
#[test]
#[ignore = "needs SlimeEngine's examples, which the weekly job builds"]
fn slimeengine_editor_rests_with_its_window_open() {
    let report = editor_at_rest().unwrap_or_else(|failure| panic!("{failure}"));
    eprintln!("{report}");
}

/// How long Yuna must go without a round before the editor counts as quiet: longer than its
/// first run's hint shows.
const EDITOR_QUIET: Duration = Duration::from_secs(11);

fn editor_at_rest() -> Result<String, String> {
    let examples = slimeengine_examples();
    let mut yuna = Yuna::start_mcp();
    // The editor writes the level it edits: a copy of the yard, with its metadata file.
    let yard = yuna.folder.join("yard");
    fs::create_dir_all(&yard).map_err(|error| error.to_string())?;
    for file in ["yard.scene", "yard.scene.meta"] {
        fs::copy(examples.join("assets").join(file), yard.join(file))
            .map_err(|error| format!("{file}: {error}"))?;
    }
    let seat = yuna.create_seat()?;
    let level = yard.join("yard.scene");
    let journal = yuna.folder.join("yard.journal");
    let cooked = examples.join("cooked");
    let editor = examples.join("editor");
    let command = [
        editor.to_str().ok_or("a path")?,
        "--level",
        level.to_str().ok_or("a path")?,
        "--assets",
        cooked.to_str().ok_or("a path")?,
        "--journal",
        journal.to_str().ok_or("a path")?,
    ];
    let program = yuna.start_program(seat, &command, &[slimeengine_libraries()])?;
    let failure = |what: String| format!("editor: {what}\n{}", yuna.diagnosis(&program.log));
    yuna.agent_window(seat).map_err(failure)?;
    // Its first run shows a hint, a notification that goes after up to 10 s, uwui's longest
    // (`Severity::Warning`): its rest is measured once Yuna has had no round for longer.
    let quiet = yuna
        .wait_until_quiet(EDITOR_QUIET, PATIENCE)
        .map_err(failure)?;
    let rest = yuna
        .measure_rest()
        .map_err(|activity| failure(format!("Yuna did not rest: {activity}")))?;
    if !yuna.program_runs(seat, &program).map_err(failure)? {
        return Err(failure("it left during the rest window".to_owned()));
    }
    if !yuna.window_shown(seat).map_err(failure)? {
        return Err(failure("its window was not shown at rest".to_owned()));
    }
    yuna.stop()?;
    Ok(format!(
        "SlimeEngine's editor: quiet after {} frames drawn, {} commits; then {rest}",
        quiet.frames_drawn, quiet.commits
    ))
}

/// Where the bar of SlimeEngine's `interface` example puts what the story uses, in logical
/// units from its window's top-left corner, at the pinned revision, its window 936 × 1048 as the
/// story's output tiles it: the text field, its second button, "view", and the end of the bar,
/// which says what the last action did.
const INTERFACE_FIELD: (f64, f64) = (572.0, 20.0);
const INTERFACE_FIELD_AREA: (u32, u32, u32, u32) = (492, 6, 160, 28);
const INTERFACE_VIEW_BUTTON: (f64, f64) = (152.0, 20.0);
const INTERFACE_STATUS_AREA: (u32, u32, u32, u32) = (744, 0, 176, 46);
/// The size of the window those places assume.
const INTERFACE_WINDOW: (f64, f64) = (936.0, 1048.0);
/// How long a part of a window must stay the same to count as still.
const STILL: Duration = Duration::from_secs(1);

/// The done-when of M1.5: an agent, through MCP alone, opens one of SlimeEngine's examples,
/// types, clicks, and checks the result by capture. It starts `interface`, clicks its text field
/// and waits for the field to ask for an input method, types text with characters outside `us`,
/// committed through `text-input-v3`, shows text being composed, and clicks a button. Before each
/// step, the part of the window it changes is captured once it stands still; after the step, a
/// capture of it must differ. The window's size and a field not enabled before the click are
/// checked first.
#[test]
#[ignore = "needs SlimeEngine's examples, which the weekly job builds"]
fn an_agent_types_and_clicks_in_slimeengines_interface_and_checks_by_capture() {
    let report = interface_story().unwrap_or_else(|failure| panic!("{failure}"));
    eprintln!("{report}");
}

fn interface_story() -> Result<String, String> {
    let examples = slimeengine_examples();
    let mut yuna = Yuna::start_mcp();
    let seat = yuna.create_seat_with(
        json!({"action": "create", "width": 1920, "height": 1080, "rate": 60, "scale": 1}),
    )?;
    let interface = examples.join("interface");
    let cooked = examples.join("cooked");
    let command = [
        interface.to_str().ok_or("a path")?,
        "--assets",
        cooked.to_str().ok_or("a path")?,
    ];
    let program = yuna.start_program(seat, &command, &[slimeengine_libraries()])?;
    let failure = |what: String| format!("interface: {what}\n{}", yuna.diagnosis(&program.log));
    let window = yuna.agent_window(seat).map_err(failure)?;
    let origin = (
        window["rect"]["x"].as_f64().ok_or("no rectangle")?,
        window["rect"]["y"].as_f64().ok_or("no rectangle")?,
    );
    // The window is placed on whole logical units at scale 1.
    let area = |(x, y, width, height): (u32, u32, u32, u32)| {
        json!({
            "x": origin.0 as u32 + x,
            "y": origin.1 as u32 + y,
            "width": width,
            "height": height,
        })
    };
    let field_area = area(INTERFACE_FIELD_AREA);
    let status_area = area(INTERFACE_STATUS_AREA);
    let size = (
        window["rect"]["width"].as_f64(),
        window["rect"]["height"].as_f64(),
    );
    if size != (Some(INTERFACE_WINDOW.0), Some(INTERFACE_WINDOW.1)) {
        return Err(failure(format!(
            "its window is {size:?}, not the {INTERFACE_WINDOW:?} the story's places assume"
        )));
    }
    yuna.wait_for_frames().map_err(failure)?;
    let keyboard = |yuna: &Yuna| {
        yuna.call("keyboard", json!({"action": "get", "seat": seat}))
            .map(|got| got["structuredContent"]["text_field"] == json!(true))
    };
    if keyboard(&yuna).map_err(failure)? {
        return Err(failure("a field was enabled before the click".to_owned()));
    }

    let click = |(x, y): (f64, f64)| {
        yuna.call(
            "pointer",
            json!({"action": "click", "seat": seat, "x": origin.0 + x, "y": origin.1 + y}),
        )
    };
    click(INTERFACE_FIELD).map_err(failure)?;
    let started = Instant::now();
    while !keyboard(&yuna).map_err(failure)? {
        if started.elapsed() > PATIENCE {
            return Err(failure(
                "its field never asked for an input method".to_owned(),
            ));
        }
        std::thread::sleep(POLL);
    }
    let field_enabled = started.elapsed();

    // Each step is checked against a capture taken once the part it changes stood still after
    // the step before, so that only that step's input can change it.
    let key = |action: &str, field: &str, value: &str| {
        yuna.call(
            "keyboard",
            json!({"action": action, "seat": seat, field: value}),
        )
    };
    key("tap", "keys", "ctrl+a").map_err(failure)?;
    let selected = yuna.still_capture(seat, &field_area).map_err(failure)?;
    key("type", "text", "Grüße").map_err(failure)?;
    yuna.capture_changed(seat, &field_area, &selected)
        .map_err(failure)?;
    let typed = yuna.still_capture(seat, &field_area).map_err(failure)?;
    key("preedit", "text", "ña").map_err(failure)?;
    yuna.capture_changed(seat, &field_area, &typed)
        .map_err(failure)?;
    let status = yuna.still_capture(seat, &status_area).map_err(failure)?;
    click(INTERFACE_VIEW_BUTTON).map_err(failure)?;
    yuna.capture_changed(seat, &status_area, &status)
        .map_err(failure)?;
    yuna.stop()?;
    Ok(format!(
        "SlimeEngine's interface through MCP: its field enabled {} ms after the click; the \
         field changed when typed into and when text was composed, and its bar when a button \
         was clicked, each against the part at rest after the step before",
        field_enabled.as_millis()
    ))
}

fn run_and_rest(name: &str, command: impl FnOnce(&Yuna) -> Command) -> Result<String, String> {
    let started = Instant::now();
    let yuna = Yuna::start();
    let mut app = Application::start(name, &yuna, command(&yuna));
    let app_log = app.log.clone();
    let failure = |what: String| format!("{name}: {what}\n{}", yuna.diagnosis(&app_log));
    yuna.wait_for_window(&mut app).map_err(failure)?;
    let shown = started.elapsed();
    let quiet = yuna.wait_until_quiet(QUIET, PATIENCE).map_err(failure)?;
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
    let left = yuna.wait_until_quiet(QUIET, PATIENCE).map_err(failure)?;
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

/// Qt's QML runner on its Wayland platform.
fn qml(yuna: &Yuna) -> Command {
    let mut command = application(yuna, "qml");
    command.envs(qt_variables("wayland"));
    command
}

/// What Qt's QML runner needs on `platform`, `wayland` or X11's `xcb`: its plugins and QML's
/// modules, since `qml` is not wrapped; its OpenGL renderer, the default, pinned; and its log on
/// its standard error, which Qt would send to the machine's journal when that is not a
/// terminal.
fn qt_variables(platform: &str) -> Vec<(&'static str, String)> {
    vec![
        ("QT_QPA_PLATFORM", platform.to_owned()),
        ("QT_PLUGIN_PATH", shell_variable("YUNA_QT_PLUGINS")),
        ("QML_IMPORT_PATH", shell_variable("YUNA_QML_IMPORTS")),
        ("QSG_RHI_BACKEND", "opengl".to_owned()),
        ("QT_FORCE_STDERR_LOGGING", "1".to_owned()),
    ]
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

/// A `yuna` process, `yuna --headless` or `yuna mcp`, with one output, in a folder of its own:
/// its runtime folder, its log, and its application's home. The output is 1920 × 1080 at 60 Hz
/// and scale 1, or the mode `YUNA_APPLICATIONS_OUTPUT` names, as `--output` takes it, to measure
/// rest on another output; `yuna mcp` gives it to the seat the test creates.
struct Yuna {
    child: Child,
    folder: PathBuf,
    log: PathBuf,
    /// Its Wayland socket's path: the owner's seat's for `yuna --headless`, the agent's seat's
    /// for `yuna mcp` once [`Yuna::create_seat`] made it.
    socket: PathBuf,
    /// The MCP connection of `yuna mcp`: its input, and its replies, read on a thread.
    mcp: Option<RefCell<Mcp>>,
    /// Whether a failure was reported: the folder, with its logs, is then kept.
    failed: Cell<bool>,
}

/// The test's side of `yuna mcp`'s standard input and output.
struct Mcp {
    input: ChildStdin,
    replies: mpsc::Receiver<Value>,
    next_id: u64,
}

/// A program an agent started through `spawn`: its id and its log.
struct Program {
    id: u64,
    log: PathBuf,
}

impl Yuna {
    /// `yuna --headless`, its socket read from what it prints.
    fn start() -> Yuna {
        let mut yuna = Yuna::launch(&["--headless"], Stdio::null());
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

    /// `yuna mcp`, driven as an agent drives it: requests on its standard input, one a line,
    /// replies read from its standard output on a thread.
    fn start_mcp() -> Yuna {
        let mut yuna = Yuna::launch(&["mcp"], Stdio::piped());
        let stdout = yuna.child.stdout.take().expect("its standard output");
        let input = yuna.child.stdin.take().expect("its standard input");
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
        yuna.mcp = Some(RefCell::new(Mcp {
            input,
            replies,
            next_id: 1,
        }));
        yuna
    }

    /// Starts `yuna` with `mode`'s arguments and its output, at the `debug` level.
    fn launch(mode: &[&str], input: Stdio) -> Yuna {
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
            .args(mode)
            .args(["--output", &output])
            // What it draws with, from the shell; Xwayland, for its seats' X11 programs; a
            // throwaway home, so that Mesa reads none of the user's settings.
            .env_clear()
            .env("PATH", shell_variable("YUNA_XWAYLAND_PATH"))
            .env("LD_LIBRARY_PATH", shell_variable("LD_LIBRARY_PATH"))
            .env(
                "__EGL_VENDOR_LIBRARY_FILENAMES",
                shell_variable("__EGL_VENDOR_LIBRARY_FILENAMES"),
            )
            .env("XKB_CONFIG_ROOT", shell_variable("XKB_CONFIG_ROOT"))
            .env("HOME", folder.join("yuna-home"))
            .env("XDG_RUNTIME_DIR", folder.join("runtime"))
            .env("YUNA_LOG", "debug")
            .stdin(input)
            .stdout(Stdio::piped())
            .stderr(File::create(&log).expect("Yuna's log"));
        dies_with_test(&mut command);
        let child = command.spawn().expect("the yuna binary runs");
        Yuna {
            child,
            folder,
            log,
            socket: PathBuf::new(),
            mcp: None,
            failed: Cell::new(false),
        }
    }

    /// Calls an MCP tool of `yuna mcp` and gives its result, or what it refused.
    fn call(&self, tool: &str, arguments: Value) -> Result<Value, String> {
        let mut mcp = self.mcp.as_ref().expect("yuna mcp").borrow_mut();
        let id = mcp.next_id;
        mcp.next_id += 1;
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
        writeln!(mcp.input, "{request}").map_err(|error| format!("{tool}: {error}"))?;
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let reply = mcp
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

    /// Creates an agent's seat through MCP, with an output of `--output`'s mode, and gives its
    /// handle; its socket is then the one its programs use.
    fn create_seat(&mut self) -> Result<u64, String> {
        self.create_seat_with(json!({"action": "create"}))
    }

    /// Creates an agent's seat through MCP with `arguments`, `action` among them.
    fn create_seat_with(&mut self, arguments: Value) -> Result<u64, String> {
        let created = self.call("seat", arguments)?;
        let seat = &created["structuredContent"];
        self.socket = PathBuf::from(seat["socket"].as_str().ok_or("no socket")?);
        seat["seat"].as_u64().ok_or_else(|| "no handle".to_owned())
    }

    /// Starts `command` on `seat` through MCP, with the environment `application` gives, and
    /// `variables` over it.
    fn start_program(
        &self,
        seat: u64,
        command: &[&str],
        variables: &[(&str, String)],
    ) -> Result<Program, String> {
        let mut env: BTreeMap<String, String> = application_environment(&self.home())
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_string_lossy().into_owned()))
            .collect();
        env.extend(
            variables
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone())),
        );
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

    /// Waits through MCP for `seat`'s first window, placed, and gives it.
    fn agent_window(&self, seat: u64) -> Result<Value, String> {
        let found = self.call(
            "windows",
            json!({"seat": seat, "wait_ms": PATIENCE.as_millis().min(50_000)}),
        )?;
        let window = found["structuredContent"]["windows"][0].clone();
        if window.is_null() {
            return Err(format!("no window in {} s", PATIENCE.as_secs()));
        }
        Ok(window)
    }

    /// Waits until Yuna has drawn a frame since this call: its client draws.
    fn wait_for_frames(&self) -> Result<(), String> {
        let before = self.counters();
        let deadline = Instant::now() + PATIENCE;
        while self.counters().frames_drawn == before.frames_drawn {
            if Instant::now() > deadline {
                return Err(format!("no frame drawn in {} s", PATIENCE.as_secs()));
            }
            std::thread::sleep(POLL);
        }
        Ok(())
    }

    /// Captures `region` of `seat`'s first output through MCP, and gives its PNG in base64: two
    /// captures are the same image when their texts are equal.
    fn capture(&self, seat: u64, region: &Value) -> Result<String, String> {
        let captured = self.call("capture", json!({"seat": seat, "region": region}))?;
        captured["content"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|content| content["type"] == json!("image"))
            .and_then(|image| image["data"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| format!("no image in {captured}"))
    }

    /// Captures `region` again and again until it differs from `before`, as an agent waits for
    /// its application to draw what its input changed, and gives it.
    fn capture_changed(&self, seat: u64, region: &Value, before: &str) -> Result<String, String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let now = self.capture(seat, region)?;
            if now != before {
                return Ok(now);
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "{region} did not change in {} s",
                    PATIENCE.as_secs()
                ));
            }
            std::thread::sleep(POLL);
        }
    }

    /// Captures `region` every [`POLL`] until it has stayed the same image for [`STILL`], and
    /// gives it: sampled through the whole second, so that a caret blinking or an animation is
    /// not taken for still.
    fn still_capture(&self, seat: u64, region: &Value) -> Result<String, String> {
        let deadline = Instant::now() + PATIENCE;
        let mut last = self.capture(seat, region)?;
        let mut since = Instant::now();
        loop {
            std::thread::sleep(POLL);
            let now = self.capture(seat, region)?;
            if now != last {
                last = now;
                since = Instant::now();
            } else if since.elapsed() >= STILL {
                return Ok(last);
            }
            if Instant::now() > deadline {
                return Err(format!("{region} never stood still"));
            }
        }
    }

    /// Captures through MCP with `arguments`, and gives the image decoded, with where it is on
    /// its output. Its PNG is kept in Yuna's folder, which a failure keeps.
    fn capture_image(&self, arguments: Value) -> Result<Capture, String> {
        let captured = self.call("capture", arguments.clone())?;
        let data = captured["content"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|content| content["type"] == json!("image"))
            .and_then(|image| image["data"].as_str())
            .ok_or_else(|| format!("no image in {captured}"))?;
        let png = decode_base64(data)?;
        let mapping = &captured["structuredContent"];
        let file = format!("capture-{}-{}.png", arguments["seat"], mapping["capture"]);
        fs::write(self.folder.join(&file), &png).map_err(|error| format!("{file}: {error}"))?;
        let number = |name: &str| {
            mapping[name]
                .as_f64()
                .ok_or_else(|| format!("no {name} in {mapping}"))
        };
        Ok(Capture {
            image: Image::from_png(&png)?,
            x: number("x")?,
            y: number("y")?,
            units_per_pixel: number("units_per_pixel")?,
        })
    }

    /// `seat`'s X display, as its programs get it: `env`, started through `spawn`, prints it.
    fn seat_display(&self, seat: u64) -> Result<SeatDisplay, String> {
        let program = self.start_program(seat, &["env"], &[])?;
        let deadline = Instant::now() + PATIENCE;
        loop {
            // Its log is whole once the program has ended and the log ends with a line's end.
            let log = fs::read_to_string(&program.log).unwrap_or_default();
            let variable = |name: &str| {
                log.lines()
                    .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            };
            if let (Some(display), Some(xauthority), true) = (
                variable("DISPLAY"),
                variable("XAUTHORITY"),
                log.ends_with('\n'),
            ) && self.listed_program(seat, &program)?["state"] != json!("running")
            {
                let number = display
                    .strip_prefix(':')
                    .and_then(|number| number.parse().ok())
                    .ok_or_else(|| format!("DISPLAY is {display}, not :<number>"))?;
                return Ok(SeatDisplay {
                    number,
                    cookie: cookie_of(Path::new(xauthority))?,
                });
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "no DISPLAY and XAUTHORITY in {} s: {log}",
                    PATIENCE.as_secs()
                ));
            }
            std::thread::sleep(POLL);
        }
    }

    /// What X11 holds of `seat`'s one managed window, read as a program of that seat reads it,
    /// through its display.
    fn x11_window(&self, seat: u64) -> Result<X11Window, String> {
        read_x11_window(&self.seat_display(seat)?)
    }

    /// `window`'s top-left corner, as `windows` gives it, on its output of `seat`, in logical
    /// units: `windows` gives rectangles in the space all outputs share.
    fn corner_on_output(&self, seat: u64, window: &Value) -> Result<(f64, f64), String> {
        let listed = self.call("outputs", json!({"action": "list", "seat": seat}))?;
        let output = listed["structuredContent"]["outputs"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|output| output["id"] == window["output"])
            .ok_or_else(|| format!("no output of {window}"))?;
        let at = |of: &Value, axis: &str| {
            of[axis]
                .as_f64()
                .ok_or_else(|| format!("no {axis} in {of}"))
        };
        Ok((
            at(&window["rect"], "x")? - at(&output["logical"], "x")?,
            at(&window["rect"], "y")? - at(&output["logical"], "y")?,
        ))
    }

    /// Adds an output of `mode` to `seat`, shows a new workspace on it and moves `window` there;
    /// gives the window once Yuna has gone quiet.
    fn move_to_new_output(
        &self,
        seat: u64,
        window: &Value,
        (width, height, scale): (u32, u32, f64),
    ) -> Result<Value, String> {
        let added = self.call(
            "outputs",
            json!({
                "action": "create", "seat": seat, "width": width, "height": height,
                "rate": 60, "scale": scale,
            }),
        )?;
        let output = &added["structuredContent"]["output"]["id"];
        let workspace = "moved";
        self.call(
            "workspaces",
            json!({"action": "create", "seat": seat, "name": workspace, "output": output}),
        )?;
        self.call(
            "workspaces",
            json!({"action": "focus", "seat": seat, "name": workspace}),
        )?;
        self.call(
            "workspaces",
            json!({
                "action": "move_window", "seat": seat, "window": window["id"],
                "workspace": workspace,
            }),
        )?;
        self.wait_until_quiet(QUIET, PATIENCE)?;
        let listed = self.call("windows", json!({"seat": seat}))?;
        let moved = listed["structuredContent"]["windows"][0].clone();
        if moved["output"] != *output || moved["rect"].is_null() {
            return Err(format!(
                "the window was not shown on output {output}: {moved}"
            ));
        }
        Ok(moved)
    }

    /// Waits until Yuna's log holds `count` lines that contain `text`.
    fn wait_until_logged(&self, text: &str, count: usize) -> Result<(), String> {
        let deadline = Instant::now() + PATIENCE;
        while self
            .read_log()
            .lines()
            .filter(|line| line.contains(text))
            .count()
            < count
        {
            if Instant::now() > deadline {
                return Err(format!(
                    "Yuna did not log \"{text}\" {count} times in {} s",
                    PATIENCE.as_secs()
                ));
            }
            std::thread::sleep(POLL);
        }
        Ok(())
    }

    /// Whether `seat`'s first window is still shown, placed on an output.
    fn window_shown(&self, seat: u64) -> Result<bool, String> {
        let listed = self.call("windows", json!({"seat": seat}))?;
        Ok(!listed["structuredContent"]["windows"][0]["rect"].is_null())
    }

    /// Whether `program` of `seat` still runs, as `spawn list` says.
    fn program_runs(&self, seat: u64, program: &Program) -> Result<bool, String> {
        Ok(self.listed_program(seat, program)?["state"] == json!("running"))
    }

    /// `program` of `seat` as `spawn list` gives it: its state, exit code and signal among
    /// them; null when it is not listed.
    fn listed_program(&self, seat: u64, program: &Program) -> Result<Value, String> {
        let listed = self.call("spawn", json!({"action": "list", "seat": seat}))?;
        Ok(listed["structuredContent"]["programs"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["id"] == json!(program.id))
            .cloned()
            .unwrap_or_default())
    }

    /// Waits through MCP until `program` of `seat` no longer runs, and gives what `spawn list`
    /// then says of it.
    fn wait_for_end(&self, seat: u64, program: &Program) -> Result<Value, String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let listed = self.listed_program(seat, program)?;
            if listed["state"] != json!("running") {
                return Ok(listed);
            }
            if Instant::now() > deadline {
                return Err(format!("it did not end in {} s", PATIENCE.as_secs()));
            }
            std::thread::sleep(POLL);
        }
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

/// An image of R, G, B pixels, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// Pixel (`x`, `y`).
    fn at(&self, x: u32, y: u32) -> [u8; 3] {
        self.pixels[(y * self.width + x) as usize]
    }

    /// The part of this image `width` × `height` from (`left`, `top`), if it holds it all.
    fn part(&self, left: u32, top: u32, width: u32, height: u32) -> Option<Image> {
        if left + width > self.width || top + height > self.height {
            return None;
        }
        let pixels = (top..top + height)
            .flat_map(|y| (left..left + width).map(move |x| (x, y)))
            .map(|(x, y)| self.at(x, y))
            .collect();
        Some(Image {
            width,
            height,
            pixels,
        })
    }

    /// Where this image and `other` differ by more than `tolerance` in a channel.
    fn differences(&self, other: &Image, tolerance: u8) -> Differences {
        let mut differences = Differences {
            sizes: ((self.width, self.height), (other.width, other.height)),
            count: 0,
            largest: 0,
            first: None,
        };
        if differences.sizes.0 != differences.sizes.1 {
            differences.count = self.pixels.len().max(other.pixels.len());
            return differences;
        }
        for (index, (&ours, &theirs)) in self.pixels.iter().zip(&other.pixels).enumerate() {
            let off = (0..3)
                .map(|c| ours[c].abs_diff(theirs[c]))
                .max()
                .unwrap_or(0);
            if off > tolerance {
                differences.count += 1;
                differences.largest = differences.largest.max(off);
                let index = u32::try_from(index).expect("an image's pixels fit in u32");
                differences.first.get_or_insert((
                    index % self.width,
                    index / self.width,
                    ours,
                    theirs,
                ));
            }
        }
        differences
    }
}

/// How two images differ, as [`Image::differences`] counts it.
#[derive(Debug)]
struct Differences {
    /// Both images' sizes: when they differ, every pixel counts as off.
    sizes: ((u32, u32), (u32, u32)),
    /// The pixels off by more than the tolerance in a channel.
    count: usize,
    /// The most a channel is off by.
    largest: u8,
    /// The first pixel off, row by row: its place, then its value in each image.
    first: Option<(u32, u32, [u8; 3], [u8; 3])>,
}

impl fmt::Display for Differences {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ((width, height), (other_width, other_height)) = self.sizes;
        if self.sizes.0 != self.sizes.1 {
            return write!(
                f,
                "{width} × {height} pixels against {other_width} × {other_height}"
            );
        }
        write!(
            f,
            "{} pixels of {width} × {height} off, by up to {}",
            self.count, self.largest
        )?;
        if let Some((x, y, ours, theirs)) = self.first {
            write!(f, "; the first at ({x}, {y}): {ours:?} against {theirs:?}")?;
        }
        Ok(())
    }
}

/// A capture's image and where it is on its output: its pixel (`px`, `py`) is the logical point
/// (`x + px × units_per_pixel`, `y + py × units_per_pixel`).
struct Capture {
    image: Image,
    x: f64,
    y: f64,
    units_per_pixel: f64,
}

impl Capture {
    /// The pixels of a window whose corner is at `corner` on its output, in logical units, in
    /// this capture at the output's physical size, of the size of `x11`, X11's image of it: at
    /// XWayland's scale, X11's pixels are the output's, and the window's corner is on a whole
    /// pixel.
    fn window_part(&self, corner: (f64, f64), x11: &Image) -> Result<Image, String> {
        let pixel = |unit: f64, origin: f64| {
            let pixel = (unit - origin) / self.units_per_pixel;
            if (pixel - pixel.round()).abs() > 1e-6 || pixel < 0.0 {
                return Err(format!(
                    "the window's corner is at {unit}, pixel {pixel} of a capture from {origin}"
                ));
            }
            Ok(pixel.round() as u32)
        };
        let (left, top) = (pixel(corner.0, self.x)?, pixel(corner.1, self.y)?);
        self.image
            .part(left, top, x11.width, x11.height)
            .ok_or_else(|| {
                format!(
                    "a capture of {} × {} holds no {} × {} window from ({left}, {top})",
                    self.image.width, self.image.height, x11.width, x11.height
                )
            })
    }
}

/// A seat's X display, as its programs get it: its number, and the cookie of its Xauthority.
struct SeatDisplay {
    number: u32,
    cookie: Vec<u8>,
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

/// What X11 holds of a seat's one managed window: `Xft.dpi`, which Yuna sets, and the window's
/// pixels.
struct X11Window {
    dpi: u32,
    image: Image,
}

/// Connects to `display` as an X11 program of its seat does, through its abstract socket and
/// with its cookie, and reads `Xft.dpi` and the one window of `_NET_CLIENT_LIST`, through
/// `GetImage`. Disconnects before it returns.
fn read_x11_window(display: &SeatDisplay) -> Result<X11Window, String> {
    let x11 = |error: &dyn fmt::Display| format!("X11: {error}");
    let address = SocketAddr::from_abstract_name(format!("/tmp/.X11-unix/X{}", display.number))
        .map_err(|e| x11(&e))?;
    let stream = UnixStream::connect_addr(&address).map_err(|e| x11(&e))?;
    let (stream, _) = DefaultStream::from_unix_stream(stream).map_err(|e| x11(&e))?;
    let connection = RustConnection::connect_to_stream_with_auth_info(
        stream,
        0,
        b"MIT-MAGIC-COOKIE-1".to_vec(),
        display.cookie.clone(),
    )
    .map_err(|e| x11(&e))?;
    let root = connection.setup().roots[0].root;
    let property = |name: AtomEnum, kind: AtomEnum| {
        connection
            .get_property(false, root, name, kind, 0, 1024)
            .map_err(|e| x11(&e))?
            .reply()
            .map_err(|e| x11(&e))
    };
    let resources = property(AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING)?.value;
    let dpi = String::from_utf8_lossy(&resources)
        .lines()
        .find_map(|line| line.strip_prefix("Xft.dpi:")?.trim().parse().ok())
        .ok_or("no Xft.dpi in RESOURCE_MANAGER")?;
    let client_list = connection
        .intern_atom(true, b"_NET_CLIENT_LIST")
        .map_err(|e| x11(&e))?
        .reply()
        .map_err(|e| x11(&e))?
        .atom;
    let windows: Vec<u32> = connection
        .get_property(false, root, client_list, AtomEnum::WINDOW, 0, 64)
        .map_err(|e| x11(&e))?
        .reply()
        .map_err(|e| x11(&e))?
        .value32()
        .ok_or("_NET_CLIENT_LIST is not of windows")?
        .collect();
    let [window] = windows[..] else {
        return Err(format!("X11 manages {} windows, not one", windows.len()));
    };
    let geometry = connection
        .get_geometry(window)
        .map_err(|e| x11(&e))?
        .reply()
        .map_err(|e| x11(&e))?;
    let image = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            window,
            0,
            0,
            geometry.width,
            geometry.height,
            !0,
        )
        .map_err(|e| x11(&e))?
        .reply()
        .map_err(|e| x11(&e))?;
    // Xwayland's pixels of depth 24 or 32 are 32 bits, the least significant byte first: blue,
    // green, red, then nothing or alpha.
    let setup = connection.setup();
    let bits = setup
        .pixmap_formats
        .iter()
        .find(|format| format.depth == image.depth)
        .map(|format| format.bits_per_pixel);
    if bits != Some(32) || setup.image_byte_order != ImageOrder::LSB_FIRST {
        return Err(format!(
            "a window of depth {}, {bits:?} bits a pixel, {:?}: not what this test reads",
            image.depth, setup.image_byte_order
        ));
    }
    let (width, height) = (u32::from(geometry.width), u32::from(geometry.height));
    if image.data.len() != (width * height * 4) as usize {
        return Err(format!(
            "GetImage gave {} bytes for {width} × {height} pixels",
            image.data.len()
        ));
    }
    Ok(X11Window {
        dpi,
        image: Image {
            width,
            height,
            pixels: image
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&[blue, green, red, _]| [red, green, blue])
                .collect(),
        },
    })
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
