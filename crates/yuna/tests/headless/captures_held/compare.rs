//! How a capture is compared with its held image: the class of each pixel, from regions of the
//! output, and how far each class may stray; the differences class by class, the difference
//! image, and the PNG files.

use std::fmt;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::Path;

/// How far a blended pixel's channel may stray. Measured on 2026-11-19: llvmpipe draws every
/// scene the same at each run, no pixel differing, over 9 runs with 0, 1, 4 and its default
/// threads and with 128-bit vectors; the same GLES path on a GPU's driver, radeonsi, differs from
/// llvmpipe by 1 at most on blended and filtered pixels. The bound keeps one level above that for
/// another API's arithmetic.
pub const BLENDED_TOLERANCE: u8 = 2;

/// How far a filtered pixel's channel may stray: as a blended one's, on the same measures.
pub const FILTERED_TOLERANCE: u8 = 2;

/// What may vary of a pixel, and so how far it may stray from the held image.
///
/// The metric is, for each pixel, the largest difference over its four 8-bit channels, R, G, B
/// and A; a class holds when none of its pixels is beyond its tolerance.
///
/// - *Opaque*: an opaque buffer drawn one buffer pixel to one output pixel on whole pixels, or
///   the background; exactly. At scale 1.5 that is a client that follows its output's scale, a
///   buffer at its physical size and a viewport to its logical size, as `scale.rs` checks.
/// - *Blended*: a translucent buffer over what is below it; within [`BLENDED_TOLERANCE`].
/// - *Filtered*: a buffer drawn at a size other than its own, which the renderer resamples;
///   within [`FILTERED_TOLERANCE`]. ADR-0139's *area-filtered* class; here the renderer's own
///   bilinear filter, since X11 windows are not captured.
///
/// A pixel in no region of its scene is opaque; a pixel in several takes the most tolerant of
/// their classes.
#[derive(Debug, Clone, Copy)]
pub enum Class {
    Opaque,
    Blended,
    Filtered,
}

impl Class {
    const ALL: [Class; 3] = [Class::Opaque, Class::Blended, Class::Filtered];

    /// The largest difference allowed on each 8-bit channel.
    fn tolerance(self) -> u8 {
        match self {
            Class::Opaque => 0,
            Class::Blended => BLENDED_TOLERANCE,
            Class::Filtered => FILTERED_TOLERANCE,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Class::Opaque => "opaque",
            Class::Blended => "blended",
            Class::Filtered => "filtered",
        }
    }

    fn index(self) -> usize {
        match self {
            Class::Opaque => 0,
            Class::Blended => 1,
            Class::Filtered => 2,
        }
    }
}

/// A logical rectangle: x, y, width, height.
pub type LogicalRect = (f64, f64, f64, f64);

/// A rectangle of an output's pixels and the class of the pixels in it.
#[derive(Debug, Clone, Copy)]
pub struct Region {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    class: Class,
}

impl Region {
    /// The pixels of the logical rectangle `rect` on an output at `scale`.
    pub fn of((x, y, width, height): LogicalRect, scale: f64, class: Class) -> Region {
        Region {
            x: whole_pixels(x, scale),
            y: whole_pixels(y, scale),
            width: whole_pixels(width, scale),
            height: whole_pixels(height, scale),
            class,
        }
    }

    fn contains(&self, x: u32, y: u32) -> bool {
        (self.x..self.x + self.width).contains(&x) && (self.y..self.y + self.height).contains(&y)
    }
}

/// `logical` units at `scale`, in pixels: a scene's rectangles lie on whole pixels, so that its
/// classes depend on no rounding.
pub fn whole_pixels(logical: f64, scale: f64) -> u32 {
    let pixels = logical * scale;
    assert!(
        pixels.fract() == 0.0 && pixels >= 0.0,
        "{logical} units at scale {scale} are not whole pixels"
    );
    pixels as u32
}

/// The class of pixel (`x`, `y`): the most tolerant of the regions that hold it, opaque in none.
fn class_at(regions: &[Region], x: u32, y: u32) -> Class {
    regions
        .iter()
        .filter(|region| region.contains(x, y))
        .map(|region| region.class)
        .max_by_key(|class| class.tolerance())
        .unwrap_or(Class::Opaque)
}

/// An image of 8-bit R, G, B, A pixels, row by row from the top.
#[derive(Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    fn pixel(&self, x: u32, y: u32) -> &[u8] {
        let start = ((y * self.width + x) * 4) as usize;
        &self.rgba[start..start + 4]
    }

    /// The PNG at `path`, 8-bit RGBA; none when there is no file.
    pub fn read_png(path: &Path) -> Option<Image> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => panic!("{} cannot be read: {error}", path.display()),
        };
        let decoder = png::Decoder::new(BufReader::new(file));
        let mut reader = decoder
            .read_info()
            .unwrap_or_else(|e| panic!("{} is not a PNG: {e}", path.display()));
        let mut rgba = vec![0; reader.output_buffer_size().expect("a size")];
        let info = reader
            .next_frame(&mut rgba)
            .unwrap_or_else(|e| panic!("{} cannot be decoded: {e}", path.display()));
        assert_eq!(
            (info.color_type, info.bit_depth),
            (png::ColorType::Rgba, png::BitDepth::Eight),
            "{} is not 8-bit RGBA",
            path.display()
        );
        rgba.truncate(info.buffer_size());
        Some(Image {
            width: info.width,
            height: info.height,
            rgba,
        })
    }

    /// Writes it as an 8-bit RGBA PNG at `path`, through a file beside it renamed over it, so
    /// that an interrupted write leaves the old image whole.
    pub fn write_png(&self, path: &Path) {
        let partial = path.with_extension("png.partial");
        {
            let file = File::create(&partial)
                .unwrap_or_else(|e| panic!("{} cannot be written: {e}", partial.display()));
            let mut encoder = png::Encoder::new(BufWriter::new(file), self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::High);
            let mut writer = encoder.write_header().expect("the PNG's header is written");
            writer
                .write_image_data(&self.rgba)
                .expect("the PNG's pixels are written");
            writer.finish().expect("the PNG is written");
        }
        fs::rename(&partial, path)
            .unwrap_or_else(|e| panic!("{} cannot be written: {e}", path.display()));
    }
}

/// How the pixels of one class of an image differ from a reference.
#[derive(Debug, Default, Clone, Copy)]
struct ClassDifference {
    /// The pixels of the class.
    pixels: usize,
    /// Those that differ at all.
    changed: usize,
    /// The sum of their differences, for their mean.
    sum: u64,
    /// The largest difference.
    largest: u8,
    /// The pixels that differ beyond the class's tolerance.
    beyond: usize,
    /// The first of those, row by row from the top.
    first_beyond: Option<(u32, u32)>,
}

/// How an image differs from a reference of the same size, class by class: what tells a change
/// made on purpose, which moves the class it meant to (blending, or a driver's filter), from a
/// regression.
pub struct Differences {
    classes: [ClassDifference; 3],
}

impl Differences {
    /// `actual` against `reference`, of the same size, under `regions`' classes.
    pub fn of(reference: &Image, actual: &Image, regions: &[Region]) -> Differences {
        assert_eq!(
            (reference.width, reference.height),
            (actual.width, actual.height),
            "images of different sizes"
        );
        let mut classes = [ClassDifference::default(); 3];
        for y in 0..reference.height {
            for x in 0..reference.width {
                let class = class_at(regions, x, y);
                let difference = pixel_difference(reference.pixel(x, y), actual.pixel(x, y));
                let counted = &mut classes[class.index()];
                counted.pixels += 1;
                if difference > 0 {
                    counted.changed += 1;
                    counted.sum += u64::from(difference);
                    counted.largest = counted.largest.max(difference);
                }
                if difference > class.tolerance() {
                    counted.beyond += 1;
                    counted.first_beyond.get_or_insert((x, y));
                }
            }
        }
        Differences { classes }
    }

    /// Whether every class holds: none of its pixels beyond its tolerance.
    pub fn hold(&self) -> bool {
        self.classes.iter().all(|class| class.beyond == 0)
    }
}

impl fmt::Display for Differences {
    /// One line per class the image has: unchanged; or how many pixels changed, by how much on
    /// average and at most, and how many are beyond the class's tolerance, with the first.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for class in Class::ALL {
            let counted = self.classes[class.index()];
            if counted.pixels == 0 {
                continue;
            }
            let name = class.name();
            let tolerance = class.tolerance();
            if counted.changed == 0 {
                writeln!(f, "- {name}: unchanged over {} pixels", counted.pixels)?;
                continue;
            }
            let mean = counted.sum as f64 / counted.changed as f64;
            write!(
                f,
                "- {name}: {} of {} pixels changed, by {mean:.1} on average and {} at most; ",
                counted.changed, counted.pixels, counted.largest
            )?;
            match counted.first_beyond {
                Some((x, y)) => writeln!(
                    f,
                    "{} beyond ±{tolerance}, the first at ({x}, {y})",
                    counted.beyond
                )?,
                None => writeln!(f, "none beyond ±{tolerance}")?,
            }
        }
        Ok(())
    }
}

/// The largest difference of two pixels on any channel.
fn pixel_difference(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0)
}

/// Where `actual` differs from `reference`: magenta beyond its class's tolerance, yellow within
/// it, and elsewhere the reference at a quarter of its brightness.
pub fn difference_image(reference: &Image, actual: &Image, regions: &[Region]) -> Image {
    let mut rgba = Vec::with_capacity(reference.rgba.len());
    for y in 0..reference.height {
        for x in 0..reference.width {
            let difference = pixel_difference(reference.pixel(x, y), actual.pixel(x, y));
            let pixel = if difference > class_at(regions, x, y).tolerance() {
                [255, 0, 255, 255]
            } else if difference > 0 {
                [255, 255, 0, 255]
            } else {
                let [r, g, b, _] =
                    <[u8; 4]>::try_from(reference.pixel(x, y)).expect("four channels");
                [r / 4, g / 4, b / 4, 255]
            };
            rgba.extend(pixel);
        }
    }
    Image {
        width: reference.width,
        height: reference.height,
        rgba,
    }
}
