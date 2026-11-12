//! Viewporter and fractional scale: what each surface is told of its output's scale, and a window
//! drawn at its output's physical size sitting exactly on its pixels.
//!
//! Stories run on the default output (3840 × 2160 at 240 Hz, scale 1.5) unless they say
//! otherwise; those with outputs at 60 Hz count boundaries with `boundary_at_60_hz`.

use std::time::Duration;

use smithay::utils::Rectangle;
use wayland_protocols::wp::viewporter::client::wp_viewport;
use yuna::yuna_layout::{OutputId, WindowId};
use yuna::{ClockMode, Compositor, Config, OWNER_SEAT, OutputMode};

use crate::client::{Client, ProtocolError};
use crate::{
    BLUE, OUTPUT, RED, TILE, advance_to, assert_rests, boundary, manual_compositor, open_and_commit,
};

/// `RED` and `BLUE` as `read_pixels` gives them: R, G, B, A.
const RED_PIXEL: [u8; 4] = [255, 0, 0, 255];
const BLUE_PIXEL: [u8; 4] = [0, 0, 255, 255];

/// A compositor on the manual clock with `outputs`, laid out left to right.
fn compositor_with(outputs: Vec<OutputMode>) -> Compositor {
    Compositor::new(Config {
        outputs,
        clock: ClockMode::Manual,
        listen: false,
        ..Config::default()
    })
    .expect("the compositor starts")
}

/// 1920 × 1080 at 60 Hz, at `scale`.
fn hd_output(scale: f64) -> OutputMode {
    OutputMode::new(1920, 1080, 60.0, scale).expect("a valid mode")
}

/// Refresh boundary `k` of an output created at `start`, at 60 Hz.
fn boundary_at_60_hz(start: Duration, k: u64) -> Duration {
    start + Duration::from_nanos(k * 1_000_000_000 / 60)
}

/// A checkerboard of 1-pixel squares, `RED` where `x + y` is even, `BLUE` elsewhere, row by row.
fn checkerboard(width: i32, height: i32) -> Vec<u32> {
    (0..height)
        .flat_map(|y| (0..width).map(move |x| if (x + y) % 2 == 0 { RED } else { BLUE }))
        .collect()
}

/// Story 1: on the default output, at scale 1.5, a client that makes a fractional scale object
/// for its window's surface before its initial commit is told `preferred_scale` 180 (1.5 × 120)
/// at once, before its first configure; its `wl_surface`, version 6, is told a preferred buffer
/// scale of 2, the ceiling of 1.5, at the same time. Mapping the window on that output, drawing
/// and resting send neither again.
#[test]
fn a_surface_is_told_its_outputs_scale_once() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = client.create_window();
    client.get_fractional_scale(w);
    client.roundtrip(&mut compositor);
    let window = client.window(w);
    assert!(window.configures.is_empty());
    assert_eq!(window.preferred_scales, vec![180]);
    assert_eq!(window.preferred_buffer_scales, vec![2]);

    client.commit(w);
    client.roundtrip(&mut compositor);
    assert_eq!(client.window(w).configures.len(), 1);
    client.ack(w);
    client.attach(w, TILE.0, TILE.1, RED);
    client.commit(w);
    client.roundtrip(&mut compositor);
    // The window enters the output at the boundary: the same scale, so nothing is sent.
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert_eq!(compositor.counters().frames_drawn, 1);
    // As a real client: it acknowledges the activated configure and commits.
    client.ack(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));

    assert_rests(&mut compositor);
    client.roundtrip(&mut compositor);
    let window = client.window(w);
    assert_eq!(window.preferred_scales, vec![180]);
    assert_eq!(window.preferred_buffer_scales, vec![2]);
}

/// Story 2: on the default output, a window asked 1256 × 1408 logical units, its tile, commits a
/// buffer of exactly 1884 × 2112 pixels, its size × 1.5, with a viewport destination of
/// 1256 × 1408, drawn as a checkerboard of 1-pixel squares. The layout puts it at (16, 16), so
/// its physical rectangle is (24, 24) to (1908, 2136): read back, every pixel of it is exactly
/// the checkerboard's, none blended, at all four edges, and the pixels around it are background.
#[test]
fn a_window_drawn_at_its_physical_size_sits_exactly_on_its_pixels() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = client.create_window();
    client.get_fractional_scale(w);
    client.get_viewport(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    let asked = client.window(w).configures[0].clone();
    assert_eq!((asked.width, asked.height), TILE);
    let scale = f64::from(client.window(w).preferred_scales[0]) / 120.0;
    // wp_fractional_scale_v1: the logical size times the scale, rounded half away from zero.
    let physical = |logical: i32| (f64::from(logical) * scale).round() as i32;
    let (width, height) = (physical(asked.width), physical(asked.height));
    assert_eq!((width, height), (1884, 2112));

    client.ack(w);
    client.attach_pixels(w, width, height, &checkerboard(width, height));
    client.set_viewport_destination(w, asked.width, asked.height);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 1));

    let placed = compositor
        .layout()
        .arrange(OUTPUT)
        .expect("the output exists");
    let rect = placed[0].rect;
    assert_eq!(
        (rect.x, rect.y, rect.w, rect.h),
        (16.0, 16.0, 1256.0, 1408.0)
    );
    let window_pixels = compositor
        .read_pixels(OUTPUT, Rectangle::new((24, 24).into(), (1884, 2112).into()))
        .expect("the window's rectangle reads");
    let wrong: Vec<(usize, usize, [u8; 4])> = window_pixels
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .map(|(index, pixel)| (index % 1884, index / 1884, *pixel))
        .filter(|&(x, y, pixel)| {
            let expected = if (x + y) % 2 == 0 {
                RED_PIXEL
            } else {
                BLUE_PIXEL
            };
            pixel != expected
        })
        .take(10)
        .collect();
    assert!(wrong.is_empty(), "pixels off the checkerboard: {wrong:?}");
    // One pixel outside each edge is background.
    let background = compositor
        .read_pixels(OUTPUT, Rectangle::new((3839, 2159).into(), (1, 1).into()))
        .expect("the background reads");
    for (x, y) in [(23, 24), (24, 23), (1908, 2135), (1907, 2136)] {
        let pixel = compositor
            .read_pixels(OUTPUT, Rectangle::new((x, y).into(), (1, 1).into()))
            .expect("the pixel reads");
        assert_eq!(pixel, background, "pixel ({x}, {y}) beside the window");
    }

    client.ack(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    assert_rests(&mut compositor);
}

/// Story 3: viewporter alone, at scale 1 (1920 × 1080 at 60 Hz): a buffer of 200 × 200 pixels
/// with a viewport destination of 100 × 100 is laid out as 100 × 100 and drawn as 100 × 100
/// pixels.
#[test]
fn a_viewport_destination_is_the_windows_size() {
    let mut compositor = compositor_with(vec![hd_output(1.0)]);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = client.open_window(&mut compositor);
    client.get_viewport(w);
    client.ack(w);
    client.attach(w, 200, 200, RED);
    client.set_viewport_destination(w, 100, 100);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary_at_60_hz(start, 1));

    let placed = compositor
        .layout()
        .arrange(OUTPUT)
        .expect("the output exists");
    let rect = placed[0].rect;
    assert_eq!((rect.w, rect.h), (100.0, 100.0));
    let (x, y) = (rect.x as i32, rect.y as i32);
    let mut pixel = |px: i32, py: i32| -> Vec<u8> {
        compositor
            .read_pixels(OUTPUT, Rectangle::new((px, py).into(), (1, 1).into()))
            .expect("the pixel reads")
    };
    let background = pixel(1919, 1079);
    for (px, py) in [(x, y), (x + 99, y), (x, y + 99), (x + 99, y + 99)] {
        assert_eq!(pixel(px, py), RED_PIXEL, "pixel ({px}, {py}) of the window");
    }
    for (px, py) in [
        (x + 100, y),
        (x, y + 100),
        (x + 100, y + 99),
        (x + 99, y + 100),
    ] {
        assert_eq!(
            pixel(px, py),
            background,
            "pixel ({px}, {py}) beside the window"
        );
    }

    client.ack(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary_at_60_hz(start, 2));
    assert_rests(&mut compositor);
}

/// Story 4: two outputs side by side, of scale 1 and 2, each 1920 × 1080 at 60 Hz. A window
/// opens on the first: its surface is told 120 (1 × 120), and no buffer scale, 1 being
/// `wl_surface`'s default. Moved to the second through the layout, it is told 240 and a buffer
/// scale of 2, once each.
#[test]
fn moving_to_an_output_of_another_scale_tells_the_new_scale_once() {
    let mut compositor = compositor_with(vec![hd_output(1.0), hd_output(2.0)]);
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let w = client.create_window();
    client.get_fractional_scale(w);
    client.commit(w);
    client.roundtrip(&mut compositor);
    let asked = client.window(w).configures[0].clone();
    client.ack(w);
    client.attach(w, asked.width, asked.height, RED);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary_at_60_hz(start, 1));
    let info = compositor.layout().window(WindowId(1)).expect("the window");
    assert_eq!(info.output, Some(OutputId(1)));
    assert_eq!(client.window(w).preferred_scales, vec![120]);
    assert!(client.window(w).preferred_buffer_scales.is_empty());

    compositor.change_layout(|layout| {
        layout
            .add_workspace(OWNER_SEAT, "two", Some(OutputId(2)))
            .expect("the workspace is added");
        layout
            .move_to_workspace(OWNER_SEAT, "two")
            .expect("the window moves");
    });
    advance_to(&mut compositor, &mut client, boundary_at_60_hz(start, 2));
    let info = compositor.layout().window(WindowId(1)).expect("the window");
    assert_eq!(info.output, Some(OutputId(2)));
    assert_eq!(client.window(w).preferred_scales, vec![120, 240]);
    assert_eq!(client.window(w).preferred_buffer_scales, vec![2]);

    // As a client at scale 2: it acknowledges its new size and draws at it.
    let asked = client
        .window(w)
        .configures
        .last()
        .expect("a configure")
        .clone();
    client.ack(w);
    client.attach(w, asked.width, asked.height, BLUE);
    client.commit(w);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary_at_60_hz(start, 3));
    assert_rests(&mut compositor);
    client.roundtrip(&mut compositor);
    assert_eq!(client.window(w).preferred_scales, vec![120, 240]);
    assert_eq!(client.window(w).preferred_buffer_scales, vec![2]);
}

/// A subsurface added to a window shown on the default output is told a preferred buffer scale
/// of 2 at its first commit, once; its window was told it before it opened.
#[test]
fn a_new_subsurface_is_told_its_windows_scale_at_its_first_commit() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let a = open_and_commit(&mut compositor, &mut client, RED);
    advance_to(&mut compositor, &mut client, boundary(start, 1));
    assert_eq!(client.window(a).preferred_buffer_scales, vec![2]);

    let sub = client.add_subsurface(a);
    client.roundtrip(&mut compositor);
    assert_eq!(client.subsurface(sub).preferred_buffer_scales, vec![2]);
    advance_to(&mut compositor, &mut client, boundary(start, 2));
    client.ack(a);
    client.commit(a);
    client.roundtrip(&mut compositor);
    advance_to(&mut compositor, &mut client, boundary(start, 3));
    assert_rests(&mut compositor);
    client.roundtrip(&mut compositor);
    assert_eq!(client.subsurface(sub).preferred_buffer_scales, vec![2]);
    assert_eq!(client.window(a).preferred_buffer_scales, vec![2]);
}

/// What a misbehaving client asks of its viewport.
#[derive(Debug, Clone, Copy)]
enum ViewportNonsense {
    EmptyDestination,
    NegativeDestination,
    SourceOutsideItsBuffer,
}

/// Story 6: a client that gives its viewport a destination of 0 × 0, a negative destination, or
/// a source outside its 100 × 100 buffer gets `wp_viewport`'s error (`bad_value`, `bad_value`,
/// `out_of_buffer`): it ends that client alone. Yuna goes on, and another client keeps its
/// frame callbacks, one at each boundary, its focus, and gets no configure.
#[test]
fn a_viewports_nonsense_ends_its_client_alone() {
    let mut compositor = manual_compositor();
    let mut staying = Client::connect(&mut compositor);
    let start = compositor.now();
    let a = open_and_commit(&mut compositor, &mut staying, RED);
    advance_to(&mut compositor, &mut staying, boundary(start, 1));
    let configures = staying.window(a).configures.len();
    let mut k = 1;
    let cases = [
        (
            ViewportNonsense::EmptyDestination,
            wp_viewport::Error::BadValue,
        ),
        (
            ViewportNonsense::NegativeDestination,
            wp_viewport::Error::BadValue,
        ),
        (
            ViewportNonsense::SourceOutsideItsBuffer,
            wp_viewport::Error::OutOfBuffer,
        ),
    ];
    for (nonsense, expected) in cases {
        let mut leaving = Client::connect(&mut compositor);
        let w = leaving.open_window(&mut compositor);
        leaving.get_viewport(w);
        match nonsense {
            ViewportNonsense::EmptyDestination => leaving.set_viewport_destination(w, 0, 0),
            ViewportNonsense::NegativeDestination => leaving.set_viewport_destination(w, -10, -10),
            ViewportNonsense::SourceOutsideItsBuffer => {
                leaving.ack(w);
                leaving.attach(w, 100, 100, BLUE);
                leaving.set_viewport_source(w, (50.0, 50.0), (100.0, 100.0));
                leaving.commit(w);
            }
        }
        let error = leaving.protocol_error(&mut compositor);
        assert_eq!(
            error,
            ProtocolError {
                interface: "wp_viewport".to_owned(),
                code: u32::from(expected),
            },
            "{nonsense:?}"
        );
        drop(leaving);

        staying.request_frame(a);
        staying.commit(a);
        staying.roundtrip(&mut compositor);
        let frames_before = staying.window(a).frames.len();
        k += 1;
        advance_to(&mut compositor, &mut staying, boundary(start, k));
        assert_eq!(
            staying.window(a).frames.len(),
            frames_before + 1,
            "{nonsense:?}"
        );
        let placed = compositor
            .layout()
            .arrange(OUTPUT)
            .expect("the output exists");
        assert_eq!(placed.len(), 1, "{nonsense:?}");
        assert_eq!(
            compositor.layout().focused(OWNER_SEAT),
            Some(WindowId(1)),
            "{nonsense:?}"
        );
        assert_eq!(
            staying.window(a).configures.len(),
            configures,
            "{nonsense:?}"
        );
    }

    staying.ack(a);
    staying.commit(a);
    staying.roundtrip(&mut compositor);
    k += 1;
    advance_to(&mut compositor, &mut staying, boundary(start, k));
    assert_rests(&mut compositor);
}
