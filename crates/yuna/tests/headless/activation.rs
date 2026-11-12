//! `xdg-activation-v1`: a valid token, asked by the focused window with its keyboard `enter`
//! serial, moves the focus; any other token, or a fullscreen window holding the screen, only marks
//! the window urgent (ADR-0026, games first).

use std::time::Duration;

use yuna::yuna_layout::WindowId;
use yuna::{Compositor, OWNER_SEAT, TOKEN_LIFETIME};

use crate::client::{Client, Parent, Placement, State};
use crate::{
    BLUE, RED, TILE, advance_to, assert_rests, boundary, manual_compositor, open_and_commit, tiled,
    tiled_and,
};

/// Opens windows a and b, b last and so focused, draws them at boundary 1, and gives the client
/// its keyboard, which enters b.
fn two_windows(
    compositor: &mut Compositor,
    client: &mut Client,
    start: Duration,
) -> (usize, usize) {
    let a = open_and_commit(compositor, client, RED);
    let b = open_and_commit(compositor, client, BLUE);
    advance_to(compositor, client, boundary(start, 1));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));
    client.bind_keyboard(compositor);
    assert_eq!(
        client.keyboard_focus().surface,
        Some(client.window(b).surface.clone())
    );
    (a, b)
}

/// Story 6a: a token asked by the focused window b, with its `enter` serial, activates a: the
/// layout focuses a at once, and at the next boundary a is told it is activated and b that it no
/// longer is; the keyboard follows.
#[test]
fn a_valid_token_moves_the_focus_at_the_next_boundary() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let (a, b) = two_windows(&mut compositor, &mut client, start);
    let configures = (
        client.window(a).configures.len(),
        client.window(b).configures.len(),
    );

    let token = client.request_token(&mut compositor, Some(client.enter_serial()), Some(b));
    client.activate(&token, a);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    // Configures wait for the boundary.
    assert_eq!(
        (
            client.window(a).configures.len(),
            client.window(b).configures.len()
        ),
        configures
    );

    advance_to(&mut compositor, &mut client, boundary(start, 2));
    let last_a = client.window(a).configures.last().expect("a configure");
    assert_eq!((last_a.width, last_a.height), TILE);
    assert_eq!(last_a.states, tiled_and(&[State::Activated]));
    let last_b = client.window(b).configures.last().expect("a configure");
    assert_eq!(last_b.states, tiled());
    assert_eq!(
        client.keyboard_focus().surface,
        Some(client.window(a).surface.clone())
    );
    assert!(!compositor.is_urgent(WindowId(1)));

    // A token is used once: used again, Yuna no longer knows it, and nothing happens.
    client.activate(&token, b);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    assert!(!compositor.is_urgent(WindowId(2)));
    assert_rests(&mut compositor);
}

/// Story 6b: a token asked without a serial is given, but marks the window it activates urgent
/// and moves nothing: no focus, no configure. Urgency ends when the window gets the focus.
#[test]
fn a_token_without_a_serial_only_marks_the_window_urgent() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let (a, b) = two_windows(&mut compositor, &mut client, start);
    let configures_before = compositor.counters().configures;

    let token = client.request_token(&mut compositor, None, Some(b));
    assert!(!token.is_empty());
    client.activate(&token, a);
    client.roundtrip(&mut compositor);
    assert!(compositor.is_urgent(WindowId(1)));
    assert!(!compositor.is_urgent(WindowId(2)));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));
    assert_eq!(compositor.next_deadline(), None);
    assert_eq!(compositor.counters().configures, configures_before);
    assert_rests(&mut compositor);

    // The owner gives it the focus: its urgency ends at the boundary that tells it so.
    compositor.change_layout(|layout| layout.focus_window(WindowId(1)).expect("a focuses"));
    let past_next_boundary = compositor.now() + Duration::from_millis(5);
    advance_to(&mut compositor, &mut client, past_next_boundary);
    assert!(!compositor.is_urgent(WindowId(1)));
    let last_a = client.window(a).configures.last().expect("a configure");
    assert!(last_a.states.contains(&State::Activated));
    assert_rests(&mut compositor);
}

/// A valid token used once its lifetime is over marks the window urgent, as an invalid one.
#[test]
fn an_expired_token_only_marks_the_window_urgent() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let (a, b) = two_windows(&mut compositor, &mut client, start);
    let token = client.request_token(&mut compositor, Some(client.enter_serial()), Some(b));
    let asked_at = compositor.now();

    advance_to(&mut compositor, &mut client, asked_at + TOKEN_LIFETIME);
    client.activate(&token, a);
    client.roundtrip(&mut compositor);
    assert!(compositor.is_urgent(WindowId(1)));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));
    assert_rests(&mut compositor);
}

/// A token asked for a surface that does not have the keyboard, even with a serial the keyboard
/// sent, is not valid.
#[test]
fn a_token_for_a_surface_without_the_keyboard_is_not_valid() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let (a, _b) = two_windows(&mut compositor, &mut client, start);
    let token = client.request_token(&mut compositor, Some(client.enter_serial()), Some(a));
    client.activate(&token, a);
    client.roundtrip(&mut compositor);
    assert!(compositor.is_urgent(WindowId(1)));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));
    assert_rests(&mut compositor);
}

/// An activation of a surface that is not a toplevel, here a popup, is ignored: no focus, no
/// urgency. Its token is used all the same.
#[test]
fn an_activation_of_a_popup_is_ignored() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let (a, b) = two_windows(&mut compositor, &mut client, start);
    let menu = Placement::below_right((200, 100), (10, 10, 1, 1));
    let popup = client.map_popup(&mut compositor, Parent::Window(a), menu, BLUE);
    advance_to(&mut compositor, &mut client, boundary(start, 2));

    let token = client.request_token(&mut compositor, Some(client.enter_serial()), Some(b));
    client.activate_surface(&token, &client.popup(popup).surface);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));
    assert!(!compositor.is_urgent(WindowId(1)));
    client.activate(&token, a);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));
    assert_rests(&mut compositor);
}

/// Story 6c: while a fullscreen window holds the focus, even a valid activation of another window
/// only marks it urgent: the game keeps its screen and stays activated. When the urgent window
/// gets the focus, its urgency ends.
#[test]
fn a_fullscreen_window_keeps_the_focus_and_the_other_turns_urgent() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let game = open_and_commit(&mut compositor, &mut client, RED);
    client.set_fullscreen(game, true);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    let other = open_and_commit(&mut compositor, &mut client, BLUE);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    client.bind_keyboard(&mut compositor);

    let token = client.request_token(&mut compositor, Some(client.enter_serial()), Some(game));
    client.activate(&token, other);
    client.roundtrip(&mut compositor);
    assert!(compositor.is_urgent(WindowId(2)));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    assert_eq!(compositor.next_deadline(), None);
    let last_game = client.window(game).configures.last().expect("a configure");
    assert_eq!(
        last_game.states,
        tiled_and(&[State::Fullscreen, State::Activated])
    );

    compositor.change_layout(|layout| layout.focus_window(WindowId(2)).expect("it focuses"));
    advance_to(&mut compositor, &mut client, boundary(start, 3));
    assert!(!compositor.is_urgent(WindowId(2)));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));
    assert_rests(&mut compositor);
}

/// Story 7: an activation of a toplevel not mapped yet waits for its mapping. A dialog of a
/// window that does not have the focus opens without it; with a valid activation asked before
/// it mapped, it takes the focus when it maps. Behind a fullscreen window, the same request
/// marks it urgent at its mapping instead.
#[test]
fn an_activation_before_mapping_is_applied_at_the_mapping() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let (parent, focused) = two_windows(&mut compositor, &mut client, start);

    let dialog = client.create_window();
    client.set_parent(dialog, parent);
    client.commit(dialog);
    client.roundtrip(&mut compositor);
    let token = client.request_token(&mut compositor, Some(client.enter_serial()), Some(focused));
    client.activate(&token, dialog);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));

    client.ack(dialog);
    client.attach(dialog, 400, 300, RED);
    client.commit(dialog);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(3)));
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    let last = client
        .window(dialog)
        .configures
        .last()
        .expect("a configure");
    assert!(last.states.contains(&State::Activated));
    assert_rests(&mut compositor);
}

/// Story 7, behind a fullscreen window: the activation waits for the mapping, then marks the
/// window urgent.
#[test]
fn an_activation_before_mapping_behind_a_game_marks_it_urgent() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let game = open_and_commit(&mut compositor, &mut client, RED);
    client.set_fullscreen(game, true);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    client.bind_keyboard(&mut compositor);

    let late = client.open_window(&mut compositor);
    let token = client.request_token(&mut compositor, Some(client.enter_serial()), Some(game));
    client.activate(&token, late);
    client.roundtrip(&mut compositor);
    client.ack(late);
    client.attach(late, TILE.0, TILE.1, BLUE);
    client.commit(late);
    client.roundtrip(&mut compositor);
    assert!(compositor.is_urgent(WindowId(2)));
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert!(compositor.is_urgent(WindowId(2)));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    assert_rests(&mut compositor);
}

/// A token lasts [`TOKEN_LIFETIME`], 10 s: used 1 ms before its end, it still activates.
#[test]
fn a_token_lasts_ten_seconds() {
    assert_eq!(TOKEN_LIFETIME, Duration::from_secs(10));
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let (a, b) = two_windows(&mut compositor, &mut client, start);
    let token = client.request_token(&mut compositor, Some(client.enter_serial()), Some(b));
    let asked_at = compositor.now();

    let last_moment = asked_at + TOKEN_LIFETIME - Duration::from_millis(1);
    advance_to(&mut compositor, &mut client, last_moment);
    client.activate(&token, a);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    assert!(!compositor.is_urgent(WindowId(1)));
    let past_next_boundary = compositor.now() + Duration::from_millis(5);
    advance_to(&mut compositor, &mut client, past_next_boundary);
    let last_a = client.window(a).configures.last().expect("a configure");
    assert_eq!(last_a.states, tiled_and(&[State::Activated]));
    assert_rests(&mut compositor);
}

/// An activation waiting for its window keeps the time its token was asked, and is judged when
/// the window maps: a valid token that expired by then marks the window urgent, and moves
/// nothing. The window is a dialog of a window without the focus, which opens without it.
#[test]
fn an_activation_waiting_for_its_window_expires_with_its_token() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let (parent, focused) = two_windows(&mut compositor, &mut client, start);

    let dialog = client.create_window();
    client.set_parent(dialog, parent);
    client.commit(dialog);
    client.roundtrip(&mut compositor);
    let token = client.request_token(&mut compositor, Some(client.enter_serial()), Some(focused));
    let asked_at = compositor.now();
    client.activate(&token, dialog);
    client.roundtrip(&mut compositor);

    advance_to(&mut compositor, &mut client, asked_at + TOKEN_LIFETIME);
    client.ack(dialog);
    client.attach(dialog, 400, 300, RED);
    client.commit(dialog);
    client.roundtrip(&mut compositor);
    assert!(compositor.is_urgent(WindowId(3)));
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(2)));
    let past_next_boundary = compositor.now() + Duration::from_millis(5);
    advance_to(&mut compositor, &mut client, past_next_boundary);
    assert!(compositor.is_urgent(WindowId(3)));
    let last = client
        .window(dialog)
        .configures
        .last()
        .expect("a configure");
    assert!(!last.states.contains(&State::Activated));
    assert_rests(&mut compositor);
}

/// The tokens Yuna keeps are counted per client: one that asks 64 tokens pushes out only its
/// own, and another client's valid token, asked 1 ms before them and so the oldest, still
/// activates.
#[test]
fn a_clients_tokens_never_push_out_another_clients() {
    let mut compositor = manual_compositor();
    let mut flood = Client::connect(&mut compositor);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let (a, b) = two_windows(&mut compositor, &mut client, start);
    let token = client.request_token(&mut compositor, Some(client.enter_serial()), Some(b));

    let later = compositor.now() + Duration::from_millis(1);
    advance_to(&mut compositor, &mut flood, later);
    for _ in 0..64 {
        flood.request_token(&mut compositor, None, None);
    }
    client.activate(&token, a);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    let past_next_boundary = compositor.now() + Duration::from_millis(5);
    advance_to(&mut compositor, &mut client, past_next_boundary);
    let last_a = client.window(a).configures.last().expect("a configure");
    assert_eq!(last_a.states, tiled_and(&[State::Activated]));
    flood.roundtrip(&mut compositor);
    assert_rests(&mut compositor);
}

/// Between refresh boundaries the keyboard stays in the window it is on. While a popup of b
/// holds a grab, a valid token asked from that popup activates a: the layout focuses a at once.
/// The popup's destruction gives the keyboard back to b, not to a, and a new grab on b takes it.
/// At the next boundary the grab ends, its popup dismissed, and the keyboard goes to a, which is
/// told it is activated.
#[test]
fn the_keyboard_changes_window_only_at_a_boundary() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let (a, b) = two_windows(&mut compositor, &mut client, start);
    let menu = Placement::below_right((200, 100), (10, 10, 1, 1));
    let grabbing_popup = |compositor: &mut Compositor, client: &mut Client| {
        let popup = client.create_popup(Parent::Window(b), menu);
        client.grab_popup(popup, client.enter_serial());
        client.commit_popup(popup);
        client.roundtrip(compositor);
        popup
    };
    let first = grabbing_popup(&mut compositor, &mut client);
    let first_surface = client.popup(first).surface.clone();
    assert_eq!(client.keyboard_focus().surface, Some(first_surface.clone()));

    let token = client.request_token_for_surface(
        &mut compositor,
        Some(client.enter_serial()),
        Some(&first_surface),
    );
    client.activate(&token, a);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    assert_eq!(client.keyboard_focus().surface, Some(first_surface));
    let configures = (
        client.window(a).configures.len(),
        client.window(b).configures.len(),
    );

    client.destroy_popup(first);
    client.roundtrip(&mut compositor);
    let b_surface = client.window(b).surface.clone();
    assert_eq!(client.keyboard_focus().surface, Some(b_surface));

    let second = grabbing_popup(&mut compositor, &mut client);
    assert!(!client.popup(second).done);
    let second_surface = client.popup(second).surface.clone();
    assert_eq!(client.keyboard_focus().surface, Some(second_surface));
    assert_eq!(
        (
            client.window(a).configures.len(),
            client.window(b).configures.len()
        ),
        configures
    );

    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert!(client.popup(second).done);
    assert_eq!(
        client.keyboard_focus().surface,
        Some(client.window(a).surface.clone())
    );
    let last_a = client.window(a).configures.last().expect("a configure");
    assert_eq!(last_a.states, tiled_and(&[State::Activated]));
    let last_b = client.window(b).configures.last().expect("a configure");
    assert_eq!(last_b.states, tiled());
    client.destroy_popup(second);
    client.roundtrip(&mut compositor);
    // A popup's destruction arms its window's next boundary.
    advance_to(&mut compositor, &mut client, boundary(start, 3));
    assert_rests(&mut compositor);
}
