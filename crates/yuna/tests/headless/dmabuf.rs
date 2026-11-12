//! `linux-dmabuf` on the software device, llvmpipe, as the tests and CI run Yuna.
//!
//! Whether the global exists depends on the Mesa the development shell gives: Yuna offers it only
//! when the renderer imports a format. The story checks either branch, and prints which it took.

use std::os::fd::AsFd;

use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::{self, Flags};

use crate::client::{Client, ProtocolError, memory_file};
use crate::{RED, advance_to, assert_rests, boundary, manual_compositor, open_and_commit};

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
                // The first `create` was answered before the error: a memfd is no dmabuf, but
                // llvmpipe may still import it.
                let results = &leaving.state.dmabuf_results;
                assert_eq!(results.len(), 1);
                eprintln!("llvmpipe imported a memfd as a dmabuf: {}", results[0]);
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
