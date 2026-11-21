//! X11 override-redirect windows, menus and tooltips: drawn where their client put them, over
//! the window they belong to and sliding with it, or alone at their own X11 position; clicked
//! on the X11 pixel drawn under the pointer
//! ([ADR-0066](../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md),
//! [ADR-0054](../../../../docs/adr/0054-motion-sampled-at-presentation-drawn-apart-from-told.md)).
//!
//! The seats are `x11_windows.rs`'s: an output at 1.5, 2880 × 1620 pixels, and one at 1, 1920 ×
//! 1080, both 1920 × 1080 logical units, XWayland's scale S 1.5. A window's tile is at (16, 16)
//! of its output, in X11 at (24, 24) of the output's X11 corner, which is (0, 0) for the output at
//! 1.5 and (2880, 0) for the one at 1.

use std::time::{Duration, Instant};

use x11rb::protocol::xproto::Window;
use yuna::yuna_motion::Settings;
use yuna::{CaptureSize, CaptureTarget, ClockMode, Compositor, DrawnX11Menu, OutputMode};
use yuna_ipc::{Caller, Command};
use yuna_types::{Direction, OutputId};

use crate::x11_client::{Image, WindowSpec, X11Client, step};
use crate::x11_windows::{
    X11Seat, area_reference, button_press_at, capture_until, largest_difference, open_drawn, wait,
};

/// The tile's corner in X11 pixels, from its output's X11 corner, at S = 1.5.
const X11_CORNER: i32 = 24;

/// The X11 corner of the output at 1, right of the one at 1.5.
const X11_SECOND_OUTPUT: i32 = 2880;

/// Maps `spec`, an override-redirect window, and draws [`Image::menu`] into it at its size, as a
/// menu's client does; returns it and its image.
fn open_menu(
    compositor: &mut Compositor,
    client: X11Client,
    spec: &WindowSpec,
) -> (X11Client, Window, Image) {
    let spec = spec.clone();
    let (client, (window, image)) = step(compositor, client, move |client| {
        let window = client.open(&spec)?;
        let image = Image::menu(u32::from(spec.rect.2), u32::from(spec.rect.3));
        client.draw(window, &image)?;
        Ok((window, image))
    });
    (client, window, image)
}

/// Where `output`'s frames draw `menu`, once they do.
fn drawn_menu(compositor: &mut Compositor, output: OutputId, menu: Window) -> DrawnX11Menu {
    wait(compositor, "the menu is drawn", |compositor| {
        compositor
            .drawn_x11_menus(output)
            .into_iter()
            .find(|drawn| drawn.window == menu)
    })
}

/// The physical pixels of `rect` of `output`'s newest frame, R, G, B, once `good` takes them.
fn pixels_until(
    seat: &mut X11Seat,
    output: OutputId,
    rect: (u32, u32, u32, u32),
    good: impl Fn(&[u8]) -> bool,
) -> Vec<u8> {
    let whole = CaptureTarget::Output {
        output: Some(output),
        region: None,
    };
    let frame = capture_until(
        &mut seat.compositor,
        seat.seat,
        whole,
        CaptureSize::Physical,
        |frame| good(&Image::crop(&frame.image(), frame.mapping.width, rect)),
    );
    Image::crop(&frame.image(), frame.mapping.width, rect)
}

#[test]
fn an_x11_menu_opens_at_its_place_over_its_window() {
    let mut seat = X11Seat::at_both("menu-place");
    let (at_one_and_a_half, at_one) = (seat.outputs[0], seat.outputs[1]);
    let client = seat.client();
    let (client, _, window, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("menus"));
    // A popup menu of the same client, 100 × 150 X11 pixels into its window, which it does not
    // name: the window whose X11 rectangle holds its corner is its.
    let spec = WindowSpec {
        window_type: Some(client.atoms._NET_WM_WINDOW_TYPE_POPUP_MENU),
        ..WindowSpec::menu(
            "menu",
            (X11_CORNER + 100) as i16,
            (X11_CORNER + 150) as i16,
            300,
            240,
        )
    };
    let (client, menu, image) = open_menu(&mut seat.compositor, client, &spec);
    let drawn = drawn_menu(&mut seat.compositor, at_one_and_a_half, menu);
    // At 1.5 an X11 pixel is a physical pixel: where its client put it, pixel for pixel.
    assert_eq!(drawn.parent, Some(window.id));
    assert_eq!(drawn.rect, (124, 174, 300, 240));
    let pixels = pixels_until(
        &mut seat,
        at_one_and_a_half,
        (124, 174, 300, 240),
        |pixels| pixels == image.pixels.concat(),
    );
    assert!(
        pixels == image.pixels.concat(),
        "the menu is drawn exactly where its client put it"
    );

    // On the output at 1: another window there, its menu 200 × 100 into it.
    seat.open_on(at_one);
    let (client, _, second, _) = open_drawn(
        &mut seat.compositor,
        client,
        &WindowSpec::named("menus-at-1"),
    );
    assert_eq!(second.output, Some(at_one));
    let spec = WindowSpec::menu(
        "menu-at-1",
        (X11_SECOND_OUTPUT + X11_CORNER + 200) as i16,
        (X11_CORNER + 100) as i16,
        300,
        240,
    );
    let (_client, menu, image) = open_menu(&mut seat.compositor, client, &spec);
    let drawn = drawn_menu(&mut seat.compositor, at_one, menu);
    // Its window's corner, 16, plus 200 / 1.5 and 100 / 1.5, rounded; 300 × 240 X11 pixels are
    // 200 × 160 there, through the area filter.
    assert_eq!(drawn.parent, Some(second.id));
    assert_eq!(drawn.rect, (16 + 133, 16 + 67, 200, 160));
    let reference = area_reference(&image, 200, 160);
    let pixels = pixels_until(&mut seat, at_one, (149, 83, 200, 160), |pixels| {
        largest_difference(pixels, &reference) <= 1
    });
    assert!(largest_difference(&pixels, &reference) <= 1);
}

#[test]
fn a_tooltip_without_a_parent_is_drawn_at_its_position() {
    let mut seat = X11Seat::at_both("tooltip");
    let at_one_and_a_half = seat.outputs[0];
    // Another program's window lies under it.
    let other = seat.client();
    let (_other, _, window, _) =
        open_drawn(&mut seat.compositor, other, &WindowSpec::named("under"));
    // A program with no window of its own shows a tooltip at X11 (300, 150): logical (200, 100),
    // drawn over every window.
    let client = seat.client();
    let spec = WindowSpec {
        window_type: Some(client.atoms._NET_WM_WINDOW_TYPE_TOOLTIP),
        ..WindowSpec::menu("tooltip", 300, 150, 150, 60)
    };
    let (_client, tooltip, image) = open_menu(&mut seat.compositor, client, &spec);
    let drawn = drawn_menu(&mut seat.compositor, at_one_and_a_half, tooltip);
    assert_eq!(drawn.parent, None);
    assert_eq!(drawn.rect, (300, 150, 150, 60));
    let place = window.place.expect("the window's place");
    assert!(
        place.x * 1.5 < 300.0 && place.right() * 1.5 > 450.0,
        "the tooltip lies over the window"
    );
    let pixels = pixels_until(
        &mut seat,
        at_one_and_a_half,
        (300, 150, 150, 60),
        |pixels| pixels == image.pixels.concat(),
    );
    assert!(
        pixels == image.pixels.concat(),
        "the tooltip is drawn over the window, pixel for pixel"
    );
}

#[test]
fn clicking_an_x11_menu_reaches_it() {
    let mut seat = X11Seat::at_both("menu-click");
    let (at_one_and_a_half, at_one) = (seat.outputs[0], seat.outputs[1]);
    let client = seat.client();
    let (client, parent, window, _) = open_drawn(
        &mut seat.compositor,
        client,
        &WindowSpec::named("menu-owner"),
    );
    // A menu that names its window, 200 × 100 X11 pixels into it.
    let spec = WindowSpec {
        transient_for: Some(parent),
        ..WindowSpec::menu(
            "clicked",
            (X11_CORNER + 200) as i16,
            (X11_CORNER + 100) as i16,
            300,
            200,
        )
    };
    let (client, menu, _) = open_menu(&mut seat.compositor, client, &spec);
    let drawn = drawn_menu(&mut seat.compositor, at_one_and_a_half, menu);
    assert_eq!(drawn.rect, (224, 124, 300, 200));
    // Drawn from logical (149.33, 82.67): a click at (200.25, 120.25) is 76.375 × 56.375 X11
    // pixels into it, on pixel (76, 56).
    crate::x11_windows::click_at(&mut seat, at_one_and_a_half, 200.25, 120.25);
    let (client, at) = button_press_at(&mut seat.compositor, client, menu);
    assert_eq!(at, (76, 56), "at 1.5");
    // The keyboard stays with the window: X11 menus grab it within Xwayland.
    assert_eq!(seat.compositor.keyboard_window(seat.seat), Some(window.id));

    // On the output at 1, the menu of a window there, drawn from (149, 83): a click 50.5 ×
    // 30.5 units into it is 75.75 × 45.75 X11 pixels, on pixel (75, 45).
    seat.open_on(at_one);
    let (client, parent, second, _) = open_drawn(
        &mut seat.compositor,
        client,
        &WindowSpec::named("menu-owner-at-1"),
    );
    let spec = WindowSpec {
        transient_for: Some(parent),
        ..WindowSpec::menu(
            "clicked-at-1",
            (X11_SECOND_OUTPUT + X11_CORNER + 200) as i16,
            (X11_CORNER + 100) as i16,
            300,
            200,
        )
    };
    let (client, menu, _) = open_menu(&mut seat.compositor, client, &spec);
    let drawn = drawn_menu(&mut seat.compositor, at_one, menu);
    assert_eq!(drawn.rect, (149, 83, 200, 133));
    crate::x11_windows::click_at(&mut seat, at_one, 149.0 + 50.5, 83.0 + 30.5);
    let (_client, at) = button_press_at(&mut seat.compositor, client, menu);
    assert_eq!(at, (75, 45), "at 1");
    assert_eq!(seat.compositor.keyboard_window(seat.seat), Some(second.id));
}

#[test]
fn an_x11_menu_slides_with_its_window() {
    let spring = yuna::yuna_motion::SpringParams::new(1.0, 800.0).expect("a valid spring");
    let settings = Settings {
        off: false,
        camera: spring,
        windows: spring,
        ..Settings::default()
    };
    let mode = OutputMode::new(2880, 1620, 240.0, 1.5).expect("a valid mode");
    let mut seat = X11Seat::new("menu-slides", &[mode], ClockMode::Manual, settings);
    let output = seat.outputs[0];
    let client = seat.client();
    let (client, _, _, _) = open_drawn(&mut seat.compositor, client, &WindowSpec::named("left"));
    let (client, right, right_described, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("right"));
    let right_id = right_described.id;
    // Its menu, 120 × 80 X11 pixels into it.
    let corner = wait(
        &mut seat.compositor,
        "the right window rests",
        |compositor| {
            compositor
                .windows()
                .into_iter()
                .find(|window| window.id == right_id && window.motion_ends.is_none())
                .and_then(|window| window.place)
        },
    );
    let spec = WindowSpec {
        transient_for: Some(right),
        ..WindowSpec::menu(
            "sliding",
            ((corner.x * 1.5).round() + 120.0) as i16,
            ((corner.y * 1.5).round() + 80.0) as i16,
            200,
            100,
        )
    };
    let (_client, menu, image) = open_menu(&mut seat.compositor, client, &spec);
    let offset_of = |compositor: &Compositor| -> Option<(i32, i32)> {
        let drawn = compositor
            .drawn_x11_menus(output)
            .into_iter()
            .find(|drawn| drawn.window == menu)?;
        assert_eq!(drawn.parent, Some(right_id));
        let window = compositor
            .drawn(output)
            .iter()
            .find(|drawn| drawn.window == right_id)?
            .rect;
        // Where the frame drew the window's corner: its rectangle's on the output's pixels.
        let x = (window.x * 1.5).round() as i32;
        let y = (window.y * 1.5).round() as i32;
        Some((drawn.rect.0 - x, drawn.rect.1 - y))
    };
    let at_rest = wait(&mut seat.compositor, "the menu is drawn", |compositor| {
        offset_of(compositor)
    });
    assert_eq!(at_rest, (120, 80));
    // Every deadline run, nothing moves.
    crate::motion::settle(&mut seat.compositor, |_| {});

    // The right column moves left: the window slides, and its menu with it.
    let id = seat.seat;
    seat.compositor
        .command(Caller::on_seat(id), Command::MoveFocused(Direction::Left))
        .expect("the column moves");
    let start: Option<f64> = seat
        .compositor
        .drawn(output)
        .iter()
        .find(|drawn| drawn.window == right_id)
        .map(|drawn| drawn.rect.x);
    let mut frames = 0;
    let mut moved = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    while let Some(at) = seat.compositor.next_deadline() {
        assert!(Instant::now() < deadline, "the motion never ends");
        let by = at.saturating_sub(seat.compositor.now());
        seat.compositor.advance(by).expect("the clock moves");
        let drawn_x = seat
            .compositor
            .drawn(output)
            .iter()
            .find(|drawn| drawn.window == right_id)
            .map(|drawn| drawn.rect.x);
        if drawn_x != start {
            moved += 1;
        }
        frames += 1;
        assert_eq!(
            offset_of(&seat.compositor),
            Some(at_rest),
            "frame {frames}: the menu's offset to its window's drawn corner"
        );
        assert!(frames < 2_000, "the motion never ends");
    }
    assert!(moved > 10, "the window slid over {moved} frames");
    // Where the last frame drew it, its pixels.
    let drawn = drawn_menu(&mut seat.compositor, output, menu);
    let (x, y, width, height) = drawn.rect;
    let rect = (x as u32, y as u32, width as u32, height as u32);
    let pixels = pixels_until(&mut seat, output, rect, |pixels| {
        pixels == image.pixels.concat()
    });
    assert!(
        pixels == image.pixels.concat(),
        "the menu is drawn at {rect:?}"
    );
}

#[test]
fn a_menu_whose_window_goes_stays_shown() {
    let mut seat = X11Seat::at_both("menu-orphan");
    let at_one_and_a_half = seat.outputs[0];
    let client = seat.client();
    let (client, parent, window, _) =
        open_drawn(&mut seat.compositor, client, &WindowSpec::named("leaving"));
    let spec = WindowSpec {
        transient_for: Some(parent),
        ..WindowSpec::menu(
            "orphan",
            (X11_CORNER + 300) as i16,
            (X11_CORNER + 200) as i16,
            150,
            90,
        )
    };
    let (client, menu, image) = open_menu(&mut seat.compositor, client, &spec);
    let drawn = drawn_menu(&mut seat.compositor, at_one_and_a_half, menu);
    assert_eq!(drawn.parent, Some(window.id));
    // Its window is withdrawn; the menu stays mapped. Its client has no other window: it is
    // drawn alone, where X11 has it.
    let (_client, ()) = step(&mut seat.compositor, client, move |client| {
        client.withdraw(parent)
    });
    let drawn = wait(
        &mut seat.compositor,
        "the menu is drawn alone",
        |compositor| {
            compositor
                .drawn_x11_menus(at_one_and_a_half)
                .into_iter()
                .find(|drawn| drawn.window == menu && drawn.parent.is_none())
        },
    );
    assert_eq!(drawn.rect, (324, 224, 150, 90));
    let pixels = pixels_until(
        &mut seat,
        at_one_and_a_half,
        (324, 224, 150, 90),
        |pixels| pixels == image.pixels.concat(),
    );
    assert!(pixels == image.pixels.concat(), "the menu is still drawn");
    seat.compositor
        .check_windows()
        .expect("the layout and the shells agree");
}
