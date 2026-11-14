//! X11 windows in Yuna's layout, as `xdg_toplevel`s are: placed, sized, focused, typed into,
//! clicked, closed, made fullscreen, hidden, captured, slid by the motion, and drawn sharp at
//! XWayland's scale and the right size on every output
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

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt as _, Window};
use yuna::yuna_layout::geometry::Rect;
use yuna::yuna_layout::{Direction, OutputId, SeatId, WindowId};
use yuna::yuna_motion::{Change, Motion, Settings};
use yuna::{
    BUTTON_LEFT, CaptureSize, CaptureTarget, CapturedFrame, ClockMode, Compositor, Config,
    MAX_WINDOWS_PER_CLIENT, OutputMode, WindowDescription, WindowShell, X11Display, XwaylandConfig,
    axis_taps,
};

use crate::x11_client::{Geometry, Image, PATIENCE, WindowSpec, X11Client, connect, step};
use crate::{LEAD, boundary, first_boundary_at_or_after};

/// The output at XWayland's scale: 1920 × 1080 logical units at 1.5.
fn at_1_5() -> OutputMode {
    OutputMode::new(2880, 1620, 60.0, 1.5).expect("a valid mode")
}

/// The output below it: 1920 × 1080 at 1.
fn at_1() -> OutputMode {
    OutputMode::new(1920, 1080, 60.0, 1.0).expect("a valid mode")
}

/// A window's place on either output, from the output's corner, in logical units.
const TILE: Rect = Rect {
    x: 16.0,
    y: 16.0,
    w: 936.0,
    h: 1048.0,
};

/// The tile in X11 pixels at S = 1.5: 1404 × 1572, its corner at (24, 24) from the output's.
const X11_TILE: (u32, u32) = (1404, 1572);

/// A story's folder, for its seat's socket and cookie, removed when it drops.
struct Folder(PathBuf);

impl Folder {
    fn new(story: &str) -> Folder {
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
struct X11Seat {
    compositor: Compositor,
    seat: SeatId,
    outputs: Vec<OutputId>,
    display: X11Display,
    _folder: Folder,
}

impl X11Seat {
    /// A seat of `modes`, laid out left to right, on `clock`, its motion as `animations` says.
    fn new(story: &str, modes: &[OutputMode], clock: ClockMode, animations: Settings) -> X11Seat {
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
    fn at_both(story: &str) -> X11Seat {
        X11Seat::new(story, &[at_1_5(), at_1()], ClockMode::Real, off())
    }

    fn client(&mut self) -> X11Client {
        connect(&mut self.compositor, &self.display)
    }

    fn output_rect(&self, output: OutputId) -> Rect {
        self.compositor
            .output_descriptions()
            .into_iter()
            .find(|description| description.id == output)
            .expect("the output")
            .rect
    }

    /// Shows the next window that opens on `output`.
    fn open_on(&mut self, output: OutputId) {
        let seat = self.seat;
        self.compositor
            .change_layout(|layout| layout.focus_output(seat, output))
            .expect("the output takes the focus");
    }

    /// Moves `seat`'s focused window to a workspace of `output`'s own.
    fn move_to(&mut self, output: OutputId) {
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

fn off() -> Settings {
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
fn wait<T>(
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

/// The window titled `title`, once the layout holds it and its output draws it.
fn window_titled(compositor: &mut Compositor, title: &str) -> WindowDescription {
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
fn open_drawn(
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
fn geometry_becomes(
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
fn capture_until(
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
fn is_exactly(frame: &CapturedFrame, image: &Image) -> bool {
    frame.mapping.width == image.width
        && frame.mapping.height == image.height
        && frame.image() == image.pixels.concat()
}

/// What the area filter draws of `image` at `width` × `height` pixels: each pixel the average of
/// the texels under it, with the weights of the captures' encoder (`axis_taps`), rounded. R, G,
/// B, row by row.
fn area_reference(image: &Image, width: u32, height: u32) -> Vec<u8> {
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
fn largest_difference(drawn: &[u8], reference: &[u8]) -> u8 {
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
fn click_at(seat: &mut X11Seat, output: OutputId, x: f64, y: f64) {
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

fn button_press_at(
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
    let (client, image) = step(&mut seat.compositor, client, move |client| {
        client.set_mode(2880, 0, 1920, 1080)?;
        client
            .connection
            .configure_window(
                window,
                &ConfigureWindowAux::new()
                    .x(2880)
                    .y(0)
                    .width(1920)
                    .height(1080),
            )
            .map_err(|e| e.to_string())?;
        client.connection.flush().map_err(|e| e.to_string())?;
        let deadline = Instant::now() + PATIENCE;
        while client.geometry(window)?.width != 1920 {
            if Instant::now() > deadline {
                return Err("the emulated mode's size was not granted".to_owned());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let image = Image::sharp(1920, 1080);
        client.draw(window, &image)?;
        Ok(image)
    });
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
