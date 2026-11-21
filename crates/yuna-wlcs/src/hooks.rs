//! The hooks WLCS calls, behind the C ABI of `abi.rs`, and the one symbol the module exports.
//!
//! **Pointers.** Each object WLCS is given is a `Box` of the shim's, whose first field is the C
//! struct WLCS reads: [`ServerHandle`], [`PointerDevice`], [`TouchDevice`], each `#[repr(C)]`,
//! so that the struct's address is the box's. WLCS hands back only what it was given, until it
//! frees it through the matching hook (`destroy_server`, a device's `destroy`), and calls one
//! hook at a time. A hook reads its object through a shared reference, and the C fields are
//! never written after the box is made, so that WLCS's own reads of them never race a write.
//!
//! **Panics** abort the process once the standard hook printed the panic's message
//! ([`setup_process`]): none unwinds into WLCS's C++, and none is caught and hidden. A panic of
//! Yuna's state is fatal, as in Yuna's program, and the server's thread is the only thread of
//! Yuna's the shim runs: no Xwayland, no MCP.

use std::ffi::{c_char, c_int};
use std::os::fd::IntoRawFd;
use std::sync::{Arc, Once};

use tracing::level_filters::LevelFilter;

use crate::abi::{
    DISPLAY_SERVER_VERSION, POINTER_VERSION, SERVER_INTEGRATION_VERSION, TOUCH_VERSION, WlDisplay,
    WlFixed, WlSurface, WlcsDisplayServer, WlcsIntegrationDescriptor, WlcsPointer,
    WlcsServerIntegration, WlcsTouch, display_fd, fixed_to_f64, surface_id,
};
use crate::descriptor::Descriptor;
use crate::server::{Link, Markers, Server, report};

/// What WLCS looks the module up by: how it makes and frees a server.
#[unsafe(export_name = "wlcs_server_integration")]
pub static WLCS_SERVER_INTEGRATION: WlcsServerIntegration = WlcsServerIntegration {
    version: SERVER_INTEGRATION_VERSION,
    create_server: Some(create_server),
    destroy_server: Some(destroy_server),
};

/// The marker of every test that asks for touch, which Yuna does not have.
const TOUCH: &str = "touch";

/// One server, as WLCS holds it.
#[repr(C)]
struct ServerHandle {
    /// First: WLCS's pointer to it is the handle's own address.
    abi: WlcsDisplayServer,
    descriptor: Descriptor,
    server: Server,
}

/// A pointer device, as WLCS holds it: the link to its server's thread, none when the server
/// did not run when it was made.
#[repr(C)]
struct PointerDevice {
    /// First, as [`ServerHandle::abi`].
    abi: WlcsPointer,
    link: Option<Link>,
}

/// A touch device, which does nothing but print the touch marker: Yuna has no touch.
#[repr(C)]
struct TouchDevice {
    /// First, as [`ServerHandle::abi`].
    abi: WlcsTouch,
    markers: Arc<Markers>,
}

/// Sets what the whole process keeps once the module is loaded, at the first server: panics
/// abort, and Yuna logs to standard error, at the level `RUST_LOG` names (`error`, `warn`,
/// `info`, `debug` or `trace`), `warn` without it.
fn setup_process() {
    static SETUP: Once = Once::new();
    SETUP.call_once(|| {
        let standard_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            standard_hook(info);
            report("yuna-wlcs: a panic in Yuna or its shim is fatal: WLCS stops");
            std::process::abort();
        }));
        start_logging();
    });
}

/// Logs to standard error at `RUST_LOG`'s level, `warn` without it: a value that names no level
/// is reported and ignored. The module's own copy of `tracing`, so that no logger of WLCS's is
/// touched.
fn start_logging() {
    let value = std::env::var_os("RUST_LOG");
    let level = match value
        .as_ref()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .as_deref()
    {
        None | Some("warn") => Some(LevelFilter::WARN),
        Some("error") => Some(LevelFilter::ERROR),
        Some("info") => Some(LevelFilter::INFO),
        Some("debug") => Some(LevelFilter::DEBUG),
        Some("trace") => Some(LevelFilter::TRACE),
        Some(_) => None,
    };
    let _ = tracing_subscriber::fmt()
        .with_max_level(level.unwrap_or(LevelFilter::WARN))
        .with_writer(std::io::stderr)
        .try_init();
    if level.is_none() {
        tracing::warn!(
            ?value,
            "RUST_LOG names no level (error, warn, info, debug, trace); logging at warn"
        );
    }
}

/// The handle behind `server`.
///
/// # Safety
///
/// `server` is what [`create_server`] returned, not yet given to [`destroy_server`].
unsafe fn handle<'a>(server: *const WlcsDisplayServer) -> &'a ServerHandle {
    // SAFETY: by the caller, `server` is the address of a live `ServerHandle`'s box, whose first
    // field it names (`repr(C)`), and the pointer came from the box's own: it covers the whole
    // handle. Nothing writes the handle while WLCS holds it, but through its `Mutex`es.
    unsafe { &*server.cast::<ServerHandle>() }
}

/// `create_server`: a server, not yet running. WLCS's arguments for the compositor are
/// ignored: the shim takes none.
unsafe extern "C" fn create_server(
    _argc: c_int,
    _argv: *const *const c_char,
) -> *mut WlcsDisplayServer {
    setup_process();
    let handle = Box::new(ServerHandle {
        abi: WlcsDisplayServer {
            version: DISPLAY_SERVER_VERSION,
            start: Some(start),
            stop: Some(stop),
            create_client_socket: Some(create_client_socket),
            position_window_absolute: Some(position_window_absolute),
            create_pointer: Some(create_pointer),
            create_touch: Some(create_touch),
            get_descriptor: Some(get_descriptor),
            start_on_this_thread: None,
        },
        descriptor: Descriptor::new(),
        server: Server::new(),
    });
    Box::into_raw(handle).cast::<WlcsDisplayServer>()
}

/// `destroy_server`: stops `server` if it runs, and frees it.
unsafe extern "C" fn destroy_server(server: *mut WlcsDisplayServer) {
    if server.is_null() {
        return;
    }
    // SAFETY: WLCS gives back what `create_server` made, once, for the last time: the box is
    // rebuilt from its own pointer and dropped, which stops its server (`Server`'s drop).
    drop(unsafe { Box::from_raw(server.cast::<ServerHandle>()) });
}

/// `start`: starts the server's thread, and returns once Yuna runs. A Yuna that does not start
/// is reported, and the server stays stopped: each test then fails at its first client.
unsafe extern "C" fn start(server: *mut WlcsDisplayServer) {
    // SAFETY: WLCS calls the hook with the server it was given (`handle`'s contract).
    let handle = unsafe { handle(server) };
    if let Err(error) = handle.server.start() {
        report(&format!("yuna-wlcs: the server did not start: {error}"));
    }
}

/// `stop`: returns once the server's thread has ended (`Server::stop`).
unsafe extern "C" fn stop(server: *mut WlcsDisplayServer) {
    // SAFETY: as in `start`.
    let handle = unsafe { handle(server) };
    handle.server.stop();
}

/// `create_client_socket`: a new client's end of a connection, which WLCS then owns; -1 when
/// none could be made, said on standard error.
unsafe extern "C" fn create_client_socket(server: *mut WlcsDisplayServer) -> c_int {
    // SAFETY: as in `start`.
    let handle = unsafe { handle(server) };
    match handle.server.connect_client() {
        Ok(stream) => stream.into_raw_fd(),
        Err(error) => {
            report(&format!("yuna-wlcs: no client socket: {error}"));
            -1
        }
    }
}

/// `position_window_absolute`: places the window of `client`'s `surface`, its top-left corner at
/// (`x`, `y`). The client is found by its connection's descriptor, the one
/// `create_client_socket` gave; the surface, by its protocol id, which WLCS's roundtrip before
/// the call has made known to Yuna.
unsafe extern "C" fn position_window_absolute(
    server: *mut WlcsDisplayServer,
    client: *mut WlDisplay,
    surface: *mut WlSurface,
    x: c_int,
    y: c_int,
) {
    // SAFETY: as in `start`.
    let handle = unsafe { handle(server) };
    if client.is_null() || surface.is_null() {
        report("yuna-wlcs: a window to place, of no client or no surface: nothing is placed");
        return;
    }
    // SAFETY: WLCS passes the live `wl_display` of one of its clients, and one of its live
    // `wl_surface`s, as the hook's ABI says.
    let (fd, id) = unsafe { (display_fd(client), surface_id(surface)) };
    handle.server.place_window(fd, id, x, y);
}

/// `create_pointer`: a pointer device of the server's seat. Every device moves the seat's one
/// pointer.
unsafe extern "C" fn create_pointer(server: *mut WlcsDisplayServer) -> *mut WlcsPointer {
    // SAFETY: as in `start`.
    let handle = unsafe { handle(server) };
    let device = Box::new(PointerDevice {
        abi: WlcsPointer {
            version: POINTER_VERSION,
            move_absolute: Some(pointer_move_absolute),
            move_relative: Some(pointer_move_relative),
            button_up: Some(pointer_button_up),
            button_down: Some(pointer_button_down),
            destroy: Some(pointer_destroy),
        },
        link: handle.server.link(),
    });
    Box::into_raw(device).cast::<WlcsPointer>()
}

/// `create_touch`: a touch device, which prints the touch marker, once for the server, and does
/// nothing else: Yuna has no touch.
unsafe extern "C" fn create_touch(server: *mut WlcsDisplayServer) -> *mut WlcsTouch {
    // SAFETY: as in `start`.
    let handle = unsafe { handle(server) };
    let markers = handle.server.markers().clone();
    markers.unsupported(TOUCH, None);
    let device = Box::new(TouchDevice {
        abi: WlcsTouch {
            version: TOUCH_VERSION,
            touch_down: Some(touch_down),
            touch_move: Some(touch_move),
            touch_up: Some(touch_up),
            destroy: Some(touch_destroy),
        },
        markers,
    });
    Box::into_raw(device).cast::<WlcsTouch>()
}

/// `get_descriptor`: the globals Yuna offers, valid while the server lives.
unsafe extern "C" fn get_descriptor(
    server: *const WlcsDisplayServer,
) -> *const WlcsIntegrationDescriptor {
    // SAFETY: as in `start`.
    let handle = unsafe { handle(server) };
    handle.descriptor.as_ptr()
}

/// The pointer device behind `pointer`.
///
/// # Safety
///
/// `pointer` is what [`create_pointer`] returned, not yet given to [`pointer_destroy`].
unsafe fn pointer_device<'a>(pointer: *mut WlcsPointer) -> &'a PointerDevice {
    // SAFETY: as in `handle`, for a `PointerDevice`'s box.
    unsafe { &*pointer.cast::<PointerDevice>() }
}

/// The link of a pointer device, or what WLCS is told when it has none.
fn link_of(device: &PointerDevice) -> Option<&Link> {
    if device.link.is_none() {
        report("yuna-wlcs: a pointer of a server that did not run: nothing moves");
    }
    device.link.as_ref()
}

/// `move_absolute`: moves the seat's pointer to (`x`, `y`).
unsafe extern "C" fn pointer_move_absolute(pointer: *mut WlcsPointer, x: WlFixed, y: WlFixed) {
    // SAFETY: WLCS calls a device's hook with the device it was given (`pointer_device`).
    let device = unsafe { pointer_device(pointer) };
    if let Some(link) = link_of(device) {
        link.move_pointer_to(fixed_to_f64(x), fixed_to_f64(y));
    }
}

/// `move_relative`: moves the seat's pointer by (`dx`, `dy`).
unsafe extern "C" fn pointer_move_relative(pointer: *mut WlcsPointer, dx: WlFixed, dy: WlFixed) {
    // SAFETY: as in `pointer_move_absolute`.
    let device = unsafe { pointer_device(pointer) };
    if let Some(link) = link_of(device) {
        link.move_pointer_by(fixed_to_f64(dx), fixed_to_f64(dy));
    }
}

/// `button_down`: presses `button`.
unsafe extern "C" fn pointer_button_down(pointer: *mut WlcsPointer, button: c_int) {
    // SAFETY: as in `pointer_move_absolute`.
    let device = unsafe { pointer_device(pointer) };
    if let Some(link) = link_of(device) {
        link.press_button(button, true);
    }
}

/// `button_up`: releases `button`.
unsafe extern "C" fn pointer_button_up(pointer: *mut WlcsPointer, button: c_int) {
    // SAFETY: as in `pointer_move_absolute`.
    let device = unsafe { pointer_device(pointer) };
    if let Some(link) = link_of(device) {
        link.press_button(button, false);
    }
}

/// `destroy`: frees the pointer device.
unsafe extern "C" fn pointer_destroy(pointer: *mut WlcsPointer) {
    if pointer.is_null() {
        return;
    }
    // SAFETY: WLCS gives back what `create_pointer` made, once, for the last time.
    drop(unsafe { Box::from_raw(pointer.cast::<PointerDevice>()) });
}

/// The touch device behind `touch`.
///
/// # Safety
///
/// `touch` is what [`create_touch`] returned, not yet given to [`touch_destroy`].
unsafe fn touch_device<'a>(touch: *mut WlcsTouch) -> &'a TouchDevice {
    // SAFETY: as in `handle`, for a `TouchDevice`'s box.
    unsafe { &*touch.cast::<TouchDevice>() }
}

/// `touch_down`: the touch marker.
unsafe extern "C" fn touch_down(touch: *mut WlcsTouch, _x: WlFixed, _y: WlFixed) {
    // SAFETY: WLCS calls a device's hook with the device it was given (`touch_device`).
    unsafe { touch_device(touch) }
        .markers
        .unsupported(TOUCH, None);
}

/// `touch_move`: the touch marker.
unsafe extern "C" fn touch_move(touch: *mut WlcsTouch, _x: WlFixed, _y: WlFixed) {
    // SAFETY: as in `touch_down`.
    unsafe { touch_device(touch) }
        .markers
        .unsupported(TOUCH, None);
}

/// `touch_up`: the touch marker.
unsafe extern "C" fn touch_up(touch: *mut WlcsTouch) {
    // SAFETY: as in `touch_down`.
    unsafe { touch_device(touch) }
        .markers
        .unsupported(TOUCH, None);
}

/// `destroy`: frees the touch device.
unsafe extern "C" fn touch_destroy(touch: *mut WlcsTouch) {
    if touch.is_null() {
        return;
    }
    // SAFETY: WLCS gives back what `create_touch` made, once, for the last time.
    drop(unsafe { Box::from_raw(touch.cast::<TouchDevice>()) });
}
