//! WLCS's integration ABI, version 1.8.1, declared from what the ABI is: the structs WLCS reads
//! and calls through, field for field in C's order, and the two calls of libwayland-client's the
//! shim makes. No text of WLCS's headers is copied (GPL-3.0-only, ADR-0149); `tests/abi.rs`
//! compiles a C program against those headers and checks every size, alignment and offset here.
//!
//! Every function pointer is an `Option`: C may leave any of them null, and `Option` of a
//! function pointer has the layout of a nullable one. WLCS calls each hook with the pointer the
//! shim gave it, and never at the same time from two threads (`hooks.rs` says what each hook
//! relies on).

#![expect(
    unsafe_code,
    reason = "libwayland-client's two calls, declared and wrapped"
)]

use std::ffi::{c_char, c_int};

/// `wl_fixed_t`: libwayland's 24.8 fixed-point number, a signed 32-bit integer whose low 8 bits
/// are the fraction.
pub type WlFixed = i32;

/// The logical value of a `wl_fixed_t`.
pub fn fixed_to_f64(value: WlFixed) -> f64 {
    f64::from(value) / 256.0
}

/// A client's `wl_display`, as WLCS's client side holds it: opaque.
#[repr(C)]
pub struct WlDisplay {
    _opaque: [u8; 0],
}

/// A client's `wl_surface` proxy, as WLCS's client side holds it: opaque.
#[repr(C)]
pub struct WlSurface {
    _opaque: [u8; 0],
}

/// A proxy of libwayland-client's, which every client object is, a `wl_surface` among them:
/// opaque.
#[repr(C)]
struct WlProxy {
    _opaque: [u8; 0],
}

/// The event loop WLCS offers `start_on_this_thread`, which the shim does not take: opaque.
#[repr(C)]
pub struct WlEventLoop {
    _opaque: [u8; 0],
}

/// The version of [`WlcsServerIntegration`] the shim provides.
pub const SERVER_INTEGRATION_VERSION: u32 = 1;

/// The version of [`WlcsDisplayServer`] the shim provides: 3, the newest WLCS 1.8.1 reads, with
/// `get_descriptor` (2) and `start_on_this_thread` (3), left null.
pub const DISPLAY_SERVER_VERSION: u32 = 3;

/// The version of [`WlcsIntegrationDescriptor`] the shim provides.
pub const INTEGRATION_DESCRIPTOR_VERSION: u32 = 1;

/// The version of [`WlcsPointer`] the shim provides; WLCS asks for this one exactly.
pub const POINTER_VERSION: u32 = 1;

/// The version of [`WlcsTouch`] the shim provides; WLCS asks for this one exactly.
pub const TOUCH_VERSION: u32 = 1;

/// What the module exports under the name `wlcs_server_integration`: how WLCS makes and frees a
/// server.
#[repr(C)]
pub struct WlcsServerIntegration {
    /// [`SERVER_INTEGRATION_VERSION`].
    pub version: u32,
    /// Makes a server, its loop not yet started, from the arguments left on WLCS's command line
    /// after its own.
    pub create_server: Option<
        unsafe extern "C" fn(argc: c_int, argv: *const *const c_char) -> *mut WlcsDisplayServer,
    >,
    /// Frees a server `create_server` made.
    pub destroy_server: Option<unsafe extern "C" fn(server: *mut WlcsDisplayServer)>,
}

/// One server, which WLCS makes for each test and drives through these hooks.
#[repr(C)]
pub struct WlcsDisplayServer {
    /// [`DISPLAY_SERVER_VERSION`].
    pub version: u32,
    /// Starts the server's loop without blocking: the other hooks work once it returns.
    pub start: Option<unsafe extern "C" fn(server: *mut WlcsDisplayServer)>,
    /// Stops the loop, and returns once the server is torn down.
    pub stop: Option<unsafe extern "C" fn(server: *mut WlcsDisplayServer)>,
    /// A new client's end of a connection, a descriptor WLCS then owns; negative on failure.
    pub create_client_socket: Option<unsafe extern "C" fn(server: *mut WlcsDisplayServer) -> c_int>,
    /// Moves the window of `client`'s `surface` so that its top-left corner is at (`x`, `y`) in
    /// the compositor's logical space.
    pub position_window_absolute: Option<
        unsafe extern "C" fn(
            server: *mut WlcsDisplayServer,
            client: *mut WlDisplay,
            surface: *mut WlSurface,
            x: c_int,
            y: c_int,
        ),
    >,
    /// A new pointer device.
    pub create_pointer:
        Option<unsafe extern "C" fn(server: *mut WlcsDisplayServer) -> *mut WlcsPointer>,
    /// A new touch device.
    pub create_touch:
        Option<unsafe extern "C" fn(server: *mut WlcsDisplayServer) -> *mut WlcsTouch>,
    /// What the server supports, which WLCS reads to skip the tests of what it lacks.
    pub get_descriptor: Option<
        unsafe extern "C" fn(server: *const WlcsDisplayServer) -> *const WlcsIntegrationDescriptor,
    >,
    /// Runs the loop on WLCS's thread, in WLCS's event loop: not provided.
    pub start_on_this_thread:
        Option<unsafe extern "C" fn(server: *mut WlcsDisplayServer, event_loop: *mut WlEventLoop)>,
}

/// What a server supports: the globals it offers, each at its newest version.
#[repr(C)]
pub struct WlcsIntegrationDescriptor {
    /// [`INTEGRATION_DESCRIPTOR_VERSION`].
    pub version: u32,
    /// How many entries `supported_extensions` holds.
    pub num_extensions: usize,
    /// The first of them.
    pub supported_extensions: *const WlcsExtensionDescriptor,
}

/// One global a server offers.
#[repr(C)]
pub struct WlcsExtensionDescriptor {
    /// Its interface's name, a C string.
    pub name: *const c_char,
    /// Its newest version.
    pub version: u32,
}

/// A pointer device.
#[repr(C)]
pub struct WlcsPointer {
    /// [`POINTER_VERSION`].
    pub version: u32,
    /// Moves it to (`x`, `y`) in the compositor's logical space.
    pub move_absolute:
        Option<unsafe extern "C" fn(pointer: *mut WlcsPointer, x: WlFixed, y: WlFixed)>,
    /// Moves it by (`dx`, `dy`).
    pub move_relative:
        Option<unsafe extern "C" fn(pointer: *mut WlcsPointer, dx: WlFixed, dy: WlFixed)>,
    /// Releases `button`, a Linux input code.
    pub button_up: Option<unsafe extern "C" fn(pointer: *mut WlcsPointer, button: c_int)>,
    /// Presses `button`, a Linux input code.
    pub button_down: Option<unsafe extern "C" fn(pointer: *mut WlcsPointer, button: c_int)>,
    /// Frees the device.
    pub destroy: Option<unsafe extern "C" fn(pointer: *mut WlcsPointer)>,
}

/// A touch device.
#[repr(C)]
pub struct WlcsTouch {
    /// [`TOUCH_VERSION`].
    pub version: u32,
    /// A touch begins at (`x`, `y`).
    pub touch_down: Option<unsafe extern "C" fn(touch: *mut WlcsTouch, x: WlFixed, y: WlFixed)>,
    /// The touch moves to (`x`, `y`).
    pub touch_move: Option<unsafe extern "C" fn(touch: *mut WlcsTouch, x: WlFixed, y: WlFixed)>,
    /// The touch ends.
    pub touch_up: Option<unsafe extern "C" fn(touch: *mut WlcsTouch)>,
    /// Frees the device.
    pub destroy: Option<unsafe extern "C" fn(touch: *mut WlcsTouch)>,
}

// SAFETY: these declarations match libwayland-client's own (`wayland-client-core.h`):
// `int wl_display_get_fd(struct wl_display *)` and `uint32_t wl_proxy_get_id(struct wl_proxy *)`.
// They are left undefined in the module and bound when WLCS loads it (`RTLD_NOW`), to the
// libwayland-client WLCS itself links and makes its clients with; the module links no copy of
// its own, so that both calls read the very objects WLCS made.
unsafe extern "C" {
    fn wl_display_get_fd(display: *mut WlDisplay) -> c_int;
    fn wl_proxy_get_id(proxy: *mut WlProxy) -> u32;
}

/// The descriptor of `display`'s connection: the very descriptor `create_client_socket` gave,
/// which WLCS's client connected through.
///
/// # Safety
///
/// `display` is a live `wl_display` of libwayland-client's, as WLCS passes to
/// `position_window_absolute`.
pub unsafe fn display_fd(display: *mut WlDisplay) -> c_int {
    // SAFETY: the caller gives a live `wl_display`; the call only reads its descriptor.
    unsafe { wl_display_get_fd(display) }
}

/// The protocol id of `surface`, as its client's side and Yuna both name it.
///
/// # Safety
///
/// `surface` is a live `wl_surface` proxy of libwayland-client's, as WLCS passes to
/// `position_window_absolute`.
pub unsafe fn surface_id(surface: *mut WlSurface) -> u32 {
    // SAFETY: the caller gives a live proxy; libwayland's `wl_surface` is a `wl_proxy` under
    // another name, so that the cast names the same object, and the call only reads its id.
    unsafe { wl_proxy_get_id(surface.cast::<WlProxy>()) }
}
