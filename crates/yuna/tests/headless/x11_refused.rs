//! An X11 window refused a place in the layout outside a commit still gets its frame callbacks.
//! Xwayland may commit a new window's surface, its buffer and a frame callback with it, before
//! Yuna's window manager reads the window's serial: the surface waits, and the serial's pairing
//! later opens the window, or tries to, outside any commit (`xwayland/windows.rs`). Refused, over
//! its client's cap or by the layout, the window gets its frame callbacks at the hidden windows'
//! rate all the same (`State::map` in `shell.rs`, `frames.rs`): Xwayland posts none of its next
//! damage before them, and would never draw it again.
//!
//! Each story holds the window's serial back as `x11_pairing.rs` does: just after its program
//! maps the new window, it renames another window of its own [`RENAMES`] times, and Yuna's window
//! manager reads each rename before the serial, while Xwayland commits the surface at once. Each
//! asserts that order from Yuna's counters: a manager that keeps up, the serial first, fails the
//! story rather than leaving it to guard nothing. They run the development shell's Xwayland for
//! an agent's seat, on the real clock.
//!
//! A cap refuses an X11 window outside a commit only when the layout holds [`MAX_WINDOWS`]: one
//! X11 program never passes its own cap with a window Yuna is told of, since its seat's window
//! manager tells Yuna of 1,000 windows of one X11 client at most, the cap's own figure
//! ([`yuna::MAX_PROPERTIES_PER_CLIENT`]).

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode, Window};
use x11rb::wrapper::ConnectionExt as _;
use yuna::{ClockMode, MAX_WINDOWS, MAX_WINDOWS_PER_CLIENT, OutputMode};
use yuna_types::WindowId;

use crate::client::Client;
use crate::x11_client::{Image, WindowSpec, X11Client, step};
use crate::x11_drags::RENAMES;
use crate::x11_pairing::pairings_since;
use crate::x11_windows::{X11Seat, off, open_drawn, rest, wait};

/// Grey, as X11 and Wayland pixels.
const GREY: u32 = 0xFF_80_80_80;

/// A small output, so that the windows' buffers stay small: 200 × 100 at 1.
fn small() -> OutputMode {
    OutputMode::new(200, 100, 60.0, 1.0).expect("a valid mode")
}

/// A grey window of 16 × 16 X11 pixels titled `title`, which X11 paints when it maps.
fn grey(title: &str) -> WindowSpec {
    WindowSpec {
        background: Some(GREY & 0x00FF_FFFF),
        rect: (0, 0, 16, 16),
        ..WindowSpec::named(title)
    }
}

/// `client` maps a new grey window, then renames `renamed`, a window of its own in the layout,
/// [`RENAMES`] times. Returns once Yuna has paired the new window with its surface, which it did
/// commit first, as Yuna's counters say: its window manager read the renames, then the window's
/// serial, while Xwayland committed the window's surface at once. A manager that reads the serial
/// first fails the story here.
fn map_with_serial_held_back(
    seat: &mut X11Seat,
    client: X11Client,
    renamed: Window,
) -> (X11Client, Window) {
    rest(seat);
    let before = pairings_since(&seat.compositor, (0, 0));
    let (client, late) = step(&mut seat.compositor, client, move |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let late = client.create(&grey("late"))?;
        client.connection.map_window(late).map_err(fail)?;
        for n in 0..RENAMES {
            client
                .connection
                .change_property8(
                    PropMode::REPLACE,
                    renamed,
                    AtomEnum::WM_NAME,
                    AtomEnum::STRING,
                    format!("renamed {n} times").as_bytes(),
                )
                .map_err(fail)?;
        }
        client.connection.flush().map_err(fail)?;
        client.wait_for(
            "the late window mapped",
            |event| matches!(event, Event::MapNotify(mapped) if mapped.window == late),
        )?;
        Ok(late)
    });
    let paired = wait(
        &mut seat.compositor,
        "the late window pairs",
        |compositor| {
            let paired = pairings_since(compositor, before);
            (paired != (0, 0)).then_some(paired)
        },
    );
    assert_eq!(
        paired,
        (1, 0),
        "the late window, the one window paired, its surface committed before the manager read \
         its serial"
    );
    (client, late)
}

/// Once the seat rests, so that the next commit counted is the drawing's, `client` draws into
/// `window` again: Xwayland commits its surface once the window's last frame callback is sent,
/// and only then.
fn draws_again(seat: &mut X11Seat, client: X11Client, window: Window) -> X11Client {
    rest(seat);
    let before = seat.compositor.seat_commits(seat.seat);
    let (client, ()) = step(&mut seat.compositor, client, move |client| {
        let geometry = client.geometry(window)?;
        client.draw(window, &Image::sharp(geometry.width, geometry.height))
    });
    let what = "Xwayland commits the refused window's new drawing, its frame callback sent";
    wait(&mut seat.compositor, what, |compositor| {
        (compositor.seat_commits(seat.seat) > before).then_some(())
    });
    client
}

/// Wayland clients of the owner's seat, which has no output here, open windows of 1 × 1 pixel
/// until the layout holds [`MAX_WINDOWS`], `held` of them open already. Each client opens
/// [`MAX_WINDOWS_PER_CLIENT`] at most; their windows stay while they live. Each round trip has
/// Yuna handle every request sent before it, so that the windows are open once it returns.
fn fill_the_layout(seat: &mut X11Seat, held: usize) -> Vec<Client> {
    let mut clients = Vec::new();
    let mut left = MAX_WINDOWS - held;
    while left > 0 {
        let count = left.min(MAX_WINDOWS_PER_CLIENT);
        let mut client = Client::connect(&mut seat.compositor);
        let windows: Vec<usize> = (0..count)
            .map(|_| {
                let window = client.create_window();
                client.commit(window);
                window
            })
            .collect();
        client.roundtrip(&mut seat.compositor);
        for &window in &windows {
            client.ack(window);
            client.attach(window, 1, 1, GREY);
            client.commit(window);
        }
        client.roundtrip(&mut seat.compositor);
        clients.push(client);
        left -= count;
    }
    assert_eq!(seat.compositor.windows().len(), MAX_WINDOWS);
    clients
}

/// An X11 program has a window open, and Wayland clients fill the layout to [`MAX_WINDOWS`]; the
/// program then maps one more window, whose serial comes after its surface's commit: the cap
/// refuses it then. Yuna counts it among the refused windows waiting for frame callbacks at
/// once (`check_windows`), and sends them: once its program draws into it again, Xwayland
/// commits its surface. With ten thousand windows, each check of Yuna's state costs a debug
/// build seconds: the story makes two.
#[test]
fn a_window_over_the_cap_refused_after_its_commit_gets_its_frame_callbacks() {
    let mut seat = X11Seat::new("refused-cap", &[small()], ClockMode::Real, off());
    let client = seat.client();
    let (client, first, _, _) = open_drawn(&mut seat.compositor, client, &grey("first"));
    let _fillers = fill_the_layout(&mut seat, 1);

    let (client, late) = map_with_serial_held_back(&mut seat, client, first);
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
    let _client = draws_again(&mut seat, client, late);
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
}

/// As [`a_window_over_the_cap_refused_after_its_commit_gets_its_frame_callbacks`], the layout
/// refusing the window: the id Yuna gives the next window, one past the last, is taken in the
/// layout already. That window of the layout's alone has no toplevel, against what
/// `check_windows` holds: Yuna's state is not checked here.
#[test]
fn a_window_the_layout_refuses_after_its_commit_gets_its_frame_callbacks() {
    let mut seat = X11Seat::new("refused-layout", &[small()], ClockMode::Real, off());
    let client = seat.client();
    let (client, first, opened, _) = open_drawn(&mut seat.compositor, client, &grey("first"));
    let seat_id = seat.seat;
    seat.compositor
        .hold_window_id_in_layout(seat_id, WindowId(opened.id.0 + 1))
        .expect("the layout takes the id");

    let (client, late) = map_with_serial_held_back(&mut seat, client, first);
    let _client = draws_again(&mut seat, client, late);
    assert_eq!(seat.compositor.windows().len(), 1);
}
