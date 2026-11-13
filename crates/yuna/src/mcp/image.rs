//! A capture's image as MCP gives it: a PNG, in base64
//! ([ADR-0034](../../../../docs/adr/0034-captures.md)). Made on a thread of its own, from the
//! pixels the compositor copied, so that no frame waits for it.

use crate::CapturedFrame;

/// The most bytes of base64 an image may take: Claude's API refuses an image over 5 MB
/// ([survey](../../../../docs/survey.md#agents-and-mcp)), counted here in decimal megabytes,
/// under either reading.
pub const MAX_IMAGE_BYTES: usize = 5_000_000;

/// The most pixels an image may have on a side: Claude's API refuses more than 8,000.
pub const MAX_IMAGE_SIDE: u32 = 8_000;

/// A capture's image, encoded.
#[derive(Debug)]
pub struct Encoded {
    /// The PNG, in base64.
    pub base64: String,
    /// The PNG's size in bytes, before base64.
    pub png_bytes: usize,
}

/// The PNG of `frame`'s image, in base64; refused past [`MAX_IMAGE_BYTES`], with what to do.
pub fn encode(frame: &CapturedFrame) -> Result<Encoded, String> {
    let (width, height) = (frame.mapping.width, frame.mapping.height);
    let png = png_of(&frame.image(), width, height)?;
    let base64 = base64(&png);
    if base64.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "the capture's PNG takes {} bytes of base64, over the {MAX_IMAGE_BYTES} an image may \
             take: capture a region or a window, or at the logical size",
            base64.len()
        ));
    }
    Ok(Encoded {
        base64,
        png_bytes: png.len(),
    })
}

/// `rgb`, `width` × `height` pixels of R, G, B, as a PNG: fdeflate's fast compression, which a
/// screen's large flat areas suit.
fn png_of(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let failed = |e: png::EncodingError| format!("the capture could not be encoded: {e}");
    let mut writer = encoder.write_header().map_err(failed)?;
    writer.write_image_data(rgb).map_err(failed)?;
    writer.finish().map_err(failed)?;
    Ok(png)
}

/// `bytes` in base64, RFC 4648's alphabet, padded.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_follows_rfc_4648() {
        // RFC 4648's own vectors.
        for (input, output) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), output);
        }
        assert_eq!(base64(&[0xFF, 0xFE, 0xFD]), "//79");
    }

    #[test]
    fn a_png_decodes_to_its_pixels() {
        let rgb: Vec<u8> = (0..6 * 4 * 3).map(|i| (i * 7 % 256) as u8).collect();
        let png = png_of(&rgb, 6, 4).expect("encoded");
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().expect("a PNG");
        let mut decoded = vec![0; reader.output_buffer_size().expect("a size")];
        let info = reader.next_frame(&mut decoded).expect("its pixels");
        assert_eq!((info.width, info.height), (6, 4));
        assert_eq!(info.color_type, png::ColorType::Rgb);
        assert_eq!(&decoded[..info.buffer_size()], &rgb[..]);
    }
}
