//! Reading the cookie of an Xauthority file, shared by `headless/x11_client.rs`,
//! `headless/xwayland.rs`, `applications.rs` and the example `xwayland_time.rs`.

use std::fs;
use std::path::Path;

/// The four fields after the family of the first entry of an Xauthority file's `bytes`, each
/// led by its length: the address, the display's number, the cookie's name and the cookie. And
/// where that entry ends.
pub fn entry_fields(bytes: &[u8]) -> Result<([Vec<u8>; 4], usize), String> {
    let mut at = 2;
    let mut fields: [Vec<u8>; 4] = Default::default();
    for field in &mut fields {
        let length = bytes
            .get(at..at + 2)
            .map(|length| usize::from(u16::from_be_bytes([length[0], length[1]])))
            .ok_or("a short Xauthority")?;
        *field = bytes
            .get(at + 2..at + 2 + length)
            .ok_or("a short Xauthority")?
            .to_vec();
        at += 2 + length;
    }
    Ok((fields, at))
}

/// The cookie of the Xauthority file at `path`: its one entry's last field.
pub fn cookie_of(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let (mut fields, _) = entry_fields(&bytes)?;
    Ok(std::mem::take(&mut fields[3]))
}
