//! The bounds on what a seat's keyboard and pointer hold at once
//! ([ADR-0131](../../../../docs/adr/0131-bounds-on-what-a-client-or-an-agent-may-ask.md)): a virtual
//! keyboard's press past the keys a seat may hold, or of a key its keymap does not have, and a
//! virtual pointer's press past the buttons a seat may hold, are dropped, as a device past its
//! client's cap is; MCP's own presses past them are refused. A `wl_keyboard.enter` lists every
//! key held, 4 bytes each, in one message of 4,096 bytes at most: unbounded, a virtual keyboard
//! holding about 1,018 keys made every client its seat's keyboard entered unable to read it, and
//! the display ended them.
//!
//! The compositor is `seats.rs`'s: the owner's seat with its 4K output, and two agents' seats,
//! `agent-2` and `agent-3`, each with an output of 1920 × 1080 at scale 1.

use smithay::input::keyboard::xkb;
use yuna::yuna_layout::SeatId;
use yuna::{BUTTON_LEFT, Compositor, MAX_HELD_BUTTONS, MAX_HELD_KEYS};

use crate::input_client::{InputClient, InputEvent};
use crate::seats::{open, settle, two_seats, windows_of};
use crate::virtual_input_client::VirtualDriver;

/// [`MAX_HELD_KEYS`] as evdev's codes count: the stories press the codes from 1 to it.
const HELD_KEYS: u32 = MAX_HELD_KEYS as u32;

/// [`MAX_HELD_BUTTONS`] as Linux's codes of buttons count.
const HELD_BUTTONS: u32 = MAX_HELD_BUTTONS as u32;

/// An evdev key code no keymap of XKB's rules has: past `KEY_MAX`, 0x2ff.
const KEY_PAST_ANY_KEYMAP: u32 = 0xffff;

/// The first button the stories press, `BTN_MISC`; the buttons above it follow.
const FIRST_BUTTON: u32 = 0x100;

/// The keys pressed and released that `events` carry, as evdev codes, in order.
fn keys(events: &[InputEvent]) -> Vec<(u32, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Key { key, pressed, .. } => Some((*key, *pressed)),
            _ => None,
        })
        .collect()
}

/// The buttons pressed and released that `events` carry, in order.
fn buttons(events: &[InputEvent]) -> Vec<(u32, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Button {
                button, pressed, ..
            } => Some((*button, *pressed)),
            _ => None,
        })
        .collect()
}

/// A seat with a client's window, focused, and a driver of that seat.
fn a_window_and_a_driver() -> (Compositor, SeatId, InputClient, VirtualDriver) {
    let (mut compositor, a, _) = two_seats();
    let mut client = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut client, &mut compositor);
    settle(&mut compositor);
    let driver = VirtualDriver::connect(&mut compositor, a);
    (compositor, a, client, driver)
}

/// Runs the driver's requests, then reads what the client was sent.
fn exchange(
    compositor: &mut Compositor,
    driver: &mut VirtualDriver,
    client: &mut InputClient,
) -> Vec<InputEvent> {
    driver.roundtrip(compositor);
    client.roundtrip(compositor);
    client.take_events()
}

/// The audit's story (m1.13-audit-core-2): on agent seat 2, a client maps a window, a driver's
/// virtual keyboard presses evdev keys 1 to 1,100 and releases none, then another client of the
/// seat maps a window, which takes the keyboard. Its `wl_keyboard.enter` lists the keys held:
/// past about 1,018 it no longer fits in a Wayland message, and the display ended both clients,
/// one after the other. Both must stay, their windows on the seat, and the second be told at
/// most [`MAX_HELD_KEYS`] keys.
#[test]
fn a_virtual_keyboard_holding_many_keys() {
    const HELD: u32 = 1_100;
    let (mut compositor, a, _) = two_seats();
    let mut first = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut first, &mut compositor);
    settle(&mut compositor);
    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let keyboard = driver.add_keyboard("agent-2", "us");
    driver.roundtrip(&mut compositor);
    for key in 1..=HELD {
        driver.key(keyboard, key, true);
        if key % 64 == 0 {
            driver.roundtrip(&mut compositor);
            first.roundtrip(&mut compositor);
            first.take_events();
        }
    }
    driver.roundtrip(&mut compositor);
    first.roundtrip(&mut compositor);
    first.take_events();
    compositor.watch_client_departures();
    assert_eq!(
        compositor.client_pids(a).len(),
        2,
        "the window's client and the driver"
    );

    let mut second = InputClient::connect(&mut compositor, a, "agent-2");
    let window = second.client.create_window();
    second.client.commit(window);
    second
        .client
        .try_roundtrip(&mut compositor)
        .expect("the second client's initial commit is answered");
    second.client.ack(window);
    second.client.attach(window, 400, 300, 0xFF_FF_00_00);
    second.client.commit(window);
    second
        .client
        .try_roundtrip(&mut compositor)
        .expect("the second client's first buffer is taken");
    settle(&mut compositor);

    let departures = compositor.take_client_departures();
    assert!(departures.is_empty(), "clients were ended: {departures:?}");
    assert_eq!(
        compositor.client_pids(a).len(),
        3,
        "both clients and the driver"
    );
    second
        .client
        .try_roundtrip(&mut compositor)
        .expect("the second client lives");
    first
        .client
        .try_roundtrip(&mut compositor)
        .expect("the first client lives");
    second.roundtrip(&mut compositor);
    let enters: Vec<usize> = second
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            InputEvent::KeyboardEnter { keys, .. } => Some(keys.len()),
            _ => None,
        })
        .collect();
    assert!(
        !enters.is_empty(),
        "the keyboard never entered the second window"
    );
    assert!(
        enters.iter().all(|&held| held <= MAX_HELD_KEYS),
        "the enters carried {enters:?} keys"
    );
    assert_eq!(windows_of(&compositor, a).len(), 2);
}

/// At [`MAX_HELD_KEYS`] keys held, a virtual keyboard's press of another key is dropped: its
/// client's window gets nothing, and its release later is nothing either. Once a key is let go,
/// a new press goes through. A key the keymap does not have is dropped below the bound too. And
/// MCP's own press of a key past the bound is refused.
#[test]
fn a_seat_holds_a_bounded_number_of_keys() {
    let (mut compositor, a, mut client, mut driver) = a_window_and_a_driver();
    let keyboard = driver.add_keyboard("agent-2", "us");
    exchange(&mut compositor, &mut driver, &mut client);

    for key in 1..=HELD_KEYS {
        driver.key(keyboard, key, true);
    }
    let pressed = keys(&exchange(&mut compositor, &mut driver, &mut client));
    assert_eq!(
        pressed,
        (1..=HELD_KEYS).map(|key| (key, true)).collect::<Vec<_>>()
    );

    let past = HELD_KEYS + 1;
    driver.key(keyboard, past, true);
    assert_eq!(
        keys(&exchange(&mut compositor, &mut driver, &mut client)),
        []
    );
    driver.key(keyboard, past, false);
    assert_eq!(
        keys(&exchange(&mut compositor, &mut driver, &mut client)),
        []
    );
    let refused = compositor
        .key(a, "F12", true)
        .expect_err("MCP's press past the bound is refused");
    assert!(
        refused.to_string().contains(&MAX_HELD_KEYS.to_string()),
        "{refused}"
    );

    driver.key(keyboard, 1, false);
    driver.key(keyboard, past, true);
    assert_eq!(
        keys(&exchange(&mut compositor, &mut driver, &mut client)),
        [(1, false), (past, true)]
    );

    driver.key(keyboard, 2, false);
    driver.key(keyboard, KEY_PAST_ANY_KEYMAP, true);
    driver.key(keyboard, KEY_PAST_ANY_KEYMAP, false);
    driver.key(keyboard, 2, true);
    assert_eq!(
        keys(&exchange(&mut compositor, &mut driver, &mut client)),
        [(2, false), (2, true)]
    );
}

/// Evdev's code of the A key, `KEY_A`.
const KEY_A: u32 = 30;

/// The first evdev code inside the range of XKB's `us` keymap, as a virtual keyboard sends it,
/// that names no key of that keymap: XKB numbers a key by its evdev code plus 8. The keymap is
/// compiled as the driver compiles the one it sends (`virtual_input_client.rs`).
fn a_code_inside_the_us_keymap_that_names_no_key() -> u32 {
    let context = xkb::Context::new(xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
    let keymap = xkb::Keymap::new_from_names(
        &context,
        "evdev",
        "pc105",
        "us",
        "",
        None,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .expect("the layout compiles");
    let (first, last) = (keymap.min_keycode().raw(), keymap.max_keycode().raw());
    (first..=last)
        .find(|&code| keymap.key_get_name(xkb::Keycode::new(code)).is_none())
        .map(|code| code - 8)
        .expect("a code the keymap's range holds names no key")
}

/// A virtual keyboard's press of a key its keymap does not have is dropped, far below the bound:
/// a code past the keymap's range, and one inside it that names no key. Their releases are
/// nothing either, and a key the keymap has goes through.
#[test]
fn a_key_the_keymap_does_not_have_is_dropped() {
    let (mut compositor, _, mut client, mut driver) = a_window_and_a_driver();
    let keyboard = driver.add_keyboard("agent-2", "us");
    exchange(&mut compositor, &mut driver, &mut client);

    for key in [
        KEY_PAST_ANY_KEYMAP,
        a_code_inside_the_us_keymap_that_names_no_key(),
    ] {
        driver.key(keyboard, key, true);
        driver.key(keyboard, key, false);
    }
    driver.key(keyboard, KEY_A, true);
    assert_eq!(
        keys(&exchange(&mut compositor, &mut driver, &mut client)),
        [(KEY_A, true)]
    );
}

/// At [`MAX_HELD_BUTTONS`] buttons held, a virtual pointer's press of another button is
/// dropped, however many it sends; once a button is let go, a new press goes through. MCP's own
/// press past the bound is refused.
#[test]
fn a_seat_holds_a_bounded_number_of_buttons() {
    let (mut compositor, a, mut client, mut driver) = a_window_and_a_driver();
    let pointer = driver.add_pointer(None);
    let window = windows_of(&compositor, a)[0];
    let rect = compositor
        .windows()
        .into_iter()
        .find(|description| description.id == window)
        .and_then(|description| description.rect)
        .expect("a placed window");
    let output = compositor
        .output_descriptions()
        .into_iter()
        .find(|description| description.seat == a)
        .expect("the seat's output")
        .rect;
    // A fraction of the seat's one output of 1920 × 1080 logical units, as an absolute motion
    // gives it.
    let inside = (
        (rect.x - output.x + 10.0) as u32,
        (rect.y - output.y + 10.0) as u32,
    );
    driver.move_to(pointer, inside, (1920, 1080));
    exchange(&mut compositor, &mut driver, &mut client);

    let held: Vec<u32> = (FIRST_BUTTON..FIRST_BUTTON + HELD_BUTTONS).collect();
    for &button in &held {
        driver.button(pointer, button, true);
    }
    assert_eq!(
        buttons(&exchange(&mut compositor, &mut driver, &mut client)),
        held.iter()
            .map(|&button| (button, true))
            .collect::<Vec<_>>()
    );
    assert_eq!(compositor.pointer(a).buttons, held);

    // The audit's related story: a flood of distinct buttons, each dropped.
    let past = FIRST_BUTTON + HELD_BUTTONS;
    for button in past..past + 1_000 {
        driver.button(pointer, button, true);
    }
    let through = buttons(&exchange(&mut compositor, &mut driver, &mut client));
    assert!(through.is_empty(), "{} presses went through", through.len());
    assert_eq!(compositor.pointer(a).buttons, held);
    let refused = compositor
        .pointer_button(a, BUTTON_LEFT, true)
        .expect_err("MCP's press past the bound is refused");
    assert!(
        refused.to_string().contains(&MAX_HELD_BUTTONS.to_string()),
        "{refused}"
    );

    driver.button(pointer, FIRST_BUTTON, false);
    driver.button(pointer, past, true);
    assert_eq!(
        buttons(&exchange(&mut compositor, &mut driver, &mut client)),
        [(FIRST_BUTTON, false), (past, true)]
    );
    assert_eq!(compositor.pointer(a).buttons.len(), MAX_HELD_BUTTONS);
}
