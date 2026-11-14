//! Yuna's X11 window manager, one per Xwayland, on a thread of its own
//! ([ADR-0066](../../../../docs/adr/0066-yunas-own-x11-window-manager-on-x11rb.md)): the thread
//! waits on X11's replies, so that Yuna's loop never does.
//!
//! It connects through Xwayland's `-wm` socket once `-displayfd` has said Xwayland is ready, then
//! takes charge in an order that lets no X11 client read a setting before it exists
//! ([ADR-0067](../../../../docs/adr/0067-one-xwayland-per-seat-on-demand-behind-a-cookie.md)):
//!
//! 1. XFixes' disconnection mode, *terminate*, for its own connection: Xwayland's `-terminate`
//!    fires only once every client left has asked it, the manager included.
//! 2. `RESOURCE_MANAGER` on the root, `Xft.dpi` at 96 × S, S being the scale Xwayland's client
//!    was given ([ADR-0068](../../../../docs/adr/0068-x11-windows-drawn-by-yuna-through-an-area-filter.md)).
//! 3. XSETTINGS: `Xft/DPI` with it, and a window scale of 1, since X11 programs draw in X11
//!    pixels.
//! 4. Composite's manual redirection of the root's windows, and SubstructureRedirect on the root:
//!    Yuna draws and places X11 windows.
//! 5. `_NET_SUPPORTED` and `_NET_SUPPORTING_WM_CHECK`.
//! 6. `WM_S0`, last: Xwayland listens on its sockets only from then on, so the first client
//!    finds everything above in place.
//!
//! Then it reads X11's events and posts what Yuna decides on to Yuna's loop, through a calloop
//! channel, [`ManagerEvent`]; Yuna's [`Command`]s come back through a channel the thread reads
//! when its wake-up socket says so. Windows are not managed yet: a window asking to be mapped is
//! mapped as it asks, so that its program does not wait for ever, until Yuna places X11
//! windows; every other event is logged and left.
//!
//! The thread ends when Xwayland closes the connection, when Yuna lets the commands go, or on an
//! error; its channel's end tells Yuna, which then ends that Xwayland, even when the thread
//! panicked.

use std::io;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;

use smithay::reexports::calloop::channel::SyncSender;
use smithay::reexports::rustix;
use smithay::reexports::rustix::event::{PollFd, PollFlags};
use smithay::reexports::rustix::io::Errno;
use smithay::reexports::rustix::net::{RecvFlags, SendFlags};
use x11rb::connection::Connection;
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::Event;
use x11rb::protocol::composite::{ConnectionExt as _, Redirect};
use x11rb::protocol::xfixes::{ClientDisconnectFlags, ConnectionExt as _};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _,
    CreateWindowAux, EventMask, PropMode, Window, WindowClass,
};
use x11rb::rust_connection::{DefaultStream, RustConnection};
use x11rb::wrapper::ConnectionExt as _;
use yuna_layout::SeatId;

/// How many of the manager's events wait for Yuna's loop before the thread waits in turn: X11's
/// own buffers then hold what comes, rather than Yuna's memory.
pub(crate) const EVENT_QUEUE: usize = 256;

/// The DPI of an X11 program at scale 1, as X11 and its toolkits count it.
const BASE_DPI: f64 = 96.0;

/// What the manager's thread posts to Yuna's loop.
#[derive(Debug)]
pub(crate) enum ManagerEvent {
    /// It took charge: `WM_S0` is its, and X11 clients are let in.
    Ready,
    /// An X11 client asks for its window to be mapped.
    MapRequest(Window),
    /// The thread ends, for this reason.
    Stopped(Stop),
}

/// Why the manager's thread ended.
#[derive(Debug)]
pub(crate) enum Stop {
    /// Xwayland closed the connection: it is ending.
    Closed,
    /// Yuna let it go.
    Released,
    /// Something failed; its reason.
    Failed(String),
}

/// What Yuna's loop asks of the manager.
#[derive(Debug)]
pub(crate) enum Command {
    /// Maps a window that asked to be: until Yuna places X11 windows, each is mapped as its
    /// program asks.
    Map(Window),
}

/// Yuna's side of a manager: its commands, and the socket that wakes its thread for them. When it
/// drops, the thread ends.
#[derive(Debug)]
pub(crate) struct Commands {
    sender: mpsc::Sender<Command>,
    wake: UnixStream,
}

impl Commands {
    /// Passes `command` to the manager's thread and wakes it. Never waits: a thread that has
    /// ended loses it, and Yuna learns of its end through the events' channel.
    pub(crate) fn send(&self, command: Command) {
        if self.sender.send(command).is_ok() {
            // A full socket wakes the thread all the same; NOSIGNAL, so that a thread gone is
            // no SIGPIPE.
            let _ = rustix::net::send(&self.wake, &[0], SendFlags::DONTWAIT | SendFlags::NOSIGNAL);
        }
    }
}

/// Why the manager stopped: an X11 error, an I/O error, or what it found wrong.
type Failure = Box<dyn std::error::Error + Send + Sync>;

x11rb::atom_manager! {
    /// The atoms the manager names.
    Atoms: AtomsCookie {
        WM_S0,
        MANAGER,
        UTF8_STRING,
        _NET_SUPPORTED,
        _NET_SUPPORTING_WM_CHECK,
        _NET_WM_NAME,
        _XSETTINGS_S0,
        _XSETTINGS_SETTINGS,
    }
}

/// Starts the manager of seat `seat`'s Xwayland on a thread of its own, on `wm`, Xwayland's
/// `-wm` socket, for a scale of `scale`. Its events go to `events`.
pub(crate) fn start(
    seat: SeatId,
    wm: UnixStream,
    scale: f64,
    events: SyncSender<ManagerEvent>,
) -> io::Result<Commands> {
    let (sender, commands) = mpsc::channel();
    let (wake, woken) = UnixStream::pair()?;
    wake.set_nonblocking(true)?;
    woken.set_nonblocking(true)?;
    std::thread::Builder::new()
        .name(format!("yuna-x11-{}", seat.0))
        .spawn(move || {
            let stop = match run(wm, scale, &events, &commands, &woken) {
                Ok(()) => Stop::Released,
                Err(failure) if is_closed(&*failure) => Stop::Closed,
                Err(failure) => Stop::Failed(failure.to_string()),
            };
            let _ = events.send(ManagerEvent::Stopped(stop));
        })?;
    Ok(Commands { sender, wake })
}

/// Whether `failure` is Xwayland closing the connection.
fn is_closed(failure: &(dyn std::error::Error + 'static)) -> bool {
    let connection = failure.downcast_ref::<ConnectionError>().or_else(|| {
        match failure.downcast_ref::<ReplyError>() {
            Some(ReplyError::ConnectionError(error)) => Some(error),
            _ => None,
        }
    });
    matches!(connection, Some(ConnectionError::IoError(error))
        if matches!(error.kind(), io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe))
}

/// The thread's work: connects, takes charge, says so, then serves until the end.
fn run(
    wm: UnixStream,
    scale: f64,
    events: &SyncSender<ManagerEvent>,
    commands: &mpsc::Receiver<Command>,
    woken: &UnixStream,
) -> Result<(), Failure> {
    let (stream, _) = DefaultStream::from_unix_stream(wm)?;
    // Xwayland has a single screen.
    let connection = RustConnection::connect_to_stream(stream, 0)?;
    take_charge(&connection, scale)?;
    if events.send(ManagerEvent::Ready).is_err() {
        return Ok(());
    }
    serve(&connection, events, commands, woken)
}

/// Takes charge of Xwayland's root, in the order the module's documentation gives, and checks
/// that `WM_S0` is the manager's.
fn take_charge(connection: &RustConnection, scale: f64) -> Result<(), Failure> {
    let atoms = Atoms::new(connection)?.reply()?;
    let root = connection
        .setup()
        .roots
        .first()
        .ok_or("Xwayland's setup lists no screen")?
        .root;

    // 6 is the version of `SetClientDisconnectMode`; XFixes refuses a request above the version
    // its client asked.
    let xfixes = connection.xfixes_query_version(6, 0)?.reply()?;
    if xfixes.major_version < 6 {
        return Err(format!(
            "Xwayland's XFixes is version {}: `-terminate` needs 6",
            xfixes.major_version
        )
        .into());
    }
    connection
        .xfixes_set_client_disconnect_mode(ClientDisconnectFlags::TERMINATE)?
        .check()?;

    connection.change_property8(
        PropMode::REPLACE,
        root,
        AtomEnum::RESOURCE_MANAGER,
        AtomEnum::STRING,
        resources(scale).as_bytes(),
    )?;

    // One window owns the manager's selections and is its check window; it is never mapped.
    let window = connection.generate_id()?;
    connection.create_window(
        0,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new(),
    )?;
    connection.change_property8(
        PropMode::REPLACE,
        window,
        atoms._XSETTINGS_SETTINGS,
        atoms._XSETTINGS_SETTINGS,
        &xsettings(0, &settings(scale)),
    )?;
    take_selection(connection, root, window, atoms._XSETTINGS_S0, &atoms)?;

    connection.composite_query_version(0, 4)?.reply()?;
    connection
        .composite_redirect_subwindows(root, Redirect::MANUAL)?
        .check()?;
    connection
        .change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY),
        )?
        .check()?;

    // What this manager supports so far: the check window alone.
    connection.change_property32(
        PropMode::REPLACE,
        root,
        atoms._NET_SUPPORTED,
        AtomEnum::ATOM,
        &[atoms._NET_SUPPORTING_WM_CHECK],
    )?;
    for on in [root, window] {
        connection.change_property32(
            PropMode::REPLACE,
            on,
            atoms._NET_SUPPORTING_WM_CHECK,
            AtomEnum::WINDOW,
            &[window],
        )?;
    }
    connection.change_property8(
        PropMode::REPLACE,
        window,
        atoms._NET_WM_NAME,
        atoms.UTF8_STRING,
        b"Yuna",
    )?;

    take_selection(connection, root, window, atoms.WM_S0, &atoms)?;
    // A reply after every request above: each was handled, and the selection is the manager's.
    let owner = connection.get_selection_owner(atoms.WM_S0)?.reply()?.owner;
    if owner != window {
        return Err(format!("WM_S0 belongs to window {owner}, not the manager's").into());
    }
    Ok(())
}

/// Makes `window` the owner of `selection` and tells the root's listeners, as ICCCM asks of a
/// manager (its section 2.8).
fn take_selection(
    connection: &RustConnection,
    root: Window,
    window: Window,
    selection: Atom,
    atoms: &Atoms,
) -> Result<(), Failure> {
    connection.set_selection_owner(window, selection, x11rb::CURRENT_TIME)?;
    let announcement = ClientMessageEvent::new(
        32,
        root,
        atoms.MANAGER,
        [x11rb::CURRENT_TIME, selection, window, 0, 0],
    );
    connection.send_event(false, root, EventMask::STRUCTURE_NOTIFY, announcement)?;
    Ok(())
}

/// Reads X11's events and Yuna's commands until Xwayland closes the connection or Yuna lets the
/// commands go. Sleeps in `poll` between them: nothing is polled on a timer.
fn serve(
    connection: &RustConnection,
    events: &SyncSender<ManagerEvent>,
    commands: &mpsc::Receiver<Command>,
    woken: &UnixStream,
) -> Result<(), Failure> {
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Map(window)) => {
                    connection.map_window(window)?;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        // Flushed before the events are read: a flush that would block reads X11's packets into
        // x11rb's own queue, where `poll` would not see them, so that an event could wait for
        // unrelated traffic. Read after it, the queue and the socket are both empty at the sleep.
        connection.flush()?;
        // Every event read so far, the replies' waits and the flush included.
        while let Some(event) = connection.poll_for_event()? {
            match event {
                Event::MapRequest(request) => {
                    if events
                        .send(ManagerEvent::MapRequest(request.window))
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                Event::Error(error) => {
                    tracing::debug!(?error, "an X11 request of the manager failed");
                }
                event => tracing::trace!(?event, "an X11 event the manager leaves"),
            }
        }
        let mut sleep = [
            PollFd::new(connection.stream(), PollFlags::IN),
            PollFd::new(woken, PollFlags::IN),
        ];
        match rustix::event::poll(&mut sleep, None) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
        if !drain(woken)? {
            return Ok(());
        }
    }
}

/// Reads every byte waiting on the wake-up socket; false once Yuna's end has closed.
fn drain(woken: &UnixStream) -> Result<bool, Failure> {
    let mut bytes = [0; 64];
    loop {
        match rustix::net::recv(woken.as_fd(), &mut bytes, RecvFlags::DONTWAIT) {
            Ok((0, _)) => return Ok(false),
            Ok(_) => {}
            Err(Errno::AGAIN) => return Ok(true),
            Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// X11's DPI at scale `scale`, rounded.
fn dpi(scale: f64) -> i32 {
    (BASE_DPI * scale).round() as i32
}

/// The root's `RESOURCE_MANAGER` at scale `scale`: `Xft.dpi`, which Xlib and Xft programs read
/// for their fonts and Qt for its scale.
fn resources(scale: f64) -> String {
    format!("Xft.dpi:\t{}\n", dpi(scale))
}

/// The XSETTINGS at scale `scale`: `Xft/DPI` in 1024ths of a DPI, as `RESOURCE_MANAGER` gives
/// it; a window scale of 1, since an X11 program draws in X11 pixels, which S already makes the
/// right size; and the DPI at scale 1, which GTK divides by its window scale when one is forced.
fn settings(scale: f64) -> [(&'static str, i32); 3] {
    [
        ("Xft/DPI", (BASE_DPI * scale * 1024.0).round() as i32),
        ("Gdk/WindowScalingFactor", 1),
        ("Gdk/UnscaledDPI", (BASE_DPI * 1024.0) as i32),
    ]
}

/// The XSETTINGS property holding integer `settings`, in the encoding of the XSETTINGS
/// specification, 0.5: little-endian, its byte order first; then the serial and the count; then
/// each setting's type (0, an integer), its name padded to four bytes, the serial it last changed
/// at, 0, and its value.
fn xsettings(serial: u32, settings: &[(&str, i32)]) -> Vec<u8> {
    const LITTLE_ENDIAN: u8 = 0;
    const INTEGER: u8 = 0;
    let mut blob = vec![LITTLE_ENDIAN, 0, 0, 0];
    blob.extend_from_slice(&serial.to_le_bytes());
    // A few settings, named by Yuna: far below either bound.
    let count = u32::try_from(settings.len()).unwrap_or(u32::MAX);
    blob.extend_from_slice(&count.to_le_bytes());
    for (name, value) in settings {
        blob.extend_from_slice(&[INTEGER, 0]);
        let length = u16::try_from(name.len()).unwrap_or(u16::MAX);
        blob.extend_from_slice(&length.to_le_bytes());
        blob.extend_from_slice(name.as_bytes());
        blob.resize(blob.len().next_multiple_of(4), 0);
        blob.extend_from_slice(&0_u32.to_le_bytes());
        blob.extend_from_slice(&value.to_le_bytes());
    }
    blob
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_xsettings_blob_is_little_endian_with_each_name_padded() {
        let blob = xsettings(7, &[("Xft/DPI", 147_456), ("Gdk/WindowScalingFactor", 1)]);
        let mut expected = vec![0, 0, 0, 0, 7, 0, 0, 0, 2, 0, 0, 0];
        // "Xft/DPI": 7 bytes, padded with one.
        expected.extend_from_slice(&[0, 0, 7, 0]);
        expected.extend_from_slice(b"Xft/DPI\0");
        expected.extend_from_slice(&[0, 0, 0, 0]);
        expected.extend_from_slice(&147_456_i32.to_le_bytes());
        // "Gdk/WindowScalingFactor": 23 bytes, padded with one.
        expected.extend_from_slice(&[0, 0, 23, 0]);
        expected.extend_from_slice(b"Gdk/WindowScalingFactor\0");
        expected.extend_from_slice(&[0, 0, 0, 0]);
        expected.extend_from_slice(&[1, 0, 0, 0]);
        assert_eq!(blob, expected);
    }

    #[test]
    fn a_name_of_four_bytes_takes_no_padding() {
        let blob = xsettings(0, &[("Net/", -1)]);
        assert_eq!(&blob[12..16], &[0, 0, 4, 0]);
        assert_eq!(&blob[16..20], b"Net/");
        assert_eq!(&blob[20..24], &[0, 0, 0, 0]);
        assert_eq!(&blob[24..], &[0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn the_dpi_follows_the_scale() {
        assert_eq!(resources(1.5), "Xft.dpi:\t144\n");
        assert_eq!(resources(1.0), "Xft.dpi:\t96\n");
        // 96 × 1.75 = 168; 96 × 1.1 = 105.6, rounded.
        assert_eq!(resources(1.75), "Xft.dpi:\t168\n");
        assert_eq!(resources(1.1), "Xft.dpi:\t106\n");
        assert_eq!(
            settings(1.5),
            [
                ("Xft/DPI", 147_456),
                ("Gdk/WindowScalingFactor", 1),
                ("Gdk/UnscaledDPI", 98_304)
            ]
        );
        // 96 × 1.1 × 1024 = 108,134.4.
        assert_eq!(settings(1.1)[0], ("Xft/DPI", 108_134));
    }
}
