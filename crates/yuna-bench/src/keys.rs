//! The keyboard, `yuna-bench client keys`: types key chords into one compositor through
//! `zwp_virtual_keyboard_v1`, for a compositor that takes no command but its keys (dwl).
//!
//! It runs in a sandbox of its own, as every client of the bench, and connects to the
//! `WAYLAND_DISPLAY` that sandbox gives it: the compositor's socket in the run's own runtime
//! folder. Its keys reach that compositor alone, never the owner's session.
//!
//! It sends its own keymap first ([`keymap`]), which holds only the keys of [`KEYS`], and prints
//! `ready` once the compositor took it. It then reads a chord a line on its standard input, such
//! as `alt+j` ([`Chord`]), sends it ([`requests`]), waits until the compositor has handled it (a
//! round trip: requests are handled in order), and prints `sent <n>`, `<n>` its chords so far. It
//! ends when its standard input closes.
//!
//! wlroots' virtual keyboard takes no modifier from the keys themselves (`update_state = false`
//! in `types/wlr_virtual_keyboard_v1.c`): a chord sets its modifiers with a request of their own.

use std::fs::File;
use std::io::{BufRead, Write};
use std::os::fd::AsFd;

use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
};

use crate::clock::monotonic_ns;

/// The keys the keymap holds, those dwl's `motion` presses: each keysym's name and its Linux key
/// code (`KEY_1` is 2, `KEY_J` 36). XKB's key code is the Linux one plus 8.
pub const KEYS: [(&str, u32); 4] = [("1", 2), ("2", 3), ("j", 36), ("k", 37)];

/// The modifiers a chord may hold, the one dwl's bindings use: its name and its mask among XKB's
/// real modifiers, whose indices are fixed (Shift 0, Lock 1, Control 2, Mod1 3, …). wlroots' Alt
/// is Mod1.
pub const MODIFIERS: [(&str, u32); 1] = [("alt", 1 << 3)];

/// `wl_keyboard`'s keymap format `xkb_v1`.
const XKB_V1: u32 = 1;

/// A key chord: its modifiers held while one key is pressed and released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    /// The modifiers' names, of [`MODIFIERS`].
    pub modifiers: &'static [&'static str],
    /// The key's keysym name, of [`KEYS`].
    pub key: &'static str,
}

impl Chord {
    /// Its text, as the keyboard reads it on its standard input: `alt+j`.
    pub fn text(&self) -> String {
        let mut parts: Vec<&str> = self.modifiers.to_vec();
        parts.push(self.key);
        parts.join("+")
    }
}

/// One request of the virtual keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// The modifiers now held, as a mask of XKB's real modifiers.
    Modifiers(u32),
    /// A key, by its Linux key code, pressed or released.
    Key {
        /// Its Linux key code.
        code: u32,
        /// Pressed, or released.
        pressed: bool,
    },
}

/// The requests that type `text`, a chord as [`Chord::text`] writes it: its modifiers held, its
/// key pressed and released, its modifiers let go. An unknown key or modifier is an error.
pub fn requests(text: &str) -> Result<Vec<Request>, String> {
    let mut parts: Vec<&str> = text.trim().split('+').collect();
    let key = parts
        .pop()
        .filter(|key| !key.is_empty())
        .ok_or("an empty chord")?;
    let code = KEYS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, code)| *code)
        .ok_or_else(|| format!("the keymap has no key {key:?}"))?;
    let mut mask = 0;
    for modifier in parts {
        mask |= MODIFIERS
            .iter()
            .find(|(name, _)| *name == modifier)
            .map(|(_, mask)| *mask)
            .ok_or_else(|| format!("no modifier {modifier:?}"))?;
    }
    let mut sent = Vec::new();
    if mask != 0 {
        sent.push(Request::Modifiers(mask));
    }
    sent.push(Request::Key {
        code,
        pressed: true,
    });
    sent.push(Request::Key {
        code,
        pressed: false,
    });
    if mask != 0 {
        sent.push(Request::Modifiers(0));
    }
    Ok(sent)
}

/// The keymap the keyboard sends: [`KEYS`] alone, each on one level, with XKB's own types and
/// compatibility, which the compositor's xkbcommon includes from its keyboard data.
pub fn keymap() -> String {
    let mut codes = String::new();
    let mut symbols = String::new();
    for (name, code) in KEYS {
        codes.push_str(&format!("    <K{code}> = {};\n", code + 8));
        symbols.push_str(&format!("    key <K{code}> {{ [ {name} ] }};\n"));
    }
    format!(
        "xkb_keymap {{\n\
         xkb_keycodes \"yuna-bench\" {{\n    minimum = 8;\n    maximum = 255;\n{codes}}};\n\
         xkb_types \"yuna-bench\" {{ include \"complete\" }};\n\
         xkb_compat \"yuna-bench\" {{ include \"complete\" }};\n\
         xkb_symbols \"yuna-bench\" {{\n{symbols}}};\n\
         }};\n"
    )
}

struct Keyboard;

/// Runs the keyboard until its standard input closes.
pub fn run() -> Result<(), String> {
    let connection = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<Keyboard>(&connection).map_err(|e| format!("Wayland: {e}"))?;
    let qh = queue.handle();
    let missing = |name: &'static str| move |e| format!("the compositor offers no {name}: {e}");
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).map_err(missing("wl_seat"))?;
    let manager: zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(missing("zwp_virtual_keyboard_manager_v1"))?;
    let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
    let mut text = keymap().into_bytes();
    // xkbcommon reads the buffer as a C string: the size counts its NUL.
    text.push(0);
    let file = File::from(
        memfd_create("yuna-bench-keymap", MemfdFlags::CLOEXEC)
            .map_err(|e| format!("memfd: {e}"))?,
    );
    (&file)
        .write_all(&text)
        .map_err(|e| format!("memfd: {e}"))?;
    let size = u32::try_from(text.len()).map_err(|_| "a keymap too large")?;
    keyboard.keymap(XKB_V1, file.as_fd(), size);
    let mut state = Keyboard;
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Wayland: {e}"))?;
    let mut out = std::io::stdout().lock();
    let say = |out: &mut std::io::StdoutLock, line: &str| -> Result<(), String> {
        writeln!(out, "{line}")
            .and_then(|()| out.flush())
            .map_err(|e| format!("standard output: {e}"))
    };
    say(&mut out, "ready")?;
    let mut sent = 0u64;
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| format!("standard input: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        for request in requests(&line)? {
            match request {
                Request::Modifiers(mask) => keyboard.modifiers(mask, 0, 0, 0),
                // The time is in milliseconds, of an unspecified base that wraps.
                Request::Key { code, pressed } => keyboard.key(
                    (monotonic_ns() / 1_000_000) as u32,
                    code,
                    u32::from(pressed),
                ),
            }
        }
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland: {e}"))?;
        sent += 1;
        say(&mut out, &format!("sent {sent}"))?;
    }
    keyboard.destroy();
    let _ = connection.flush();
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Keyboard {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(Keyboard: ignore wl_seat::WlSeat);
delegate_noop!(Keyboard: ignore zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1);
delegate_noop!(Keyboard: ignore zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_holds_its_modifiers_around_its_key() {
        let chord = Chord {
            modifiers: &["alt"],
            key: "j",
        };
        assert_eq!(chord.text(), "alt+j");
        assert_eq!(
            requests(&chord.text()),
            Ok(vec![
                Request::Modifiers(8),
                Request::Key {
                    code: 36,
                    pressed: true
                },
                Request::Key {
                    code: 36,
                    pressed: false
                },
                Request::Modifiers(0),
            ])
        );
        // A key alone sends no modifiers.
        assert_eq!(requests("2").unwrap().len(), 2);
        assert!(requests("alt+q").is_err());
        assert!(requests("hyper+j").is_err());
        assert!(requests("alt+").is_err());
        assert!(requests("").is_err());
    }

    #[test]
    fn the_keymap_holds_each_key_at_its_xkb_code() {
        let keymap = keymap();
        assert!(keymap.contains("<K36> = 44;"), "{keymap}");
        assert!(keymap.contains("key <K36> { [ j ] };"), "{keymap}");
        assert!(keymap.contains("key <K2> { [ 1 ] };"), "{keymap}");
        assert!(keymap.contains("xkb_types \"yuna-bench\" { include \"complete\" };"));
        // One `{` for each `}`: a keymap xkbcommon can read.
        assert_eq!(keymap.matches('{').count(), keymap.matches('}').count());
        assert!(!keymap.contains('\0'));
    }
}
