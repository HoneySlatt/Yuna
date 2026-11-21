//! What the shim tells WLCS Yuna supports: every global a client of an agent's seat is offered,
//! each at the version Yuna offers. When a test binds a global the registry does not offer, at
//! the version it needs, WLCS skips the test if this list leaves the global out, and fails it if
//! the list names it: the list must be the registry's exactly. `tests/descriptor.rs` compares it
//! with what a client of the shim's own compositor is announced. A global Yuna offers later
//! turns its tests on once it is listed here.

use std::ffi::CStr;

use crate::abi::{
    INTEGRATION_DESCRIPTOR_VERSION, WlcsExtensionDescriptor, WlcsIntegrationDescriptor,
};

/// The one global whose offer the renderer decides: `linux-dmabuf`, offered when it imports a
/// format (`docs/protocols.md`). WLCS 1.8.1 has no test of it.
pub const DMABUF: &CStr = c"zwp_linux_dmabuf_v1";

/// Each global an agent seat's client is offered, by its interface's name, with its version, in
/// the order Yuna announces them.
pub const EXTENSIONS: &[(&CStr, u32)] = &[
    (c"wl_compositor", 6),
    (c"wl_subcompositor", 1),
    (c"wl_shm", 2),
    (c"wl_data_device_manager", 3),
    (c"wl_seat", 9),
    (c"wl_output", 4),
    (c"xdg_wm_base", 7),
    (c"xdg_wm_dialog_v1", 1),
    (c"zxdg_decoration_manager_v1", 2),
    (c"zxdg_output_manager_v1", 3),
    (c"wp_viewporter", 1),
    (c"wp_fractional_scale_manager_v1", 1),
    (c"wp_presentation", 2),
    (c"wp_cursor_shape_manager_v1", 2),
    (c"xdg_activation_v1", 1),
    (c"wp_color_manager_v1", 3),
    (DMABUF, 6),
    (c"zwp_primary_selection_device_manager_v1", 1),
    (c"zwp_text_input_manager_v3", 1),
    (c"zwlr_screencopy_manager_v1", 3),
    (c"ext_image_copy_capture_manager_v1", 1),
    (c"ext_output_image_capture_source_manager_v1", 1),
    (c"zwlr_virtual_pointer_manager_v1", 2),
    (c"zwp_virtual_keyboard_manager_v1", 1),
];

/// [`EXTENSIONS`] in WLCS's form, held by one server: the descriptor points into its list, and
/// both live as long as the server, never moving.
pub struct Descriptor {
    /// The entries, whose names point into [`EXTENSIONS`]' static strings.
    extensions: Box<[WlcsExtensionDescriptor]>,
    /// What `get_descriptor` returns: it points into `extensions`, whose heap allocation a move
    /// of the `Descriptor` does not move.
    descriptor: WlcsIntegrationDescriptor,
}

impl Descriptor {
    /// The descriptor of [`EXTENSIONS`].
    pub fn new() -> Descriptor {
        let extensions: Box<[WlcsExtensionDescriptor]> = EXTENSIONS
            .iter()
            .map(|&(name, version)| WlcsExtensionDescriptor {
                name: name.as_ptr(),
                version,
            })
            .collect();
        let descriptor = WlcsIntegrationDescriptor {
            version: INTEGRATION_DESCRIPTOR_VERSION,
            num_extensions: extensions.len(),
            supported_extensions: extensions.as_ptr(),
        };
        Descriptor {
            extensions,
            descriptor,
        }
    }

    /// What `get_descriptor` returns: valid while `self` lives.
    pub fn as_ptr(&self) -> *const WlcsIntegrationDescriptor {
        debug_assert_eq!(
            self.descriptor.supported_extensions,
            self.extensions.as_ptr()
        );
        &raw const self.descriptor
    }
}

impl Default for Descriptor {
    fn default() -> Descriptor {
        Descriptor::new()
    }
}
