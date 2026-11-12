//! The cap on a client's windows: past [`MAX_WINDOWS_PER_CLIENT`], a window is not shown, as one
//! the layout refused, and its client keeps the others.

use std::time::Duration;

use yuna::yuna_layout::WindowId;
use yuna::{HIDDEN_FRAME_INTERVAL, MAX_WINDOWS, MAX_WINDOWS_PER_CLIENT};

use crate::client::Client;
use crate::{advance_to, assert_rests, manual_compositor, millis};

const GREY: u32 = 0xFF_80_80_80;

/// Story 9: a client maps 1,000 windows of 1 × 1 pixel; its 1,001st is not in the layout, yet
/// gets its frame callbacks at the hidden windows' rate, and the layout keeps its invariants.
/// Once one of the 1,000 closes, the 1,001st maps at its next commit.
#[test]
fn a_client_maps_at_most_its_cap_of_windows() {
    assert_eq!((MAX_WINDOWS_PER_CLIENT, MAX_WINDOWS), (1_000, 10_000));
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);

    // All initial commits, then all first buffers: two round trips for the 1,000.
    let windows: Vec<usize> = (0..MAX_WINDOWS_PER_CLIENT)
        .map(|_| {
            let window = client.create_window();
            client.commit(window);
            window
        })
        .collect();
    client.roundtrip(&mut compositor);
    for &window in &windows {
        client.ack(window);
        client.attach(window, 1, 1, GREY);
        client.commit(window);
    }
    client.roundtrip(&mut compositor);
    let last = WindowId(MAX_WINDOWS_PER_CLIENT as u64);
    assert!(compositor.layout().window(last).is_some());

    let over = client.open_window(&mut compositor);
    client.ack(over);
    client.attach(over, 1, 1, GREY);
    client.request_frame(over);
    client.commit(over);
    client.roundtrip(&mut compositor);
    let next = WindowId(MAX_WINDOWS_PER_CLIENT as u64 + 1);
    assert_eq!(compositor.layout().window(next), None);
    compositor
        .layout()
        .check_invariants()
        .expect("the layout keeps its invariants");

    // Its callback comes at once, through the hidden windows' deadline, then once a second.
    let committed_at = compositor.now();
    advance_to(&mut compositor, &mut client, committed_at);
    assert_eq!(client.window(over).frames, vec![millis(committed_at)]);
    client.request_frame(over);
    client.commit(over);
    client.roundtrip(&mut compositor);
    let one_second = committed_at + HIDDEN_FRAME_INTERVAL;
    advance_to(
        &mut compositor,
        &mut client,
        one_second - Duration::from_millis(1),
    );
    assert_eq!(client.window(over).frames.len(), 1);
    advance_to(&mut compositor, &mut client, one_second);
    assert_eq!(
        client.window(over).frames,
        vec![millis(committed_at), millis(one_second)]
    );
    assert_eq!(compositor.layout().window(next), None);

    // One window closes: the next commit of the one over the cap maps it.
    client.close(windows[0]);
    client.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().window(WindowId(1)), None);
    client.attach(over, 1, 1, GREY);
    client.commit(over);
    client.roundtrip(&mut compositor);
    assert!(compositor.layout().window(next).is_some());
    compositor
        .layout()
        .check_invariants()
        .expect("the layout keeps its invariants");

    let settled = compositor.now() + Duration::from_millis(5);
    advance_to(&mut compositor, &mut client, settled);
    assert_rests(&mut compositor);
}
