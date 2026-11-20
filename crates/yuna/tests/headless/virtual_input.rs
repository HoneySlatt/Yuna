//! The virtual keyboard and pointer of agents' seats
//! ([ADR-0089](../../../../docs/adr/0089-yuna-for-the-owners-other-projects.md),
//! [ADR-0078](../../../../docs/adr/0078-agents-seats-in-one-yuna-frozen-until-stage-2s-core.md)):
//! a driver drives its own seat's windows as Yuna's own input does, and never the owner's seat,
//! nor another agent's.
//!
//! The compositor is `seats.rs`'s: the owner's seat with its 4K output, and two agents' seats,
//! `agent-2` and `agent-3`, each with an output of 1920 × 1080 at scale 1.

use yuna::{BUTTON_LEFT, Compositor, MAX_VIRTUAL_DEVICES_PER_CLIENT, OWNER_SEAT};
use yuna_types::{SeatId, WindowId};

use crate::input_client::{InputClient, InputEvent};
use crate::seats::{open, settle, two_seats, windows_of};
use crate::virtual_input_client::VirtualDriver;

const KEYBOARD_MANAGER: &str = "zwp_virtual_keyboard_manager_v1";
const POINTER_MANAGER: &str = "zwlr_virtual_pointer_manager_v1";
/// evdev's keys: X, and Q, which types `a` in a French layout.
const KEY_X: u32 = 45;
const KEY_Q: u32 = 16;

/// Where a point 10 units inside `window`'s top-left corner is, as a fraction of its seat's
/// outputs together, which are one output of 1920 × 1080 logical units here: what a driver's
/// absolute motion sends.
fn inside(compositor: &Compositor, seat: SeatId, window: WindowId) -> (u32, u32) {
    let rect = compositor
        .windows()
        .into_iter()
        .find(|description| description.id == window)
        .and_then(|description| description.rect)
        .expect("a placed window");
    let output = compositor
        .output_descriptions()
        .into_iter()
        .find(|description| description.seat == seat)
        .expect("the seat's output")
        .rect;
    (
        (rect.x - output.x + 10.0) as u32,
        (rect.y - output.y + 10.0) as u32,
    )
}

const EXTENT: (u32, u32) = (1920, 1080);

fn presses(events: &[InputEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, InputEvent::Button { pressed: true, .. }))
        .count()
}

fn no_input(events: &[InputEvent]) -> bool {
    events
        .iter()
        .all(|event| matches!(event, InputEvent::Keymap | InputEvent::Modifiers { .. }))
}

/// ADR-0058: the owner's seat is never offered the virtual keyboard or pointer, nor a client of
/// it shown every seat's globals; an agent's seat's clients are offered both.
#[test]
fn the_owners_seat_is_offered_no_virtual_input() {
    let (mut compositor, a, _) = two_seats();
    let owner = VirtualDriver::connect(&mut compositor, OWNER_SEAT);
    let owner_spy = VirtualDriver::connect_spy(&mut compositor, OWNER_SEAT);
    let agent = VirtualDriver::connect(&mut compositor, a);
    for (name, driver) in [("the owner's", &owner), ("the owner's spy", &owner_spy)] {
        for global in [KEYBOARD_MANAGER, POINTER_MANAGER] {
            assert!(
                !driver.state.globals.iter().any(|seen| seen == global),
                "{name} client is offered {global}"
            );
        }
    }
    for global in [KEYBOARD_MANAGER, POINTER_MANAGER] {
        assert!(
            agent.state.globals.iter().any(|seen| seen == global),
            "{global}"
        );
    }
}

/// A driver of seat A clicks, scrolls and types in A's window, which gets each event once; B's
/// client and the owner's get nothing. A virtual keyboard's keys are read with its own keymap,
/// French here, and Yuna's own keys with the seat's `us` again, in turn.
#[test]
fn a_virtual_keyboard_and_pointer_drive_their_own_seat_alone() {
    let (mut compositor, a, b) = two_seats();
    let mut on_a = InputClient::connect(&mut compositor, a, "agent-2");
    let mut on_b = InputClient::connect(&mut compositor, b, "agent-3");
    let mut owner = InputClient::connect(&mut compositor, OWNER_SEAT, "seat0");
    open(&mut on_a, &mut compositor);
    open(&mut on_b, &mut compositor);
    open(&mut owner, &mut compositor);
    settle(&mut compositor);
    let window_a = windows_of(&compositor, a)[0];
    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let pointer = driver.add_pointer(None);
    let keyboard = driver.add_keyboard("agent-2", "fr");
    for client in [&mut on_a, &mut on_b, &mut owner] {
        client.roundtrip(&mut compositor);
        client.take_events();
    }

    driver.move_to(pointer, inside(&compositor, a, window_a), EXTENT);
    driver.button(pointer, BUTTON_LEFT, true);
    driver.button(pointer, BUTTON_LEFT, false);
    driver.wheel(pointer, 1);
    driver.roundtrip(&mut compositor);
    settle(&mut compositor);
    driver.key(keyboard, KEY_Q, true);
    driver.key(keyboard, KEY_Q, false);
    driver.roundtrip(&mut compositor);
    compositor.tap_keys(a, "x").expect("Yuna's own key");
    driver.key(keyboard, KEY_Q, true);
    driver.key(keyboard, KEY_Q, false);
    driver.roundtrip(&mut compositor);

    for client in [&mut on_a, &mut on_b, &mut owner] {
        client.roundtrip(&mut compositor);
    }
    let events_a = on_a.take_events();
    assert_eq!(presses(&events_a), 1, "{events_a:?}");
    assert!(
        events_a
            .iter()
            .any(|event| matches!(event, InputEvent::Value120 { steps: 120, .. })),
        "{events_a:?}"
    );
    assert_eq!(on_a.state.typed, "axa");
    assert_eq!(compositor.keyboard_window(a), Some(window_a));
    for (name, client) in [("B", &mut on_b), ("the owner", &mut owner)] {
        let events = client.take_events();
        assert!(no_input(&events), "{name}: {events:?}");
        assert_eq!(client.state.typed, "", "{name}");
    }
    compositor
        .check_popups_and_keyboard()
        .expect("the keyboards are sound");
}

/// A spy of seat A, shown B's `wl_seat`, makes a keyboard and a pointer for it: they do nothing,
/// on B or on A. Relative motion moves from where the pointer is, and stops at the edge of the
/// seat's outputs, as wlroots' cursor does; a seat's pointer that never moved starts at its
/// first output's top-left corner, so that relative moves on B, whose output is right of A's,
/// stay on B.
#[test]
fn a_device_for_another_seat_does_nothing_and_motion_stays_on_its_seat() {
    let (mut compositor, a, b) = two_seats();
    let mut on_a = InputClient::connect(&mut compositor, a, "agent-2");
    let mut on_b = InputClient::connect(&mut compositor, b, "agent-3");
    open(&mut on_a, &mut compositor);
    open(&mut on_b, &mut compositor);
    settle(&mut compositor);
    let window_a = windows_of(&compositor, a)[0];
    let window_b = windows_of(&compositor, b)[0];
    compositor
        .change_layout(|layout| layout.focus_window(window_b))
        .expect("B's window has B's keyboard");
    settle(&mut compositor);

    let mut spy = VirtualDriver::connect_spy(&mut compositor, a);
    let keyboard = spy.add_keyboard("agent-3", "us");
    let pointer = spy.add_pointer(Some("agent-3"));
    for client in [&mut on_a, &mut on_b] {
        client.roundtrip(&mut compositor);
        client.take_events();
    }
    spy.move_to(pointer, inside(&compositor, b, window_b), EXTENT);
    spy.button(pointer, BUTTON_LEFT, true);
    spy.button(pointer, BUTTON_LEFT, false);
    spy.key(keyboard, KEY_X, true);
    spy.key(keyboard, KEY_X, false);
    spy.roundtrip(&mut compositor);
    for (name, client) in [("A", &mut on_a), ("B", &mut on_b)] {
        client.roundtrip(&mut compositor);
        let events = client.take_events();
        assert!(no_input(&events), "{name}: {events:?}");
    }

    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let own = driver.add_pointer(Some("agent-2"));
    let (x, y) = inside(&compositor, a, window_a);
    driver.move_to(own, (x, y), EXTENT);
    driver.move_by(own, 5.0, 7.0);
    driver.move_by(own, -10_000.0, 0.0);
    driver.roundtrip(&mut compositor);
    let at = compositor.pointer(a);
    let output = compositor
        .output_descriptions()
        .into_iter()
        .find(|description| description.seat == a)
        .expect("A's output");
    assert_eq!(at.output, Some(output.id));
    assert_eq!((at.x, at.y), (0.0, f64::from(y) + 7.0));

    let mut on_b_driver = VirtualDriver::connect(&mut compositor, b);
    let relative = on_b_driver.add_pointer(None);
    on_b_driver.move_by(relative, 30.0, 40.0);
    on_b_driver.roundtrip(&mut compositor);
    let at = compositor.pointer(b);
    let output_b = compositor
        .output_descriptions()
        .into_iter()
        .find(|description| description.seat == b)
        .expect("B's output");
    assert_eq!(at.output, Some(output_b.id));
    assert_eq!((at.x, at.y), (30.0, 40.0));
}

/// A driver that leaves with a button and a key held lets them go: its window gets their
/// releases, and the seat holds nothing after.
#[test]
fn a_driver_that_leaves_releases_what_it_held() {
    let (mut compositor, a, _) = two_seats();
    let mut on_a = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut on_a, &mut compositor);
    settle(&mut compositor);
    let window_a = windows_of(&compositor, a)[0];
    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let pointer = driver.add_pointer(None);
    let keyboard = driver.add_keyboard("agent-2", "us");
    driver.move_to(pointer, inside(&compositor, a, window_a), EXTENT);
    driver.button(pointer, BUTTON_LEFT, true);
    driver.key(keyboard, KEY_X, true);
    driver.roundtrip(&mut compositor);
    on_a.roundtrip(&mut compositor);
    on_a.take_events();

    drop(driver);
    compositor
        .dispatch(None)
        .expect("Yuna reads the driver leave");
    on_a.roundtrip(&mut compositor);
    let events = on_a.take_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, InputEvent::Button { pressed: false, .. })),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, InputEvent::Key { pressed: false, .. })),
        "{events:?}"
    );
    // Yuna's own press of the same button is no "held already": the seat holds nothing.
    compositor
        .pointer_button(a, BUTTON_LEFT, true)
        .expect("the button was released");
}

/// A client's devices past [`MAX_VIRTUAL_DEVICES_PER_CLIENT`] do nothing; those before act.
#[test]
fn devices_past_a_clients_cap_do_nothing() {
    let (mut compositor, a, _) = two_seats();
    let mut on_a = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut on_a, &mut compositor);
    settle(&mut compositor);
    let window_a = windows_of(&compositor, a)[0];
    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let pointers: Vec<usize> = (0..=MAX_VIRTUAL_DEVICES_PER_CLIENT)
        .map(|_| driver.add_pointer(None))
        .collect();
    driver.move_to(pointers[0], inside(&compositor, a, window_a), EXTENT);
    driver.roundtrip(&mut compositor);
    on_a.roundtrip(&mut compositor);
    on_a.take_events();

    let past = *pointers.last().expect("a pointer past the cap");
    driver.button(past, BUTTON_LEFT, true);
    driver.button(past, BUTTON_LEFT, false);
    driver.roundtrip(&mut compositor);
    on_a.roundtrip(&mut compositor);
    assert_eq!(presses(&on_a.take_events()), 0);

    let last_acting = pointers[MAX_VIRTUAL_DEVICES_PER_CLIENT - 1];
    driver.button(last_acting, BUTTON_LEFT, true);
    driver.button(last_acting, BUTTON_LEFT, false);
    driver.destroy_pointer(past);
    driver.roundtrip(&mut compositor);
    on_a.roundtrip(&mut compositor);
    assert_eq!(presses(&on_a.take_events()), 1);
}

/// A keymap whose size is told larger than its file, or larger than any keymap, is refused, and
/// Yuna serves on: it never reads past the client's file, which would end Yuna with SIGBUS, and
/// another keyboard of the same driver types.
#[test]
fn a_keymap_told_larger_than_its_file_is_refused() {
    let (mut compositor, a, _) = two_seats();
    let mut on_a = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut on_a, &mut compositor);
    settle(&mut compositor);
    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let lying = driver.add_keyboard_claiming("agent-2", "us", Some(1 << 20));
    let huge = driver.add_keyboard_claiming("agent-2", "us", Some(u32::MAX));
    driver.roundtrip(&mut compositor);
    for keyboard in [lying, huge] {
        driver.key(keyboard, KEY_X, true);
        driver.key(keyboard, KEY_X, false);
    }
    // The protocol ends a client whose keyboard has no keymap and sends a key: the driver's
    // own error, not Yuna's.
    let _ = driver.client.try_roundtrip(&mut compositor);
    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let keyboard = driver.add_keyboard("agent-2", "us");
    driver.key(keyboard, KEY_X, true);
    driver.key(keyboard, KEY_X, false);
    driver.roundtrip(&mut compositor);
    on_a.roundtrip(&mut compositor);
    assert_eq!(on_a.state.typed, "x");
}

/// XKB's modifier masks in a `us` keymap: Shift and Caps Lock.
const SHIFT: u32 = 1;
const CAPS_LOCK: u32 = 2;

/// A virtual keyboard's `modifiers` request sets its seat's modifiers, as `pluto-input` sends
/// Shift around a key; a keyboard that leaves with Caps Lock on takes it with it, and Yuna's own
/// keys type small letters again. A keyboard with `no_keymap` reads its keys with the seat's own
/// keymap, whatever keymap another keyboard left.
#[test]
fn modifiers_go_with_their_keyboard_and_no_keymap_is_the_seats() {
    let (mut compositor, a, _) = two_seats();
    let mut on_a = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut on_a, &mut compositor);
    settle(&mut compositor);
    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let keyboard = driver.add_keyboard("agent-2", "us");
    driver.modifiers(keyboard, SHIFT, 0, 0);
    driver.key(keyboard, KEY_X, true);
    driver.key(keyboard, KEY_X, false);
    driver.modifiers(keyboard, 0, 0, 0);
    driver.key(keyboard, KEY_X, true);
    driver.key(keyboard, KEY_X, false);
    driver.modifiers(keyboard, 0, 0, CAPS_LOCK);
    driver.roundtrip(&mut compositor);
    driver.destroy_keyboard(keyboard);
    driver.roundtrip(&mut compositor);
    compositor.tap_keys(a, "x").expect("Yuna's own key");
    on_a.roundtrip(&mut compositor);
    assert_eq!(on_a.state.typed, "Xxx");

    let french = driver.add_keyboard("agent-2", "fr");
    driver.key(french, KEY_Q, true);
    driver.key(french, KEY_Q, false);
    let plain = driver.add_keyboard_without_keymap("agent-2");
    driver.key(plain, KEY_Q, true);
    driver.key(plain, KEY_Q, false);
    driver.roundtrip(&mut compositor);
    on_a.roundtrip(&mut compositor);
    assert_eq!(on_a.state.typed, "Xxxaq");
}

/// A keyboard that sends a second keymap has its keys read with it from then on.
#[test]
fn a_keyboards_new_keymap_reads_its_next_keys() {
    let (mut compositor, a, _) = two_seats();
    let mut on_a = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut on_a, &mut compositor);
    settle(&mut compositor);
    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let keyboard = driver.add_keyboard("agent-2", "us");
    driver.key(keyboard, KEY_Q, true);
    driver.key(keyboard, KEY_Q, false);
    driver.change_keymap(keyboard, "fr");
    driver.key(keyboard, KEY_Q, true);
    driver.key(keyboard, KEY_Q, false);
    driver.roundtrip(&mut compositor);
    on_a.roundtrip(&mut compositor);
    assert_eq!(on_a.state.typed, "qa");
}

/// An absolute motion at the very end of its extent lands on the seat's output's last unit, not
/// past it, where no output is.
#[test]
fn an_absolute_motion_at_the_extents_end_stays_on_the_output() {
    let (mut compositor, a, _) = two_seats();
    let mut driver = VirtualDriver::connect(&mut compositor, a);
    let pointer = driver.add_pointer(None);
    driver.move_to(pointer, EXTENT, EXTENT);
    driver.roundtrip(&mut compositor);
    let at = compositor.pointer(a);
    assert!(at.output.is_some(), "{at:?}");
    assert_eq!((at.x, at.y), (1919.0, 1079.0));
}
