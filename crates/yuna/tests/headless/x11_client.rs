//! An X11 test client on x11rb, as an X11 program is to Yuna's X11 window manager: it reads its
//! scale from `Xft.dpi`, makes and maps windows, draws known pixels into them with `PutImage`,
//! asks for states and modes, and keeps the events X11 sends it.
//!
//! It waits on Xwayland, which Yuna's loop serves, so each of its steps runs on a thread of its
//! own while the test runs the compositor's loop ([`step`]). It connects through the display's
//! abstract socket, giving the seat's cookie itself: tests share one process, whose
//! `XAUTHORITY` they cannot set.

use std::collections::{HashMap, VecDeque};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::sync::{ConnectionExt as _, Counter, Int64};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask,
    Gcontext, ImageFormat, ImageOrder, PropMode, UNMAP_NOTIFY_EVENT, UnmapNotifyEvent, Window,
    WindowClass,
};
use x11rb::rust_connection::{DefaultStream, RustConnection};
use x11rb::wrapper::ConnectionExt as _;
use yuna::{Compositor, X11Display};

/// The cookie of a display's Xauthority file, which `xwayland.rs` reads too.
#[path = "../common/xauthority.rs"]
pub(crate) mod xauthority;

/// How long a step waits for X11 or the compositor: a start takes a fraction of a second, longer
/// while every test runs at once.
pub(crate) const PATIENCE: Duration = Duration::from_secs(60);

x11rb::atom_manager! {
    /// The atoms the test client names.
    pub(crate) Atoms: AtomsCookie {
        UTF8_STRING,
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_STATE,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_HIDDEN,
        _NET_ACTIVE_WINDOW,
        _NET_CLIENT_LIST,
        _NET_WM_PING,
        _NET_WM_SYNC_REQUEST,
        _NET_WM_SYNC_REQUEST_COUNTER,
        _NET_STARTUP_ID,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DIALOG,
        _NET_WM_WINDOW_TYPE_POPUP_MENU,
        _NET_WM_WINDOW_TYPE_TOOLTIP,
        _XWAYLAND_ALLOW_COMMITS,
        WL_SURFACE_SERIAL,
        WM_STATE,
    }
}

/// An image of R, G, B pixels, row by row: what the client draws into a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 3]>,
}

impl Image {
    /// One-pixel lines of colour every 50 pixels across and down, over a one-pixel checkerboard:
    /// a window's text and rules at their sharpest.
    pub(crate) fn sharp(width: u32, height: u32) -> Image {
        let pixels = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| {
                if x % 50 == 7 {
                    [255, 64, 0]
                } else if y % 50 == 13 {
                    [0, 160, 255]
                } else if (x + y) % 2 == 0 {
                    [255, 255, 255]
                } else {
                    [0, 0, 0]
                }
            })
            .collect();
        Image {
            width,
            height,
            pixels,
        }
    }

    /// A menu's pixels, unlike any window's of [`Image::sharp`] wherever it lies over one: green
    /// and magenta lines every 40 and 30 pixels over a checkerboard of two-pixel columns.
    pub(crate) fn menu(width: u32, height: u32) -> Image {
        let pixels = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| {
                if x % 40 == 3 {
                    [0, 200, 60]
                } else if y % 30 == 11 {
                    [220, 0, 200]
                } else if (x / 2 + y) % 2 == 0 {
                    [250, 240, 120]
                } else {
                    [20, 30, 90]
                }
            })
            .collect();
        Image {
            width,
            height,
            pixels,
        }
    }

    /// The pixels of the rectangle of `width` × `height` at (`x`, `y`) of an image whose bytes
    /// are `bytes`, R, G, B, row by row, `stride` pixels a row.
    pub(crate) fn crop(
        bytes: &[u8],
        stride: u32,
        (x, y, width, height): (u32, u32, u32, u32),
    ) -> Vec<u8> {
        let mut cropped = Vec::with_capacity((width * height * 3) as usize);
        for row in y..y + height {
            let start = ((row * stride + x) * 3) as usize;
            cropped.extend_from_slice(&bytes[start..start + (width * 3) as usize]);
        }
        cropped
    }

    pub(crate) fn at(&self, x: u32, y: u32) -> [u8; 3] {
        self.pixels[(y * self.width + x) as usize]
    }
}

/// What a window is made with.
#[derive(Debug, Clone)]
pub(crate) struct WindowSpec {
    pub title: String,
    pub class: String,
    /// Whether it lists `WM_DELETE_WINDOW`.
    pub deletes: bool,
    /// Whether it asks for fullscreen before it is mapped.
    pub fullscreen: bool,
    /// Its background, painted by X11 when it is exposed; none, and only what the client draws
    /// shows.
    pub background: Option<u32>,
    /// Its place and size as made: x, y, width, height.
    pub rect: (i16, i16, u16, u16),
    /// Whether it is override-redirect, a menu or a tooltip: mapped by its client, no manager
    /// asked.
    pub override_redirect: bool,
    /// The window it is a dialog of, its `WM_TRANSIENT_FOR`.
    pub transient_for: Option<Window>,
    /// Its `_NET_WM_WINDOW_TYPE`, one atom.
    pub window_type: Option<u32>,
    /// Its `_NET_STARTUP_ID`.
    pub startup_id: Option<String>,
    /// Whether it lists `_NET_WM_PING`: it answers pings ([`X11Client::answer_pings`]).
    pub pings: bool,
    /// Whether it lists `_NET_WM_SYNC_REQUEST`, with a counter of its own in
    /// `_NET_WM_SYNC_REQUEST_COUNTER` ([`X11Client::counter_of`]).
    pub syncs: bool,
}

impl WindowSpec {
    pub(crate) fn named(title: &str) -> WindowSpec {
        WindowSpec {
            title: title.to_owned(),
            class: "X11Test".to_owned(),
            deletes: true,
            fullscreen: false,
            background: None,
            rect: (0, 0, 640, 480),
            override_redirect: false,
            transient_for: None,
            window_type: None,
            startup_id: None,
            pings: false,
            syncs: false,
        }
    }

    /// An override-redirect window of `width` × `height` at (`x`, `y`) on the root, painted
    /// `background`: a menu or a tooltip.
    pub(crate) fn menu(title: &str, x: i16, y: i16, width: u16, height: u16) -> WindowSpec {
        WindowSpec {
            rect: (x, y, width, height),
            override_redirect: true,
            deletes: false,
            ..WindowSpec::named(title)
        }
    }
}

/// A window's place in X11 pixels, as `GetGeometry` gives it: relative to the root, since Yuna
/// does not reparent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The X11 test client: its connection, and the events read while it waited for others.
pub(crate) struct X11Client {
    pub connection: RustConnection,
    pub root: Window,
    pub atoms: Atoms,
    /// `Xft.dpi` in `RESOURCE_MANAGER`: 96 × XWayland's scale.
    pub dpi: u32,
    events: VecDeque<Event>,
    gc: Option<Gcontext>,
    /// The sync counter of each window made with one.
    counters: HashMap<Window, Counter>,
}

impl X11Client {
    /// Connects to `display` through its abstract socket, with its cookie, and reads `Xft.dpi`.
    pub(crate) fn connect(display: &X11Display) -> Result<X11Client, String> {
        let cookie = xauthority::cookie_of(&display.xauthority)?;
        let address = SocketAddr::from_abstract_name(format!("/tmp/.X11-unix/X{}", display.number))
            .map_err(|e| e.to_string())?;
        let stream = UnixStream::connect_addr(&address).map_err(|e| e.to_string())?;
        let (stream, _) = DefaultStream::from_unix_stream(stream).map_err(|e| e.to_string())?;
        let connection = RustConnection::connect_to_stream_with_auth_info(
            stream,
            0,
            b"MIT-MAGIC-COOKIE-1".to_vec(),
            cookie,
        )
        .map_err(|e| e.to_string())?;
        let root = connection.setup().roots[0].root;
        let atoms = Atoms::new(&connection)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let resources = connection
            .get_property(
                false,
                root,
                AtomEnum::RESOURCE_MANAGER,
                AtomEnum::STRING,
                0,
                1024,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .value;
        let dpi = String::from_utf8_lossy(&resources)
            .lines()
            .find_map(|line| line.strip_prefix("Xft.dpi:"))
            .and_then(|value| value.trim().parse().ok())
            .ok_or("no Xft.dpi")?;
        Ok(X11Client {
            connection,
            root,
            atoms,
            dpi,
            events: VecDeque::new(),
            gc: None,
            counters: HashMap::new(),
        })
    }

    /// XWayland's scale, from `Xft.dpi`.
    pub(crate) fn scale(&self) -> f64 {
        f64::from(self.dpi) / 96.0
    }

    /// Makes a window as `spec` says and maps it; returns once X11 says it is mapped.
    pub(crate) fn open(&mut self, spec: &WindowSpec) -> Result<Window, String> {
        let window = self.create(spec)?;
        self.connection
            .map_window(window)
            .map_err(|e| e.to_string())?;
        self.connection.flush().map_err(|e| e.to_string())?;
        self.wait_for(
            "the window mapped",
            |event| matches!(event, Event::MapNotify(mapped) if mapped.window == window),
        )?;
        Ok(window)
    }

    /// Makes a window as `spec` says, its events selected, its properties set, not mapped.
    pub(crate) fn create(&mut self, spec: &WindowSpec) -> Result<Window, String> {
        let connection = &self.connection;
        let window = connection.generate_id().map_err(|e| e.to_string())?;
        let mut aux = CreateWindowAux::new()
            .event_mask(
                EventMask::STRUCTURE_NOTIFY
                    | EventMask::KEY_PRESS
                    | EventMask::BUTTON_PRESS
                    | EventMask::EXPOSURE
                    | EventMask::FOCUS_CHANGE
                    | EventMask::PROPERTY_CHANGE,
            )
            .override_redirect(u32::from(spec.override_redirect));
        if let Some(background) = spec.background {
            aux = aux.background_pixel(background);
        }
        let (x, y, width, height) = spec.rect;
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                self.root,
                x,
                y,
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &aux,
            )
            .map_err(|e| e.to_string())?;
        let atoms = self.atoms;
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                spec.title.as_bytes(),
            )
            .map_err(fail)?;
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                atoms._NET_WM_NAME,
                atoms.UTF8_STRING,
                spec.title.as_bytes(),
            )
            .map_err(fail)?;
        let class = format!("x11-test\0{}\0", spec.class);
        connection
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                class.as_bytes(),
            )
            .map_err(fail)?;
        let protocols: Vec<u32> = [
            (spec.deletes, atoms.WM_DELETE_WINDOW),
            (spec.pings, atoms._NET_WM_PING),
            (spec.syncs, atoms._NET_WM_SYNC_REQUEST),
        ]
        .into_iter()
        .filter_map(|(listed, atom)| listed.then_some(atom))
        .collect();
        connection
            .change_property32(
                PropMode::REPLACE,
                window,
                atoms.WM_PROTOCOLS,
                AtomEnum::ATOM,
                &protocols,
            )
            .map_err(fail)?;
        let mut counter_made = None;
        if spec.syncs {
            let counter = connection.generate_id().map_err(|e| e.to_string())?;
            connection
                .sync_create_counter(counter, Int64 { hi: 0, lo: 0 })
                .map_err(fail)?;
            connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms._NET_WM_SYNC_REQUEST_COUNTER,
                    AtomEnum::CARDINAL,
                    &[counter],
                )
                .map_err(fail)?;
            counter_made = Some(counter);
        }
        if let Some(parent) = spec.transient_for {
            connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    AtomEnum::WM_TRANSIENT_FOR,
                    AtomEnum::WINDOW,
                    &[parent],
                )
                .map_err(fail)?;
        }
        if let Some(window_type) = spec.window_type {
            connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms._NET_WM_WINDOW_TYPE,
                    AtomEnum::ATOM,
                    &[window_type],
                )
                .map_err(fail)?;
        }
        if let Some(startup_id) = &spec.startup_id {
            connection
                .change_property8(
                    PropMode::REPLACE,
                    window,
                    atoms._NET_STARTUP_ID,
                    atoms.UTF8_STRING,
                    startup_id.as_bytes(),
                )
                .map_err(fail)?;
        }
        connection
            .change_property32(
                PropMode::REPLACE,
                window,
                atoms._NET_WM_PID,
                AtomEnum::CARDINAL,
                &[std::process::id()],
            )
            .map_err(fail)?;
        if spec.fullscreen {
            connection
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    atoms._NET_WM_STATE,
                    AtomEnum::ATOM,
                    &[atoms._NET_WM_STATE_FULLSCREEN],
                )
                .map_err(fail)?;
        }
        if let Some(counter) = counter_made {
            self.counters.insert(window, counter);
        }
        Ok(window)
    }

    /// `window`'s place.
    pub(crate) fn geometry(&self, window: Window) -> Result<Geometry, String> {
        let reply = self
            .connection
            .get_geometry(window)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok(Geometry {
            x: i32::from(reply.x),
            y: i32::from(reply.y),
            width: u32::from(reply.width),
            height: u32::from(reply.height),
        })
    }

    /// Draws `image` into `window` from its top-left corner with `PutImage`, in strips that fit a
    /// request, then waits until X11 handled it.
    pub(crate) fn draw(&mut self, window: Window, image: &Image) -> Result<(), String> {
        let fail = |e: x11rb::errors::ConnectionError| e.to_string();
        let gc = match self.gc {
            Some(gc) => gc,
            None => {
                let gc = self.connection.generate_id().map_err(|e| e.to_string())?;
                self.connection
                    .create_gc(gc, window, &CreateGCAux::new())
                    .map_err(fail)?;
                self.gc = Some(gc);
                gc
            }
        };
        let little = self.connection.setup().image_byte_order == ImageOrder::LSB_FIRST;
        let depth = self.connection.setup().roots[0].root_depth;
        let row_bytes = image.width as usize * 4;
        let room = self.connection.maximum_request_bytes().saturating_sub(64);
        let rows = (room / row_bytes.max(1)).clamp(1, image.height as usize);
        for top in (0..image.height as usize).step_by(rows) {
            let bottom = (top + rows).min(image.height as usize);
            let mut data = Vec::with_capacity((bottom - top) * row_bytes);
            for pixel in &image.pixels[top * image.width as usize..bottom * image.width as usize] {
                let [r, g, b] = *pixel;
                if little {
                    data.extend_from_slice(&[b, g, r, 0]);
                } else {
                    data.extend_from_slice(&[0, r, g, b]);
                }
            }
            self.connection
                .put_image(
                    ImageFormat::Z_PIXMAP,
                    window,
                    gc,
                    image.width as u16,
                    (bottom - top) as u16,
                    0,
                    top as i16,
                    0,
                    depth,
                    &data,
                )
                .map_err(fail)?;
        }
        self.sync()
    }

    /// Waits until X11 handled every request sent: a round trip.
    pub(crate) fn sync(&self) -> Result<(), String> {
        self.connection
            .get_input_focus()
            .map_err(|e| e.to_string())?
            .reply()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// The window that has the X input focus.
    pub(crate) fn input_focus(&self) -> Result<Window, String> {
        Ok(self
            .connection
            .get_input_focus()
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .focus)
    }

    /// The atoms of `window`'s `_NET_WM_STATE`, as its manager set them.
    pub(crate) fn net_wm_state(&self, window: Window) -> Result<Vec<u32>, String> {
        let reply = self
            .connection
            .get_property(
                false,
                window,
                self.atoms._NET_WM_STATE,
                AtomEnum::ATOM,
                0,
                64,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok(reply.value32().map(Iterator::collect).unwrap_or_default())
    }

    /// The root's `_NET_CLIENT_LIST`.
    pub(crate) fn client_list(&self) -> Result<Vec<Window>, String> {
        let reply = self
            .connection
            .get_property(
                false,
                self.root,
                self.atoms._NET_CLIENT_LIST,
                AtomEnum::WINDOW,
                0,
                4096,
            )
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok(reply.value32().map(Iterator::collect).unwrap_or_default())
    }

    /// Asks the manager to add or remove `window`'s fullscreen state, as EWMH says a client
    /// does once it is mapped.
    pub(crate) fn ask_fullscreen(&self, window: Window, fullscreen: bool) -> Result<(), String> {
        let message = ClientMessageEvent::new(
            32,
            window,
            self.atoms._NET_WM_STATE,
            [
                u32::from(fullscreen),
                self.atoms._NET_WM_STATE_FULLSCREEN,
                0,
                1,
                0,
            ],
        );
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                message,
            )
            .map_err(|e| e.to_string())?;
        self.sync()
    }

    /// The values of `window`'s property `atom` in format 32, whatever its type; none when it is
    /// not set.
    pub(crate) fn property32(&self, window: Window, atom: u32) -> Result<Option<Vec<u32>>, String> {
        let reply = self
            .connection
            .get_property(false, window, atom, AtomEnum::ANY, 0, 1024)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok(reply.value32().map(Iterator::collect))
    }

    /// `window`'s `_XWAYLAND_ALLOW_COMMITS` as its manager set it: 0 while Xwayland holds its
    /// commits, none when never set.
    pub(crate) fn allow_commits(&self, window: Window) -> Result<Option<u32>, String> {
        let values = self.property32(window, self.atoms._XWAYLAND_ALLOW_COMMITS)?;
        Ok(values.and_then(|values| values.first().copied()))
    }

    /// The sync counter of `window`, made with [`WindowSpec::syncs`].
    pub(crate) fn counter_of(&self, window: Window) -> Option<Counter> {
        self.counters.get(&window).copied()
    }

    /// Sets `window`'s sync counter to `value`, as a client does once it drew the size a sync
    /// request came with.
    pub(crate) fn set_counter(&self, window: Window, value: u64) -> Result<(), String> {
        let counter = self.counter_of(window).ok_or("no sync counter")?;
        let value = Int64 {
            hi: (value >> 32) as i32,
            lo: value as u32,
        };
        self.connection
            .sync_set_counter(counter, value)
            .map_err(|e| e.to_string())?;
        self.sync()
    }

    /// Sends a `WL_SURFACE_SERIAL` naming `window` and `serial` to the manager, as Xwayland does:
    /// from a client, it carries `SendEvent`'s bit.
    pub(crate) fn forge_serial(&self, window: Window, serial: u64) -> Result<(), String> {
        let message = ClientMessageEvent::new(
            32,
            window,
            self.atoms.WL_SURFACE_SERIAL,
            [serial as u32, (serial >> 32) as u32, 0, 0, 0],
        );
        self.connection
            .send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT, message)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Withdraws `window`, as ICCCM 4.1.4 says: it is unmapped, and a synthetic `UnmapNotify`
    /// goes to the root.
    pub(crate) fn withdraw(&self, window: Window) -> Result<(), String> {
        self.connection
            .unmap_window(window)
            .map_err(|e| e.to_string())?;
        let notify = UnmapNotifyEvent {
            response_type: UNMAP_NOTIFY_EVENT,
            sequence: 0,
            event: self.root,
            window,
            from_configure: false,
        };
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                notify,
            )
            .map_err(|e| e.to_string())?;
        self.sync()
    }

    /// Answers every `_NET_WM_PING` that came, as EWMH says: the message back to the root.
    /// Returns how many were answered.
    pub(crate) fn answer_pings(&mut self) -> Result<usize, String> {
        self.read_events()?;
        let atoms = self.atoms;
        let pings: Vec<ClientMessageEvent> = self
            .events
            .iter()
            .filter_map(|event| match event {
                Event::ClientMessage(message)
                    if message.type_ == atoms.WM_PROTOCOLS
                        && message.data.as_data32()[0] == atoms._NET_WM_PING =>
                {
                    Some(*message)
                }
                _ => None,
            })
            .collect();
        self.events.retain(|event| {
            !matches!(event, Event::ClientMessage(message)
                if message.type_ == atoms.WM_PROTOCOLS
                    && message.data.as_data32()[0] == atoms._NET_WM_PING)
        });
        for ping in &pings {
            let mut answer = *ping;
            answer.window = self.root;
            self.connection
                .send_event(
                    false,
                    self.root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    answer,
                )
                .map_err(|e| e.to_string())?;
        }
        self.sync()?;
        Ok(pings.len())
    }

    /// Sets the mode of the output at X11 position (`x`, `y`) to `width` × `height`, through
    /// RandR, as a game does: Xwayland emulates it for this client alone.
    pub(crate) fn set_mode(&self, x: i32, y: i32, width: u16, height: u16) -> Result<(), String> {
        let connection = &self.connection;
        let fail = |e: x11rb::errors::ReplyError| e.to_string();
        let send = |e: x11rb::errors::ConnectionError| e.to_string();
        connection
            .randr_query_version(1, 6)
            .map_err(send)?
            .reply()
            .map_err(fail)?;
        let resources = connection
            .randr_get_screen_resources_current(self.root)
            .map_err(send)?
            .reply()
            .map_err(fail)?;
        for &crtc in &resources.crtcs {
            let info = connection
                .randr_get_crtc_info(crtc, resources.config_timestamp)
                .map_err(send)?
                .reply()
                .map_err(fail)?;
            if (i32::from(info.x), i32::from(info.y)) != (x, y) || info.outputs.is_empty() {
                continue;
            }
            let output = connection
                .randr_get_output_info(info.outputs[0], resources.config_timestamp)
                .map_err(send)?
                .reply()
                .map_err(fail)?;
            let mode = resources
                .modes
                .iter()
                .find(|mode| {
                    mode.width == width && mode.height == height && output.modes.contains(&mode.id)
                })
                .ok_or_else(|| {
                    format!("the output at ({x}, {y}) has no mode {width} × {height}")
                })?;
            connection
                .randr_set_crtc_config(
                    crtc,
                    x11rb::CURRENT_TIME,
                    resources.config_timestamp,
                    info.x,
                    info.y,
                    mode.id,
                    info.rotation,
                    &info.outputs,
                )
                .map_err(send)?
                .reply()
                .map_err(fail)?;
            return Ok(());
        }
        Err(format!("no output at ({x}, {y})"))
    }

    /// Reads every event that came, into the client's queue.
    fn read_events(&mut self) -> Result<(), String> {
        while let Some(event) = self
            .connection
            .poll_for_event()
            .map_err(|e| e.to_string())?
        {
            self.events.push_back(event);
        }
        Ok(())
    }

    /// Waits for the first event `wanted` takes, among those that came and those that come,
    /// for up to [`PATIENCE`]; the others stay queued.
    pub(crate) fn wait_for(
        &mut self,
        what: &str,
        mut wanted: impl FnMut(&Event) -> bool,
    ) -> Result<Event, String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.read_events()?;
            if let Some(index) = self.events.iter().position(&mut wanted) {
                return Ok(self.events.remove(index).expect("an event at its index"));
            }
            if Instant::now() > deadline {
                return Err(format!("{what}: not within {PATIENCE:?}"));
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// The first event that came and was not taken yet, without waiting; none when none came:
    /// for a client that runs its own loop, as a toolkit does.
    pub(crate) fn next_event(&mut self) -> Result<Option<Event>, String> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        self.connection.poll_for_event().map_err(|e| e.to_string())
    }

    /// Every event that came until X11 handled the requests sent so far, taken from the queue.
    pub(crate) fn take_events(&mut self) -> Result<Vec<Event>, String> {
        self.sync()?;
        self.read_events()?;
        Ok(self.events.drain(..).collect())
    }
}

/// Runs `work` on `client` on a thread of its own while the compositor's loop runs, then gives
/// the client back with what `work` returned. Fails the story if `work` failed or panicked.
pub(crate) fn step<T: Send + 'static>(
    compositor: &mut Compositor,
    client: X11Client,
    work: impl FnOnce(&mut X11Client) -> Result<T, String> + Send + 'static,
) -> (X11Client, T) {
    step_pumping(compositor, client, crate::x11_windows::pump, work)
}

/// [`step`], the compositor's loop run by `pump` meanwhile: a story that holds the manual clock
/// back runs it its own way.
pub(crate) fn step_pumping<T: Send + 'static>(
    compositor: &mut Compositor,
    mut client: X11Client,
    mut pump: impl FnMut(&mut Compositor),
    work: impl FnOnce(&mut X11Client) -> Result<T, String> + Send + 'static,
) -> (X11Client, T) {
    let worker = thread::spawn(move || {
        let result = work(&mut client);
        (client, result)
    });
    let deadline = Instant::now() + PATIENCE + Duration::from_secs(5);
    while !worker.is_finished() {
        assert!(Instant::now() < deadline, "an X11 step never ended");
        pump(compositor);
    }
    match worker.join() {
        Ok((client, Ok(value))) => (client, value),
        Ok((_, Err(error))) => panic!("an X11 step failed: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// Connects a client to `display`, while the compositor's loop starts and serves its Xwayland.
pub(crate) fn connect(compositor: &mut Compositor, display: &X11Display) -> X11Client {
    let display = display.clone();
    let worker = thread::spawn(move || X11Client::connect(&display));
    let deadline = Instant::now() + PATIENCE;
    while !worker.is_finished() {
        assert!(Instant::now() < deadline, "an X11 client was never let in");
        crate::x11_windows::pump(compositor);
    }
    match worker.join() {
        Ok(Ok(client)) => client,
        Ok(Err(error)) => panic!("an X11 client cannot connect: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
