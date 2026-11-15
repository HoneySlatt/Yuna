//! The test pattern on the CPU: what shaders/pattern.frag writes at each pixel, to check what a
//! second device samples. The two must change together.

use ash::vk;

/// The formats the prototype imports, by how their channels are stored.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Channels {
    /// 8-bit normalised channels: `B8G8R8A8_UNORM`.
    Eight,
    /// 10-bit normalised colour and 2-bit alpha: `A2B10G10R10_UNORM_PACK32`.
    Ten,
    /// 16-bit floats: `R16G16B16A16_SFLOAT`.
    HalfFloat,
}

impl Channels {
    /// The value pattern.frag's push constant `kind` takes.
    pub fn shader_kind(self) -> u32 {
        match self {
            Self::Eight => 0,
            Self::Ten => 1,
            Self::HalfFloat => 2,
        }
    }

    /// Each channel's code is its value multiplied by this.
    fn scale(self) -> [f32; 4] {
        match self {
            Self::Eight => [255.0; 4],
            Self::Ten => [1023.0, 1023.0, 1023.0, 3.0],
            Self::HalfFloat => [1.0, 1.0, 1.0, 256.0],
        }
    }

    fn opaque_alpha(self) -> u32 {
        match self {
            Self::Eight => 255,
            Self::Ten => 3,
            Self::HalfFloat => 256,
        }
    }
}

/// One of the formats ADR-0073 asks about, with the DRM formats it carries.
#[derive(Clone, Copy)]
pub struct TestFormat {
    pub format: vk::Format,
    pub name: &'static str,
    pub drm_formats: &'static str,
    pub channels: Channels,
}

pub const TEST_FORMATS: [TestFormat; 3] = [
    TestFormat {
        format: vk::Format::B8G8R8A8_UNORM,
        name: "B8G8R8A8_UNORM",
        drm_formats: "XRGB8888, ARGB8888",
        channels: Channels::Eight,
    },
    TestFormat {
        format: vk::Format::A2B10G10R10_UNORM_PACK32,
        name: "A2B10G10R10_UNORM_PACK32",
        drm_formats: "ABGR2101010",
        channels: Channels::Ten,
    },
    TestFormat {
        format: vk::Format::R16G16B16A16_SFLOAT,
        name: "R16G16B16A16_SFLOAT",
        drm_formats: "ABGR16161616F",
        channels: Channels::HalfFloat,
    },
];

/// The integer codes of the pixel at (x, y), red to alpha.
pub fn codes(channels: Channels, x: u32, y: u32, opaque: bool) -> [u32; 4] {
    let mut codes = match channels {
        Channels::Eight => [
            x & 255,
            y & 255,
            ((x >> 8) & 7) | (((y >> 8) & 7) << 3) | (((x ^ y) & 3) << 6),
            (x + y) & 255,
        ],
        Channels::Ten => [
            x & 1023,
            y & 1023,
            ((x >> 10) & 1) | (((y >> 10) & 1) << 1) | (((x * 3 + y * 5) & 255) << 2),
            (x + y) & 3,
        ],
        Channels::HalfFloat => [x & 2047, y & 2047, (x ^ y) & 2047, (x + y) & 255],
    };
    if opaque {
        codes[3] = channels.opaque_alpha();
    }
    codes
}

/// The codes a sampled texel holds: its values multiplied back by their scale, rounded.
pub fn codes_of_sample(channels: Channels, texel: [f32; 4]) -> [u32; 4] {
    let scale = channels.scale();
    std::array::from_fn(|channel| (texel[channel] * scale[channel]).round().max(0.0) as u32)
}

/// The pattern's value at (x, y), red to alpha, as a shader reads it.
pub fn value(channels: Channels, x: u32, y: u32, opaque: bool) -> [f32; 4] {
    let codes = codes(channels, x, y, opaque);
    let scale = channels.scale();
    std::array::from_fn(|channel| codes[channel] as f32 / scale[channel])
}

/// The first pixel whose sample differs from the pattern, and how many differ.
pub struct Mismatch {
    pub count: u64,
    pub x: u32,
    pub y: u32,
    pub expected: [u32; 4],
    pub found: [u32; 4],
}

/// Compares texels sampled row by row (`width` a row, red to alpha) with the pattern.
pub fn check(channels: Channels, width: u32, texels: &[[f32; 4]]) -> Option<Mismatch> {
    let mut mismatch: Option<Mismatch> = None;
    for (index, texel) in texels.iter().enumerate() {
        let x = index as u32 % width;
        let y = index as u32 / width;
        let expected = codes(channels, x, y, false);
        let found = codes_of_sample(channels, *texel);
        if found != expected {
            match &mut mismatch {
                Some(mismatch) => mismatch.count += 1,
                None => {
                    mismatch = Some(Mismatch {
                        count: 1,
                        x,
                        y,
                        expected,
                        found,
                    })
                }
            }
        }
    }
    mismatch
}

#[cfg(test)]
mod tests {
    use super::{Channels, codes, codes_of_sample, value};
    use std::collections::HashSet;

    #[test]
    fn every_pixel_of_a_1080p_image_has_its_own_codes() {
        for channels in [Channels::Eight, Channels::Ten, Channels::HalfFloat] {
            let mut seen = HashSet::new();
            for y in 0..1080 {
                for x in 0..1920 {
                    let [red, green, blue, _] = codes(channels, x, y, false);
                    assert!(seen.insert((red, green, blue)), "({x}, {y}) repeats");
                }
            }
        }
    }

    #[test]
    fn a_value_reads_back_as_its_codes() {
        for channels in [Channels::Eight, Channels::Ten, Channels::HalfFloat] {
            for (x, y) in [(0, 0), (1919, 1079), (255, 256), (1023, 1024)] {
                let expected = codes(channels, x, y, false);
                assert_eq!(
                    codes_of_sample(channels, value(channels, x, y, false)),
                    expected
                );
            }
        }
    }
}
