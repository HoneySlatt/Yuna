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
//! whether it can be used (`created` or `failed`). Parameters that break the protocol (no plane,
//! a plane past its file's end, parameters used twice) are refused by Smithay with a protocol
//! error, which ends that client only. A buffer that fails later, at drawing, is left out of that
//! frame by Smithay's renderer, which logs it.

use std::os::unix::fs::MetadataExt;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::renderer::ImportDma;
use smithay::backend::renderer::gles::GlesRenderer;
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
                    tracing::warn!(%reason, "no dmabuf feedback; linux-dmabuf is offered without it");
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
        match self.renderer.import_dmabuf(&dmabuf, None) {
            Ok(_) => {
                if notifier.successful::<State>().is_err() {
                    tracing::debug!("a client left before its dmabuf's buffer was created");
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
    use super::*;

    #[test]
    fn the_software_device_offers_version_3_and_nothing_without_formats() {
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
}
