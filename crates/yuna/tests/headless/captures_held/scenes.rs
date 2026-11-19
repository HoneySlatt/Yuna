//! The scenes of the held captures, and the patterns their windows draw.
//!
//! Each scene is a manual-clock compositor with animations off and one output of
//! [`LOGICAL_SIZE`] at 60 Hz, 320 × 180 pixels at scale 1 and 480 × 270 at scale 1.5, on the
//! owner's seat, with a test client's windows on it. A new column is (320 − 3 × 16) / 2 = 136
//! units wide and 180 − 2 × 16 = 148 tall, at (16, 16); a second at (168, 16). Each rectangle lies
//! on whole pixels at both scales. Yuna draws no borders: what lies between windows is the gap,
//! the background. The background is (20, 20, 26) in 8 bits, whole levels that every driver
//! stores alike (`render.rs`). Each scene checks where its windows are drawn, and its
//! [`Scene::regions`] give the classes of its pixels from those rectangles (`compare.rs`).
//!
//! The pixels are patterns made here: ramps, and checkers with sharp edges.
//!
//! Not captured: the cursor, which Yuna does not draw and its captures do not hold
//! (`screencopy.rs`); X11 windows and the area filter Yuna draws them through below XWayland's
//! scale (ADR-0068), which need Xwayland, a process of its own that runs on the real clock.

use yuna::{ClockMode, Compositor, Config, OutputMode};

use super::compare::{Class, LogicalRect, Region, whole_pixels};
use crate::client::{Client, Parent, Placement, State};
use crate::run_armed_deadlines;

/// The logical size of every scene's output.
const LOGICAL_SIZE: (u32, u32) = (320, 180);

/// The first column's rectangle, and the second's.
const LEFT_COLUMN: LogicalRect = (16.0, 16.0, 136.0, 148.0);
const RIGHT_COLUMN: LogicalRect = (168.0, 16.0, 136.0, 148.0);

/// A scene: a compositor with one output on the owner's seat, a client whose windows it shows,
/// and the classes of the output's pixels.
pub struct Scene {
    pub compositor: Compositor,
    client: Client,
    scale: f64,
    pub regions: Vec<Region>,
}

impl Scene {
    /// An empty scene on an output of [`LOGICAL_SIZE`] at `scale`.
    fn new(scale: f64) -> Scene {
        let mode = OutputMode::new(
            whole_pixels(f64::from(LOGICAL_SIZE.0), scale),
            whole_pixels(f64::from(LOGICAL_SIZE.1), scale),
            60.0,
            scale,
        )
        .expect("a mode");
        let mut compositor = Compositor::new(Config {
            outputs: vec![mode],
            clock: ClockMode::Manual,
            listen: false,
            ..Config::default()
        })
        .expect("the compositor starts");
        let client = Client::connect(&mut compositor);
        Scene {
            compositor,
            client,
            scale,
            regions: Vec::new(),
        }
    }

    /// Opens a window, `prepare`d before its initial commit, and acknowledges its first
    /// configure; returns it and the logical size it was asked.
    fn open(&mut self, prepare: impl FnOnce(&mut Client, usize)) -> (usize, (i32, i32)) {
        let window = self.client.create_window();
        prepare(&mut self.client, window);
        self.client.commit(window);
        self.client.roundtrip(&mut self.compositor);
        self.client.ack(window);
        let asked = self
            .client
            .window(window)
            .configures
            .last()
            .expect("a configure")
            .clone();
        (window, (asked.width, asked.height))
    }

    /// Opens a window, `prepare`d before its initial commit, drawn in `channels` one buffer pixel
    /// to one physical pixel, as a client following its output's scale draws it: a buffer of
    /// its size × the scale, rounded as `wp_fractional_scale_v1` says, half away from zero, and a
    /// viewport to its size.
    fn open_sharp(
        &mut self,
        channels: Channels,
        prepare: impl FnOnce(&mut Client, usize),
    ) -> usize {
        let (window, (width, height)) = self.open(|client, window| {
            client.get_viewport(window);
            prepare(client, window);
        });
        let (buffer_width, buffer_height) = (
            (f64::from(width) * self.scale).round() as i32,
            (f64::from(height) * self.scale).round() as i32,
        );
        self.client.attach_pixels(
            window,
            buffer_width,
            buffer_height,
            &pattern((buffer_width, buffer_height), channels),
        );
        self.client.set_viewport_destination(window, width, height);
        self.client.commit(window);
        self.settle();
        window
    }

    /// Opens a window drawn in `channels` into a buffer of `buffer_scale`: its size × that
    /// scale, which the renderer resamples to the output's scale.
    fn open_scaled(&mut self, channels: Channels, buffer_scale: i32) -> usize {
        let (window, (width, height)) = self.open(|_, _| {});
        let size = (width * buffer_scale, height * buffer_scale);
        self.client
            .window(window)
            .surface
            .set_buffer_scale(buffer_scale);
        self.client
            .attach_pixels(window, size.0, size.1, &pattern(size, channels));
        self.client.commit(window);
        self.settle();
        window
    }

    /// Lets the compositor read what the client sent, and draw it.
    fn settle(&mut self) {
        self.client.roundtrip(&mut self.compositor);
        run_armed_deadlines(&mut self.compositor);
        self.client.roundtrip(&mut self.compositor);
    }

    /// Where each window is drawn, in logical units, by the order they opened; none for one not
    /// drawn.
    fn drawn(&self) -> Vec<Option<LogicalRect>> {
        let mut windows = self.compositor.windows();
        windows.sort_by_key(|window| window.id);
        windows
            .iter()
            .map(|window| window.rect.map(|rect| (rect.x, rect.y, rect.w, rect.h)))
            .collect()
    }

    /// Marks the pixels of the logical rectangle `rect` as `class`.
    fn mark(&mut self, rect: LogicalRect, class: Class) {
        self.regions.push(Region::of(rect, self.scale, class));
    }
}

/// Two opaque windows side by side, each drawn pixel for pixel, with the gaps around them.
pub fn opaque_columns(scale: f64) -> Scene {
    let mut scene = Scene::new(scale);
    scene.open_sharp(WARM, |_, _| {});
    scene.open_sharp(COOL, |_, _| {});
    assert_eq!(scene.drawn(), [Some(LEFT_COLUMN), Some(RIGHT_COLUMN)]);
    scene
}

/// A translucent window floating over an opaque column and the background, at scale 1: a window
/// of a fixed size floats, centred on the output.
pub fn a_translucent_window_over_another() -> Scene {
    let mut scene = Scene::new(1.0);
    scene.open_sharp(WARM, |_, _| {});
    let size = (120, 80);
    let (window, asked) = scene.open(|client, window| {
        client.set_min_size(window, size.0, size.1);
        client.set_max_size(window, size.0, size.1);
    });
    assert_eq!(asked, size);
    scene
        .client
        .attach_pixels(window, size.0, size.1, &translucent_pattern(size, COOL));
    scene.client.commit(window);
    scene.settle();
    let floating = (100.0, 50.0, 120.0, 80.0);
    assert_eq!(scene.drawn(), [Some(LEFT_COLUMN), Some(floating)]);
    scene.mark(floating, Class::Blended);
    scene
}

/// At scale 1.5, two windows whose buffers the renderer resamples: one of buffer scale 1, drawn
/// 1.5 times its size, and one of buffer scale 2, drawn at three quarters of its size.
pub fn buffers_resampled_at_1_5() -> Scene {
    let mut scene = Scene::new(1.5);
    scene.open_scaled(WARM, 1);
    scene.open_scaled(COOL, 2);
    assert_eq!(scene.drawn(), [Some(LEFT_COLUMN), Some(RIGHT_COLUMN)]);
    scene.mark(LEFT_COLUMN, Class::Filtered);
    scene.mark(RIGHT_COLUMN, Class::Filtered);
    scene
}

/// A window with two subsurfaces, at scale 1, the second above the first and partly outside its
/// parent, which its window's rectangle cuts.
pub fn subsurfaces_stacked_and_cut_to_their_window() -> Scene {
    let mut scene = Scene::new(1.0);
    let window = scene.open_sharp(WARM, |_, _| {});
    for (at, size, channels) in [((16, 24), (96, 100), COOL), ((72, 96), (88, 72), GRASS)] {
        // A desynchronised subsurface, its own commit applied at once; its place waits for its
        // parent's commit.
        let subsurface = scene.client.add_subsurface_at(window, at);
        let surface = scene.client.subsurface(subsurface).surface.clone();
        scene
            .client
            .attach_pixels_to(&surface, size.0, size.1, &pattern(size, channels));
        surface.commit();
    }
    // The window's geometry is its own surface, so that the subsurface past it is cut.
    scene.client.set_window_geometry(window, (0, 0), (136, 148));
    scene.client.commit(window);
    scene.settle();
    assert_eq!(scene.drawn(), [Some(LEFT_COLUMN)]);
    scene
}

/// A popup over its window, at scale 1, past the window's edge, over the gap and the background.
pub fn a_popup_over_its_window() -> Scene {
    let mut scene = Scene::new(1.0);
    let window = scene.open_sharp(WARM, |_, _| {});
    let size = (120, 40);
    let popup = scene.client.open_popup(
        &mut scene.compositor,
        Parent::Window(window),
        Placement::below_right(size, (96, 100, 8, 8)),
    );
    let placed = scene.client.popup(popup).configures[0];
    assert_eq!(
        (placed.x, placed.y, placed.width, placed.height),
        (104, 108, 120, 40)
    );
    scene.client.ack_popup(popup);
    let surface = scene.client.popup(popup).surface.clone();
    scene
        .client
        .attach_pixels_to(&surface, size.0, size.1, &pattern(size, COOL));
    scene.client.commit_popup(popup);
    scene.settle();
    assert_eq!(scene.drawn(), [Some(LEFT_COLUMN)]);
    scene
}

/// A fullscreen window drawn pixel for pixel over the whole output, a column behind it, which is
/// not drawn.
pub fn fullscreen(scale: f64) -> Scene {
    let mut scene = Scene::new(scale);
    scene.open_sharp(GRASS, |_, _| {});
    let window = scene.open_sharp(WARM, |client, window| client.set_fullscreen(window, true));
    assert!(
        scene
            .client
            .window(window)
            .configures
            .last()
            .expect("a configure")
            .states
            .contains(&State::Fullscreen)
    );
    assert_eq!(scene.drawn(), [None, Some((0.0, 0.0, 320.0, 180.0))]);
    scene
}

/// Which channel of a pattern takes each of its parts, as the channel's shift in `0xAARRGGBB`.
#[derive(Debug, Clone, Copy)]
struct Channels {
    across: u32,
    down: u32,
    checker: u32,
}

const RED_SHIFT: u32 = 16;
const GREEN_SHIFT: u32 = 8;
const BLUE_SHIFT: u32 = 0;

/// Red ramps across, green down, the checker in blue.
const WARM: Channels = Channels {
    across: RED_SHIFT,
    down: GREEN_SHIFT,
    checker: BLUE_SHIFT,
};
/// Blue ramps across, red down, the checker in green.
const COOL: Channels = Channels {
    across: BLUE_SHIFT,
    down: RED_SHIFT,
    checker: GREEN_SHIFT,
};
/// Green ramps across, blue down, the checker in red.
const GRASS: Channels = Channels {
    across: GREEN_SHIFT,
    down: BLUE_SHIFT,
    checker: RED_SHIFT,
};

/// The rows along a pattern's top that hold squares of one pixel.
const FINE_ROWS: u32 = 6;
/// The side of a pattern's checker's squares, in pixels.
const CHECKER_CELL: u32 = 8;

/// `index` of `count`, from 0 to 255.
fn ramp(index: u32, count: u32) -> u32 {
    index * 255 / count.saturating_sub(1).max(1)
}

/// `width` × `height` opaque pixels, `0xAARRGGBB` row by row: a ramp across, a ramp down and a
/// checker of 8-pixel squares, each in its channel of `channels`, under 6 rows of black and white
/// squares of one pixel along the top; so that a pixel moved, resampled or blended otherwise
/// changes, at sharp edges as on smooth ramps.
fn pattern((width, height): (i32, i32), channels: Channels) -> Vec<u32> {
    let (width, height) = (width as u32, height as u32);
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let pixel = if y < FINE_ROWS {
                if (x + y).is_multiple_of(2) {
                    0x00FF_FFFF
                } else {
                    0
                }
            } else {
                let checker = if (x / CHECKER_CELL + y / CHECKER_CELL).is_multiple_of(2) {
                    0x30
                } else {
                    0xD0
                };
                ramp(x, width) << channels.across
                    | ramp(y, height) << channels.down
                    | checker << channels.checker
            };
            pixels.push(0xFF00_0000 | pixel);
        }
    }
    pixels
}

/// [`pattern`] at an alpha that ramps across from 0x30 to 0xD0, premultiplied, as `wl_shm`'s
/// ARGB8888 is.
fn translucent_pattern(size: (i32, i32), channels: Channels) -> Vec<u32> {
    let width = size.0 as u32;
    pattern(size, channels)
        .into_iter()
        .enumerate()
        .map(|(index, pixel)| {
            let x = index as u32 % width;
            let alpha = 0x30 + ramp(x, width) * 0xA0 / 255;
            let premultiplied =
                |shift: u32| ((((pixel >> shift) & 0xFF) * alpha + 127) / 255) << shift;
            alpha << 24
                | premultiplied(RED_SHIFT)
                | premultiplied(GREEN_SHIFT)
                | premultiplied(BLUE_SHIFT)
        })
        .collect()
}
