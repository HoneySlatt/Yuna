//! Popups: placed by their positioner within their window's output, drawn just above their
//! window, called back with it, grabbing the keyboard only with a serial Yuna sent, and
//! dismissed with their window or when they cannot be placed.
//!
//! The first window is a half-width column at (16, 16) logical units, after a gap: (24, 24) in
//! pixels at scale 1.5. A popup's place is relative to its parent's window geometry.

use std::time::Duration;

use smithay::utils::Rectangle;
use yuna::yuna_layout::WindowId;
use yuna::{Compositor, MAX_POPUP_DEPTH, OWNER_SEAT};

use crate::client::{Client, ConstraintAdjustment, Feedback, Kind, Parent, Placement};
use crate::{
    BLUE, OUTPUT, PERIOD, RED, advance_to, assert_rests, boundary, manual_compositor, millis,
    open_and_commit,
};

const GREEN: u32 = 0xFF_00_FF_00;
const RED_PIXEL: [u8; 4] = [255, 0, 0, 255];
const GREEN_PIXEL: [u8; 4] = [0, 255, 0, 255];

/// A menu of 200 × 100 under a 1 × 1 button at (10, 10) of its parent: at (11, 11).
fn menu() -> Placement {
    Placement::below_right((200, 100), (10, 10, 1, 1))
}

fn pixel(compositor: &mut Compositor, x: i32, y: i32) -> [u8; 4] {
    let bytes = compositor
        .read_pixels(OUTPUT, Rectangle::new((x, y).into(), (1, 1).into()))
        .expect("the pixel reads");
    bytes.try_into().expect("four bytes")
}

/// Opens a window, draws it at boundary 1, and checks it is where these stories expect it.
fn mapped_window(compositor: &mut Compositor, client: &mut Client, start: Duration) -> usize {
    let w = open_and_commit(compositor, client, RED);
    advance_to(compositor, client, boundary(start, 1));
    let placed = compositor.layout().arrange(OUTPUT).expect("the output");
    let rect = placed[0].rect;
    assert_eq!((rect.x, rect.y), (16.0, 16.0));
    w
}

/// Story 1: a popup of a mapped window gets its configure at its initial commit: (11, 11),
/// 200 × 100. Its first buffer is drawn in exactly one frame at the next boundary, over its
/// window: its corner at 16 + 11 = 27 logical units, 40.5 pixels, rounded to 41; 300 × 150
/// pixels. Its frame callback and presentation feedback come at that boundary; then rest.
#[test]
fn a_popup_is_placed_drawn_over_its_window_and_called_back() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = mapped_window(&mut compositor, &mut client, start);

    let p = client.open_popup(&mut compositor, Parent::Window(w), menu());
    let configures = &client.popup(p).configures;
    assert_eq!(configures.len(), 1);
    let first = configures[0];
    assert_eq!(
        (first.x, first.y, first.width, first.height),
        (11, 11, 200, 100)
    );
    // Popups are not windows of the layout.
    assert_eq!(compositor.layout().window(WindowId(2)), None);

    client.ack_popup(p);
    client.attach_popup(p, 200, 100, GREEN);
    client.request_popup_frame(p);
    client.request_popup_feedback(p);
    client.commit_popup(p);
    client.roundtrip(&mut compositor);
    let drawn_before = compositor.counters().frames_drawn;
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(compositor.counters().frames_drawn, drawn_before + 1);

    let popup = client.popup(p);
    assert_eq!(popup.frames, vec![millis(boundary(start, 2))]);
    assert_eq!(
        popup.feedback,
        vec![Feedback::Presented {
            time: boundary(start, 2),
            refresh: PERIOD,
            seq: 2,
            flags: Kind::Vsync,
        }]
    );
    assert_eq!(pixel(&mut compositor, 41, 41), GREEN_PIXEL);
    assert_eq!(pixel(&mut compositor, 340, 190), GREEN_PIXEL);
    assert_eq!(pixel(&mut compositor, 40, 41), RED_PIXEL);
    assert_eq!(pixel(&mut compositor, 41, 40), RED_PIXEL);
    assert_eq!(pixel(&mut compositor, 341, 100), RED_PIXEL);
    assert_eq!(pixel(&mut compositor, 100, 191), RED_PIXEL);
    assert_rests(&mut compositor);
}

/// Story 2: a popup 2,000 wide hanging from (1200, 10) would end at 16 + 1201 + 2,000 = 3,217,
/// past the output's right edge at 2,560. With `slide_x` it moves left by the 657 it overflows:
/// to x 544, its right edge on the output's, at 3,840 pixels. It is not cut to its window, which
/// ends at 1,272 logical units, 1,908 pixels.
#[test]
fn a_popup_past_the_output_slides_back_inside() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = mapped_window(&mut compositor, &mut client, start);

    let placement = Placement {
        adjustment: ConstraintAdjustment::SlideX,
        ..Placement::below_right((2000, 100), (1200, 10, 1, 1))
    };
    let p = client.map_popup(&mut compositor, Parent::Window(w), placement, GREEN);
    let configure = client.popup(p).configures[0];
    assert_eq!(
        (configure.x, configure.y, configure.width, configure.height),
        (544, 11, 2000, 100)
    );
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    let background = pixel(&mut compositor, 3839, 2159);
    // 16 + 544 = 560 logical units: 840 pixels.
    assert_eq!(pixel(&mut compositor, 839, 41), RED_PIXEL);
    assert_eq!(pixel(&mut compositor, 840, 41), GREEN_PIXEL);
    assert_eq!(pixel(&mut compositor, 1000, 41), GREEN_PIXEL);
    // Past the window's right edge, over the background.
    assert_eq!(pixel(&mut compositor, 2000, 41), GREEN_PIXEL);
    assert_eq!(pixel(&mut compositor, 3839, 190), GREEN_PIXEL);

    // Without an adjustment allowed, the popup stays where its positioner puts it, past the edge.
    let unadjusted = client.open_popup(
        &mut compositor,
        Parent::Window(w),
        Placement::below_right((2000, 100), (1200, 10, 1, 1)),
    );
    assert_eq!(client.popup(unadjusted).configures[0].x, 1201);

    // Placed again, as its client asks: the answer carries the token.
    client.reposition_popup(p, menu(), 7);
    client.roundtrip(&mut compositor);
    let popup = client.popup(p);
    assert_eq!(popup.repositioned, vec![7]);
    let moved = popup.configures.last().expect("a configure");
    assert_eq!(
        (moved.x, moved.y, moved.width, moved.height),
        (11, 11, 200, 100)
    );

    client.ack_popup(p);
    client.attach_popup(p, 200, 100, GREEN);
    client.commit_popup(p);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 3));
    assert_eq!(pixel(&mut compositor, 41, 41), GREEN_PIXEL);
    assert_eq!(pixel(&mut compositor, 1000, 41), RED_PIXEL);
    assert_eq!(pixel(&mut compositor, 2000, 41), background);
    assert_rests(&mut compositor);
}

/// Story 3: a popup that asks a grab with a serial Yuna never sent is dismissed, without a
/// configure; one that asks it with its window's keyboard `enter` serial gets it, and the
/// keyboard goes to the popup; when the popup goes, the keyboard comes back to the window.
#[test]
fn a_popup_grab_needs_a_serial_yuna_sent() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = mapped_window(&mut compositor, &mut client, start);
    client.bind_keyboard(&mut compositor);
    let window_surface = client.window(w).surface.clone();
    assert_eq!(
        client.keyboard_focus().surface,
        Some(window_surface.clone())
    );
    let enter = client.enter_serial();

    let refused = client.create_popup(Parent::Window(w), menu());
    // Serials are counted from 1 upwards by one counter: this one is far ahead of any sent.
    client.grab_popup(refused, enter.wrapping_add(1_000_000));
    client.commit_popup(refused);
    client.roundtrip(&mut compositor);
    assert!(client.popup(refused).done);
    assert!(client.popup(refused).configures.is_empty());
    assert_eq!(
        client.keyboard_focus().surface,
        Some(window_surface.clone())
    );
    client.destroy_popup(refused);

    let granted = client.create_popup(Parent::Window(w), menu());
    client.grab_popup(granted, enter);
    client.commit_popup(granted);
    client.roundtrip(&mut compositor);
    assert!(!client.popup(granted).done);
    assert_eq!(client.popup(granted).configures.len(), 1);
    let popup_surface = client.popup(granted).surface.clone();
    assert_eq!(client.keyboard_focus().surface, Some(popup_surface));

    client.ack_popup(granted);
    client.attach_popup(granted, 200, 100, GREEN);
    client.commit_popup(granted);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(pixel(&mut compositor, 41, 41), GREEN_PIXEL);

    client.destroy_popup(granted);
    client.roundtrip(&mut compositor);
    assert_eq!(client.keyboard_focus().surface, Some(window_surface));
    advance_to(&mut compositor, &mut client, boundary(start, 3));
    assert_eq!(pixel(&mut compositor, 41, 41), RED_PIXEL);
    assert_rests(&mut compositor);
}

/// A second grabbing popup on the window while a first holds the grab: xdg-shell allows it, so
/// the first is dismissed and the second takes the keyboard, rather than Smithay's protocol
/// error ending the client.
#[test]
fn a_new_grab_on_the_window_ends_the_older_one() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = mapped_window(&mut compositor, &mut client, start);
    client.bind_keyboard(&mut compositor);

    let first = client.create_popup(Parent::Window(w), menu());
    client.grab_popup(first, client.enter_serial());
    client.commit_popup(first);
    client.roundtrip(&mut compositor);
    let first_surface = client.popup(first).surface.clone();
    assert_eq!(client.keyboard_focus().surface, Some(first_surface));

    let second = client.create_popup(Parent::Window(w), menu());
    client.grab_popup(second, client.enter_serial());
    client.commit_popup(second);
    client.roundtrip(&mut compositor);
    assert!(client.popup(first).done);
    assert!(!client.popup(second).done);
    let second_surface = client.popup(second).surface.clone();
    assert_eq!(client.keyboard_focus().surface, Some(second_surface));
    // The popups' commits armed their window's next boundary.
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_rests(&mut compositor);
}

/// Story 4: while its window is on a workspace not shown, a popup that keeps asking gets one
/// frame callback a second, as the window would, and its content updates are discarded; when the
/// window unmaps, its popup is dismissed.
#[test]
fn a_hidden_windows_popups_are_called_back_once_a_second_and_go_with_it() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = mapped_window(&mut compositor, &mut client, start);
    let p = client.map_popup(&mut compositor, Parent::Window(w), menu(), GREEN);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(client.popup(p).frames, vec![millis(boundary(start, 2))]);
    // It asks again at once, as a game's overlay may.
    client.request_popup_frame(p);
    client.commit_popup(p);
    client.roundtrip(&mut compositor);

    compositor.change_layout(|layout| {
        layout
            .move_to_special(OWNER_SEAT, "scratch")
            .expect("the window moves");
    });
    advance_to(&mut compositor, &mut client, boundary(start, 3));
    let drawn_when_hidden = compositor.counters().frames_drawn;
    let hidden_at = compositor.now();
    let mut answered = client.popup(p).frames.len();
    while compositor.now() < hidden_at + Duration::from_secs(3) {
        let step = compositor.now() + Duration::from_millis(10);
        advance_to(&mut compositor, &mut client, step);
        if client.popup(p).frames.len() > answered {
            answered = client.popup(p).frames.len();
            client.request_popup_frame(p);
            client.request_popup_feedback(p);
            client.commit_popup(p);
            client.roundtrip(&mut compositor);
        }
    }
    let since_hidden = &client.popup(p).frames[1..];
    let expected: Vec<u32> = (1..=3)
        .map(|second| millis(boundary(start, 2) + Duration::from_secs(second)))
        .collect();
    assert_eq!(since_hidden, expected.as_slice());
    assert_eq!(compositor.counters().frames_drawn, drawn_when_hidden);
    let feedback = &client.popup(p).feedback;
    assert_eq!(feedback.len(), 4);
    assert!(feedback[1..].iter().all(|f| *f == Feedback::Discarded));

    client.unmap(w);
    client.roundtrip(&mut compositor);
    assert!(client.popup(p).done);
    client.destroy_popup(p);
    client.roundtrip(&mut compositor);
    // The last callback asked is answered within the second; then nothing runs.
    advance_to(
        &mut compositor,
        &mut client,
        hidden_at + Duration::from_secs(5),
    );
    assert_rests(&mut compositor);
}

/// Story 5: popups whose parent goes first, a popup of a window not mapped, popups nested three
/// deep, and a parent popup destroyed before its child, which xdg-shell makes a protocol error:
/// Yuna survives each, and a second client keeps getting its frame callbacks.
#[test]
fn popups_misused_take_nothing_down() {
    let mut compositor = manual_compositor();
    let mut other = Client::connect(&mut compositor);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let steady = open_and_commit(&mut compositor, &mut other, BLUE);
    advance_to(&mut compositor, &mut other, boundary(start, 1));
    // From here, each step asks a frame of the steady window and checks it comes at the next
    // boundary.
    let mut k = 1;
    let mut next_boundary = |compositor: &mut Compositor, other: &mut Client| {
        other.request_frame(steady);
        other.commit(steady);
        other.roundtrip(compositor);
        let frames = other.window(steady).frames.len();
        k += 1;
        advance_to(compositor, other, boundary(start, k));
        assert_eq!(other.window(steady).frames.len(), frames + 1);
    };
    next_boundary(&mut compositor, &mut other);

    // A toplevel destroyed before its popup.
    let w = open_and_commit(&mut compositor, &mut client, RED);
    next_boundary(&mut compositor, &mut other);
    let p = client.map_popup(&mut compositor, Parent::Window(w), menu(), GREEN);
    client.close(w);
    client.roundtrip(&mut compositor);
    assert!(client.popup(p).done);
    client.destroy_popup(p);
    next_boundary(&mut compositor, &mut other);

    // A toplevel's surface destroyed before its popup.
    let w = open_and_commit(&mut compositor, &mut client, RED);
    next_boundary(&mut compositor, &mut other);
    let p = client.map_popup(&mut compositor, Parent::Window(w), menu(), GREEN);
    client.destroy_surface(w);
    client.roundtrip(&mut compositor);
    assert!(client.popup(p).done);
    client.destroy_popup(p);
    next_boundary(&mut compositor, &mut other);

    // A popup of a window not mapped: dismissed at once, never configured.
    let unmapped = client.open_window(&mut compositor);
    let p = client.open_popup(&mut compositor, Parent::Window(unmapped), menu());
    assert!(client.popup(p).done);
    assert!(client.popup(p).configures.is_empty());
    client.destroy_popup(p);
    next_boundary(&mut compositor, &mut other);

    // Popups nested three deep, each at (11, 11) of its parent.
    let w = open_and_commit(&mut compositor, &mut client, RED);
    next_boundary(&mut compositor, &mut other);
    let first = client.map_popup(&mut compositor, Parent::Window(w), menu(), GREEN);
    let second = client.map_popup(&mut compositor, Parent::Popup(first), menu(), BLUE);
    let third = client.map_popup(&mut compositor, Parent::Popup(second), menu(), GREEN);
    for popup in [first, second, third] {
        let configure = client.popup(popup).configures[0];
        assert_eq!((configure.x, configure.y), (11, 11));
        assert!(!client.popup(popup).done);
    }
    next_boundary(&mut compositor, &mut other);
    client.roundtrip(&mut compositor);
    for popup in [first, second, third] {
        assert_eq!(client.popup(popup).frames.len(), 1);
    }
    for popup in [third, second, first] {
        client.destroy_popup(popup);
    }
    next_boundary(&mut compositor, &mut other);

    // A parent popup destroyed while its child lives: a protocol error ends this client alone.
    let first = client.map_popup(&mut compositor, Parent::Window(w), menu(), GREEN);
    let second = client.map_popup(&mut compositor, Parent::Popup(first), menu(), BLUE);
    assert!(!client.popup(second).done);
    client.destroy_popup(first);
    client.send();
    drop(client);
    next_boundary(&mut compositor, &mut other);
    next_boundary(&mut compositor, &mut other);
    assert_eq!(compositor.layout().focused(OWNER_SEAT), Some(WindowId(1)));
    assert_rests(&mut compositor);
}

/// A cycle of popup parents, which xdg-shell forbids but Smithay does not refuse: each popup's
/// `xdg_surface` is the other's parent before it has a role. Both are dismissed, and Yuna does
/// not walk the cycle for ever.
#[test]
fn a_cycle_of_popup_parents_is_dismissed() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = mapped_window(&mut compositor, &mut client, start);
    let (a, b) = client.create_popup_cycle(menu());
    client.commit_popup(a);
    client.commit_popup(b);
    client.roundtrip(&mut compositor);
    assert!(client.popup(a).done);
    assert!(client.popup(b).done);

    // The window and its own popups still work.
    let p = client.map_popup(&mut compositor, Parent::Window(w), menu(), GREEN);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(client.popup(p).frames, vec![millis(boundary(start, 2))]);
    assert_rests(&mut compositor);
}

/// Popups nest at most [`MAX_POPUP_DEPTH`] deep: the next one is dismissed.
#[test]
fn a_popup_nested_past_the_limit_is_dismissed() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = mapped_window(&mut compositor, &mut client, start);
    let mut parent = Parent::Window(w);
    for _ in 0..MAX_POPUP_DEPTH {
        let popup = client.create_popup(parent, menu());
        parent = Parent::Popup(popup);
    }
    let too_deep = client.create_popup(parent, menu());
    client.roundtrip(&mut compositor);
    assert_eq!(MAX_POPUP_DEPTH, 32);
    assert!(!client.popup(too_deep - 1).done);
    assert!(client.popup(too_deep).done);
    assert_rests(&mut compositor);
}
