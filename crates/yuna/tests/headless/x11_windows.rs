//! X11 windows in Yuna's layout, as `xdg_toplevel`s are: placed, sized, focused, typed into,
//! clicked, closed, made fullscreen, kept in an emulated mode, across an output's change of mode
//! too, hidden, captured, slid by the motion, and drawn sharp at XWayland's scale and the right
//! size on every output
//! ([ADR-0066](../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md),
//! [ADR-0068](../../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md),
//! [ADR-0069](../../../../docs/adr/0069-fullscreen-x11-games-gamescope-and-xwaylands-emulated-modes-kept.md)).
//!
//! Each story runs the development shell's Xwayland for an agent's seat, and the X11 test client
//! (`x11_client.rs`). The seat's outputs here are 2880 × 1620 at 1.5 and 1920 × 1080 at 1: both
//! 1920 × 1080 logical units, so that a window's column is (1920 − 3 × 16) / 2 = 936 units wide
//! and 1080 − 2 × 16 = 1048 tall on either, at (16, 16) from its output's corner. XWayland's
//! scale S is the larger, 1.5: an X11 window there is 1404 × 1572 X11 pixels. The stories run on
//! the real clock, but for the motion's, which steps the manual clock against `yuna-motion`'s
//! reference.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt as _, Window};
use yuna::yuna_layout::geometry::Rect;
use yuna::yuna_layout::{Direction, OutputId, SeatId, WindowId};
use yuna::yuna_motion::{Change, Motion, Settings};
use yuna::{
    BUTTON_LEFT, CaptureSize, CaptureTarget, CapturedFrame, ClockMode, Compositor, Config,
    MAX_WINDOWS_PER_CLIENT, OutputMode, WindowDescription, WindowShell, X11_PING_TIMEOUT,
    X11_SYNC_TIMEOUT, X11Display, XwaylandConfig, axis_taps,
};

use crate::x11_client::{
    Geometry, Image, PATIENCE, WindowSpec, X11Client, connect, step, step_pumping,
};
use crate::{LEAD, boundary, first_boundary_at_or_after};

/// The output at XWayland's scale: 1920 × 1080 logical units at 1.5.
pub(crate) fn at_1_5() -> OutputMode {
    OutputMode::new(2880, 1620, 60.0, 1.5).expect("a valid mode")
}

/// The output below it: 1920 × 1080 at 1.
pub(crate) fn at_1() -> OutputMode {
    OutputMode::new(1920, 1080, 60.0, 1.0).expect("a valid mode")
}

/// A window's place on either output, from the output's corner, in logical units.
pub(crate) const TILE: Rect = Rect {
    x: 16.0,
    y: 16.0,
    w: 936.0,
    h: 1048.0,
};

/// The tile in X11 pixels at S = 1.5: 1404 × 1572, its corner at (24, 24) from the output's.
const X11_TILE: (u32, u32) = (1404, 1572);

/// A story's folder, for its seat's socket and cookie, removed when it drops.
pub(crate) struct Folder(pub PathBuf);

impl Folder {
    pub(crate) fn new(story: &str) -> Folder {
        let path = std::env::temp_dir().join(format!("yuna-x11-{story}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("the story's folder");
        Folder(path)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An agent's seat with an X display, in a compositor of its own.
pub(crate) struct X11Seat {
    pub compositor: Compositor,
    pub seat: SeatId,
    pub outputs: Vec<OutputId>,
    pub display: X11Display,
    _folder: Folder,
}

impl X11Seat {
    /// A seat of `modes`, laid out left to right, on `clock`, its motion as `animations` says.
    pub(crate) fn new(
        story: &str,
        modes: &[OutputMode],
        clock: ClockMode,
        animations: Settings,
    ) -> X11Seat {
        let folder = Folder::new(story);
        let mut compositor = Compositor::new(Config {
            outputs: Vec::new(),
            clock,
            listen: false,
            animations,
            xwayland: Some(XwaylandConfig::default()),
            ..Config::default()
        })
        .expect("the compositor starts");
        let seat = compositor.add_seat().expect("a seat");
        let outputs = modes
            .iter()
            .map(|&mode| compositor.add_output(seat, mode).expect("an output"))
            .collect();
        compositor
            .listen_for_seat(seat, folder.0.join("seat"))
            .expect("the seat listens");
        let display = compositor.x11_display(seat).expect("the seat's display");
        X11Seat {
            compositor,
            seat,
            outputs,
            display,
            _folder: folder,
        }
    }

    /// The seat's outputs at 1.5 then 1, on the real clock, without motion.
    pub(crate) fn at_both(story: &str) -> X11Seat {
        X11Seat::new(story, &[at_1_5(), at_1()], ClockMode::Real, off())
    }

    pub(crate) fn client(&mut self) -> X11Client {
        connect(&mut self.compositor, &self.display)
    }

    pub(crate) fn output_rect(&self, output: OutputId) -> Rect {
        self.compositor
            .output_descriptions()
            .into_iter()
            .find(|description| description.id == output)
            .expect("the output")
            .rect
    }

    /// Shows the next window that opens on `output`.
    pub(crate) fn open_on(&mut self, output: OutputId) {
        let seat = self.seat;
        self.compositor
            .change_layout(|layout| layout.focus_output(seat, output))
            .expect("the output takes the focus");
    }

    /// Moves `seat`'s focused window to a workspace of `output`'s own.
    pub(crate) fn move_to(&mut self, output: OutputId) {
        let seat = self.seat;
        let name = format!("on-{}", output.0);
        self.compositor
            .change_layout(|layout| {
                let _ = layout.add_workspace(seat, &name, Some(output));
                layout.move_to_workspace(seat, &name)
            })
            .expect("the window moves");
    }
}

pub(crate) fn off() -> Settings {
    Settings {
        off: true,
        ..Settings::default()
    }
}

/// One round of the compositor's loop, a short wait for its sources; on the manual clock, the
/// clock moves toward its next deadline, a few milliseconds at most, so that frames come as
/// Xwayland's commits need them.
pub(crate) fn pump(compositor: &mut Compositor) {
    compositor
        .dispatch(Some(Duration::from_millis(2)))
        .expect("Yuna dispatches");
    if compositor.clock_mode() == ClockMode::Manual
        && let Some(at) = compositor.next_deadline()
    {
        let by = at
            .saturating_sub(compositor.now())
            .min(Duration::from_millis(5));
        compositor.advance(by).expect("the manual clock moves");
    }
}

/// Runs the compositor's loop until `done` gives something, for up to [`PATIENCE`].
pub(crate) fn wait<T>(
    compositor: &mut Compositor,
    what: &str,
    mut done: impl FnMut(&mut Compositor) -> Option<T>,
) -> T {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(value) = done(compositor) {
            return value;
        }
        assert!(Instant::now() < deadline, "{what}: not within {PATIENCE:?}");
        pump(compositor);
    }
}

/// Runs the compositor's loop until, for half a second, the seat's Xwayland has committed nothing
/// and no deadline has been armed. It returns with none armed.
pub(crate) fn rest(seat: &mut X11Seat) {
    let deadline = Instant::now() + PATIENCE;
    let mut commits = seat.compositor.seat_commits(seat.seat);
    let mut quiet_since = Instant::now();
    loop {
        assert!(Instant::now() < deadline, "the seat never rests");
        pump(&mut seat.compositor);
        let now = seat.compositor.seat_commits(seat.seat);
        if now != commits || seat.compositor.next_deadline().is_some() {
            commits = now;
            quiet_since = Instant::now();
        } else if quiet_since.elapsed() >= Duration::from_millis(500) {
            return;
        }
    }
}

/// The window titled `title`, once the layout holds it and its output draws it.
pub(crate) fn window_titled(compositor: &mut Compositor, title: &str) -> WindowDescription {
    wait(
        compositor,
        &format!("the window {title:?} opens"),
        |compositor| {
            compositor
                .windows()
                .into_iter()
                .find(|window| window.title.as_deref() == Some(title) && window.rect.is_some())
        },
    )
}

/// Opens a window as `spec` says and draws [`Image::sharp`] into it at its size; once Yuna
/// shows it, draws again if Yuna resized it. Returns the window, its description and the image
/// drawn.
pub(crate) fn open_drawn(
    compositor: &mut Compositor,
    client: X11Client,
    spec: &WindowSpec,
) -> (X11Client, Window, WindowDescription, Image) {
    let opening = spec.clone();
    let (client, (window, image)) = step(compositor, client, move |client| {
        let window = client.open(&opening)?;
        let geometry = client.geometry(window)?;
        let image = Image::sharp(geometry.width, geometry.height);
        client.draw(window, &image)?;
        Ok((window, image))
    });
    let described = window_titled(compositor, &spec.title);
    let (client, image) = redraw_if_resized(compositor, client, window, image);
    (client, window, described, image)
}

/// Draws [`Image::sharp`] into `window` again if its size is no longer `image`'s.
fn redraw_if_resized(
    compositor: &mut Compositor,
    client: X11Client,
    window: Window,
    image: Image,
) -> (X11Client, Image) {
    step(compositor, client, move |client| {
        let geometry = client.geometry(window)?;
        if (geometry.width, geometry.height) == (image.width, image.height) {
            return Ok(image);
        }
        let image = Image::sharp(geometry.width, geometry.height);
        client.draw(window, &image)?;
        Ok(image)
    })
}

/// `window`'s place, once it is `wanted`.
pub(crate) fn geometry_becomes(
    compositor: &mut Compositor,
    client: X11Client,
    window: Window,
    wanted: Geometry,
) -> X11Client {
    step(compositor, client, move |client| {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let geometry = client.geometry(window)?;
            if geometry == wanted {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(format!("the window stays at {geometry:?}, not {wanted:?}"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    })
    .0
}

/// The X11 rectangle of a tile on `output`, of rectangle `output_rect`, at S = 1.5.
fn x11_tile(output_rect: Rect) -> Geometry {
    Geometry {
        x: ((output_rect.x + TILE.x) * 1.5).round() as i32,
        y: ((output_rect.y + TILE.y) * 1.5).round() as i32,
        width: X11_TILE.0,
        height: X11_TILE.1,
    }
}

/// Captures `target` at `size` again and again, each the newest frame, until `good` takes one,
/// for up to [`PATIENCE`]: Xwayland's last buffer reaches Yuna in its own time. Returns the
/// last.
pub(crate) fn capture_until(
    compositor: &mut Compositor,
    seat: SeatId,
    target: CaptureTarget,
    size: CaptureSize,
    good: impl Fn(&CapturedFrame) -> bool,
) -> CapturedFrame {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let ticket = compositor.capture(seat, target, size).expect("a capture");
        let frame = wait(compositor, "a capture", |compositor| {
            compositor
                .take_captures()
                .into_iter()
                .find(|(taken, _)| *taken == ticket)
                .map(|(_, frame)| frame)
        })
        .expect("a frame captured");
        if good(&frame) || Instant::now() > deadline {
            return frame;
        }
        pump(compositor);
    }
}

/// Whether `frame`'s image is `image`, pixel for pixel.
pub(crate) fn is_exactly(frame: &CapturedFrame, image: &Image) -> bool {
    frame.mapping.width == image.width
        && frame.mapping.height == image.height
        && frame.image() == image.pixels.concat()
}

/// What the area filter draws of `image` at `width` × `height` pixels: each pixel the average of
/// the texels under it, with the weights of the captures' encoder (`axis_taps`), rounded. R, G,
/// B, row by row.
pub(crate) fn area_reference(image: &Image, width: u32, height: u32) -> Vec<u8> {
    let taps = |count: u32, texels: u32| {
        axis_taps(
            0.0,
            count,
            f64::from(texels) / f64::from(count),
            0,
            texels as usize,
        )
    };
    let columns = taps(width, image.width);
    let rows = taps(height, image.height);
    let mut reference = Vec::with_capacity((width * height * 3) as usize);
    for (first_row, row_weights) in &rows {
        for (first_column, column_weights) in &columns {
            let mut sum = [0.0f64; 3];
            for (dy, row_weight) in row_weights.iter().enumerate() {
                for (dx, column_weight) in column_weights.iter().enumerate() {
                    let texel = image.at((first_column + dx) as u32, (first_row + dy) as u32);
                    let weight = f64::from(*row_weight) * f64::from(*column_weight);
                    for (total, value) in sum.iter_mut().zip(texel) {
                        *total += weight * f64::from(value);
                    }
                }
            }
            reference.extend(sum.map(|total| total.round().clamp(0.0, 255.0) as u8));
        }
    }
    reference
}

/// The largest difference of `drawn` from `reference` on any channel.
pub(crate) fn largest_difference(drawn: &[u8], reference: &[u8]) -> u8 {
    assert_eq!(drawn.len(), reference.len());
    drawn
        .iter()
        .zip(reference)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0)
}

/// Asserts `frame`, a logical capture of a window on the output at 1, is `image` area-filtered to
/// the window's logical size, each channel within 1 of the reference.
fn assert_area_filtered(frame: &CapturedFrame, image: &Image) {
    let (width, height) = (TILE.w as u32, TILE.h as u32);
    assert_eq!(
        (frame.mapping.width, frame.mapping.height),
        (width, height),
        "the capture is the window's logical size"
    );
    let difference = largest_difference(&frame.image(), &area_reference(image, width, height));
    assert!(
        difference <= 1,
        "a pixel differs from the reference by {difference}"
    );
}

/// Whether `frame` is within 1 of the area-filtered `image` at the tile's size.
fn is_area_filtered(frame: &CapturedFrame, image: &Image) -> bool {
    let (width, height) = (TILE.w as u32, TILE.h as u32);
    (frame.mapping.width, frame.mapping.height) == (width, height)
        && largest_difference(&frame.image(), &area_reference(image, width, height)) <= 1
}

#[test]
fn an_x11_window_opens_tiled_and_focused() {
    let mut seat = X11Seat::new("opens", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    assert_eq!(client.dpi, 144, "Xft.dpi is 96 × S");
    assert_eq!(client.scale(), 1.5);
    let spec = WindowSpec::named("opens");
    let (client, window, described, _) = open_drawn(&mut seat.compositor, client, &spec);
    let output = seat.output_rect(seat.outputs[0]);
    // Its X11 size is the tile × S, at its corner × S.
    let client = geometry_becomes(&mut seat.compositor, client, window, x11_tile(output));
    assert_eq!(described.shell, WindowShell::X11);
    assert_eq!(described.app_id.as_deref(), Some("X11Test"));
    assert_eq!(described.pid, Some(std::process::id() as i32));
    assert_eq!(described.seat, seat.seat);
    assert_eq!(described.place, Some(TILE));
    assert!(described.focused);
    assert_eq!(
        seat.compositor.keyboard_window(seat.seat),
        Some(described.id),
        "wl_keyboard is on its surface"
    );
    // The X input focus names it.
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        let deadline = Instant::now() + PATIENCE;
        while client.input_focus()? != window {
            if Instant::now() > deadline {
                return Err("the X input focus is not on the window".to_owned());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    });
    // Its own request for another place and size is answered with its place: the layout
    // decides, and ICCCM's synthetic ConfigureNotify says so.
    let (_client, answer) = step(&mut seat.compositor, client, move |client| {
        client.take_events()?;
        client
            .connection
            .configure_window(
                window,
                &ConfigureWindowAux::new().x(5).y(5).width(300).height(200),
            )
            .map_err(|e| e.to_string())?;
        client.connection.flush().map_err(|e| e.to_string())?;
        let answer = client.wait_for("the answer", |event| {
            matches!(event, Event::ConfigureNotify(configured)
                if configured.window == window && configured.response_type & 0x80 != 0)
        })?;
        let geometry = client.geometry(window)?;
        match answer {
            Event::ConfigureNotify(answer) => Ok((
                (
                    i32::from(answer.x),
                    i32::from(answer.y),
                    u32::from(answer.width),
                    u32::from(answer.height),
                ),
                geometry,
            )),
            _ => unreachable!("a ConfigureNotify"),
        }
    });
    let tile = x11_tile(output);
    assert_eq!(answer.0, (tile.x, tile.y, tile.width, tile.height));
    assert_eq!(answer.1, tile);
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
}

#[test]
fn an_x11_window_is_sharp_at_the_x11_scale() {
    let mut seat = X11Seat::at_both("sharp");
    let client = seat.client();
    let (_client, _, described, image) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("sharp"));
    assert_eq!((image.width, image.height), X11_TILE);
    let frame = capture_until(
        &mut seat.compositor,
        seat.seat,
        CaptureTarget::Window(described.id),
        CaptureSize::Physical,
        |frame| is_exactly(frame, &image),
    );
    assert!(
        is_exactly(&frame, &image),
        "the capture of the window at 1.5 is the pixels drawn, exactly"
    );
}

#[test]
fn an_x11_window_on_the_output_at_1_is_area_filtered_to_its_size() {
    let mut seat = X11Seat::at_both("area");
    let at_one = seat.outputs[1];
    seat.open_on(at_one);
    let client = seat.client();
    let (_client, _, described, image) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("area"));
    assert_eq!(described.output, Some(at_one));
    assert_eq!((image.width, image.height), X11_TILE);
    let frame = capture_until(
        &mut seat.compositor,
        seat.seat,
        CaptureTarget::Window(described.id),
        CaptureSize::Logical,
        |frame| is_area_filtered(frame, &image),
    );
    assert_area_filtered(&frame, &image);
}

#[test]
fn an_x11_window_moved_between_outputs_keeps_its_size() {
    let mut seat = X11Seat::at_both("moved");
    let (at_one_and_a_half, at_one) = (seat.outputs[0], seat.outputs[1]);
    let client = seat.client();
    let (client, window, described, image) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("moved"));
    let id = described.id;
    let frame = capture_until(
        &mut seat.compositor,
        seat.seat,
        CaptureTarget::Window(id),
        CaptureSize::Physical,
        |frame| is_exactly(frame, &image),
    );
    assert!(is_exactly(&frame, &image), "sharp at 1.5");

    seat.move_to(at_one);
    let moved = wait(&mut seat.compositor, "the window moves", |compositor| {
        compositor
            .windows()
            .into_iter()
            .find(|window| window.id == id && window.output == Some(at_one))
    });
    let output = seat.output_rect(at_one);
    assert_eq!(
        moved.place.map(|place| (place.w, place.h)),
        Some((TILE.w, TILE.h)),
        "the same logical size on both"
    );
    // Moved, not resized: its X11 size is the same, its client draws nothing again.
    let client = geometry_becomes(&mut seat.compositor, client, window, x11_tile(output));
    let frame = capture_until(
        &mut seat.compositor,
        seat.seat,
        CaptureTarget::Window(id),
        CaptureSize::Logical,
        |frame| is_area_filtered(frame, &image),
    );
    assert_area_filtered(&frame, &image);

    // And back, sharp again.
    seat.move_to(at_one_and_a_half);
    let output = seat.output_rect(at_one_and_a_half);
    let _client = geometry_becomes(&mut seat.compositor, client, window, x11_tile(output));
    let frame = capture_until(
        &mut seat.compositor,
        seat.seat,
        CaptureTarget::Window(id),
        CaptureSize::Physical,
        |frame| is_exactly(frame, &image),
    );
    assert!(is_exactly(&frame, &image), "sharp at 1.5 again");
}

/// The X11 pixel `ButtonPress` names after a click at (`x`, `y`) from `output`'s corner, in
/// logical units, the window at the tile: the point's distance from the window's corner, times
/// S, whole pixels.
pub(crate) fn click_at(seat: &mut X11Seat, output: OutputId, x: f64, y: f64) {
    seat.compositor
        .pointer_move(seat.seat, Some(output), x, y)
        .expect("the pointer moves");
    seat.compositor
        .pointer_button(seat.seat, BUTTON_LEFT, true)
        .expect("pressed");
    seat.compositor
        .pointer_button(seat.seat, BUTTON_LEFT, false)
        .expect("released");
}

pub(crate) fn button_press_at(
    compositor: &mut Compositor,
    client: X11Client,
    window: Window,
) -> (X11Client, (i16, i16)) {
    step(compositor, client, move |client| {
        let event = client.wait_for(
            "a button press",
            |event| matches!(event, Event::ButtonPress(press) if press.event == window),
        )?;
        match event {
            Event::ButtonPress(press) => {
                assert_eq!(press.detail, 1, "the left button");
                Ok((press.event_x, press.event_y))
            }
            _ => unreachable!("a button press"),
        }
    })
}

#[test]
fn typing_and_clicking_reach_an_x11_window() {
    let mut seat = X11Seat::at_both("input");
    let (at_one_and_a_half, at_one) = (seat.outputs[0], seat.outputs[1]);
    let client = seat.client();
    let (client, window, described, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("input"));
    let output = seat.output_rect(at_one_and_a_half);
    let client = geometry_becomes(&mut seat.compositor, client, window, x11_tile(output));

    // 100.25 × 50.25 units into the window at 1.5: 150.375 × 75.375 X11 pixels, in pixel
    // (150, 75).
    click_at(
        &mut seat,
        at_one_and_a_half,
        TILE.x + 100.25,
        TILE.y + 50.25,
    );
    let (client, at) = button_press_at(&mut seat.compositor, client, window);
    assert_eq!(at, (150, 75), "at 1.5");
    seat.compositor.tap_keys(seat.seat, "a").expect("typed");
    let (client, key) = step(&mut seat.compositor, client, move |client| {
        match client.wait_for(
            "a key press",
            |event| matches!(event, Event::KeyPress(press) if press.event == window),
        )? {
            Event::KeyPress(press) => Ok(press.detail),
            _ => unreachable!("a key press"),
        }
    });
    // XKB's keycode of `a` on the `us` layout, which X11's is.
    assert_eq!(key, 38);

    // On the output at 1, the same window, the same X11 pixels, at another point too:
    // 300.5 × 200.75 units, 450.75 × 301.125 X11 pixels.
    seat.move_to(at_one);
    let output = seat.output_rect(at_one);
    let client = geometry_becomes(&mut seat.compositor, client, window, x11_tile(output));
    assert_eq!(
        seat.compositor.keyboard_window(seat.seat),
        Some(described.id)
    );
    click_at(&mut seat, at_one, TILE.x + 100.25, TILE.y + 50.25);
    let (client, at) = button_press_at(&mut seat.compositor, client, window);
    assert_eq!(at, (150, 75), "at 1");
    click_at(&mut seat, at_one, TILE.x + 300.5, TILE.y + 200.75);
    let (_client, at) = button_press_at(&mut seat.compositor, client, window);
    assert_eq!(at, (450, 301), "at 1, further in");
}

/// The windows of a frame that show on an output, with their rectangles, bottom to top.
fn shown(drawn: &[yuna::yuna_motion::Drawn], output: Rect) -> Vec<(WindowId, Rect)> {
    drawn
        .iter()
        .filter(|drawn| drawn.rect.overlaps(&output))
        .map(|drawn| (drawn.window, drawn.rect))
        .collect()
}

#[test]
fn an_x11_window_slides_with_the_motion() {
    let spring = yuna::yuna_motion::SpringParams::new(1.0, 800.0).expect("a valid spring");
    let settings = Settings {
        off: false,
        camera: spring,
        windows: spring,
        ..Settings::default()
    };
    // 240 Hz, as the boundaries `boundary` counts.
    let mode = OutputMode::new(2880, 1620, 240.0, 1.5).expect("a valid mode");
    let mut seat = X11Seat::new("motion", &[mode], ClockMode::Manual, settings);
    let start = seat.compositor.now();
    let output = seat.outputs[0];
    let output_rect = seat.output_rect(output);
    let client = seat.client();
    let (client, left, _, _) = open_drawn(&mut seat.compositor, client, &WindowSpec::named("left"));
    let (client, right, _, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("right"));
    // Everything drawn and answered: the motions of the openings end, Xwayland commits nothing.
    let mut client = client;
    loop {
        crate::motion::settle(&mut seat.compositor, |_| {});
        let commits = seat.compositor.counters().commits;
        let settled = Instant::now() + Duration::from_millis(300);
        while Instant::now() < settled {
            seat.compositor
                .dispatch(Some(Duration::from_millis(5)))
                .expect("Yuna dispatches");
        }
        if seat.compositor.counters().commits == commits
            && seat.compositor.next_deadline().is_none()
        {
            break;
        }
    }
    // The events of the openings are behind.
    client = step(&mut seat.compositor, client, |client| {
        client.take_events().map(|_| ())
    })
    .0;
    let mut reference = Motion::new(settings);
    reference.update(
        seat.compositor.layout(),
        seat.compositor.now(),
        Change::Instant,
    );
    assert_eq!(
        seat.compositor.drawn(output),
        reference.frame(output, seat.compositor.now()).windows
    );

    // The right column moves left, 0.1 ms after a boundary of the idle output: both windows
    // slide past each other.
    let k = first_boundary_at_or_after(start, seat.compositor.now()) + 1;
    let operation = boundary(start, k) + Duration::from_micros(100);
    seat.compositor
        .advance(operation - seat.compositor.now())
        .expect("the clock moves");
    let id = seat.seat;
    seat.compositor
        .change_layout(|layout| layout.move_focused(id, Direction::Left))
        .expect("the column moves");
    reference.update(seat.compositor.layout(), operation, Change::Animated);

    // Each refresh's frame is the reference's at the boundary it aims at: the first, due at
    // once, the next boundary; each later one, a lead before its boundary, that boundary.
    let mut due = seat.compositor.now();
    let mut k = first_boundary_at_or_after(start, due + LEAD);
    let mut frames = 0;
    let mut moving = 0;
    loop {
        seat.compositor
            .advance(due.saturating_sub(seat.compositor.now()))
            .expect("the clock moves");
        let sample = boundary(start, k);
        let expected = reference.frame(output, sample).windows;
        assert_eq!(
            seat.compositor.drawn(output),
            expected.as_slice(),
            "boundary {k}"
        );
        frames += 1;
        if shown(&expected, output_rect)
            != shown(&reference.frame(output, operation).windows, output_rect)
        {
            moving += 1;
        }
        if reference.end(output, sample).is_none() {
            break;
        }
        assert!(frames < 1_000, "the motion never ends");
        seat.compositor
            .advance(sample - seat.compositor.now())
            .expect("the clock moves");
        due = boundary(start, k + 1) - LEAD;
        k += 1;
    }
    assert!(moving > 10, "the windows slid over {moving} frames");

    // Each X11 window was configured once, at its new place: its corner × S.
    let places: Vec<Rect> = seat
        .compositor
        .layout()
        .arrange(output)
        .expect("arranged")
        .iter()
        .map(|placed| placed.rect)
        .collect();
    let (_client, configured) = step(&mut seat.compositor, client, move |client| {
        let events = client.take_events()?;
        let real = |window: Window| -> Vec<(i16, i16, u16, u16)> {
            events
                .iter()
                .filter_map(|event| match event {
                    // A synthetic one, ICCCM's, has the top bit of its type set.
                    Event::ConfigureNotify(configured)
                        if configured.window == window && configured.response_type & 0x80 == 0 =>
                    {
                        Some((
                            configured.x,
                            configured.y,
                            configured.width,
                            configured.height,
                        ))
                    }
                    _ => None,
                })
                .collect()
        };
        Ok((real(left), real(right)))
    });
    let x11 = |rect: Rect| {
        (
            (rect.x * 1.5).round() as i16,
            (rect.y * 1.5).round() as i16,
            X11_TILE.0 as u16,
            X11_TILE.1 as u16,
        )
    };
    // The right window is now the left column, and the left one the right.
    assert_eq!(configured.1, vec![x11(places[0])], "the right window, once");
    assert_eq!(configured.0, vec![x11(places[1])], "the left window, once");

    // With X11 windows open and nothing new, Yuna rests.
    crate::run_armed_deadlines(&mut seat.compositor);
    crate::assert_rests(&mut seat.compositor);
}

#[test]
fn closing_an_x11_window_sends_wm_delete_window() {
    let mut seat = X11Seat::new("close", &[at_1()], ClockMode::Real, off());
    let client = seat.client();
    let (client, window, described, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("polite"));
    seat.compositor
        .close_window(described.id)
        .expect("asked to close");
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        let atoms = client.atoms;
        client.wait_for("WM_DELETE_WINDOW", |event| {
            matches!(event, Event::ClientMessage(message)
                if message.window == window
                    && message.type_ == atoms.WM_PROTOCOLS
                    && message.data.as_data32()[0] == atoms.WM_DELETE_WINDOW)
        })?;
        Ok(())
    });
    // Asked, not closed: the window stays until its client closes it.
    assert!(
        seat.compositor
            .windows()
            .iter()
            .any(|listed| listed.id == described.id)
    );
    step(&mut seat.compositor, client, move |client| {
        client
            .connection
            .destroy_window(window)
            .map_err(|e| e.to_string())?;
        client.sync()
    });
    wait(&mut seat.compositor, "the window closes", |compositor| {
        (!compositor
            .windows()
            .iter()
            .any(|listed| listed.id == described.id))
        .then_some(())
    });

    // A window that does not take WM_DELETE_WINDOW has its client ended.
    let rude = seat.client();
    let spec = WindowSpec {
        deletes: false,
        ..WindowSpec::named("rude")
    };
    let (rude, _, described, _) = open_drawn(&mut seat.compositor, rude, &spec);
    seat.compositor
        .close_window(described.id)
        .expect("asked to close");
    step(&mut seat.compositor, rude, |client| {
        let deadline = Instant::now() + PATIENCE;
        while client.sync().is_ok() {
            if Instant::now() > deadline {
                return Err("the client was not ended".to_owned());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    });
    wait(
        &mut seat.compositor,
        "the ended client's window closes",
        |compositor| compositor.windows().is_empty().then_some(()),
    );
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
}

#[test]
fn an_x11_window_asks_fullscreen() {
    let mut seat = X11Seat::new("fullscreen", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, window, described, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("full"));
    let output = seat.output_rect(seat.outputs[0]);
    let fullscreen_atom = client.atoms._NET_WM_STATE_FULLSCREEN;
    let client = step(&mut seat.compositor, client, move |client| {
        client.ask_fullscreen(window, true)
    })
    .0;
    wait(
        &mut seat.compositor,
        "the window is fullscreen",
        |compositor| {
            compositor
                .windows()
                .into_iter()
                .find(|listed| listed.id == described.id && listed.fullscreen)
        },
    );
    // The whole output, × S, and the state that says so.
    let whole = Geometry {
        x: 0,
        y: 0,
        width: 2880,
        height: 1620,
    };
    let client = geometry_becomes(&mut seat.compositor, client, window, whole);
    let (client, state) = step(&mut seat.compositor, client, move |client| {
        client.net_wm_state(window)
    });
    assert!(state.contains(&fullscreen_atom), "{state:?}");

    let client = step(&mut seat.compositor, client, move |client| {
        client.ask_fullscreen(window, false)
    })
    .0;
    let client = geometry_becomes(&mut seat.compositor, client, window, x11_tile(output));
    let (_, state) = step(&mut seat.compositor, client, move |client| {
        client.net_wm_state(window)
    });
    assert!(!state.contains(&fullscreen_atom), "{state:?}");
}

#[test]
fn a_hidden_x11_window_is_told_hidden() {
    let mut seat = X11Seat::new("hidden", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, window, described, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("hidden"));
    let id = seat.seat;
    seat.compositor
        .change_layout(|layout| layout.move_to_workspace(id, "two"))
        .expect("moved to a workspace of its own");
    let output = seat.output_rect(seat.outputs[0]);
    let client = geometry_becomes(&mut seat.compositor, client, window, x11_tile(output));
    let hidden_atom = client.atoms._NET_WM_STATE_HIDDEN;
    let state_has_hidden = move |compositor: &mut Compositor, client: X11Client, wanted: bool| {
        step(compositor, client, move |client| {
            let deadline = Instant::now() + PATIENCE;
            loop {
                let state = client.net_wm_state(window)?;
                if state.contains(&hidden_atom) == wanted {
                    return Ok(());
                }
                if Instant::now() > deadline {
                    return Err(format!("its state stays {state:?}"));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        })
        .0
    };
    let client = state_has_hidden(&mut seat.compositor, client, false);

    // Another workspace shown: the window is hidden, and told so, where it is.
    seat.compositor
        .change_layout(|layout| layout.focus_workspace(id, "one"))
        .expect("another workspace");
    let client = state_has_hidden(&mut seat.compositor, client, true);
    let (client, geometry) = step(&mut seat.compositor, client, move |client| {
        client.geometry(window)
    });
    assert_eq!(geometry, x11_tile(output), "it keeps its X11 place");
    assert!(
        seat.compositor
            .windows()
            .iter()
            .any(|listed| listed.id == described.id && listed.output.is_none())
    );

    seat.compositor
        .change_layout(|layout| layout.focus_workspace(id, "two"))
        .expect("its workspace again");
    state_has_hidden(&mut seat.compositor, client, false);
}

/// The game `window` of `client` sets the `width` × `height` mode Xwayland offers on the output at
/// X11 position (`x`, `y`), sizes its window to it there, as a fullscreen game does, and draws
/// [`Image::sharp`] at that size once X11 granted it. Returns the image drawn.
fn play_in_emulated_mode(
    compositor: &mut Compositor,
    client: X11Client,
    window: Window,
    (x, y): (i32, i32),
    (width, height): (u16, u16),
) -> (X11Client, Image) {
    step(compositor, client, move |client| {
        client.set_mode(x, y, width, height)?;
        client
            .connection
            .configure_window(
                window,
                &ConfigureWindowAux::new()
                    .x(x)
                    .y(y)
                    .width(u32::from(width))
                    .height(u32::from(height)),
            )
            .map_err(|e| e.to_string())?;
        client.connection.flush().map_err(|e| e.to_string())?;
        let deadline = Instant::now() + PATIENCE;
        while client.geometry(window)?.width != u32::from(width) {
            if Instant::now() > deadline {
                return Err("the emulated mode's size was not granted".to_owned());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let image = Image::sharp(u32::from(width), u32::from(height));
        client.draw(window, &image)?;
        Ok(image)
    })
}

#[test]
fn a_fullscreen_x11_window_keeps_its_emulated_mode() {
    let mut seat = X11Seat::at_both("emulated");
    let at_one = seat.outputs[1];
    seat.open_on(at_one);
    let client = seat.client();
    let spec = WindowSpec {
        fullscreen: true,
        ..WindowSpec::named("game")
    };
    let (client, window, described, _) = open_drawn(&mut seat.compositor, client, &spec);
    assert!(described.fullscreen);
    assert_eq!(described.output, Some(at_one));
    // X11 sees the output at 1 as 2880 × 1620 at (2880, 0).
    let client = geometry_becomes(
        &mut seat.compositor,
        client,
        window,
        Geometry {
            x: 2880,
            y: 0,
            width: 2880,
            height: 1620,
        },
    );

    // The game sets that output's 1920 × 1080 mode, as Xwayland offers it, sizes its window to
    // it, and draws.
    let (client, image) = play_in_emulated_mode(
        &mut seat.compositor,
        client,
        window,
        (2880, 0),
        (1920, 1080),
    );
    // Xwayland scales it through its viewport to the whole output: drawn at the output's mode,
    // pixel for pixel.
    let whole = CaptureTarget::Output {
        output: Some(at_one),
        region: None,
    };
    let frame = capture_until(
        &mut seat.compositor,
        seat.seat,
        whole,
        CaptureSize::Physical,
        |frame| is_exactly(frame, &image),
    );
    assert!(
        is_exactly(&frame, &image),
        "the game's frame is the output's, pixel for pixel"
    );
    // Arrangements again keep it: its size stays the mode's.
    seat.compositor.change_layout(|_| {});
    let client = geometry_becomes(
        &mut seat.compositor,
        client,
        window,
        Geometry {
            x: 2880,
            y: 0,
            width: 1920,
            height: 1080,
        },
    );
    drop(client);
    let frame = capture_until(
        &mut seat.compositor,
        seat.seat,
        whole,
        CaptureSize::Physical,
        |frame| is_exactly(frame, &image),
    );
    assert!(is_exactly(&frame, &image));
}

/// debt.md, The headless compositor: "No test drives an X11 window that keeps its size across an
/// output's change". The X11 side of `outputs.rs`'s game that keeps its size: a fullscreen game
/// sets the 1920 × 1080 mode Xwayland emulates on an output of 2880 × 1620 at 1.5, sizes its
/// window to it and draws, and Xwayland's viewport shows it over the whole output, 1920 × 1080
/// units. The output is given 3840 × 2160 at 1.5: Yuna asks the game the whole output, 2560 ×
/// 1440 units, and tells its window the emulated mode's rectangle, which it has already. The
/// window keeps its size and commits nothing; the layout places it at the size asked. X11
/// acknowledges no configure: the game's next frame, its first commit since, answers, though its
/// size through the viewport Xwayland keeps, 1920 × 1080 units, did not change, and the game is
/// centred, at (320, 180).
#[test]
fn an_x11_game_that_keeps_its_size_is_centred_again_once_it_commits_after_the_new_mode() {
    let mut seat = X11Seat::new("keeps-size", &[at_1_5()], ClockMode::Real, off());
    let output = seat.outputs[0];
    let client = seat.client();
    let spec = WindowSpec {
        fullscreen: true,
        ..WindowSpec::named("game")
    };
    let (client, window, described, _) = open_drawn(&mut seat.compositor, client, &spec);
    let id = described.id;
    let game = |seat: &X11Seat| {
        seat.compositor
            .windows()
            .into_iter()
            .find(|listed| listed.id == id)
            .expect("the game is listed")
    };
    // X11 sees the output as 2880 × 1620 at (0, 0).
    let whole = Geometry {
        x: 0,
        y: 0,
        width: 2880,
        height: 1620,
    };
    let client = geometry_becomes(&mut seat.compositor, client, window, whole);
    let (client, image) =
        play_in_emulated_mode(&mut seat.compositor, client, window, (0, 0), (1920, 1080));
    rest(&mut seat);
    let emulated = Geometry {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    let at_corner = |w: f64, h: f64| Rect {
        x: 0.0,
        y: 0.0,
        w,
        h,
    };
    assert_eq!(game(&seat).place, Some(at_corner(1920.0, 1080.0)));

    let commits = seat.compositor.seat_commits(seat.seat);
    let larger = OutputMode::new(3840, 2160, 60.0, 1.5).expect("a valid mode");
    seat.compositor
        .configure_output(output, larger)
        .expect("the output takes the mode");
    rest(&mut seat);
    let (client, geometry) = step(&mut seat.compositor, client, move |client| {
        client.geometry(window)
    });
    assert_eq!(
        geometry, emulated,
        "its window keeps the emulated mode's size"
    );
    assert_eq!(
        seat.compositor.seat_commits(seat.seat),
        commits,
        "the game commits nothing"
    );
    assert!(seat.compositor.layout().awaits_commit(id));
    assert_eq!(
        game(&seat).place,
        Some(at_corner(2560.0, 1440.0)),
        "placed at the size asked until it answers"
    );

    // The game's next frame, at its own size.
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        client.draw(window, &image)
    });
    rest(&mut seat);
    let (client, geometry) = step(&mut seat.compositor, client, move |client| {
        client.geometry(window)
    });
    assert!(seat.compositor.seat_commits(seat.seat) > commits);
    assert_eq!(geometry, emulated, "Yuna leaves its window as it is");
    assert!(
        !seat.compositor.layout().awaits_commit(id),
        "its first commit answered the size asked"
    );
    let centred = Rect {
        x: 320.0,
        y: 180.0,
        w: 1920.0,
        h: 1080.0,
    };
    let answered = game(&seat);
    assert_eq!(
        (answered.place, answered.rect),
        (Some(centred), Some(centred)),
        "centred at the size it committed"
    );
    drop(client);
}

#[test]
fn the_x11_cap_counts_x11_clients_not_xwayland() {
    // A small output, so that a thousand windows' buffers stay small: 200 × 100 at 1, its
    // columns 76 × 68.
    let small = OutputMode::new(200, 100, 60.0, 1.0).expect("a valid mode");
    let mut seat = X11Seat::new("cap", &[small], ClockMode::Real, off());
    let many = seat.client();
    let spec = |index: usize| WindowSpec {
        background: Some(0x0080_8080),
        ..WindowSpec::named(&format!("many-{index}"))
    };
    let specs: Vec<WindowSpec> = (0..MAX_WINDOWS_PER_CLIENT).map(spec).collect();
    let (many, ()) = step(&mut seat.compositor, many, move |client| {
        let mut windows = HashSet::new();
        for spec in &specs {
            let window = client.create(spec)?;
            client
                .connection
                .map_window(window)
                .map_err(|e| e.to_string())?;
            windows.insert(window);
        }
        client.connection.flush().map_err(|e| e.to_string())?;
        while !windows.is_empty() {
            if let Event::MapNotify(mapped) = client.wait_for("the windows mapped", |event| {
                matches!(event, Event::MapNotify(_))
            })? {
                windows.remove(&mapped.window);
            }
        }
        Ok(())
    });
    let of_class = |compositor: &Compositor, class: &str| {
        compositor
            .windows()
            .iter()
            .filter(|window| window.app_id.as_deref() == Some(class))
            .count()
    };
    wait(
        &mut seat.compositor,
        "a thousand windows open",
        |compositor| (of_class(compositor, "X11Test") == MAX_WINDOWS_PER_CLIENT).then_some(()),
    );
    // One more of the same X11 client is past its cap, and not shown.
    let over = spec(MAX_WINDOWS_PER_CLIENT);
    let (_many, _) = step(&mut seat.compositor, many, move |client| client.open(&over));
    // Another X11 client of the same Xwayland, one Wayland client, still opens its own.
    let other = seat.client();
    let spec = WindowSpec {
        class: "Other".to_owned(),
        background: Some(0x0040_4040),
        ..WindowSpec::named("other")
    };
    let (_other, _) = step(&mut seat.compositor, other, move |client| {
        client.open(&spec)
    });
    wait(
        &mut seat.compositor,
        "the other client's window opens",
        |compositor| (of_class(compositor, "Other") == 1).then_some(()),
    );
    let settled = Instant::now() + Duration::from_millis(500);
    while Instant::now() < settled {
        pump(&mut seat.compositor);
    }
    assert_eq!(
        of_class(&seat.compositor, "X11Test"),
        MAX_WINDOWS_PER_CLIENT
    );
    assert_eq!(of_class(&seat.compositor, "Other"), 1);
    seat.compositor
        .layout()
        .check_invariants()
        .expect("the layout keeps its invariants");
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
}

#[test]
fn x11_windows_of_two_seats_stay_apart() {
    let folder = Folder::new("two-seats");
    let mut compositor = Compositor::new(Config {
        outputs: Vec::new(),
        clock: ClockMode::Real,
        listen: false,
        xwayland: Some(XwaylandConfig::default()),
        ..Config::default()
    })
    .expect("the compositor starts");
    let mut seats = Vec::new();
    for name in ["a", "b"] {
        let seat = compositor.add_seat().expect("a seat");
        let output = compositor.add_output(seat, at_1()).expect("an output");
        compositor
            .listen_for_seat(seat, folder.0.join(name))
            .expect("the seat listens");
        let display = compositor.x11_display(seat).expect("a display");
        let client = connect(&mut compositor, &display);
        let (client, window, described, _) =
            open_drawn(&mut compositor, client, &WindowSpec::named(name));
        assert_eq!(described.seat, seat);
        assert_eq!(described.output, Some(output));
        seats.push((seat, output, client, window));
    }
    let [
        (seat_a, output_a, client_a, window_a),
        (_, _, client_b, window_b),
    ] = <[_; 2]>::try_from(seats).unwrap_or_else(|_| unreachable!("two seats"));
    // Each Xwayland lists its own seat's window alone.
    let (client_a, listed) = step(&mut compositor, client_a, |client| client.client_list());
    assert_eq!(listed, vec![window_a]);
    let (client_b, listed) = step(&mut compositor, client_b, |client| client.client_list());
    assert_eq!(listed, vec![window_b]);

    // A click on seat a's window reaches it, and nothing of it seat b's.
    compositor
        .pointer_move(seat_a, Some(output_a), TILE.x + 10.0, TILE.y + 10.0)
        .expect("the pointer moves");
    compositor
        .pointer_button(seat_a, BUTTON_LEFT, true)
        .expect("pressed");
    compositor
        .pointer_button(seat_a, BUTTON_LEFT, false)
        .expect("released");
    let (_, at) = button_press_at(&mut compositor, client_a, window_a);
    assert_eq!(at, (10, 10));
    let (_, events) = step(&mut compositor, client_b, |client| client.take_events());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::ButtonPress(_) | Event::KeyPress(_))),
        "seat b's client got another seat's input"
    );
}

#[test]
fn a_transient_x11_dialog_opens_floating_at_its_own_size_over_its_parent() {
    let mut seat = X11Seat::new("dialog", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, parent, parent_described, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("parent"));
    let spec = WindowSpec {
        transient_for: Some(parent),
        rect: (0, 0, 300, 200),
        ..WindowSpec::named("dialog")
    };
    let (client, dialog, described, _) = open_drawn(&mut seat.compositor, client, &spec);
    assert!(described.floating, "a dialog floats");
    // Its own size, 300 × 200 X11 pixels: 200 × 133.33 units at S = 1.5.
    let place = described.place.expect("the dialog's place");
    assert_eq!(
        ((place.w * 1.5).round(), (place.h * 1.5).round()),
        (300.0, 200.0),
        "{place:?}"
    );
    // Over its parent, centred on it.
    let over = parent_described.place.expect("the parent's place");
    assert!(
        place.x >= over.x
            && place.y >= over.y
            && place.right() <= over.right()
            && place.bottom() <= over.bottom(),
        "{place:?} is not over {over:?}"
    );
    let centre = |rect: Rect| (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    let (dx, dy) = (
        centre(place).0 - centre(over).0,
        centre(place).1 - centre(over).1,
    );
    assert!(
        dx.abs() <= 1.0 && dy.abs() <= 1.0,
        "off centre by {dx}, {dy}"
    );
    // In X11: its place's corner × S, its own size.
    let at = Geometry {
        x: (place.x * 1.5).round() as i32,
        y: (place.y * 1.5).round() as i32,
        width: 300,
        height: 200,
    };
    let client = geometry_becomes(&mut seat.compositor, client, dialog, at);
    // Its own request for another size is granted on the axes the layout leaves to it, both
    // here; its place stays the layout's.
    let client = step(&mut seat.compositor, client, move |client| {
        client
            .connection
            .configure_window(
                dialog,
                &ConfigureWindowAux::new().x(5).y(5).width(360).height(240),
            )
            .map_err(|e| e.to_string())?;
        client.sync()
    })
    .0;
    let _client = geometry_becomes(
        &mut seat.compositor,
        client,
        dialog,
        Geometry {
            width: 360,
            height: 240,
            ..at
        },
    );
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
}

#[test]
fn a_client_sending_a_forged_serial_pairs_nothing() {
    let mut seat = X11Seat::new("forged", &[at_1_5()], ClockMode::Real, off());
    let forger = seat.client();
    // Xwayland numbers its surfaces' serials from 1, so the next ones are easy to guess: the
    // forger claims them for windows of its own, never mapped.
    let (forger, decoys) = step(&mut seat.compositor, forger, |client| {
        let mut decoys = Vec::new();
        for index in 0..16 {
            decoys.push(client.create(&WindowSpec::named(&format!("decoy-{index}")))?);
        }
        for (serial, &decoy) in (1..).zip(&decoys) {
            client.forge_serial(decoy, serial)?;
        }
        client.sync()?;
        Ok(decoys)
    });
    let settled = Instant::now() + Duration::from_millis(200);
    while Instant::now() < settled {
        pump(&mut seat.compositor);
    }
    // Another program's windows, the first surfaces of this Xwayland, pair with their own.
    let mut victim = seat.client();
    for name in ["first", "second", "third"] {
        let (client, _, described, image) =
            open_drawn(&mut seat.compositor, victim, &WindowSpec::named(name));
        victim = client;
        let frame = capture_until(
            &mut seat.compositor,
            seat.seat,
            CaptureTarget::Window(described.id),
            CaptureSize::Physical,
            |frame| is_exactly(frame, &image),
        );
        assert!(is_exactly(&frame, &image), "{name} shows its own pixels");
    }
    // A decoy mapped at last pairs with its own surface.
    let decoy = decoys[0];
    let (_forger, image) = step(&mut seat.compositor, forger, move |client| {
        client
            .connection
            .map_window(decoy)
            .map_err(|e| e.to_string())?;
        client.connection.flush().map_err(|e| e.to_string())?;
        client.wait_for(
            "the decoy mapped",
            |event| matches!(event, Event::MapNotify(mapped) if mapped.window == decoy),
        )?;
        let geometry = client.geometry(decoy)?;
        let image = Image::sharp(geometry.width, geometry.height);
        client.draw(decoy, &image)?;
        Ok(image)
    });
    let described = window_titled(&mut seat.compositor, "decoy-0");
    let frame = capture_until(
        &mut seat.compositor,
        seat.seat,
        CaptureTarget::Window(described.id),
        CaptureSize::Physical,
        |frame| is_exactly(frame, &image),
    );
    assert!(is_exactly(&frame, &image), "the decoy shows its own pixels");
    assert_eq!(seat.compositor.windows().len(), 4);
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
}

#[test]
fn a_withdrawn_x11_window_maps_again_without_its_old_states() {
    let mut seat = X11Seat::new("withdrawn", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, window, described, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("again"));
    let client = step(&mut seat.compositor, client, move |client| {
        client.ask_fullscreen(window, true)
    })
    .0;
    wait(
        &mut seat.compositor,
        "the window is fullscreen",
        |compositor| {
            compositor
                .windows()
                .into_iter()
                .find(|listed| listed.id == described.id && listed.fullscreen)
        },
    );
    // Withdrawn, it loses its states, as EWMH asks: `_NET_WM_STATE` is deleted.
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        client.withdraw(window)?;
        let deadline = Instant::now() + PATIENCE;
        loop {
            let state = client.property32(window, client.atoms._NET_WM_STATE)?;
            let wm_state = client.property32(window, client.atoms.WM_STATE)?;
            if state.is_none() && wm_state.as_deref().and_then(<[u32]>::first) == Some(&0) {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "withdrawn, its _NET_WM_STATE stays {state:?}, WM_STATE {wm_state:?}"
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    wait(&mut seat.compositor, "the window closes", |compositor| {
        compositor.windows().is_empty().then_some(())
    });
    // Mapped again, it opens tiled.
    let client = step(&mut seat.compositor, client, move |client| {
        client
            .connection
            .map_window(window)
            .map_err(|e| e.to_string())?;
        client.connection.flush().map_err(|e| e.to_string())?;
        client.wait_for(
            "the window mapped again",
            |event| matches!(event, Event::MapNotify(mapped) if mapped.window == window),
        )?;
        let geometry = client.geometry(window)?;
        client.draw(window, &Image::sharp(geometry.width, geometry.height))
    })
    .0;
    let output = seat.output_rect(seat.outputs[0]);
    let _client = geometry_becomes(&mut seat.compositor, client, window, x11_tile(output));
    let again = window_titled(&mut seat.compositor, "again");
    assert!(!again.fullscreen, "it maps again tiled");
    assert_eq!(again.place, Some(TILE));
}

/// One round of the compositor's loop on the manual clock, the clock moved toward its next
/// deadline only while that deadline comes before `limit`: a story holds a sync request's
/// deadline back while its client works.
fn pump_before(compositor: &mut Compositor, limit: Duration) {
    compositor
        .dispatch(Some(Duration::from_millis(2)))
        .expect("Yuna dispatches");
    if let Some(at) = compositor.next_deadline().filter(|&at| at < limit) {
        let by = at
            .saturating_sub(compositor.now())
            .min(Duration::from_millis(5));
        compositor.advance(by).expect("the manual clock moves");
    }
}

/// A capture of `target`'s newest frame at its physical size, the clock held before `limit`.
fn capture_before(
    compositor: &mut Compositor,
    seat: SeatId,
    target: CaptureTarget,
    limit: Duration,
) -> CapturedFrame {
    let ticket = compositor
        .capture(seat, target, CaptureSize::Physical)
        .expect("a capture");
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some((_, frame)) = compositor
            .take_captures()
            .into_iter()
            .find(|(taken, _)| *taken == ticket)
        {
            return frame.expect("a frame captured");
        }
        assert!(Instant::now() < deadline, "a capture never came");
        pump_before(compositor, limit);
    }
}

/// Whether `frame` holds a pixel of pure red, R, G, B.
fn has_red(frame: &CapturedFrame) -> bool {
    frame.image().as_chunks::<3>().0.contains(&[255, 0, 0])
}

/// An image of one colour.
fn solid(width: u32, height: u32, pixel: [u8; 3]) -> Image {
    Image {
        width,
        height,
        pixels: vec![pixel; (width * height) as usize],
    }
}

/// What a window saw of a sync request: its value, the size configured after it, and the
/// window's `_XWAYLAND_ALLOW_COMMITS` then.
struct SyncSeen {
    value: u64,
    size: (u32, u32),
    held: Option<u32>,
}

/// Waits for `window`'s sync request, then for its configure to a new size.
fn sync_request_of(
    client: &mut X11Client,
    window: Window,
    old_width: u32,
) -> Result<SyncSeen, String> {
    let atoms = client.atoms;
    let request = client.wait_for("a sync request", |event| {
        matches!(event, Event::ClientMessage(message)
            if message.window == window
                && message.type_ == atoms.WM_PROTOCOLS
                && message.data.as_data32()[0] == atoms._NET_WM_SYNC_REQUEST)
    })?;
    let Event::ClientMessage(request) = request else {
        unreachable!("a client message");
    };
    let data = request.data.as_data32();
    let value = u64::from(data[2]) | (u64::from(data[3]) << 32);
    client.wait_for("its configure", |event| {
        matches!(event, Event::ConfigureNotify(configured)
            if configured.window == window && u32::from(configured.width) != old_width)
    })?;
    let geometry = client.geometry(window)?;
    let held = client.allow_commits(window)?;
    Ok(SyncSeen {
        value,
        size: (geometry.width, geometry.height),
        held,
    })
}

#[test]
fn a_resize_waits_for_the_sync_counter_and_never_past_the_timeout() {
    let mut seat = X11Seat::new("sync", &[at_1_5()], ClockMode::Manual, off());
    let client = seat.client();
    let spec = WindowSpec {
        syncs: true,
        ..WindowSpec::named("answers")
    };
    let (client, window, described, _) = open_drawn(&mut seat.compositor, client, &spec);

    // Yuna widens it: a sync request comes before the configure, and Xwayland holds its
    // commits. Its client draws red at the new size first, which never shows.
    let id = described.id;
    let changed = seat.compositor.now();
    let limit = changed + X11_SYNC_TIMEOUT - Duration::from_millis(10);
    seat.compositor
        .change_layout(|layout| layout.set_full_width(id, true))
        .expect("full width");
    let pump = move |compositor: &mut Compositor| pump_before(compositor, limit);
    let (client, SyncSeen { value, size, held }) =
        step_pumping(&mut seat.compositor, client, pump, move |client| {
            let seen = sync_request_of(client, window, X11_TILE.0)?;
            client.draw(window, &solid(seen.size.0, seen.size.1, [255, 0, 0]))?;
            Ok(seen)
        });
    assert_eq!(value, 1, "the first sync request's value");
    assert_eq!(held, Some(0), "Xwayland holds its commits");
    assert!(size.0 > X11_TILE.0);
    let settled = Instant::now() + Duration::from_millis(100);
    while Instant::now() < settled {
        pump_before(&mut seat.compositor, limit);
    }
    let frame = capture_before(
        &mut seat.compositor,
        seat.seat,
        CaptureTarget::Window(id),
        limit,
    );
    assert!(
        !has_red(&frame),
        "the frame drawn before the sync's answer shows"
    );

    // Drawn again whole, the counter set: its commits go on.
    let image = Image::sharp(size.0, size.1);
    let drawn = image.clone();
    let (client, ()) = step_pumping(&mut seat.compositor, client, pump, move |client| {
        client.draw(window, &drawn)?;
        client.set_counter(window, value)?;
        let deadline = Instant::now() + PATIENCE;
        while client.allow_commits(window)? != Some(1) {
            if Instant::now() > deadline {
                return Err("its commits stay held".to_owned());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    });
    assert!(
        seat.compositor.now() < changed + X11_SYNC_TIMEOUT,
        "answered before the timeout"
    );
    let frame = capture_until(
        &mut seat.compositor,
        seat.seat,
        CaptureTarget::Window(id),
        CaptureSize::Physical,
        |frame| is_exactly(frame, &image),
    );
    assert!(is_exactly(&frame, &image), "the new size shows, whole");

    // A window that never sets its counter is held until the timeout, then shows what it drew.
    let spec = WindowSpec {
        syncs: true,
        ..WindowSpec::named("silent")
    };
    let (client, silent, described, _) = open_drawn(&mut seat.compositor, client, &spec);
    let id = described.id;
    let changed = seat.compositor.now();
    let limit = changed + X11_SYNC_TIMEOUT - Duration::from_millis(10);
    seat.compositor
        .change_layout(|layout| layout.set_full_width(id, true))
        .expect("full width");
    let pump = move |compositor: &mut Compositor| pump_before(compositor, limit);
    let (client, SyncSeen { held, .. }) =
        step_pumping(&mut seat.compositor, client, pump, move |client| {
            let seen = sync_request_of(client, silent, X11_TILE.0)?;
            client.draw(silent, &solid(seen.size.0, seen.size.1, [255, 0, 0]))?;
            Ok(seen)
        });
    assert_eq!(held, Some(0));
    let settled = Instant::now() + Duration::from_millis(100);
    while Instant::now() < settled {
        pump_before(&mut seat.compositor, limit);
    }
    let frame = capture_before(
        &mut seat.compositor,
        seat.seat,
        CaptureTarget::Window(id),
        limit,
    );
    assert!(!has_red(&frame), "held until the timeout");
    // The timeout passes: its commits go on, red at last.
    let past = changed + X11_SYNC_TIMEOUT + Duration::from_millis(1);
    seat.compositor
        .advance(past.saturating_sub(seat.compositor.now()))
        .expect("the clock moves");
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        let deadline = Instant::now() + PATIENCE;
        while client.allow_commits(silent)? != Some(1) {
            if Instant::now() > deadline {
                return Err("its commits stay held past the timeout".to_owned());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    });
    let frame = capture_until(
        &mut seat.compositor,
        seat.seat,
        CaptureTarget::Window(id),
        CaptureSize::Physical,
        has_red,
    );
    assert!(has_red(&frame), "its frame shows after the timeout");
    drop(client);

    // Nothing waits any more: Yuna rests.
    settle_x11(&mut seat.compositor);
    crate::assert_rests(&mut seat.compositor);
}

/// Runs every deadline until Xwayland commits nothing for 300 ms and none is armed.
fn settle_x11(compositor: &mut Compositor) {
    loop {
        crate::motion::settle(compositor, |_| {});
        let commits = compositor.counters().commits;
        let quiet = Instant::now() + Duration::from_millis(300);
        while Instant::now() < quiet {
            compositor
                .dispatch(Some(Duration::from_millis(5)))
                .expect("Yuna dispatches");
        }
        if compositor.counters().commits == commits && compositor.next_deadline().is_none() {
            return;
        }
    }
}

/// Whether `window` of the layout is marked unresponsive.
fn unresponsive(compositor: &Compositor, window: WindowId) -> bool {
    compositor
        .windows()
        .iter()
        .find(|listed| listed.id == window)
        .is_some_and(|listed| listed.unresponsive)
}

#[test]
fn an_unanswered_ping_marks_the_window_unresponsive() {
    let mut seat = X11Seat::new("ping", &[at_1_5()], ClockMode::Manual, off());
    let client = seat.client();
    let spec = |title: &str| WindowSpec {
        pings: true,
        ..WindowSpec::named(title)
    };
    let (client, answering, described, _) =
        open_drawn(&mut seat.compositor, client, &spec("answering"));
    let answering_id = described.id;
    // Asked to close, it is pinged too; it answers, and keeps its window open.
    seat.compositor
        .close_window(answering_id)
        .expect("asked to close");
    let asked = seat.compositor.now();
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        let atoms = client.atoms;
        client.wait_for("WM_DELETE_WINDOW", |event| {
            matches!(event, Event::ClientMessage(message)
                if message.window == answering
                    && message.data.as_data32()[0] == atoms.WM_DELETE_WINDOW)
        })?;
        let deadline = Instant::now() + PATIENCE;
        while client.answer_pings()? == 0 {
            if Instant::now() > deadline {
                return Err("no ping came".to_owned());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    });
    let settled = Instant::now() + Duration::from_millis(300);
    while Instant::now() < settled {
        seat.compositor
            .dispatch(Some(Duration::from_millis(5)))
            .expect("Yuna dispatches");
    }
    let past = asked + X11_PING_TIMEOUT + Duration::from_secs(1);
    seat.compositor
        .advance(past.saturating_sub(seat.compositor.now()))
        .expect("the clock moves");
    assert!(!unresponsive(&seat.compositor, answering_id), "it answered");

    // Another never answers: unresponsive once the ping's time is up, not before; never ended.
    let (client, frozen, described, _) = open_drawn(&mut seat.compositor, client, &spec("frozen"));
    let frozen_id = described.id;
    seat.compositor
        .close_window(frozen_id)
        .expect("asked to close");
    let asked = seat.compositor.now();
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        let atoms = client.atoms;
        client.wait_for("its ping", |event| {
            matches!(event, Event::ClientMessage(message)
                if message.window == frozen && message.data.as_data32()[0] == atoms._NET_WM_PING)
        })?;
        Ok(())
    });
    let before = asked + X11_PING_TIMEOUT - Duration::from_millis(1);
    seat.compositor
        .advance(before.saturating_sub(seat.compositor.now()))
        .expect("the clock moves");
    assert!(
        !unresponsive(&seat.compositor, frozen_id),
        "not before its time"
    );
    seat.compositor
        .advance(Duration::from_millis(2))
        .expect("the clock moves");
    assert!(unresponsive(&seat.compositor, frozen_id), "unresponsive");
    assert!(!unresponsive(&seat.compositor, answering_id));
    // An hour on, it is still there, its client alive.
    seat.compositor
        .advance(Duration::from_secs(3600))
        .expect("the clock moves");
    let (_client, ()) = step(&mut seat.compositor, client, |client| client.sync());
    assert!(unresponsive(&seat.compositor, frozen_id));
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
}

#[test]
fn a_startup_id_carries_an_activation_token() {
    let mut seat = X11Seat::new("startup", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, parent, _, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("parent"));
    // A Wayland program of the same seat takes the focus, and asks a token, as a launcher does.
    let mut launcher = crate::client::Client::connect_to(&mut seat.compositor, seat.seat);
    let w = launcher.open_window(&mut seat.compositor);
    launcher.ack(w);
    launcher.attach(w, 400, 300, 0xFF_20_40_60);
    launcher.commit(w);
    launcher.roundtrip(&mut seat.compositor);
    let launcher_window = wait(
        &mut seat.compositor,
        "the launcher has the focus",
        |compositor| {
            compositor
                .windows()
                .into_iter()
                .find(|window| window.shell == WindowShell::Xdg && window.focused)
        },
    );
    let seat_id = seat.seat;
    wait(
        &mut seat.compositor,
        "the launcher has the keyboard",
        |compositor| {
            (compositor.keyboard_window(seat_id) == Some(launcher_window.id)).then_some(())
        },
    );
    launcher.bind_keyboard(&mut seat.compositor);
    let token =
        launcher.request_token(&mut seat.compositor, Some(launcher.enter_serial()), Some(w));

    // A dialog of the unfocused window would not take the focus; its startup id carries the
    // token, and it does.
    let spec = WindowSpec {
        transient_for: Some(parent),
        startup_id: Some(token),
        rect: (0, 0, 300, 200),
        ..WindowSpec::named("activated")
    };
    let (client, _, activated, _) = open_drawn(&mut seat.compositor, client, &spec);
    wait(
        &mut seat.compositor,
        "the dialog takes the focus",
        |compositor| {
            compositor
                .windows()
                .into_iter()
                .find(|window| window.id == activated.id && window.focused)
        },
    );

    // The launcher focused again, a dialog whose startup id is no token Yuna gave opens
    // without the focus.
    let launcher_id = launcher_window.id;
    seat.compositor
        .change_layout(|layout| layout.focus_window(launcher_id))
        .expect("the launcher focused");
    let spec = WindowSpec {
        transient_for: Some(parent),
        startup_id: Some("launcher-1234_TIME5678".to_owned()),
        rect: (0, 0, 300, 200),
        ..WindowSpec::named("not-activated")
    };
    let (_client, _, other, _) = open_drawn(&mut seat.compositor, client, &spec);
    let settled = Instant::now() + Duration::from_millis(300);
    while Instant::now() < settled {
        pump(&mut seat.compositor);
    }
    let listed = seat.compositor.windows();
    let focused = |id: WindowId| {
        listed
            .iter()
            .any(|window| window.id == id && window.focused)
    };
    assert!(!focused(other.id), "no token, no focus");
    assert!(focused(launcher_id));
}

#[test]
fn a_sync_request_crossing_an_unmap_never_holds_the_window_mapped_again() {
    let mut seat = X11Seat::new("sync-unmap", &[at_1_5()], ClockMode::Manual, off());
    let client = seat.client();
    let spec = WindowSpec {
        syncs: true,
        ..WindowSpec::named("crossing")
    };
    let (client, window, described, _) = open_drawn(&mut seat.compositor, client, &spec);
    // The client withdraws its window while Yuna's loop does not run: the manager sees it
    // unmapped, and Yuna does not yet.
    let idle = |_: &mut Compositor| std::thread::sleep(Duration::from_millis(2));
    let (client, ()) = step_pumping(&mut seat.compositor, client, idle, move |client| {
        client.withdraw(window)?;
        client.wait_for(
            "the window unmapped",
            |event| matches!(event, Event::UnmapNotify(unmapped) if unmapped.window == window),
        )?;
        std::thread::sleep(Duration::from_millis(200));
        Ok(())
    });
    // Yuna widens it all the same, as it still shows it: a configure with a sync request reaches
    // a manager that saw the window go.
    let id = described.id;
    let configures = seat.compositor.counters().configures;
    seat.compositor
        .change_layout(|layout| layout.set_full_width(id, true))
        .expect("full width");
    // The clock alone moves, to the refresh that arranges it: no event of the manager is read.
    while seat.compositor.counters().configures == configures {
        let at = seat.compositor.next_deadline().expect("a refresh armed");
        seat.compositor
            .advance(at.saturating_sub(seat.compositor.now()))
            .expect("the clock moves");
    }
    // The window comes back: its commits are not held, and it opens.
    let (_client, ()) = step(&mut seat.compositor, client, move |client| {
        std::thread::sleep(Duration::from_millis(200));
        client
            .connection
            .map_window(window)
            .map_err(|e| e.to_string())?;
        client.connection.flush().map_err(|e| e.to_string())?;
        client.wait_for(
            "the window mapped again",
            |event| matches!(event, Event::MapNotify(mapped) if mapped.window == window),
        )?;
        let held = client.allow_commits(window)?;
        if held == Some(0) {
            return Err("mapped again, its commits are held".to_owned());
        }
        let geometry = client.geometry(window)?;
        client.draw(window, &Image::sharp(geometry.width, geometry.height))
    });
    window_titled(&mut seat.compositor, "crossing");
}

#[test]
fn a_pinged_window_that_closes_leaves_no_deadline() {
    let mut seat = X11Seat::new("ping-closed", &[at_1_5()], ClockMode::Manual, off());
    let client = seat.client();
    let spec = WindowSpec {
        pings: true,
        ..WindowSpec::named("polite")
    };
    let (client, window, described, _) = open_drawn(&mut seat.compositor, client, &spec);
    seat.compositor
        .close_window(described.id)
        .expect("asked to close");
    let asked = seat.compositor.now();
    // Its client closes it, as asked, without answering the ping.
    let (_client, ()) = step(&mut seat.compositor, client, move |client| {
        let atoms = client.atoms;
        client.wait_for("its ping", |event| {
            matches!(event, Event::ClientMessage(message)
                if message.window == window && message.data.as_data32()[0] == atoms._NET_WM_PING)
        })?;
        client
            .connection
            .destroy_window(window)
            .map_err(|e| e.to_string())?;
        client.sync()
    });
    let limit = asked + X11_PING_TIMEOUT - Duration::from_secs(1);
    wait(&mut seat.compositor, "the window closes", |compositor| {
        compositor.windows().is_empty().then_some(())
    });
    // What is due runs; the ping's deadline went with its window.
    let quiet = Instant::now() + Duration::from_millis(300);
    while Instant::now() < quiet {
        pump_before(&mut seat.compositor, limit);
    }
    assert!(seat.compositor.now() < limit);
    assert_eq!(
        seat.compositor.next_deadline(),
        None,
        "nothing is left armed"
    );

    // Another is pinged, and Xwayland is killed before it answers: the ping's deadline goes with
    // its Xwayland.
    let client = seat.client();
    let (client, window, described, _) = open_drawn(&mut seat.compositor, client, &spec);
    seat.compositor
        .close_window(described.id)
        .expect("asked to close");
    let asked = seat.compositor.now();
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        let atoms = client.atoms;
        client.wait_for("its ping", |event| {
            matches!(event, Event::ClientMessage(message)
                if message.window == window && message.data.as_data32()[0] == atoms._NET_WM_PING)
        })?;
        Ok(())
    });
    let pid = seat
        .compositor
        .x11_display(seat.seat)
        .and_then(|display| display.server)
        .expect("Xwayland runs");
    let pid = Pid::from_raw(pid).expect("a process id");
    kill_process(pid, Signal::KILL).expect("Xwayland is killed");
    drop(client);
    let seat_id = seat.seat;
    wait(&mut seat.compositor, "Xwayland is reaped", |compositor| {
        compositor
            .x11_display(seat_id)
            .is_some_and(|display| display.server.is_none())
            .then_some(())
    });
    let limit = asked + X11_PING_TIMEOUT - Duration::from_secs(1);
    let quiet = Instant::now() + Duration::from_millis(300);
    while Instant::now() < quiet {
        pump_before(&mut seat.compositor, limit);
    }
    assert!(seat.compositor.now() < limit);
    assert_eq!(
        seat.compositor.next_deadline(),
        None,
        "nothing is left armed"
    );
    // Its windows closed with it (ADR-0067).
    assert!(
        seat.compositor.windows().is_empty(),
        "a window of the crashed Xwayland is left"
    );
}

#[test]
fn a_configure_request_before_the_window_opens_gets_its_foretold_place() {
    let mut seat = X11Seat::new("early-configure", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    // It asks to be mapped, then, before it opens, for another place and size.
    let (client, window) = step(&mut seat.compositor, client, |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let window = client.create(&WindowSpec::named("early"))?;
        client.connection.map_window(window).map_err(fail)?;
        client
            .connection
            .configure_window(
                window,
                &ConfigureWindowAux::new().x(5).y(5).width(300).height(200),
            )
            .map_err(fail)?;
        client.connection.flush().map_err(fail)?;
        client.wait_for(
            "the window mapped",
            |event| matches!(event, Event::MapNotify(mapped) if mapped.window == window),
        )?;
        let geometry = client.geometry(window)?;
        client.draw(window, &Image::sharp(geometry.width, geometry.height))?;
        Ok(window)
    });
    window_titled(&mut seat.compositor, "early");
    let output = seat.output_rect(seat.outputs[0]);
    let client = geometry_becomes(&mut seat.compositor, client, window, x11_tile(output));
    // Never configured to what it asked: the size foretold, then its place.
    let (_client, asked) = step(&mut seat.compositor, client, move |client| {
        let events = client.take_events()?;
        Ok(events
            .iter()
            .filter_map(|event| match event {
                Event::ConfigureNotify(configured) if configured.window == window => {
                    Some((configured.width, configured.height))
                }
                _ => None,
            })
            .filter(|&size| size == (300, 200))
            .count())
    });
    assert_eq!(asked, 0, "granted what it asked after asking to be mapped");
}
