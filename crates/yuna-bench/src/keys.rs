//! The keyboard, `yuna-bench client keys`: types keys into one compositor, for a compositor that
//! takes no command but its keys (dwl, cosmic-comp), and for the tour's latency round.
//!
//! It runs in a sandbox of its own, as every client of the bench, and connects to the
//! `WAYLAND_DISPLAY` that sandbox gives it: the compositor's socket in the run's own runtime
//! folder. Its keys reach that compositor alone, never the owner's session: they go through a
//! protocol of that compositor's, never through a device of the machine's.
//!
//! It types through the first of these the compositor offers (`Typist`):
//!
//! - `zwp_virtual_keyboard_v1`, wlroots', Hyprland's, niri's and cosmic-comp's. It sends its own
//!   keymap first ([`keymap`]), which holds only the keys of [`KEYS`]. wlroots' virtual keyboard
//!   takes no modifier from the keys themselves (`update_state = false` in
//!   `types/wlr_virtual_keyboard_v1.c`): a chord sets its modifiers with a request of their own.
//! - KWin's `org_kde_kwin_fake_input`, which KWin 6.8.2 grants any client outside a sandbox once
//!   it says why (`src/backends/fakeinput/fakeinputbackend.cpp:118`, `src/wayland_server.cpp:157`).
//!   Its keys are Linux key codes, in KWin's own keymap; it types single keys alone.
//! - gamescope's `gamescope_input_method`, through which Steam's keyboard types: its `submit`
//!   action makes gamescope press Enter on its own keyboard, into the focused window, and
//!   release it 30 ms later (`src/ime.cpp:412` and `:311`). It types single keys alone.
//!
//! No crate binds the last two: their few requests are written here (`fake_input`,
//! `input_method`) and sent through wayland-client's backend.
//!
//! It prints `ready <protocol>` once the compositor took its keyboard. It then reads a line on its
//! standard input: a chord, such as `alt+j` ([`Chord`]), or `press`, one key alone
//! ([`PRESS_KEY`]). It sends it ([`requests`]), waits until the compositor has handled it (a
//! round trip: requests are handled in order), and prints `sent <n> <ns>`, `<n>` its lines so
//! far, `<ns>` the CLOCK_MONOTONIC time just before its key's press left for the compositor. It ends when its
//! standard input closes.

use std::ffi::CString;
use std::fs::File;
use std::io::{BufRead, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::backend::protocol::{
    ANONYMOUS_INTERFACE, AllowNull, Argument, ArgumentType, Interface, Message, MessageDesc,
};
use wayland_client::backend::smallvec::SmallVec;
use wayland_client::backend::{Backend, ObjectData, ObjectId};
use wayland_client::globals::{Global, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
};

use crate::clock::monotonic_ns;

/// The keys the keymap holds, those dwl's and cosmic-comp's `motion` press: each keysym's name and
/// its Linux key code (`KEY_1` is 2, `KEY_J` 36). XKB's key code is the Linux one plus 8.
pub const KEYS: [(&str, u32); 4] = [("1", 2), ("2", 3), ("j", 36), ("k", 37)];

/// The key `press` types: one no compositor binds alone, which goes to the focused window.
pub const PRESS_KEY: &str = "k";

/// The modifiers a chord may hold, those dwl's and cosmic-comp's bindings use: each name and its
/// mask among XKB's real modifiers, whose indices are fixed (Shift 0, Lock 1, Control 2, Mod1 3,
/// …, Mod4 6). wlroots' and Smithay's Alt is Mod1, their Super Mod4.
pub const MODIFIERS: [(&str, u32); 2] = [("alt", 1 << 3), ("super", 1 << 6)];

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

/// `wl_keyboard`'s key states.
const RELEASED: u32 = 0;
const PRESSED: u32 = 1;

/// The protocol a compositor is typed into, the first it offers of the module's list.
enum Typist {
    /// `zwp_virtual_keyboard_v1`, with the keymap of [`keymap`].
    Virtual(zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1),
    /// KWin's `org_kde_kwin_fake_input`.
    FakeInput(ObjectId),
    /// gamescope's `gamescope_input_method`, with the serial of its last `done`.
    InputMethod(ObjectId, Arc<InputMethodState>),
}

impl Typist {
    /// Its protocol's name, for `ready`.
    fn protocol(&self) -> &'static str {
        match self {
            Typist::Virtual(_) => "zwp_virtual_keyboard_v1",
            Typist::FakeInput(_) => fake_input::INTERFACE.name,
            Typist::InputMethod(..) => input_method::INTERFACE.name,
        }
    }
}

/// Runs the keyboard until its standard input closes.
pub fn run() -> Result<(), String> {
    let connection = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
    let (globals, mut queue) =
        registry_queue_init::<Keyboard>(&connection).map_err(|e| format!("Wayland: {e}"))?;
    let qh = queue.handle();
    let missing = |name: &'static str| move |e| format!("the compositor offers no {name}: {e}");
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).map_err(missing("wl_seat"))?;
    let offered = globals.contents().clone_list();
    let typist = if let Ok(manager) = globals
        .bind::<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, _, _>(
        &qh,
        1..=1,
        (),
    ) {
        let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
        send_keymap(&keyboard)?;
        Typist::Virtual(keyboard)
    } else if let Some(global) = find(&offered, &fake_input::INTERFACE, 4) {
        let object = bind(
            &connection,
            globals.registry(),
            global,
            &fake_input::INTERFACE,
            4,
        )?;
        fake_input::authenticate(&connection.backend(), &object)?;
        Typist::FakeInput(object)
    } else if let Some(global) = find(&offered, &input_method::MANAGER_INTERFACE, 1) {
        let manager = bind(
            &connection,
            globals.registry(),
            global,
            &input_method::MANAGER_INTERFACE,
            1,
        )?;
        let state = Arc::new(InputMethodState::default());
        let object =
            input_method::create(&connection.backend(), &manager, &seat, Arc::clone(&state))?;
        Typist::InputMethod(object, state)
    } else {
        return Err(
            "the compositor offers no zwp_virtual_keyboard_v1, org_kde_kwin_fake_input or \
             gamescope_input_method_manager: no key can be typed into it"
                .to_owned(),
        );
    };
    let mut state = Keyboard;
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Wayland: {e}"))?;
    if let Typist::InputMethod(_, ime) = &typist
        && ime.unavailable.load(Ordering::Relaxed)
    {
        return Err("gamescope says its input method is unavailable".to_owned());
    }
    let mut out = std::io::stdout().lock();
    let say = |out: &mut std::io::StdoutLock, line: &str| -> Result<(), String> {
        writeln!(out, "{line}")
            .and_then(|()| out.flush())
            .map_err(|e| format!("standard output: {e}"))
    };
    say(&mut out, &format!("ready {}", typist.protocol()))?;
    let mut sent = 0u64;
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| format!("standard input: {e}"))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let chord = if line == "press" { PRESS_KEY } else { line };
        let pressed_at = type_line(&connection, &typist, chord)?;
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland: {e}"))?;
        sent += 1;
        say(&mut out, &format!("sent {sent} {pressed_at}"))?;
    }
    if let Typist::Virtual(keyboard) = &typist {
        keyboard.destroy();
    }
    let _ = connection.flush();
    Ok(())
}

/// Sends the keymap of [`keymap`] to `keyboard`.
fn send_keymap(keyboard: &zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1) -> Result<(), String> {
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
    Ok(())
}

/// Types `chord` through `typist`; gives when its key's press was flushed to the compositor, on
/// CLOCK_MONOTONIC, in ns.
fn type_line(connection: &Connection, typist: &Typist, chord: &str) -> Result<u64, String> {
    let requests = requests(chord)?;
    let flush = || connection.flush().map_err(|e| format!("Wayland: {e}"));
    let mut pressed_at = None;
    match typist {
        Typist::Virtual(keyboard) => {
            for request in requests {
                match request {
                    Request::Modifiers(mask) => keyboard.modifiers(mask, 0, 0, 0),
                    // The time is in milliseconds, of an unspecified base that wraps.
                    Request::Key { code, pressed } => {
                        keyboard.key(
                            (monotonic_ns() / 1_000_000) as u32,
                            code,
                            u32::from(pressed),
                        );
                        // Stamped before it leaves: it never arrives before its time.
                        if pressed {
                            pressed_at = Some(monotonic_ns());
                            flush()?;
                        }
                    }
                }
            }
        }
        Typist::FakeInput(object) => {
            let backend = connection.backend();
            for request in requests {
                let Request::Key { code, pressed } = request else {
                    return Err(format!(
                        "{chord}: KWin's fake input types single keys alone"
                    ));
                };
                fake_input::key(
                    &backend,
                    object,
                    code,
                    if pressed { PRESSED } else { RELEASED },
                )?;
                if pressed {
                    pressed_at = Some(monotonic_ns());
                    flush()?;
                }
            }
        }
        Typist::InputMethod(object, state) => {
            if requests.len() != 2 {
                return Err(format!(
                    "{chord}: gamescope's input method types single keys alone"
                ));
            }
            input_method::submit(
                &connection.backend(),
                object,
                state.serial.load(Ordering::Relaxed),
            )?;
            pressed_at = Some(monotonic_ns());
            flush()?;
        }
    }
    pressed_at.ok_or_else(|| format!("{chord} pressed no key"))
}

/// The global of `interface` the compositor offers at `version` at least.
fn find<'a>(offered: &'a [Global], interface: &Interface, version: u32) -> Option<&'a Global> {
    offered
        .iter()
        .find(|global| global.interface == interface.name && global.version >= version)
}

/// Binds `global` as `interface` at `version`, an object without events of its own to handle.
fn bind(
    connection: &Connection,
    registry: &wl_registry::WlRegistry,
    global: &Global,
    interface: &'static Interface,
    version: u32,
) -> Result<ObjectId, String> {
    let name = CString::new(interface.name).map_err(|e| e.to_string())?;
    let message = Message {
        sender_id: registry.id(),
        // wl_registry.bind
        opcode: 0,
        args: SmallVec::from_vec(vec![
            Argument::Uint(global.name),
            Argument::Str(Some(Box::new(name))),
            Argument::Uint(version),
            Argument::NewId(ObjectId::null()),
        ]),
    };
    connection
        .backend()
        .send_request(
            message,
            Some(Arc::new(NoEvents)),
            Some((interface, version)),
        )
        .map_err(|e| format!("binding {}: {e}", interface.name))
}

/// The data of an object whose events, if any, the keyboard ignores.
struct NoEvents;

impl ObjectData for NoEvents {
    fn event(
        self: Arc<Self>,
        _: &Backend,
        _: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData>> {
        None
    }

    fn destroyed(&self, _: ObjectId) {}
}

/// What gamescope's input method told: the serial of its last `done`, and whether it is
/// unavailable.
#[derive(Default)]
struct InputMethodState {
    serial: AtomicU32,
    unavailable: AtomicBool,
}

impl ObjectData for InputMethodState {
    fn event(
        self: Arc<Self>,
        _: &Backend,
        message: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData>> {
        match (message.opcode, message.args.first()) {
            (input_method::UNAVAILABLE, _) => self.unavailable.store(true, Ordering::Relaxed),
            (input_method::DONE, Some(Argument::Uint(serial))) => {
                self.serial.store(*serial, Ordering::Relaxed);
            }
            _ => {}
        }
        None
    }

    fn destroyed(&self, _: ObjectId) {}
}

/// A request's description, as wayland-client's backend checks what it sends against.
const fn request(
    name: &'static str,
    signature: &'static [ArgumentType],
    since: u32,
) -> MessageDesc {
    MessageDesc {
        name,
        signature,
        since,
        is_destructor: false,
        child_interface: None,
        arg_interfaces: &[],
    }
}

/// KWin's `org_kde_kwin_fake_input`, as plasma-wayland-protocols 1.23.0's `fake-input.xml`
/// describes it (LGPL-2.1-or-later): its requests up to `keyboard_key`, whose place in that list
/// is its opcode. It has no events.
mod fake_input {
    use super::*;

    const UINT: ArgumentType = ArgumentType::Uint;
    const FIXED: ArgumentType = ArgumentType::Fixed;
    const STRING: ArgumentType = ArgumentType::Str(AllowNull::No);

    /// The interface, at the version whose `keyboard_key` the keyboard sends.
    pub static INTERFACE: Interface = Interface {
        name: "org_kde_kwin_fake_input",
        version: 4,
        requests: &[
            request("authenticate", &[STRING, STRING], 1),
            request("pointer_motion", &[FIXED, FIXED], 1),
            request("button", &[UINT, UINT], 1),
            request("axis", &[UINT, FIXED], 1),
            request("touch_down", &[UINT, FIXED, FIXED], 2),
            request("touch_motion", &[UINT, FIXED, FIXED], 2),
            request("touch_up", &[UINT], 2),
            request("touch_cancel", &[], 2),
            request("touch_frame", &[], 2),
            request("pointer_motion_absolute", &[FIXED, FIXED], 3),
            request("keyboard_key", &[UINT, UINT], 4),
        ],
        events: &[],
        c_ptr: None,
    };
    const AUTHENTICATE: u16 = 0;
    const KEYBOARD_KEY: u16 = 10;

    /// Tells KWin who types and why, which KWin 6.8.2 grants at once.
    pub fn authenticate(backend: &Backend, object: &ObjectId) -> Result<(), String> {
        let text =
            |text: &str| Argument::Str(Some(Box::new(CString::new(text).unwrap_or_default())));
        send(
            backend,
            object,
            AUTHENTICATE,
            vec![
                text("yuna-bench"),
                text("the rivals' bench types its keys into the KWin it started"),
            ],
        )
    }

    /// Presses or releases the key of Linux key code `code`.
    pub fn key(backend: &Backend, object: &ObjectId, code: u32, state: u32) -> Result<(), String> {
        send(
            backend,
            object,
            KEYBOARD_KEY,
            vec![Argument::Uint(code), Argument::Uint(state)],
        )
    }
}

/// gamescope's `gamescope_input_method_manager` and `gamescope_input_method`, as gamescope's
/// `protocol/gamescope-input-method.xml` describes them (BSD-2-Clause): the manager's requests,
/// and the input method's up to `set_action`, whose places are their opcodes, and its events.
mod input_method {
    use super::*;

    /// The input method.
    pub static INTERFACE: Interface = Interface {
        name: "gamescope_input_method",
        version: 1,
        requests: &[
            MessageDesc {
                is_destructor: true,
                ..request("destroy", &[], 1)
            },
            request("commit", &[ArgumentType::Uint], 1),
            request("set_string", &[ArgumentType::Str(AllowNull::No)], 1),
            request("set_action", &[ArgumentType::Uint], 1),
        ],
        events: &[
            request("unavailable", &[], 1),
            request("done", &[ArgumentType::Uint], 1),
        ],
        c_ptr: None,
    };
    /// Its manager.
    pub static MANAGER_INTERFACE: Interface = Interface {
        name: "gamescope_input_method_manager",
        version: 1,
        requests: &[
            MessageDesc {
                is_destructor: true,
                ..request("destroy", &[], 1)
            },
            MessageDesc {
                child_interface: Some(&INTERFACE),
                arg_interfaces: &[&ANONYMOUS_INTERFACE],
                ..request(
                    "create_input_method",
                    &[ArgumentType::Object(AllowNull::No), ArgumentType::NewId],
                    1,
                )
            },
        ],
        events: &[],
        c_ptr: None,
    };
    /// The event `unavailable`'s opcode.
    pub const UNAVAILABLE: u16 = 0;
    /// The event `done`'s opcode.
    pub const DONE: u16 = 1;
    const CREATE_INPUT_METHOD: u16 = 1;
    const COMMIT: u16 = 1;
    const SET_ACTION: u16 = 3;
    /// The action `submit`, which gamescope types as Enter.
    const SUBMIT: u32 = 1;

    /// Creates the input method of `seat`, whose events go to `state`.
    pub fn create(
        backend: &Backend,
        manager: &ObjectId,
        seat: &wl_seat::WlSeat,
        state: Arc<InputMethodState>,
    ) -> Result<ObjectId, String> {
        let message = Message {
            sender_id: manager.clone(),
            opcode: CREATE_INPUT_METHOD,
            args: SmallVec::from_vec(vec![
                Argument::Object(seat.id()),
                Argument::NewId(ObjectId::null()),
            ]),
        };
        backend
            .send_request(message, Some(state), Some((&INTERFACE, 1)))
            .map_err(|e| format!("gamescope_input_method_manager: {e}"))
    }

    /// Asks gamescope to press Enter, its `submit` action, applied at once by a `commit` of the
    /// last serial it gave.
    pub fn submit(backend: &Backend, object: &ObjectId, serial: u32) -> Result<(), String> {
        send(backend, object, SET_ACTION, vec![Argument::Uint(SUBMIT)])?;
        send(backend, object, COMMIT, vec![Argument::Uint(serial)])
    }
}

/// Sends request `opcode` of `object` with `args`.
fn send(
    backend: &Backend,
    object: &ObjectId,
    opcode: u16,
    args: Vec<Argument<ObjectId, std::os::fd::RawFd>>,
) -> Result<(), String> {
    let message = Message {
        sender_id: object.clone(),
        opcode,
        args: SmallVec::from_vec(args),
    };
    backend
        .send_request(message, None, None)
        .map(|_| ())
        .map_err(|e| format!("request {opcode}: {e}"))
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
        // A key alone sends no modifiers; Super is Mod4.
        assert_eq!(requests("2").unwrap().len(), 2);
        assert_eq!(requests("super+1").unwrap()[0], Request::Modifiers(64));
        assert_eq!(requests(PRESS_KEY).unwrap().len(), 2);
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
