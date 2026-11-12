//! `linux-dmabuf`: clients' buffers in GPU memory, imported where the renderer can import them
//! ([ADR-0022](../../../docs/adr/0022-the-headless-backend-in-yuna.md)).
//!
//! The global offers the formats the renderer imports (`ImportDma::dmabuf_formats`), each a
//! fourcc code and a modifier. On the software device, it is version 3, without feedback: there is
//! no device to name. With `--gpu`, it has default feedback, which Smithay offers as version 6:
//! the render node as main device, one tranche of the formats. A renderer that imports no format
//! gets no global, and Mesa's clients use `wl_shm`.
//!
//! Each buffer a client creates is imported at once, so that the client learns at `create`
//! whether it can be used (`created` or `failed`; with `create_immed`, a failure is the
//! protocol's error, which ends the client). Parameters that break the protocol (no plane, a
//! plane past its file's end, parameters used twice) are refused by Smithay with a protocol
//! error, which ends that client only. A plane must lie in a file that can never shrink, a real
//! dma-buf or a file sealed against shrinking: any other fails, since a file shrunk under the
//! renderer would kill Yuna at drawing. A buffer that fails later, at drawing, is left out of
//! that frame by Smithay's renderer, which logs it.

use std::os::fd::BorrowedFd;
use std::os::unix::fs::MetadataExt;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::renderer::ImportDma;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::reexports::rustix::fs::{FsWord, SealFlags, fcntl_get_seals, fstatfs};
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::dmabuf::{
    DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier,
};

use crate::render::Device;
use crate::state::State;

/// The `linux-dmabuf` global a renderer offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobalKind {
    /// Version 3: the formats alone.
    WithoutFeedback,
    /// Default feedback, version 4 and later: the formats and the device they are for.
    WithDefaultFeedback,
}

/// The global a renderer on `device` that imports `format_count` formats offers; none without a
/// format. The software device has no device node to name in feedback.
fn global_kind(device: Device, format_count: usize) -> Option<GlobalKind> {
    if format_count == 0 {
        return None;
    }
    Some(match device {
        Device::Software => GlobalKind::WithoutFeedback,
        Device::Gpu => GlobalKind::WithDefaultFeedback,
    })
}

/// Creates the `linux-dmabuf` global for `renderer`, on `device`, and returns how many formats it
/// offers: 0 when there is no global.
pub(crate) fn create_global(
    display: &DisplayHandle,
    dmabuf_state: &mut DmabufState,
    renderer: &GlesRenderer,
    device: Device,
) -> usize {
    let formats = renderer.dmabuf_formats();
    let count = formats.indexset().len();
    match global_kind(device, count) {
        None => {
            tracing::info!(
                "the renderer imports no dmabuf: no linux-dmabuf global, clients use wl_shm"
            );
            return 0;
        }
        Some(GlobalKind::WithoutFeedback) => {
            dmabuf_state.create_global::<State>(display, formats);
        }
        Some(GlobalKind::WithDefaultFeedback) => {
            match default_feedback(renderer, formats.clone()) {
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
        }
    }
    tracing::info!(formats = count, "linux-dmabuf offered");
    count
}

/// The feedback a GPU's renderer gives: its render node as main device, and one tranche of the
/// formats it imports.
fn default_feedback(renderer: &GlesRenderer, formats: FormatSet) -> Result<DmabufFeedback, String> {
    let node = crate::render::render_node(renderer).map_err(|error| error.to_string())?;
    // Its device number, read without opening it.
    let main_device = std::fs::metadata(&node)
        .map_err(|error| format!("cannot read {}: {error}", node.display()))?
        .rdev();
    DmabufFeedbackBuilder::new(main_device, formats)
        .build()
        .map_err(|error| format!("cannot write the format table: {error}"))
}

/// `DMA_BUF_MAGIC` in `linux/magic.h`: the file system type `fstatfs` gives a real dma-buf.
const DMA_BUF_MAGIC: FsWord = 0x444d_4142;

/// `TMPFS_MAGIC` in `linux/magic.h`: the file system type of a memfd, unless it was made with
/// `MFD_HUGETLB`.
const TMPFS_MAGIC: FsWord = 0x0102_1994;

/// Whether every plane of `dmabuf` lies in a file that the renderer may always read. A renderer
/// may map a plane's file and read it each time it draws; Smithay checks the plane against the
/// file's size at `create` only, and catches SIGBUS for `wl_shm` alone. A file shrunk since
/// would kill Yuna with SIGBUS at drawing (ADR-0007). llvmpipe takes a plain memfd as a plane.
fn planes_are_safe_to_read(dmabuf: &Dmabuf) -> bool {
    dmabuf.handles().all(file_is_safe_to_read)
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
        if !planes_are_safe_to_read(&dmabuf) {
            tracing::debug!("a client's dmabuf lies in a file that may shrink: refused");
            notifier.failed();
            return;
        }
        match self.renderer.import_dmabuf(&dmabuf, None) {
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
    use std::fs::File;
    use std::os::fd::AsFd;

    use smithay::reexports::rustix::fs::{MemfdFlags, fcntl_add_seals, memfd_create};
    use smithay::reexports::rustix::io::Errno;

    use super::*;

    #[test]
    fn version_3_on_the_software_device_feedback_on_a_gpu_and_no_global_without_formats() {
        assert_eq!(
            global_kind(Device::Software, 12),
            Some(GlobalKind::WithoutFeedback)
        );
        assert_eq!(global_kind(Device::Software, 0), None);
        assert_eq!(
            global_kind(Device::Gpu, 12),
            Some(GlobalKind::WithDefaultFeedback)
        );
        assert_eq!(global_kind(Device::Gpu, 0), None);
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
