//! Text fields through `text-input-v3`, Yuna being the input method of each agent's seat
//! ([ADR-0043](../../../../docs/adr/0043-text-input-and-the-rest-of-slimeengines-examples.md)):
//! a field is entered with its window's keyboard and left with it; once enabled, a character
//! outside `us` is committed to it rather than typed through a keymap of its own, text being
//! composed shows until the next text typed, and no seat's text reaches another's field. The
//! owner's seat has no input method: its fields are never entered, and it takes no text. Text
//! holding a NUL, and a key name holding one, are refused.

use yuna::{BUTTON_LEFT, MAX_TEXT_BYTES, OWNER_SEAT};

use crate::input_client::{InputClient, InputEvent, TextEvent};
use crate::seats::{click, open, settle, two_seats, windows_of};

/// The keymaps among `events`: one for each run typed through an injected keymap, and one for
/// the seat's keymap given back after it.
fn keymaps(events: &[InputEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, InputEvent::Keymap))
        .count()
}

/// A client of seat A, its window open and focused, its text field entered.
fn focused_field() -> (yuna::Compositor, yuna::yuna_layout::SeatId, InputClient) {
    let (mut compositor, a, _) = two_seats();
    let mut client = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut client, &mut compositor);
    settle(&mut compositor);
    client.roundtrip(&mut compositor);
    assert!(
        matches!(client.state.text_events.as_slice(), [TextEvent::Enter(_)]),
        "{:?}",
        client.state.text_events
    );
    (compositor, a, client)
}

/// Enabled, the field takes the characters outside `us` as committed text, between the keys of
/// the others, in order, with no keymap change; disabled, they go through an injected keymap
/// again.
#[test]
fn an_enabled_field_takes_text_outside_us_committed_and_keys_for_the_rest() {
    let (mut compositor, a, mut client) = focused_field();
    assert!(!compositor.text_field_enabled(a), "not enabled yet");
    client.enable_text_field(true);
    client.roundtrip(&mut compositor);
    assert!(compositor.text_field_enabled(a));
    client.take_events();
    client.state.text_events.clear();

    compositor.type_text(a, "Grüße, 日本!").expect("text typed");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.received, "Grüße, 日本!");
    assert_eq!(client.state.typed, "Gre, !", "the keys typed us alone");
    assert_eq!(keymaps(&client.take_events()), 0, "no keymap change");
    assert_eq!(
        client.state.text_events,
        [
            // Any text being composed cleared first.
            TextEvent::Done(1),
            TextEvent::Commit(Some("üß".to_owned())),
            TextEvent::Done(1),
            TextEvent::Commit(Some("日本".to_owned())),
            TextEvent::Done(1),
        ]
    );

    client.enable_text_field(false);
    client.roundtrip(&mut compositor);
    assert!(!compositor.text_field_enabled(a));
    client.state.received.clear();
    compositor.type_text(a, "ç").expect("text typed");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.received, "ç");
    assert_eq!(
        keymaps(&client.take_events()),
        2,
        "its keymap, then the seat's"
    );
}

/// Text being composed shows in the field, its cursor at its end, until text typed is committed
/// in its place or empty text clears it; it is refused with no field enabled, and past
/// [`MAX_TEXT_BYTES`]. A long run is committed in pieces that each fit a message.
#[test]
fn text_composed_shows_until_text_is_committed_and_long_text_goes_in_pieces() {
    let (mut compositor, a, mut client) = focused_field();
    assert!(
        compositor.compose_text(a, "にほ").is_err(),
        "no field enabled"
    );
    client.enable_text_field(true);
    client.roundtrip(&mut compositor);

    compositor.compose_text(a, "にほ").expect("text composed");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.preedit.as_deref(), Some("にほ"));
    assert!(client.state.text_events.contains(&TextEvent::Preedit {
        text: Some("にほ".to_owned()),
        begin: 6,
        end: 6,
    }));
    compositor.type_text(a, "日本").expect("text typed");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.preedit, None, "the commit replaced it");
    assert_eq!(client.state.received, "日本");

    compositor.compose_text(a, "x").expect("text composed");
    compositor.compose_text(a, "").expect("cleared");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.preedit, None);
    assert!(
        compositor
            .compose_text(a, &"é".repeat(MAX_TEXT_BYTES / 2 + 1))
            .is_err()
    );

    // 2,100 characters of 2 bytes: 4,200 bytes, in two pieces.
    client.state.received.clear();
    client.state.text_events.clear();
    let long = "é".repeat(2_100);
    compositor.type_text(a, &long).expect("text typed");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.received, long);
    let pieces: Vec<usize> = client
        .state
        .text_events
        .iter()
        .filter_map(|event| match event {
            TextEvent::Commit(Some(text)) => Some(text.len()),
            _ => None,
        })
        .collect();
    assert_eq!(pieces, [MAX_TEXT_BYTES, 4_200 - MAX_TEXT_BYTES]);
}

/// The field follows its window's keyboard: left when another window takes it, and no longer
/// enabled there; entered again when its window takes it back, to be enabled again.
#[test]
fn the_field_is_left_with_the_keyboard_and_entered_again_with_it() {
    let (mut compositor, a, mut client) = focused_field();
    client.enable_text_field(true);
    client.roundtrip(&mut compositor);
    let first = windows_of(&compositor, a)[0];

    let mut other = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut other, &mut compositor);
    settle(&mut compositor);
    let second = windows_of(&compositor, a)[1];
    click(&mut compositor, a, second, BUTTON_LEFT);
    settle(&mut compositor);
    client.roundtrip(&mut compositor);
    other.roundtrip(&mut compositor);
    assert!(matches!(
        client.state.text_events.last(),
        Some(TextEvent::Leave(_))
    ));
    assert!(matches!(
        other.state.text_events.as_slice(),
        [TextEvent::Enter(_)]
    ));
    assert!(
        !compositor.text_field_enabled(a),
        "the other field is not enabled"
    );

    click(&mut compositor, a, first, BUTTON_LEFT);
    settle(&mut compositor);
    client.roundtrip(&mut compositor);
    assert!(matches!(
        client.state.text_events.last(),
        Some(TextEvent::Enter(_))
    ));
    client.enable_text_field(true);
    client.roundtrip(&mut compositor);
    assert!(compositor.text_field_enabled(a));
}

/// No text crosses seats: a client of B that holds a field of A's `wl_seat` is never entered and
/// gets nothing of A's text; a client of the owner's seat, which has no input method, is never
/// entered either.
#[test]
fn no_seats_text_reaches_another_seats_field() {
    let (mut compositor, a, b) = two_seats();
    let mut own = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut own, &mut compositor);
    let mut spy = InputClient::connect_spy(&mut compositor, b, "agent-2");
    open(&mut spy, &mut compositor);
    let mut owners = InputClient::connect(&mut compositor, OWNER_SEAT, "seat0");
    open(&mut owners, &mut compositor);
    settle(&mut compositor);
    for client in [&mut own, &mut spy, &mut owners] {
        client.roundtrip(&mut compositor);
    }
    own.enable_text_field(true);
    spy.enable_text_field(true);
    owners.enable_text_field(true);
    own.roundtrip(&mut compositor);
    spy.roundtrip(&mut compositor);
    owners.roundtrip(&mut compositor);

    compositor.type_text(a, "日本").expect("text typed");
    compositor.compose_text(a, "にほ").expect("text composed");
    for client in [&mut own, &mut spy, &mut owners] {
        client.roundtrip(&mut compositor);
    }
    assert_eq!(own.state.received, "日本");
    assert!(
        spy.state.text_events.is_empty(),
        "{:?}",
        spy.state.text_events
    );
    assert!(owners.state.text_events.is_empty());
    assert!(!compositor.text_field_enabled(b));
    assert!(compositor.compose_text(b, "x").is_err());
}

/// Text that holds a NUL character, which no Wayland string can carry, is refused, whether
/// composed or typed, and Yuna goes on.
#[test]
fn text_holding_a_nul_is_refused() {
    let (mut compositor, a, mut client) = focused_field();
    client.enable_text_field(true);
    client.roundtrip(&mut compositor);
    assert!(compositor.compose_text(a, "a\0b").is_err());
    assert!(compositor.type_text(a, "日\0本").is_err());
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.received, "", "nothing typed");
    assert_eq!(client.state.preedit, None);
}

/// As ADR-0043 says, the next `type` replaces the text composed, even when it holds only keys
/// of the `us` layout.
#[test]
fn the_next_type_replaces_the_text_composed() {
    let (mut compositor, a, mut client) = focused_field();
    client.enable_text_field(true);
    client.roundtrip(&mut compositor);
    compositor.compose_text(a, "にほ").expect("text composed");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.preedit.as_deref(), Some("にほ"));
    compositor.type_text(a, "ab").expect("text typed");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.received, "ab");
    assert_eq!(client.state.preedit, None, "the text composed still shows");
}

/// A long run of three-byte characters is cut below 4,000 bytes at a character's boundary, and
/// arrives whole.
#[test]
fn long_text_is_cut_at_a_characters_boundary() {
    let (mut compositor, a, mut client) = focused_field();
    client.enable_text_field(true);
    client.roundtrip(&mut compositor);
    let long = "日".repeat(1_400);
    compositor.type_text(a, &long).expect("text typed");
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.received, long);
    let pieces: Vec<usize> = client
        .state
        .text_events
        .iter()
        .filter_map(|event| match event {
            TextEvent::Commit(Some(text)) => Some(text.len()),
            _ => None,
        })
        .collect();
    assert_eq!(pieces, [3_999, 201]);
}

/// A client that dies with its field enabled leaves none enabled, and the next client's field is
/// entered and can be enabled.
#[test]
fn a_field_whose_client_died_blocks_no_other() {
    let (mut compositor, a, mut client) = focused_field();
    client.enable_text_field(true);
    client.roundtrip(&mut compositor);
    assert!(compositor.text_field_enabled(a));
    drop(client);
    compositor
        .dispatch(Some(std::time::Duration::ZERO))
        .expect("a round");
    assert!(!compositor.text_field_enabled(a), "a dead client's field");
    let mut other = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut other, &mut compositor);
    settle(&mut compositor);
    other.roundtrip(&mut compositor);
    assert!(
        matches!(other.state.text_events.as_slice(), [TextEvent::Enter(_)]),
        "{:?}",
        other.state.text_events
    );
    other.enable_text_field(true);
    other.roundtrip(&mut compositor);
    assert!(compositor.text_field_enabled(a));
}

/// Text input goes to agents' seats alone: the owner's seat is refused, whatever input method
/// it has.
#[test]
fn the_owners_seat_takes_no_text_composed() {
    let (mut compositor, _, _) = two_seats();
    let refused = compositor.compose_text(OWNER_SEAT, "x");
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| !error.to_string().contains("no text field")),
        "{refused:?}"
    );
}

/// A key name holding a NUL is refused, not a panic.
#[test]
fn a_key_name_holding_a_nul_is_refused() {
    let (mut compositor, a, _client) = focused_field();
    assert!(compositor.key(a, "a\0", true).is_err());
    assert!(compositor.tap_keys(a, "ctrl+\0").is_err());
}
