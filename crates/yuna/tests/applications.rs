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
//! SlimeEngine's examples run in ignored tests, which its weekly job runs: `window` draws its
//! frames and leaves, `editor` rests with its window open, and an agent types and clicks in
//! `interface` and checks it by capture, M1.5's done-when
//! ([ADR-0043](../../../docs/adr/0043-text-input-and-the-rest-of-slimeengines-examples.md)).
//!
//! What makes an application draw again is pinned, so that only Yuna's behaviour decides the
//! result. Yuna and each application get an environment of their own, nothing of the caller's:
//! no `DISPLAY` and no D-Bus, so that they reach neither the user's desktop nor an accessibility
//! bus; a `PATH` of the store alone; throwaway home and XDG folders, the system's XDG folders
//! included, so that no settings of the machine or the user are read; DejaVu alone, through a
//! fontconfig file that includes none of the machine's; Mesa on the CPU, lavapipe alone for
//! Vulkan, without the machine's implicit layers; cursors that never blink; GTK's animations off.
//! What still reads the machine, measured with strace on NixOS: Mesa reads `/etc/drirc` where a
//! machine has one, and lists `/dev/dri` without opening a device; Vulkan's loader reads the
//! machine's list of layers and loads none; the fontconfig of Alacritty and Qt looks for two
//! NixOS files in `/etc/fonts`, `local.conf` and `70-no-bitmaps-except-emoji.conf`; lavapipe, in
//! mpv, opens `/dev/udmabuf`; mpv reads `/etc/localtime`, and looks for each of its libraries in
//! the machine's `/run/opengl-driver/lib` first, where the machine measured has none of them.
//!
//! Every program comes from the development shell (`flake.nix`). Each process the test starts is
//! killed if the test's thread ends first, so that a test killed or out of time leaves nothing.

#[path = "common/application_environment.rs"]
mod application_environment;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, DirBuilder, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use application_environment::{application_environment, shell_variable};
use serde_json::{Value, json};
use smithay::reexports::rustix::io::Errno;
use smithay::reexports::rustix::process::{
    Pid, Signal, getppid, kill_process, set_parent_process_death_signal,
};

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

/// Qt's QML runner on its Wayland platform and its OpenGL renderer, the default pinned.
fn qml(yuna: &Yuna) -> Command {
    let mut command = application(yuna, "qml");
    command
        .env("QT_QPA_PLATFORM", "wayland")
        .env("QT_PLUGIN_PATH", shell_variable("YUNA_QT_PLUGINS"))
        .env("QML_IMPORT_PATH", shell_variable("YUNA_QML_IMPORTS"))
        .env("QSG_RHI_BACKEND", "opengl");
    command
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

/// Makes `command`'s process end with SIGKILL when the thread that starts it ends, so that a test
/// killed, or stopped at CI's time limit, leaves no Yuna or application running. Every run drops
/// its processes before its thread ends.
fn dies_with_test(command: &mut Command) {
    let parent = std::process::id();
    // SAFETY: the closure runs in the child between fork and exec, where only async-signal-safe
    // calls are sound: it makes two system calls, `prctl` and `getppid`, and allocates nothing,
    // an `Errno` turning into an `io::Error` without a box.
    unsafe {
        command.pre_exec(move || {
            set_parent_process_death_signal(Some(Signal::KILL))?;
            // The parent may have ended before the signal was set: then it never comes.
            if u32::try_from(Pid::as_raw(getppid())) != Ok(parent) {
                return Err(Errno::SRCH.into());
            }
            Ok(())
        });
    }
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
            // What it draws with, from the shell; a throwaway home, so that Mesa reads none of
            // the user's settings.
            .env_clear()
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
