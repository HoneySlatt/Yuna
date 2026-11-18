//! An agent's seat's keyboard layout, changed while Yuna runs: every client of the seat gets the
//! new keymap, an X11 one through Xwayland too, and `type` types through it with keys, AltGr and
//! the layout's groups included, never through a keymap of its own; a layout XKB does not know,
//! or a change while a key is held, is refused with nothing changed; and a virtual keyboard's
//! keymap gives way to the seat's new one again.
//!
//! The compositor is `seats.rs`'s: the owner's seat with its 4K output, and two agents' seats,
//! `agent-2` and `agent-3`, each with an output of 1920 × 1080 at scale 1.

use serde_json::{Value, json};
use smithay::input::keyboard::Keysym;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::ConnectionExt as _;
use yuna::yuna_layout::SeatId;
use yuna::{ClockMode, Compositor, KeyboardLayout, MAX_LAYOUT_NAME, OWNER_SEAT};

use crate::input_client::{InputClient, InputEvent};
use crate::mcp::Mcp;
use crate::seats::{click, open, settle, two_seats, windows_of};
use crate::virtual_input_client::VirtualDriver;
use crate::x11_client::{PATIENCE, WindowSpec, X11Client, step};
use crate::x11_windows::{TILE, X11Seat, at_1, button_press_at, click_at, off, open_drawn, wait};

/// evdev's keys, as `wl_keyboard.key` numbers them: Q, A, 0, Y, Left Shift, and Right Alt, which
/// Yuna holds for AltGr where the layout puts level 3 on it.
const KEY_Q: u32 = 16;
const KEY_A: u32 = 30;
const KEY_0: u32 = 11;
const KEY_Y: u32 = 21;
const KEY_LEFT_SHIFT: u32 = 42;
const KEY_RIGHT_ALT: u32 = 100;
/// The real modifier XKB's level 3 is, `Mod5`, as `wl_keyboard.modifiers` tells it.
const MOD5: u32 = 1 << 7;

fn layout(layout: &str, variant: &str, options: &str) -> KeyboardLayout {
    KeyboardLayout {
        layout: layout.to_owned(),
        variant: variant.to_owned(),
        options: options.to_owned(),
    }
}

/// Agent seat A with a client whose window has its keyboard, its events read.
fn focused_client() -> (Compositor, SeatId, InputClient) {
    let (mut compositor, a, _) = two_seats();
    let mut client = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut client, &mut compositor);
    settle(&mut compositor);
    client.roundtrip(&mut compositor);
    client.take_events();
    (compositor, a, client)
}

fn keymaps(events: &[InputEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, InputEvent::Keymap))
        .count()
}

/// The keys pressed among `events`, as evdev numbers them.
fn presses(events: &[InputEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Key {
                key, pressed: true, ..
            } => Some(*key),
            _ => None,
        })
        .collect()
}

/// Changes the layout and reads what the client was sent: one keymap, of the new layout.
fn change_to(
    compositor: &mut Compositor,
    seat: SeatId,
    client: &mut InputClient,
    new: &KeyboardLayout,
) {
    compositor
        .set_keyboard_layout(seat, new.clone())
        .expect("the layout changes");
    client.roundtrip(compositor);
    assert_eq!(keymaps(&client.take_events()), 1, "the new keymap, once");
    assert_eq!(compositor.keyboard_layout(seat).as_ref(), Some(new));
}

/// French: `type "aq@"` types a and q with the keys that carry them there, us's Q and A, and @
/// with AltGr, Right Alt, and 0's key, all read through the French keymap the client was sent,
/// which no keymap of `type`'s own replaces.
#[test]
fn french_types_a_q_and_at_with_its_own_keys() {
    let (mut compositor, a, mut client) = focused_client();
    assert_eq!(client.keymap_layouts(), ["English (US)"]);
    change_to(&mut compositor, a, &mut client, &layout("fr", "", ""));
    assert_eq!(client.keymap_layouts(), ["French"]);

    compositor.type_text(a, "aq@").expect("text typed");
    client.roundtrip(&mut compositor);
    let events = client.take_events();
    assert_eq!(keymaps(&events), 0, "no keymap swap");
    assert_eq!(presses(&events), [KEY_Q, KEY_A, KEY_RIGHT_ALT, KEY_0]);
    assert_eq!(
        client.state.keysyms,
        [Keysym::a, Keysym::q, Keysym::ISO_Level3_Shift, Keysym::at]
    );
    assert_eq!(client.state.typed, "aq@");
    assert_eq!(client.state.received, "aq@", "nothing committed");
    assert!(events.contains(&InputEvent::Modifiers { depressed: MOD5 }));
    assert_eq!(events.last(), Some(&InputEvent::Modifiers { depressed: 0 }));
}

/// German: `type "@€"` holds AltGr for each, on Q's and E's keys.
#[test]
fn german_types_at_and_the_euro_through_altgr() {
    let (mut compositor, a, mut client) = focused_client();
    change_to(&mut compositor, a, &mut client, &layout("de", "", ""));
    compositor.type_text(a, "@€").expect("text typed");
    client.roundtrip(&mut compositor);
    let events = client.take_events();
    assert_eq!(keymaps(&events), 0, "no keymap swap");
    assert_eq!(
        client.state.keysyms,
        [
            Keysym::ISO_Level3_Shift,
            Keysym::at,
            Keysym::ISO_Level3_Shift,
            Keysym::EuroSign
        ]
    );
    assert_eq!(client.state.typed, "@€");
    assert!(events.contains(&InputEvent::Modifiers { depressed: MOD5 }));
}

/// us,ru with Alt+Shift to switch: a Cyrillic letter is typed on its key in the second group,
/// which Alt and Shift lock first; the first group is locked again before the next Latin letter,
/// and stays at the end.
#[test]
fn a_cyrillic_letter_is_typed_through_its_group() {
    let (mut compositor, a, mut client) = focused_client();
    let both = layout("us,ru", "", "grp:alt_shift_toggle");
    change_to(&mut compositor, a, &mut client, &both);
    assert_eq!(client.keymap_layouts(), ["English (US)", "Russian"]);
    client.state.groups.clear();

    compositor.type_text(a, "aфa").expect("text typed");
    client.roundtrip(&mut compositor);
    let events = client.take_events();
    assert_eq!(keymaps(&events), 0, "no keymap swap");
    assert_eq!(client.state.typed, "aфa");
    assert_eq!(
        client
            .state
            .keysyms
            .iter()
            .filter(|&&k| k == Keysym::Cyrillic_ef)
            .count(),
        1
    );
    assert!(
        client.state.groups.contains(&1),
        "{:?}",
        client.state.groups
    );
    assert_eq!(
        client.state.groups.last(),
        Some(&0),
        "the first group again"
    );

    // Typing that ends in the second group locks the first again.
    client.state.typed.clear();
    compositor.type_text(a, "ф").expect("text typed");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.typed, "ф");
    assert_eq!(client.state.groups.last(), Some(&0));
}

/// A layout XKB does not know, a name XKB's names never hold, too long a name, more than four
/// layouts, no layout, an empty one after a comma, an option XKB's rules do not name, and the
/// owner's seat: each refused, the keymap unchanged, no keymap sent,
/// and `type` still types us's keys.
#[test]
fn a_layout_xkb_does_not_know_is_refused_and_nothing_changes() {
    let (mut compositor, a, mut client) = focused_client();
    let long = "a".repeat(MAX_LAYOUT_NAME + 1);
    let refused = [
        layout("nosuchlayout", "", ""),
        layout("fr", "nosuchvariant", ""),
        layout("us,zz", "", ""),
        layout("../../etc", "", ""),
        layout("fr\0", "", ""),
        layout(&long, "", ""),
        layout("us,fr,de,ru,es", "", ""),
        layout("", "", ""),
        layout("fr,", "", ""),
        layout("us,ru", "", "grp:alt_shft_toggle"),
    ];
    for wrong in refused {
        let result = compositor.set_keyboard_layout(a, wrong.clone());
        assert!(result.is_err(), "{wrong:?}");
    }
    let unknown = compositor
        .set_keyboard_layout(a, layout("nosuchlayout", "", ""))
        .expect_err("refused")
        .to_string();
    assert!(
        unknown.contains("XKB has no layout \"nosuchlayout\""),
        "{unknown}"
    );
    let misspelt = compositor
        .set_keyboard_layout(a, layout("us,ru", "", "grp:alt_shft_toggle"))
        .expect_err("refused")
        .to_string();
    assert_eq!(
        misspelt,
        "XKB's evdev rules know no option \"grp:alt_shft_toggle\""
    );
    assert!(
        compositor
            .set_keyboard_layout(OWNER_SEAT, layout("fr", "", ""))
            .is_err()
    );
    assert!(compositor.keyboard_layout(OWNER_SEAT).is_none());

    client.roundtrip(&mut compositor);
    assert_eq!(keymaps(&client.take_events()), 0, "no keymap sent");
    assert_eq!(
        compositor.keyboard_layout(a),
        Some(KeyboardLayout::agents_default())
    );
    compositor.type_text(a, "a").expect("text typed");
    client.roundtrip(&mut compositor);
    assert_eq!(presses(&client.take_events()), [KEY_A]);
    assert_eq!(client.state.typed, "a");
}

/// A change while the seat holds a key, Yuna's own or a virtual keyboard's, is refused, the
/// keymap unchanged; once the keys are released, it is made.
#[test]
fn a_change_while_a_key_is_held_is_refused() {
    let (mut compositor, a, mut client) = focused_client();
    let fr = layout("fr", "", "");
    compositor.key(a, "a", true).expect("a press");
    client.roundtrip(&mut compositor);
    client.take_events();
    let held = compositor
        .set_keyboard_layout(a, fr.clone())
        .expect_err("refused")
        .to_string();
    assert_eq!(
        held,
        "the seat holds a key: release it before changing the layout"
    );
    client.roundtrip(&mut compositor);
    assert_eq!(keymaps(&client.take_events()), 0, "no keymap sent");
    assert_eq!(client.keymap_layouts(), ["English (US)"]);
    compositor.key(a, "a", false).expect("a release");

    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let keyboard = driver.add_keyboard("agent-2", "us");
    driver.key(keyboard, KEY_Y, true);
    driver.roundtrip(&mut compositor);
    assert!(compositor.set_keyboard_layout(a, fr.clone()).is_err());
    driver.key(keyboard, KEY_Y, false);
    driver.roundtrip(&mut compositor);
    assert_eq!(
        compositor.keyboard_layout(a),
        Some(KeyboardLayout::agents_default())
    );

    compositor
        .set_keyboard_layout(a, fr.clone())
        .expect("no key held");
    assert_eq!(compositor.keyboard_layout(a), Some(fr));
    client.roundtrip(&mut compositor);
    assert_eq!(client.keymap_layouts(), ["French"]);
}

/// ADR-0089 after a change: a virtual keyboard's keys are read with its own `us` keymap, then
/// Yuna's own keys switch the seat's keyboard back to the new German one, not to `us`; a keyboard
/// with `no_keymap` reads its keys with the German one too.
#[test]
fn a_virtual_keyboards_keymap_gives_way_to_the_new_layout() {
    let (mut compositor, a, mut client) = focused_client();
    change_to(&mut compositor, a, &mut client, &layout("de", "", ""));
    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let us = driver.add_keyboard("agent-2", "us");
    driver.key(us, KEY_Y, true);
    driver.key(us, KEY_Y, false);
    driver.roundtrip(&mut compositor);
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.typed, "y", "Y's key in us");
    assert_eq!(client.keymap_layouts(), ["English (US)"]);
    client.take_events();

    // z is on Y's key in German.
    compositor.type_text(a, "z").expect("text typed");
    client.roundtrip(&mut compositor);
    let events = client.take_events();
    assert_eq!(keymaps(&events), 1, "the seat's keymap again");
    assert_eq!(presses(&events), [KEY_Y]);
    assert_eq!(client.keymap_layouts(), ["German"]);
    assert_eq!(client.state.typed, "yz");

    let plain = driver.add_keyboard_without_keymap("agent-2");
    driver.key(plain, KEY_Y, true);
    driver.key(plain, KEY_Y, false);
    driver.roundtrip(&mut compositor);
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.typed, "yzz");
}

/// Every client of the seat is sent the new keymap, one whose window has no keyboard focus too.
#[test]
fn every_client_of_the_seat_gets_the_new_keymap() {
    let (mut compositor, a, mut focused) = focused_client();
    let mut unfocused = InputClient::connect(&mut compositor, a, "agent-2");
    unfocused.take_events();
    assert_ne!(
        compositor.keyboard_window(a),
        None,
        "the first client's window has the keyboard"
    );
    compositor
        .set_keyboard_layout(a, layout("fr", "", ""))
        .expect("the layout changes");
    for client in [&mut focused, &mut unfocused] {
        client.roundtrip(&mut compositor);
        assert_eq!(keymaps(&client.take_events()), 1);
        assert_eq!(client.keymap_layouts(), ["French"]);
    }
}

/// A tap of `parenleft` types `(`, with the keypad's key that types it alone, and not 9
/// without its Shift; `type` types it with Shift and 9.
#[test]
fn a_tap_of_a_parenthesis_types_it() {
    let (mut compositor, a, mut client) = focused_client();
    compositor.tap_keys(a, "parenleft").expect("a tap");
    compositor.key(a, "parenright", true).expect("a press");
    compositor.key(a, "parenright", false).expect("a release");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.typed, "()");
    client.take_events();
    compositor.type_text(a, "(").expect("text typed");
    client.roundtrip(&mut compositor);
    assert_eq!(presses(&client.take_events()), [KEY_LEFT_SHIFT, 10]);
    assert_eq!(client.state.typed, "()(");
}

/// A key pressed by its name in one group is released by the same name after another group is
/// locked, where that name is another key's: us's a, on A, is let go once French is in effect,
/// where a is on Q.
#[test]
fn a_key_pressed_in_one_group_is_released_in_another() {
    let (mut compositor, a, mut client) = focused_client();
    let both = layout("us,fr", "", "grp:alt_shift_toggle");
    change_to(&mut compositor, a, &mut client, &both);
    compositor.key(a, "a", true).expect("a press");
    compositor.tap_keys(a, "alt+shift").expect("French locked");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.groups.last(), Some(&1));
    client.take_events();
    compositor
        .key(a, "a", false)
        .expect("the key held is released");
    client.roundtrip(&mut compositor);
    let released: Vec<u32> = client
        .take_events()
        .iter()
        .filter_map(|event| match event {
            InputEvent::Key {
                key,
                pressed: false,
                ..
            } => Some(*key),
            _ => None,
        })
        .collect();
    assert_eq!(released, [KEY_A]);
    assert!(compositor.key(a, "a", false).is_err(), "nothing held");
}

/// Characters no key types, typed through a keymap of their own, between two letters of the
/// second group: the second group stays in effect across them, and the first is locked again
/// at the end.
#[test]
fn injected_characters_between_group_switches_keep_the_group() {
    let (mut compositor, a, mut client) = focused_client();
    let both = layout("us,ru", "", "grp:alt_shift_toggle");
    change_to(&mut compositor, a, &mut client, &both);
    client.state.groups.clear();
    compositor.type_text(a, "ф😀ф").expect("text typed");
    client.roundtrip(&mut compositor);
    let events = client.take_events();
    assert_eq!(client.state.typed, "ф😀ф");
    assert_eq!(
        keymaps(&events),
        2,
        "the run's keymap, then the seat's again"
    );
    assert_eq!(client.keymap_layouts(), ["English (US)", "Russian"]);
    assert_eq!(
        client.state.groups.last(),
        Some(&0),
        "the first group again"
    );
}

/// With Caps Lock on, a small letter is typed with Shift and a capital without, as the client
/// reads them; with Shift held, a capital is its key alone, and a small letter, which no key
/// types then, comes through a keymap of its own. Each types as written.
#[test]
fn caps_lock_on_or_shift_held_still_type_as_written() {
    let (mut compositor, a, mut client) = focused_client();
    compositor.tap_keys(a, "Caps_Lock").expect("Caps Lock on");
    client.roundtrip(&mut compositor);
    client.take_events();
    client.state.typed.clear();
    compositor.type_text(a, "aA").expect("text typed");
    client.roundtrip(&mut compositor);
    let events = client.take_events();
    assert_eq!(keymaps(&events), 0, "keys alone");
    assert_eq!(presses(&events), [KEY_LEFT_SHIFT, KEY_A, KEY_A]);
    assert_eq!(client.state.typed, "aA");
    compositor.tap_keys(a, "Caps_Lock").expect("Caps Lock off");

    compositor.key(a, "Shift_L", true).expect("Shift held");
    client.roundtrip(&mut compositor);
    client.take_events();
    client.state.typed.clear();
    compositor.type_text(a, "Aa").expect("text typed");
    client.roundtrip(&mut compositor);
    let events = client.take_events();
    assert_eq!(client.state.typed, "Aa");
    assert_eq!(presses(&events)[0], KEY_A, "A's key alone");
    assert_eq!(
        keymaps(&events),
        2,
        "a through a keymap of its own, then the seat's"
    );
    compositor.key(a, "Shift_L", false).expect("Shift released");
}

/// Another seat's keyboard keeps its layout: a change is the seat's own.
#[test]
fn a_change_is_the_seats_own() {
    let (mut compositor, a, b) = two_seats();
    let mut on_b = InputClient::connect(&mut compositor, b, "agent-3");
    open(&mut on_b, &mut compositor);
    settle(&mut compositor);
    on_b.roundtrip(&mut compositor);
    on_b.take_events();
    compositor
        .set_keyboard_layout(a, layout("fr", "", ""))
        .expect("the layout changes");
    on_b.roundtrip(&mut compositor);
    assert_eq!(keymaps(&on_b.take_events()), 0);
    assert_eq!(
        compositor.keyboard_layout(b),
        Some(KeyboardLayout::agents_default())
    );
    let window = windows_of(&compositor, b)[0];
    click(&mut compositor, b, window, yuna::BUTTON_LEFT);
    settle(&mut compositor);
    compositor.type_text(b, "a").expect("text typed");
    on_b.roundtrip(&mut compositor);
    assert_eq!(presses(&on_b.take_events()), [KEY_A]);
}

/// The first keysym X11 maps `keycode` to, in the core keyboard mapping.
fn first_keysym(client: &X11Client, keycode: u8) -> Result<u32, String> {
    let reply = client
        .connection
        .get_keyboard_mapping(keycode, 1)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    reply
        .keysyms
        .first()
        .copied()
        .ok_or_else(|| format!("no keysym for keycode {keycode}"))
}

/// X11's keycode of Q's key, which types a in French.
const X11_KEY_Q: u8 = 24;

/// An X11 client reads the new keymap: Xwayland maps Q's key to a once the seat is French, and a
/// key Yuna types then reaches the X11 window as that key.
#[test]
fn an_x11_client_reads_the_new_layout() {
    let mut seat = X11Seat::new("layout", &[at_1()], ClockMode::Real, off());
    let output = seat.outputs[0];
    let client = seat.client();
    let (client, us_q) = step(&mut seat.compositor, client, |client| {
        first_keysym(client, X11_KEY_Q)
    });
    assert_eq!(us_q, Keysym::q.raw(), "us first");
    let (client, window, described, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("layout"));
    click_at(&mut seat, output, TILE.x + 100.0, TILE.y + 50.0);
    let (client, _) = button_press_at(&mut seat.compositor, client, window);
    let seat_id = seat.seat;
    wait(
        &mut seat.compositor,
        "the window has the keyboard",
        |compositor| (compositor.keyboard_window(seat_id) == Some(described.id)).then_some(()),
    );

    seat.compositor
        .set_keyboard_layout(seat.seat, layout("fr", "", ""))
        .expect("the layout changes");
    let (client, ()) = step(&mut seat.compositor, client, |client| {
        let deadline = std::time::Instant::now() + PATIENCE;
        while first_keysym(client, X11_KEY_Q)? != Keysym::a.raw() {
            if std::time::Instant::now() > deadline {
                return Err("Xwayland never mapped Q's key to a".to_owned());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(())
    });

    seat.compositor.type_text(seat.seat, "a").expect("typed");
    let (_client, key) = step(&mut seat.compositor, client, move |client| {
        match client.wait_for(
            "a key press",
            |event| matches!(event, Event::KeyPress(press) if press.event == window),
        )? {
            Event::KeyPress(press) => Ok(press.detail),
            _ => unreachable!("a key press"),
        }
    });
    assert_eq!(key, X11_KEY_Q, "a on Q's key, as in French");
}

/// The text of a failed call.
fn failure(result: &Value) -> &str {
    assert_eq!(result["isError"], json!(true), "{result}");
    result["content"][0]["text"].as_str().unwrap_or_default()
}

/// Through MCP: `keyboard layout` changes the seat's layout and tells it, `type` types through
/// it, and a layout XKB does not know, or no layout, is refused with a plain message.
#[test]
fn the_layout_changes_through_mcp() {
    let mut mcp = Mcp::new(ClockMode::Real);
    let a = mcp.seat();
    let mut client = InputClient::connect(mcp.server.compositor(), SeatId(a), "agent-2");
    let window = client.client.create_window();
    client.client.commit(window);
    client.roundtrip(mcp.server.compositor());
    client.client.ack(window);
    client.client.attach(window, 400, 300, crate::RED);
    client.client.commit(window);
    client.roundtrip(mcp.server.compositor());
    let found = mcp.call("windows", json!({"seat": a, "wait_ms": 5_000}));
    let id = found["structuredContent"]["windows"][0]["id"].clone();
    mcp.call(
        "pointer",
        json!({"action": "click", "seat": a, "x": 26, "y": 26}),
    );

    let changed = mcp.call(
        "keyboard",
        json!({"action": "layout", "seat": a, "layout": "us,ru",
            "options": "grp:alt_shift_toggle"}),
    );
    assert_eq!(
        changed["structuredContent"],
        json!({"keyboard_window": id, "text_field": false, "layout": "us,ru", "variant": "",
            "options": "grp:alt_shift_toggle"}),
        "{changed}"
    );
    let typed = mcp.call(
        "keyboard",
        json!({"action": "type", "seat": a, "text": "hi ф"}),
    );
    assert_eq!(typed["isError"], json!(false), "{typed}");
    client.roundtrip(mcp.server.compositor());
    assert_eq!(client.keymap_layouts(), ["English (US)", "Russian"]);
    assert_eq!(client.state.typed, "hi ф");

    let unknown = mcp.call(
        "keyboard",
        json!({"action": "layout", "seat": a, "layout": "nosuchlayout"}),
    );
    assert!(
        failure(&unknown).contains("XKB has no layout \"nosuchlayout\""),
        "{unknown}"
    );
    let missing = mcp.call("keyboard", json!({"action": "layout", "seat": a}));
    failure(&missing);
    let kept = mcp.call(
        "keyboard",
        json!({"action": "layout", "seat": a, "layout": "fr", "variant": "nosuchvariant"}),
    );
    failure(&kept);
    let typed = mcp.call(
        "keyboard",
        json!({"action": "type", "seat": a, "text": "a"}),
    );
    assert_eq!(typed["isError"], json!(false));
    client.roundtrip(mcp.server.compositor());
    assert_eq!(client.state.typed, "hi фa", "us,ru still");
    assert_eq!(
        mcp.server.compositor().keyboard_layout(SeatId(a)),
        Some(layout("us,ru", "", "grp:alt_shift_toggle"))
    );
}
