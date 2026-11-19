//! An X11 window's opening, from Yuna's side of XWayland: its pairing with the surface Xwayland
//! draws it into, in each order the two meet, the order taken read from Yuna's log
//! (`xwayland/windows.rs` tests every order alone, on the struct that pairs them); the size its
//! hints may ask, bounded by its seat's outputs as they are when it opens and as they change; the
//! activation token its startup id names, taken for a window of the token's own seat alone, as a
//! Wayland window's is; and the menus one Xwayland shows at once, and shows again once others go.
//!
//! The X11 stories run the development shell's Xwayland for an agent's seat, its output at 1.5
//! (`x11_windows.rs`), on the real clock.

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode, Window};
use x11rb::wrapper::ConnectionExt as _;
use yuna::yuna_layout::{OutputId, SeatId, WindowId};
use yuna::{ClockMode, Compositor, MAX_MENUS_PER_CLIENT, MAX_MENUS_PER_XWAYLAND, OutputMode};

use crate::client::Client;
use crate::seats::{two_seats, windows_of};
use crate::x11_client::{WindowSpec, X11Client, step};
use crate::x11_drags::RENAMES;
use crate::x11_windows::{X11Seat, at_1_5, off, open_drawn, pump, wait};
use crate::{BLUE, RED};

/// Runs the compositor's loop for `how_long`.
fn pump_for(compositor: &mut Compositor, how_long: Duration) {
    let until = Instant::now() + how_long;
    while Instant::now() < until {
        pump(compositor);
    }
}

/// Whether Yuna lists a window titled `title`.
fn listed(compositor: &Compositor, title: &str) -> bool {
    compositor
        .windows()
        .iter()
        .any(|window| window.title.as_deref() == Some(title))
}

/// A surface that commits its serial before the manager names its window waits for that window,
/// and pairs with it once the manager does. It is the order most pairings take, the serial
/// coming through the manager's thread: but a quick manager may win. One X11 program renames a
/// window of its own [`RENAMES`] times, then maps a menu: X11 queues the menu's serial for the
/// manager behind every rename, each of which the manager reads, while Xwayland commits the
/// menu's surface to Yuna at once. Yuna's counters say the menu was paired commit first, and the
/// menu is drawn where its program put it. `xwayland/windows.rs`'s tests force each order alone.
#[test]
fn a_surface_committed_before_its_windows_serial_is_paired_with_it() {
    let mut seat = X11Seat::new("pair-commit", &[at_1_5()], ClockMode::Real, off());
    let output = seat.outputs[0];
    let client = seat.client();
    let (client, renamed, _, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("renamed"));
    let before = seat.compositor.counters();
    let (client, menu) = step(&mut seat.compositor, client, move |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let spec = WindowSpec {
            background: Some(0x0040_C040),
            ..WindowSpec::menu("late", 300, 200, 60, 40)
        };
        let menu = client.create(&spec)?;
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
        client.connection.map_window(menu).map_err(fail)?;
        client.sync()?;
        Ok(menu)
    });
    let drawn = wait(&mut seat.compositor, "the menu is drawn", |compositor| {
        compositor
            .drawn_x11_menus(output)
            .into_iter()
            .find(|drawn| drawn.window == menu)
    });
    // At S = s = 1.5, an X11 pixel is a physical one, and the output's X11 corner is (0, 0).
    assert_eq!(drawn.rect, (300, 200, 60, 40));
    let after = seat.compositor.counters();
    assert_eq!(
        (
            after.x11_paired_commit_first - before.x11_paired_commit_first,
            after.x11_paired_serial_first - before.x11_paired_serial_first,
        ),
        (1, 0),
        "the menu, the one window paired, its surface committed before the manager named its serial"
    );
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
    drop(client);
}

/// Maps a window of `client`'s titled `title`, painted, whose `WM_NORMAL_HINTS` ask the minimum
/// `(width, height)` in X11 pixels; returns once X11 says it is mapped.
fn map_with_minimum(
    compositor: &mut Compositor,
    client: X11Client,
    title: &str,
    (width, height): (u32, u32),
) -> (X11Client, Window) {
    let spec = WindowSpec {
        background: Some(0x0060_6060),
        ..WindowSpec::named(title)
    };
    step(compositor, client, move |client| {
        let window = client.create(&spec)?;
        // `WM_SIZE_HINTS`: its flags, PMinSize, then the minimum at fields 5 and 6.
        let mut hints = vec![0_u32; 18];
        hints[0] = 1 << 4;
        hints[5] = width;
        hints[6] = height;
        client
            .connection
            .change_property32(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NORMAL_HINTS,
                AtomEnum::WM_SIZE_HINTS,
                &hints,
            )
            .map_err(|e| e.to_string())?;
        client
            .connection
            .map_window(window)
            .map_err(|e| e.to_string())?;
        client.connection.flush().map_err(|e| e.to_string())?;
        client.wait_for(
            "the window mapped",
            |event| matches!(event, Event::MapNotify(mapped) if mapped.window == window),
        )?;
        client.sync()?;
        Ok(window)
    })
}

/// Runs the compositor's loop until `window`'s size is `wanted`, for up to 10 s; returns the
/// sizes it had meanwhile, in order, the last one `wanted` unless the time ran out.
fn sizes_until(
    compositor: &mut Compositor,
    mut client: X11Client,
    window: Window,
    wanted: (u32, u32),
) -> (X11Client, Vec<(u32, u32)>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    loop {
        let (back, geometry) = step(compositor, client, move |client| client.geometry(window));
        client = back;
        let size = (geometry.width, geometry.height);
        if sizes.last() != Some(&size) {
            sizes.push(size);
        }
        if size == wanted || Instant::now() > deadline {
            return (client, sizes);
        }
        pump(compositor);
    }
}

/// The largest output of a seat whose outputs are `logical` in logical units, in X11 pixels at
/// `client`'s XWayland scale: what bounds a window's minimum.
fn in_x11_pixels(client: &X11Client, (width, height): (f64, f64)) -> (u32, u32) {
    let scale = client.scale();
    (
        (width * scale).round() as u32,
        (height * scale).round() as u32,
    )
}

/// A window whose `WM_NORMAL_HINTS` ask a minimum of 32,767 × 32,767, X11's largest, asks to be
/// mapped on an output of 2880 × 1620 pixels at 1.5. Yuna configures it to no more than its
/// output's X11 size, as a window manager bounds windows to its screens, and opens it.
#[test]
fn a_huge_minimum_size_is_bounded_by_the_output() {
    let mut seat = X11Seat::new("huge-min", &[at_1_5()], ClockMode::Real, off());
    let client = seat.client();
    let (client, window) = map_with_minimum(
        &mut seat.compositor,
        client,
        "huge-minimum",
        (32_767, 32_767),
    );
    let (client, geometry) = step(&mut seat.compositor, client, move |client| {
        client.geometry(window)
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !listed(&seat.compositor, "huge-minimum") {
        pump(&mut seat.compositor);
    }
    let opened = seat
        .compositor
        .windows()
        .into_iter()
        .find(|described| described.title.as_deref() == Some("huge-minimum"));
    println!("a minimum of 32,767 × 32,767: configured to {geometry:?}; listed as {opened:?}");
    drop(client);
    assert!(
        geometry.width <= 2880 && geometry.height <= 1620,
        "configured to {} × {}, beyond its output's 2880 × 1620",
        geometry.width,
        geometry.height
    );
    assert!(
        opened.is_some(),
        "the window bounded to its output never opened"
    );
}

/// The sizes `window` was configured to, in order, from the `ConfigureNotify` events that came
/// to its client and were not taken yet, Yuna's and its manager's both.
fn configured_sizes(
    compositor: &mut Compositor,
    client: X11Client,
    window: Window,
) -> (X11Client, Vec<(u32, u32)>) {
    step(compositor, client, move |client| {
        let sizes = client
            .take_events()?
            .into_iter()
            .filter_map(|event| match event {
                Event::ConfigureNotify(configured) if configured.window == window => {
                    Some((u32::from(configured.width), u32::from(configured.height)))
                }
                _ => None,
            })
            .collect();
        Ok(sizes)
    })
}

/// A seat without an output: a window asks a minimum of 32,767 × 32,767, and opens, shown
/// nowhere. No output bounds its minimum, which is taken as none until one comes. Once the seat
/// has an output, Yuna bounds the minimum by it and configures the window to it, never once to
/// 32,766.
#[test]
fn a_huge_minimum_size_without_an_output_is_bounded_once_one_comes() {
    let mut seat = X11Seat::new("huge-min-none", &[], ClockMode::Real, off());
    let client = seat.client();
    let (client, window) = map_with_minimum(
        &mut seat.compositor,
        client,
        "huge-minimum",
        (32_767, 32_767),
    );
    wait(&mut seat.compositor, "the window opens", |compositor| {
        listed(compositor, "huge-minimum").then_some(())
    });
    let (client, before) = configured_sizes(&mut seat.compositor, client, window);
    seat.compositor
        .add_output(seat.seat, at_1_5())
        .expect("an output");
    let largest = in_x11_pixels(&client, (1920.0, 1080.0));
    let (client, last) = sizes_until(&mut seat.compositor, client, window, largest);
    pump_for(&mut seat.compositor, Duration::from_millis(500));
    let (client, configured) = configured_sizes(&mut seat.compositor, client, window);
    println!(
        "without an output: configured to {before:?}; then on one of {largest:?} X11 pixels: \
         {configured:?}, its size {last:?}"
    );
    drop(client);
    assert!(
        configured
            .iter()
            .all(|&(width, height)| width <= largest.0 && height <= largest.1),
        "configured beyond its output's {largest:?}: {configured:?}"
    );
    assert_eq!(
        last.last(),
        Some(&largest),
        "its minimum, bounded by its output"
    );
}

/// A seat of two outputs at 1.5: 1920 × 1080 logical units, 2880 × 1620 X11 pixels, and a mode of
/// 1280 × 720, 1280 × 720 X11 pixels. A window on the smaller output whose minimum is exactly the
/// larger output opens at that size. The larger output is removed: Yuna bounds the window's
/// minimum by the output left, though the window commits nothing new and nothing moved it.
#[test]
fn a_minimum_size_follows_the_outputs_left() {
    // At the larger's scale, so that X11's scale stays as it is when the larger goes: a change of
    // scale would size every X11 window again, its limits with it.
    let smaller = OutputMode::new(1280, 720, 60.0, 1.5).expect("a valid mode");
    let mut seat = X11Seat::new("min-follows", &[at_1_5(), smaller], ClockMode::Real, off());
    let larger: OutputId = seat.outputs[0];
    // Opened on the output that stays, so that removing the other moves nothing: only the
    // limits following the outputs can change its size.
    seat.open_on(seat.outputs[1]);
    let client = seat.client();
    let at_larger = in_x11_pixels(&client, (1920.0, 1080.0));
    let (client, window) =
        map_with_minimum(&mut seat.compositor, client, "as-large-as-it", at_larger);
    let (client, sizes) = sizes_until(&mut seat.compositor, client, window, at_larger);
    assert_eq!(sizes.last(), Some(&at_larger), "opened at its minimum");
    wait(&mut seat.compositor, "the window opens", |compositor| {
        compositor
            .windows()
            .into_iter()
            .find(|described| described.title.as_deref() == Some("as-large-as-it"))
            .map(|described| described.id)
    });
    seat.compositor
        .remove_output(larger)
        .expect("the larger output goes");
    let at_smaller = in_x11_pixels(&client, (1280.0 / 1.5, 720.0 / 1.5));
    let (client, sizes) = sizes_until(&mut seat.compositor, client, window, at_smaller);
    println!("its output of {at_larger:?} removed, one of {at_smaller:?} left: {sizes:?}");
    drop(client);
    assert_eq!(
        sizes.last(),
        Some(&at_smaller),
        "its minimum bounded by the output left"
    );
}

/// A Wayland program of `seat` with two windows, the second focused, its keyboard in it, and the
/// token it asked as a launcher does, valid: with its keyboard's `enter` serial, for its focused
/// window.
struct Launcher {
    client: Client,
    /// Its first window, not focused: its index among the program's windows, and its id.
    first: (usize, WindowId),
    /// Its second window, focused.
    second: WindowId,
    token: String,
}

impl Launcher {
    fn on(compositor: &mut Compositor, seat: SeatId) -> Launcher {
        let mut client = Client::connect_to(compositor, seat);
        let mut open = |compositor: &mut Compositor, colour| {
            let window = client.open_window(compositor);
            client.ack(window);
            client.attach(window, 400, 300, colour);
            client.commit(window);
            client.roundtrip(compositor);
            window
        };
        let first = open(compositor, RED);
        let second = open(compositor, BLUE);
        let (first_id, second_id) =
            wait(
                compositor,
                "both windows open",
                |compositor| match windows_of(compositor, seat)[..] {
                    [first, second] => Some((first, second)),
                    _ => None,
                },
            );
        wait(compositor, "the second has the keyboard", |compositor| {
            (compositor.keyboard_window(seat) == Some(second_id)).then_some(())
        });
        client.bind_keyboard(compositor);
        let token = client.request_token(compositor, Some(client.enter_serial()), Some(second));
        Launcher {
            client,
            first: (first, first_id),
            second: second_id,
            token,
        }
    }

    /// Activates its first window with its token: still unused and valid, it moves the seat's
    /// focus there.
    fn activates_its_first_window(&mut self, compositor: &mut Compositor, seat: SeatId) {
        assert_eq!(compositor.layout().focused(seat), Some(self.second));
        self.client.activate(&self.token, self.first.0);
        self.client.roundtrip(compositor);
        assert_eq!(
            compositor.layout().focused(seat),
            Some(self.first.1),
            "the launcher's token was used up by another seat's window"
        );
    }
}

/// A launcher of one agent's seat asks a token; an X11 window of another agent's seat names it
/// in its `_NET_STARTUP_ID`, as a program that read the launcher's environment could. The token
/// stays the launcher's seat's: the other seat's window opens without using it up, and the
/// launcher activates its own window with it afterwards.
#[test]
fn an_x11_windows_startup_id_never_uses_another_seats_token() {
    let mut seat = X11Seat::new("token-x11", &[at_1_5()], ClockMode::Real, off());
    let x11_seat = seat.seat;
    let launcher_seat = seat.compositor.add_seat().expect("a second seat");
    seat.compositor
        .add_output(launcher_seat, at_1_5())
        .expect("its output");
    // Xwayland runs before the token is asked: the token's 10 s on the real clock are the
    // window's opening alone.
    let client = seat.client();
    let mut launcher = Launcher::on(&mut seat.compositor, launcher_seat);
    let spec = WindowSpec {
        startup_id: Some(launcher.token.clone()),
        ..WindowSpec::named("other-seat")
    };
    let (client, _, opened, _) = open_drawn(&mut seat.compositor, client, &spec);
    assert_eq!(opened.seat, x11_seat);
    launcher.activates_its_first_window(&mut seat.compositor, launcher_seat);
    drop(client);
}

/// The same on Wayland's path: a client of another agent's seat asks, with the launcher's
/// token, that its own window be activated. The token stays the launcher's seat's.
#[test]
fn a_wayland_activation_never_uses_another_seats_token() {
    let (mut compositor, launcher_seat, other_seat) = two_seats();
    let mut launcher = Launcher::on(&mut compositor, launcher_seat);
    let mut other = Client::connect_to(&mut compositor, other_seat);
    let window = other.open_window(&mut compositor);
    other.ack(window);
    other.attach(window, 400, 300, RED);
    other.commit(window);
    other.roundtrip(&mut compositor);
    let opened = wait(&mut compositor, "its window opens", |compositor| {
        windows_of(compositor, other_seat).first().copied()
    });
    other.activate(&launcher.token, window);
    other.roundtrip(&mut compositor);
    assert_eq!(compositor.layout().focused(other_seat), Some(opened));
    launcher.activates_its_first_window(&mut compositor, launcher_seat);
}

/// Maps [`MAX_MENUS_PER_CLIENT`] small override-redirect windows of a new connection to the
/// seat's Xwayland, in row `row` of the output, and waits until X11 maps them all; returns the
/// connection and its menus.
fn map_menus(seat: &mut X11Seat, row: usize) -> (X11Client, Vec<Window>) {
    let client = seat.client();
    step(&mut seat.compositor, client, move |client| {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let mut menus = Vec::new();
        for index in 0..MAX_MENUS_PER_CLIENT {
            let x = ((index % 16) * 20) as i16;
            let y = (row * 80 + index / 16 * 20) as i16;
            let spec = WindowSpec {
                background: Some(0x00C0_4040),
                ..WindowSpec::menu(&format!("menu-{row}-{index}"), x, y, 8, 8)
            };
            let menu = client.create(&spec)?;
            client.connection.map_window(menu).map_err(fail)?;
            menus.push(menu);
        }
        client.connection.flush().map_err(fail)?;
        for &menu in &menus {
            client.wait_for(
                "a menu mapped",
                |event| matches!(event, Event::MapNotify(mapped) if mapped.window == menu),
            )?;
        }
        Ok(menus)
    })
}

/// The menus of `connections` that `seat`'s output does not draw now.
fn not_drawn(
    seat: &X11Seat,
    output: OutputId,
    connections: &[(X11Client, Vec<Window>)],
) -> Vec<Window> {
    let drawn: Vec<u32> = seat
        .compositor
        .drawn_x11_menus(output)
        .iter()
        .map(|menu| menu.window)
        .collect();
    connections
        .iter()
        .flat_map(|(_, menus)| menus)
        .copied()
        .filter(|menu| !drawn.contains(menu))
        .collect()
}

/// Maps a small override-redirect window of a new connection, a tooltip that draws itself once
/// and never again; returns the connection and the tooltip once X11 maps it.
fn map_tooltip(seat: &mut X11Seat) -> (X11Client, Window) {
    let client = seat.client();
    step(&mut seat.compositor, client, |client| {
        let spec = WindowSpec {
            background: Some(0x0040_40C0),
            ..WindowSpec::menu("tooltip", 600, 600, 40, 20)
        };
        client.open(&spec)
    })
}

/// ADR-0070 counts menus per X11 client, so that one program cannot take every menu of its seat;
/// but an X11 client is a connection. One program opens five connections and maps
/// [`MAX_MENUS_PER_CLIENT`] menus on each: Yuna shows [`MAX_MENUS_PER_XWAYLAND`] of them, the menus
/// one Xwayland shows at once, and no more. Once one connection leaves, the menus that waited
/// show in its menus' place, up to the bound again; the menus of a new connection then wait.
#[test]
fn one_program_with_several_connections_shows_at_most_the_cap_of_menus() {
    const CONNECTIONS: usize = 5;
    let mut seat = X11Seat::new("menu-conns", &[at_1_5()], ClockMode::Real, off());
    let output = seat.outputs[0];
    let mut connections: Vec<(X11Client, Vec<Window>)> = (0..CONNECTIONS)
        .map(|row| map_menus(&mut seat, row))
        .collect();
    let drawn = |seat: &X11Seat| seat.compositor.drawn_x11_menus(output).len();
    wait(&mut seat.compositor, "the menus are drawn", |compositor| {
        (compositor.drawn_x11_menus(output).len() >= MAX_MENUS_PER_XWAYLAND).then_some(())
    });
    pump_for(&mut seat.compositor, Duration::from_millis(500));
    println!(
        "one program, {CONNECTIONS} X11 connections, {MAX_MENUS_PER_CLIENT} menus each: {} drawn",
        drawn(&seat)
    );
    assert_eq!(drawn(&seat), MAX_MENUS_PER_XWAYLAND);

    let (first, gone) = connections.remove(0);
    let waiting = not_drawn(&seat, output, &connections);
    assert_eq!(
        waiting.len(),
        CONNECTIONS * MAX_MENUS_PER_CLIENT - MAX_MENUS_PER_XWAYLAND,
        "the menus past the bound wait"
    );
    drop(first);
    wait(
        &mut seat.compositor,
        "the first connection's menus go, and those that waited show",
        |compositor| {
            let drawn: Vec<u32> = compositor
                .drawn_x11_menus(output)
                .iter()
                .map(|menu| menu.window)
                .collect();
            let shown = !drawn.iter().any(|menu| gone.contains(menu))
                && waiting.iter().all(|menu| drawn.contains(menu));
            shown.then_some(())
        },
    );
    pump_for(&mut seat.compositor, Duration::from_millis(500));
    assert_eq!(drawn(&seat), MAX_MENUS_PER_XWAYLAND);
    connections.push(map_menus(&mut seat, CONNECTIONS));
    pump_for(&mut seat.compositor, Duration::from_millis(500));
    assert_eq!(
        drawn(&seat),
        MAX_MENUS_PER_XWAYLAND,
        "the new menus wait in turn"
    );
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
    drop(connections);
}

/// A program floods its Xwayland with menus over five connections, past the bound of one
/// Xwayland's menus; another program then maps a tooltip, which draws itself once and never
/// again, and waits behind the flood. Once one of the flood's connections leaves, the flood's
/// menus that waited show first, mapped before the tooltip; once the whole flood leaves, the
/// tooltip shows.
#[test]
fn a_programs_tooltip_shows_once_a_flooding_programs_menus_go() {
    const CONNECTIONS: usize = 5;
    let mut seat = X11Seat::new("menu-flood", &[at_1_5()], ClockMode::Real, off());
    let output = seat.outputs[0];
    let mut flood: Vec<(X11Client, Vec<Window>)> = (0..CONNECTIONS)
        .map(|row| map_menus(&mut seat, row))
        .collect();
    wait(&mut seat.compositor, "the flood is drawn", |compositor| {
        (compositor.drawn_x11_menus(output).len() >= MAX_MENUS_PER_XWAYLAND).then_some(())
    });
    let (tooltip_client, tooltip) = map_tooltip(&mut seat);
    let tooltip_drawn = |compositor: &Compositor| {
        compositor
            .drawn_x11_menus(output)
            .iter()
            .any(|menu| menu.window == tooltip)
    };
    pump_for(&mut seat.compositor, Duration::from_millis(500));
    assert!(!tooltip_drawn(&seat.compositor), "past the bound, it waits");

    let (first, _) = flood.remove(0);
    let waiting = not_drawn(&seat, output, &flood);
    drop(first);
    wait(
        &mut seat.compositor,
        "the flood's menus that waited show",
        |compositor| {
            let drawn: Vec<u32> = compositor
                .drawn_x11_menus(output)
                .iter()
                .map(|menu| menu.window)
                .collect();
            waiting
                .iter()
                .all(|menu| drawn.contains(menu))
                .then_some(())
        },
    );
    pump_for(&mut seat.compositor, Duration::from_millis(500));
    assert!(
        !tooltip_drawn(&seat.compositor),
        "the menus that waited longer show first"
    );

    drop(flood);
    wait(&mut seat.compositor, "the tooltip shows", |compositor| {
        tooltip_drawn(compositor).then_some(())
    });
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
    drop(tooltip_client);
}
