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
//! manager reads each rename before the serial, while Xwayland commits the surface at once. On a
//! machine where the manager keeps up anyway, the serial comes first, and these stories guard
//! nothing there. They run the development shell's Xwayland for an agent's seat, on the real
//! clock.
//!
//! A cap refuses an X11 window outside a commit only when the layout holds [`MAX_WINDOWS`]: one
//! X11 program never passes its own cap with a window Yuna is told of, since its seat's window
//! manager tells Yuna of 1,000 windows of one X11 client at most, the cap's own figure
//! (`MAX_PROPERTIES_PER_CLIENT` in `xwayland/manager/mod.rs`).

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode, Window};
use x11rb::wrapper::ConnectionExt as _;
use yuna::yuna_layout::{OpenOptions, WindowId};
use yuna::{ClockMode, MAX_WINDOWS, MAX_WINDOWS_PER_CLIENT, OutputMode};

use crate::client::Client;
use crate::x11_client::{Image, PATIENCE, WindowSpec, X11Client, step};
use crate::x11_drags::RENAMES;
use crate::x11_windows::{X11Seat, off, open_drawn, pump, wait};

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

/// Runs the compositor's loop until the seat's Xwayland has committed nothing for half a second
/// and nothing is armed: the windows opened before have their frame callbacks, and no deadline
/// of the hidden windows would send a refused window's by chance.
fn rest(seat: &mut X11Seat) {
    let deadline = Instant::now() + PATIENCE;
    let mut commits = seat.compositor.seat_commits(seat.seat);
    let mut quiet_since = Instant::now();
    loop {
        assert!(Instant::now() < deadline, "the seat never rests");
        pump(&mut seat.compositor);
        let now = seat.compositor.seat_commits(seat.seat);
        if now != commits || seat.compositor.next_deadline().is_some() {
            commits = now;
            quiet_since = Instant::now();
        } else if quiet_since.elapsed() >= Duration::from_millis(500) {
            return;
        }
    }
}

/// `client` maps a new grey window, then renames `renamed`, a window of its own in the layout,
/// [`RENAMES`] times, and once X11 says the new window is mapped, maps a menu of 8 × 8 X11 pixels
/// at the output's corner. Returns once Yuna draws that menu: its window manager has read the
/// renames, and the new window's serial, which came before the menu's, while Xwayland committed
/// the new window's surface.
fn map_with_serial_held_back(
    seat: &mut X11Seat,
    client: X11Client,
    renamed: Window,
) -> (X11Client, Window) {
    rest(seat);
    let (client, (late, marker)) = step(&mut seat.compositor, client, move |client| {
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
        let marker = client.create(&WindowSpec {
            background: Some(GREY & 0x00FF_FFFF),
            ..WindowSpec::menu("marker", 0, 0, 8, 8)
        })?;
        client.connection.map_window(marker).map_err(fail)?;
        client.sync()?;
        Ok((late, marker))
    });
    let output = seat.outputs[0];
    wait(&mut seat.compositor, "the menu is drawn", |compositor| {
        compositor
            .drawn_x11_menus(output)
            .iter()
            .any(|drawn| drawn.window == marker)
            .then_some(())
    });
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
        .change_layout(|layout| {
            layout.open_window(seat_id, WindowId(opened.id.0 + 1), OpenOptions::default())
        })
        .expect("the layout takes the id");

    let (client, late) = map_with_serial_held_back(&mut seat, client, first);
    let _client = draws_again(&mut seat, client, late);
    assert_eq!(seat.compositor.windows().len(), 1);
}
