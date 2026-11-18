//! `linux-dmabuf` on the software device, llvmpipe, as the tests and CI run Yuna.
//!
//! Whether the global exists depends on the Mesa the development shell gives: Yuna offers it only
//! when the renderer imports a format. Each story checks either branch, and prints which it took.
//! llvmpipe takes a plain memfd as a dmabuf's plane, so these stories draw a client's dmabuf
//! with no GPU; Yuna takes a plane only in a file that can never shrink, and that maps as
//! llvmpipe maps it.

use std::cell::RefCell;
use std::fs::File;
use std::io::ErrorKind;
use std::os::fd::AsFd;
use std::rc::Rc;
use std::time::Duration;

use smithay::reexports::rustix::fs::{SealFlags, fcntl_add_seals};
use smithay::utils::{Physical, Rectangle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::{self, Flags};
use yuna::Compositor;
use yuna::yuna_layout::WindowId;

use crate::client::{
    Client, ProtocolError, memory_file, memory_file_sealed_with, sealed_memory_file, write_only,
};
use crate::{
    LEAD, OUTPUT, RED, advance_to, assert_rests, boundary, manual_compositor, millis,
    open_and_commit,
};

/// DRM_FORMAT_ARGB8888, `AR24`.
const ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
/// DRM_FORMAT_MOD_LINEAR.
const LINEAR: u64 = 0;
/// The `dev_t` that names no device, which the software device's feedback names.
const NO_DEVICE: u64 = 0;
/// `zwp_linux_dmabuf_feedback_v1.tranche_flags.sampling`, from version 6.
const SAMPLING: u32 = 2;
/// The formats of one plane whose pixels Yuna knows the size of, which llvmpipe 26.2 imports:
/// red, red and green, and RGB. Its YUV formats, packed or of several planes, are not offered.
/// A Mesa that imports another fails the story that checks them, until it is checked and added.
const SINGLE_PLANE_FORMATS: [u32; 23] = [
    u32::from_le_bytes(*b"R8  "),
    u32::from_le_bytes(*b"R16 "),
    u32::from_le_bytes(*b"GR88"),
    u32::from_le_bytes(*b"GR32"),
    u32::from_le_bytes(*b"RG16"),
    u32::from_le_bytes(*b"AR12"),
    u32::from_le_bytes(*b"AB12"),
    u32::from_le_bytes(*b"AR15"),
    u32::from_le_bytes(*b"AB15"),
    u32::from_le_bytes(*b"AR24"),
    u32::from_le_bytes(*b"XR24"),
    u32::from_le_bytes(*b"AB24"),
    u32::from_le_bytes(*b"XB24"),
    u32::from_le_bytes(*b"RA24"),
    u32::from_le_bytes(*b"RX24"),
    u32::from_le_bytes(*b"BA24"),
    u32::from_le_bytes(*b"BX24"),
    u32::from_le_bytes(*b"AR30"),
    u32::from_le_bytes(*b"XR30"),
    u32::from_le_bytes(*b"AB30"),
    u32::from_le_bytes(*b"XB30"),
    u32::from_le_bytes(*b"AB4H"),
    u32::from_le_bytes(*b"XB4H"),
];

/// A fourcc code's four characters.
fn fourcc_name(code: u32) -> String {
    String::from_utf8_lossy(&code.to_le_bytes()).into_owned()
}
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
/// imports a format. If it does, the global is version 6 and offers those of one plane whose
/// pixels Yuna knows the size of: to a client bound at version 3, as `modifier` events; to one
/// at version 4, as default feedback whose main device is no device, `dev_t` 0, with one tranche
/// for that device holding every format, so that a client on a GPU takes Yuna to be on another
/// device than its own; to one at version 6, the same tranche, flagged for sampling, and no main
/// device, which version 6 no longer sends. A client whose parameters have no plane, a plane
/// whose offset lies past its file's end, or that uses them twice gets the protocol's error
/// (`incomplete`, `out_of_bounds`, `already_used`), which ends it alone; another client keeps
/// its frame callbacks and gets no configure. Without a format, there is no global.
#[test]
fn dmabuf_on_the_software_device() {
    let mut compositor = manual_compositor();
    let offered = compositor.dmabuf_format_count();
    eprintln!("the software device offers {offered} dmabuf formats (fourcc and modifier pairs)");
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
        assert_eq!(staying.dmabuf_version(), Some(6));
        assert_eq!(staying.state.dmabuf_formats.len(), offered);
        let mut offered_formats = staying.state.dmabuf_formats.clone();
        offered_formats.sort_unstable();
        let mut fourccs: Vec<u32> = offered_formats.iter().map(|&(code, _)| code).collect();
        fourccs.dedup();
        let names: Vec<String> = fourccs.iter().map(|code| fourcc_name(*code)).collect();
        eprintln!("the formats offered: {}", names.join(" "));
        for code in &fourccs {
            assert!(
                SINGLE_PLANE_FORMATS.contains(code),
                "{} is not a format of one plane whose pixels Yuna knows",
                fourcc_name(*code)
            );
        }
        // Version 4 has the main device and no flag; version 6 has no main device, and its
        // tranche the sampling flag.
        for (version, main_device, flags) in [(4, Some(NO_DEVICE), 0), (6, None, SAMPLING)] {
            let feedback = staying.default_dmabuf_feedback(&mut compositor, version);
            assert_eq!(feedback.main_device, main_device, "version {version}");
            assert_eq!(feedback.tranches.len(), 1, "version {version}");
            let tranche = &feedback.tranches[0];
            assert_eq!(tranche.target_device, Some(NO_DEVICE), "version {version}");
            assert_eq!(tranche.flags, flags, "version {version}");
            let mut in_the_tranche: Vec<(u32, u64)> = tranche
                .formats
                .iter()
                .map(|&index| feedback.table[usize::from(index)])
                .collect();
            in_the_tranche.sort_unstable();
            assert_eq!(in_the_tranche, offered_formats, "version {version}");
        }
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
/// does whenever it imports dmabufs, at version 6. A renderer that imports no format gets no
/// global: this checks that branch, which the story then takes, and returns false.
fn offers_linear_argb8888(compositor: &Compositor, client: &Client) -> bool {
    let offered = compositor.dmabuf_format_count();
    eprintln!("the software device offers {offered} dmabuf formats (fourcc and modifier pairs)");
    if offered == 0 {
        eprintln!("no linux-dmabuf global: clients use wl_shm");
        assert_eq!(client.dmabuf_version(), None);
        assert!(client.state.dmabuf_formats.is_empty());
        return false;
    }
    assert_eq!(client.dmabuf_version(), Some(6));
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
            Some(&millis(boundary(start, k) - LEAD))
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
        assert_eq!(compositor.counters().dmabuf_imports, 1);

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
            Some(&millis(boundary(start, k) - LEAD))
        );
        assert_all_green(&mut compositor, window_pixels);
    }

    assert_rests(&mut compositor);
}

/// How a client's dmabuf lies in a file llvmpipe cannot map for reading and writing.
#[derive(Debug, Clone, Copy)]
enum Unmappable {
    /// Its descriptor is open for writing alone, as a GPU's buffer in video memory the processor
    /// cannot reach refuses every mapping (`amdgpu` answers `EPERM`). Mesa's path for files it
    /// cannot map maps it again at each drawing, fails, and draws from address zero: SIGSEGV.
    WriteOnly,
    /// Its memfd is sealed against writing (`F_SEAL_WRITE`). Mesa's path for files it cannot map
    /// maps it for reading at each drawing, which works, but reads it with a stride of its own
    /// rather than the client's: with 1,024 bytes, as here, the same; with another, rows read
    /// askew, the last ones past the file.
    SealedAgainstWriting,
    /// Its memfd is sealed against future writes (`F_SEAL_FUTURE_WRITE`): as against writing, a
    /// seal a client may add at any time, even while the file is mapped.
    SealedAgainstFutureWriting,
}

/// A dmabuf whose plane lies in a file llvmpipe cannot map for reading and writing, as it maps
/// each plane it imports, is refused before the import, though its file can never shrink. Mesa
/// would take it all the same, through a path that ignores its stride and maps the file again at
/// each drawing (see [`Unmappable`]): the descriptor open for writing alone killed Yuna there
/// with SIGSEGV. Each is refused both ways, and Yuna asks its renderer to import none
/// ([`Refusals::assert_refused`]). Then Yuna rests.
#[test]
fn a_dmabuf_llvmpipe_cannot_map_is_refused() {
    let mut story = Refusals::new();
    if story.offers_linear_argb8888() {
        let pixels = vec![GREEN; SHARED_FILE_SIZE as usize / 4];
        let cases = [
            Unmappable::WriteOnly,
            Unmappable::SealedAgainstWriting,
            Unmappable::SealedAgainstFutureWriting,
        ];
        for case in cases {
            let file = match case {
                Unmappable::WriteOnly => write_only(&sealed_memory_file(&pixels)),
                Unmappable::SealedAgainstWriting => memory_file_sealed_with(
                    &pixels,
                    SealFlags::SHRINK | SealFlags::WRITE | SealFlags::SEAL,
                ),
                Unmappable::SealedAgainstFutureWriting => memory_file_sealed_with(
                    &pixels,
                    SealFlags::SHRINK | SealFlags::FUTURE_WRITE | SealFlags::SEAL,
                ),
            };
            let case = format!("{case:?}");
            story.assert_refused(&Plane::shared(&file), Refused::BeforeImport, &case);
        }
    }
    story.end();
}

/// A dmabuf whose stride is shorter than a row of its pixels is refused before the import:
/// 1,024 × 1,024 pixels of ARGB8888, rows of 4,096 bytes, with a stride of 4, in a sealed memfd
/// of 4,096 bytes. Smithay and Mesa check `offset + stride × height`, 4,096 bytes, against the
/// file, which holds them, but llvmpipe reads each row whole: the last ends 8,188 bytes in, past
/// the file's one page of mapping, where it reads whatever Yuna has there or dies of SIGSEGV. It
/// is refused both ways, and Yuna asks its renderer to import none
/// ([`Refusals::assert_refused`]). Then Yuna rests.
#[test]
fn a_dmabuf_whose_stride_is_shorter_than_a_row_is_refused() {
    let mut story = Refusals::new();
    if story.offers_linear_argb8888() {
        let file = sealed_memory_file(&[GREEN; 1024]);
        let plane = Plane {
            file: &file,
            stride: 4,
            width: 1024,
            height: 1024,
        };
        let case = "a stride of 4 for rows of 4,096 bytes";
        story.assert_refused(&plane, Refused::BeforeImport, case);
    }
    story.end();
}

/// A dmabuf whose client seals its file against future writes while the renderer imports it is
/// refused after the import: Mesa then took the file through its path for files it cannot map,
/// which ignores the client's stride. The story's hook seals each file (`F_SEAL_FUTURE_WRITE`)
/// just before the import, after Yuna's first check took it. Each way, with a file of its own,
/// the buffer is imported, then refused ([`Refusals::assert_refused_at_once`],
/// [`Refusals::assert_refused_when_asked`]). Then Yuna rests.
#[test]
fn a_dmabuf_sealed_while_it_is_imported_is_refused_after_the_import() {
    let mut story = Refusals::new();
    if story.offers_linear_argb8888() {
        let sealed_at_import: Rc<RefCell<Vec<File>>> = Rc::default();
        let to_seal = Rc::clone(&sealed_at_import);
        story.compositor.set_dmabuf_import_hook(move || {
            for file in to_seal.borrow().iter() {
                fcntl_add_seals(file, SealFlags::FUTURE_WRITE).expect("the memfd takes a seal");
            }
        });
        let pixels = vec![GREEN; SHARED_FILE_SIZE as usize / 4];
        let case = "sealed against future writes during the import";

        let file = memory_file_sealed_with(&pixels, SealFlags::SHRINK);
        sealed_at_import
            .borrow_mut()
            .push(file.try_clone().expect("the memfd's descriptor copies"));
        story.assert_refused_at_once(&Plane::shared(&file), Refused::AfterImport, case);

        let file = memory_file_sealed_with(&pixels, SealFlags::SHRINK);
        sealed_at_import
            .borrow_mut()
            .push(file.try_clone().expect("the memfd's descriptor copies"));
        story.assert_refused_when_asked(&Plane::shared(&file), Refused::AfterImport, case);
    }
    story.end();
}

/// The one plane of a buffer of ARGB8888 in linear layout, at offset 0 in its file, as a story's
/// client describes it.
struct Plane<'a> {
    file: &'a File,
    stride: u32,
    width: i32,
    height: i32,
}

impl Plane<'_> {
    /// The plane of the buffer the stories share: 256 × 256 pixels, rows of 1,024 bytes.
    fn shared(file: &File) -> Plane<'_> {
        Plane {
            file,
            stride: SHARED_STRIDE,
            width: SHARED_SIDE,
            height: SHARED_SIDE,
        }
    }

    /// Adds the plane to `params`.
    fn add_to(&self, params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1) {
        let (modifier_hi, modifier_lo) = ((LINEAR >> 32) as u32, LINEAR as u32);
        params.add(
            self.file.as_fd(),
            0,
            0,
            self.stride,
            modifier_hi,
            modifier_lo,
        );
    }
}

/// When Yuna refuses a buffer.
#[derive(Debug, Clone, Copy)]
enum Refused {
    /// Before its renderer imports it: Mesa never sees it.
    BeforeImport,
    /// Once its renderer imported it, by the check after the import.
    AfterImport,
}

impl Refused {
    /// The imports Yuna attempts for one buffer refused so.
    fn imports(self) -> u64 {
        match self {
            Refused::BeforeImport => 0,
            Refused::AfterImport => 1,
        }
    }
}

/// The stories of dmabufs Yuna refuses: a compositor, and a client whose window stays, drawn at
/// each boundary.
struct Refusals {
    compositor: Compositor,
    staying: Client,
    window: usize,
    start: Duration,
    boundary: u64,
    configures: usize,
}

impl Refusals {
    /// The compositor, and the staying client's window, drawn at the first boundary.
    fn new() -> Refusals {
        let mut compositor = manual_compositor();
        let mut staying = Client::connect(&mut compositor);
        let start = compositor.now();
        let window = open_and_commit(&mut compositor, &mut staying, RED);
        advance_to(&mut compositor, &mut staying, boundary(start, 1));
        let configures = staying.window(window).configures.len();
        Refusals {
            compositor,
            staying,
            window,
            start,
            boundary: 1,
            configures,
        }
    }

    /// Whether the compositor offers ARGB8888 in linear layout, as llvmpipe does whenever it
    /// imports dmabufs; without, the story checks there is no global and refuses nothing.
    fn offers_linear_argb8888(&self) -> bool {
        offers_linear_argb8888(&self.compositor, &self.staying)
    }

    /// How many dmabufs Yuna handed its renderer to import so far.
    fn imports(&self) -> u64 {
        self.compositor.counters().dmabuf_imports
    }

    /// Asserts that Yuna refuses a buffer of `plane`, named `case` in failures, both ways
    /// ([`Refusals::assert_refused_at_once`], [`Refusals::assert_refused_when_asked`]), at `when`.
    fn assert_refused(&mut self, plane: &Plane<'_>, when: Refused, case: &str) {
        self.assert_refused_at_once(plane, when, case);
        self.assert_refused_when_asked(plane, when, case);
    }

    /// Asserts that Yuna refuses a buffer of `plane` made with `create_immed`, at `when`, as a
    /// client that attaches the buffer and commits: the client gets `invalid_wl_buffer`, which
    /// ends it alone, its window never maps, and the next boundary draws; the staying client
    /// gets its frame callback at that exact boundary, and no configure.
    fn assert_refused_at_once(&mut self, plane: &Plane<'_>, when: Refused, case: &str) {
        let imports = self.imports();
        let (compositor, staying, a) = (&mut self.compositor, &mut self.staying, self.window);
        let mut leaving = Client::connect(compositor);
        let w = leaving.open_window(compositor);
        leaving.ack(w);
        let params = leaving.create_dmabuf_params();
        plane.add_to(&params);
        let buffer =
            leaving.create_dmabuf_buffer_at_once(&params, (plane.width, plane.height), ARGB8888);
        leaving.attach_buffer(w, &buffer, plane.width, plane.height);
        leaving.commit(w);
        leaving.send();
        staying.request_frame(a);
        staying.commit(a);
        // Yuna handles what both clients sent.
        staying.roundtrip(compositor);
        self.boundary += 1;
        let at = boundary(self.start, self.boundary);
        advance_to(compositor, staying, at);
        assert_eq!(
            staying.window(a).frames.last(),
            Some(&millis(at - LEAD)),
            "{case}"
        );
        assert_eq!(
            staying.window(a).configures.len(),
            self.configures,
            "{case}"
        );
        let error = leaving.protocol_error(compositor);
        assert_eq!(
            error,
            ProtocolError {
                interface: "zwp_linux_buffer_params_v1".to_owned(),
                code: u32::from(zwp_linux_buffer_params_v1::Error::InvalidWlBuffer),
            },
            "{case}"
        );
        // The only window besides the staying one, had any leaving client's mapped.
        assert_eq!(compositor.layout().window(WindowId(2)), None, "{case}");
        drop(leaving);
        assert_eq!(self.imports(), imports + when.imports(), "{case}, at once");
    }

    /// Asserts that Yuna refuses a buffer of `plane` made with `create`, at `when`: the client
    /// is told `failed`, and stays connected.
    fn assert_refused_when_asked(&mut self, plane: &Plane<'_>, when: Refused, case: &str) {
        let imports = self.imports();
        let compositor = &mut self.compositor;
        let mut asking = Client::connect(compositor);
        let params = asking.create_dmabuf_params();
        plane.add_to(&params);
        params.create(plane.width, plane.height, ARGB8888, Flags::empty());
        // A round trip survives: the client is not ended.
        asking.roundtrip(compositor);
        assert_eq!(asking.state.dmabuf_results, vec![false], "{case}");
        assert_eq!(
            self.imports(),
            imports + when.imports(),
            "{case}, when asked"
        );
    }

    /// The story's end: the staying window acknowledged, committed and drawn, then Yuna rests.
    fn end(mut self) {
        let (compositor, staying, a) = (&mut self.compositor, &mut self.staying, self.window);
        staying.ack(a);
        staying.commit(a);
        staying.roundtrip(compositor);
        advance_to(compositor, staying, boundary(self.start, self.boundary + 1));
        assert_rests(compositor);
    }
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
