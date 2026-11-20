//! The scene the processor tier's budget is measured on (ADR-0139): one 4K output and two windows,
//! drawn by today's Yuna on Mesa's llvmpipe and by the prototype on lavapipe. It is defined here
//! once, for both sides: the prototype's `scene` (`scene.rs`) and `prototypes/llvmpipe-scene`, a
//! `wl_shm` client of Yuna, which includes this file.
//!
//! - The output is Yuna's default: 3840 × 2160 at 240 Hz and scale 1.5, 8 bits a channel.
//! - Two windows, in Yuna's two columns of half the width, drawn where Yuna draws them (`WINDOWS`,
//!   read from Yuna's own capture, which `llvmpipe-scene` checks again at each run). Yuna draws
//!   nothing around them but its background, `BACKGROUND`: no border, no shadow, no title bar.
//! - Each window's content is a `wl_shm` buffer of its physical size, `XRGB8888`, opaque, whose
//!   texels tell where they lie (`texel`). Its client follows the fractional scale: its viewport
//!   maps one buffer pixel to one pixel of the output.
//! - Full damage: each frame, both windows commit a buffer with full damage. Partial damage: each
//!   frame, the 64 × 64 region `REGION` of window 0 changes and is damaged, alone.
//! - Pacing, when a side is given a rate: each frame starts at the first boundary of that rate
//!   after the frame before ended (`wait_for_boundary`). `scene` starts its drawing there;
//!   `llvmpipe-scene` commits there, once Yuna's frame callback came, and Yuna draws it at its
//!   next refresh. Without a rate, Yuna's client commits at each frame callback, and Yuna's
//!   frame loop sets the cadence.

use std::time::{Duration, Instant};

/// The output, in pixels, and its rate.
pub const OUTPUT_WIDTH: u32 = 3840;
pub const OUTPUT_HEIGHT: u32 = 2160;
pub const OUTPUT_RATE_HZ: f64 = 240.0;
/// The fractional scale Yuna tells the windows, in 120ths, as `wp_fractional_scale_v1` sends it.
pub const OUTPUT_SCALE_120THS: u32 = 180;

/// Yuna's background, `BACKGROUND` in `crates/yuna/src/render.rs`: red, green, blue, alpha.
pub const BACKGROUND: [f32; 4] = [0.08, 0.08, 0.1, 1.0];

/// A rectangle of the output, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// One window: the logical size Yuna configures it with, its buffer's size, and where Yuna
/// draws it.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub logical: (u32, u32),
    pub buffer: (u32, u32),
    pub rect: Rect,
}

/// The windows as Yuna draws two of them on its default output: its default gap of 16 logical
/// units around them and between them, two columns of half the width,
/// `(2560 - 16) × 0.5 - 16 = 1256` logical units wide, `1440 - 2 × 16 = 1408` high. At scale
/// 1.5 every edge falls on a whole pixel, so each buffer is its rectangle's size.
pub const WINDOWS: [Window; 2] = [
    Window {
        logical: (1256, 1408),
        buffer: (1884, 2112),
        rect: Rect {
            x: 24,
            y: 24,
            width: 1884,
            height: 2112,
        },
    },
    Window {
        logical: (1256, 1408),
        buffer: (1884, 2112),
        rect: Rect {
            x: 1932,
            y: 24,
            width: 1884,
            height: 2112,
        },
    },
];

/// The region of window 0 that partial damage changes, in its buffer: 64 × 64 texels at a place
/// that lines up with nothing, so that it spans four of llvmpipe's 64 × 64 tiles.
pub const REGION: Rect = Rect {
    x: 899,
    y: 1001,
    width: 64,
    height: 64,
};

/// Sleeps until the first boundary of `period` from `origin` after `last_end`, when the frame
/// before ended: the start of the next frame, on both sides of the scene.
pub fn wait_for_boundary(origin: Instant, last_end: Instant, period: Duration) {
    let since = last_end.duration_since(origin).as_nanos();
    let boundaries = since / period.as_nanos() + 1;
    let next = origin + Duration::from_nanos((boundaries * period.as_nanos()) as u64);
    std::thread::sleep(next.saturating_duration_since(Instant::now()));
}

/// The scene in a few lines of Markdown, for the reports of both sides.
pub fn describe_scene() -> String {
    let [red, green, blue, _] = BACKGROUND;
    let mut text = format!(
        "- Output: {OUTPUT_WIDTH} × {OUTPUT_HEIGHT} at {OUTPUT_RATE_HZ} Hz, scale {}/120, 8 bits a \
         channel; background ({red}, {green}, {blue}), {:?} in BGRA bytes.\n",
        OUTPUT_SCALE_120THS,
        background_bytes()
    );
    for (index, window) in WINDOWS.iter().enumerate() {
        let Rect {
            x,
            y,
            width,
            height,
        } = window.rect;
        text.push_str(&format!(
            "- Window {index}: logical {} × {}, an `XRGB8888` buffer of {} × {}, drawn at \
             ({x}, {y}), {width} × {height}.\n",
            window.logical.0, window.logical.1, window.buffer.0, window.buffer.1
        ));
    }
    text.push_str(&format!(
        "- Partial damage: window 0's {} × {} texels at ({}, {}) in its buffer.\n",
        REGION.width, REGION.height, REGION.x, REGION.y
    ));
    text
}

/// The bytes of window `window`'s texel at (x, y) at frame `frame`, as `XRGB8888` lays them out in
/// memory: blue, green, red, then 255 where alpha would be. Blue and green are x's and y's low
/// bytes; red holds their high bits and the window's number, so that a capture tells which
/// buffer texel each pixel shows. Inside `REGION` of window 0, an odd frame inverts the colour:
/// frame 0 is every buffer's first content.
pub fn texel(window: usize, x: u32, y: u32, frame: u64) -> [u8; 4] {
    let blue = (x & 255) as u8;
    let green = (y & 255) as u8;
    let red = (((x >> 8) & 7) | (((y >> 8) & 15) << 3) | ((window as u32 & 1) << 7)) as u8;
    if window == 0 && frame % 2 == 1 && REGION.contains(x, y) {
        [255 - blue, 255 - green, 255 - red, 255]
    } else {
        [blue, green, red, 255]
    }
}

/// `BACKGROUND` in 8 bits, blue, green, red, alpha: each channel times 255, to the nearest.
pub fn background_bytes() -> [u8; 4] {
    let [red, green, blue, alpha] = BACKGROUND.map(|channel| (channel * 255.0).round() as u8);
    [blue, green, red, alpha]
}

/// The output's pixel at (x, y) after frame `frame`, blue, green, red, alpha: a window's texel
/// where a window is, opaque; the background elsewhere.
pub fn expected_pixel(x: u32, y: u32, frame: u64) -> [u8; 4] {
    for (index, window) in WINDOWS.iter().enumerate() {
        if window.rect.contains(x, y) {
            let [blue, green, red, _] = texel(index, x - window.rect.x, y - window.rect.y, frame);
            return [blue, green, red, 255];
        }
    }
    background_bytes()
}

/// How an output's image, `B8G8R8A8` rows of `OUTPUT_WIDTH`, differs from the scene's after frame
/// `frame`.
pub struct ImageCheck {
    pub pixels: u64,
    /// Pixels whose blue, green or red differ; alpha is not compared, since an `XRGB8888` capture
    /// leaves it undefined.
    pub differing: u64,
    pub largest_difference: u8,
    /// The first differing pixels, at most `FIRST_DIFFERENCES`.
    pub first: Vec<Difference>,
}

/// A pixel that differs: where, what the scene has there, and what the image has.
pub struct Difference {
    pub at: (u32, u32),
    pub expected: [u8; 4],
    pub found: [u8; 4],
}

const FIRST_DIFFERENCES: usize = 8;

pub fn check_image(pixels: &[u8], frame: u64) -> ImageCheck {
    let mut check = ImageCheck {
        pixels: 0,
        differing: 0,
        largest_difference: 0,
        first: Vec::new(),
    };
    for (index, found) in pixels.as_chunks::<4>().0.iter().enumerate() {
        let (x, y) = (index as u32 % OUTPUT_WIDTH, index as u32 / OUTPUT_WIDTH);
        let expected = expected_pixel(x, y, frame);
        check.pixels += 1;
        let difference = (0..3)
            .map(|channel| found[channel].abs_diff(expected[channel]))
            .max()
            .unwrap_or(0);
        if difference > 0 {
            check.differing += 1;
            check.largest_difference = check.largest_difference.max(difference);
            if check.first.len() < FIRST_DIFFERENCES {
                check.first.push(Difference {
                    at: (x, y),
                    expected,
                    found: *found,
                });
            }
        }
    }
    check
}

impl ImageCheck {
    pub fn describe(&self) -> String {
        if self.differing == 0 {
            return format!("all {} pixels as the scene defines them", self.pixels);
        }
        let first: Vec<String> = self
            .first
            .iter()
            .map(|difference| {
                let (x, y) = difference.at;
                format!(
                    "({x}, {y}): expected BGRA {:?}, found {:?}",
                    difference.expected, difference.found
                )
            })
            .collect();
        format!(
            "{} of {} pixels differ, by {} at most; the first: {}",
            self.differing,
            self.pixels,
            self.largest_difference,
            first.join("; ")
        )
    }
}

/// Writes an image of the output, `B8G8R8A8` rows of `OUTPUT_WIDTH`, as a PAM file of RGB_ALPHA
/// (netpbm's format, which needs no library), through a temporary file and a rename.
pub fn write_pam(path: &std::path::Path, pixels: &[u8]) -> std::io::Result<()> {
    let mut bytes = format!(
        "P7\nWIDTH {OUTPUT_WIDTH}\nHEIGHT {OUTPUT_HEIGHT}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
    )
    .into_bytes();
    bytes.reserve(pixels.len());
    for [blue, green, red, alpha] in pixels.as_chunks::<4>().0 {
        bytes.extend_from_slice(&[*red, *green, *blue, *alpha]);
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    std::fs::write(&temporary, &bytes)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::{
        OUTPUT_HEIGHT, OUTPUT_WIDTH, REGION, WINDOWS, background_bytes, check_image,
        expected_pixel, texel, wait_for_boundary,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn a_frame_waits_for_the_boundary_after_the_one_before_ended() {
        let origin = Instant::now();
        let period = Duration::from_millis(20);
        // The frame before ended 25 ms in: the next starts at the second boundary, 40 ms in.
        wait_for_boundary(origin, origin + Duration::from_millis(25), period);
        let waited = origin.elapsed();
        assert!(waited >= Duration::from_millis(40), "{waited:?}");
        assert!(waited < Duration::from_millis(60), "{waited:?}");
    }

    #[test]
    fn each_texel_tells_where_it_lies() {
        let decode = |[blue, green, red, _]: [u8; 4]| {
            let (blue, green, red) = (u32::from(blue), u32::from(green), u32::from(red));
            (
                (red >> 7) as usize,
                blue | ((red & 7) << 8),
                green | (((red >> 3) & 15) << 8),
            )
        };
        for (window, rect) in WINDOWS.iter().map(|window| window.rect).enumerate() {
            for (x, y) in [(0, 0), (rect.width - 1, rect.height - 1), (300, 2000)] {
                assert_eq!(decode(texel(window, x, y, 0)), (window, x, y));
            }
        }
        let inside = (REGION.x + 3, REGION.y + 5);
        assert_ne!(
            texel(0, inside.0, inside.1, 1),
            texel(0, inside.0, inside.1, 0)
        );
        assert_eq!(
            texel(0, inside.0, inside.1, 2),
            texel(0, inside.0, inside.1, 0)
        );
        assert_eq!(
            texel(1, inside.0, inside.1, 1),
            texel(1, inside.0, inside.1, 0)
        );
    }

    #[test]
    fn the_windows_lie_within_the_output_apart() {
        let [left, right] = WINDOWS.map(|window| window.rect);
        assert!(left.x + left.width < right.x);
        assert!(right.x + right.width <= OUTPUT_WIDTH && right.y + right.height <= OUTPUT_HEIGHT);
        assert_eq!(expected_pixel(0, 0, 0), background_bytes());
        assert_eq!(background_bytes(), [26, 20, 20, 255]);
        let mut image = Vec::new();
        for y in 0..OUTPUT_HEIGHT {
            for x in 0..OUTPUT_WIDTH {
                image.extend_from_slice(&expected_pixel(x, y, 3));
            }
        }
        assert_eq!(check_image(&image, 3).differing, 0);
        let changed = check_image(&image, 2);
        assert_eq!(changed.differing, u64::from(REGION.width * REGION.height));
    }
}
