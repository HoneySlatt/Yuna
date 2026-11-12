//! `linux-dmabuf` on the software device, llvmpipe, as the tests and CI run Yuna.
//!
//! Whether the global exists depends on the Mesa the development shell gives: Yuna offers it only
//! when the renderer imports a format. Each story checks either branch, and prints which it took.
//! llvmpipe takes a plain memfd as a dmabuf's plane, so these stories draw a client's dmabuf
//! with no GPU; Yuna takes a plane only in a file that can never shrink.

use std::fs::File;
use std::io::ErrorKind;
use std::os::fd::AsFd;

use smithay::utils::{Physical, Rectangle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::{self, Flags};
use yuna::Compositor;
use yuna::yuna_layout::WindowId;

use crate::client::{Client, ProtocolError, memory_file, sealed_memory_file};
use crate::{
    OUTPUT, RED, advance_to, assert_rests, boundary, manual_compositor, millis, open_and_commit,
};

/// DRM_FORMAT_ARGB8888, `AR24`.
const ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
/// DRM_FORMAT_MOD_LINEAR.
const LINEAR: u64 = 0;
/// The buffers the misbehaving clients describe: 64 × 64 pixels of 4 bytes.
const SIDE: i32 = 64;
const STRIDE: u32 = 64 * 4;
const FILE_SIZE: u64 = 64 * 64 * 4;

/// What a misbehaving client does with its `zwp_linux_buffer_params_v1`.
#[derive(Debug, Clone, Copy)]
enum ParamsNonsense {
    NoPlane,
    PlanePastItsFilesEnd,
    UsedTwice,
}

/// Story 5: on the software device, the `linux-dmabuf` global exists exactly when the renderer
/// imports a format. If it does, the global is version 3 and offers those formats; a client whose
/// parameters have no plane, a plane whose offset lies past its file's end, or that uses them
/// twice gets the protocol's error (`incomplete`, `out_of_bounds`, `already_used`), which ends
/// it alone; another client keeps its frame callbacks and gets no configure. Without a format,
/// there is no global.
#[test]
fn dmabuf_on_the_software_device() {
    let mut compositor = manual_compositor();
    let offered = compositor.dmabuf_format_count();
    eprintln!("the software device imports {offered} dmabuf formats (fourcc and modifier pairs)");
    let mut staying = Client::connect(&mut compositor);
    let start = compositor.now();
    let a = open_and_commit(&mut compositor, &mut staying, RED);
    advance_to(&mut compositor, &mut staying, boundary(start, 1));
    let configures = staying.window(a).configures.len();
    let mut k = 1;

    if offered == 0 {
        eprintln!("no linux-dmabuf global: clients use wl_shm");
        assert_eq!(staying.dmabuf_version(), None);
        assert!(staying.state.dmabuf_formats.is_empty());
    } else {
        assert_eq!(staying.dmabuf_version(), Some(3));
        assert_eq!(staying.state.dmabuf_formats.len(), offered);
        let (format, modifier) = staying
            .state
            .dmabuf_formats
            .iter()
            .copied()
            .find(|&pair| pair == (ARGB8888, LINEAR))
            .unwrap_or(staying.state.dmabuf_formats[0]);
        eprintln!("the misbehaving clients use format {format:#x}, modifier {modifier:#x}");
        let cases = [
            (
                ParamsNonsense::NoPlane,
                zwp_linux_buffer_params_v1::Error::Incomplete,
            ),
            (
                ParamsNonsense::PlanePastItsFilesEnd,
                zwp_linux_buffer_params_v1::Error::OutOfBounds,
            ),
            (
                ParamsNonsense::UsedTwice,
                zwp_linux_buffer_params_v1::Error::AlreadyUsed,
            ),
        ];
        for (nonsense, expected) in cases {
            let mut leaving = Client::connect(&mut compositor);
            let params = leaving.create_dmabuf_params();
            let file = memory_file(FILE_SIZE);
            let (modifier_hi, modifier_lo) = ((modifier >> 32) as u32, modifier as u32);
            match nonsense {
                ParamsNonsense::NoPlane => {}
                ParamsNonsense::PlanePastItsFilesEnd => {
                    let past_the_end = u32::try_from(FILE_SIZE).expect("a small file") + 4096;
                    params.add(
                        file.as_fd(),
                        0,
                        past_the_end,
                        STRIDE,
                        modifier_hi,
                        modifier_lo,
                    );
                }
                ParamsNonsense::UsedTwice => {
                    params.add(file.as_fd(), 0, 0, STRIDE, modifier_hi, modifier_lo);
                    params.create(SIDE, SIDE, format, Flags::empty());
                }
            }
            params.create(SIDE, SIDE, format, Flags::empty());
            let error = leaving.protocol_error(&mut compositor);
            assert_eq!(
                error,
                ProtocolError {
                    interface: "zwp_linux_buffer_params_v1".to_owned(),
                    code: u32::from(expected),
                },
                "{nonsense:?}"
            );
            if let ParamsNonsense::UsedTwice = nonsense {
                // The first `create` was answered before the error: `failed`, since Yuna takes
                // no memfd its client did not seal against shrinking.
                let results = &leaving.state.dmabuf_results;
                assert_eq!(results.len(), 1);
                let answer = if results[0] { "created" } else { "failed" };
                eprintln!("the first create, of an unsealed memfd, came to {answer}");
            }
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
            assert_eq!(
                staying.window(a).configures.len(),
                configures,
                "{nonsense:?}"
            );
        }
    }

    staying.ack(a);
    staying.commit(a);
    staying.roundtrip(&mut compositor);
    k += 1;
    advance_to(&mut compositor, &mut staying, boundary(start, k));
    assert_rests(&mut compositor);
}

/// The buffer the stories of shrinking files share: 256 × 256 pixels of ARGB8888, linear.
const SHARED_SIDE: i32 = 256;
const SHARED_STRIDE: u32 = 256 * 4;
const SHARED_FILE_SIZE: u64 = 256 * 256 * 4;
const GREEN: u32 = 0xFF_00_FF_00;
/// `GREEN` as `read_pixels` gives it: R, G, B, A.
const GREEN_PIXEL: [u8; 4] = [0, 255, 0, 255];

/// Whether the compositor offers ARGB8888 in linear layout through `linux-dmabuf`, as llvmpipe
/// does whenever it imports dmabufs, at version 3. A renderer that imports no format gets no
/// global: this checks that branch, which the story then takes, and returns false.
fn offers_linear_argb8888(compositor: &Compositor, client: &Client) -> bool {
    let offered = compositor.dmabuf_format_count();
    eprintln!("the software device imports {offered} dmabuf formats (fourcc and modifier pairs)");
    if offered == 0 {
        eprintln!("no linux-dmabuf global: clients use wl_shm");
        assert_eq!(client.dmabuf_version(), None);
        assert!(client.state.dmabuf_formats.is_empty());
        return false;
    }
    assert_eq!(client.dmabuf_version(), Some(3));
    assert!(
        client.state.dmabuf_formats.contains(&(ARGB8888, LINEAR)),
        "llvmpipe imports ARGB8888 in linear layout"
    );
    true
}

/// Adds `file` to `params` as the one plane of a buffer of 256 × 256 pixels of ARGB8888, linear.
fn add_shared_plane(params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, file: &File) {
    let (modifier_hi, modifier_lo) = ((LINEAR >> 32) as u32, LINEAR as u32);
    params.add(file.as_fd(), 0, 0, SHARED_STRIDE, modifier_hi, modifier_lo);
}

/// A dmabuf whose plane lies in a file that may shrink, a memfd its client did not seal, is
/// refused. With `create_immed`, as a client that attaches the buffer, commits, then shrinks the
/// file to nothing before the next boundary: the client gets `invalid_wl_buffer`, which ends it
/// alone, its window never maps, and the drawing at that boundary reads nothing of the file
/// (were it imported, reading it would kill Yuna with SIGBUS). Another client gets its frame
/// callback at that exact boundary, and no configure. With `create`: the client is told
/// `failed`, and stays connected. Then Yuna rests.
#[test]
fn a_dmabuf_in_a_file_that_may_shrink_is_refused() {
    let mut compositor = manual_compositor();
    let mut staying = Client::connect(&mut compositor);
    let start = compositor.now();
    let a = open_and_commit(&mut compositor, &mut staying, RED);
    advance_to(&mut compositor, &mut staying, boundary(start, 1));
    let configures = staying.window(a).configures.len();
    let mut k = 1;

    if offers_linear_argb8888(&compositor, &staying) {
        let mut leaving = Client::connect(&mut compositor);
        let w = leaving.open_window(&mut compositor);
        leaving.ack(w);
        let file = memory_file(SHARED_FILE_SIZE);
        let params = leaving.create_dmabuf_params();
        add_shared_plane(&params, &file);
        let buffer =
            leaving.create_dmabuf_buffer_at_once(&params, (SHARED_SIDE, SHARED_SIDE), ARGB8888);
        leaving.attach_buffer(w, &buffer, SHARED_SIDE, SHARED_SIDE);
        leaving.commit(w);
        leaving.send();
        staying.request_frame(a);
        staying.commit(a);
        // Yuna handles what both clients sent.
        staying.roundtrip(&mut compositor);
        file.set_len(0).expect("an unsealed memfd shrinks");
        k += 1;
        advance_to(&mut compositor, &mut staying, boundary(start, k));
        assert_eq!(
            staying.window(a).frames.last(),
            Some(&millis(boundary(start, k)))
        );
        assert_eq!(staying.window(a).configures.len(), configures);
        let error = leaving.protocol_error(&mut compositor);
        assert_eq!(
            error,
            ProtocolError {
                interface: "zwp_linux_buffer_params_v1".to_owned(),
                code: u32::from(zwp_linux_buffer_params_v1::Error::InvalidWlBuffer),
            }
        );
        assert_eq!(compositor.layout().window(WindowId(2)), None);
        drop(leaving);

        let mut asking = Client::connect(&mut compositor);
        let file = memory_file(SHARED_FILE_SIZE);
        let params = asking.create_dmabuf_params();
        add_shared_plane(&params, &file);
        params.create(SHARED_SIDE, SHARED_SIDE, ARGB8888, Flags::empty());
        // A round trip survives: the client is not ended.
        asking.roundtrip(&mut compositor);
        assert_eq!(asking.state.dmabuf_results, vec![false]);
    }

    staying.ack(a);
    staying.commit(a);
    staying.roundtrip(&mut compositor);
    k += 1;
    advance_to(&mut compositor, &mut staying, boundary(start, k));
    assert_rests(&mut compositor);
}

/// A dmabuf whose plane lies in a memfd sealed against shrinking is taken: `create` gives
/// `created`, and the same file through `create_immed` opens a window of 256 × 256 logical units,
/// drawn green at its place at the next boundary, at scale 1.5: every one of its 384 × 384
/// pixels. The client then tries to shrink the file, which the kernel refuses (`EPERM`), and the
/// next boundary draws the window again. Then Yuna rests.
#[test]
fn a_dmabuf_in_a_file_sealed_against_shrinking_is_drawn() {
    let mut compositor = manual_compositor();
    let mut client = Client::connect(&mut compositor);
    let start = compositor.now();
    let mut k = 0;

    if offers_linear_argb8888(&compositor, &client) {
        let file = sealed_memory_file(&vec![GREEN; SHARED_FILE_SIZE as usize / 4]);
        let params = client.create_dmabuf_params();
        add_shared_plane(&params, &file);
        params.create(SHARED_SIDE, SHARED_SIDE, ARGB8888, Flags::empty());
        client.roundtrip(&mut compositor);
        assert_eq!(client.state.dmabuf_results, vec![true]);

        let w = client.open_window(&mut compositor);
        client.ack(w);
        let params = client.create_dmabuf_params();
        add_shared_plane(&params, &file);
        let buffer =
            client.create_dmabuf_buffer_at_once(&params, (SHARED_SIDE, SHARED_SIDE), ARGB8888);
        client.attach_buffer(w, &buffer, SHARED_SIDE, SHARED_SIDE);
        client.request_frame(w);
        client.commit(w);
        client.roundtrip(&mut compositor);
        let drawn = compositor.counters().frames_drawn;
        k += 1;
        advance_to(&mut compositor, &mut client, boundary(start, k));
        assert_eq!(compositor.counters().frames_drawn, drawn + 1);
        let placed = compositor
            .layout()
            .arrange(OUTPUT)
            .expect("the output exists");
        let rect = placed[0].rect;
        assert_eq!((rect.x, rect.y, rect.w, rect.h), (16.0, 16.0, 256.0, 256.0));
        // That rectangle at scale 1.5, in physical pixels.
        let window_pixels = Rectangle::new((24, 24).into(), (384, 384).into());
        assert_all_green(&mut compositor, window_pixels);

        let refused = file.set_len(0).expect_err("a sealed memfd never shrinks");
        assert_eq!(refused.kind(), ErrorKind::PermissionDenied);
        client.ack(w);
        client.attach_buffer(w, &buffer, SHARED_SIDE, SHARED_SIDE);
        client.request_frame(w);
        client.commit(w);
        client.roundtrip(&mut compositor);
        k += 1;
        advance_to(&mut compositor, &mut client, boundary(start, k));
        assert_eq!(compositor.counters().frames_drawn, drawn + 2);
        assert_eq!(
            client.window(w).frames.last(),
            Some(&millis(boundary(start, k)))
        );
        assert_all_green(&mut compositor, window_pixels);
    }

    assert_rests(&mut compositor);
}

/// Asserts that every pixel of `region` of the output is `GREEN`.
fn assert_all_green(compositor: &mut Compositor, region: Rectangle<i32, Physical>) {
    let pixels = compositor
        .read_pixels(OUTPUT, region)
        .expect("the region reads");
    let wrong = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|&&pixel| pixel != GREEN_PIXEL)
        .count();
    assert_eq!(wrong, 0, "pixels of {region:?} that are not green");
}
