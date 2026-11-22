//! `linux-dmabuf`: clients' buffers in GPU memory, imported where the renderer can import them.
//! Mesa's EGL and Vulkan on the processor, llvmpipe and lavapipe, fall back on `wl_shm` when the
//! compositor names no GPU: on llvmpipe, Mesa's clients on the processor never send a dmabuf.
//!
//! The global offers the formats the renderer imports (`ImportDma::dmabuf_formats`), each a
//! fourcc code and a modifier, with default feedback, which Smithay offers as version 6: one
//! tranche of the formats, for the feedback's main device. With `--gpu`, that device is the
//! render node. On the software device, llvmpipe, it is no device, `dev_t` 0: llvmpipe reads a
//! buffer with the processor, through a mapping of its file, and no GPU's video memory is its
//! own ([ADR-0123](../../../docs/adr/0123-the-headless-backend-virtual-outputs-llvmpipe-and-dmabufs.md)).
//! A client on a GPU then finds another device than its own, and shares buffers outside its
//! video memory: Mesa's Vulkan copies each frame into a linear buffer there, which llvmpipe maps.
//! That holds for clients that compare the device's number with their GPU's, as Mesa does, which
//! the protocol tells clients not to rely on; any other may still share a buffer the processor
//! cannot map, which Yuna refuses. Without feedback, Mesa's Vulkan takes the compositor to be on
//! its own GPU, and shares buffers in video memory the processor cannot map. On llvmpipe, only
//! formats of one plane whose pixels Yuna knows the size of are offered; on any device, at most
//! [`MAX_OFFERED_FORMATS`], so that the tranche's event fits 4,096 bytes. If the feedback cannot
//! be made, the global is version 3, the formats alone. A renderer that imports no format gets
//! no global, and Mesa's clients use `wl_shm`.
//!
//! Each buffer a client creates is imported at once, so that the client learns at `create`
//! whether it can be used (`created` or `failed`; with `create_immed`, a failure is the
//! protocol's error, which ends the client). Parameters that break the protocol (no plane, a
//! plane past its file's end, parameters used twice) are refused by Smithay with a protocol
//! error, which ends that client only. A plane must lie in a file that can never shrink, a real
//! dma-buf or a file sealed against shrinking: any other fails, since a file shrunk under the
//! renderer would kill Yuna at drawing. On llvmpipe, a buffer must also lie in its file row by
//! row, and each plane's file map as llvmpipe maps it, checked before the import and the mapping
//! again after it: Mesa takes a file it cannot map so through a path that reads address zero at
//! drawing. A buffer that fails later, at drawing, is left out of that frame by Smithay's
//! renderer, which logs it.

#![expect(
    unsafe_code,
    reason = "a client's dmabuf mapped and unmapped at once, to check that it maps"
)]

use std::os::fd::BorrowedFd;
use std::os::unix::fs::MetadataExt;
use std::ptr;

use drm_fourcc::{DrmFormat, DrmFourcc};
use rustix::fs::{FsWord, SealFlags, SeekFrom, fcntl_get_seals, fstatfs, seek};
use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};
use smithay::backend::allocator::Buffer;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::format::get_bpp;
use smithay::backend::renderer::ImportDma;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::wayland::dmabuf::{
    DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier,
};
use wayland_server::DisplayHandle;

use crate::render::Device;
use crate::state::State;

/// What the `linux-dmabuf` global offers, and how the renderer reads the buffers it takes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Offer {
    /// How many formats clients are offered; 0 when there is no global.
    pub formats: usize,
    /// Whether the renderer reads each plane through a mapping of its file, as llvmpipe does: a
    /// buffer is then offered and taken only in a format of one plane whose pixels Yuna knows the
    /// size of, with rows that lie in its file, and a plane only if its file maps as llvmpipe maps
    /// it.
    pub planes_mapped: bool,
}

/// The device a renderer's `linux-dmabuf` feedback names as its main device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainDevice {
    /// No device, `dev_t` 0 ([`NO_DEVICE`]): llvmpipe reads buffers with the processor.
    NoDevice,
    /// The GPU's render node.
    RenderNode,
}

/// The `dev_t` that names no device: Linux gives none the number 0, and DRM's nodes have major
/// 226. A client looking for a render node by it finds none, and one comparing it with its own
/// GPU finds another device.
const NO_DEVICE: u64 = 0;

/// The most formats, each a fourcc code and a modifier, the global offers: the feedback's tranche
/// lists them in one `tranche_formats` event, 2 bytes an index after an 8-byte header and the
/// array's 4-byte length, and a client of wayland-rs reads no event past 4,096 bytes, whatever
/// the server's buffer (ADR-0136). A GPU imports a few hundred; llvmpipe 23.
const MAX_OFFERED_FORMATS: usize = (4_096 - 8 - 4) / 2;
const _: () = assert!(8 + 4 + 2 * MAX_OFFERED_FORMATS <= 4_096);

/// The main device the feedback of a renderer on `device` that imports `format_count` formats
/// names; none, and no global, without a format.
fn main_device(device: Device, format_count: usize) -> Option<MainDevice> {
    if format_count == 0 {
        return None;
    }
    Some(match device {
        Device::Software => MainDevice::NoDevice,
        Device::Gpu => MainDevice::RenderNode,
    })
}

/// Creates the `linux-dmabuf` global for `renderer`, on `device`, and returns what it offers.
pub(crate) fn create_global(
    display: &DisplayHandle,
    dmabuf_state: &mut DmabufState,
    renderer: &GlesRenderer,
    device: Device,
) -> Offer {
    let planes_mapped = device == Device::Software;
    let imported = renderer.dmabuf_formats();
    let formats = formats_to_offer(imported.iter().copied(), planes_mapped);
    let count = formats.len();
    let left_out = imported.indexset().len() - count;
    let Some(main_device) = main_device(device, count) else {
        tracing::info!(
            "the renderer imports no dmabuf Yuna can offer: no linux-dmabuf global, clients use wl_shm"
        );
        return Offer {
            formats: 0,
            planes_mapped,
        };
    };
    match default_feedback(renderer, main_device, formats.clone()) {
        Ok(feedback) => {
            dmabuf_state.create_global_with_default_feedback::<State>(display, &feedback);
        }
        Err(reason) => {
            tracing::warn!(
                %reason,
                "no dmabuf feedback; linux-dmabuf is offered without it"
            );
            dmabuf_state.create_global::<State>(display, formats);
        }
    }
    tracing::info!(
        formats = count,
        left_out,
        ?main_device,
        "linux-dmabuf offered"
    );
    Offer {
        formats: count,
        planes_mapped,
    }
}

/// The formats of `imported` the global offers: on a device whose planes the processor maps, only
/// those whose pixels Yuna knows the size of; [`MAX_OFFERED_FORMATS`] at most, in the renderer's
/// order, the rest left out with a warning.
fn formats_to_offer(
    imported: impl Iterator<Item = DrmFormat>,
    planes_mapped: bool,
) -> Vec<DrmFormat> {
    let mut formats: Vec<DrmFormat> = imported
        .filter(|format| !planes_mapped || bytes_per_pixel(format.code).is_some())
        .collect();
    if formats.len() > MAX_OFFERED_FORMATS {
        tracing::warn!(
            imported = formats.len(),
            offered = MAX_OFFERED_FORMATS,
            "the renderer imports more dmabuf formats than one event lists; the last are left out"
        );
        formats.truncate(MAX_OFFERED_FORMATS);
    }
    formats
}

/// The feedback a renderer gives: `main_device`, and one tranche of the formats offered.
fn default_feedback(
    renderer: &GlesRenderer,
    main_device: MainDevice,
    formats: Vec<DrmFormat>,
) -> Result<DmabufFeedback, String> {
    let device_number = match main_device {
        MainDevice::NoDevice => NO_DEVICE,
        MainDevice::RenderNode => render_node_number(renderer)?,
    };
    DmabufFeedbackBuilder::new(device_number, formats)
        .build()
        .map_err(|error| format!("cannot write the format table: {error}"))
}

/// The device number of the render node `renderer` draws on, read without opening it.
fn render_node_number(renderer: &GlesRenderer) -> Result<u64, String> {
    let node = crate::render::render_node(renderer).map_err(|error| error.to_string())?;
    std::fs::metadata(&node)
        .map(|metadata| metadata.rdev())
        .map_err(|error| format!("cannot read {}: {error}", node.display()))
}

/// `DMA_BUF_MAGIC` in `linux/magic.h`: the file system type `fstatfs` gives a real dma-buf.
const DMA_BUF_MAGIC: FsWord = 0x444d_4142;

/// `TMPFS_MAGIC` in `linux/magic.h`: the file system type of a memfd, unless it was made with
/// `MFD_HUGETLB`.
const TMPFS_MAGIC: FsWord = 0x0102_1994;

/// Whether every plane of `dmabuf` lies in a file that the renderer may always read: one that
/// never shrinks and, when the renderer maps planes (`planes_mapped`), a buffer whose rows lie
/// in its file, in a file that maps as llvmpipe maps it. A renderer may map a plane's file and
/// read it each time it draws; Smithay checks the plane against the file's size at `create`
/// only, and catches SIGBUS for `wl_shm` alone. A file shrunk since would kill Yuna with SIGBUS
/// at drawing (ADR-0080). llvmpipe takes a plain memfd as a plane.
fn planes_are_safe_to_read(dmabuf: &Dmabuf, planes_mapped: bool) -> bool {
    dmabuf.handles().all(file_is_safe_to_read)
        && (!planes_mapped
            || (rows_lie_in_the_file(dmabuf) && dmabuf.handles().all(maps_as_llvmpipe_maps_it)))
}

/// The bytes of one pixel of `code`, for a format of one plane whose pixels Smithay knows the
/// size of; none for any other. Smithay's table also gives YUV420 and NV12, of several planes,
/// their mean bits per pixel: they are left out, as is any whose bits make no whole byte. Of the
/// 57 formats Mesa 26.2's llvmpipe imports, this keeps its 23 of red, red and green, and RGB
/// (`R8`, `AR24`, `AB4H`…); its YUV formats, packed or of several planes, are left out.
fn bytes_per_pixel(code: DrmFourcc) -> Option<u64> {
    if matches!(code, DrmFourcc::Yuv420 | DrmFourcc::Nv12) {
        return None;
    }
    let bits = get_bpp(code)?;
    (bits % 8 == 0).then_some(bits as u64 / 8)
}

/// Whether llvmpipe reads every pixel of `dmabuf` within its file: one plane, in a format whose
/// bytes per pixel Yuna knows, whose stride holds a row of its pixels and whose last row ends
/// within the file. Smithay checks `offset + stride × height` against the file, and Mesa 26.2 the
/// same (`dri_sw_displaytarget_create_mapped`), never the stride against a row: a shorter stride
/// makes llvmpipe read each row whole past the plane's end, past the file's mapping, where it
/// shows whatever Yuna has there or kills it with SIGSEGV.
fn rows_lie_in_the_file(dmabuf: &Dmabuf) -> bool {
    let Some(bytes_per_pixel) = bytes_per_pixel(dmabuf.format().code) else {
        return false;
    };
    if dmabuf.num_planes() != 1 {
        return false;
    }
    let (Some(fd), Some(offset), Some(stride)) = (
        dmabuf.handles().next(),
        dmabuf.offsets().next(),
        dmabuf.strides().next(),
    ) else {
        return false;
    };
    let Some(file_size) = file_size(fd) else {
        return false;
    };
    let size = dmabuf.size();
    let layout = PlaneLayout {
        width: size.w,
        height: size.h,
        bytes_per_pixel,
        offset,
        stride,
    };
    layout.rows_fit_in(file_size)
}

/// Where a plane's pixels lie in its file.
#[derive(Debug, Clone, Copy)]
struct PlaneLayout {
    width: i32,
    height: i32,
    bytes_per_pixel: u64,
    offset: u32,
    stride: u32,
}

impl PlaneLayout {
    /// Whether every row lies whole in a file of `file_size` bytes: the stride holds a row, and
    /// the last row ends within the file. A plane with no pixel fits nothing.
    fn rows_fit_in(self, file_size: u64) -> bool {
        let (Ok(width), Ok(height)) = (u64::try_from(self.width), u64::try_from(self.height))
        else {
            return false;
        };
        if width == 0 || height == 0 {
            return false;
        }
        let stride = u64::from(self.stride);
        let end = width
            .checked_mul(self.bytes_per_pixel)
            .filter(|&row| stride >= row)
            .and_then(|row| {
                stride
                    .checked_mul(height - 1)?
                    .checked_add(u64::from(self.offset))?
                    .checked_add(row)
            });
        end.is_some_and(|end| end <= file_size)
    }
}

/// The size of the file `fd` as Smithay and Mesa read it, the end `lseek` finds, the offset then
/// put back at the start as they leave it.
fn file_size(fd: BorrowedFd<'_>) -> Option<u64> {
    let end = seek(fd, SeekFrom::End(0)).ok()?;
    let _ = seek(fd, SeekFrom::Start(0));
    Some(end)
}

/// Whether the file `fd` can be read in full for as long as it lives: a real dma-buf, whose
/// size is fixed; or a tmpfs file, a memfd, sealed against shrinking (`F_SEAL_SHRINK`, which no
/// one can take off). A tmpfs file reads a hole punched in it as zeros; a hugetlbfs memfd may
/// raise SIGBUS there when its pool of huge pages is empty, so it is refused, as is a memfd its
/// client did not seal, or any other file.
fn file_is_safe_to_read(fd: BorrowedFd<'_>) -> bool {
    let Ok(file_system) = fstatfs(fd).map(|stat| stat.f_type) else {
        return false;
    };
    file_system == DMA_BUF_MAGIC
        || (file_system == TMPFS_MAGIC
            && fcntl_get_seals(fd).is_ok_and(|seals| seals.contains(SealFlags::SHRINK)))
}

/// Whether the file `fd` maps as llvmpipe maps each plane it imports: whole, from its start to
/// the end `lseek` finds, for reading and writing, shared (`llvmpipe_dmabuf_import`, Mesa 26.2).
/// Mesa does not refuse a plane it cannot map so: it takes it through another path, which
/// ignores the plane's stride and maps the file again at each drawing, and a mapping that fails
/// there gives the drawing address zero, which kills Yuna with SIGSEGV. A GPU's buffer in video
/// memory the processor cannot reach fails so (`amdgpu` answers `EPERM`), as do a descriptor
/// open for writing alone and a memfd sealed against writing. The mapping made here is never
/// read nor written, and is undone at once.
fn maps_as_llvmpipe_maps_it(fd: BorrowedFd<'_>) -> bool {
    let Some(Ok(length)) = file_size(fd).map(usize::try_from) else {
        return false;
    };
    if length == 0 {
        return false;
    }
    // SAFETY: a new shared mapping at an address the kernel chooses (no `MAP_FIXED`), so it
    // replaces no mapping of this process. Nothing reads or writes through it, so a client
    // changing or shrinking its file meanwhile cannot fault here; it is undone just below.
    let mapped = unsafe {
        mmap(
            ptr::null_mut(),
            length,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::SHARED,
            fd,
            0,
        )
    };
    let Ok(address) = mapped else {
        return false;
    };
    // SAFETY: `address` and `length` are the mapping made just above, which nothing else knows
    // of or uses.
    if let Err(error) = unsafe { munmap(address, length) } {
        tracing::warn!(%error, "a test mapping of a client's dmabuf could not be undone");
    }
    true
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        let planes_mapped = self.dmabuf_offer.planes_mapped;
        if !planes_are_safe_to_read(&dmabuf, planes_mapped) {
            tracing::debug!("a client's dmabuf lies in a file the renderer cannot always read");
            notifier.failed();
            return;
        }
        self.counters.dmabuf_imports += 1;
        if let Some(hook) = &mut self.dmabuf_import_hook {
            hook();
        }
        match self.renderer.import_dmabuf(&dmabuf, None) {
            // Its client may have sealed a plane's file against writing while Mesa imported it,
            // which sends Mesa down the path that reads address zero at drawing. A seal is never
            // taken off, nor a descriptor's mode changed, and the file never shrinks: whatever
            // its client does, a file that maps now mapped when Mesa mapped it.
            Ok(_) if planes_mapped && !dmabuf.handles().all(maps_as_llvmpipe_maps_it) => {
                tracing::debug!("a client's dmabuf stopped mapping while it was imported");
                // Its texture goes with the dmabuf, dropped at the end of this call.
                self.surfaces_gone = true;
                notifier.failed();
            }
            Ok(_) => {
                if notifier.successful::<State>().is_err() {
                    tracing::debug!("a client left before its dmabuf's buffer was created");
                    // Its texture goes with the dmabuf, dropped at the end of this call.
                    self.surfaces_gone = true;
                }
            }
            Err(error) => {
                tracing::debug!(%error, "a client's dmabuf could not be imported");
                notifier.failed();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{File, OpenOptions};
    use std::os::fd::{AsFd, AsRawFd};

    use drm_fourcc::DrmModifier;
    use rustix::fs::{MemfdFlags, fcntl_add_seals, memfd_create};
    use rustix::io::Errno;

    use super::*;

    /// A tranche's formats travel as one event, which a client of wayland-rs cannot read past
    /// 4,096 bytes whatever the server's buffer (ADR-0136).
    #[test]
    fn the_formats_offered_fit_one_event_however_many_the_renderer_imports() {
        let imported: Vec<DrmFormat> = (0..3_000u64)
            .map(|modifier| DrmFormat {
                code: DrmFourcc::Argb8888,
                modifier: DrmModifier::from(modifier),
            })
            .collect();
        let offered = formats_to_offer(imported.iter().copied(), false);
        assert_eq!(offered.len(), MAX_OFFERED_FORMATS);
        assert_eq!(offered[..], imported[..MAX_OFFERED_FORMATS]);
        let few = formats_to_offer(imported[..23].iter().copied(), false);
        assert_eq!(few.len(), 23);
    }

    #[test]
    fn feedback_names_no_device_on_the_software_device_the_render_node_on_a_gpu() {
        assert_eq!(
            main_device(Device::Software, 12),
            Some(MainDevice::NoDevice)
        );
        assert_eq!(main_device(Device::Software, 0), None);
        assert_eq!(main_device(Device::Gpu, 12), Some(MainDevice::RenderNode));
        assert_eq!(main_device(Device::Gpu, 0), None);
    }

    /// The bytes of a pixel are known for formats of one plane alone: never for YUV, packed or
    /// of several planes, though Smithay gives NV12 and YUV420 a mean of bits per pixel.
    #[test]
    fn bytes_per_pixel_of_formats_of_one_plane_alone() {
        assert_eq!(bytes_per_pixel(DrmFourcc::R8), Some(1));
        assert_eq!(bytes_per_pixel(DrmFourcc::Rgb565), Some(2));
        assert_eq!(bytes_per_pixel(DrmFourcc::Argb8888), Some(4));
        assert_eq!(bytes_per_pixel(DrmFourcc::Abgr16161616f), Some(8));
        for code in [
            DrmFourcc::Nv12,
            DrmFourcc::Yuv420,
            DrmFourcc::Nv16,
            DrmFourcc::P010,
            DrmFourcc::Yuyv,
            DrmFourcc::Ayuv,
        ] {
            assert_eq!(bytes_per_pixel(code), None, "{code:?}");
        }
    }

    /// A plane's rows fit when its stride holds a row and its last row, which needs only its
    /// own bytes, ends within the file; a plane with no pixel never fits, and no size overflows.
    #[test]
    fn rows_fit_with_a_stride_that_holds_a_row_and_the_last_row_in_the_file() {
        let plane = PlaneLayout {
            width: 1024,
            height: 1024,
            bytes_per_pixel: 4,
            offset: 0,
            stride: 4096,
        };
        assert!(plane.rows_fit_in(4096 * 1024));
        assert!(!plane.rows_fit_in(4096 * 1024 - 1));

        let short = PlaneLayout { stride: 4, ..plane };
        assert!(!short.rows_fit_in(u64::MAX));

        let padded = PlaneLayout {
            stride: 4096 + 64,
            ..plane
        };
        assert!(padded.rows_fit_in(4160 * 1023 + 4096));
        assert!(!padded.rows_fit_in(4160 * 1023 + 4095));

        let after_a_header = PlaneLayout {
            offset: 100,
            ..plane
        };
        assert!(after_a_header.rows_fit_in(100 + 4096 * 1024));
        assert!(!after_a_header.rows_fit_in(100 + 4096 * 1024 - 1));

        for (width, height) in [(0, 1), (1, 0), (-1, 1), (1, -1)] {
            let empty = PlaneLayout {
                width,
                height,
                ..plane
            };
            assert!(!empty.rows_fit_in(u64::MAX), "{width} × {height}");
        }

        let tallest = PlaneLayout {
            width: 1,
            height: i32::MAX,
            bytes_per_pixel: 8,
            offset: u32::MAX,
            stride: u32::MAX,
        };
        let end = u64::from(u32::MAX) * (i32::MAX as u64 - 1) + u64::from(u32::MAX) + 8;
        assert!(tallest.rows_fit_in(end));
        assert!(!tallest.rows_fit_in(end - 1));
        let widest = PlaneLayout {
            width: i32::MAX,
            ..tallest
        };
        assert!(!widest.rows_fit_in(u64::MAX));
    }

    /// A memfd of 4096 bytes, made with `flags`.
    fn memfd(flags: MemfdFlags) -> File {
        let file = File::from(memfd_create("yuna-test", flags).expect("a memfd"));
        file.set_len(4096).expect("the memfd takes its size");
        file
    }

    #[test]
    fn only_a_memfd_sealed_against_shrinking_is_taken() {
        let plain = memfd(MemfdFlags::CLOEXEC);
        assert!(!file_is_safe_to_read(plain.as_fd()));
        let sealable = memfd(MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING);
        assert!(!file_is_safe_to_read(sealable.as_fd()));
        fcntl_add_seals(&sealable, SealFlags::GROW | SealFlags::SEAL).expect("sealed");
        assert!(!file_is_safe_to_read(sealable.as_fd()));
        let sealed = memfd(MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING);
        fcntl_add_seals(&sealed, SealFlags::SHRINK).expect("sealed");
        assert!(file_is_safe_to_read(sealed.as_fd()));
    }

    /// The same file as `file`, opened again through `/proc/self/fd` for `access` alone.
    fn reopened(file: &File, access: &mut OpenOptions) -> File {
        access
            .open(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .expect("a memfd opens again")
    }

    /// llvmpipe maps each plane for reading and writing: a memfd sealed against shrinking
    /// alone maps so; one sealed against writing, or against future writes, does not, nor does
    /// a descriptor open for reading alone or for writing alone, nor an empty file.
    #[test]
    fn only_a_file_that_maps_for_reading_and_writing_maps_as_llvmpipe_maps_it() {
        let sealed = memfd(MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING);
        fcntl_add_seals(&sealed, SealFlags::SHRINK).expect("sealed");
        assert!(maps_as_llvmpipe_maps_it(sealed.as_fd()));
        assert!(!maps_as_llvmpipe_maps_it(
            reopened(&sealed, OpenOptions::new().read(true)).as_fd()
        ));
        assert!(!maps_as_llvmpipe_maps_it(
            reopened(&sealed, OpenOptions::new().write(true)).as_fd()
        ));

        for seal in [SealFlags::WRITE, SealFlags::FUTURE_WRITE] {
            let file = memfd(MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING);
            fcntl_add_seals(&file, SealFlags::SHRINK | seal).expect("sealed");
            assert!(!maps_as_llvmpipe_maps_it(file.as_fd()), "{seal:?}");
        }

        let empty = File::from(memfd_create("yuna-test", MemfdFlags::CLOEXEC).expect("a memfd"));
        assert!(!maps_as_llvmpipe_maps_it(empty.as_fd()));
    }

    /// A hugetlbfs memfd may raise SIGBUS when a hole punched in it is read, sealed or not. Where
    /// the kernel has no hugetlbfs, the memfd cannot be made, and that branch is checked instead.
    #[test]
    fn a_huge_page_memfd_is_refused_even_sealed() {
        let flags = MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING | MemfdFlags::HUGETLB;
        match memfd_create("yuna-test", flags) {
            Ok(fd) => {
                let file = File::from(fd);
                fcntl_add_seals(&file, SealFlags::SHRINK).expect("sealed");
                assert!(!file_is_safe_to_read(file.as_fd()));
            }
            Err(error) => assert!(
                [Errno::INVAL, Errno::NOENT, Errno::NOSYS].contains(&error),
                "{error}"
            ),
        }
    }
}
