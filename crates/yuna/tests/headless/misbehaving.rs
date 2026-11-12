//! Misbehaving clients (ADR-0007, ADR-0025): clients that ignore their size, send absurd values,
//! break the protocol, leave mid-frame or flood. None takes Yuna down, nor another client's
//! frames.
//!
//! Every story has a well-behaved client, the sentinel, whose window opened first and was drawn
//! at boundary 1, and ends the same way ([`the_others_go_on`]): Yuna still dispatches, the layout
//! keeps its invariants and agrees with the shell, the sentinel's next frame callback comes at
//! the exact next boundary, and then Yuna rests.

use std::time::Duration;

use wayland_client::protocol::{wl_shm, wl_surface};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use yuna::Compositor;
use yuna::yuna_layout::WindowId;

use crate::client::{Client, Parent, Placement, ProtocolError};
use crate::popups::pixel;
use crate::selection_client::{SelectionClient, SelectionEvent};
use crate::{
    BLUE, OUTPUT, RED, TILE, advance_to, assert_rests, boundary, manual_compositor, millis,
    open_and_commit,
};

const GREEN: u32 = 0xFF_00_FF_00;
/// Colours as `read_pixels` gives them: R, G, B, A.
const GREEN_PIXEL: [u8; 4] = [0, 255, 0, 255];
const BLUE_PIXEL: [u8; 4] = [0, 0, 255, 255];
const RED_PIXEL: [u8; 4] = [255, 0, 0, 255];
const BLACK_PIXEL: [u8; 4] = [0, 0, 0, 255];
const BACKGROUND_PIXEL: [u8; 4] = [20, 20, 26, 255];

/// The well-behaved client and its window.
struct Sentinel {
    client: Client,
    window: usize,
}

/// Connects the sentinel and draws its window at boundary 1: window 1, the first column.
fn sentinel(compositor: &mut Compositor, start: Duration) -> Sentinel {
    let mut client = Client::connect(compositor);
    let window = open_and_commit(compositor, &mut client, GREEN);
    advance_to(compositor, &mut client, boundary(start, 1));
    assert_eq!(
        client.window(window).frames,
        vec![millis(boundary(start, 1))]
    );
    Sentinel { client, window }
}

/// The first refresh boundary after `now`, at 240 Hz from `start`.
fn next_boundary(start: Duration, now: Duration) -> Duration {
    let elapsed = now.saturating_sub(start).as_nanos();
    let mut k = u64::try_from(elapsed * 240 / 1_000_000_000).expect("a boundary");
    while boundary(start, k) <= now {
        k += 1;
    }
    boundary(start, k)
}

/// The end of every story: Yuna still dispatches; the layout keeps its invariants and agrees
/// with the shell; the sentinel's frame callback, asked now, comes at the next boundary exactly,
/// not a nanosecond before; then rest.
fn the_others_go_on(compositor: &mut Compositor, start: Duration, sentinel: &mut Sentinel) {
    sentinel.client.roundtrip(compositor);
    compositor
        .layout()
        .check_invariants()
        .expect("the layout keeps its invariants");
    compositor
        .check_windows()
        .expect("the layout and the shell agree");

    let w = sentinel.window;
    let frames = sentinel.client.window(w).frames.len();
    sentinel.client.request_frame(w);
    sentinel.client.commit(w);
    sentinel.client.roundtrip(compositor);
    let next = next_boundary(start, compositor.now());
    advance_to(
        compositor,
        &mut sentinel.client,
        next - Duration::from_nanos(1),
    );
    assert_eq!(sentinel.client.window(w).frames.len(), frames);
    advance_to(compositor, &mut sentinel.client, next);
    assert_eq!(sentinel.client.window(w).frames[frames..], [millis(next)]);
    assert_rests(compositor);
}

/// Where the layout draws `window` on the default output: x, y, width, height.
fn drawn_at(compositor: &Compositor, window: u64) -> (f64, f64, f64, f64) {
    let placed = compositor.layout().arrange(OUTPUT).expect("the output");
    let rect = placed
        .iter()
        .find(|placed| placed.window == WindowId(window))
        .expect("the window is placed")
        .rect;
    (rect.x, rect.y, rect.w, rect.h)
}

/// Opens a window of a misbehaving client: its first configure acknowledged, as xdg-shell
/// requires before a buffer, then a buffer of `size` committed. It acknowledges nothing after.
fn open_unacknowledging(
    compositor: &mut Compositor,
    client: &mut Client,
    size: (i32, i32),
) -> usize {
    let window = client.open_window(compositor);
    client.ack(window);
    client.attach(window, size.0, size.1, BLUE);
    client.commit(window);
    client.roundtrip(compositor);
    window
}

/// A window that never acknowledges a configure after its first commits sizes of its own,
/// smaller and larger than its tile, and taller than the output: its column is as wide as what
/// it commits, from 1,288 (16 + 1,256 + 16), and it is drawn at its committed size; the
/// sentinel's column keeps its place.
#[test]
fn a_tiled_window_that_ignores_its_configures() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut client = Client::connect(&mut compositor);
    let w = open_unacknowledging(&mut compositor, &mut client, (700, 300));
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    let asked = client.window(w).configures.last().expect("a configure");
    assert_eq!((asked.width, asked.height), TILE);
    assert_eq!(drawn_at(&compositor, 1), (16.0, 16.0, 1256.0, 1408.0));
    assert_eq!(drawn_at(&compositor, 2), (1288.0, 16.0, 700.0, 300.0));

    for (size, at) in [
        ((900, 1600), boundary(start, 3)),
        ((40, 30), boundary(start, 4)),
    ] {
        client.attach(w, size.0, size.1, RED);
        client.commit(w);
        client.roundtrip(&mut compositor);
        advance_to(&mut compositor, &mut client, at);
        let (_, _, width, height) = drawn_at(&compositor, 2);
        assert_eq!((width, height), (f64::from(size.0), f64::from(size.1)));
    }
    assert_eq!(drawn_at(&compositor, 1), (16.0, 16.0, 1256.0, 1408.0));
    // It was asked its tile, and activated, once each: nothing more for what it ignored.
    assert_eq!(client.window(w).configures.len(), 2);
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// A dialog that never acknowledges a configure after its first floats at what it commits,
/// keeping its top-left corner: first 640 × 480, centred on its parent, then 300 × 200, then
/// 4,000 × 3,000, past the output.
#[test]
fn a_floating_window_that_ignores_its_configures() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut client = Client::connect(&mut compositor);
    open_and_commit(&mut compositor, &mut client, RED);
    let dialog = client.create_window();
    client.set_parent(dialog, 0);
    client.commit(dialog);
    client.roundtrip(&mut compositor);
    let first = client.window(dialog).configures[0].clone();
    assert_eq!((first.width, first.height), (0, 0));
    client.ack(dialog);
    client.attach(dialog, 640, 480, BLUE);
    client.commit(dialog);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    // Centred on its parent, the second column: 1,288 + (1,256 − 640) / 2, 16 + (1,408 − 480) / 2.
    let corner = drawn_at(&compositor, 3);
    assert_eq!(corner, (1596.0, 480.0, 640.0, 480.0));

    for (size, at) in [
        ((300, 200), boundary(start, 3)),
        ((4000, 3000), boundary(start, 4)),
    ] {
        client.attach(dialog, size.0, size.1, RED);
        client.commit(dialog);
        client.roundtrip(&mut compositor);
        advance_to(&mut compositor, &mut client, at);
        assert_eq!(
            drawn_at(&compositor, 3),
            (corner.0, corner.1, f64::from(size.0), f64::from(size.1))
        );
    }
    assert_eq!(drawn_at(&compositor, 1), (16.0, 16.0, 1256.0, 1408.0));
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// A window whose buffer is 1 × 1 is a column 1 wide, 1.5 pixels at scale 1.5, snapped to the
/// physical grid: 2 pixels, 4/3 of a logical unit, from (1,932, 24), drawn blue.
#[test]
fn a_one_pixel_window() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut client = Client::connect(&mut compositor);
    open_unacknowledging(&mut compositor, &mut client, (1, 1));
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(
        drawn_at(&compositor, 2),
        (1288.0, 16.0, 4.0 / 3.0, 4.0 / 3.0)
    );
    assert_eq!(pixel(&mut compositor, 1932, 24), BLUE_PIXEL);
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// A window whose buffer is 8,192 × 8,192 pixels, 256 MiB, the largest these stories take: its
/// column starts at 1,288 and is 8,192 wide and tall, drawn black from pixel 1,932 to the
/// output's edge. The sentinel takes the focus back, and
/// with it the view, so that both are shown.
#[test]
fn an_8192_pixel_window() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut client = Client::connect(&mut compositor);
    let w = client.open_window(&mut compositor);
    client.ack(w);
    let surface = client.window(w).surface.clone();
    client.attach_black(&surface, 8192, 8192);
    client.commit(w);
    client.roundtrip(&mut compositor);
    compositor.change_layout(|layout| layout.focus_window(WindowId(1)).expect("window 1 exists"));
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_eq!(drawn_at(&compositor, 2), (1288.0, 16.0, 8192.0, 8192.0));
    assert_eq!(pixel(&mut compositor, 1931, 24), BACKGROUND_PIXEL);
    assert_eq!(pixel(&mut compositor, 1932, 24), BLACK_PIXEL);
    assert_eq!(pixel(&mut compositor, 3839, 2159), BLACK_PIXEL);
    assert_eq!(pixel(&mut compositor, 1907, 2135), GREEN_PIXEL);
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// A window geometry of no area, of a negative size, or a negative minimum or maximum, is the
/// client's protocol error: it ends that client alone, its window with it.
#[test]
fn limits_and_geometry_that_break_the_protocol_end_their_client() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let toplevel_size = |code: xdg_toplevel::Error| ProtocolError {
        interface: "xdg_toplevel".to_owned(),
        code: code as u32,
    };
    let geometry_size = ProtocolError {
        interface: "xdg_surface".to_owned(),
        code: xdg_surface::Error::InvalidSize as u32,
    };
    let breaches: [(Breach, ProtocolError); 5] = [
        (
            |client, w| client.set_min_size(w, -1, 100),
            toplevel_size(xdg_toplevel::Error::InvalidSize),
        ),
        (
            |client, w| client.set_max_size(w, 100, i32::MIN),
            toplevel_size(xdg_toplevel::Error::InvalidSize),
        ),
        (
            |client, w| client.set_window_geometry(w, (0, 0), (0, 0)),
            geometry_size.clone(),
        ),
        (
            |client, w| client.set_window_geometry(w, (10, 10), (-100, 50)),
            geometry_size.clone(),
        ),
        (
            |client, w| client.set_window_geometry(w, (0, 0), (100, i32::MIN)),
            geometry_size,
        ),
    ];
    for (breach, expected) in breaches {
        let mut client = Client::connect(&mut compositor);
        let w = open_and_commit(&mut compositor, &mut client, BLUE);
        breach(&mut client, w);
        client.commit(w);
        assert_eq!(client.protocol_error(&mut compositor), expected);
        drop(client);
        sentinel.client.roundtrip(&mut compositor);
        let placed = compositor.layout().arrange(OUTPUT).expect("the output");
        assert_eq!(placed.len(), 1);
    }
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// Limits valid in the protocol but absurd are clamped or dropped, never a reason not to open.
/// A minimum of `i32::MAX` is the layout's limit, 100,000: the window opens tiled in the second
/// column and is asked 100,000 × 100,000, which overflows its column as a minimum may. With the
/// same maximum its size is fixed and it floats, centred on the output at its committed
/// 500 × 400: (2,560 − 500) / 2, (1,440 − 400) / 2. A maximum below the minimum is dropped: the
/// window is asked its tile. Each is drawn at the 500 × 400 it commits.
#[test]
fn absurd_limits_are_clamped_or_dropped() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut client = Client::connect(&mut compositor);
    let huge = (100_000, 100_000);
    for (window, (min, max), asked, floating, rect) in [
        (
            2,
            ((i32::MAX, i32::MAX), (0, 0)),
            huge,
            false,
            (1288.0, 16.0, 500.0, 400.0),
        ),
        (
            3,
            ((i32::MAX, i32::MAX), (i32::MAX, i32::MAX)),
            huge,
            true,
            (1030.0, 520.0, 500.0, 400.0),
        ),
        (
            4,
            ((800, 600), (400, 300)),
            TILE,
            false,
            (1288.0, 16.0, 500.0, 400.0),
        ),
    ] {
        let w = client.create_window();
        client.set_min_size(w, min.0, min.1);
        client.set_max_size(w, max.0, max.1);
        client.commit(w);
        client.roundtrip(&mut compositor);
        let first = client.window(w).configures[0].clone();
        client.ack(w);
        client.attach(w, 500, 400, BLUE);
        client.commit(w);
        client.roundtrip(&mut compositor);
        let at = next_boundary(start, compositor.now());
        advance_to(&mut compositor, &mut client, at);
        assert_eq!((first.width, first.height), asked);
        let info = compositor
            .layout()
            .window(WindowId(window))
            .expect("mapped");
        assert_eq!(info.floating, floating);
        assert_eq!(drawn_at(&compositor, window), rect);
        client.close(w);
        client.roundtrip(&mut compositor);
    }
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// A window geometry outside its buffer is cut to it, and one wholly outside is the buffer's:
/// a 640 × 480 buffer is drawn 640 × 480 under a geometry wholly outside it, at the extremes of
/// `i32` included, and 40 × 80 under one at (600, 400) of 100 × 100.
#[test]
fn a_window_geometry_outside_its_buffer_is_cut_to_it() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut client = Client::connect(&mut compositor);
    let w = open_unacknowledging(&mut compositor, &mut client, (640, 480));
    for (geometry, drawn) in [
        (((5000, 5000), (100, 100)), (640.0, 480.0)),
        (((600, 400), (100, 100)), (40.0, 80.0)),
        (((i32::MAX, i32::MAX), (i32::MAX, i32::MAX)), (640.0, 480.0)),
        (((i32::MIN, i32::MIN), (i32::MAX, i32::MAX)), (640.0, 480.0)),
        (((-100, -100), (50, 50)), (640.0, 480.0)),
    ] {
        client.set_window_geometry(w, geometry.0, geometry.1);
        client.attach(w, 640, 480, BLUE);
        client.commit(w);
        client.roundtrip(&mut compositor);
        let at = next_boundary(start, compositor.now());
        advance_to(&mut compositor, &mut client, at);
        let (_, _, width, height) = drawn_at(&compositor, 2);
        assert_eq!((width, height), drawn, "{geometry:?}");
    }
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// A misbehaving client's request on its mapped window, which ends that client.
type Breach = fn(&mut Client, usize);

/// Protocol breaches end their client alone, its window with it, each with the error xdg-shell
/// or `wl_surface` names: an acknowledgement of a serial never sent; the `xdg_surface` destroyed
/// before its toplevel; a second role on the toplevel's surface, a popup's or a second toplevel's,
/// on its `xdg_surface` or on a second one; a buffer attached at an offset by a `wl_surface` of
/// version 6. A buffer before the first acknowledgement is one too.
#[test]
fn protocol_breaches_end_their_client_alone() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let error = |interface: &str, code: u32| ProtocolError {
        interface: interface.to_owned(),
        code,
    };
    let breaches: [(Breach, ProtocolError); 7] = [
        (
            |client, w| client.ack_serial(w, u32::MAX),
            error(
                "xdg_wm_base",
                xdg_wm_base::Error::InvalidSurfaceState as u32,
            ),
        ),
        (
            |client, w| client.destroy_xdg_surface(w),
            error("xdg_wm_base", xdg_wm_base::Error::Role as u32),
        ),
        (
            |client, w| {
                client.create_popup_on_window_surface(w, Parent::Window(w));
            },
            error("xdg_wm_base", xdg_wm_base::Error::Role as u32),
        ),
        (
            |client, w| {
                let menu = Placement::below_right((200, 100), (10, 10, 1, 1));
                let p = client.create_popup(Parent::Window(w), menu);
                client.create_popup_on_popup_surface(p, Parent::Window(w));
            },
            error("xdg_surface", xdg_surface::Error::AlreadyConstructed as u32),
        ),
        (
            |client, w| {
                client.create_second_toplevel(w, true);
            },
            error("xdg_surface", xdg_surface::Error::AlreadyConstructed as u32),
        ),
        (
            |client, w| {
                client.create_second_toplevel(w, false);
            },
            error("xdg_surface", xdg_surface::Error::AlreadyConstructed as u32),
        ),
        (
            |client, w| {
                let surface = client.window(w).surface.clone();
                client.attach_at_offset(&surface, (100, 100), RED, (5, 0));
            },
            error("wl_surface", wl_surface::Error::InvalidOffset as u32),
        ),
    ];
    for (breach, expected) in breaches {
        let mut client = Client::connect(&mut compositor);
        let w = open_and_commit(&mut compositor, &mut client, BLUE);
        let at = next_boundary(start, compositor.now());
        advance_to(&mut compositor, &mut client, at);
        breach(&mut client, w);
        client.commit(w);
        assert_eq!(client.protocol_error(&mut compositor), expected);
        drop(client);
        sentinel.client.roundtrip(&mut compositor);
        compositor
            .check_windows()
            .expect("the layout and the shell agree");
        assert_eq!(
            compositor
                .layout()
                .arrange(OUTPUT)
                .expect("the output")
                .len(),
            1
        );
    }

    let mut client = Client::connect(&mut compositor);
    let w = client.open_window(&mut compositor);
    client.attach(w, 100, 100, BLUE);
    client.commit(w);
    assert_eq!(
        client.protocol_error(&mut compositor),
        error("xdg_surface", xdg_surface::Error::UnconfiguredBuffer as u32)
    );
    drop(client);
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// A client shrinks the memory of its buffer's pool after `create_pool`, so that reading the
/// buffer raises SIGBUS. Yuna reads it when it draws, at boundary 2: Smithay's handler for SIGBUS
/// turns the fault into `wl_shm.invalid_fd` on the buffer, which ends the client. The process
/// lives; the window's buffer is left out, so that boundary has nothing new to draw; and the
/// sentinel goes on.
#[test]
fn a_pool_shrunk_under_its_buffer_ends_its_client_alone() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut client = Client::connect(&mut compositor);
    let w = client.open_window(&mut compositor);
    client.ack(w);
    let surface = client.window(w).surface.clone();
    let file = client.attach_shrinkable(&surface, (640, 480), BLUE);
    client.commit(w);
    client.roundtrip(&mut compositor);
    file.set_len(0).expect("the pool's file shrinks");

    let drawn = compositor.counters().frames_drawn;
    compositor
        .advance(boundary(start, 2) - compositor.now())
        .expect("the manual clock moves");
    assert_eq!(compositor.counters().frames_drawn, drawn);
    assert_eq!(
        client.protocol_error(&mut compositor),
        ProtocolError {
            interface: "wl_buffer".to_owned(),
            code: wl_shm::Error::InvalidFd as u32,
        }
    );
    // Its window's place is the background: 1,288 logical units are pixel 1,932.
    assert_eq!(pixel(&mut compositor, 1940, 30), BACKGROUND_PIXEL);
    drop(client);
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// A client commits a new frame of its window and of its popup, with frame callbacks and
/// feedback, then leaves before the boundary: the frame is drawn, once, at boundary 3; its window
/// closes when the display sees it gone, and the next boundary draws without it.
#[test]
fn a_client_leaving_mid_frame_takes_nothing_down() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut client = Client::connect(&mut compositor);
    let w = open_and_commit(&mut compositor, &mut client, BLUE);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    let menu = Placement::below_right((200, 100), (10, 10, 1, 1));
    let p = client.map_popup(&mut compositor, Parent::Window(w), menu, RED);

    client.attach(w, TILE.0, TILE.1, RED);
    client.request_frame(w);
    client.request_feedback(w);
    client.commit(w);
    client.attach_popup(p, 200, 100, BLUE);
    client.request_popup_frame(p);
    client.commit_popup(p);
    client.roundtrip(&mut compositor);
    drop(client);
    let drawn = compositor.counters().frames_drawn;
    compositor
        .advance(boundary(start, 3) - compositor.now())
        .expect("the manual clock moves");
    assert_eq!(compositor.counters().frames_drawn, drawn + 1);

    sentinel.client.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().window(WindowId(2)), None);
    advance_to(&mut compositor, &mut sentinel.client, boundary(start, 4));
    assert_eq!(compositor.counters().frames_drawn, drawn + 2);
    assert_eq!(pixel(&mut compositor, 1940, 30), BACKGROUND_PIXEL);
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// A client leaves while it holds the keyboard through a popup's grab, the clipboard through a
/// source of its own, and three activation tokens not used: its window and popup go, the grab
/// ends, the clipboard empties, which a clipboard manager is told, and the keyboard goes to the
/// sentinel's window at the next boundary.
#[test]
fn a_client_leaving_with_a_grab_a_selection_and_tokens() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    sentinel.client.bind_keyboard(&mut compositor);
    let mut manager = SelectionClient::connect_data_control(&mut compositor);
    let mut leaving = SelectionClient::connect(&mut compositor);
    let w = open_and_commit(&mut compositor, &mut leaving.client, BLUE);
    leaving.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut leaving.client, boundary(start, 2));
    leaving.roundtrip(&mut compositor);
    let serial = leaving.enter_serial();

    let menu = Placement::below_right((200, 100), (10, 10, 1, 1));
    let p = leaving.client.create_popup(Parent::Window(w), menu);
    leaving.client.grab_popup(p, serial);
    leaving.client.commit_popup(p);
    leaving.roundtrip(&mut compositor);
    leaving.client.ack_popup(p);
    leaving.client.attach_popup(p, 200, 100, RED);
    leaving.client.commit_popup(p);
    leaving.set_clipboard(&["text/plain;charset=utf-8"], serial);
    let surface = leaving.client.window(w).surface.clone();
    for _ in 0..3 {
        leaving.client.ask_token(Some(serial), Some(&surface));
    }
    leaving.roundtrip(&mut compositor);
    manager.roundtrip(&mut compositor);
    assert_eq!(
        manager.state.events.last(),
        Some(&SelectionEvent::ControlClipboard(Some(0)))
    );
    // The grab holds: the keyboard went from the window to the menu.
    assert!(!leaving.client.popup(p).done);
    assert_eq!(leaving.state.enters.len(), 2);

    drop(leaving);
    sentinel.client.roundtrip(&mut compositor);
    manager.roundtrip(&mut compositor);
    assert_eq!(
        manager.state.events.last(),
        Some(&SelectionEvent::ControlClipboard(None))
    );
    assert_eq!(compositor.layout().window(WindowId(2)), None);
    let at = next_boundary(start, compositor.now());
    advance_to(&mut compositor, &mut sentinel.client, at);
    let sentinel_surface = sentinel.client.window(sentinel.window).surface.clone();
    assert_eq!(
        sentinel.client.keyboard_focus().surface,
        Some(sentinel_surface)
    );
    the_others_go_on(&mut compositor, start, &mut sentinel);
    manager.roundtrip(&mut compositor);
}

/// A client floods: a new buffer, then 10,000 commits of its damage without a frame callback,
/// all between boundaries 2 and 3. Yuna takes every commit and draws one frame, at boundary 3,
/// of the last content.
#[test]
fn ten_thousand_commits_between_two_boundaries_draw_one_frame() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut client = Client::connect(&mut compositor);
    let w = open_and_commit(&mut compositor, &mut client, BLUE);
    advance_to(&mut compositor, &mut client, boundary(start, 2));

    let before = compositor.counters();
    client.attach(w, TILE.0, TILE.1, RED);
    for _ in 0..100 {
        for _ in 0..100 {
            client.damage(w, TILE.0, TILE.1);
            client.commit(w);
        }
        client.roundtrip(&mut compositor);
    }
    assert!(compositor.now() < boundary(start, 3));
    assert_eq!(compositor.counters().commits - before.commits, 10_000);
    assert_eq!(compositor.counters().frames_drawn, before.frames_drawn);
    advance_to(&mut compositor, &mut client, boundary(start, 3));
    assert_eq!(compositor.counters().frames_drawn, before.frames_drawn + 1);
    assert_eq!(pixel(&mut compositor, 1940, 30), RED_PIXEL);
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// A client that never reads its socket, and keeps asking for answers, fills its connection's
/// buffers: the display drops it once the 4 KiB it buffers for the client cannot take the next
/// event, and the others go on. Each `wl_display.sync` is answered by two events of 12 bytes,
/// `wl_callback.done` and `wl_display.delete_id`. On the development machine the client was
/// dropped after 5,100 syncs: 10,200 events, 122,400 bytes; the kernel's socket buffers set the
/// figure, which the test prints, and which other machines may change.
#[test]
fn a_client_that_never_reads_is_dropped() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut client = Client::connect(&mut compositor);
    let mut syncs = 0u64;
    'flood: for _ in 0..10_000 {
        for _ in 0..100 {
            if !client.sync_without_reading() {
                break 'flood;
            }
            syncs += 1;
        }
        compositor
            .dispatch(Some(Duration::ZERO))
            .expect("the compositor dispatches");
    }
    println!("dropped after {syncs} syncs, {} events", 2 * syncs);
    assert!(syncs > 4096 / 24 && syncs < 1_000_000, "{syncs} syncs");
    the_others_go_on(&mut compositor, start, &mut sentinel);
}

/// A client destroys a window's surface, then asks a toplevel again on its `xdg_surface`, which
/// lives on. Smithay makes one, on the dead surface: Yuna keeps no record of it, which would hold
/// a dead surface until the toplevel goes, and the client is not ended, since xdg-shell names no
/// error for it. Found by the random requests.
#[test]
fn a_toplevel_on_a_destroyed_surface_is_ignored() {
    let mut compositor = manual_compositor();
    let start = compositor.now();
    let mut sentinel = sentinel(&mut compositor, start);
    let mut client = Client::connect(&mut compositor);
    let w = client.open_window(&mut compositor);
    client.destroy_surface(w);
    client.roundtrip(&mut compositor);
    assert!(client.create_second_toplevel(w, true).is_some());
    client.roundtrip(&mut compositor);
    compositor
        .check_windows()
        .expect("the layout and the shell agree");
    drop(client);
    the_others_go_on(&mut compositor, start, &mut sentinel);
}
