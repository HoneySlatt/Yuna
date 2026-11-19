//! Decoding the base64 of an MCP capture's image, shared by `applications.rs`, `opium.rs` and
//! `headless/captures.rs`.

/// The bytes of base64 `text`, as RFC 4648 writes them, padded.
pub fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Ok(c - b'A'),
        b'a'..=b'z' => Ok(c - b'a' + 26),
        b'0'..=b'9' => Ok(c - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(format!("not base64: {c}")),
    };
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    for chunk in text.as_bytes().chunks(4) {
        let digits = chunk
            .iter()
            .take_while(|&&c| c != b'=')
            .map(|&c| value(c))
            .collect::<Result<Vec<u8>, String>>()?;
        if digits.len() < 2 {
            return Err("base64 cut short".to_owned());
        }
        let n = digits.iter().fold(0u32, |n, &d| n << 6 | u32::from(d)) << (6 * (4 - digits.len()));
        bytes.extend_from_slice(&n.to_be_bytes()[1..digits.len()]);
    }
    Ok(bytes)
}
