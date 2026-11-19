//! A capture's pixels, decoded from the PNG Yuna encodes, shared by `applications.rs` and
//! `opium.rs`. Each adds the checks of its own pictures in its own `impl Image`.

/// An image of R, G, B pixels, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 3]>,
}

impl Image {
    /// A capture's PNG, as Yuna encodes it: 8-bit RGB.
    pub fn from_png(png: &[u8]) -> Result<Image, String> {
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
}
