//! Agents' seats ([ADR-0035](../../../../docs/adr/0035-agents-seats.md),
//! [ADR-0037](../../../../docs/adr/0037-seats-tools-input-and-closing.md)): each seat's windows
//! open on its own outputs, a client sees its own seat's globals alone, its pointer and keyboard reach its own clients alone, a click
//! focuses, text types through the keymap its client reads, a popup grab and a drag take a
//! press's serial of the seat's own, and a seat that closes lets go of everything first.
//!
//! The compositor has the owner's seat, with its 4K output, and two agents' seats, `agent-2` and
//! `agent-3`, each with an output of 1920 × 1080 at scale 1.

use yuna::yuna_layout::{OutputId, SeatId, WindowId};
use yuna::{BUTTON_LEFT, BUTTON_RIGHT, Compositor, OWNER_SEAT, OutputMode};

use crate::client::{Client, Parent, Placement};
use crate::input_client::{InputClient, InputEvent, Shape};
use crate::{RED, manual_compositor, run_armed_deadlines};

/// The size of each test window's buffer, which is what the layout shows of it.
const WINDOW: (i32, i32) = (400, 300);

/// A compositor with the owner's seat and two agents' seats, A and B, each with an output.
pub(crate) fn two_seats() -> (Compositor, SeatId, SeatId) {
    let mut compositor = manual_compositor();
    let mode = OutputMode::new(1920, 1080, 60.0, 1.0).expect("a mode");
    let a = compositor.add_seat().expect("a seat");
    compositor.add_output(a, mode).expect("an output");
    let b = compositor.add_seat().expect("a seat");
    compositor.add_output(b, mode).expect("an output");
    assert_eq!((a, b), (SeatId(2), SeatId(3)));
    (compositor, a, b)
}

/// Opens a window of `input`'s client with a buffer of [`WINDOW`]'s size.
pub(crate) fn open(input: &mut InputClient, compositor: &mut Compositor) -> usize {
    let window = input.client.open_window(compositor);
    input.client.ack(window);
    input.client.attach(window, WINDOW.0, WINDOW.1, RED);
    input.client.commit(window);
    input.roundtrip(compositor);
    window
}

/// Runs every refresh due, twice over, so that outputs of two rates are all arranged.
pub(crate) fn settle(compositor: &mut Compositor) {
    run_armed_deadlines(compositor);
    run_armed_deadlines(compositor);
}

/// Seat `seat`'s windows, in the order they opened.
pub(crate) fn windows_of(compositor: &Compositor, seat: SeatId) -> Vec<WindowId> {
    compositor
        .windows()
        .into_iter()
        .filter(|window| window.seat == seat)
        .map(|window| window.id)
        .collect()
}

/// A point 10 units inside `window`'s top-left corner, on the output that shows it, in that
/// output's coordinates.
fn inside(compositor: &Compositor, window: WindowId) -> (OutputId, f64, f64) {
    let description = compositor
        .windows()
        .into_iter()
        .find(|description| description.id == window)
        .expect("the window");
    let output = description.output.expect("shown");
    let rect = description.rect.expect("placed");
    let output_rect = compositor
        .output_descriptions()
        .into_iter()
        .find(|description| description.id == output)
        .expect("the output")
        .rect;
    (
        output,
        rect.x - output_rect.x + 10.0,
        rect.y - output_rect.y + 10.0,
    )
}

/// Moves `seat`'s pointer inside `window` and clicks `button` there.
pub(crate) fn click(compositor: &mut Compositor, seat: SeatId, window: WindowId, button: u32) {
    let (output, x, y) = inside(compositor, window);
    let pointer = compositor
        .pointer_move(seat, Some(output), x, y)
        .expect("the pointer moves");
    assert_eq!(pointer.window, Some(window));
    compositor
        .pointer_button(seat, button, true)
        .expect("a press");
    compositor
        .pointer_button(seat, button, false)
        .expect("a release");
}

fn roundtrip_all(compositor: &mut Compositor, clients: &mut [&mut InputClient]) {
    for client in clients {
        client.roundtrip(compositor);
    }
}

/// The input events among `events`: pointer, keyboard and drag events, without the keymaps a
/// client is sent when it binds its keyboard.
fn input_events(events: &[InputEvent]) -> Vec<&InputEvent> {
    events
        .iter()
        .filter(|event| !matches!(event, InputEvent::Keymap))
        .collect()
}

/// The names of the outputs `client` was shown, in order.
fn output_names(client: &Client) -> Vec<&str> {
    let mut names: Vec<&str> = client
        .state
        .output_names
        .iter()
        .map(|(_, name)| name.as_str())
        .collect();
    names.sort_unstable();
    names
}

/// ADR-0042: a client is shown its own seat's `wl_seat` and its seat's outputs' `wl_output`s
/// alone, the owner's clients as agents'; an output added later is announced to its own seat's
/// clients alone.
#[test]
fn a_client_sees_its_own_seat_and_its_outputs_alone() {
    let (mut compositor, a, b) = two_seats();
    let owner = InputClient::connect(&mut compositor, OWNER_SEAT, "seat0");
    let on_a = InputClient::connect(&mut compositor, a, "agent-2");
    let on_b = InputClient::connect(&mut compositor, b, "agent-3");
    assert_eq!(owner.seat_names(), ["seat0"]);
    assert_eq!(on_a.seat_names(), ["agent-2"]);
    assert_eq!(on_b.seat_names(), ["agent-3"]);
    let mut owners = Client::connect(&mut compositor);
    let mut as_ = Client::connect_to(&mut compositor, a);
    let mut bs = Client::connect_to(&mut compositor, b);
    assert_eq!(output_names(&owners), ["HEADLESS-1"]);
    assert_eq!(output_names(&as_), ["HEADLESS-2"]);
    assert_eq!(output_names(&bs), ["HEADLESS-3"]);

    let mode = OutputMode::new(1280, 720, 60.0, 1.0).expect("a mode");
    compositor.add_output(b, mode).expect("an output");
    for client in [&mut owners, &mut as_, &mut bs] {
        client.roundtrip(&mut compositor);
        client.roundtrip(&mut compositor);
    }
    assert_eq!(output_names(&owners), ["HEADLESS-1"]);
    assert_eq!(output_names(&as_), ["HEADLESS-2"]);
    assert_eq!(output_names(&bs), ["HEADLESS-3", "HEADLESS-4"]);
}

/// The test ADR-0015 and ADR-0035 ask for: an agent's input never reaches another seat. Seat A's
/// pointer, buttons, wheel and keys reach A's client alone: not B's, not the owner's, and not a
/// client of B that bound A's `wl_seat`, a spy the filter of ADR-0042 would never show it, made
/// here to check the layer below: every focus is on a seat's own clients. B's input
/// reaches B's client alone, the window of that client of B included, which bound no pointer
/// of B: it gets nothing. Input to the owner's seat, or to another seat's output, is refused.
#[test]
fn an_agents_input_reaches_its_own_seats_clients_alone() {
    let (mut compositor, a, b) = two_seats();
    let mut on_a = InputClient::connect(&mut compositor, a, "agent-2");
    let mut on_b = InputClient::connect(&mut compositor, b, "agent-3");
    let mut spy = InputClient::connect_spy(&mut compositor, b, "agent-2");
    let mut owner = InputClient::connect(&mut compositor, OWNER_SEAT, "seat0");
    assert_eq!(on_a.seat_names(), ["agent-2"]);
    assert_eq!(spy.seat_names(), ["seat0", "agent-2", "agent-3"]);
    open(&mut on_a, &mut compositor);
    open(&mut on_b, &mut compositor);
    open(&mut spy, &mut compositor);
    open(&mut owner, &mut compositor);
    settle(&mut compositor);
    roundtrip_all(
        &mut compositor,
        &mut [&mut on_a, &mut on_b, &mut spy, &mut owner],
    );
    let window_a = windows_of(&compositor, a)[0];
    let [window_b, window_spy] = windows_of(&compositor, b)[..] else {
        panic!("two windows on seat B");
    };
    // Each seat's keyboard went to its own client's window; the spy's keyboard is A's, whose
    // focus is on A's window: it got no enter.
    assert!(
        on_a.take_events()
            .iter()
            .any(|event| matches!(event, InputEvent::KeyboardEnter { .. }))
    );
    assert_eq!(input_events(&spy.take_events()), Vec::<&InputEvent>::new());
    on_b.take_events();
    owner.take_events();

    click(&mut compositor, a, window_a, BUTTON_LEFT);
    compositor.pointer_scroll(a, 0.0, 1.0).expect("a scroll");
    compositor.tap_keys(a, "x").expect("a tap");
    compositor.type_text(a, "é").expect("text typed");
    roundtrip_all(
        &mut compositor,
        &mut [&mut on_a, &mut on_b, &mut spy, &mut owner],
    );
    let events_a = on_a.take_events();
    let presses = events_a
        .iter()
        .filter(|event| matches!(event, InputEvent::Button { pressed: true, .. }))
        .count();
    assert_eq!(presses, 1, "{events_a:?}");
    assert!(
        events_a
            .iter()
            .any(|event| matches!(event, InputEvent::PointerEnter { .. }))
    );
    assert!(
        events_a
            .iter()
            .any(|event| matches!(event, InputEvent::Value120 { steps: 120, .. }))
    );
    assert_eq!(on_a.state.typed, "xé");
    for (name, client) in [
        ("B", &mut on_b),
        ("the spy", &mut spy),
        ("the owner", &mut owner),
    ] {
        let events = client.take_events();
        assert_eq!(input_events(&events), Vec::<&InputEvent>::new(), "{name}");
        assert_eq!(client.state.typed, "", "{name}");
    }

    // B's input: a click on its own client's window, then, its keyboard moved there, a key; then
    // a click on the spy's window, whose client has no pointer of B.
    click(&mut compositor, b, window_b, BUTTON_LEFT);
    settle(&mut compositor);
    compositor.tap_keys(b, "y").expect("a tap");
    click(&mut compositor, b, window_spy, BUTTON_LEFT);
    roundtrip_all(
        &mut compositor,
        &mut [&mut on_a, &mut on_b, &mut spy, &mut owner],
    );
    assert_eq!(on_b.state.typed, "y");
    for (name, client) in [
        ("A", &mut on_a),
        ("the spy", &mut spy),
        ("the owner", &mut owner),
    ] {
        let events = client.take_events();
        assert_eq!(input_events(&events), Vec::<&InputEvent>::new(), "{name}");
    }
    assert_eq!(on_a.state.typed, "xé");

    // Input to the owner's seat, or onto another seat's output, is refused.
    assert!(compositor.pointer_move(OWNER_SEAT, None, 1.0, 1.0).is_err());
    assert!(compositor.tap_keys(OWNER_SEAT, "a").is_err());
    let (output_b, x, y) = inside(&compositor, window_b);
    assert!(compositor.pointer_move(a, Some(output_b), x, y).is_err());
    assert!(compositor.pointer_move(a, None, 1920.0, 0.0).is_err());
    assert!(compositor.pointer_move(a, None, f64::NAN, 0.0).is_err());
    compositor
        .check_windows()
        .expect("the shell agrees with the layout");
    compositor
        .check_popups_and_keyboard()
        .expect("the popups and the keyboards are sound");
}

/// A press focuses the window under it; the seat's keyboard follows at the next refresh, and a
/// key then reaches that window. The window under the pointer is told in the result.
#[test]
fn a_press_focuses_its_window_and_the_keyboard_follows_at_the_refresh() {
    let (mut compositor, a, _) = two_seats();
    let mut first = InputClient::connect(&mut compositor, a, "agent-2");
    let mut second = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut first, &mut compositor);
    open(&mut second, &mut compositor);
    settle(&mut compositor);
    let [window_1, window_2] = windows_of(&compositor, a)[..] else {
        panic!("two windows");
    };
    assert_eq!(compositor.keyboard_window(a), Some(window_2));

    click(&mut compositor, a, window_1, BUTTON_LEFT);
    let focused = |compositor: &Compositor| {
        compositor
            .windows()
            .into_iter()
            .find(|window| window.focused && window.seat == a)
            .map(|window| window.id)
    };
    assert_eq!(
        compositor.keyboard_window(a),
        Some(window_2),
        "not before the refresh"
    );
    settle(&mut compositor);
    assert_eq!(focused(&compositor), Some(window_1));
    assert_eq!(compositor.keyboard_window(a), Some(window_1));
    compositor.type_text(a, "ok").expect("text typed");
    roundtrip_all(&mut compositor, &mut [&mut first, &mut second]);
    assert_eq!(
        (first.state.typed.as_str(), second.state.typed.as_str()),
        ("ok", "")
    );
    let pointer = compositor.pointer(a);
    assert_eq!(pointer.window, Some(window_1));
    assert_eq!(pointer.surface_position, Some((10.0, 10.0)));
    assert!(pointer.buttons.is_empty());
}

/// Text types through the seat's `us` keymap, Shift held where a character needs it, and reads
/// back the same through the keymaps the client was sent: a character `us` lacks comes with a
/// keymap of its own, then the seat's again. A tap holds its modifiers. Bad keys and too much
/// text are refused.
#[test]
fn text_types_through_the_keymaps_the_client_reads() {
    let (mut compositor, a, _) = two_seats();
    let mut client = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut client, &mut compositor);
    settle(&mut compositor);
    client.roundtrip(&mut compositor);
    client.take_events();

    let text = "Hello, World!\t~ ç→😀 back to us.\n";
    compositor.type_text(a, text).expect("text typed");
    client.roundtrip(&mut compositor);
    // Return types a carriage return, as on every keyboard.
    assert_eq!(client.state.typed, text.replace('\n', "\r"));
    let keymaps = client
        .take_events()
        .iter()
        .filter(|event| matches!(event, InputEvent::Keymap))
        .count();
    // One run outside `us`, "ç→😀": its keymap, then the seat's again.
    assert_eq!(keymaps, 2);

    client.state.typed.clear();
    compositor.tap_keys(a, "ctrl+shift+t").expect("a tap");
    client.roundtrip(&mut compositor);
    let events = client.take_events();
    let keys: Vec<bool> = events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Key { pressed, .. } => Some(*pressed),
            _ => None,
        })
        .collect();
    assert_eq!(keys, [true, true, true, false, false, false]);
    // Shift (1) and Control (4) held together.
    assert!(
        events.contains(&InputEvent::Modifiers { depressed: 5 }),
        "{events:?}"
    );
    assert!(events.last() == Some(&InputEvent::Modifiers { depressed: 0 }));

    assert!(compositor.key(a, "NoSuchKey", true).is_err());
    assert!(compositor.tap_keys(a, "ctrl+ctrl").is_err());
    compositor.key(a, "Return", true).expect("a press");
    assert!(compositor.key(a, "Return", true).is_err(), "held already");
    assert!(compositor.tap_keys(a, "Return").is_err(), "held already");
    compositor
        .key(a, "return", false)
        .expect("a release, any case");
    assert!(compositor.key(a, "Return", false).is_err(), "not held");
    assert!(
        compositor
            .type_text(a, &"a".repeat(yuna::MAX_TYPED + 1))
            .is_err()
    );
    // A key the text needs, held, refuses the whole text: nothing typed, no Shift left held.
    client.roundtrip(&mut compositor);
    client.state.typed.clear();
    compositor.key(a, "a", true).expect("a press");
    assert!(compositor.type_text(a, "bA").is_err());
    compositor.key(a, "a", false).expect("a release");
    assert!(
        compositor.key(a, "Shift_L", false).is_err(),
        "Shift is not held"
    );
    client.roundtrip(&mut compositor);
    assert_eq!(client.state.typed, "a");
}

/// A wheel step scrolls 15 units and 120ths of a step; the cursor a client asks with its
/// `enter` serial is what the pointer reports; a serial of no `enter` is ignored.
#[test]
fn the_wheel_scrolls_and_the_cursor_is_the_one_its_client_asked() {
    let (mut compositor, a, _) = two_seats();
    let mut client = InputClient::connect(&mut compositor, a, "agent-2");
    open(&mut client, &mut compositor);
    settle(&mut compositor);
    let window = windows_of(&compositor, a)[0];
    let (output, x, y) = inside(&compositor, window);
    compositor
        .pointer_move(a, Some(output), x, y)
        .expect("a move");
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.pointer(a).cursor, "default");
    let serial = client.pointer_enter_serial();
    client.set_cursor_shape(serial.wrapping_add(1000), Shape::Wait);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.pointer(a).cursor, "default", "a wrong serial");
    client.set_cursor_shape(serial, Shape::Text);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.pointer(a).cursor, "text");
    client.take_events();

    compositor.pointer_scroll(a, -1.0, 2.0).expect("a scroll");
    client.roundtrip(&mut compositor);
    let events = client.take_events();
    assert!(events.contains(&InputEvent::Axis {
        horizontal: false,
        value: 30.0
    }));
    assert!(events.contains(&InputEvent::Value120 {
        horizontal: false,
        steps: 240
    }));
    assert!(events.contains(&InputEvent::Axis {
        horizontal: true,
        value: -15.0
    }));
    assert!(events.contains(&InputEvent::Value120 {
        horizontal: true,
        steps: -120
    }));
    assert_eq!(events.last(), Some(&InputEvent::Frame));
    assert!(compositor.pointer_scroll(a, 0.0, 0.0).is_err());
    assert!(compositor.pointer_scroll(a, 0.0, 1e9).is_err());
}

/// A menu opened on a right press takes its grab with the press's serial; a click outside the
/// client's surfaces dismisses it. A client of another seat that names A's seat and A's serial
/// is refused its grab, its popup dismissed: that serial went to no window of its own, and it
/// names a seat not its client's, two checks that each refuse it.
#[test]
fn a_popup_grab_takes_a_press_serial_of_the_seat_of_its_client() {
    let (mut compositor, a, b) = two_seats();
    let mut client = InputClient::connect(&mut compositor, a, "agent-2");
    let mut spy = InputClient::connect_spy(&mut compositor, b, "agent-2");
    let window = open(&mut client, &mut compositor);
    let spy_window = open(&mut spy, &mut compositor);
    settle(&mut compositor);
    let window_a = windows_of(&compositor, a)[0];
    client.roundtrip(&mut compositor);
    client.take_events();

    let (output, x, y) = inside(&compositor, window_a);
    compositor
        .pointer_move(a, Some(output), x, y)
        .expect("a move");
    compositor
        .pointer_button(a, BUTTON_RIGHT, true)
        .expect("a press");
    client.roundtrip(&mut compositor);
    let serial = client
        .take_events()
        .iter()
        .find_map(|event| match event {
            InputEvent::Button {
                pressed: true,
                serial,
                ..
            } => Some(*serial),
            _ => None,
        })
        .expect("the press");
    let placement = Placement::below_right((100, 80), (10, 10, 1, 1));
    let seat_a = client.wl_seat("agent-2");
    let menu = client
        .client
        .create_popup(Parent::Window(window), placement);
    client.client.grab_popup_on(menu, &seat_a, serial);
    client.client.commit_popup(menu);
    client.roundtrip(&mut compositor);
    client.client.ack_popup(menu);
    client.client.attach_popup(menu, 100, 80, RED);
    client.client.commit_popup(menu);
    client.roundtrip(&mut compositor);
    assert!(!client.client.popup(menu).done, "the grab is granted");
    compositor
        .pointer_button(a, BUTTON_RIGHT, false)
        .expect("a release");

    // The spy names A's seat and the serial A's client got.
    let seat_a_of_spy = spy.wl_seat("agent-2");
    let stolen = spy
        .client
        .create_popup(Parent::Window(spy_window), placement);
    spy.client.grab_popup_on(stolen, &seat_a_of_spy, serial);
    spy.client.commit_popup(stolen);
    spy.roundtrip(&mut compositor);
    assert!(spy.client.popup(stolen).done, "the spy's grab is refused");
    assert!(!client.client.popup(menu).done);

    // A click outside the client's surfaces ends the grab.
    compositor
        .pointer_move(a, Some(output), 1900.0, 1000.0)
        .expect("a move");
    compositor
        .pointer_button(a, BUTTON_LEFT, true)
        .expect("a press");
    compositor
        .pointer_button(a, BUTTON_LEFT, false)
        .expect("a release");
    client.roundtrip(&mut compositor);
    assert!(
        client.client.popup(menu).done,
        "dismissed by the click outside"
    );
    compositor
        .check_popups_and_keyboard()
        .expect("the popups and the keyboards are sound");
}

/// A drag started with a press's serial carries its offer to the seat's windows it crosses, and
/// drops where the button is released, the target having accepted it. A client of another seat
/// that starts a drag on A's seat with A's serial has its source cancelled, and A's pointer
/// stays its own.
#[test]
fn a_drag_crosses_its_seats_windows_and_never_starts_from_another_seat() {
    let (mut compositor, a, b) = two_seats();
    let mut source = InputClient::connect(&mut compositor, a, "agent-2");
    let mut target = InputClient::connect(&mut compositor, a, "agent-2");
    let mut spy = InputClient::connect_spy(&mut compositor, b, "agent-2");
    let source_window = open(&mut source, &mut compositor);
    open(&mut target, &mut compositor);
    let spy_window = open(&mut spy, &mut compositor);
    settle(&mut compositor);
    let [window_1, window_2] = windows_of(&compositor, a)[..] else {
        panic!("two windows");
    };
    roundtrip_all(&mut compositor, &mut [&mut source, &mut target, &mut spy]);
    source.take_events();
    target.take_events();

    // The spy first: A's pointer pressed on A's window, the spy starts a drag with its serial.
    let (output, x, y) = inside(&compositor, window_1);
    compositor
        .pointer_move(a, Some(output), x, y)
        .expect("a move");
    compositor
        .pointer_button(a, BUTTON_LEFT, true)
        .expect("a press");
    source.roundtrip(&mut compositor);
    let serial = source
        .take_events()
        .iter()
        .find_map(|event| match event {
            InputEvent::Button {
                pressed: true,
                serial,
                ..
            } => Some(*serial),
            _ => None,
        })
        .expect("the press");
    let spy_surface = spy.client.window(spy_window).surface.clone();
    let stolen = spy.start_drag(&spy_surface, serial);
    spy.roundtrip(&mut compositor);
    assert!(
        spy.state.sources[stolen].cancelled,
        "the spy's drag is refused"
    );
    let (output_2, x_2, y_2) = inside(&compositor, window_2);
    compositor
        .pointer_move(a, Some(output_2), x_2, y_2)
        .expect("a move");
    target.roundtrip(&mut compositor);
    assert!(
        !target
            .take_events()
            .iter()
            .any(|event| matches!(event, InputEvent::DragEnter { .. })),
        "no drag crosses A's windows"
    );
    compositor
        .pointer_button(a, BUTTON_LEFT, false)
        .expect("a release");

    // Now the source's own drag.
    compositor
        .pointer_move(a, Some(output), x, y)
        .expect("a move");
    compositor
        .pointer_button(a, BUTTON_LEFT, true)
        .expect("a press");
    source.roundtrip(&mut compositor);
    let serial = source
        .take_events()
        .iter()
        .find_map(|event| match event {
            InputEvent::Button {
                pressed: true,
                serial,
                ..
            } => Some(*serial),
            _ => None,
        })
        .expect("the press");
    let origin = source.client.window(source_window).surface.clone();
    let drag = source.start_drag(&origin, serial);
    source.roundtrip(&mut compositor);
    compositor
        .pointer_move(a, Some(output_2), x_2, y_2)
        .expect("a move");
    target.roundtrip(&mut compositor);
    let enter_serial = target
        .take_events()
        .iter()
        .find_map(|event| match event {
            InputEvent::DragEnter { serial, .. } => Some(*serial),
            _ => None,
        })
        .expect("the drag enters the target");
    target.accept_drag(enter_serial);
    target.roundtrip(&mut compositor);
    compositor
        .pointer_button(a, BUTTON_LEFT, false)
        .expect("a release");
    roundtrip_all(&mut compositor, &mut [&mut source, &mut target]);
    assert!(target.take_events().contains(&InputEvent::Drop));
    let told = &source.state.sources[drag];
    // A drop is performed only once the target accepted a type and an action.
    assert!(told.drop_performed && !told.cancelled, "{told:?}");
}

/// Closing a seat releases the keys and buttons it holds, and its focus leaves, before its
/// clients are ended; then its windows, its outputs and its `wl_seat` are gone, the other
/// seats untouched, and a new seat takes a new id.
#[test]
fn closing_a_seat_lets_go_of_everything_before_its_clients_end() {
    let (mut compositor, a, b) = two_seats();
    let mut on_a = InputClient::connect(&mut compositor, a, "agent-2");
    let mut on_b = InputClient::connect(&mut compositor, b, "agent-3");
    open(&mut on_a, &mut compositor);
    open(&mut on_b, &mut compositor);
    settle(&mut compositor);
    let window_a = windows_of(&compositor, a)[0];
    let (output, x, y) = inside(&compositor, window_a);
    compositor
        .pointer_move(a, Some(output), x, y)
        .expect("a move");
    compositor
        .pointer_button(a, BUTTON_LEFT, true)
        .expect("a press");
    compositor.key(a, "Shift_L", true).expect("a press");
    roundtrip_all(&mut compositor, &mut [&mut on_a, &mut on_b]);
    on_a.take_events();
    on_b.take_events();

    let pids = compositor.remove_seat(a).expect("the seat closes");
    assert_eq!(
        pids,
        [std::process::id() as i32],
        "its one client, in this process"
    );
    on_a.read_until_closed();
    let events = on_a.take_events();
    let position = |wanted: fn(&InputEvent) -> bool| {
        events
            .iter()
            .position(wanted)
            .unwrap_or_else(|| panic!("missing in {events:?}"))
    };
    let key_up = position(|e| matches!(e, InputEvent::Key { pressed: false, .. }));
    let button_up = position(|e| matches!(e, InputEvent::Button { pressed: false, .. }));
    let keyboard_left = position(|e| matches!(e, InputEvent::KeyboardLeave { .. }));
    let pointer_left = position(|e| matches!(e, InputEvent::PointerLeave { .. }));
    assert!(
        key_up < keyboard_left && button_up < pointer_left,
        "{events:?}"
    );

    assert_eq!(compositor.seats(), [OWNER_SEAT, b]);
    assert_eq!(windows_of(&compositor, a), Vec::new());
    assert_eq!(windows_of(&compositor, b).len(), 1);
    assert!(compositor.outputs().iter().all(|&id| id != output));
    assert!(compositor.remove_seat(a).is_err(), "closed already");
    assert!(compositor.remove_seat(OWNER_SEAT).is_err());
    let late = InputClient::connect(&mut compositor, b, "agent-3");
    assert_eq!(late.seat_names(), ["agent-3"]);
    assert_eq!(compositor.add_seat().expect("a seat"), SeatId(4));
    on_b.roundtrip(&mut compositor);
    compositor
        .check_windows()
        .expect("the shell agrees with the layout");
    compositor
        .check_popups_and_keyboard()
        .expect("the popups and the keyboards are sound");
}

/// The serial of the last keyboard `enter` among `events`.
fn last_enter(events: &[InputEvent]) -> u32 {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            InputEvent::KeyboardEnter { serial, .. } => Some(*serial),
            _ => None,
        })
        .expect("a keyboard enter")
}

/// Activation and the clipboard keep to their seat: a token asked with A's seat and A's serial
/// activates a window of A; asked by a client of B naming A's seat and serial, or carried over to
/// B, it is not valid there and only marks B's window urgent. No agent's client sees
/// `ext-data-control-v1`, which reaches any seat's selections; the owner's clients do.
#[test]
fn activation_and_the_clipboard_keep_to_their_seat() {
    let (mut compositor, a, b) = two_seats();
    let mut on_a = InputClient::connect(&mut compositor, a, "agent-2");
    let mut spy = InputClient::connect_spy(&mut compositor, b, "agent-2");
    let owner = InputClient::connect(&mut compositor, OWNER_SEAT, "seat0");
    let first = open(&mut on_a, &mut compositor);
    let second = open(&mut on_a, &mut compositor);
    let spy_first = open(&mut spy, &mut compositor);
    let spy_second = open(&mut spy, &mut compositor);
    settle(&mut compositor);
    roundtrip_all(&mut compositor, &mut [&mut on_a, &mut spy]);
    let [window_1, _] = windows_of(&compositor, a)[..] else {
        panic!("two windows on A");
    };
    let [spy_window_1, _] = windows_of(&compositor, b)[..] else {
        panic!("two windows on B");
    };
    let serial = last_enter(&on_a.take_events());
    let seat_a = on_a.wl_seat("agent-2");
    let valid = on_a
        .client
        .request_token_on(&mut compositor, &seat_a, serial, second);
    on_a.client.activate(&valid, first);
    on_a.roundtrip(&mut compositor);
    settle(&mut compositor);
    assert_eq!(compositor.keyboard_window(a), Some(window_1));
    assert!(!compositor.is_urgent(window_1));

    // The spy names A's seat and A's serial, for its own window: not valid on B.
    let seat_a_of_spy = spy.wl_seat("agent-2");
    let stolen = spy
        .client
        .request_token_on(&mut compositor, &seat_a_of_spy, serial, spy_second);
    spy.client.activate(&stolen, spy_first);
    spy.roundtrip(&mut compositor);
    settle(&mut compositor);
    assert!(compositor.is_urgent(spy_window_1));
    // A valid token of A, carried over to B, is not valid there either.
    roundtrip_all(&mut compositor, &mut [&mut on_a]);
    let serial = last_enter(&on_a.take_events());
    let carried = on_a
        .client
        .request_token_on(&mut compositor, &seat_a, serial, first);
    spy.client.activate(&carried, spy_first);
    spy.roundtrip(&mut compositor);
    settle(&mut compositor);
    let focused_b = compositor
        .windows()
        .into_iter()
        .find(|window| window.seat == b && window.focused)
        .map(|window| window.id);
    assert_ne!(focused_b, Some(spy_window_1));

    let data_control = "ext_data_control_manager_v1".to_owned();
    assert!(!on_a.state.globals.contains(&data_control));
    assert!(!spy.state.globals.contains(&data_control));
    assert!(owner.state.globals.contains(&data_control));
}
