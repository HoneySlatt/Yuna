//! Reference captures held beside the tests, for Stage 2a's parity
//! ([ADR-0139](../../../../docs/adr/0139-stage-2-in-three-gated-parts-2as-path-revised.md)): each
//! scene (`captures_held/scenes.rs`) is captured by a client of its seat through
//! `ext-image-copy-capture-v1`, and its pixels read from that client's own `wl_shm` buffer, which
//! Yuna fills from its readback (`render::read_pixels`, then `screencopy::copy_into_shm`). The
//! image is compared with a PNG held in `crates/yuna/tests/captures/`, class by class
//! (`captures_held/compare.rs`), so that a new scene or renderer must draw what today's GLES path
//! draws.
//!
//! A mismatch fails with one line per class: how many pixels changed, by how much on average and
//! at most, and how many are beyond the class's tolerance, the first named. So a change made on
//! purpose, a driver's new filter or a new way of blending, shows as the class it meant to move,
//! and a regression as one it did not. The image captured and a difference image go to a
//! temporary folder the failure names. A deliberate change rewrites the held images with
//! `YUNA_WRITE_CAPTURES=1`.

mod compare;
mod scenes;

use std::path::{Path, PathBuf};
use std::time::Instant;

use compare::{Differences, Image};
use scenes::Scene;
use yuna::OWNER_SEAT;

use crate::capture_client::{CaptureClient, Format};
use crate::run_armed_deadlines;

/// Captures `scene`'s output as a client of its seat does, into a `wl_shm` buffer of ARGB8888,
/// and reads that buffer.
fn capture(scene: &mut Scene) -> Image {
    let compositor = &mut scene.compositor;
    let mode = compositor.output_descriptions()[0].mode;
    let (width, height) = (mode.width(), mode.height());
    let mut capture = CaptureClient::connect(compositor, OWNER_SEAT);
    let session = capture.start_session("HEADLESS-1");
    let frame = capture.capture(session, Format::Argb8888, width, height);
    capture.roundtrip(compositor);
    run_armed_deadlines(compositor);
    capture.roundtrip(compositor);
    assert_eq!(capture.session(session).buffer_size, Some((width, height)));
    assert!(
        capture.frame(frame).ready,
        "the capture failed: {:?}",
        capture.frame(frame)
    );
    // ARGB8888 is B, G, R, A in memory.
    let mut rgba = capture.image(frame, width, height);
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Image {
        width,
        height,
        rgba,
    }
}

/// Builds a scene with `build`, captures it, and compares the capture with the image held as
/// `name`; or rewrites that image when `YUNA_WRITE_CAPTURES` is set.
fn assert_held(name: &str, build: impl FnOnce() -> Scene) {
    let started = Instant::now();
    let mut scene = build();
    let image = capture(&mut scene);
    let path = held_path(name);
    if std::env::var_os("YUNA_WRITE_CAPTURES").is_some() {
        image.write_png(&path);
        return;
    }
    let held = Image::read_png(&path).unwrap_or_else(|| {
        panic!(
            "no image is held for scene `{name}` at {}: write it with \
             `YUNA_WRITE_CAPTURES=1 cargo test -p yuna --test headless captures_held`",
            path.display()
        )
    });
    if (held.width, held.height) != (image.width, image.height) {
        let folder = write_failure(name, &image, None);
        panic!(
            "the capture of scene `{name}` is {} × {} pixels, its held image {} × {} ({}); \
             the capture is in {}",
            image.width,
            image.height,
            held.width,
            held.height,
            path.display(),
            folder.display()
        );
    }
    let differences = Differences::of(&held, &image, &scene.regions);
    if differences.hold() {
        // Each run's figures, seen with `--nocapture`: the spread of the renderer itself.
        print!(
            "{name}: {} ms\n{differences}",
            started.elapsed().as_millis()
        );
        return;
    }
    let difference = compare::difference_image(&held, &image, &scene.regions);
    let folder = write_failure(name, &image, Some(&difference));
    panic!(
        "the capture of scene `{name}` differs from its held image {} beyond a class's \
         tolerance:\n{differences}The image captured and the difference (magenta: beyond the \
         tolerance; yellow: within it) are in {}.\nIf the change is meant, rewrite the held \
         images with `YUNA_WRITE_CAPTURES=1 cargo test -p yuna --test headless captures_held`.",
        path.display(),
        folder.display()
    );
}

/// The held image of scene `name`.
fn held_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/captures")
        .join(format!("{name}.png"))
}

/// Writes scene `name`'s capture, and its difference image if any, to a temporary folder of
/// this run's own, and returns the folder.
fn write_failure(name: &str, actual: &Image, difference: Option<&Image>) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("yuna-captures-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("a folder for the failed captures");
    actual.write_png(&folder.join(format!("{name}.actual.png")));
    if let Some(difference) = difference {
        difference.write_png(&folder.join(format!("{name}.difference.png")));
    }
    folder
}

#[test]
fn opaque_columns_at_1() {
    assert_held("opaque_columns_at_1", || scenes::opaque_columns(1.0));
}

#[test]
fn opaque_columns_at_1_5() {
    assert_held("opaque_columns_at_1_5", || scenes::opaque_columns(1.5));
}

#[test]
fn a_translucent_window_over_another() {
    assert_held(
        "a_translucent_window_over_another",
        scenes::a_translucent_window_over_another,
    );
}

#[test]
fn buffers_resampled_at_1_5() {
    assert_held("buffers_resampled_at_1_5", scenes::buffers_resampled_at_1_5);
}

#[test]
fn subsurfaces_stacked_and_cut_to_their_window() {
    assert_held(
        "subsurfaces_stacked_and_cut_to_their_window",
        scenes::subsurfaces_stacked_and_cut_to_their_window,
    );
}

#[test]
fn a_popup_over_its_window() {
    assert_held("a_popup_over_its_window", scenes::a_popup_over_its_window);
}

#[test]
fn fullscreen_at_1() {
    assert_held("fullscreen_at_1", || scenes::fullscreen(1.0));
}

#[test]
fn fullscreen_at_1_5() {
    assert_held("fullscreen_at_1_5", || scenes::fullscreen(1.5));
}
