//! The scenarios on the real clock: `yuna --headless` run as a process, as an agent runs it, in a
//! runtime folder of its own, and the test clients against its socket. Each step waits for the
//! answers it needs by blocking on the socket, never by polling with roundtrips, whose number
//! would follow the machine's speed: a roundtrip where everything asked is answered at once, a
//! wait (`Client::wait_for_process`) for what comes at a refresh. Times are masked.

use std::fs::{DirBuilder, File};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use wayland_client::protocol::wl_surface::WlSurface;
use yuna::BUTTON_LEFT;

use super::application_environment::shell_variable;
use super::dies_with_test::dies_with_test;
use super::output_client::OutputsClient;
use super::{mark_client, yuna_folder, yuna_run};
use crate::client::Client;
use crate::input_client::{InputClient, InputEvent};
use crate::virtual_input_client::VirtualDriver;
use crate::{BLUE, RED};

/// evdev's keys: left Shift and A.
const KEY_LEFTSHIFT: u32 = 42;
const KEY_A: u32 = 30;
/// The virtual pointer's extent: the agent's seat's one output, in logical units.
const EXTENT: (u32, u32) = (1920, 1080);
/// How long `yuna --headless` may take to print its socket, far past its start on a busy machine.
const START_PATIENCE: Duration = Duration::from_secs(30);

/// A `yuna --headless` process, in a folder of its own in its scenario's ([`yuna_folder`]): its
/// runtime folder, its home and its log.
struct Headless {
    child: Child,
    folder: PathBuf,
    log: PathBuf,
    /// Its Wayland socket.
    socket: PathBuf,
    /// Whether it failed to stop well: its folder is then kept, for its log.
    failed: bool,
}

impl Headless {
    /// Starts `yuna --headless` with `arguments`, and reads the socket it prints.
    fn start(arguments: &[&str]) -> Headless {
        let folder = yuna_folder();
        // Wayland asks that the runtime folder be the user's alone.
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder.join("runtime"))
            .expect("a runtime folder");
        let log = folder.join("yuna.log");
        let mut command = Command::new(env!("CARGO_BIN_EXE_yuna"));
        command
            .arg("--headless")
            .args(arguments)
            // What it draws with and its keymaps, from the shell; a throwaway home, so that Mesa
            // reads none of the user's settings; nothing of the traced clients' own variables.
            .env_clear()
            .env("LD_LIBRARY_PATH", shell_variable("LD_LIBRARY_PATH"))
            .env(
                "__EGL_VENDOR_LIBRARY_FILENAMES",
                shell_variable("__EGL_VENDOR_LIBRARY_FILENAMES"),
            )
            .env("XKB_CONFIG_ROOT", shell_variable("XKB_CONFIG_ROOT"))
            .env("HOME", folder.join("home"))
            .env("XDG_RUNTIME_DIR", folder.join("runtime"))
            .env("YUNA_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(File::create(&log).expect("Yuna's log"));
        dies_with_test(&mut command);
        let mut child = command.spawn().expect("the yuna binary runs");
        // Read on a thread of its own, so that a Yuna that never prints fails the scenario in
        // time.
        let stdout = child.stdout.take().expect("its standard output");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = sender.send(line);
        });
        let mut yuna = Headless {
            child,
            folder,
            log,
            socket: PathBuf::new(),
            failed: false,
        };
        let line = receiver.recv_timeout(START_PATIENCE).unwrap_or_default();
        let Some(name) = line.trim_end().strip_prefix("WAYLAND_DISPLAY=") else {
            yuna.failed = true;
            panic!(
                "yuna printed no socket: {line:?}\n{}",
                yuna_run::log_tail(&yuna.log)
            );
        };
        yuna.socket = yuna.folder.join("runtime").join(name);
        yuna
    }

    /// Stops Yuna with SIGTERM, as an agent does, and requires that it ends well.
    fn stop(mut self) {
        if let Err(why) = yuna_run::stop_on_sigterm(&mut self.child, &self.log) {
            self.failed = true;
            panic!("{why}");
        }
    }
}

impl Drop for Headless {
    /// Ends Yuna if it still runs; its folder goes, unless it failed or the scenario panics.
    fn drop(&mut self) {
        yuna_run::end_run(&mut self.child, &self.folder, self.failed);
    }
}

/// Acknowledges the window's last configure and commits a buffer of the size it gives, with a
/// frame callback and presentation feedback, then waits for both: the frame drawn and presented.
fn redraw(client: &mut Client, window: usize, colour: u32) {
    let configure = client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.ack(window);
    client.attach(window, configure.width, configure.height, colour);
    client.request_frame(window);
    client.request_feedback(window);
    client.commit(window);
    wait_for_frame(client, window);
}

/// Waits for the window's next frame callback and presentation feedback, asked with the last
/// commit.
fn wait_for_frame(client: &mut Client, window: usize) {
    let (frames, feedback) = (
        client.window(window).frames.len(),
        client.window(window).feedback.len(),
    );
    while client.window(window).frames.len() == frames
        || client.window(window).feedback.len() == feedback
    {
        client.wait_for_process();
    }
}

/// Waits for the window's next configure.
fn wait_for_configure(client: &mut Client, window: usize) {
    let configures = client.window(window).configures.len();
    while client.window(window).configures.len() == configures {
        client.wait_for_process();
    }
}

/// Scenario (1) on the real clock, on the default output, 3840 × 2160 at 240 Hz and scale 1.5:
/// a window opens, is configured, and commits its first buffer; it is drawn and presented, and
/// told it is activated. A commit damages part of it. It asks to be maximised, then to be
/// fullscreen and to leave it, each answered at a refresh and drawn at the size told. It closes.
pub fn window() {
    let yuna = Headless::start(&[]);
    mark_client("window");
    let mut client = Client::connect_to_process(&yuna.socket);
    let w = client.create_window();
    client.commit(w);
    client.roundtrip_with_process();
    redraw(&mut client, w, RED);
    if client.window(w).configures.len() < 2 {
        wait_for_configure(&mut client, w);
    }

    client.ack(w);
    client.damage(w, 64, 32);
    client.request_frame(w);
    client.request_feedback(w);
    client.commit(w);
    wait_for_frame(&mut client, w);

    for change in [
        Client::set_maximized as fn(&mut Client, usize),
        |client, w| client.set_fullscreen(w, true),
        |client, w| client.set_fullscreen(w, false),
    ] {
        change(&mut client, w);
        wait_for_configure(&mut client, w);
        redraw(&mut client, w, BLUE);
    }

    client.close(w);
    client.roundtrip_with_process();
    drop(client);
    yuna.stop();
}

/// Waits until the input client is told that its keyboard entered `surface`, in an event from
/// the `since`-th on.
fn wait_for_keyboard_enter(input: &mut InputClient, surface: &WlSurface, since: usize) {
    let entered = |events: &[InputEvent]| {
        events.iter().any(
            |event| matches!(event, InputEvent::KeyboardEnter { surface: on, .. } if on == surface),
        )
    };
    while !entered(&input.state.events[since..]) {
        input.wait_for_process();
    }
}

/// One step of the driver, answered, then what the input client read of it.
fn drive(driver: &mut VirtualDriver, input: &mut InputClient, step: impl Fn(&VirtualDriver)) {
    mark_client("driver");
    step(driver);
    driver.roundtrip_with_process();
    mark_client("input");
    input.roundtrip_with_process();
}

/// Opens a window of the input client and draws it at the size its first configure gives;
/// waits until it is drawn.
fn open_input_window(input: &mut InputClient) -> usize {
    let window = input.client.create_window();
    input.client.commit(window);
    input.roundtrip_with_process();
    let configure = input
        .client
        .window(window)
        .configures
        .last()
        .expect("a configure")
        .clone();
    input.client.ack(window);
    input
        .client
        .attach(window, configure.width, configure.height, RED);
    input.client.request_frame(window);
    input.client.commit(window);
    while input.client.window(window).frames.is_empty() {
        input.wait_for_process();
    }
    window
}

/// Scenario (3) on the real clock: `yuna --headless --agent-seat` with one output of 1920 × 1080
/// at 60 Hz and scale 1, a client of its seat with two windows side by side, each half the
/// output, the second focused, and a driver of the seat's virtual pointer and keyboard. The
/// pointer enters the first window and presses its button there, which gives it the keyboard at
/// the next refresh, and releases it; Shift and A are pressed and released there; the pointer
/// moves to the second window, then off every window. The client reads what each step sent once
/// the driver's step is answered.
pub fn input() {
    let yuna = Headless::start(&["--agent-seat", "--output", "1920x1080@60,1"]);
    mark_client("input");
    let mut input = InputClient::connect_to_process(&yuna.socket, "agent-2");
    let first = open_input_window(&mut input);
    let second = open_input_window(&mut input);
    let (first, second) = (
        input.client.window(first).surface.clone(),
        input.client.window(second).surface.clone(),
    );
    wait_for_keyboard_enter(&mut input, &second, 0);

    mark_client("driver");
    let mut driver = VirtualDriver::connect_to_process(&yuna.socket);
    let pointer = driver.add_pointer(None);
    let keyboard = driver.add_keyboard("agent-2", "us");
    driver.roundtrip_with_process();

    let (driver, input) = (&mut driver, &mut input);
    drive(driver, input, |driver| {
        driver.move_to(pointer, (100, 100), EXTENT)
    });
    drive(driver, input, |driver| {
        driver.move_to(pointer, (120, 110), EXTENT)
    });
    let since = input.state.events.len();
    drive(driver, input, |driver| {
        driver.button(pointer, BUTTON_LEFT, true)
    });
    // The press's window takes the keyboard at the next refresh, in the press's own round when
    // the output is idle: either way, before the release.
    wait_for_keyboard_enter(input, &first, since);
    drive(driver, input, |driver| {
        driver.button(pointer, BUTTON_LEFT, false)
    });
    drive(driver, input, |driver| {
        driver.key(keyboard, KEY_LEFTSHIFT, true)
    });
    drive(driver, input, |driver| driver.modifiers(keyboard, 1, 0, 0));
    drive(driver, input, |driver| driver.key(keyboard, KEY_A, true));
    drive(driver, input, |driver| driver.key(keyboard, KEY_A, false));
    drive(driver, input, |driver| {
        driver.key(keyboard, KEY_LEFTSHIFT, false)
    });
    drive(driver, input, |driver| driver.modifiers(keyboard, 0, 0, 0));
    drive(driver, input, |driver| {
        driver.move_to(pointer, (1100, 100), EXTENT)
    });
    drive(driver, input, |driver| {
        driver.move_to(pointer, (1919, 1079), EXTENT)
    });
    yuna.stop();
}

/// Scenario (6) on the real clock: `yuna --headless` with two outputs, 1920 × 1080 at 60 Hz and
/// scale 1, and 1280 × 720 at 60 Hz and scale 2, laid out left to right; a client binds every
/// `wl_output` and its `xdg-output`. `yuna --headless` adds or removes no output while it runs:
/// that part is the manual clock's alone.
pub fn outputs() {
    let yuna = Headless::start(&["--output", "1920x1080@60,1", "--output", "1280x720@60,2"]);
    mark_client("outputs");
    let outputs = OutputsClient::connect_to_process(&yuna.socket);
    drop(outputs);
    yuna.stop();
}
